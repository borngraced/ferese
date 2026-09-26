mod config;
mod control;
mod motion;
mod status;
mod status_ui;

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

// Older libcosmic backends expose native surfaces in frame events. Keep
// that compatibility path gated; current backends use Window::Opened and
// window::run to retrieve handles without updates at the display refresh rate.
static EFFECT_FRAME_PENDING: AtomicBool = AtomicBool::new(true);

use cosmic::Element;
use cosmic::app::{Core, Settings, Task};
use cosmic::iced::alignment;
use cosmic::iced::event::{self, PlatformSpecific, wayland};
use cosmic::iced::platform_specific::{
    runtime::wayland::layer_surface::{IcedMargin, IcedOutput, SctkLayerSurfaceSettings},
    shell::commands::layer_surface::set_input_zone,
    shell::commands::layer_surface::{Anchor, KeyboardInteractivity, Layer},
};
use cosmic::iced::{
    Background, Border, Color, ContentFit, Event, Length, Limits, Subscription, window,
};
use cosmic::theme;
use cosmic::widget::{button, container, icon, image, row};
use ferese_protocols::effects::v1::client::{
    ferese_effects_manager_v1::FereseEffectsManagerV1,
    ferese_surface_effects_v1::{self, FereseSurfaceEffectsV1},
};
use jiff::Zoned;
use wayland_client::{
    Connection, Dispatch, EventQueue, Proxy, QueueHandle, delegate_noop,
    globals::{GlobalListContents, registry_queue_init},
    protocol::{wl_registry, wl_surface},
};

use crate::config::{ShellConfig, ShellTheme, WallpaperMode};
use crate::control::{ShellControl, ShellSnapshot};

const APP_ID: &str = "dev.ferese.Shell";

static SHELL_FONT: std::sync::OnceLock<cosmic::font::Font> = std::sync::OnceLock::new();

fn configured_font(family: Option<&str>) -> cosmic::font::Font {
    family.map_or_else(cosmic::font::default, |family| {
        let family = Box::leak(family.to_owned().into_boxed_str());
        cosmic::font::Font::with_name(family)
    })
}

// COSMIC's text helper explicitly selects its interface font, overriding
// Settings::default_font. Use this helper for every bar and menu label.
fn text<'a>(
    content: impl Into<std::borrow::Cow<'a, str>> + 'a,
) -> cosmic::widget::Text<'a, cosmic::Theme, cosmic::Renderer> {
    cosmic::widget::text(content).font(*SHELL_FONT.get_or_init(cosmic::font::default))
}

#[derive(Clone, Copy)]
struct BarMetrics {
    height: f32,
    text_size: u16,
    icon_size: u16,
    overview_icon_size: u16,
    control_height: f32,
    workspace_hit_width: f32,
    active_marker_width: f32,
    active_marker_height: f32,
    dot_marker_size: f32,
    empty_marker_size: f32,
}

impl From<ShellTheme> for BarMetrics {
    fn from(theme: ShellTheme) -> Self {
        let height = theme.bar_height;

        Self {
            height,
            // Logical sizes: Wayland/iced applies each surface's output scale.
            // Compacting the bar must not also shrink its readable content.
            text_size: 13,
            icon_size: 19,
            overview_icon_size: 19,
            control_height: (height - 4.0).max(21.0).min(height),
            workspace_hit_width: 22.0,
            active_marker_width: 14.0,
            active_marker_height: 4.0,
            dot_marker_size: 4.0,
            empty_marker_size: 3.0,
        }
    }
}

fn main() -> cosmic::iced::Result {
    // Small shell surfaces do not need a second GPU device and shader setup.
    // Keep wgpu as a fallback and honor an explicit renderer preference.
    if std::env::var_os("ICED_BACKEND").is_none() {
        // SAFETY: this is the process entry point, before any worker or
        // toolkit threads are started.
        unsafe { std::env::set_var("ICED_BACKEND", "tiny-skia,wgpu") };
    }
    let config = config::load();
    let shell_font = configured_font(config.font_family.as_deref());
    let _ = SHELL_FONT.set(shell_font);
    // Decode alongside toolkit/GPU initialization, never during a UI draw.
    let wallpaper = config.wallpaper.path.clone().map(|path| {
        let (sender, receiver) = cosmic::iced::futures::channel::oneshot::channel();
        std::thread::spawn(move || {
            let result =
                cosmic::iced::advanced::graphics::image::load(&image::Handle::from_path(path))
                    .map(|pixels| {
                        image::Handle::from_rgba(pixels.width(), pixels.height(), pixels.into_raw())
                    })
                    .map_err(|error| format!("{error:?}"));
            let _ = sender.send(result);
        });
        receiver
    });
    let mut settings = Settings::default()
        .no_main_window(true)
        .client_decorations(false)
        .transparent(true)
        .is_daemon(true);
    settings = settings.default_font(shell_font);

    cosmic::app::run::<FereseShell>(settings, (config, wallpaper))
}

type WallpaperLoad =
    cosmic::iced::futures::channel::oneshot::Receiver<Result<image::Handle, String>>;

