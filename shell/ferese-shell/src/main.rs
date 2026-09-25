mod config;
mod control;

use std::time::Duration;

use cosmic::Element;
use cosmic::app::{Core, Settings, Task};
use cosmic::iced::alignment;
use cosmic::iced::event::{self, PlatformSpecific, wayland};
use cosmic::iced::platform_specific::{
    runtime::wayland::layer_surface::{IcedMargin, IcedOutput, SctkLayerSurfaceSettings},
    shell::commands::layer_surface::{Anchor, KeyboardInteractivity, Layer},
};
use cosmic::iced::{
    Background, Border, Color, ContentFit, Event, Length, Limits, Shadow, Subscription, Vector,
    window,
};
use cosmic::theme;
use cosmic::widget::{button, container, image, row, text};
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

use crate::config::{ShellConfig, WallpaperMode};
use crate::control::{ShellControl, ShellSnapshot};

const APP_ID: &str = "dev.ferese.Shell";
const BAR_HEIGHT: u32 = 32;
const BAR_MARGIN: i32 = 4;
const EXCLUSIVE_ZONE: i32 = BAR_HEIGHT as i32;
const BAR_TEXT_SIZE: u16 = (BAR_HEIGHT * 3 / 8) as u16;
const BAR_ICON_SIZE: u16 = (BAR_HEIGHT / 2) as u16;
const CONTROL_HEIGHT: f32 = BAR_HEIGHT as f32 * 0.75;
const WORKSPACE_HIT_WIDTH: f32 = BAR_HEIGHT as f32 * 0.6875;
const ACTIVE_MARKER_WIDTH: f32 = BAR_HEIGHT as f32 * 0.5;
const ACTIVE_MARKER_HEIGHT: f32 = BAR_HEIGHT as f32 * 0.125;
const DOT_MARKER_SIZE: f32 = BAR_HEIGHT as f32 * 0.1875;
const EMPTY_MARKER_SIZE: f32 = BAR_HEIGHT as f32 * 0.125;

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
    clock: String,
    effects: Option<EffectsBinding>,
}

