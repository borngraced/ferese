mod compositor_ipc;
mod config;
mod control;
mod keybinding_guide;
mod motion;
mod note_store;
mod notification_ui;
mod notifications;
mod recording;
mod status;
mod status_ui;
mod system_modal;

use cosmic::Element;
use cosmic::app::{Core, Settings, Task};
use cosmic::iced::alignment;
use cosmic::iced::event::{self, PlatformSpecific, wayland};
use cosmic::iced::platform_specific::{
    runtime::wayland::layer_surface::{IcedMargin, IcedOutput, SctkLayerSurfaceSettings},
    shell::commands::layer_surface::{Anchor, KeyboardInteractivity, Layer},
    shell::commands::layer_surface::{
        destroy_layer_surface, set_anchor, set_exclusive_zone, set_input_zone, set_margin, set_size,
    },
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
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use wayland_client::{
    Connection, Dispatch, EventQueue, Proxy, QueueHandle, delegate_noop,
    globals::{GlobalListContents, registry_queue_init},
    protocol::{wl_output, wl_registry, wl_surface},
};

use crate::config::{ShellConfig, ShellTheme, WallpaperMode};
use crate::control::{ShellControl, ShellSnapshot};
use ferese_theme::calendar::format_bar_time;
use ferese_theme::calendar::stacked_digits as stacked_clock_digits;
use ferese_theme::icons::{accented as accented_icon, tinted as bar_icon};

const APP_ID: &str = "dev.ferese.Shell";

// Older libcosmic backends expose native surfaces in frame events. Keep
// that compatibility path gated; current backends use Window::Opened and
// window::run to retrieve handles without updates at the display refresh rate.
static EFFECT_FRAME_PENDING: AtomicBool = AtomicBool::new(true);
static SHELL_FONT: std::sync::OnceLock<std::sync::RwLock<cosmic::font::Font>> =
    std::sync::OnceLock::new();

fn configured_font(family: Option<&str>) -> cosmic::font::Font {
    ferese_theme::font(family)
}

fn shell_font() -> cosmic::font::Font {
    *SHELL_FONT
        .get_or_init(|| std::sync::RwLock::new(cosmic::font::default()))
        .read()
        .unwrap()
}

fn text<'a>(
    content: impl Into<std::borrow::Cow<'a, str>> + 'a,
) -> cosmic::widget::Text<'a, cosmic::Theme, cosmic::Renderer> {
    ferese_theme::text(content, shell_font())
}

#[derive(Clone, Copy)]
struct BarMetrics {
    height: f32,
    text_size: u16,
    icon_size: u16,
    overview_icon_size: u16,
    control_height: f32,
}