struct FereseShell {
    core: Core,
    bar_surface_id: window::Id,
    config: ShellConfig,
    wallpaper: Option<image::Handle>,
    control: Option<ShellControl>,
    snapshot: ShellSnapshot,
    overview_active: bool,
    clock: String,
    effects: Option<EffectsBinding>,
    status_service: status::Service,
    status: status::Snapshot,
    status_error: Option<String>,
    menu: Option<status_ui::OpenMenu>,
}

#[derive(Clone, Debug)]
enum Message {
    WallpaperLoaded(Result<image::Handle, String>),
    Event(Event, window::Id),
    NativeSurface(
        window::Id,
        Result<(Connection, wl_surface::WlSurface), String>,
    ),
    Tick,
    ActivateWorkspace(u64),
    ActivateWindow(u64),
    ToggleOverview,
    StatusTick,
    AnimateMenu,
    OpenMenu(status_ui::Menu, cosmic::iced::Rectangle<i32>),
    Control(status::Action),
    ConfirmPower(status::Action),
    CancelPower,
}

impl cosmic::Application for FereseShell {
    type Executor = cosmic::executor::Default;
    type Flags = (ShellConfig, Option<WallpaperLoad>);
    type Message = Message;

    const APP_ID: &'static str = APP_ID;

    fn core(&self) -> &Core {
        &self.core
    }

    fn core_mut(&mut self) -> &mut Core {
        &mut self.core
    }

    fn init(core: Core, (config, wallpaper): Self::Flags) -> (Self, Task<Self::Message>) {
        let bar_surface_id = window::Id::unique();
        let wallpaper_surface_id = window::Id::unique();
        let shell_theme = config.theme;
        let bar = BarMetrics::from(shell_theme);
        let app = Self {
            core,
            bar_surface_id,
            status_service: status::Service::start(config.status.settings_command.clone()),
            status: status::Snapshot::default(),
            status_error: None,
            menu: None,
            config,
            wallpaper: None,
            control: ShellControl::connect()
                .map_err(|error| {
                    eprintln!("ferese-shell: shell control unavailable: {error}");
                })
                .ok(),
            snapshot: ShellSnapshot::default(),
            overview_active: false,
            clock: current_time(),
            effects: None,
        };
        let wallpaper_action = cosmic::surface::action::app_layer_shell::<Self>(
            |_| Default::default(),
            move |_| SctkLayerSurfaceSettings {
                id: wallpaper_surface_id,
                layer: Layer::Background,
                keyboard_interactivity: KeyboardInteractivity::None,
                input_zone: Some(Vec::new()),
                anchor: Anchor::TOP | Anchor::RIGHT | Anchor::BOTTOM | Anchor::LEFT,
                output: IcedOutput::Active,
                namespace: "ferese-shell-wallpaper".to_owned(),
                // Wallpaper covers the full output, including beneath the bar
                // and its floating margins. Zero would use the remaining workspace.
                exclusive_zone: -1,
                size: Some((None, None)),
                size_limits: Limits::NONE,
                ..Default::default()
            },
            Some(Box::new(Self::view_wallpaper)),
        );
        let bar_action = cosmic::surface::action::app_layer_shell::<Self>(
            |_| Default::default(),
            move |_| SctkLayerSurfaceSettings {
                id: bar_surface_id,
                layer: Layer::Top,
                keyboard_interactivity: KeyboardInteractivity::None,
                anchor: Anchor::TOP | Anchor::LEFT | Anchor::RIGHT,
                output: IcedOutput::Active,
                namespace: "ferese-shell-top-bar".to_owned(),
                margin: IcedMargin {
                    top: shell_theme.bar_margin_top,
                    right: shell_theme.bar_margin_horizontal,
                    bottom: 0,
                    left: shell_theme.bar_margin_horizontal,
                },
                size: Some((None, Some(bar.height.round() as u32))),
                size_limits: Limits::NONE,
                // Reserve breathing room below the visible bar; layer-shell
                // accounts for the top margin separately.
                exclusive_zone: (bar.height.round() as i32)
                    .saturating_add(shell_theme.bar_window_gap),
                ..Default::default()
            },
            Some(Box::new(Self::view_layer)),
        );

        // Submit the small interactive surface before the full-screen image.
        let tasks = [bar_action, wallpaper_action]
            .map(cosmic::Action::Surface)
            .map(cosmic::task::message);

        let wallpaper_task = match wallpaper {
            Some(receiver) => cosmic::task::future(async move {
                cosmic::Action::App(Message::WallpaperLoaded(
                    receiver
                        .await
                        .unwrap_or_else(|error| Err(error.to_string())),
                ))
            }),
            None => Task::none(),
        };
        (app, Task::batch([Task::batch(tasks), wallpaper_task]))
    }

    fn subscription(&self) -> Subscription<Self::Message> {
        Subscription::batch([
            event::listen_with(|event, _status, id| match &event {
                Event::PlatformSpecific(PlatformSpecific::Wayland(wayland::Event::Frame(..))) => {
                    EFFECT_FRAME_PENDING
                        .load(Ordering::Relaxed)
                        .then_some(Message::Event(event, id))
                }
                Event::Keyboard(_)
                | Event::Window(window::Event::Opened { .. } | window::Event::Closed)
                | Event::PlatformSpecific(PlatformSpecific::Wayland(
                    wayland::Event::Popup(..) | wayland::Event::Layer(..),
                )) => Some(Message::Event(event, id)),
                _ => None,
            }),
            cosmic::iced::time::every(Duration::from_millis(500)).map(|_| Message::Tick),
            cosmic::iced::time::every(Duration::from_millis(250)).map(|_| Message::StatusTick),
            if self.menu.as_ref().is_some_and(|menu| menu.animating()) {
                cosmic::iced::time::every(Duration::from_millis(16)).map(|_| Message::AnimateMenu)
            } else {
                Subscription::none()
            },
        ])
    }

