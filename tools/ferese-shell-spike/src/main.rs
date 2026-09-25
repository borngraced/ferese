use cosmic::Element;
use cosmic::app::{Core, Settings, Task};
use cosmic::iced::event::{self, PlatformSpecific, wayland};
use cosmic::iced::keyboard::{self, Key, key::Named};
use cosmic::iced::platform_specific::{
    runtime::wayland::layer_surface::{IcedOutput, SctkLayerSurfaceSettings},
    shell::commands::layer_surface::{Anchor, KeyboardInteractivity, Layer},
};
use cosmic::iced::{Event, Length, Subscription, window};
use cosmic::widget::{button, column, container, text};
use ferese_protocols::effects::v1::client::{
    ferese_effects_manager_v1::FereseEffectsManagerV1,
    ferese_surface_effects_v1::{self, FereseSurfaceEffectsV1},
};
use std::time::Duration;
use wayland_client::{
    Connection, Dispatch, EventQueue, Proxy, QueueHandle, delegate_noop,
    globals::{GlobalListContents, registry_queue_init},
    protocol::{wl_registry, wl_surface},
};

const APP_ID: &str = "dev.ferese.ShellSpike";
const BAR_HEIGHT: u32 = 48;

fn main() -> cosmic::iced::Result {
    let settings = Settings::default()
        .no_main_window(true)
        .client_decorations(false)
        .transparent(true)
        .is_daemon(true);

    cosmic::app::run::<ShellSpike>(settings, ())
}

struct ShellSpike {
    core: Core,
    surface_id: window::Id,
    configured: bool,
    focused: bool,
    outputs: usize,
    exit_after: Option<Duration>,
    effects: Option<EffectsBinding>,
}

#[derive(Clone, Debug)]
enum Message {
    Event(Event, window::Id),
    Exit,
}

impl cosmic::Application for ShellSpike {
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
        let force_focus = std::env::var_os("FERESE_SHELL_SPIKE_EXCLUSIVE_FOCUS").is_some();
        let app = Self {
            core,
            surface_id,
            configured: false,
            focused: false,
            outputs: 0,
            exit_after: std::env::var("FERESE_SHELL_SPIKE_EXIT_AFTER_MS")
                .ok()
                .and_then(|value| value.parse().ok())
                .map(Duration::from_millis),
            effects: None,
        };
        let action = cosmic::surface::action::app_layer_shell::<Self>(
            |_| Default::default(),
            move |_| SctkLayerSurfaceSettings {
                id: surface_id,
                layer: Layer::Top,
                keyboard_interactivity: if force_focus {
                    KeyboardInteractivity::Exclusive
                } else {
                    KeyboardInteractivity::OnDemand
                },
                anchor: Anchor::TOP | Anchor::LEFT | Anchor::RIGHT,
                output: IcedOutput::Active,
                namespace: "ferese-shell-spike".to_owned(),
                size: Some((None, Some(BAR_HEIGHT))),
                exclusive_zone: BAR_HEIGHT as i32,
                ..Default::default()
            },
            Some(Box::new(Self::view_layer)),
        );

        println!(
            "PASS startup requested active-output layer surface focus={}",
            if force_focus {
                "exclusive"
            } else {
                "on-demand"
            }
        );

        (app, cosmic::task::message(cosmic::Action::Surface(action)))
    }

    fn subscription(&self) -> Subscription<Self::Message> {
        let events = event::listen_with(|event, _status, id| Some(Message::Event(event, id)));
        let mut subscriptions = vec![events];
        if let Some(delay) = self.exit_after {
            subscriptions.push(cosmic::iced::time::every(delay).map(|_| Message::Exit));
        }
        Subscription::batch(subscriptions)
    }

    fn update(&mut self, message: Self::Message) -> Task<Self::Message> {
        match message {
            Message::Event(event, id) => self.handle_event(event, id),
            Message::Exit => {
                println!("PASS clean shutdown requested");
                cosmic::iced::exit()
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

impl ShellSpike {
    fn handle_event(&mut self, event: Event, id: window::Id) -> Task<Message> {
        match event {
            Event::Window(window::Event::Opened { size, .. }) if id == self.surface_id => {
                self.configured = true;
                println!(
                    "PASS configure surface={} size={}x{}",
                    id, size.width, size.height
                );
            }
            Event::Window(window::Event::Resized(size)) if id == self.surface_id => {
                println!(
                    "PASS reconfigure surface={} size={}x{}",
                    id, size.width, size.height
                );
            }
            Event::Window(window::Event::Focused) if id == self.surface_id => {
                self.focused = true;
                println!("PASS keyboard focus acquired surface={id}");
            }
            Event::Window(window::Event::Unfocused) if id == self.surface_id => {
                self.focused = false;
                println!("PASS keyboard focus released surface={id}");
            }
            Event::Window(window::Event::Closed) if id == self.surface_id => {
                println!("PASS compositor closed layer surface={id}");
                return cosmic::iced::exit();
            }
            Event::Keyboard(keyboard::Event::KeyPressed {
                key: Key::Named(Named::Escape),
                ..
            }) if id == self.surface_id => return cosmic::task::message(Message::Exit),
            Event::PlatformSpecific(PlatformSpecific::Wayland(wayland::Event::Output(
                output_event,
                output,
            ))) => match output_event {
                wayland::OutputEvent::Created(info) => {
                    self.outputs += 1;
                    println!(
                        "PASS output discovered output={output:?} info={info:?} count={}",
                        self.outputs,
                    );
                }
                wayland::OutputEvent::InfoUpdate(info) => {
                    println!("PASS output updated output={output:?} info={info:?}");
                }
                wayland::OutputEvent::Removed => {
                    self.outputs = self.outputs.saturating_sub(1);
                    println!(
                        "PASS output removed output={output:?} count={}",
                        self.outputs,
                    );
                }
            },
            Event::PlatformSpecific(PlatformSpecific::Wayland(wayland::Event::Frame(
                _,
                surface,
                frame_id,
            ))) if frame_id == self.surface_id => self.attach_effects(&surface),
            Event::PlatformSpecific(PlatformSpecific::Wayland(wayland::Event::Layer(
                layer_event,
                surface,
                layer_id,
            ))) if layer_id == self.surface_id => {
                self.attach_effects(&surface);
                println!(
                    "PASS layer event={layer_event:?} surface={surface:?} configured={} focused={}",
                    self.configured, self.focused
                );
            }
            _ => {}
        }

        Task::none()
    }

    fn attach_effects(&mut self, surface: &wl_surface::WlSurface) {
        if self.effects.is_some() {
            return;
        }

        match EffectsBinding::attach(surface) {
            Ok(binding) => {
                self.effects = Some(binding);
                println!("PASS ferese-effects-v1 attached role=panel");
            }
            Err(error) => eprintln!("FAIL ferese-effects-v1 attachment: {error}"),
        }
    }

    fn view_layer(&self) -> Element<'_, cosmic::Action<Message>> {
        let status = format!(
            "Ferese shell spike  |  configured: {}  |  focused: {}  |  outputs: {}",
            self.configured, self.focused, self.outputs
        );
        let content = column![
            text(status),
            button::standard("Exit spike").on_press(cosmic::Action::App(Message::Exit))
        ]
        .spacing(4)
        .padding([4, 16]);

        container(content)
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
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