#[derive(Clone, Debug)]
enum Message {
    Event(Event, window::Id),
    Tick,
    ActivateWorkspace(u64),
    EnterOverview,
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
                    top: BAR_MARGIN,
                    right: 10,
                    bottom: 0,
                    left: 10,
                },
                size: Some((None, Some(BAR_HEIGHT))),
                size_limits: Limits::NONE,
                exclusive_zone: EXCLUSIVE_ZONE,
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
                    if let Some(snapshot) = poll.snapshot {
                        self.snapshot = snapshot;
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
            Message::EnterOverview => {
                if let Some(control) = &self.control {
                    control.enter_overview();
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

        match EffectsBinding::attach(surface) {
            Ok(binding) => self.effects = Some(binding),
            Err(error) => eprintln!("ferese-shell: panel material unavailable: {error}"),
        }
    }

    fn view_layer(&self) -> Element<'_, cosmic::Action<Message>> {
        let focused_output = self
            .snapshot
            .outputs
            .iter()
            .find(|output| output.focused)
            .or_else(|| self.snapshot.outputs.first());
        let focused_output_id = focused_output.map(|output| output.id);
        let mut workspace_row = row::with_capacity(self.snapshot.workspaces.len() + 1)
            .spacing(1)
            .align_y(cosmic::iced::Alignment::Center);

        workspace_row = workspace_row.push(
            button::suggested("F")
                .font_size(BAR_TEXT_SIZE)
                .height(CONTROL_HEIGHT)
                .padding([2, 9])
                .on_press(cosmic::Action::App(Message::EnterOverview)),
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
            let indicator = workspace_indicator(workspace.active, occupied);
            let workspace_button = button::custom(indicator)
                .height(CONTROL_HEIGHT)
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
        let left = row![workspace_row, text(app_name).size(BAR_TEXT_SIZE)]
            .spacing(12)
            .align_y(cosmic::iced::Alignment::Center);
        let center = text(&self.clock).size(BAR_TEXT_SIZE);
        let right = row![
            text("◌").size(BAR_ICON_SIZE),
            text("♪").size(BAR_ICON_SIZE),
            text("●").size(BAR_ICON_SIZE),
        ]
        .spacing(10)
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
            .padding([0, 12])
            .class(theme::Container::custom(bar_style))
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

fn workspace_indicator(active: bool, occupied: bool) -> Element<'static, cosmic::Action<Message>> {
    let (width, height, style) = if active {
        (
            ACTIVE_MARKER_WIDTH,
            ACTIVE_MARKER_HEIGHT,
            active_workspace_style as fn(&cosmic::Theme) -> container::Style,
        )
    } else if occupied {
        (
            DOT_MARKER_SIZE,
            DOT_MARKER_SIZE,
            occupied_workspace_style as fn(&cosmic::Theme) -> container::Style,
        )
    } else {
        (
            EMPTY_MARKER_SIZE,
            EMPTY_MARKER_SIZE,
            empty_workspace_style as fn(&cosmic::Theme) -> container::Style,
        )
    };
    let marker = container(text(""))
        .width(width)
        .height(height)
        .class(theme::Container::custom(style));

    container(marker)
        .width(WORKSPACE_HIT_WIDTH)
        .height(CONTROL_HEIGHT)
        .align_x(alignment::Horizontal::Center)
        .align_y(alignment::Vertical::Center)
        .into()
}

fn active_workspace_style(_theme: &cosmic::Theme) -> container::Style {
    workspace_marker_style(Color::from_rgb8(91, 140, 255), 2.0)
}

fn occupied_workspace_style(_theme: &cosmic::Theme) -> container::Style {
    workspace_marker_style(Color::from_rgba8(221, 228, 240, 0.68), 3.0)
}

fn empty_workspace_style(_theme: &cosmic::Theme) -> container::Style {
    workspace_marker_style(Color::from_rgba8(221, 228, 240, 0.25), 2.0)
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

fn bar_style(_theme: &cosmic::Theme) -> container::Style {
    container::Style {
        icon_color: Some(Color::from_rgb8(235, 239, 247)),
        text_color: Some(Color::from_rgb8(235, 239, 247)),
        background: Some(Background::Color(Color::from_rgba8(18, 21, 28, 0.88))),
        border: Border {
            color: Color::from_rgba8(255, 255, 255, 0.14),
            width: 1.0,
            radius: 16.0.into(),
        },
        shadow: Shadow {
            color: Color::from_rgba8(0, 0, 0, 0.32),
            offset: Vector::new(0.0, 4.0),
            blur_radius: 18.0,
        },
        snap: true,
    }
}

fn wallpaper_fallback_style(_theme: &cosmic::Theme) -> container::Style {
    let [red, green, blue] = config::default_background();

    container::Style {
        background: Some(Background::Color(Color::from_rgb8(red, green, blue))),
        ..container::Style::default()
    }
}

struct EffectsBinding {
    _manager: FereseEffectsManagerV1,
    _surface: FereseSurfaceEffectsV1,
    _queue: EventQueue<EffectsState>,
}

impl EffectsBinding {
    fn attach(surface: &wl_surface::WlSurface) -> Result<Self, Box<dyn std::error::Error>> {
        let backend = surface
            .backend()
            .upgrade()
            .ok_or("libcosmic Wayland connection is no longer alive")?;
        let connection = Connection::from_backend(backend);
        let (globals, queue) = registry_queue_init::<EffectsState>(&connection)?;
        let qh = queue.handle();
        let manager = globals.bind::<FereseEffectsManagerV1, _, _>(&qh, 1..=1, ())?;
        let effects = manager.get_surface_effects(surface, &qh, ());

        effects.set_role(ferese_surface_effects_v1::Role::Panel);
        connection.flush()?;

        Ok(Self {
            _manager: manager,
            _surface: effects,
            _queue: queue,
        })
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