    fn update(&mut self, message: Self::Message) -> Task<Self::Message> {
        if let Some(menu) = &self.menu
            && let Some(effects) = &menu.effects
        {
            let regions = menu.regions.lock().unwrap().clone();
            if let Err(error) = effects.set_regions(&regions) {
                eprintln!("ferese-shell: could not update card materials: {error}");
            }
        }
        match message {
            Message::WallpaperLoaded(result) => {
                match result {
                    Ok(handle) => self.wallpaper = Some(handle),
                    Err(error) => eprintln!("ferese-shell: wallpaper unavailable: {error}"),
                }
                Task::none()
            }
            Message::NativeSurface(id, result) => {
                match result {
                    Ok((_connection, surface)) if id == self.bar_surface_id => {
                        self.attach_effects(&surface)
                    }
                    Ok((_connection, surface)) => {
                        if let Some(menu) = &mut self.menu
                            && menu.id == id
                            && menu.effects.is_none()
                        {
                            match EffectsBinding::attach_role(&surface, menu.kind.material_role()) {
                                Ok(binding) => menu.effects = Some(binding),
                                Err(error) => {
                                    eprintln!("ferese-shell: popover material unavailable: {error}")
                                }
                            }
                        }
                    }
                    Err(error) => eprintln!("ferese-shell: native surface unavailable: {error}"),
                }
                Task::none()
            }
            Message::StatusTick => {
                if let Some(update) = self.status_service.poll()
                    && update.generation >= self.status_service.generation
                {
                    self.status = update.snapshot;
                    self.status_error = update.error;
                }
                Task::none()
            }
            Message::AnimateMenu => Task::none(),
            Message::OpenMenu(kind, anchor) => self.open_menu(kind, anchor),
            Message::ConfirmPower(action) => {
                if let Some(menu) = &mut self.menu {
                    menu.confirm = Some(action);
                }
                Task::none()
            }
            Message::CancelPower => {
                if let Some(menu) = &mut self.menu {
                    menu.confirm = None;
                }
                Task::none()
            }
            Message::Control(action) => {
                self.status_error = self.status_service.send(action.clone()).err();
                if self.status_error.is_none() {
                    self.optimistic_status(&action);
                }
                if matches!(
                    action,
                    status::Action::Notifications | status::Action::Settings
                ) {
                    return self.destroy_menu();
                }
                Task::none()
            }
            Message::Event(event, id) => self.handle_event(event, id),
            Message::Tick => {
                self.clock = current_time();
                if let Some(control) = &self.control {
                    let poll = control.poll();

                    if poll.disconnected {
                        return cosmic::iced::exit();
                    }
                    if let Some(active) = poll.overview_active {
                        self.overview_active = active;
                    }
                    if let Some(snapshot) = poll.snapshot {
                        let was_hidden = bar_hidden(&self.snapshot);

                        self.snapshot = snapshot;

                        let hidden = bar_hidden(&self.snapshot);
                        if hidden != was_hidden {
                            if let Some(effects) = &self.effects
                                && let Err(error) = effects.set_visible(!hidden)
                            {
                                eprintln!("ferese-shell: could not update panel material: {error}");
                            }

                            let input_zone = if hidden { Some(Vec::new()) } else { None };

                            return Task::batch([
                                set_input_zone(self.bar_surface_id, input_zone),
                                self.destroy_menu(),
                            ]);
                        }
                    }
                }
                Task::none()
            }
            Message::ActivateWindow(id) => {
                if let Some(control) = &self.control {
                    control.activate_window(id);
                }
                Task::none()
            }
            Message::ActivateWorkspace(id) => {
                if let Some(control) = &self.control {
                    control.activate_workspace(id);
                }
                Task::none()
            }
            Message::ToggleOverview => {
                self.overview_active = !self.overview_active;
                if let Some(control) = &self.control {
                    control.set_overview_active(self.overview_active);
                }
                Task::none()
            }
        }
    }

    fn style(&self) -> Option<cosmic::iced::theme::Style> {
        // Shell surfaces own their backgrounds. Never inherit an opaque
        // application clear color during startup or a system-theme change.
        Some(shell_surface_style(self.config.theme))
    }

    fn view(&self) -> Element<'_, Self::Message> {
        text("").into()
    }

    fn view_window(&self, _id: window::Id) -> Element<'_, Self::Message> {
        text("").into()
    }
}

