mod animation;
mod floating;
mod focus;
mod hit_testing;
mod layout;
mod navigation;
mod outputs;
mod prediction;
pub(crate) use prediction::FrameScene;
mod window_registry;
mod windows;

use std::collections::{HashMap, HashSet};
use std::error::Error;
use std::ffi::OsString;
use std::sync::Arc;
use std::time::{Duration, Instant};

use calloop::LoopHandle;
use ferese_animation::{AnimatedValue, ClientSize, PresentationMode, SpringConfig, WindowGeometry};
use ferese_core::{
    LayoutMode, OutputGeometry, OutputId, OutputWorkspaceMap, WindowPlacement, WorkspaceId, WorkspaceLayout,
    WorkspaceSet,
};
use ferese_layout::{
    Axis, ColumnWidth, Direction, GapConfig, LayoutResult, Rect, SizeConstraints, ViewportFocusStrategy, WindowId,
};
use ferese_protocols::shell::v1::server::ferese_shell_v1::FereseShellV1;
use smithay::backend::allocator::dmabuf::Dmabuf;
use smithay::backend::drm::{DrmEventTime, DrmNode};
use smithay::backend::input::Keycode;
use smithay::backend::renderer::ImportDma;
use smithay::desktop::space::SpaceElement;
use smithay::desktop::utils::send_frames_surface_tree;
use smithay::desktop::{LayerSurface, PopupKind, PopupManager, Space, Window, WindowSurfaceType, layer_map_for_output};
use smithay::input::keyboard::XkbConfig;
use smithay::input::pointer::{CursorIcon, CursorImageStatus};
use smithay::input::{Seat, SeatState};
use smithay::output::Output;
use smithay::reexports::calloop::generic::Generic;
use smithay::reexports::calloop::{EventLoop, Interest, LoopSignal, Mode, PostAction};
use smithay::reexports::drm::control::crtc;
use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel;
use smithay::reexports::wayland_server::backend::{ClientData, ClientId, DisconnectReason, ObjectId};
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::reexports::wayland_server::{Display, DisplayHandle, Weak};
use smithay::utils::{Logical, Point, Rectangle, Size};
use smithay::wayland::alpha_modifier::AlphaModifierState;
use smithay::wayland::compositor::{CompositorClientState, CompositorState, with_states};
use smithay::wayland::cursor_shape::CursorShapeManagerState;
use smithay::wayland::dmabuf::{DmabufState, ImportNotifier};
use smithay::wayland::fractional_scale::FractionalScaleManagerState;
use smithay::wayland::idle_inhibit::IdleInhibitManagerState;
use smithay::wayland::idle_notify::IdleNotifierState;
use smithay::wayland::input_method::InputMethodManagerState;
use smithay::wayland::keyboard_shortcuts_inhibit::{KeyboardShortcutsInhibitState, KeyboardShortcutsInhibitor};
use smithay::wayland::output::OutputManagerState;
use smithay::wayland::pointer_constraints::PointerConstraintsState;
use smithay::wayland::presentation::PresentationState;
use smithay::wayland::relative_pointer::RelativePointerManagerState;
use smithay::wayland::selection::data_device::DataDeviceState;
use smithay::wayland::selection::primary_selection::PrimarySelectionState;
use smithay::wayland::selection::{ext_data_control, wlr_data_control};
use smithay::wayland::session_lock::SessionLockManagerState;
use smithay::wayland::shell::wlr_layer::{Layer, WlrLayerShellState};
use smithay::wayland::shell::xdg::decoration::XdgDecorationState;
use smithay::wayland::shell::xdg::{SurfaceCachedState, XdgShellState, XdgToplevelSurfaceData};
use smithay::wayland::shm::ShmState;
use smithay::wayland::single_pixel_buffer::SinglePixelBufferState;
use smithay::wayland::socket::ListeningSocketSource;
use smithay::wayland::text_input::TextInputManagerState;
use smithay::wayland::viewporter::ViewporterState;
use smithay::wayland::xdg_activation::XdgActivationState;
use smithay::wayland::xdg_foreign::XdgForeignState;
use smithay::wayland::xdg_toplevel_icon::XdgToplevelIconManager;
use window_registry::WindowRegistry;

use crate::backends::direct::DirectBackendState;
use crate::config::{Binding, DaemonConfig, InputSettings, OutputProfile, ThemeSettings};
use crate::cursor::{NamedCursor, cursor_theme, load_named_cursor};
use crate::dimming::DimAnimation;
use crate::gestures::{Swipe, SwipeDirection};
use crate::handlers::screencopy::PendingScreencopy;
use crate::handlers::screenshot::{Coordinator, PartSender};
use crate::handlers::screenshot_worker::Worker;
use crate::input::LockedPointerHint;
use crate::input_capture::InputCapture;
use crate::ipc::IpcSocketGuard;
use crate::overview::OverviewState;
use crate::portal_session::PortalSession;
use crate::portal_shortcuts::PortalShortcuts;
use crate::session_lock::{IdleSettings, Lock};
use crate::shell_control::ShellSnapshot;
use crate::stacking::WindowStack;
use crate::wallpaper::{WallpaperConfig, WallpaperState};
use crate::window_rules::{WindowRule, resolve as resolve_window_rules};
use crate::winit::NestedBackend;

const CLOSE_ANIMATION_DURATION: Duration = Duration::from_millis(140);
const WORKSPACE_SLIDE_DURATION: Duration = Duration::from_millis(220);
const WORKSPACE_SLIDE_FIRST_FRAME: Duration = Duration::from_millis(8);

