mod config;
mod control;

use std::time::Duration;

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
    Background, Border, Color, ContentFit, Event, Length, Limits, Shadow, Subscription, Vector,
    window,
};
use cosmic::theme;
use cosmic::widget::{button, container, icon, image, row, text};
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

#[derive(Clone, Copy)]
struct BarMetrics {
    height: f32,
    text_size: u16,
    icon_size: u16,
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
            text_size: (height * 0.368).round() as u16,
            icon_size: (height * 0.553).round() as u16,
            control_height: height * 0.79,
            workspace_hit_width: height * 0.68,
            active_marker_width: height * 0.47,
            active_marker_height: height * 0.13,
            dot_marker_size: height * 0.18,
            empty_marker_size: height * 0.13,
        }
    }
}

fn main() -> cosmic::iced::Result {
    let config = config::load();
    let mut settings = Settings::default()
        .no_main_window(true)
        .client_decorations(false)
        .transparent(true)
        .is_daemon(true);
    if let Some(font_family) = config.font_family.clone() {
        let font_family = Box::leak(font_family.into_boxed_str());

        settings = settings.default_font(cosmic::font::Font::with_name(font_family));
    }

    cosmic::app::run::<FereseShell>(settings, config)
}

struct FereseShell {
    core: Core,
    bar_surface_id: window::Id,
    config: ShellConfig,
    control: Option<ShellControl>,
    snapshot: ShellSnapshot,
    overview_active: bool,
    clock: String,
    effects: Option<EffectsBinding>,
}

#[derive(Clone, Debug)]
enum Message {
    Event(Event, window::Id),
    Tick,
    ActivateWorkspace(u64),
    ToggleOverview,
}

impl cosmic::Application for FereseShell {
    type Executor = cosmic::executor::Default;
    type Flags = ShellConfig;
    type Message = Message;

    const APP_ID: &'static str = APP_ID;

    fn core(&self) -> &Core {
        &self.core
    }

    fn core_mut(&mut self) -> &mut Core {
        &mut self.core
    }

    fn init(core: Core, config: Self::Flags) -> (Self, Task<Self::Message>) {
        let bar_surface_id = window::Id::unique();
        let wallpaper_surface_id = window::Id::unique();
        let shell_theme = config.theme;
        let bar = BarMetrics::from(shell_theme);
        let app = Self {
            core,
            bar_surface_id,
            config,
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
                exclusive_zone: bar.height.round() as i32,
                ..Default::default()
            },
            Some(Box::new(Self::view_layer)),
        );

        let tasks = [wallpaper_action, bar_action]
            .map(cosmic::Action::Surface)
            .map(cosmic::task::message);

        (app, Task::batch(tasks))
    }

    fn subscription(&self) -> Subscription<Self::Message> {
        Subscription::batch([
            event::listen_with(|event, _status, id| Some(Message::Event(event, id))),
            cosmic::iced::time::every(Duration::from_millis(250)).map(|_| Message::Tick),
        ])
    }

    fn update(&mut self, message: Self::Message) -> Task<Self::Message> {
        match message {
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

                            return set_input_zone(self.bar_surface_id, input_zone);
                        }
                    }
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
            Event::Window(window::Event::Closed) if id == self.bar_surface_id => {
                cosmic::iced::exit()
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
        let shell_theme = self.config.theme;
        let bar = BarMetrics::from(shell_theme);
        let mut workspace_row = row::with_capacity(self.snapshot.workspaces.len() + 1)
            .spacing(1)
            .align_y(cosmic::iced::Alignment::Center);

        workspace_row = workspace_row.push(
            button::custom(overview_control(bar, shell_theme))
                .height(bar.control_height)
                .padding(0)
                .class(theme::Button::Transparent)
                .on_press(cosmic::Action::App(Message::ToggleOverview)),
        );
        for workspace in self
            .snapshot
            .workspaces
            .iter()
            .filter(|workspace| workspace.output == focused_output_id)
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
                .class(theme::Button::Transparent)
                .on_press(cosmic::Action::App(Message::ActivateWorkspace(
                    workspace.id,
                )));

            workspace_row = workspace_row.push(workspace_button);
        }

        let app_name = self
            .snapshot
            .windows
            .iter()
            .find(|window| {
                window.focused
                    && focused_output
                        .is_none_or(|output| window.workspace == output.active_workspace)
            })
            .map(display_app_name)
            .unwrap_or_else(|| "Desktop".to_owned());
        let left = row![workspace_row, text(app_name).size(bar.text_size)]
            .spacing(shell_theme.control_gap)
            .align_y(cosmic::iced::Alignment::Center);
        let center = text(&self.clock).size(bar.text_size);
        let right = row![
            symbolic_icon(include_bytes!("../assets/icons/wifi.svg"), bar.icon_size),
            symbolic_icon(include_bytes!("../assets/icons/volume.svg"), bar.icon_size),
            symbolic_icon(include_bytes!("../assets/icons/battery.svg"), bar.icon_size),
        ]
        .spacing(shell_theme.control_gap)
        .align_y(cosmic::iced::Alignment::Center);
        let content = row![
            container(left)
                .width(Length::FillPortion(1))
                .align_x(alignment::Horizontal::Left),
            container(center)
                .width(Length::FillPortion(1))
                .align_x(alignment::Horizontal::Center),
            container(right)
                .width(Length::FillPortion(1))
                .align_x(alignment::Horizontal::Right),
        ]
        .align_y(cosmic::iced::Alignment::Center)
        .height(Length::Fill);

        container(content)
            .width(Length::Fill)
            .height(Length::Fill)
            .padding([0, shell_theme.panel_padding.round() as u16])
            .class(theme::Container::custom(move |_| bar_style(shell_theme)))
            .into()
    }

    fn view_wallpaper(&self) -> Element<'_, cosmic::Action<Message>> {
        let Some(path) = self.config.wallpaper.path.as_ref() else {
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

        image(path.clone())
            .width(Length::Fill)
            .height(Length::Fill)
            .content_fit(content_fit)
            .into()
    }
}