impl FereseShell {
    fn handle_event(&mut self, event: Event, id: window::Id) -> Task<Message> {
        match event {
            Event::Window(window::Event::Opened { .. })
                if id == self.bar_surface_id
                    || self.menu.as_ref().is_some_and(|menu| menu.id == id) =>
            {
                window::run(id, native_wayland_surface)
                    .map(move |surface| cosmic::Action::App(Message::NativeSurface(id, surface)))
            }
            Event::Keyboard(cosmic::iced::keyboard::Event::KeyPressed {
                key: cosmic::iced::keyboard::Key::Named(cosmic::iced::keyboard::key::Named::Escape),
                ..
            }) if self.menu.is_some() => self.destroy_menu(),
            Event::PlatformSpecific(PlatformSpecific::Wayland(wayland::Event::Popup(
                event,
                surface,
                popup_id,
            ))) => {
                if let Some(menu) = &mut self.menu
                    && menu.id == popup_id
                {
                    match event {
                        wayland::PopupEvent::Done => {
                            self.menu = None;
                            EFFECT_FRAME_PENDING.store(false, Ordering::Relaxed);
                        }
                        wayland::PopupEvent::Focused => {
                            if menu.effects.is_none() {
                                match EffectsBinding::attach_role(
                                    &surface,
                                    menu.kind.material_role(),
                                ) {
                                    Ok(binding) => menu.effects = Some(binding),
                                    Err(error) => eprintln!(
                                        "ferese-shell: popover material unavailable: {error}"
                                    ),
                                }
                                EFFECT_FRAME_PENDING.store(false, Ordering::Relaxed);
                            }
                        }
                        wayland::PopupEvent::Configured { .. } => {
                            if menu.effects.is_none() {
                                match EffectsBinding::attach_role(
                                    &surface,
                                    menu.kind.material_role(),
                                ) {
                                    Ok(binding) => menu.effects = Some(binding),
                                    Err(error) => eprintln!(
                                        "ferese-shell: popover material unavailable: {error}"
                                    ),
                                }
                                EFFECT_FRAME_PENDING.store(false, Ordering::Relaxed);
                            }
                        }
                        _ => {}
                    }
                }
                Task::none()
            }
            Event::Window(window::Event::Closed)
                if self.menu.as_ref().is_some_and(|menu| menu.id == id) =>
            {
                self.menu = None;
                EFFECT_FRAME_PENDING.store(false, Ordering::Relaxed);
                Task::none()
            }
            Event::Window(window::Event::Closed) if id == self.bar_surface_id => {
                cosmic::iced::exit()
            }
            Event::PlatformSpecific(PlatformSpecific::Wayland(wayland::Event::Frame(
                _,
                surface,
                frame_id,
            ))) if self.menu.as_ref().is_some_and(|menu| menu.id == frame_id) => {
                if let Some(menu) = &mut self.menu {
                    if menu.effects.is_none() {
                        match EffectsBinding::attach_role(&surface, menu.kind.material_role()) {
                            Ok(binding) => menu.effects = Some(binding),
                            Err(error) => {
                                eprintln!("ferese-shell: popover material unavailable: {error}")
                            }
                        }
                        EFFECT_FRAME_PENDING.store(false, Ordering::Relaxed);
                    }
                }
                Task::none()
            }
            Event::PlatformSpecific(PlatformSpecific::Wayland(wayland::Event::Frame(
                _,
                surface,
                frame_id,
            ))) if frame_id == self.bar_surface_id => {
                self.attach_effects(&surface);
                Task::none()
            }
            Event::PlatformSpecific(PlatformSpecific::Wayland(wayland::Event::Layer(
                _,
                surface,
                layer_id,
            ))) if layer_id == self.bar_surface_id => {
                self.attach_effects(&surface);
                Task::none()
            }
            _ => Task::none(),
        }
    }

    fn attach_effects(&mut self, surface: &wl_surface::WlSurface) {
        if self.effects.is_some() {
            return;
        }

        EFFECT_FRAME_PENDING.store(false, Ordering::Relaxed);

        match EffectsBinding::attach(surface, !bar_hidden(&self.snapshot)) {
            Ok(binding) => self.effects = Some(binding),
            Err(error) => eprintln!("ferese-shell: panel material unavailable: {error}"),
        }
    }

    fn view_layer(&self) -> Element<'_, cosmic::Action<Message>> {
        if bar_hidden(&self.snapshot) {
            return container(text(""))
                .width(Length::Fill)
                .height(Length::Fill)
                .into();
        }

        let focused_output = self
            .snapshot
            .outputs
            .iter()
            .find(|output| output.focused)
            .or_else(|| self.snapshot.outputs.first());
        let focused_output_id = focused_output.map(|output| output.id);
        let shell_theme = self.config.theme.for_bar();
        let bar = BarMetrics::from(shell_theme);
        let mut workspace_row = row::with_capacity(self.snapshot.workspaces.len() + 1)
            .spacing(1)
            .align_y(cosmic::iced::Alignment::Center);

        workspace_row = workspace_row.push(
            button::custom(overview_control(bar, color(shell_theme.text_primary)))
                .height(bar.control_height)
                .padding([0, 7])
                .class(status_ui::button_style(
                    color(shell_theme.text_primary),
                    self.overview_active,
                    1.0,
                ))
                .on_press(cosmic::Action::App(Message::ToggleOverview)),
        );
        for workspace in self
            .snapshot
            .workspaces
            .iter()
            .filter(|workspace| self.overview_active && workspace.output == focused_output_id)
        {
            let occupied = self
                .snapshot
                .windows
                .iter()
                .any(|window| window.workspace == workspace.id);
            let indicator = workspace_indicator(workspace.active, occupied, bar, shell_theme);
            let workspace_button = button::custom(indicator)
                .height(bar.control_height)
                .padding(0)
                .class(status_ui::button_style(
                    color(shell_theme.text_primary),
                    false,
                    1.0,
                ))
                .on_press(cosmic::Action::App(Message::ActivateWorkspace(
                    workspace.id,
                )));

            workspace_row = workspace_row.push(workspace_button);
        }