fn workspace_swipe_is_current(
    outputs: &OutputWorkspaceMap,
    output: OutputId,
    from: WorkspaceId,
    to: WorkspaceId,
) -> bool {
    outputs.focused_output() == Some(output)
        && outputs.active_workspace(output) == Some(from)
        && outputs.output_for_workspace(to).is_none_or(|owner| owner == output)
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct SlideOffset {
    x: f64,
    y: f64,
}

impl SlideOffset {
    fn for_swipe(direction: SwipeDirection) -> Self {
        match direction {
            SwipeDirection::Left => Self { x: -1.0, y: 0.0 },
            SwipeDirection::Right => Self { x: 1.0, y: 0.0 },
            SwipeDirection::Up => Self { x: 0.0, y: -1.0 },
            SwipeDirection::Down => Self { x: 0.0, y: 1.0 },
        }
    }

    fn opposite(self) -> Self {
        Self { x: -self.x, y: -self.y }
    }

    fn between(self, target: Self, progress: f64) -> Self {
        Self {
            x: self.x + (target.x - self.x) * progress,
            y: self.y + (target.y - self.y) * progress,
        }
    }
}

struct FocusSwipe {
    workspace: WorkspaceId,
    from: WindowId,
    to: WindowId,
    direction: Direction,
    gesture_direction: SwipeDirection,
    start: f64,
    destination: f64,
    progress: f64,
}

impl FocusSwipe {
    fn position(&self) -> f64 {
        let distance = self.destination - self.start;
        // When both columns already fit, give bounded drag feedback and settle
        // back to the unchanged viewport after selecting the next window.
        let distance = if distance.abs() < 0.001 {
            if self.direction == Direction::Right {
                64.0
            } else {
                -64.0
            }
        } else {
            distance
        };
        self.start + distance * self.progress
    }
}

#[derive(Clone)]
struct WorkspaceSlideItem {
    workspace: WorkspaceId,
    start: SlideOffset,
    target: SlideOffset,
}

#[derive(Clone)]
struct WorkspaceSlide {
    items: Vec<WorkspaceSlideItem>,
    elapsed: Duration,
    held_progress: Option<f64>,
    gesture: Option<(WorkspaceId, WorkspaceId, SwipeDirection)>,
}

impl WorkspaceSlide {
    fn new(previous: Option<Self>, from: WorkspaceId, to: WorkspaceId, direction: SwipeDirection) -> Self {
        let movement = SlideOffset::for_swipe(direction);
        let mut items = previous
            .map(|slide| {
                let progress = slide.progress();
                slide
                    .items
                    .into_iter()
                    .map(|mut item| {
                        item.start = item.start.between(item.target, progress);
                        item
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();

        if let Some(item) = items.iter_mut().find(|item| item.workspace == from) {
            item.target = movement;
        } else {
            items.push(WorkspaceSlideItem {
                workspace: from,
                start: SlideOffset::default(),
                target: movement,
            });
        }
        if let Some(item) = items.iter_mut().find(|item| item.workspace == to) {
            item.target = SlideOffset::default();
        } else {
            items.push(WorkspaceSlideItem {
                workspace: to,
                start: movement.opposite(),
                target: SlideOffset::default(),
            });
        }

        Self {
            items,
            elapsed: WORKSPACE_SLIDE_FIRST_FRAME,
            held_progress: None,
            gesture: None,
        }
    }

    fn progress(&self) -> f64 {
        self.held_progress.unwrap_or_else(|| {
            smoothstep((self.elapsed.as_secs_f64() / WORKSPACE_SLIDE_DURATION.as_secs_f64()).min(1.0))
        })
    }

    fn release(&mut self, committed: bool) {
        let progress = self.progress();
        for item in &mut self.items {
            let position = item.start.between(item.target, progress);
            if !committed && let Some((from, to, direction)) = self.gesture {
                if item.workspace == from {
                    item.target = SlideOffset::default();
                } else if item.workspace == to {
                    item.target = SlideOffset::for_swipe(direction).opposite();
                }
            }
            item.start = position;
        }
        self.held_progress = None;
        self.gesture = None;
        self.elapsed = Duration::ZERO;
    }

    fn advance(&mut self, delta: Duration) -> bool {
        self.elapsed = (self.elapsed + delta).min(WORKSPACE_SLIDE_DURATION);
        self.elapsed < WORKSPACE_SLIDE_DURATION
    }

    fn contains(&self, workspace: WorkspaceId) -> bool {
        self.items.iter().any(|item| item.workspace == workspace)
    }

    fn offset(&self, workspace: WorkspaceId, width: f64, height: f64) -> (f64, f64) {
        let progress = self.progress();
        let position = self
            .items
            .iter()
            .find(|item| item.workspace == workspace)
            .map(|item| item.start.between(item.target, progress))
            .unwrap_or_default();
        (position.x * width, position.y * height)
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct ClosingAnimation {
    progress: f64,
    close_sent: bool,
}

impl ClosingAnimation {
    fn advance(&mut self, delta: Duration, animations_enabled: bool) -> bool {
        if self.close_sent {
            return false;
        }

        self.progress = if animations_enabled {
            (self.progress + delta.as_secs_f64() / CLOSE_ANIMATION_DURATION.as_secs_f64()).min(1.0)
        } else {
            1.0
        };
        if self.progress < 1.0 {
            return false;
        }

        self.close_sent = true;
        true
    }
}

fn scaled_animation_duration(duration: Duration, enabled: bool, speed: f64) -> Duration {
    if enabled {
        duration.div_f64(speed)
    } else {
        Duration::ZERO
    }
}

pub struct Ferese {
    pub(crate) session_lock_state: SessionLockManagerState,
    pub(crate) session_lock: Lock,
    pub(crate) lock_idle: IdleSettings,
    pub(crate) config_source: Option<String>,
    pub(crate) theme_engine: crate::theme::Engine,
    pub start_time: Instant,
    pub socket_name: OsString,
    pub display_handle: DisplayHandle,
    pub loop_signal: LoopSignal,
    pub(crate) loop_handle: LoopHandle<'static, Self>,
    pub(crate) input_capture: InputCapture,
    pub(crate) portal_session: PortalSession,
    pub space: Space<Window>,
    pub workspaces: WorkspaceSet,
    pub output_workspaces: OutputWorkspaceMap,
    pub(crate) output_ids: HashMap<Output, OutputId>,
    output_identity_ids: HashMap<String, OutputId>,
    pub(crate) windows: WindowRegistry<Window>,
    pub(crate) render: crate::render::RenderResources,
    pub(crate) nested_backend: Option<NestedBackend>,
    pub(crate) wallpaper: WallpaperState,
    window_stack: WindowStack,
    floating_above_fullscreen: HashMap<WindowId, WindowId>,
    floating_memory: crate::floating::Memory,
    floating_save_worker: crate::floating::SaveWorker,
    floating_cascade: crate::floating::Cascade,
    pub(crate) portal_shortcuts: PortalShortcuts,
    pub(crate) pending_logout: Option<u32>,
    pub(crate) logout_query: Option<u32>,
    pub(crate) logout_owner: Option<ObjectId>,
    pub(crate) backdrop_generation: u64,
    viewport_animations: HashMap<WorkspaceId, AnimatedValue>,
    focus_swipe: Option<FocusSwipe>,
    workspace_slides: HashMap<OutputId, WorkspaceSlide>,
    pub focused_window: Option<WindowId>,
    pub(crate) focus_history: ferese_core::FocusHistory,
    pub(crate) focus_cycle: Option<ferese_core::FocusCycle>,
    column_width_presets: Vec<ColumnWidth>,
    gap_config: GapConfig,
    pub(crate) input_settings: InputSettings,
    pub(crate) bindings: Vec<Binding>,
    workspace_auto_back_and_forth: bool,
    window_rules: Vec<WindowRule>,
    pub(crate) theme_settings: ThemeSettings,
    pub(crate) inactive_dim: crate::config::InactiveDimSettings,
    animations_enabled: bool,
    animation_speed: f64,
    spring_config: SpringConfig,
    viewport_spring_config: SpringConfig,
    pub(crate) output_profiles: Vec<OutputProfile>,
    pub(crate) autostart: Vec<DaemonConfig>,
    pub cursor_status: CursorImageStatus,
    // Cursor callbacks may run with Smithay's pointer mutex held. Rendering
    // reads the pointer position, so defer it until event dispatch returns.
    pub(crate) cursor_redraw_pending: bool,
    pub(crate) locked_pointer_hint: Option<LockedPointerHint>,
    pub(crate) last_pointer_time: u32,
    pub(crate) cursor_theme: xcursor::CursorTheme,
    pub(crate) named_cursors: HashMap<CursorIcon, NamedCursor>,
    pub intercepted_keys: HashSet<Keycode>,
    pub(crate) swipe: Swipe,
    pub idle_inhibitors: HashMap<WlSurface, usize>,
    pub active_shortcuts_inhibitor: Option<KeyboardShortcutsInhibitor>,
    pub direct_backend: Option<DirectBackendState>,
    _ipc_socket: Option<IpcSocketGuard>,
    pending_dmabuf_imports: Vec<(Dmabuf, ImportNotifier)>,
    pub(crate) pending_screencopies: Vec<PendingScreencopy>,
    // Screenshot requests outlive the readback that filled them: the
    // coordinator owns them until encoding finishes and the caller is
    // answered, or the request is terminated.
    pub(crate) screenshot: Coordinator,
    pub(crate) screenshot_parts: Option<PartSender>,
    pub(crate) screenshot_worker: Option<Worker>,
    pub(crate) shell_resources: Vec<Weak<FereseShellV1>>,
    pub(crate) shell_snapshot_serial: u32,
    pub(crate) last_shell_snapshot: Option<ShellSnapshot>,
    pub(crate) overview: crate::overview::OverviewState,
    next_output_id: u64,
    last_animation_tick: Instant,
    pub popups: PopupManager,
    pub(crate) dismissing_popups: Vec<(WlSurface, PopupKind, DimAnimation)>,
    pub seat: Seat<Self>,
    pub alpha_modifier_state: AlphaModifierState,
    pub compositor_state: CompositorState,
    pub cursor_shape_state: CursorShapeManagerState,
    pub data_device_state: DataDeviceState,
    pub decoration_state: XdgDecorationState,
    pub dmabuf_state: DmabufState,
    pub fractional_scale_state: FractionalScaleManagerState,
    pub idle_inhibit_state: IdleInhibitManagerState,
    pub idle_notifier_state: IdleNotifierState<Self>,
    pub input_method_manager_state: InputMethodManagerState,
    pub keyboard_shortcuts_inhibit_state: KeyboardShortcutsInhibitState,
    pub output_manager_state: OutputManagerState,
    pub pointer_constraints_state: PointerConstraintsState,
    pub presentation_state: PresentationState,
    pub primary_selection_state: PrimarySelectionState,
    pub wlr_data_control_state: wlr_data_control::DataControlState,
    pub ext_data_control_state: ext_data_control::DataControlState,
    pub relative_pointer_state: RelativePointerManagerState,
    pub seat_state: SeatState<Self>,
    pub shm_state: ShmState,
    pub single_pixel_buffer_state: SinglePixelBufferState,
    pub text_input_manager_state: TextInputManagerState,
    pub viewporter_state: ViewporterState,
    pub layer_shell_state: WlrLayerShellState,
    pub xdg_activation_state: XdgActivationState,
    pub xdg_foreign_state: XdgForeignState,
    pub xdg_shell_state: XdgShellState,
    pub xdg_toplevel_icon_manager: XdgToplevelIconManager,
}

pub struct RuntimeConfig {
    pub(crate) lock_idle: IdleSettings,
    pub(crate) autostart: Vec<DaemonConfig>,
    pub(crate) overview_font_family: String,
    pub(crate) wallpaper: WallpaperConfig,
    pub layout_mode: LayoutMode,
    pub gap_config: GapConfig,
    pub input_settings: InputSettings,
    pub bindings: Vec<Binding>,
    pub workspace_auto_back_and_forth: bool,
    pub window_rules: Vec<WindowRule>,
    pub theme_settings: ThemeSettings,
    pub inactive_dim: crate::config::InactiveDimSettings,
    pub default_column_width: ColumnWidth,
    pub scrolling_focus_strategy: ViewportFocusStrategy,
    pub column_width_presets: Vec<ColumnWidth>,
    pub animations_enabled: bool,
    pub animation_speed: f64,
    pub spring_config: SpringConfig,
    pub viewport_spring_config: SpringConfig,
    pub output_profiles: Vec<OutputProfile>,
}

impl Ferese {
    pub fn new(
        event_loop: &mut EventLoop<'static, Self>,
        display: Display<Self>,
        config: RuntimeConfig,
    ) -> Result<Self, Box<dyn Error>> {
        let display_handle = display.handle();

        crate::handlers::screencopy::init_global(&display_handle);
        crate::effects::init_global(&display_handle);
        crate::shell_control::init_global(&display_handle);

        let alpha_modifier_state = AlphaModifierState::new::<Self>(&display_handle);
        let compositor_state = CompositorState::new_v6::<Self>(&display_handle);
        let cursor_shape_state = CursorShapeManagerState::new::<Self>(&display_handle);
        let xdg_shell_state = XdgShellState::new::<Self>(&display_handle);
        let decoration_state = XdgDecorationState::new::<Self>(&display_handle);
        let dmabuf_state = DmabufState::new();
        let fractional_scale_state = FractionalScaleManagerState::new::<Self>(&display_handle);
        let idle_inhibit_state = IdleInhibitManagerState::new::<Self>(&display_handle);
        let idle_notifier_state = IdleNotifierState::new(&display_handle, event_loop.handle());
        let session_lock_state =
            smithay::wayland::session_lock::SessionLockManagerState::new::<Self, _>(&display_handle, |_| true);
        let input_method_enabled = std::env::var_os("FERESE_ENABLE_INPUT_METHOD").is_some_and(|value| value == "1");
        let input_method_manager_state =
            InputMethodManagerState::new::<Self, _>(&display_handle, move |_| input_method_enabled);
        let keyboard_shortcuts_inhibit_state = KeyboardShortcutsInhibitState::new::<Self>(&display_handle);
        let shortcut_inhibit_enabled =
            std::env::var_os("FERESE_ENABLE_SHORTCUT_INHIBIT").is_some_and(|value| value == "1");
        if !shortcut_inhibit_enabled {
            display_handle.disable_global::<Self>(keyboard_shortcuts_inhibit_state.global());
        }
        let shm_state = ShmState::new::<Self>(&display_handle, Vec::new());
        let single_pixel_buffer_state = SinglePixelBufferState::new::<Self>(&display_handle);
        let text_input_manager_state = TextInputManagerState::new::<Self>(&display_handle);
        let output_manager_state = OutputManagerState::new_with_xdg_output::<Self>(&display_handle);
        let pointer_constraints_state = PointerConstraintsState::new::<Self>(&display_handle);
        let presentation_state = PresentationState::new::<Self>(&display_handle, libc::CLOCK_MONOTONIC as u32);
        let data_device_state = DataDeviceState::new::<Self>(&display_handle);
        let primary_selection_state = PrimarySelectionState::new::<Self>(&display_handle);
        // Clipboard tools need direct selection access. Without data control,
        // wl-copy maps a temporary window to obtain keyboard focus.
        let wlr_data_control_state =
            wlr_data_control::DataControlState::new::<Self, _>(&display_handle, Some(&primary_selection_state), |_| {
                true
            });
        let ext_data_control_state =
            ext_data_control::DataControlState::new::<Self, _>(&display_handle, Some(&primary_selection_state), |_| {
                true
            });

        let relative_pointer_state = RelativePointerManagerState::new::<Self>(&display_handle);
        let viewporter_state = ViewporterState::new::<Self>(&display_handle);
        let layer_shell_state = WlrLayerShellState::new::<Self>(&display_handle);
        let xdg_activation_state = XdgActivationState::new::<Self>(&display_handle);
        let xdg_foreign_state = XdgForeignState::new::<Self>(&display_handle);
        let xdg_toplevel_icon_manager = XdgToplevelIconManager::new::<Self>(&display_handle);
        let mut seat_state = SeatState::new();
        let mut seat = seat_state.new_wl_seat(&display_handle, "ferese-winit");
        let xkb_options =
            (!config.input_settings.xkb_options.is_empty()).then(|| config.input_settings.xkb_options.join(","));
        seat.add_keyboard(
            XkbConfig {
                layout: &config.input_settings.xkb_layout,
                variant: &config.input_settings.xkb_variant,
                options: xkb_options,
                ..XkbConfig::default()
            },
            config.input_settings.repeat_delay_ms,
            config.input_settings.repeat_rate,
        )?;
        seat.add_pointer();
        seat.add_touch();
        let socket_name = Self::init_wayland_listener(display, event_loop)?;

        let start_time = Instant::now();
        let cursor_theme = cursor_theme();
        let default_cursor = load_named_cursor(&cursor_theme, CursorIcon::Default);
        let named_cursors = HashMap::from([(CursorIcon::Default, default_cursor)]);

        let mut state = Self {
            session_lock_state,
            session_lock: crate::session_lock::Lock::default(),
            lock_idle: config.lock_idle,
            config_source: None,
            theme_engine: Default::default(),
            start_time,
            socket_name,
            display_handle,
            loop_signal: event_loop.get_signal(),
            loop_handle: event_loop.handle(),
            portal_session: Default::default(),
            input_capture: Default::default(),
            space: Space::default(),
            workspaces: WorkspaceSet::new(
                config.layout_mode,
                config.default_column_width,
                config.scrolling_focus_strategy,
            ),
            output_workspaces: OutputWorkspaceMap::default(),
            output_ids: HashMap::new(),
            output_identity_ids: HashMap::new(),
            windows: WindowRegistry::default(),
            render: Default::default(),
            nested_backend: None,
            wallpaper: WallpaperState::with_wakeup(config.wallpaper, Some(event_loop.get_signal())),
            window_stack: WindowStack::default(),
            floating_above_fullscreen: HashMap::new(),
            floating_memory: crate::floating::Memory::path()
                .map(|p| crate::floating::Memory::load(&p))
                .unwrap_or_default(),
            floating_save_worker: Default::default(),
            floating_cascade: Default::default(),
            portal_shortcuts: PortalShortcuts::default(),
            pending_logout: None,
            logout_query: None,
            logout_owner: None,
            backdrop_generation: 0,
            viewport_animations: HashMap::new(),
            focus_swipe: None,
            workspace_slides: HashMap::new(),
            focused_window: None,
            focus_history: Default::default(),
            focus_cycle: None,
            column_width_presets: config.column_width_presets,
            gap_config: config.gap_config,
            input_settings: config.input_settings,
            bindings: config.bindings,
            workspace_auto_back_and_forth: config.workspace_auto_back_and_forth,
            window_rules: config.window_rules,
            theme_settings: config.theme_settings,
            inactive_dim: config.inactive_dim,
            animations_enabled: config.animations_enabled,
            animation_speed: config.animation_speed,
            spring_config: config.spring_config,
            viewport_spring_config: config.viewport_spring_config,
            output_profiles: config.output_profiles,
            autostart: config.autostart,
            cursor_status: CursorImageStatus::default_named(),
            cursor_redraw_pending: false,
            locked_pointer_hint: None,
            last_pointer_time: 0,
            cursor_theme,
            named_cursors,
            intercepted_keys: HashSet::new(),
            swipe: Swipe::default(),
            idle_inhibitors: HashMap::new(),
            active_shortcuts_inhibitor: None,
            direct_backend: None,
            _ipc_socket: None,
            pending_dmabuf_imports: Vec::new(),
            pending_screencopies: Vec::new(),
            screenshot: Coordinator::new(),
            screenshot_parts: None,
            screenshot_worker: None,
            shell_resources: Vec::new(),
            shell_snapshot_serial: 0,
            last_shell_snapshot: None,
            overview: OverviewState::with_font_family(config.overview_font_family),
            next_output_id: 1,
            last_animation_tick: start_time,
            popups: PopupManager::default(),
            dismissing_popups: Vec::new(),
            seat,
            alpha_modifier_state,
            compositor_state,
            cursor_shape_state,
            data_device_state,
            decoration_state,
            dmabuf_state,
            fractional_scale_state,
            idle_inhibit_state,
            idle_notifier_state,
            input_method_manager_state,
            keyboard_shortcuts_inhibit_state,
            output_manager_state,
            pointer_constraints_state,
            presentation_state,
            primary_selection_state,
            wlr_data_control_state,
            ext_data_control_state,
            relative_pointer_state,
            seat_state,
            shm_state,
            single_pixel_buffer_state,
            text_input_manager_state,
            viewporter_state,
            layer_shell_state,
            xdg_activation_state,
            xdg_foreign_state,
            xdg_shell_state,
            xdg_toplevel_icon_manager,
        };
        let screenshot = crate::ipc::init(event_loop)?;
        state.screenshot_parts = Some(screenshot.parts);
        state.screenshot_worker = Some(screenshot.worker);
        state._ipc_socket = Some(screenshot._guard);

        Ok(state)
    }

    pub(crate) fn apply_runtime_config(&mut self, config: RuntimeConfig) -> Result<(), String> {
        if self.output_profiles != config.output_profiles {
            crate::backends::direct::validate_live_outputs(self, &config.output_profiles)?;
        }

        let keyboard_changed = self.input_settings.xkb_layout != config.input_settings.xkb_layout
            || self.input_settings.xkb_variant != config.input_settings.xkb_variant
            || self.input_settings.xkb_options != config.input_settings.xkb_options;

        if let Some(keyboard) = self.seat.get_keyboard() {
            if keyboard_changed {
                self.input_capture.close_all();
                if self.input_capture.restore_focus {
                    self.restore_input_capture_focus();
                }
                let options = (!config.input_settings.xkb_options.is_empty())
                    .then(|| config.input_settings.xkb_options.join(","));

                keyboard
                    .set_xkb_config(
                        self,
                        XkbConfig {
                            layout: &config.input_settings.xkb_layout,
                            variant: &config.input_settings.xkb_variant,
                            options,
                            ..XkbConfig::default()
                        },
                    )
                    .map_err(|e| format!("keymap reload failed: {e}"))?;
            }
            keyboard.change_repeat_info(config.input_settings.repeat_rate, config.input_settings.repeat_delay_ms);
        }

        self.lock_idle = config.lock_idle;
        self.advance_animations(Instant::now());

        let touchpad_changed = self.input_settings.touchpad != config.input_settings.touchpad;
        let outputs_changed = self.output_profiles != config.output_profiles;
        let bounds = self
            .workspaces
            .iter()
            .map(|workspace| {
                let output = self.output_workspaces.output_for_workspace(workspace.id);
                let rect = self
                    .output_ids
                    .iter()
                    .find(|(_, id)| Some(**id) == output)
                    .and_then(|(output, _)| self.output_bounds_for(output))
                    .or_else(|| self.output_bounds())
                    .unwrap_or(Rect::new(0., 0., 1920., 1080.));
                (workspace.id, rect)
            })
            .collect();

        self.workspaces
            .reconfigure_live(
                config.layout_mode,
                config.default_column_width,
                config.scrolling_focus_strategy,
                &bounds,
            )
            .map_err(|error| error.to_string())?;
        self.gap_config = config.gap_config;
        self.input_settings = config.input_settings;
        self.bindings = config.bindings;
        self.workspace_auto_back_and_forth = config.workspace_auto_back_and_forth;
        self.portal_shortcuts.reconcile(&self.bindings, &self.input_settings);
        let old_rules = std::mem::replace(&mut self.window_rules, config.window_rules);
        self.theme_settings = config.theme_settings;
        self.inactive_dim = config.inactive_dim;
        self.column_width_presets = config.column_width_presets;
        self.animations_enabled = config.animations_enabled;
        if !self.animations_enabled {
            self.workspace_slides.clear();
        }
        self.animation_speed = config.animation_speed;
        self.spring_config = config.spring_config;
        self.viewport_spring_config = config.viewport_spring_config;
        self.output_profiles = config.output_profiles;
        self.autostart = config.autostart;

        if touchpad_changed {
            crate::backends::direct::reload_input_devices(self);
        }

        if outputs_changed {
            crate::backends::direct::reload_outputs(self);
        }

        if old_rules != self.window_rules {
            self.reapply_window_rules(&old_rules);
        }

        self.overview.set_font_family(config.overview_font_family);
        self.wallpaper.reload(config.wallpaper);
        self.backdrop_generation = self.backdrop_generation.wrapping_add(1);
        self.relayout();
        crate::backends::direct::render_all(self);

        Ok(())
    }

    pub(crate) fn queue_dmabuf_import(&mut self, dmabuf: Dmabuf, notifier: ImportNotifier) {
        self.pending_dmabuf_imports.push((dmabuf, notifier));
    }

    pub(crate) fn process_dmabuf_imports<R>(&mut self, renderer: &mut R)
    where
        R: ImportDma,
    {
        for (dmabuf, notifier) in self.pending_dmabuf_imports.drain(..) {
            if renderer.import_dmabuf(&dmabuf, None).is_ok() {
                if let Err(error) = notifier.successful::<Self>() {
                    tracing::debug!(?error, "dma-buf client disappeared before import completed");
                }
            } else {
                notifier.failed();
            }
        }
    }

    fn init_wayland_listener(
        display: Display<Self>,
        event_loop: &mut EventLoop<'static, Self>,
    ) -> Result<OsString, Box<dyn Error>> {
        let listening_socket = ListeningSocketSource::new_auto()?;
        let socket_name = listening_socket.socket_name().to_os_string();
        let loop_handle = event_loop.handle();

        loop_handle.insert_source(listening_socket, |client_stream, _, state| {
            let mut client_state = ClientState::default();
            if crate::handlers::window_capture::is_portal(&client_stream) {
                client_state
                    .capabilities
                    .insert(crate::private_client::ClientCapabilities::WINDOW_CAPTURE);
            }
            if let Err(error) = state
                .display_handle
                .insert_client(client_stream, Arc::new(client_state))
            {
                tracing::warn!(%error, "failed to register Wayland client");
            }
        })?;
        loop_handle.insert_source(
            Generic::new(display, Interest::READ, Mode::Level),
            |_, display, state| {
                // SAFETY: this event source owns the display for the loop lifetime.
                unsafe { display.get_mut().dispatch_clients(state)? };
                Ok(PostAction::Continue)
            },
        )?;
        Ok(socket_name)
    }
}

fn maximized_rect(bounds: Rect, gap: f64) -> Rect {
    let gap = gap
        .max(0.0)
        .min(((bounds.width.min(bounds.height) - 1.0) / 2.0).max(0.0));
    Rect::new(
        bounds.x + gap,
        bounds.y + gap,
        (bounds.width - 2.0 * gap).max(1.0),
        (bounds.height - 2.0 * gap).max(1.0),
    )
}

fn moved_floating_rect(rect: Rect, old: Rect, new: Rect) -> Rect {
    let width = rect.width.min(new.width).max(1.);
    let height = rect.height.min(new.height).max(1.);
    Rect::new(
        (rect.x + new.x - old.x).clamp(new.x, new.x + (new.width - width).max(0.)),
        (rect.y + new.y - old.y).clamp(new.y, new.y + (new.height - height).max(0.)),
        width,
        height,
    )
}

fn centered_floating_rect(bounds: Rect) -> Rect {
    let width = (bounds.width * 0.6).max(1.0);
    let height = (bounds.height * 0.6).max(1.0);

    Rect::new(
        bounds.x + (bounds.width - width) / 2.0,
        bounds.y + (bounds.height - height) / 2.0,
        width,
        height,
    )
}

fn floating_commit_is_current(
    no_pending_configures: bool,
    committed: Option<smithay::utils::Serial>,
    acknowledged: Option<smithay::utils::Serial>,
    resizing: bool,
) -> bool {
    no_pending_configures && committed.is_some() && committed == acknowledged && !resizing
}

fn constrained_floating_rect(rect: Rect, constraints: SizeConstraints) -> Rect {
    let min_width = constraints.min_width.max(1.0);
    let min_height = constraints.min_height.max(1.0);
    Rect::new(
        rect.x,
        rect.y,
        rect.width
            .clamp(min_width, constraints.max_width.unwrap_or(f64::INFINITY).max(min_width)),
        rect.height.clamp(
            min_height,
            constraints.max_height.unwrap_or(f64::INFINITY).max(min_height),
        ),
    )
}

fn natural_floating_rect(bounds: Rect, size: ClientSize) -> Rect {
    let width = f64::from(size.width).clamp(1.0, bounds.width.max(1.0));
    let height = f64::from(size.height).clamp(1.0, bounds.height.max(1.0));
    Rect::new(
        bounds.x + (bounds.width - width) / 2.0,
        bounds.y + (bounds.height - height) / 2.0,
        width,
        height,
    )
}

fn smoothstep(progress: f64) -> f64 {
    let progress = progress.clamp(0.0, 1.0);

    progress * progress * (3.0 - 2.0 * progress)
}

fn restored_scrolling_world_x(had_geometry: bool, visual_x: f64, viewport_x: f64, target_world_x: f64) -> f64 {
    if had_geometry {
        visual_x + viewport_x
    } else {
        target_world_x
    }
}

fn centered_transient_rect(parent: Rect) -> Rect {
    let width = (parent.width * 0.75).clamp(1.0, 640.0);
    let height = (parent.height * 0.75).clamp(1.0, 480.0);

    Rect::new(
        parent.x + (parent.width - width) / 2.0,
        parent.y + (parent.height - height) / 2.0,
        width,
        height,
    )
}

fn client_size(window: &Window) -> Option<ClientSize> {
    // XDG geometry alone is metadata, not a painted application frame.
    // Initial empty commits must not make the placeholder presentation ready.
    if !window_has_buffer(window) {
        return None;
    }
    let size = window.geometry().size;

    (size.w > 0 && size.h > 0).then_some(ClientSize {
        width: size.w,
        height: size.h,
    })
}

pub(crate) fn window_has_buffer(window: &Window) -> bool {
    window.toplevel().is_some_and(|toplevel| {
        smithay::backend::renderer::utils::with_renderer_surface_state(toplevel.wl_surface(), |state| {
            state.buffer().is_some()
        })
        .unwrap_or(false)
    })
}

fn activate_window_workspace(
    workspaces: &mut WorkspaceSet,
    outputs: &mut OutputWorkspaceMap,
    window: WindowId,
) -> bool {
    let Some(workspace) = workspaces.workspace_for_window(window) else {
        return false;
    };
    if let Some(output) = outputs
        .focused_output()
        .or_else(|| outputs.output_for_workspace(workspace))
        && outputs.switch_workspace(output, workspace).is_err()
    {
        return false;
    }
    if workspaces.activate(workspace).is_err() {
        return false;
    }

    if let Some(fullscreen) = workspaces.active().fullscreen
        && fullscreen != window
        && workspaces.placement(window) == Some(WindowPlacement::Tiled)
        && workspaces.set_fullscreen(fullscreen, false).is_err()
    {
        return false;
    }

    workspaces.focus_window(window).is_ok()
}

fn stacking_order_settled<'a, T, I>(current: I, target: &'a [T], z_index: impl Fn(&T) -> u8) -> bool
where
    I: IntoIterator<Item = &'a T>,
    T: PartialEq,
{
    let Some((first, rest)) = target.split_first() else {
        return current.into_iter().next().is_none();
    };
    let expected = z_index(first);
    rest.iter().all(|window| z_index(window) == expected) && current.into_iter().eq(target)
}

#[derive(Default)]
pub struct ClientState {
    pub compositor_state: CompositorClientState,
    pub(crate) capabilities: crate::private_client::ClientCapabilities,
}

impl ClientData for ClientState {
    fn initialized(&self, _client_id: ClientId) {}
    fn disconnected(&self, _client_id: ClientId, _reason: DisconnectReason) {}
}

#[cfg(test)]
mod tests {
    #[test]
    fn configured_motion_policy_scales_durations_and_reduced_motion_wins() {
        for (source, milliseconds) in [
            ("animations { speed 2; }", 100),
            ("animations { speed 0.5; }", 400),
            ("animations { speed 2; reduced-motion #true; }", 0),
            ("animations { speed 0.5; enabled #false; }", 0),
        ] {
            let config = crate::config::Config::parse_source(source).unwrap();
            assert_eq!(
                scaled_animation_duration(
                    Duration::from_millis(200),
                    config.animations_enabled(),
                    config.animation_speed().unwrap()
                ),
                Duration::from_millis(milliseconds),
            );
        }
    }

    #[test]
    fn disabling_motion_finishes_an_in_progress_close_once() {
        let mut animation = ClosingAnimation::default();
        assert!(!animation.advance(Duration::from_millis(35), true));
        assert!(animation.advance(Duration::ZERO, false));
        assert_eq!(animation.progress, 1.0);
        assert!(!animation.advance(Duration::ZERO, false));
    }

    #[test]
    fn workspace_drag_cancels_when_focus_or_target_ownership_changes() {
        let mut outputs = OutputWorkspaceMap::default();
        let left = OutputId(1);
        let right = OutputId(2);
        let from = WorkspaceId(1);
        let to = WorkspaceId(3);
        outputs
            .connect(left, OutputGeometry::new(0, 0, 1000, 1000), from)
            .unwrap();
        outputs
            .connect(right, OutputGeometry::new(1000, 0, 1000, 1000), WorkspaceId(2))
            .unwrap();
        assert!(workspace_swipe_is_current(&outputs, left, from, to));
        outputs.focus_output(right).unwrap();
        assert!(!workspace_swipe_is_current(&outputs, left, from, to));
        outputs.focus_output(left).unwrap();
        outputs.assign_workspace(right, to).unwrap();
        assert!(!workspace_swipe_is_current(&outputs, left, from, to));
        assert_eq!(outputs.active_workspace(left), Some(from));
        assert_eq!(outputs.output_for_workspace(to), Some(right));
        outputs.switch_workspace(left, WorkspaceId(4)).unwrap();
        assert!(!workspace_swipe_is_current(&outputs, left, from, WorkspaceId(5)));
    }

    #[test]
    fn selecting_a_window_activates_its_workspace_on_its_owner() {
        for assignment in [None, Some(OutputId(1)), Some(OutputId(2))] {
            let mut workspaces = WorkspaceSet::default();
            let first = workspaces.active_id();
            workspaces.insert_window(WindowId(1), Axis::Horizontal, 0.5).unwrap();
            let second = workspaces.create_workspace();
            workspaces.activate(second).unwrap();
            workspaces.insert_window(WindowId(2), Axis::Horizontal, 0.5).unwrap();
            workspaces.insert_window(WindowId(3), Axis::Horizontal, 0.5).unwrap();
            workspaces.activate(first).unwrap();

            let mut outputs = OutputWorkspaceMap::default();
            outputs
                .connect(OutputId(1), OutputGeometry::new(0, 0, 1920, 1080), first)
                .unwrap();
            let owner = if assignment == Some(OutputId(2)) {
                let third = workspaces.create_workspace();
                outputs
                    .connect(OutputId(2), OutputGeometry::new(1920, 0, 1920, 1080), third)
                    .unwrap();
                outputs.assign_workspace(OutputId(2), second).unwrap();
                OutputId(2)
            } else {
                if let Some(output) = assignment {
                    outputs.assign_workspace(output, second).unwrap();
                }
                OutputId(1)
            };
            outputs.focus_output(OutputId(1)).unwrap();

            assert!(activate_window_workspace(&mut workspaces, &mut outputs, WindowId(2)));
            assert_eq!(outputs.active_workspace(owner), Some(second));
            assert_eq!(outputs.focused_output(), Some(owner));
            assert_eq!(outputs.output_for_workspace(second), Some(owner));
            assert_eq!(workspaces.active_id(), second);
            assert_eq!(workspaces.active().last_focused, Some(WindowId(2)));
            if assignment == Some(OutputId(2)) {
                assert_eq!(outputs.active_workspace(OutputId(1)), Some(first));
            }
        }
    }

    #[test]
    fn selecting_a_tiled_window_reveals_it_past_another_fullscreen_window() {
        let mut workspaces = WorkspaceSet::default();
        workspaces.insert_window(WindowId(1), Axis::Horizontal, 0.5).unwrap();
        workspaces.insert_window(WindowId(2), Axis::Horizontal, 0.5).unwrap();
        workspaces.set_fullscreen(WindowId(1), true).unwrap();
        let mut outputs = OutputWorkspaceMap::default();
        outputs
            .connect(
                OutputId(1),
                OutputGeometry::new(0, 0, 1920, 1080),
                workspaces.active_id(),
            )
            .unwrap();

        assert!(activate_window_workspace(&mut workspaces, &mut outputs, WindowId(2)));
        assert_eq!(workspaces.active().fullscreen, None);
        assert_eq!(workspaces.active().last_focused, Some(WindowId(2)));
    }

    #[test]
    fn focus_drag_reverses_and_settles_from_the_release_position() {
        for (direction, gesture_direction, destination) in [
            (Direction::Right, SwipeDirection::Left, 900.0),
            (Direction::Left, SwipeDirection::Right, 100.0),
        ] {
            for committed in [false, true] {
                let mut swipe = FocusSwipe {
                    workspace: WorkspaceId(1),
                    from: WindowId(1),
                    to: WindowId(2),
                    direction,
                    gesture_direction,
                    start: 500.0,
                    destination,
                    progress: 0.75,
                };
                let far = swipe.position();
                swipe.progress = 0.25;
                let near = swipe.position();
                assert!((near - swipe.start).abs() < (far - swipe.start).abs());
                let mut viewport = AnimatedValue::new(near);
                viewport.set_target(if committed { destination } else { swipe.start });
                assert_eq!(viewport.current, near);
                for _ in 0..300 {
                    viewport.advance(Duration::from_millis(16), SpringConfig::default());
                }
                assert_eq!(viewport.current, viewport.target);
            }
        }
    }

    #[test]
    fn fully_visible_columns_still_give_bounded_horizontal_drag_feedback() {
        for (direction, sign) in [(Direction::Right, 1.0), (Direction::Left, -1.0)] {
            let swipe = FocusSwipe {
                workspace: WorkspaceId(1),
                from: WindowId(1),
                to: WindowId(2),
                direction,
                gesture_direction: SwipeDirection::Left,
                start: 0.0,
                destination: 0.0,
                progress: 0.5,
            };
            assert_eq!(swipe.position(), sign * 32.0);
            assert_eq!(swipe.destination, 0.0);
        }
    }

    #[test]
    fn workspace_drag_tracks_progress_and_releases_without_a_jump() {
        for committed in [false, true] {
            let from = WorkspaceId(1);
            let to = WorkspaceId(2);
            let mut slide = WorkspaceSlide::new(None, from, to, SwipeDirection::Up);
            slide.gesture = Some((from, to, SwipeDirection::Up));
            slide.held_progress = Some(0.4);
            slide.advance(Duration::from_secs(10));
            assert_eq!(slide.offset(from, 1000.0, 1000.0), (0.0, -400.0));
            assert_eq!(slide.offset(to, 1000.0, 1000.0), (0.0, 600.0));
            slide.release(committed);
            assert_eq!(slide.offset(from, 1000.0, 1000.0), (0.0, -400.0));
            assert_eq!(slide.offset(to, 1000.0, 1000.0), (0.0, 600.0));
            assert!(!slide.advance(WORKSPACE_SLIDE_DURATION));
            let active = if committed { to } else { from };
            assert_eq!(slide.offset(active, 1000.0, 1000.0), (0.0, 0.0));
        }
    }

    #[test]
    fn cancelling_a_chained_drag_returns_to_the_active_workspace() {
        let first = WorkspaceId(1);
        let second = WorkspaceId(2);
        let third = WorkspaceId(3);
        let mut previous = WorkspaceSlide::new(None, first, second, SwipeDirection::Up);
        previous.advance(WORKSPACE_SLIDE_DURATION / 3);
        let mut slide = WorkspaceSlide::new(Some(previous), second, third, SwipeDirection::Up);
        slide.gesture = Some((second, third, SwipeDirection::Up));
        slide.held_progress = Some(0.25);
        let position = slide.offset(second, 1000.0, 1000.0);
        slide.release(false);
        assert_eq!(slide.offset(second, 1000.0, 1000.0), position);
        slide.advance(WORKSPACE_SLIDE_DURATION);
        assert_eq!(slide.offset(second, 1000.0, 1000.0), (0.0, 0.0));
    }

    #[test]
    fn workspace_slide_moves_both_workspaces_without_a_gap() {
        let width = 1600.0;
        let height = 900.0;
        for (direction, outgoing_target) in [
            (SwipeDirection::Left, (-width, 0.0)),
            (SwipeDirection::Right, (width, 0.0)),
            (SwipeDirection::Up, (0.0, -height)),
            (SwipeDirection::Down, (0.0, height)),
        ] {
            let from = WorkspaceId(1);
            let to = WorkspaceId(2);
            let mut slide = WorkspaceSlide::new(None, from, to, direction);
            assert!(slide.contains(from));
            assert!(slide.contains(to));
            assert!(slide.advance(WORKSPACE_SLIDE_DURATION / 2));
            let outgoing = slide.offset(from, width, height);
            let incoming = slide.offset(to, width, height);
            assert!((incoming.0 - outgoing.0).abs() == outgoing_target.0.abs());
            assert!((incoming.1 - outgoing.1).abs() == outgoing_target.1.abs());

            assert!(!slide.advance(WORKSPACE_SLIDE_DURATION));
            assert_eq!(slide.offset(from, width, height), outgoing_target);
            assert_eq!(slide.offset(to, width, height), (0.0, 0.0));
        }
    }

    #[test]
    fn chained_workspace_slides_keep_all_visible_workspaces() {
        let first = WorkspaceId(1);
        let second = WorkspaceId(2);
        let third = WorkspaceId(3);
        let mut slide = WorkspaceSlide::new(None, first, second, SwipeDirection::Up);
        slide.advance(WORKSPACE_SLIDE_DURATION / 3);
        let first_position = slide.offset(first, 1600.0, 900.0);
        let second_position = slide.offset(second, 1600.0, 900.0);

        let chained = WorkspaceSlide::new(Some(slide), second, third, SwipeDirection::Left);
        assert!(chained.contains(first));
        assert!(chained.contains(second));
        assert!(chained.contains(third));
        let first_start = chained.items.iter().find(|item| item.workspace == first).unwrap().start;
        let second_start = chained
            .items
            .iter()
            .find(|item| item.workspace == second)
            .unwrap()
            .start;
        assert_eq!((first_start.x * 1600.0, first_start.y * 900.0), first_position);
        assert_eq!((second_start.x * 1600.0, second_start.y * 900.0), second_position);

        let reversed = WorkspaceSlide::new(Some(chained), third, first, SwipeDirection::Down);
        assert_eq!(reversed.items.len(), 3);
        assert!(reversed.contains(second));
        assert_eq!(
            reversed
                .items
                .iter()
                .find(|item| item.workspace == first)
                .unwrap()
                .target,
            SlideOffset::default()
        );
    }

    #[test]
    fn floating_sizes_respect_client_limits_without_moving_the_window() {
        let rect = Rect::new(40., 60., 300., 200.);
        let constraints = SizeConstraints {
            min_width: 640.,
            min_height: 480.,
            max_width: Some(900.),
            max_height: Some(700.),
        };
        assert_eq!(
            constrained_floating_rect(rect, constraints),
            Rect::new(40., 60., 640., 480.)
        );
        assert_eq!(
            constrained_floating_rect(Rect::new(40., 60., 1000., 800.), constraints),
            Rect::new(40., 60., 900., 700.)
        );
        assert_eq!(constrained_floating_rect(rect, SizeConstraints::default()), rect);
    }

    #[test]
    fn floating_commits_cannot_undo_pending_or_interactive_resizes() {
        let serial = Some(12.into());
        assert!(floating_commit_is_current(true, serial, serial, false));
        assert!(!floating_commit_is_current(false, serial, serial, false));
        assert!(!floating_commit_is_current(true, Some(11.into()), serial, false));
        assert!(!floating_commit_is_current(true, serial, serial, true));
        assert!(!floating_commit_is_current(true, None, None, false));
    }

    #[test]
    fn display_reposition_moves_floats_with_their_output_and_clamps_after_shrinking() {
        let old = ferese_layout::Rect::new(0., 0., 1920., 1080.);
        let new = ferese_layout::Rect::new(2000., 0., 1000., 700.);
        let rect = super::moved_floating_rect(ferese_layout::Rect::new(1500., 800., 400., 300.), old, new);
        assert_eq!(rect, ferese_layout::Rect::new(2600., 400., 400., 300.));
    }
    use super::*;

    #[test]
    fn floating_dialog_keeps_its_client_size_not_workspace_proportion() {
        assert_eq!(
            natural_floating_rect(
                Rect::new(100.0, 48.0, 1600.0, 900.0),
                ClientSize {
                    width: 400,
                    height: 200
                }
            ),
            Rect::new(700.0, 398.0, 400.0, 200.0)
        );
    }

    #[test]
    fn maximization_respects_layer_exclusion_and_outer_gaps() {
        assert_eq!(
            maximized_rect(Rect::new(0.0, 48.0, 1280.0, 752.0), 10.0),
            Rect::new(10.0, 58.0, 1260.0, 732.0)
        );
        assert_eq!(
            maximized_rect(Rect::new(1600.0, 36.0, 1280.0, 764.0), 6.0),
            Rect::new(1606.0, 42.0, 1268.0, 752.0)
        );
        let tiny = maximized_rect(Rect::new(0.0, 0.0, 5.0, 3.0), 10.0);
        assert!(tiny.width >= 1.0 && tiny.height >= 1.0);
    }

    #[test]
    fn transient_geometry_is_centered_and_bounded_by_its_parent() {
        let parent = Rect::new(100.0, 50.0, 1_000.0, 800.0);
        let transient = centered_transient_rect(parent);

        assert_eq!(transient, Rect::new(280.0, 210.0, 640.0, 480.0));
    }

    #[test]
    fn close_easing_is_bounded_and_symmetric() {
        assert_eq!(smoothstep(-1.0), 0.0);
        assert_eq!(smoothstep(0.5), 0.5);
        assert_eq!(smoothstep(2.0), 1.0);
    }

    #[test]
    fn close_animation_delays_the_protocol_close_until_it_finishes() {
        let mut animation = ClosingAnimation::default();

        assert!(!animation.advance(Duration::from_millis(70), true));
        assert_eq!(animation.progress, 0.5);
        assert!(animation.advance(Duration::from_millis(70), true));
        assert!(animation.close_sent);
        assert!(!animation.advance(Duration::from_secs(1), true));
    }

    #[test]
    fn coupled_right_column_width_keeps_its_right_edge_stable() {
        let spring = SpringConfig {
            mass: 1.0,
            stiffness: 320.0,
            damping: 2.0 * 320.0_f64.sqrt(),
            position_tolerance: 0.1,
            velocity_tolerance: 0.1,
        };
        let mut width = AnimatedValue::new(980.0);
        let mut viewport = AnimatedValue::new(495.0);
        width.retarget_preserving_motion(485.0);
        viewport.retarget_preserving_motion(0.0);

        for _ in 0..120 {
            width.advance(Duration::from_secs_f64(1.0 / 120.0), spring);
            viewport.advance(Duration::from_secs_f64(1.0 / 120.0), spring);

            assert!((width.current - viewport.current - 485.0).abs() < 0.001);
        }
    }

    #[test]
    fn fullscreen_exit_restores_world_x_from_presented_position() {
        assert_eq!(restored_scrolling_world_x(true, 0.0, 0.0, 505.0), 0.0);
        assert_eq!(restored_scrolling_world_x(true, 25.0, 480.0, 505.0), 505.0);
        assert_eq!(restored_scrolling_world_x(false, 10.0, 495.0, 1_000.0), 1_000.0);
    }

    #[derive(Clone, Copy, PartialEq, Debug)]
    struct StackedWindow {
        id: u32,
        z_index: u8,
    }

    fn stacked(id: u32, z_index: u8) -> StackedWindow {
        StackedWindow { id, z_index }
    }

    #[test]
    fn stacking_is_settled_when_order_matches_and_z_index_is_uniform() {
        let current = [stacked(1, 30), stacked(2, 30), stacked(3, 30)];
        assert!(stacking_order_settled(current.iter(), &current, |window| {
            window.z_index
        }));
    }

    #[test]
    fn stacking_is_not_settled_when_order_differs() {
        let current = [stacked(2, 30), stacked(1, 30)];
        let target = [stacked(1, 30), stacked(2, 30)];
        assert!(!stacking_order_settled(current.iter(), &target, |window| {
            window.z_index
        }));
    }

    #[test]
    fn stacking_is_not_settled_when_z_index_is_mixed_even_with_matching_order() {
        let current = [stacked(1, 30), stacked(2, 40)];
        assert!(!stacking_order_settled(current.iter(), &current, |window| window.z_index));
    }

    #[test]
    fn stacking_is_settled_for_an_empty_space() {
        let empty: [StackedWindow; 0] = [];
        assert!(stacking_order_settled(empty.iter(), &empty, |window| {
            window.z_index
        }));
    }
}