impl From<ShellTheme> for BarMetrics {
    fn from(theme: ShellTheme) -> Self {
        let height = theme.bar_height;

        Self {
            height,
            // Logical sizes: Wayland/iced applies each surface's output scale.
            // Compacting the bar must not also shrink its readable content.
            text_size: 14,
            icon_size: 20,
            overview_icon_size: 20,
            control_height: (height - 4.0).max(21.0).min(height),
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

    cosmic::iced::advanced::graphics::text::font_system()
        .write()
        .unwrap()
        .load_font(std::borrow::Cow::Borrowed(include_bytes!(
            "../../../assets/fonts/Comfortaa-Regular.otf"
        )));
    cosmic::iced::advanced::graphics::text::font_system()
        .write()
        .unwrap()
        .load_font(std::borrow::Cow::Borrowed(include_bytes!(
            "../../../assets/fonts/Cantarell-ExtraBold.otf"
        )));

    let mut config = config::load();
    motion::configure(config.animations, config.theme.material_radius);
    let compositor_wallpaper = std::env::var_os("FERESE_COMPOSITOR_WALLPAPER").is_some();
    if compositor_wallpaper {
        // The compositor owns one GPU image, shared across output renderers;
        // do not decode another copy or allocate full-screen software buffers.
        config.wallpaper.path = None;
    }
    let shell_font = configured_font(config.font_family.as_deref());
    let _ = SHELL_FONT.set(std::sync::RwLock::new(shell_font));
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
    settings = settings
        .default_font(shell_font)
        .theme(config.theme.palette().native_theme());

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
    desktop_clock: (String, String),
    outputs: Vec<OutputSurfaces>,
    notifications: notifications::Center,
    notification_surface: Option<notification_ui::NotificationSurface>,
    status_service: status::Service,
    status: status::Snapshot,
    recorder: recording::Recorder,
    calendar_offset: i32,
    status_error: Option<String>,
    menu: Option<status_ui::OpenMenu>,
    system_modal: Option<system_modal::SystemModal>,
    guide_shown: bool,
    guide_loading: bool,
    guide_attempts: u8,
    pending_power: Option<(window::Id, system_modal::PowerAction)>,
    note_editor: Option<DesktopNoteEditor>,
    note_drag: Option<NoteDrag>,
    note_pointer: std::collections::HashMap<window::Id, cosmic::iced::Point>,
    note_pending: Vec<note_store::Edit>,
    note_inflight: Vec<note_store::Edit>,
    note_saving: bool,
    note_error: Option<String>,
}

struct DesktopNoteEditor {
    id: String,
    content: cosmic::widget::text_editor::Content<cosmic::Renderer>,
    revision: u64,
}

struct NoteDrag {
    id: Option<String>,
    source: window::Id,
    overlay: window::Id,
    start: cosmic::iced::Point,
    origin: cosmic::iced::Point,
    position: cosmic::iced::Point,
    output_size: (i32, i32),
    obstacles: Vec<cosmic::iced::Rectangle>,
}

struct OutputSurfaces {
    output: wl_output::WlOutput,
    name: Option<String>,
    bar: window::Id,
    wallpaper: Option<window::Id>,
    clock: Option<window::Id>,
    notes: Vec<(String, window::Id)>,
    size: Option<(i32, i32)>,
    effects: Option<EffectsBinding>,
    hidden: bool,
}

#[derive(Clone, Debug)]
enum Message {
    BeginNoteEdit(String),
    NoteAction(String, cosmic::widget::text_editor::Action),
    SaveNote(String, u64),
    FinishNoteEdit,
    BeginNoteDrag(String, window::Id),
    BeginClockDrag(window::Id),
    NotesStored(Result<(), String>),
    WallpaperLoaded(Result<image::Handle, String>),
    Event(Event, window::Id),
    NativeSurface(
        window::Id,
        Result<(Connection, wl_surface::WlSurface), String>,
    ),
    Tick,
    ActivateWorkspace(u64),
    ToggleOverview,
    StatusUpdated(status::Update),
    StartRecording,
    StopRecording,
    NotificationEvent(notifications::Event),
    NotificationTick,
    DismissNotification(u32),
    RemoveNotification(u32),
    InvokeNotification(u32, String),
    HoverNotification(u32, bool),
    ToggleNotificationHistory,
    ClearNotifications,
    ToggleNotificationGroup(String),
    RemoveNotificationGroup(String),
    AnimateMenu,
    OpenMenu(status_ui::Menu, cosmic::iced::Rectangle<i32>),
    OpenMenuOn(window::Id, status_ui::Menu, cosmic::iced::Rectangle<i32>),
    Control(status::Action),
    ShowGuide,
    GuideLoaded(Result<Vec<keybinding_guide::Entry>, String>),
    SystemInhibitors(window::Id, Result<compositor_ipc::Approval, String>),
    ConfirmPower(status::Action),
    CancelPower,
    ExecutePower,
    AnimatePower,
    PowerCompleted(window::Id, Result<(), String>),
    CalendarMonth(i32),
    CalendarToday,
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
        let desktop_clock = config
            .desktop_widgets
            .clock
            .labels(&Zoned::now())
            .unwrap_or_default();
        let app = Self {
            core,
            bar_surface_id,
            notifications: notifications::Center::new(config.notifications.clone()),
            notification_surface: None,
            status_service: status::Service::start(config.status.settings_command.clone()),
            status: status::Snapshot::default(),
            recorder: recording::Recorder::default(),
            calendar_offset: 0,
            status_error: None,
            menu: None,
            system_modal: None,
            guide_shown: false,
            guide_loading: false,
            guide_attempts: 0,
            pending_power: None,
            note_editor: None,
            note_drag: None,
            note_pointer: Default::default(),
            note_pending: Vec::new(),
            note_inflight: Vec::new(),
            note_saving: false,
            note_error: None,
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
            desktop_clock,
            outputs: Vec::new(),
        };
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

        (app, wallpaper_task)
    }

    fn subscription(&self) -> Subscription<Self::Message> {
        Subscription::batch([
            if self
                .system_modal
                .as_ref()
                .is_some_and(|modal| modal.motion.animating() || modal.motion.closing())
            {
                cosmic::iced::time::every(Duration::from_millis(16)).map(|_| Message::AnimatePower)
            } else {
                Subscription::none()
            },
            self.notifications
                .subscription()
                .map(Message::NotificationEvent),
            if self.notifications.has_toasts() {
                cosmic::iced::time::every(Duration::from_millis(
                    if self.notifications.animating() {
                        16
                    } else {
                        250
                    },
                ))
                .map(|_| Message::NotificationTick)
            } else {
                Subscription::none()
            },
            event::listen_with(|event, _status, id| match &event {
                Event::PlatformSpecific(PlatformSpecific::Wayland(wayland::Event::Frame(..))) => {
                    EFFECT_FRAME_PENDING
                        .load(Ordering::Relaxed)
                        .then_some(Message::Event(event, id))
                }
                Event::Keyboard(_)
                | Event::Window(window::Event::Opened { .. } | window::Event::Closed)
                | Event::PlatformSpecific(PlatformSpecific::Wayland(
                    wayland::Event::Popup(..)
                    | wayland::Event::Layer(..)
                    | wayland::Event::Output(..),
                )) => Some(Message::Event(event, id)),
                _ => None,
            }),
            cosmic::iced::time::every(Duration::from_millis(500)).map(|_| Message::Tick),
            self.status_service
                .subscription()
                .map(Message::StatusUpdated),
            if self.config.desktop_widgets.clock.enabled
                || self
                    .config
                    .desktop_widgets
                    .notes
                    .iter()
                    .any(|note| note.enabled && note.interactive)
            {
                event::listen_with(|event, _, id| {
                    matches!(&event, Event::Mouse(_)).then_some(Message::Event(event, id))
                })
            } else {
                Subscription::none()
            },
            if self
                .menu
                .as_ref()
                .is_some_and(|menu| menu.animating() || menu.motion.closing())
            {
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
            if let Err(error) = effects.set_opacity(menu.progress()) {
                eprintln!("ferese-shell: could not update popup opacity: {error}");
            }

            let regions = menu.regions.lock().unwrap().clone();

            if let Err(error) = effects.set_regions(&regions) {
                eprintln!("ferese-shell: could not update card materials: {error}");
            }
        }

        if let Some(surface) = &self.notification_surface
            && let Some(effects) = &surface.effects
        {
            let regions = surface.regions.lock().unwrap().clone();

            if let Err(error) = effects.set_regions(&regions) {
                eprintln!("ferese-shell: could not update notification materials: {error}");
            }
        }

        self.update_power_materials();

        match message {
            Message::BeginNoteEdit(id) => {
                let mut tasks = vec![self.finish_note_edit()];

                if let Some(note) = self
                    .config
                    .desktop_widgets
                    .notes
                    .iter()
                    .find(|note| note.id == id)
                {
                    self.note_editor = Some(DesktopNoteEditor {
                        id,
                        content: cosmic::widget::text_editor::Content::with_text(&note.text),
                        revision: 0,
                    });
                }
                tasks.push(Task::none());
                Task::batch(tasks)
            }
            Message::FinishNoteEdit => self.finish_note_edit(),
            Message::NoteAction(id, action) => {
                if let Some(editor) = &mut self.note_editor
                    && editor.id == id
                {
                    let edited = action.is_edit();
                    editor.content.perform(action);

                    if edited {
                        editor.revision = editor.revision.wrapping_add(1);
                        let revision = editor.revision;
                        return cosmic::task::future(async move {
                            // iced executor is Tokio; do not spawn blocking sleep threads.
                            tokio::time::sleep(Duration::from_millis(500)).await;
                            Message::SaveNote(id, revision)
                        });
                    }
                }
                Task::none()
            }
            Message::SaveNote(id, revision) => {
                if let Some(editor) = &self.note_editor
                    && editor.id == id
                    && editor.revision == revision
                {
                    let value = editor.content.text();
                    self.set_note_text(&id, value)
                } else {
                    Task::none()
                }
            }
            Message::BeginNoteDrag(id, surface) => self.begin_note_drag(id, surface),
            Message::BeginClockDrag(surface) => self.begin_widget_drag(None, surface),
            Message::NotesStored(result) => {
                self.note_saving = false;
                match result {
                    Ok(()) => {
                        self.note_inflight.clear();
                        self.note_error = None;
                        self.flush_note_changes()
                    }
                    Err(error) => {
                        self.note_pending.splice(0..0, self.note_inflight.drain(..));
                        self.note_error = Some(error);
                        Task::none()
                    }
                }
            }
            Message::WallpaperLoaded(result) => {
                match result {
                    Ok(handle) => self.wallpaper = Some(handle),
                    Err(error) => eprintln!("ferese-shell: wallpaper unavailable: {error}"),
                }
                Task::none()
            }
            Message::NativeSurface(id, result) => {
                match result {
                    Ok((_connection, surface))
                        if self.outputs.iter().any(|entry| entry.bar == id) =>
                    {
                        self.attach_effects(id, &surface)
                    }
                    Ok((_connection, surface)) => {
                        self.attach_power_material(id, &surface);
                        if let Some(entry) = &mut self.notification_surface
                            && entry.id == id
                            && entry.effects.is_none()
                        {
                            match EffectsBinding::attach_role(&surface, None, 1.0) {
                                Ok(binding) => entry.effects = Some(binding),
                                Err(error) => eprintln!(
                                    "ferese-shell: notification effects unavailable: {error}"
                                ),
                            }
                        }

                        if let Some(menu) = &mut self.menu
                            && menu.id == id
                            && menu.effects.is_none()
                        {
                            match EffectsBinding::attach_role(
                                &surface,
                                menu.kind.material_role(),
                                menu.progress(),
                            ) {
                                Ok(binding) => menu.effects = Some(binding),
                                Err(error) => {
                                    eprintln!("ferese-shell: popover material unavailable: {error}")
                                }
                            }

                            if let Some(effects) = &menu.effects {
                                let _ = effects.set_opacity(menu.progress());
                            }
                        }
                    }
                    Err(error) => eprintln!("ferese-shell: native surface unavailable: {error}"),
                }
                Task::none()
            }
            Message::StartRecording => {
                self.recorder.start();
                if self.recorder.busy() {
                    self.close_menu()
                } else {
                    Task::none()
                }
            }
            Message::StopRecording => {
                self.recorder.stop();
                Task::none()
            }
            Message::StatusUpdated(update) => {
                if update.generation >= self.status_service.generation {
                    self.status = update.snapshot;
                    self.status_error = update.error;
                }
                self.notifications.tick();

                if self.notifications.ready {
                    self.status.notifications = Some(status::Notifications {
                        count: self.notifications.unread(),
                        dnd: self.notifications.dnd,
                    });
                }
                self.sync_notification_surface()
            }
            Message::NotificationEvent(event) => {
                self.notifications.handle_event(event);
                if self.notifications.ready {
                    self.status.notifications = Some(status::Notifications {
                        count: self.notifications.unread(),
                        dnd: self.notifications.dnd,
                    });
                }
                self.sync_notification_surface()
            }
            Message::NotificationTick => {
                self.notifications.tick();
                self.sync_notification_surface()
            }
            Message::DismissNotification(id) => {
                self.notifications.close(id, 2);
                self.sync_notification_surface()
            }
            Message::RemoveNotification(id) => {
                self.notifications.dismiss(id);
                self.sync_notification_surface()
            }
            Message::InvokeNotification(id, action) => {
                self.notifications.invoke(id, action);
                self.sync_notification_surface()
            }
            Message::HoverNotification(id, hovered) => {
                self.notifications.hover(id, hovered);
                self.sync_notification_surface()
            }
            Message::ToggleNotificationHistory => self.toggle_notification_history(),
            Message::ToggleNotificationGroup(app) => {
                self.notifications.toggle_group(app);
                self.sync_notification_surface()
            }
            Message::RemoveNotificationGroup(app) => {
                self.notifications.dismiss_group(&app);
                self.sync_notification_surface()
            }
            Message::ClearNotifications => {
                self.notifications.clear();
                self.sync_notification_surface()
            }
            Message::AnimateMenu => self.animate_menu(),
            Message::OpenMenu(kind, anchor) => self.open_menu(kind, anchor),
            Message::OpenMenuOn(id, kind, anchor) => {
                if self.bar_surface_id != id {
                    let destroy = self.destroy_menu();
                    self.bar_surface_id = id;
                    let open = self.open_menu(kind, anchor);
                    Task::batch([destroy, open])
                } else {
                    self.open_menu(kind, anchor)
                }
            }
            Message::ShowGuide => {
                if self.guide_shown
                    || self.guide_loading
                    || self.guide_attempts >= 5
                    || !self.config.status.keybinding_guide
                    || self.outputs.is_empty()
                {
                    return Task::none();
                }
                self.guide_loading = true;
                self.guide_attempts += 1;
                Task::perform(
                    async {
                        tokio::task::spawn_blocking(keybinding_guide::load)
                            .await
                            .map_err(|error| error.to_string())
                            .and_then(|result| result)
                    },
                    |result| cosmic::Action::App(Message::GuideLoaded(result)),
                )
            }
            Message::GuideLoaded(result) => {
                self.guide_loading = false;
                match result {
                    Ok(entries)
                        if self.config.status.keybinding_guide
                            && !self.outputs.is_empty()
                            && self.system_modal.is_none()
                            && self.pending_power.is_none() =>
                    {
                        self.guide_shown = true;
                        self.open_guide(entries)
                    }
                    result => {
                        if let Err(error) = result {
                            eprintln!("ferese-shell: keybinding guide unavailable: {error}");
                        }
                        if self.guide_attempts < 5
                            && self.config.status.keybinding_guide
                            && !self.guide_shown
                        {
                            Task::perform(
                                async {
                                    tokio::time::sleep(Duration::from_secs(2)).await;
                                },
                                |_| cosmic::Action::App(Message::ShowGuide),
                            )
                        } else {
                            Task::none()
                        }
                    }
                }
            }
            Message::SystemInhibitors(id, result) => self.set_system_inhibitors(id, result),
            Message::ConfirmPower(action) => {
                if let Some(action) = system_modal::PowerAction::from_status(action) {
                    return self.open_system_modal(action, None);
                }
                Task::none()
            }
            Message::ExecutePower => self.execute_system_modal(),
            Message::AnimatePower => self.animate_system_modal(),
            Message::PowerCompleted(id, result) => self.finish_power_action(id, result),
            Message::CalendarMonth(delta) => {
                self.calendar_offset = (self.calendar_offset + delta).clamp(-1200, 1200);
                Task::none()
            }
            Message::CalendarToday => {
                self.calendar_offset = 0;
                Task::none()
            }
            Message::CancelPower => self.close_system_modal(),
            Message::Control(action) => {
                if let Some(action) = system_modal::PowerAction::from_status(action.clone()) {
                    return self.open_system_modal(action, None);
                }

                if let status::Action::PowerProfile(profile) = action {
                    self.status_error = self
                        .status_service
                        .send(status::Action::PowerProfile(profile))
                        .err();

                    if self.status_error.is_none()
                        && let Some(profiles) = &mut self.status.power_profiles
                    {
                        profiles.active = profile.to_owned();
                    }
                    return Task::none();
                }
                if self.notifications.ready {
                    match action {
                        status::Action::Dnd(value) => {
                            self.notifications.dnd = value;
                            self.status.notifications = Some(status::Notifications {
                                count: self.notifications.unread(),
                                dnd: value,
                            });
                            return Task::none();
                        }
                        status::Action::Notifications => {
                            return Task::batch([
                                self.close_menu(),
                                self.toggle_notification_history(),
                            ]);
                        }
                        _ => {}
                    }
                }

                self.status_error = self.status_service.send(action.clone()).err();

                if self.status_error.is_none() {
                    self.optimistic_status(&action);
                }

                if matches!(
                    action,
                    status::Action::Notifications | status::Action::Settings
                ) {
                    return self.close_menu();
                }

                Task::none()
            }
            Message::Event(event, id) => self.handle_event(event, id),
            Message::Tick => {
                self.recorder.poll();
                self.clock = current_time();

                if self.config.desktop_widgets.clock.enabled {
                    self.desktop_clock = self
                        .config
                        .desktop_widgets
                        .clock
                        .labels(&Zoned::now())
                        .unwrap_or_default();
                }

                let mut reload_task = Task::none();
                if let Some(control) = &self.control {
                    let poll = control.poll();

                    if poll.disconnected {
                        return cosmic::iced::exit();
                    }

                    if let Some(active) = poll.overview_active {
                        self.overview_active = active;
                    }

                    if let Some(source) = poll.config {
                        reload_task = self.reload_config(source);
                    }

                    if let Some(snapshot) = poll.snapshot {
                        self.snapshot = snapshot;
                        let mut tasks = vec![reload_task];

                        for entry in &mut self.outputs {
                            let output = self
                                .snapshot
                                .outputs
                                .iter()
                                .find(|output| Some(output.name.as_str()) == entry.name.as_deref())
                                .map(|output| output.id);
                            let hidden = output_bar_hidden(&self.snapshot, output);

                            if entry.hidden == hidden {
                                continue;
                            }

                            entry.hidden = hidden;
                            if let Some(effects) = &entry.effects
                                && let Err(error) = effects.set_visible(!hidden)
                            {
                                eprintln!("ferese-shell: could not update panel material: {error}");
                            }
                            tasks.push(set_input_zone(
                                entry.bar,
                                if hidden { Some(Vec::new()) } else { None },
                            ));
                        }
                        if self.bar_hidden(self.bar_surface_id) {
                            tasks.push(self.destroy_menu());
                        }
                        reload_task = Task::batch(tasks);
                    }
                    for serial in poll.logout_cancelled {
                        reload_task = Task::batch([reload_task, self.cancel_logout_modal(serial)]);
                    }
                    if let Some((serial, output)) = poll.logout {
                        reload_task = Task::batch([
                            reload_task,
                            self.open_system_modal(
                                system_modal::PowerAction::Logout(serial),
                                Some(&output),
                            ),
                        ]);
                    }
                }
                reload_task
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
    fn reload_config(&mut self, source: String) -> Task<Message> {
        let mut config = match config::parse_source(&source) {
            Ok(config) => config,
            Err(error) => {
                eprintln!("ferese-shell: reload rejected; keeping current config: {error}");
                return Task::none();
            }
        };

        if std::env::var_os("FERESE_COMPOSITOR_WALLPAPER").is_some() {
            config.wallpaper.path = None;
        }

        if self.config.font_family != config.font_family {
            *SHELL_FONT
                .get_or_init(|| std::sync::RwLock::new(cosmic::font::default()))
                .write()
                .unwrap() = configured_font(config.font_family.as_deref());
        }
        motion::configure(config.animations, config.theme.material_radius);

        if self.config.animations != config.animations
            && let Some(menu) = &mut self.menu
        {
            menu.motion.update_settings(config.animations);
        }
        self.status_service
            .update_settings(config.status.settings_command.clone());

        let old = self.config.theme;
        let old_clock = &self.config.desktop_widgets.clock;
        let old_notes = &self.config.desktop_widgets.notes;
        let new_notes = &config.desktop_widgets.notes;
        let notes_changed = old_notes.len() != new_notes.len()
            || old_notes
                .iter()
                .zip(new_notes)
                .any(|(old, new)| !old.same_surface(new));
        let new_clock = &config.desktop_widgets.clock;
        let clock_changed = old_clock.enabled != new_clock.enabled
            || old_clock.outputs != new_clock.outputs
            || old_clock.anchor != new_clock.anchor
            || old_clock.width != new_clock.width
            || old_clock.height != new_clock.height
            || old_clock.margin_x != new_clock.margin_x
            || old_clock.margin_y != new_clock.margin_y;
        let theme = config.theme;
        let geometry_changed = old.bar_height != theme.bar_height
            || old.bar_margin_top != theme.bar_margin_top
            || old.bar_margin_horizontal != theme.bar_margin_horizontal
            || old.bar_window_gap != theme.bar_window_gap;
        self.notifications.configure(config.notifications.clone());
        self.config = config;
        self.desktop_clock = self
            .config
            .desktop_widgets
            .clock
            .labels(&Zoned::now())
            .unwrap_or_default();
        let mut tasks = vec![if clock_changed {
            self.rebuild_clocks(true)
        } else {
            Task::none()
        }];

        if !self.config.status.keybinding_guide
            && self
                .system_modal
                .as_ref()
                .is_some_and(|modal| modal.is_guide())
        {
            tasks.push(self.destroy_system_modal(false));
        }

        if old.palette() != theme.palette() {
            tasks.push(cosmic::command::set_theme(theme.palette().native_theme()));
        }

        if clock_changed
            && self
                .note_drag
                .as_ref()
                .is_some_and(|drag| drag.id.is_none())
        {
            tasks.push(self.finish_note_drag(false));
        }

        if notes_changed {
            tasks.push(self.finish_note_drag(false));
            if self.note_editor.as_ref().is_some_and(|editor| {
                !self
                    .config
                    .desktop_widgets
                    .notes
                    .iter()
                    .any(|note| note.id == editor.id && note.interactive)
            }) {
                self.note_editor = None;
            }
            tasks.push(self.rebuild_notes(true));
        }

        for entry in self.outputs.iter().filter(|_| geometry_changed) {
            tasks.push(set_size(
                entry.bar,
                None,
                Some(theme.bar_height.round() as u32),
            ));
            tasks.push(set_margin(
                entry.bar,
                theme.bar_margin_top,
                theme.bar_margin_horizontal,
                0,
                theme.bar_margin_horizontal,
            ));
            tasks.push(set_exclusive_zone(
                entry.bar,
                (theme.bar_height.round() as i32).saturating_add(theme.bar_window_gap),
            ));
        }

        Task::batch(tasks)
    }

    fn output_event(
        &mut self,
        event: wayland::OutputEvent,
        output: wl_output::WlOutput,
    ) -> Task<Message> {
        if matches!(event, wayland::OutputEvent::Removed) {
            if let Some(index) = self.outputs.iter().position(|entry| entry.output == output) {
                let entry = self.outputs.remove(index);
                let menu = if self.bar_surface_id == entry.bar {
                    self.destroy_menu()
                } else {
                    Task::none()
                };

                let mut tasks = vec![
                    destroy_layer_surface(entry.bar),
                    menu,
                    self.rebuild_system_modal(),
                ];
                if self.note_drag.as_ref().is_some_and(|drag| {
                    entry.clock == Some(drag.source)
                        || entry.notes.iter().any(|(_, id)| *id == drag.source)
                }) {
                    tasks.push(self.finish_note_drag(false));
                }

                if let Some(wallpaper) = entry.wallpaper {
                    tasks.push(destroy_layer_surface(wallpaper));
                }

                if let Some(clock) = entry.clock {
                    tasks.push(destroy_layer_surface(clock));
                }

                for (_, id) in entry.notes {
                    self.note_pointer.remove(&id);
                    tasks.push(destroy_layer_surface(id));
                }

                return Task::batch(tasks);
            }

            return Task::none();
        }

        let size = match &event {
            wayland::OutputEvent::Created(info) => info.as_ref().and_then(|info| info.logical_size),
            wayland::OutputEvent::InfoUpdate(info) => info.logical_size,
            _ => None,
        };
        let name = match event {
            wayland::OutputEvent::Created(info) => info.and_then(|info| info.name),
            wayland::OutputEvent::InfoUpdate(info) => info.name,
            _ => None,
        };

        if let Some(entry) = self.outputs.iter_mut().find(|entry| entry.output == output) {
            if size.is_some() {
                entry.size = size;
            }

            let changed = name.is_some() && entry.name != name;
            if name.is_some() {
                entry.name = name;
            }

            return if changed {
                Task::batch([self.rebuild_clocks(true), self.rebuild_notes(true)])
            } else {
                Task::none()
            };
        }

        let bar_surface_id = window::Id::unique();
        let wallpaper_surface_id = window::Id::unique();
        let shell_theme = self.config.theme;
        let bar = BarMetrics::from(shell_theme);
        let wallpaper_output = output.clone();
        let bar_output = output.clone();
        let hidden = output_bar_hidden(
            &self.snapshot,
            self.snapshot
                .outputs
                .iter()
                .find(|output| Some(output.name.as_str()) == name.as_deref())
                .map(|output| output.id),
        );
        self.outputs.push(OutputSurfaces {
            output,
            name,
            bar: bar_surface_id,
            wallpaper: std::env::var_os("FERESE_COMPOSITOR_WALLPAPER")
                .is_none()
                .then_some(wallpaper_surface_id),
            effects: None,
            clock: None,
            notes: Vec::new(),
            size,
            hidden,
        });
        let wallpaper_action = cosmic::surface::action::app_layer_shell::<Self>(
            |_| Default::default(),
            move |_| SctkLayerSurfaceSettings {
                id: wallpaper_surface_id,
                layer: Layer::Background,
                keyboard_interactivity: KeyboardInteractivity::None,
                input_zone: Some(Vec::new()),
                anchor: Anchor::TOP | Anchor::RIGHT | Anchor::BOTTOM | Anchor::LEFT,
                output: IcedOutput::Output(wallpaper_output.clone()),
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
                input_zone: hidden.then(Vec::new),
                layer: Layer::Top,
                keyboard_interactivity: KeyboardInteractivity::None,
                anchor: Anchor::TOP | Anchor::LEFT | Anchor::RIGHT,
                output: IcedOutput::Output(bar_output.clone()),
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
            },
            Some(Box::new(move |app| app.view_layer(bar_surface_id))),
        );

        // Submit the small interactive surface before the full-screen image.
        let mut surfaces = vec![bar_action];
        if std::env::var_os("FERESE_COMPOSITOR_WALLPAPER").is_none() {
            surfaces.push(wallpaper_action);
        }

        let tasks = surfaces
            .into_iter()
            .map(cosmic::Action::Surface)
            .map(cosmic::task::message);

        Task::batch([
            Task::batch(tasks),
            self.rebuild_clocks(false),
            self.rebuild_notes(false),
            self.rebuild_system_modal(),
            cosmic::task::message(cosmic::Action::App(Message::ShowGuide)),
        ])
    }

    fn flush_note_changes(&mut self) -> Task<Message> {
        if self.note_saving || self.note_pending.is_empty() {
            return Task::none();
        }

        let Some(path) = config::config_path() else {
            self.note_error = Some("Configuration path unavailable.".into());
            return Task::none();
        };
        self.note_saving = true;
        self.note_inflight = std::mem::take(&mut self.note_pending);
        let edits = self.note_inflight.clone();
        cosmic::task::future(async move {
            let (send, receive) = cosmic::iced::futures::channel::oneshot::channel();
            std::thread::spawn(move || {
                let _ = send.send(note_store::save(&path, &edits));
            });
            Message::NotesStored(
                receive
                    .await
                    .unwrap_or_else(|_| Err("Note save worker stopped.".into())),
            )
        })
    }

    fn set_note_text(&mut self, id: &str, value: String) -> Task<Message> {
        if let Some(note) = self
            .config
            .desktop_widgets
            .notes
            .iter_mut()
            .find(|note| note.id == id)
            && (note.text != value || self.note_error.is_some())
        {
            note.text = value.clone();
            self.note_pending
                .push(note_store::Edit::Text(id.to_owned(), value));
        }
        self.flush_note_changes()
    }

    fn finish_note_edit(&mut self) -> Task<Message> {
        if let Some(editor) = self.note_editor.take() {
            self.set_note_text(&editor.id, editor.content.text())
        } else {
            self.flush_note_changes()
        }
    }

    fn begin_note_drag(&mut self, id: String, surface: window::Id) -> Task<Message> {
        self.begin_widget_drag(Some(id), surface)
    }

    fn begin_widget_drag(&mut self, id: Option<String>, surface: window::Id) -> Task<Message> {
        if self.note_drag.is_some() {
            return Task::none();
        }
        let Some(entry) = self.outputs.iter().find(|entry| {
            entry.clock == Some(surface) || entry.notes.iter().any(|(_, window)| *window == surface)
        }) else {
            return Task::none();
        };
        let Some(size) = entry.size else {
            return Task::none();
        };
        let (origin, dimensions) = if let Some(id) = &id {
            let Some(note) = self
                .config
                .desktop_widgets
                .notes
                .iter()
                .find(|note| &note.id == id)
            else {
                return Task::none();
            };
            (note_origin(note, size), (note.width, note.height))
        } else {
            let clock = &self.config.desktop_widgets.clock;
            (
                widget_origin(
                    clock.anchor,
                    clock.margin_x,
                    clock.margin_y,
                    (clock.width, clock.height),
                    size,
                ),
                (clock.width, clock.height),
            )
        };
        let output = entry.output.clone();
        let mut obstacles = Vec::new();
        let clock = &self.config.desktop_widgets.clock;

        if id.is_some() && clock.on_output(entry.name.as_deref()) {
            let position = widget_origin(
                clock.anchor,
                clock.margin_x,
                clock.margin_y,
                (clock.width, clock.height),
                size,
            );
            obstacles.push(cosmic::iced::Rectangle::new(
                position,
                cosmic::iced::Size::new(clock.width as f32, clock.height as f32),
            ));
        }

        for note in &self.config.desktop_widgets.notes {
            if Some(&note.id) != id.as_ref() && note.on_output(entry.name.as_deref()) {
                obstacles.push(cosmic::iced::Rectangle::new(
                    note_origin(note, size),
                    cosmic::iced::Size::new(note.width as f32, note.height as f32),
                ));
            }
        }

        let overlay = window::Id::unique();
        self.note_drag = Some(NoteDrag {
            id,
            source: surface,
            overlay,
            origin,
            position: origin,
            start: self.note_pointer.get(&surface).copied().unwrap_or_default(),
            output_size: size,
            obstacles,
        });
        let action = cosmic::surface::action::app_layer_shell::<Self>(
            |_| Default::default(),
            move |_| SctkLayerSurfaceSettings {
                id: overlay,
                layer: Layer::Overlay,
                keyboard_interactivity: KeyboardInteractivity::None,
                input_zone: Some(Vec::new()),
                anchor: Anchor::TOP | Anchor::LEFT,
                margin: IcedMargin {
                    top: origin.y.round() as i32,
                    left: origin.x.round() as i32,
                    ..Default::default()
                },
                output: IcedOutput::Output(output.clone()),
                namespace: "ferese-shell-note-drag".into(),
                exclusive_zone: -1,
                size: Some((Some(dimensions.0), Some(dimensions.1))),
                size_limits: Limits::NONE,
            },
            Some(Box::new(Self::view_note_drag)),
        );

        Task::batch([
            self.finish_note_edit(),
            cosmic::task::message(cosmic::Action::Surface(action)),
        ])
    }

    fn view_note_drag(&self) -> Element<'_, cosmic::Action<Message>> {
        let Some(drag) = &self.note_drag else {
            return text("").into();
        };

        if let Some(id) = &drag.id {
            self.view_note_content(id, drag.source)
        } else {
            self.view_clock_content()
        }
    }

    fn finish_note_drag(&mut self, save: bool) -> Task<Message> {
        let Some(drag) = self.note_drag.take() else {
            return Task::none();
        };
        let mut tasks = vec![destroy_layer_surface(drag.overlay)];

        if save {
            if drag.id.is_none() {
                let clock = &mut self.config.desktop_widgets.clock;
                clock.anchor = ferese_core::desktop::Anchor::TopLeft;
                clock.margin_x = drag.position.x.round() as i32;
                clock.margin_y = drag.position.y.round() as i32;
                for entry in &self.outputs {
                    if let Some(id) = entry.clock {
                        tasks.push(set_anchor(id, Anchor::TOP | Anchor::LEFT));
                        tasks.push(set_margin(id, clock.margin_y, 0, 0, clock.margin_x));
                    }
                }
                self.note_pending.push(note_store::Edit::ClockPosition(
                    clock.margin_x,
                    clock.margin_y,
                ));
            }

            if let Some(note) = self
                .config
                .desktop_widgets
                .notes
                .iter_mut()
                .find(|note| Some(&note.id) == drag.id.as_ref())
            {
                note.anchor = ferese_core::desktop::Anchor::TopLeft;
                note.margin_x = drag.position.x.round() as i32;
                note.margin_y = drag.position.y.round() as i32;

                for entry in &self.outputs {
                    if let Some((_, id)) = entry
                        .notes
                        .iter()
                        .find(|(id, _)| Some(id) == drag.id.as_ref())
                    {
                        tasks.push(set_anchor(*id, Anchor::TOP | Anchor::LEFT));
                        tasks.push(set_margin(*id, note.margin_y, 0, 0, note.margin_x));
                    }
                }

                self.note_pending.push(note_store::Edit::Position(
                    note.id.clone(),
                    note.margin_x,
                    note.margin_y,
                ));
            }

            tasks.push(self.flush_note_changes());
        }
        Task::batch(tasks)
    }

    fn rebuild_notes(&mut self, reset: bool) -> Task<Message> {
        let mut tasks = Vec::new();

        for entry in &mut self.outputs {
            if reset {
                for (_, id) in entry.notes.drain(..) {
                    self.note_pointer.remove(&id);
                    tasks.push(destroy_layer_surface(id));
                }
            }

            for note in &self.config.desktop_widgets.notes {
                if !note.on_output(entry.name.as_deref())
                    || entry.notes.iter().any(|(id, _)| id == &note.id)
                {
                    continue;
                }

                let id = window::Id::unique();
                entry.notes.push((note.id.clone(), id));
                let note = note.clone();
                let note_id = note.id.clone();
                let output = entry.output.clone();
                let action = cosmic::surface::action::app_layer_shell::<Self>(
                    |_| Default::default(),
                    move |_| {
                        let (anchor, margin) =
                            widget_placement(note.anchor, note.margin_x, note.margin_y);
                        SctkLayerSurfaceSettings {
                            id,
                            layer: Layer::Bottom,
                            keyboard_interactivity: if note.interactive {
                                KeyboardInteractivity::OnDemand
                            } else {
                                KeyboardInteractivity::None
                            },
                            input_zone: if note.interactive {
                                None
                            } else {
                                Some(Vec::new())
                            },
                            anchor,
                            margin,
                            output: IcedOutput::Output(output.clone()),
                            namespace: "ferese-shell-sticky-note".into(),
                            exclusive_zone: -1,
                            size: Some((Some(note.width), Some(note.height))),
                            size_limits: Limits::NONE,
                        }
                    },
                    Some(Box::new(move |app| app.view_note(&note_id, id))),
                );

                tasks.push(cosmic::task::message(cosmic::Action::Surface(action)));
            }
        }

        Task::batch(tasks)
    }

    fn view_note(&self, id: &str, surface: window::Id) -> Element<'_, cosmic::Action<Message>> {
        if self
            .note_drag
            .as_ref()
            .is_some_and(|drag| drag.id.as_deref() == Some(id))
        {
            return container(text(""))
                .width(Length::Fill)
                .height(Length::Fill)
                .into();
        }

        self.view_note_content(id, surface)
    }

    fn view_note_content(
        &self,
        id: &str,
        surface: window::Id,
    ) -> Element<'_, cosmic::Action<Message>> {
        let Some(note) = self
            .config
            .desktop_widgets
            .notes
            .iter()
            .find(|note| note.id == id)
        else {
            return text("").into();
        };
        let alignment = match note.alignment {
            ferese_core::desktop::Alignment::Left => alignment::Horizontal::Left,
            ferese_core::desktop::Alignment::Center => alignment::Horizontal::Center,
            ferese_core::desktop::Alignment::Right => alignment::Horizontal::Right,
        };
        let font = configured_font(
            note.font_family
                .as_deref()
                .filter(|s| !s.trim().is_empty())
                .or(self.config.font_family.as_deref()),
        );
        let mut foreground = color(
            note.color
                .as_deref()
                .and_then(config::parse_color)
                .unwrap_or(self.config.theme.text_primary),
        );

        foreground.a *= note.opacity;

        let background = if note.background.as_deref() == Some("") {
            None
        } else {
            let mut tint = color(
                note.background
                    .as_deref()
                    .and_then(config::parse_color)
                    .unwrap_or(self.config.theme.surface_base),
            );
            tint.a *= note.opacity;
            Some(Background::Color(tint))
        };
        let mut body = cosmic::widget::column([])
            .spacing(note.gap)
            .align_x(alignment);

        if note.interactive {
            let title = cosmic::widget::text(if note.title.is_empty() {
                "⋮⋮"
            } else {
                &note.title
            })
            .font(font)
            .size(note.title_size)
            .class(theme::Text::Color(foreground));
            let header = cosmic::widget::mouse_area(container(title).width(Length::Fill))
                .interaction(cosmic::iced::mouse::Interaction::Grab)
                .on_press(cosmic::Action::App(Message::BeginNoteDrag(
                    id.to_owned(),
                    surface,
                )));
            let editing = self
                .note_editor
                .as_ref()
                .is_some_and(|editor| editor.id == id);

            body = body.push(
                row([]).push(header).push(motion::button(
                    button::custom(text(if editing { "Done" } else { "Edit" }))
                        .padding([4, 8])
                        .on_press(cosmic::Action::App(if editing {
                            Message::FinishNoteEdit
                        } else {
                            Message::BeginNoteEdit(id.to_owned())
                        })),
                    foreground,
                    false,
                    1.0,
                )),
            );
        } else if !note.title.is_empty() {
            body = body.push(
                cosmic::widget::text(note.title.clone())
                    .font(cosmic::font::Font {
                        weight: cosmic::iced::font::Weight::Semibold,
                        ..font
                    })
                    .size(note.title_size)
                    .width(Length::Fill)
                    .align_x(alignment)
                    .class(theme::Text::Color(foreground)),
            );
        }

        if let Some(editor) = &self.note_editor
            && editor.id == id
        {
            let id = id.to_owned();
            body = body.push(
                cosmic::widget::TextEditor::new(&editor.content)
                    .style(|theme, status| {
                        use cosmic::iced::widget::text_editor::Catalog;
                        let mut style = theme.style(&<cosmic::Theme as Catalog>::default(), status);
                        style.border.radius = motion::radius(f32::MAX).into();
                        style
                    })
                    .height(Length::Fill)
                    .font(font)
                    .size(note.text_size)
                    .on_action(move |action| {
                        cosmic::Action::App(Message::NoteAction(id.clone(), action))
                    }),
            );
        } else {
            body = body.push(
                cosmic::widget::text(note.text.clone())
                    .font(font)
                    .size(note.text_size)
                    .width(Length::Fill)
                    .align_x(alignment)
                    .class(theme::Text::Color(foreground)),
            );
        }

        if let Some(error) = &self.note_error {
            body = body.push(
                text(error.clone())
                    .size(10)
                    .class(theme::Text::Color(foreground)),
            );
        }

        let radius = self.config.theme.material_radius;

        container(body)
            .padding(note.padding)
            .width(Length::Fill)
            .height(Length::Fill)
            .class(theme::Container::custom(move |_| container::Style {
                background,
                border: Border {
                    radius: radius.into(),
                    ..Default::default()
                },
                ..Default::default()
            }))
            .into()
    }

    fn rebuild_clocks(&mut self, reset: bool) -> Task<Message> {
        self.desktop_clock = self
            .config
            .desktop_widgets
            .clock
            .labels(&Zoned::now())
            .unwrap_or_default();
        let mut tasks = Vec::new();

        for entry in &mut self.outputs {
            if !reset && entry.clock.is_some() {
                continue;
            }

            if let Some(id) = entry.clock.take() {
                self.note_pointer.remove(&id);
                tasks.push(destroy_layer_surface(id));
            }

            let clock = self.config.desktop_widgets.clock.clone();

            if !clock.on_output(entry.name.as_deref()) {
                continue;
            }

            let id = window::Id::unique();
            entry.clock = Some(id);
            let output = entry.output.clone();
            let action = cosmic::surface::action::app_layer_shell::<Self>(
                |_| Default::default(),
                move |_| {
                    let (anchor, margin) = clock_placement(&clock);
                    SctkLayerSurfaceSettings {
                        id,
                        layer: Layer::Bottom,
                        keyboard_interactivity: KeyboardInteractivity::OnDemand,
                        input_zone: None,
                        anchor,
                        margin,
                        output: IcedOutput::Output(output.clone()),
                        namespace: "ferese-shell-desktop-clock".into(),
                        exclusive_zone: -1,
                        size: Some((Some(clock.width), Some(clock.height))),
                        size_limits: Limits::NONE,
                    }
                },
                Some(Box::new(move |app| app.view_desktop_clock(id))),
            );

            tasks.push(cosmic::task::message(cosmic::Action::Surface(action)));
        }

        Task::batch(tasks)
    }

    fn view_desktop_clock(&self, id: window::Id) -> Element<'_, cosmic::Action<Message>> {
        if self
            .note_drag
            .as_ref()
            .is_some_and(|drag| drag.id.is_none() && drag.source == id)
        {
            return container(text(""))
                .width(Length::Fill)
                .height(Length::Fill)
                .into();
        }

        cosmic::widget::mouse_area(self.view_clock_content())
            .on_press(cosmic::Action::App(Message::BeginClockDrag(id)))
            .interaction(cosmic::iced::mouse::Interaction::Grab)
            .into()
    }

    fn view_clock_content(&self) -> Element<'_, cosmic::Action<Message>> {
        use ferese_core::desktop::Alignment as ClockAlignment;
        let clock = &self.config.desktop_widgets.clock;
        let alignment = match clock.alignment {
            ClockAlignment::Left => alignment::Horizontal::Left,
            ClockAlignment::Center => alignment::Horizontal::Center,
            ClockAlignment::Right => alignment::Horizontal::Right,
        };
        let pixel = clock.style == ferese_core::desktop::ClockStyle::Pixel;
        let custom_font = clock
            .font_family
            .as_deref()
            .filter(|family| !family.trim().is_empty());
        let mut font = configured_font(if pixel {
            custom_font.or(Some("Cantarell"))
        } else {
            custom_font.or(self.config.font_family.as_deref())
        });
        if pixel && custom_font.is_none() {
            font.weight = cosmic::iced::font::Weight::Black;
        } else if clock.bold {
            font.weight = cosmic::iced::font::Weight::Bold;
        }
        let date_font = configured_font(custom_font.or(self.config.font_family.as_deref()));
        let tint = |custom: &Option<String>, fallback| {
            let mut tint = color(
                custom
                    .as_deref()
                    .and_then(config::parse_color)
                    .unwrap_or(fallback),
            );
            tint.a *= clock.opacity;
            tint
        };
        let foreground = tint(&clock.color, self.config.theme.text_primary);
        let accent = color(self.config.theme.accent);
        let custom_color = clock
            .color
            .as_deref()
            .and_then(config::parse_color)
            .is_some();
        let digit_colors = [0.8, 0.45].map(|text_mix| {
            if custom_color {
                return foreground;
            }
            Color {
                r: foreground.r * text_mix + accent.r * (1.0 - text_mix),
                g: foreground.g * text_mix + accent.g * (1.0 - text_mix),
                b: foreground.b * text_mix + accent.b * (1.0 - text_mix),
                a: foreground.a,
            }
        });
        let mut labels = cosmic::widget::column([])
            .spacing(clock.gap)
            .align_x(alignment);

        if pixel && clock.show_date {
            labels = labels.push(
                cosmic::widget::text(self.desktop_clock.1.clone())
                    .font(date_font)
                    .size(clock.date_size)
                    .class(theme::Text::Color(tint(
                        &clock.date_color,
                        self.config.theme.text_primary,
                    ))),
            );
        }

        if let Some(digits) = pixel
            .then(|| stacked_clock_digits(&self.desktop_clock.0))
            .flatten()
        {
            let available_height = (clock.height as f32
                - clock.padding * 2.0
                - if clock.show_date {
                    clock.date_size * 1.3 + clock.gap
                } else {
                    0.0
                })
            .max(16.0);
            let size = clock
                .time_size
                .min(available_height / 1.6)
                .min((clock.width as f32 - clock.padding * 2.0).max(16.0) / 1.5);
            let mut stack = cosmic::widget::column([]).spacing(0).align_x(alignment);

            for (line, digits) in digits.into_iter().enumerate() {
                let mut digit_row = row([]).spacing(0);

                for (position, digit) in digits.chars().enumerate() {
                    digit_row = digit_row.push(
                        cosmic::widget::text(digit.to_string())
                            .font(font)
                            .size(size)
                            .line_height(cosmic::iced::widget::text::LineHeight::Relative(0.8))
                            .class(theme::Text::Color(digit_colors[(line + position) % 2])),
                    );
                }

                stack = stack.push(digit_row);
            }

            labels = labels.push(stack);
        } else {
            let characters = self.desktop_clock.0.chars().count().max(1) as f32;
            let available_width = (clock.width as f32 - clock.padding * 2.0).max(1.0);
            let available_height = (clock.height as f32
                - clock.padding * 2.0
                - if clock.show_date {
                    clock.date_size * 1.3 + clock.gap
                } else {
                    0.0
                })
            .max(1.0);
            let size = clock
                .time_size
                .min(available_width / (characters * 0.75))
                .min(available_height / 1.3);

            labels = labels.push(
                cosmic::widget::text(self.desktop_clock.0.clone())
                    .font(font)
                    .size(size)
                    .width(Length::Fill)
                    .align_x(alignment)
                    .class(theme::Text::Color(foreground)),
            );
        }

        if !pixel && clock.show_date {
            labels = labels.push(
                cosmic::widget::text(self.desktop_clock.1.clone())
                    .font(font)
                    .size(clock.date_size)
                    .width(Length::Fill)
                    .align_x(alignment)
                    .class(theme::Text::Color(tint(
                        &clock.date_color,
                        self.config.theme.text_muted,
                    ))),
            );
        }
        let background = clock
            .background
            .as_deref()
            .and_then(config::parse_color)
            .map(|rgba| {
                let mut tint = color(rgba);
                tint.a *= clock.opacity;
                Background::Color(tint)
            });
        let radius = self.config.theme.material_radius;

        container(labels)
            .padding(clock.padding)
            .width(Length::Fill)
            .height(Length::Fill)
            .align_x(alignment)
            .center_y(Length::Fill)
            .class(theme::Container::custom(move |_| container::Style {
                background,
                border: Border {
                    radius: radius.into(),
                    ..Default::default()
                },
                ..Default::default()
            }))
            .into()
    }

    fn output_for_bar(&self, id: window::Id) -> Option<&control::OutputSnapshot> {
        let name = self
            .outputs
            .iter()
            .find(|entry| entry.bar == id)?
            .name
            .as_deref()?;
        self.snapshot
            .outputs
            .iter()
            .find(|output| output.name == name)
    }

    fn bar_hidden(&self, id: window::Id) -> bool {
        output_bar_hidden(
            &self.snapshot,
            self.output_for_bar(id).map(|output| output.id),
        )
    }

    fn handle_event(&mut self, event: Event, id: window::Id) -> Task<Message> {
        if let Event::Mouse(mouse) = &event {
            if !self.outputs.iter().any(|entry| {
                entry.clock == Some(id) || entry.notes.iter().any(|(_, surface)| *surface == id)
            }) {
                return Task::none();
            }

            match mouse {
                cosmic::iced::mouse::Event::CursorMoved { position } => {
                    self.note_pointer.insert(id, *position);

                    if let Some(drag) = &mut self.note_drag
                        && drag.source == id
                    {
                        let delta = *position - drag.start;
                        let dimensions = if let Some(id) = &drag.id {
                            self.config
                                .desktop_widgets
                                .notes
                                .iter()
                                .find(|note| &note.id == id)
                                .map(|note| (note.width, note.height))
                        } else {
                            let clock = &self.config.desktop_widgets.clock;
                            Some((clock.width, clock.height))
                        };

                        if let Some(dimensions) = dimensions {
                            let requested = clamp_note_position(
                                drag.origin + delta,
                                drag.output_size,
                                dimensions,
                            );
                            drag.position = avoid_widget_overlap(
                                drag.position,
                                requested,
                                dimensions,
                                &drag.obstacles,
                            );
                            // Move a compact, cached buffer in the compositor instead of
                            // repainting an output-sized buffer for every pointer event.
                            return set_margin(
                                drag.overlay,
                                drag.position.y.round() as i32,
                                0,
                                0,
                                drag.position.x.round() as i32,
                            );
                        }
                    }
                }

                cosmic::iced::mouse::Event::ButtonReleased(cosmic::iced::mouse::Button::Left)
                    if self
                        .note_drag
                        .as_ref()
                        .is_some_and(|drag| drag.source == id) =>
                {
                    return self.finish_note_drag(true);
                }
                _ => {}
            }
        }
        if matches!(
            &event,
            Event::Keyboard(cosmic::iced::keyboard::Event::KeyPressed {
                key: cosmic::iced::keyboard::Key::Named(cosmic::iced::keyboard::key::Named::Escape),
                ..
            })
        ) {
            if self.system_modal.is_some() {
                return self.close_system_modal();
            }
            if self.note_drag.is_some() {
                return self.finish_note_drag(false);
            }

            if self.note_editor.is_some() {
                return self.finish_note_edit();
            }

            if self.notifications.history_open {
                return self.close_notification_history();
            }
        }

        match event {
            Event::PlatformSpecific(PlatformSpecific::Wayland(wayland::Event::Output(
                event,
                output,
            ))) => self.output_event(event, output),
            Event::Window(window::Event::Opened { .. })
                if self.outputs.iter().any(|entry| entry.bar == id)
                    || self.menu.as_ref().is_some_and(|menu| menu.id == id)
                    || self
                        .system_modal
                        .as_ref()
                        .is_some_and(|modal| modal.contains(id))
                    || self
                        .notification_surface
                        .as_ref()
                        .is_some_and(|surface| surface.id == id) =>
            {
                // Iced emits the first xdg_popup configure as Window::Opened,
                // not Popup::Configured. This is the popup's ready signal.
                if let Some(menu) = &mut self.menu
                    && menu.id == id
                {
                    menu.motion.begin(Instant::now());
                    if let Some(effects) = &menu.effects {
                        let _ = effects.set_opacity(menu.progress());
                    }
                }

                window::run(id, native_wayland_surface)
                    .map(move |surface| cosmic::Action::App(Message::NativeSurface(id, surface)))
            }
            Event::Keyboard(cosmic::iced::keyboard::Event::KeyPressed {
                key: cosmic::iced::keyboard::Key::Named(cosmic::iced::keyboard::key::Named::Escape),
                ..
            }) if self.menu.is_some() => self.close_menu(),
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
                            if menu.kind == status_ui::Menu::Notifications {
                                self.notifications.history_open = false;
                                self.notifications.hovered = None;
                            }
                            self.menu = None;
                            EFFECT_FRAME_PENDING.store(false, Ordering::Relaxed);
                            return self.sync_notification_surface();
                        }
                        wayland::PopupEvent::Focused => {
                            if menu.effects.is_none() {
                                match EffectsBinding::attach_role(
                                    &surface,
                                    menu.kind.material_role(),
                                    menu.progress(),
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
                            menu.motion.begin(Instant::now());
                            if menu.effects.is_none() {
                                match EffectsBinding::attach_role(
                                    &surface,
                                    menu.kind.material_role(),
                                    menu.progress(),
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
                    if let Some(effects) = &menu.effects {
                        let _ = effects.set_opacity(menu.progress());
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
            Event::Window(window::Event::Closed)
                if self
                    .system_modal
                    .as_ref()
                    .is_some_and(|modal| modal.contains(id)) =>
            {
                self.close_system_modal()
            }
            Event::Window(window::Event::Closed) => Task::none(),
            Event::PlatformSpecific(PlatformSpecific::Wayland(wayland::Event::Frame(
                _,
                surface,
                frame_id,
            ))) if self.menu.as_ref().is_some_and(|menu| menu.id == frame_id) => {
                if let Some(menu) = &mut self.menu
                    && menu.effects.is_none()
                {
                    match EffectsBinding::attach_role(
                        &surface,
                        menu.kind.material_role(),
                        menu.progress(),
                    ) {
                        Ok(binding) => menu.effects = Some(binding),
                        Err(error) => {
                            eprintln!("ferese-shell: popover material unavailable: {error}")
                        }
                    }
                    EFFECT_FRAME_PENDING.store(false, Ordering::Relaxed);
                }
                Task::none()
            }
            Event::PlatformSpecific(PlatformSpecific::Wayland(wayland::Event::Frame(
                _,
                surface,
                frame_id,
            ))) if self.outputs.iter().any(|entry| entry.bar == frame_id) => {
                self.attach_effects(frame_id, &surface);
                Task::none()
            }
            Event::PlatformSpecific(PlatformSpecific::Wayland(wayland::Event::Layer(
                _,
                surface,
                layer_id,
            ))) if self
                .notification_surface
                .as_ref()
                .is_some_and(|entry| entry.id == layer_id) =>
            {
                let entry = self.notification_surface.as_mut().unwrap();

                if entry.effects.is_none() {
                    match EffectsBinding::attach_role(&surface, None, 1.0) {
                        Ok(binding) => entry.effects = Some(binding),
                        Err(error) => {
                            eprintln!("ferese-shell: notification effects unavailable: {error}")
                        }
                    }
                }
                Task::none()
            }
            Event::PlatformSpecific(PlatformSpecific::Wayland(wayland::Event::Layer(
                _,
                surface,
                layer_id,
            ))) if self.outputs.iter().any(|entry| entry.bar == layer_id) => {
                self.attach_effects(layer_id, &surface);
                self.attach_power_material(layer_id, &surface);
                Task::none()
            }
            _ => Task::none(),
        }
    }

    fn attach_effects(&mut self, id: window::Id, surface: &wl_surface::WlSurface) {
        let hidden = self.bar_hidden(id);
        let Some(entry) = self.outputs.iter_mut().find(|entry| entry.bar == id) else {
            return;
        };

        if entry.effects.is_some() {
            return;
        }

        EFFECT_FRAME_PENDING.store(false, Ordering::Relaxed);

        match EffectsBinding::attach(surface, !hidden) {
            Ok(binding) => entry.effects = Some(binding),
            Err(error) => eprintln!("ferese-shell: panel material unavailable: {error}"),
        }
    }

    fn view_layer(&self, id: window::Id) -> Element<'_, cosmic::Action<Message>> {
        if self.bar_hidden(id) {
            return container(text(""))
                .width(Length::Fill)
                .height(Length::Fill)
                .into();
        }

        let focused_output = self.output_for_bar(id);
        let shell_theme = self.config.theme.for_bar();
        let bar = BarMetrics::from(shell_theme);
        let mut workspace_row = row::with_capacity(self.snapshot.workspaces.len() + 1)
            .spacing(3)
            .align_y(cosmic::iced::Alignment::Center);

        workspace_row = workspace_row.push(motion::button(
            button::custom(overview_control(bar, color(shell_theme.accent)))
                .height(bar.control_height)
                .padding([0, 7])
                .on_press(cosmic::Action::App(Message::ToggleOverview)),
            color(shell_theme.text_primary),
            self.overview_active,
            1.0,
        ));
        let mut workspace_buttons = row::with_capacity(self.snapshot.workspaces.len())
            .spacing(1)
            .align_y(cosmic::iced::Alignment::Center);

        for workspace in &self.snapshot.workspaces {
            let active = workspace_active_on_bar(workspace.id, focused_output);
            let owner = workspace
                .output
                .and_then(|id| self.snapshot.outputs.iter().find(|output| output.id == id));
            let occupied = self
                .snapshot
                .windows
                .iter()
                .any(|window| window.workspace == workspace.id);
            let indicator = workspace_indicator(
                &workspace.name,
                active,
                workspace.active && !active,
                occupied,
                bar,
                shell_theme,
            );
            let workspace_button = button::custom(indicator)
                .name(format!(
                    "Workspace {}{}{}",
                    workspace.name,
                    if active { ", active" } else { "" },
                    owner.map_or(String::new(), |output| format!(", on {}", output.name))
                ))
                .height(bar.control_height)
                .padding(0)
                .on_press(cosmic::Action::App(Message::ActivateWorkspace(
                    workspace.id,
                )));

            let selector = motion::button(
                workspace_button,
                color(if active {
                    shell_theme.accent
                } else {
                    shell_theme.text_primary
                }),
                false,
                1.0,
            );
            // Iced tooltips are overlays inside this bar-height layer surface;
            // viewport clamping puts them back over the selector. Keep the
            // accessible button name, without an overlay stealing its target.
            workspace_buttons = workspace_buttons.push(selector);
        }
        workspace_row = workspace_row.push(container(workspace_buttons).padding([0, 3]).class(
            theme::Container::custom(move |_| bar_group_style(shell_theme)),
        ));

        let foreground = color(shell_theme.text_primary);
        let left = cosmic::iced::widget::scrollable(workspace_row)
            .direction(cosmic::iced::widget::scrollable::Direction::Horizontal(
                cosmic::iced::widget::scrollable::Scrollbar::default()
                    .width(0)
                    .scroller_width(0),
            ))
            .width(Length::Fill)
            .height(bar.control_height);
        let (date, time) = self.clock.split_once(", ").unwrap_or(("", &self.clock));
        let clock = container(motion::button(
            button::custom(
                row![
                    text(date)
                        .size(12)
                        .class(theme::Text::Color(color(shell_theme.text_muted))),
                    text(time)
                        .size(bar.text_size)
                        .class(theme::Text::Color(foreground)),
                ]
                .spacing(8)
                .align_y(cosmic::iced::Alignment::Center),
            )
            .padding([4, 8])
            .height(bar.control_height)
            .name("Open calendar")
            .on_press_with_rectangle(move |offset, bounds| {
                cosmic::Action::App(Message::OpenMenuOn(
                    id,
                    status_ui::Menu::Calendar,
                    cosmic::iced::Rectangle {
                        x: (bounds.x - offset.x).round() as i32,
                        y: (bounds.y - offset.y).round() as i32,
                        width: bounds.width.round() as i32,
                        height: bounds.height.round() as i32,
                    },
                ))
            }),
            foreground,
            self.menu
                .as_ref()
                .is_some_and(|menu| menu.kind == status_ui::Menu::Calendar),
            1.0,
        ))
        .height(bar.control_height)
        .align_y(alignment::Vertical::Center)
        .class(theme::Container::custom(move |_| {
            bar_group_style(shell_theme)
        }));
        let right = row![
            self.view_status_bar().map(move |action| match action {
                cosmic::Action::App(Message::OpenMenu(kind, anchor)) =>
                    cosmic::Action::App(Message::OpenMenuOn(id, kind, anchor)),
                other => other,
            }),
            clock
        ]
        .spacing(8)
        .align_y(cosmic::iced::Alignment::Center);
        let available = self
            .outputs
            .iter()
            .find(|output| output.bar == id)
            .and_then(|output| output.size)
            .map_or(0., |(width, _)| width as f32)
            - 2. * (shell_theme.panel_padding + shell_theme.bar_margin_horizontal as f32);
        let title_width = (available - 920.).clamp(0., 360.);
        let title = if self.config.status.window_title && title_width > 0. {
            focused_bar_title(&self.snapshot, focused_output)
        } else {
            ""
        };
        let center = text(title)
            .size(bar.text_size)
            .width(title_width)
            .height(bar.control_height)
            .align_x(alignment::Horizontal::Center)
            .align_y(alignment::Vertical::Center)
            .wrapping(cosmic::iced::widget::text::Wrapping::None)
            .ellipsize(cosmic::iced::widget::text::Ellipsize::End(
                cosmic::iced::advanced::text::EllipsizeHeightLimit::Lines(1),
            ))
            .class(theme::Text::Color(foreground));
        let right = cosmic::iced::widget::scrollable(container(right).width(Length::Shrink))
            .direction(cosmic::iced::widget::scrollable::Direction::Horizontal(
                cosmic::iced::widget::scrollable::Scrollbar::default()
                    .width(0)
                    .scroller_width(0),
            ))
            .anchor_right()
            .width(Length::Shrink)
            .height(bar.control_height);
        let content = row![
            container(left).width(Length::Fill),
            center,
            container(right)
                .width(Length::Fill)
                .align_x(alignment::Horizontal::Right),
        ]
        .spacing(8)
        .align_y(cosmic::iced::Alignment::Center)
        .height(Length::Fill);

        let compositor_material = self
            .outputs
            .iter()
            .any(|entry| entry.bar == id && entry.effects.is_some());
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

fn overview_control(
    bar: BarMetrics,
    foreground: Color,
) -> Element<'static, cosmic::Action<Message>> {
    container(bar_icon(
        ferese_theme::icons::FERESE,
        bar.overview_icon_size,
        foreground,
    ))
    .width(bar.overview_icon_size)
    .height(bar.control_height)
    .align_x(alignment::Horizontal::Center)
    .align_y(alignment::Vertical::Center)
    .into()
}

#[cfg(test)]
fn bar_hidden(snapshot: &ShellSnapshot) -> bool {
    let output = snapshot
        .outputs
        .iter()
        .find(|output| output.focused)
        .or_else(|| snapshot.outputs.first())
        .map(|output| output.id);
    output_bar_hidden(snapshot, output)
}

fn output_bar_hidden(snapshot: &ShellSnapshot, output: Option<u64>) -> bool {
    let active_workspace = snapshot
        .outputs
        .iter()
        .find(|candidate| Some(candidate.id) == output)
        .map(|output| output.active_workspace);

    active_workspace.is_some_and(|workspace| {
        snapshot
            .windows
            .iter()
            .any(|window| window.workspace == workspace && window.fullscreen)
    })
}

fn workspace_indicator(
    name: &str,
    active: bool,
    active_elsewhere: bool,
    occupied: bool,
    bar: BarMetrics,
    shell_theme: ShellTheme,
) -> Element<'static, cosmic::Action<Message>> {
    let foreground = if active {
        shell_theme.accent
    } else if occupied {
        shell_theme.text_primary
    } else {
        shell_theme.text_muted
    };
    container(
        text(name.to_owned())
            .size(12)
            .class(theme::Text::Color(color(foreground))),
    )
    .width(24)
    .height(bar.control_height)
    .align_x(alignment::Horizontal::Center)
    .align_y(alignment::Vertical::Center)
    .class(theme::Container::custom(move |_| {
        workspace_selector_style(active, active_elsewhere, occupied, shell_theme)
    }))
    .into()
}

fn focused_bar_title<'a>(
    snapshot: &'a ShellSnapshot,
    output: Option<&control::OutputSnapshot>,
) -> &'a str {
    let Some(output) = output else {
        return "";
    };
    snapshot
        .windows
        .iter()
        .find(|window| window.focused && window.workspace == output.active_workspace)
        .map_or("", |window| {
            if window.title.trim().is_empty() {
                &window.app_id
            } else {
                &window.title
            }
        })
}

fn workspace_active_on_bar(workspace: u64, output: Option<&control::OutputSnapshot>) -> bool {
    output.is_some_and(|output| output.active_workspace == workspace)
}

fn workspace_selector_style(
    active: bool,
    active_elsewhere: bool,
    occupied: bool,
    shell_theme: ShellTheme,
) -> container::Style {
    container::Style {
        background: if active {
            Some(Background::Color(color_with_opacity(
                shell_theme.accent,
                0.16,
            )))
        } else if occupied {
            Some(Background::Color(color_with_opacity(
                shell_theme.text_primary,
                0.04,
            )))
        } else {
            None
        },
        border: Border {
            color: color_with_opacity(shell_theme.accent, 0.55),
            width: if active_elsewhere { 1.0 } else { 0.0 },
            radius: shell_theme.material_radius.min(14.0).into(),
        },
        ..Default::default()
    }
}

/// Sweep each axis, allowing edge sliding without tunnelling through a widget
/// when pointer events skip ahead. Coordinates match integer layer margins.
fn avoid_widget_overlap(
    previous: cosmic::iced::Point,
    requested: cosmic::iced::Point,
    size: (u32, u32),
    obstacles: &[cosmic::iced::Rectangle],
) -> cosmic::iced::Point {
    let mut position = previous;
    let (width, height) = (size.0 as f32, size.1 as f32);
    let mut x = requested.x.round();
    for obstacle in obstacles {
        if position.y < obstacle.y + obstacle.height && position.y + height > obstacle.y {
            if x > position.x && position.x + width <= obstacle.x {
                x = x.min((obstacle.x - width).floor());
            } else if x < position.x && position.x >= obstacle.x + obstacle.width {
                x = x.max((obstacle.x + obstacle.width).ceil());
            }
        }
    }
    position.x = x;
    let mut y = requested.y.round();
    for obstacle in obstacles {
        if position.x < obstacle.x + obstacle.width && position.x + width > obstacle.x {
            if y > position.y && position.y + height <= obstacle.y {
                y = y.min((obstacle.y - height).floor());
            } else if y < position.y && position.y >= obstacle.y + obstacle.height {
                y = y.max((obstacle.y + obstacle.height).ceil());
            }
        }
    }
    position.y = y;
    position
}

fn clamp_note_position(
    position: cosmic::iced::Point,
    output: (i32, i32),
    size: (u32, u32),
) -> cosmic::iced::Point {
    cosmic::iced::Point::new(
        position
            .x
            .clamp(0., (output.0 as f32 - size.0 as f32).clamp(0., 8192.)),
        position
            .y
            .clamp(0., (output.1 as f32 - size.1 as f32).clamp(0., 8192.)),
    )
}

fn note_origin(note: &ferese_core::desktop::StickyNote, output: (i32, i32)) -> cosmic::iced::Point {
    widget_origin(
        note.anchor,
        note.margin_x,
        note.margin_y,
        (note.width, note.height),
        output,
    )
}

fn widget_origin(
    anchor: ferese_core::desktop::Anchor,
    margin_x: i32,
    margin_y: i32,
    size: (u32, u32),
    output: (i32, i32),
) -> cosmic::iced::Point {
    use ferese_core::desktop::Anchor as A;
    let x = match anchor {
        A::TopLeft | A::CenterLeft | A::BottomLeft => margin_x as f32,
        A::TopRight | A::CenterRight | A::BottomRight => {
            output.0 as f32 - size.0 as f32 - margin_x as f32
        }
        _ => (output.0 as f32 - size.0 as f32) / 2.,
    };
    let y = match anchor {
        A::TopLeft | A::TopCenter | A::TopRight => margin_y as f32,
        A::BottomLeft | A::BottomCenter | A::BottomRight => {
            output.1 as f32 - size.1 as f32 - margin_y as f32
        }
        _ => (output.1 as f32 - size.1 as f32) / 2.,
    };
    clamp_note_position(cosmic::iced::Point::new(x, y), output, size)
}

fn clock_placement(clock: &ferese_core::desktop::Clock) -> (Anchor, IcedMargin) {
    widget_placement(clock.anchor, clock.margin_x, clock.margin_y)
}

fn widget_placement(
    position: ferese_core::desktop::Anchor,
    margin_x: i32,
    margin_y: i32,
) -> (Anchor, IcedMargin) {
    use ferese_core::desktop::Anchor as Position;
    let horizontal = match position {
        Position::TopLeft | Position::CenterLeft | Position::BottomLeft => Anchor::LEFT,
        Position::TopRight | Position::CenterRight | Position::BottomRight => Anchor::RIGHT,
        _ => Anchor::empty(),
    };
    let vertical = match position {
        Position::TopLeft | Position::TopCenter | Position::TopRight => Anchor::TOP,
        Position::BottomLeft | Position::BottomCenter | Position::BottomRight => Anchor::BOTTOM,
        _ => Anchor::empty(),
    };
    (
        horizontal | vertical,
        IcedMargin {
            top: if vertical == Anchor::TOP { margin_y } else { 0 },
            bottom: if vertical == Anchor::BOTTOM {
                margin_y
            } else {
                0
            },
            left: if horizontal == Anchor::LEFT {
                margin_x
            } else {
                0
            },
            right: if horizontal == Anchor::RIGHT {
                margin_x
            } else {
                0
            },
        },
    )
}

fn current_time() -> String {
    format_bar_time(&Zoned::now())
}

fn shell_surface_style(theme: ShellTheme) -> cosmic::iced::theme::Style {
    theme.palette().application_style(Color::TRANSPARENT)
}

fn bar_style(theme: ShellTheme, compositor_material: bool) -> container::Style {
    let mut style =
        ferese_theme::controls::surface_appearance(color(theme.bar_background), theme.bar_radius);
    if compositor_material {
        style.background = None;
    }
    style.text_color = Some(color(theme.text_primary));
    style.icon_color = style.text_color;
    style.snap = true;
    style
}

fn bar_group_style(theme: ShellTheme) -> container::Style {
    container::Style {
        background: Some(Background::Color(color_with_opacity(
            theme.text_primary,
            0.035,
        ))),
        border: Border {
            color: color_with_opacity(theme.text_muted, 0.12),
            width: 1.0,
            radius: theme.material_radius.min(16.0).into(),
        },
        ..Default::default()
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
    opacity: std::cell::Cell<Option<u32>>,
}

impl EffectsBinding {
    fn attach(
        surface: &wl_surface::WlSurface,
        visible: bool,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        Self::attach_role(
            surface,
            visible.then_some(ferese_surface_effects_v1::Role::Panel),
            if visible { 1.0 } else { 0.0 },
        )
    }

    fn attach_role(
        surface: &wl_surface::WlSurface,
        role: Option<ferese_surface_effects_v1::Role>,
        opacity: f32,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let backend = surface
            .backend()
            .upgrade()
            .ok_or("libcosmic Wayland connection is no longer alive")?;
        let connection = Connection::from_backend(backend);
        let (globals, queue) = registry_queue_init::<EffectsState>(&connection)?;
        let qh = queue.handle();
        let manager = globals.bind::<FereseEffectsManagerV1, _, _>(&qh, 1..=3, ())?;
        let effects = manager.get_surface_effects(surface, &qh, ());

        let opacity = (opacity.clamp(0.0, 1.0) * 1000.0).round() as u32;
        if effects.version() >= 3 {
            effects.set_opacity(opacity);
        }
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
            opacity: std::cell::Cell::new(Some(opacity)),
        })
    }

    fn set_regions(&self, regions: &[[i32; 5]]) -> Result<(), Box<dyn std::error::Error>> {
        self.set_material_regions(regions, ferese_surface_effects_v1::Role::Popover)
    }

    fn set_material_regions(
        &self,
        regions: &[[i32; 5]],
        role: ferese_surface_effects_v1::Role,
    ) -> Result<(), Box<dyn std::error::Error>> {
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
            self.surface.set_role(role);
        }
        self.connection.flush()?;
        *self.regions.borrow_mut() = Some(regions.to_vec());
        Ok(())
    }

    fn set_visible(&self, visible: bool) -> Result<(), Box<dyn std::error::Error>> {
        self.set_opacity(1.0)?;
        if visible {
            self.surface
                .set_role(ferese_surface_effects_v1::Role::Panel);
        } else {
            self.surface.clear_role();
        }

        self.connection.flush()?;
        Ok(())
    }

    fn set_opacity(&self, opacity: f32) -> Result<(), Box<dyn std::error::Error>> {
        let opacity = (opacity.clamp(0.0, 1.0) * 1000.0).round() as u32;
        if self.surface.version() >= 3 && self.opacity.get() != Some(opacity) {
            self.surface.set_opacity(opacity);
            self.connection.flush()?;
            self.opacity.set(Some(opacity));
        }
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
    fn widget_drag_stops_at_edges_without_tunnelling_and_slides() {
        use cosmic::iced::{Point, Rectangle};
        let obstacles = [Rectangle {
            x: 100.,
            y: 0.,
            width: 40.,
            height: 100.,
        }];
        assert_eq!(
            super::avoid_widget_overlap(
                Point::new(0., 0.),
                Point::new(300., 0.),
                (20, 20),
                &obstacles
            ),
            Point::new(80., 0.)
        );
        assert_eq!(
            super::avoid_widget_overlap(
                Point::new(80., 0.),
                Point::new(120., 50.),
                (20, 20),
                &obstacles
            ),
            Point::new(80., 50.)
        );
        assert_eq!(
            super::avoid_widget_overlap(
                Point::new(160., 0.),
                Point::new(0., 0.),
                (20, 20),
                &obstacles
            ),
            Point::new(140., 0.)
        );
        assert_eq!(
            super::avoid_widget_overlap(
                Point::new(100., 120.),
                Point::new(100., 0.),
                (20, 20),
                &obstacles
            ),
            Point::new(100., 100.)
        );
        assert_eq!(
            super::avoid_widget_overlap(
                Point::new(0., 100.),
                Point::new(200., 100.),
                (20, 20),
                &obstacles
            ),
            Point::new(200., 100.)
        );
    }

    #[test]
    fn note_drag_origin_and_bounds_use_logical_output_coordinates() {
        use cosmic::iced::Point;
        let mut note = ferese_core::desktop::StickyNote::default();
        assert_eq!(
            super::note_origin(&note, (1920, 1080)),
            Point::new(1552., 80.)
        );
        note.anchor = ferese_core::desktop::Anchor::Center;
        assert_eq!(
            super::note_origin(&note, (1920, 1080)),
            Point::new(800., 420.)
        );
        assert_eq!(
            super::clamp_note_position(Point::new(-20., 1200.), (1920, 1080), (320, 240)),
            Point::new(0., 840.)
        );
        assert_eq!(
            super::clamp_note_position(Point::new(20., 30.), (200, 100), (320, 240)),
            Point::ORIGIN
        );
    }

    #[test]
    fn desktop_clock_anchor_margins_match_only_anchored_edges() {
        use ferese_core::desktop::{Anchor as Position, Clock};
        let clock = Clock {
            anchor: Position::BottomRight,
            margin_x: 30,
            margin_y: 40,
            ..Clock::default()
        };
        let (anchor, margin) = super::clock_placement(&clock);
        assert_eq!(anchor, super::Anchor::BOTTOM | super::Anchor::RIGHT);
        assert_eq!(
            (margin.top, margin.right, margin.bottom, margin.left),
            (0, 30, 40, 0)
        );
        let (anchor, margin) = super::clock_placement(&Clock {
            anchor: Position::Center,
            ..clock
        });
        assert!(anchor.is_empty());
        assert_eq!(
            (margin.top, margin.right, margin.bottom, margin.left),
            (0, 0, 0, 0)
        );
    }

    #[test]
    fn shell_font_respects_configured_family() {
        assert_eq!(
            configured_font(Some("JetBrainsMono Nerd Font")),
            cosmic::font::Font::with_name("JetBrainsMono Nerd Font")
        );
        assert_eq!(configured_font(None), cosmic::font::default());
    }

    #[test]
    fn workspace_selector_has_distinct_active_paint() {
        let theme = ShellTheme::default();
        let active = workspace_selector_style(true, false, false, theme);
        let occupied = workspace_selector_style(false, false, true, theme);
        let inactive = workspace_selector_style(false, false, false, theme);
        let remote = workspace_selector_style(false, true, false, theme);
        assert!(active.background.is_some());
        assert_eq!(
            active.background,
            Some(Background::Color(color_with_opacity(theme.accent, 0.16)))
        );
        assert!(occupied.background.is_some());
        assert!(inactive.background.is_none());
        assert_eq!(inactive.border.width, 0.0);
        assert_eq!(remote.border.width, 1.0);
        assert!(remote.background.is_none());
    }

    #[test]
    fn workspace_highlight_tracks_the_bars_output() {
        let output = control::OutputSnapshot {
            id: 1,
            name: "eDP-1".into(),
            active_workspace: 2,
            focused: false,
        };
        assert!(workspace_active_on_bar(2, Some(&output)));
        assert!(!workspace_active_on_bar(1, Some(&output)));
        assert!(!workspace_active_on_bar(2, None));
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
            assert_eq!(metrics.text_size, 14);
            assert_eq!(metrics.icon_size, 20);
            assert_eq!(metrics.overview_icon_size, 20);
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

    #[test]
    fn fullscreen_visibility_is_local_to_each_monitor() {
        let mut snapshot = snapshot_with_fullscreen_window(7);
        snapshot.outputs.push(OutputSnapshot {
            id: 2,
            name: "external-test".to_owned(),
            active_workspace: 9,
            focused: false,
        });
        assert!(output_bar_hidden(&snapshot, Some(1)));
        assert!(!output_bar_hidden(&snapshot, Some(2)));
        assert!(!output_bar_hidden(&snapshot, None));
        snapshot.outputs[0].focused = false;
        snapshot.outputs[1].focused = true;
        assert!(output_bar_hidden(&snapshot, Some(1)));
        assert!(!output_bar_hidden(&snapshot, Some(2)));
    }

    #[test]
    fn bar_title_tracks_focus_and_the_outputs_active_workspace() {
        let mut snapshot = snapshot_with_fullscreen_window(7);
        assert_eq!(
            focused_bar_title(&snapshot, snapshot.outputs.first()),
            "Test"
        );
        snapshot.windows[0].focused = false;
        assert_eq!(focused_bar_title(&snapshot, snapshot.outputs.first()), "");
        snapshot.windows[0].focused = true;
        snapshot.windows[0].workspace = 8;
        assert_eq!(focused_bar_title(&snapshot, snapshot.outputs.first()), "");
        snapshot.windows[0].workspace = 7;
        snapshot.windows[0].title.clear();
        assert_eq!(
            focused_bar_title(&snapshot, snapshot.outputs.first()),
            "dev.ferese.Test"
        );
        assert_eq!(focused_bar_title(&snapshot, None), "");
    }

    fn snapshot_with_fullscreen_window(workspace: u64) -> ShellSnapshot {
        ShellSnapshot {
            outputs: vec![OutputSnapshot {
                id: 1,
                name: "eDP-1".to_owned(),
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