        let foreground = color(shell_theme.text_primary);
        let mut windows = row::with_capacity(self.snapshot.windows.len())
            .spacing(4)
            .align_y(cosmic::iced::Alignment::Center);
        for window in self.snapshot.windows.iter().filter(|window| {
            focused_output.is_some_and(|output| window.workspace == output.active_workspace)
        }) {
            let label = compact_window_label(window);
            let foreground = color(if window.focused {
                shell_theme.text_primary
            } else {
                shell_theme.text_muted
            });
            windows = windows.push(
                button::custom(
                    container(
                        text(label)
                            .size(bar.text_size)
                            .wrapping(cosmic::iced::core::text::Wrapping::None)
                            .class(theme::Text::Color(foreground)),
                    )
                    .center_y(bar.control_height),
                )
                .padding([0, 8])
                .height(bar.control_height)
                .class(status_ui::button_style(foreground, window.focused, 1.0))
                .on_press(cosmic::Action::App(Message::ActivateWindow(window.id))),
            );
        }
        let windows = cosmic::iced::widget::scrollable(windows)
            .direction(cosmic::iced::widget::scrollable::Direction::Horizontal(
                cosmic::iced::widget::scrollable::Scrollbar::default()
                    .width(0)
                    .scroller_width(0),
            ))
            .width(Length::Fill)
            .height(bar.control_height);
        let left = row![workspace_row, windows]
            .spacing(8)
            .align_y(cosmic::iced::Alignment::Center);
        let clock = container(
            text(&self.clock)
                .size(bar.text_size)
                .wrapping(cosmic::iced::core::text::Wrapping::None)
                .class(theme::Text::Color(foreground)),
        )
        .padding([0, 6]);
        let right = row![self.view_status_bar(), clock]
            .spacing(6)
            .align_y(cosmic::iced::Alignment::Center);
        let content = row![
            container(left).width(Length::Fill),
            container(right).width(Length::Shrink),
        ]
        .spacing(16)
        .align_y(cosmic::iced::Alignment::Center)
        .height(Length::Fill);

        let compositor_material = self.effects.is_some();
        container(content)
            .width(Length::Fill)
            .height(Length::Fill)
            .padding([0, shell_theme.panel_padding.round() as u16])
            .class(theme::Container::custom(move |_| {
                bar_style(shell_theme, compositor_material)
            }))
            .into()
    }

    fn view_wallpaper(&self) -> Element<'_, cosmic::Action<Message>> {
        let Some(handle) = self.wallpaper.as_ref() else {
            return container(text(""))
                .width(Length::Fill)
                .height(Length::Fill)
                .class(theme::Container::custom(wallpaper_fallback_style))
                .into();
        };
        let content_fit = match self.config.wallpaper.mode {
            WallpaperMode::Fill => ContentFit::Cover,
            WallpaperMode::Fit => ContentFit::Contain,
        };

        container(
            image(handle.clone())
                .width(Length::Fill)
                .height(Length::Fill)
                .content_fit(content_fit),
        )
        .width(Length::Fill)
        .height(Length::Fill)
        .class(theme::Container::custom(wallpaper_fallback_style))
        .into()
    }
}

fn bar_icon(source: &'static [u8], size: u16, foreground: Color) -> icon::Icon {
    accented_icon(source, size, foreground, foreground)
}

fn accented_icon(source: &'static [u8], size: u16, foreground: Color, accent: Color) -> icon::Icon {
    let svg = std::str::from_utf8(source).expect("embedded SVG must be UTF-8");
    // Battery canvases are wider; reserve that space instead of stretching
    // or clipping them into the square slot used by the other status icons.
    let aspect = if svg.contains("viewBox=\"0 0 32 24\"") {
        4.0 / 3.0
    } else {
        1.0
    };
    icon::from_svg_bytes(tinted_svg(source, foreground, accent))
        .symbolic(false)
        .icon()
        .size(size)
        .width(Length::Fixed(f32::from(size) * aspect))
        .content_fit(ContentFit::Contain)
        .class(theme::Svg::custom(move |_| {
            cosmic::iced::widget::svg::Style { color: None }
        }))
}

fn tinted_svg(source: &[u8], foreground: Color, accent: Color) -> Vec<u8> {
    let rgba = |foreground: Color| {
        format!(
            "rgba({},{},{},{})",
            (foreground.r * 255.0).round() as u8,
            (foreground.g * 255.0).round() as u8,
            (foreground.b * 255.0).round() as u8,
            foreground.a,
        )
    };
    std::str::from_utf8(source)
        .expect("embedded icons must be UTF-8 SVG")
        .replace("currentColor", &rgba(foreground))
        // The source artwork's blue swatch is the semantic theme accent.
        // Battery warning/success swatches remain status colors.
        .replace("#3d7be6", &rgba(accent))
        .into_bytes()
}

