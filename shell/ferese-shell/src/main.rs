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
    Background, Border, Color, Event, Length, Shadow, Subscription, Vector, window,
};
use cosmic::theme;
use cosmic::widget::{button, container, row, text};
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

use crate::control::{ShellControl, ShellSnapshot};

const APP_ID: &str = "dev.ferese.Shell";
const BAR_HEIGHT: u32 = 32;
const BAR_MARGIN: i32 = 8;
const EXCLUSIVE_ZONE: i32 = BAR_HEIGHT as i32;

fn main() -> cosmic::iced::Result {
    let settings = Settings::default()
        .no_main_window(true)
        .client_decorations(false)
        .transparent(true)
        .is_daemon(true);

    cosmic::app::run::<FereseShell>(settings, ())
}

struct FereseShell {
    core: Core,
    surface_id: window::Id,
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
    type Flags = ();
    type Message = Message;

    const APP_ID: &'static str = APP_ID;

    fn core(&self) -> &Core {
        &self.core
    }

    fn core_mut(&mut self) -> &mut Core {
        &mut self.core
    }

    fn init(core: Core, _flags: Self::Flags) -> (Self, Task<Self::Message>) {
        let surface_id = window::Id::unique();
        let app = Self {
            core,
            surface_id,
            control: ShellControl::connect()
                .map_err(|error| {
                    eprintln!("ferese-shell: shell control unavailable: {error}");
                })
                .ok(),
            snapshot: ShellSnapshot::default(),
            clock: current_time(),
            effects: None,
        };
        let action = cosmic::surface::action::app_layer_shell::<Self>(
            |_| Default::default(),
            move |_| SctkLayerSurfaceSettings {
                id: surface_id,
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
                exclusive_zone: EXCLUSIVE_ZONE,
                ..Default::default()
            },
            Some(Box::new(Self::view_layer)),
        );

        (app, cosmic::task::message(cosmic::Action::Surface(action)))
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
            Event::Window(window::Event::Closed) if id == self.surface_id => cosmic::iced::exit(),
            Event::PlatformSpecific(PlatformSpecific::Wayland(wayland::Event::Frame(
                _,
                surface,
                frame_id,
            ))) if frame_id == self.surface_id => {
                self.attach_effects(&surface);
                Task::none()
            }
            Event::PlatformSpecific(PlatformSpecific::Wayland(wayland::Event::Layer(
                _,
                surface,
                layer_id,
            ))) if layer_id == self.surface_id => {
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
            .spacing(2)
            .align_y(cosmic::iced::Alignment::Center);

        workspace_row = workspace_row.push(
            button::suggested("F")
                .font_size(12)
                .height(24)
                .padding([2, 9])
                .on_press(cosmic::Action::App(Message::EnterOverview)),
        );
        for workspace in self
            .snapshot
            .workspaces
            .iter()
            .filter(|workspace| workspace.output == focused_output_id)
        {
            let button = if workspace.active {
                button::suggested(workspace.name.clone())
            } else {
                button::text(workspace.name.clone())
            };

            workspace_row =
                workspace_row.push(button.font_size(12).height(24).padding([2, 8]).on_press(
                    cosmic::Action::App(Message::ActivateWorkspace(workspace.id)),
                ));
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
        let left = row![workspace_row, text(app_name).size(12)]
            .spacing(12)
            .align_y(cosmic::iced::Alignment::Center);
        let center = text(&self.clock).size(13);
        let output_hint = focused_output
            .map(|output| output.name.as_str())
            .unwrap_or("No display");
        let right = row![
            text("◌").size(16),
            text("♪").size(15),
            text(output_hint).size(11),
            text("●").size(10),
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