fn symbolic_icon(source: &'static [u8], size: u16) -> icon::Icon {
    icon::from_svg_bytes(source)
        .symbolic(true)
        .icon()
        .size(size)
}

fn overview_control(
    bar: BarMetrics,
    shell_theme: ShellTheme,
) -> Element<'static, cosmic::Action<Message>> {
    container(symbolic_icon(
        include_bytes!("../assets/icons/overview.svg"),
        bar.icon_size,
    ))
    .width(bar.control_height)
    .height(bar.control_height)
    .align_x(alignment::Horizontal::Center)
    .align_y(alignment::Vertical::Center)
    .class(theme::Container::custom(move |_| container::Style {
        icon_color: Some(color(shell_theme.text_primary)),
        background: Some(Background::Color(color(shell_theme.accent))),
        border: Border {
            color: color_with_opacity(shell_theme.text_primary, 0.14),
            width: 1.0,
            radius: (bar.control_height * 0.34).into(),
        },
        snap: true,
        ..container::Style::default()
    }))
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
    Zoned::now().strftime("%H:%M").to_string()
}

fn bar_style(theme: ShellTheme) -> container::Style {
    let mut shadow = color(theme.shadow);
    shadow.a *= theme.shadow_opacity;

    container::Style {
        icon_color: Some(color(theme.text_primary)),
        text_color: Some(color(theme.text_primary)),
        background: None,
        border: Border {
            color: color(theme.border),
            width: 1.0,
            radius: theme.bar_radius.into(),
        },
        shadow: Shadow {
            color: shadow,
            offset: Vector::new(0.0, theme.shadow_offset_y),
            blur_radius: theme.shadow_blur,
        },
        snap: true,
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

struct EffectsBinding {
    connection: Connection,
    _manager: FereseEffectsManagerV1,
    surface: FereseSurfaceEffectsV1,
    _queue: EventQueue<EffectsState>,
}

impl EffectsBinding {
    fn attach(
        surface: &wl_surface::WlSurface,
        visible: bool,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let backend = surface
            .backend()
            .upgrade()
            .ok_or("libcosmic Wayland connection is no longer alive")?;
        let connection = Connection::from_backend(backend);
        let (globals, queue) = registry_queue_init::<EffectsState>(&connection)?;
        let qh = queue.handle();
        let manager = globals.bind::<FereseEffectsManagerV1, _, _>(&qh, 1..=1, ())?;
        let effects = manager.get_surface_effects(surface, &qh, ());

        if visible {
            effects.set_role(ferese_surface_effects_v1::Role::Panel);
        } else {
            effects.clear_role();
        }
        connection.flush()?;

        Ok(Self {
            connection,
            _manager: manager,
            surface: effects,
            _queue: queue,
        })
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
    use super::*;
    use crate::control::{OutputSnapshot, WindowSnapshot};

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
                workspace,
                app_id: "dev.ferese.Test".to_owned(),
                title: "Test".to_owned(),
                focused: true,
                fullscreen: true,
            }],
        }
    }
}