fn overview_control(
    bar: BarMetrics,
    foreground: Color,
) -> Element<'static, cosmic::Action<Message>> {
    container(bar_icon(
        include_bytes!("../assets/icons/ferese.svg"),
        bar.overview_icon_size,
        foreground,
    ))
    .width(bar.overview_icon_size)
    .height(bar.control_height)
    .align_x(alignment::Horizontal::Center)
    .align_y(alignment::Vertical::Center)
    .into()
}

fn bar_hidden(snapshot: &ShellSnapshot) -> bool {
    let active_workspace = snapshot
        .outputs
        .iter()
        .find(|output| output.focused)
        .or_else(|| snapshot.outputs.first())
        .map(|output| output.active_workspace);

    active_workspace.is_some_and(|workspace| {
        snapshot
            .windows
            .iter()
            .any(|window| window.workspace == workspace && window.fullscreen)
    })
}

fn workspace_indicator(
    active: bool,
    occupied: bool,
    bar: BarMetrics,
    shell_theme: ShellTheme,
) -> Element<'static, cosmic::Action<Message>> {
    let (width, height, color, radius) = if active {
        (
            bar.active_marker_width,
            bar.active_marker_height,
            color(shell_theme.accent),
            2.0,
        )
    } else if occupied {
        (
            bar.dot_marker_size,
            bar.dot_marker_size,
            color_with_opacity(shell_theme.text_muted, 0.78),
            3.0,
        )
    } else {
        (
            bar.empty_marker_size,
            bar.empty_marker_size,
            color_with_opacity(shell_theme.text_muted, 0.42),
            2.0,
        )
    };
    let marker = container(text(""))
        .width(width)
        .height(height)
        .class(theme::Container::custom(move |_| {
            workspace_marker_style(color, radius)
        }));

    container(marker)
        .width(bar.workspace_hit_width)
        .height(bar.control_height)
        .align_x(alignment::Horizontal::Center)
        .align_y(alignment::Vertical::Center)
        .into()
}

fn workspace_marker_style(color: Color, radius: f32) -> container::Style {
    container::Style {
        background: Some(Background::Color(color)),
        border: Border {
            radius: radius.into(),
            ..Border::default()
        },
        snap: true,
        ..container::Style::default()
    }
}

fn display_app_name(window: &control::WindowSnapshot) -> String {
    let identity = window
        .app_id
        .rsplit('.')
        .find(|part| !part.is_empty())
        .unwrap_or(&window.app_id);

    if identity.is_empty() {
        window.title.clone()
    } else {
        let mut characters = identity.chars();
        characters
            .next()
            .map(|first| first.to_uppercase().collect::<String>() + characters.as_str())
            .unwrap_or_else(|| "Desktop".to_owned())
    }
}

fn current_time() -> String {
    format_bar_time(&Zoned::now())
}

fn format_bar_time(now: &Zoned) -> String {
    let months = [
        "jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sept", "oct", "nov", "dec",
    ];
    let hour = now.hour();
    let hour12 = if hour % 12 == 0 { 12 } else { hour % 12 };
    format!(
        "{} {}, {}:{:02} {}",
        now.day(),
        months[(now.month() - 1) as usize],
        hour12,
        now.minute(),
        if hour < 12 { "am" } else { "pm" }
    )
}

fn compact_window_label(window: &crate::control::WindowSnapshot) -> String {
    let title = window.title.trim();
    let label = if title.is_empty() {
        display_app_name(window)
    } else {
        title.to_owned()
    };
    let mut characters = label.chars();
    let prefix: String = characters.by_ref().take(24).collect();
    if characters.next().is_some() {
        format!("{prefix}…")
    } else {
        prefix
    }
}

fn shell_surface_style(theme: ShellTheme) -> cosmic::iced::theme::Style {
    cosmic::iced::theme::Style {
        background_color: Color::TRANSPARENT,
        text_color: color(theme.text_primary),
        icon_color: color(theme.text_primary),
    }
}

fn bar_style(theme: ShellTheme, compositor_material: bool) -> container::Style {
    container::Style {
        icon_color: Some(color(theme.text_primary)),
        text_color: Some(color(theme.text_primary)),
        // The compositor paints the selected panel material behind this surface.
        // A client-side fill would cover its blur and tint; use it only as fallback.
        background: if compositor_material {
            None
        } else {
            Some(Background::Color(color(theme.bar_background)))
        },
        border: Border {
            radius: theme.bar_radius.into(),
            ..Border::default()
        },
        snap: true,
        ..container::Style::default()
    }
}

fn color([red, green, blue, alpha]: [u8; 4]) -> Color {
    Color::from_rgba8(red, green, blue, f32::from(alpha) / 255.0)
}

fn color_with_opacity(mut value: [u8; 4], opacity: f32) -> Color {
    value[3] = (f32::from(value[3]) * opacity).round() as u8;
    color(value)
}

fn wallpaper_fallback_style(_theme: &cosmic::Theme) -> container::Style {
    let [red, green, blue] = config::default_background();

    container::Style {
        background: Some(Background::Color(Color::from_rgb8(red, green, blue))),
        ..container::Style::default()
    }
}

// Resolve a surface while iced guarantees the native handles are alive. Keep
// the borrowed-display connection alive until the effects binding takes over.
fn native_wayland_surface(
    window: &dyn cosmic::iced::window::Window,
) -> Result<(Connection, wl_surface::WlSurface), String> {
    use cosmic::iced::window::raw_window_handle::{RawDisplayHandle, RawWindowHandle};
    let display = window.display_handle().map_err(|error| error.to_string())?;
    let handle = window.window_handle().map_err(|error| error.to_string())?;
    let (RawDisplayHandle::Wayland(display), RawWindowHandle::Wayland(handle)) =
        (display.as_raw(), handle.as_raw())
    else {
        return Err("shell material requires a Wayland surface".to_owned());
    };
    // SAFETY: iced lends matching, live Wayland handles for this callback.
    // This backend borrows the display; it never disconnects iced's connection.
    // Both handles belong to the runtime, which outlives the shell bindings.
    let backend = unsafe {
        wayland_client::backend::Backend::from_foreign_display(display.display.as_ptr().cast())
    };
    let connection = Connection::from_backend(backend);
    // SAFETY: raw-window-handle identifies this pointer as a live wl_surface.
    let id = unsafe {
        wayland_client::backend::ObjectId::from_ptr(
            wl_surface::WlSurface::interface(),
            handle.surface.as_ptr().cast(),
        )
    }
    .map_err(|error| error.to_string())?;
    let surface =
        wl_surface::WlSurface::from_id(&connection, id).map_err(|error| error.to_string())?;
    Ok((connection, surface))
}

struct EffectsBinding {
    connection: Connection,
    _manager: FereseEffectsManagerV1,
    surface: FereseSurfaceEffectsV1,
    _queue: EventQueue<EffectsState>,
    regions: std::cell::RefCell<Option<Vec<[i32; 5]>>>,
}

impl EffectsBinding {
    fn attach(
        surface: &wl_surface::WlSurface,
        visible: bool,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        Self::attach_role(
            surface,
            visible.then_some(ferese_surface_effects_v1::Role::Panel),
        )
    }

    fn attach_role(
        surface: &wl_surface::WlSurface,
        role: Option<ferese_surface_effects_v1::Role>,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let backend = surface
            .backend()
            .upgrade()
            .ok_or("libcosmic Wayland connection is no longer alive")?;
        let connection = Connection::from_backend(backend);
        let (globals, queue) = registry_queue_init::<EffectsState>(&connection)?;
        let qh = queue.handle();
        let manager = globals.bind::<FereseEffectsManagerV1, _, _>(&qh, 1..=2, ())?;
        let effects = manager.get_surface_effects(surface, &qh, ());

        if let Some(role) = role {
            effects.set_role(role);
        } else {
            effects.clear_role();
        }
        connection.flush()?;

        Ok(Self {
            connection,
            _manager: manager,
            surface: effects,
            _queue: queue,
            regions: Default::default(),
        })
    }

    fn set_regions(&self, regions: &[[i32; 5]]) -> Result<(), Box<dyn std::error::Error>> {
        if self.surface.version() < 2 || self.regions.borrow().as_deref() == Some(regions) {
            return Ok(());
        }
        self.surface.set_regions(
            regions
                .iter()
                .flat_map(|r| r.iter().flat_map(|v| v.to_ne_bytes()))
                .collect(),
        );
        if regions.is_empty() {
            self.surface.clear_role();
        } else {
            self.surface
                .set_role(ferese_surface_effects_v1::Role::Popover);
        }
        self.connection.flush()?;
        *self.regions.borrow_mut() = Some(regions.to_vec());
        Ok(())
    }

    fn set_visible(&self, visible: bool) -> Result<(), Box<dyn std::error::Error>> {
        if visible {
            self.surface
                .set_role(ferese_surface_effects_v1::Role::Panel);
        } else {
            self.surface.clear_role();
        }

        self.connection.flush()?;
        Ok(())
    }
}

struct EffectsState;

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for EffectsState {
    fn event(
        _state: &mut Self,
        _registry: &wl_registry::WlRegistry,
        _event: wl_registry::Event,
        _data: &GlobalListContents,
        _connection: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
    }
}

delegate_noop!(EffectsState: ignore FereseEffectsManagerV1);
delegate_noop!(EffectsState: ignore FereseSurfaceEffectsV1);

#[cfg(test)]
mod tests {
    #[test]
    fn clock_uses_lowercase_date_and_twelve_hour_time() {
        for (stamp, expected) in [
            (
                "2026-09-28T21:32:00+01:00[Africa/Lagos]",
                "28 sept, 9:32 pm",
            ),
            (
                "2026-09-28T00:05:00+01:00[Africa/Lagos]",
                "28 sept, 12:05 am",
            ),
            (
                "2026-09-28T12:00:00+01:00[Africa/Lagos]",
                "28 sept, 12:00 pm",
            ),
        ] {
            assert_eq!(super::format_bar_time(&stamp.parse().unwrap()), expected);
        }
    }
    use super::*;
    use crate::control::{OutputSnapshot, WindowSnapshot};

    #[test]
    fn shell_font_respects_configured_family() {
        assert_eq!(
            configured_font(Some("JetBrainsMono Nerd Font")),
            cosmic::font::Font::with_name("JetBrainsMono Nerd Font")
        );
        assert_eq!(configured_font(None), cosmic::font::default());
    }

    #[test]
    fn icons_use_theme_accent_and_preserve_battery_status_colors() {
        for (source, accent) in [
            (
                include_bytes!("../assets/icons/status/battery-full.svg").as_slice(),
                "#63c168",
            ),
            (
                include_bytes!("../assets/icons/status/battery-50.svg").as_slice(),
                "#3d7be6",
            ),
            (
                include_bytes!("../assets/icons/status/battery-25.svg").as_slice(),
                "#e0654f",
            ),
            (
                include_bytes!("../assets/icons/status/wifi-full.svg").as_slice(),
                "#3d7be6",
            ),
            (
                include_bytes!("../assets/icons/status/control-center.svg").as_slice(),
                "#3d7be6",
            ),
        ] {
            let svg = String::from_utf8(tinted_svg(
                source,
                Color::from_rgb8(205, 214, 244),
                Color::from_rgb8(203, 166, 247),
            ))
            .unwrap();
            assert!(!svg.contains("currentColor"));
            assert!(svg.contains("rgba(205,214,244,1)"));
            assert!(!svg.contains("#3d7be6"));
            assert!(svg.contains(if accent == "#3d7be6" {
                "rgba(203,166,247,1)"
            } else {
                accent
            }));
        }
    }

    #[test]
    fn window_list_labels_use_titles_and_fall_back_to_the_app_name() {
        let snapshot = snapshot_with_fullscreen_window(7);
        let mut window = snapshot.windows[0].clone();
        window.title = "  editor.rs — Project  ".to_owned();
        assert_eq!(compact_window_label(&window), "editor.rs — Project");
        window.title = " ".to_owned();
        assert_eq!(compact_window_label(&window), display_app_name(&window));
    }

    #[test]
    fn window_list_labels_truncate_unicode_without_breaking_characters() {
        let snapshot = snapshot_with_fullscreen_window(7);
        let mut window = snapshot.windows[0].clone();
        window.title = "界".repeat(25);
        assert_eq!(
            compact_window_label(&window),
            format!("{}…", "界".repeat(24))
        );
        window.title = "界".repeat(24);
        assert_eq!(compact_window_label(&window), window.title);
    }

    #[test]
    fn startup_surface_clear_is_transparent_and_bar_fallback_is_dark() {
        let theme = ShellTheme::default();
        assert_eq!(
            shell_surface_style(theme).background_color,
            Color::TRANSPARENT
        );
        let Some(Background::Color(background)) = bar_style(theme.for_bar(), false).background
        else {
            panic!("first frame needs a fallback fill before material attachment");
        };
        assert!(background.r < 0.15 && background.g < 0.15 && background.b < 0.20);
        assert!(background.a > 0.9);
        assert!(
            theme.bar_text_primary[..3]
                .iter()
                .all(|channel| *channel > 220)
        );
    }

    #[test]
    fn bar_fallback_uses_the_matching_dark_palette_and_geometry() {
        let theme = ShellTheme::default().for_bar();
        let style = bar_style(theme, false);
        assert_eq!(
            style.background,
            Some(Background::Color(color(theme.bar_background)))
        );
        assert_eq!(style.text_color, Some(color(theme.bar_text_primary)));
        assert_eq!(style.border.radius, theme.bar_radius.into());
        assert_eq!(style.shadow, cosmic::iced::Shadow::default());
    }

    #[test]
    fn bar_does_not_cover_the_selected_compositor_material() {
        let theme = ShellTheme::default().for_bar();
        let style = bar_style(theme, true);
        assert_eq!(style.background, None);
        assert_eq!(style.text_color, Some(color(theme.bar_text_primary)));
        assert_eq!(style.border.radius, theme.bar_radius.into());
        assert_eq!(style.shadow, cosmic::iced::Shadow::default());
    }

    #[test]
    fn compact_bar_preserves_logical_text_and_icon_sizes() {
        for height in [24.0, 26.0, 32.0, 38.0, 44.0] {
            let metrics = BarMetrics::from(ShellTheme {
                bar_height: height,
                ..ShellTheme::default()
            });
            assert_eq!(metrics.text_size, 13);
            assert_eq!(metrics.icon_size, 19);
            assert_eq!(metrics.overview_icon_size, 19);
            assert!(metrics.control_height >= f32::from(metrics.icon_size));
            assert!(metrics.control_height <= height);
        }
    }

    #[test]
    fn fullscreen_on_the_active_workspace_hides_the_bar() {
        let snapshot = snapshot_with_fullscreen_window(7);

        assert!(bar_hidden(&snapshot));
    }

    #[test]
    fn fullscreen_on_an_inactive_workspace_keeps_the_bar_visible() {
        let snapshot = snapshot_with_fullscreen_window(8);

        assert!(!bar_hidden(&snapshot));
    }

    fn snapshot_with_fullscreen_window(workspace: u64) -> ShellSnapshot {
        ShellSnapshot {
            outputs: vec![OutputSnapshot {
                id: 1,
                active_workspace: 7,
                focused: true,
            }],
            workspaces: Vec::new(),
            windows: vec![WindowSnapshot {
                id: 1,
                workspace,
                app_id: "dev.ferese.Test".to_owned(),
                title: "Test".to_owned(),
                focused: true,
                fullscreen: true,
            }],
        }
    }
}
