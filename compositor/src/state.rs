use std::{
    collections::{HashMap, HashSet},
    error::Error,
    ffi::OsString,
    sync::Arc,
    time::{Duration, Instant},
};

use ferese_animation::{AnimatedValue, ClientSize, PresentationMode, SpringConfig, WindowGeometry};
use ferese_core::{
    LayoutMode, OutputGeometry, OutputId, OutputWorkspaceMap, WindowPlacement, WorkspaceId,
    WorkspaceSet,
};
use ferese_layout::{
    Axis, ColumnWidth, Direction, GapConfig, LayoutResult, Rect, SizeConstraints,
    ViewportFocusStrategy, WindowId,
};

use smithay::{
    backend::{
        allocator::dmabuf::Dmabuf,
        drm::{DrmEventTime, DrmNode},
        renderer::{ErasedContextId, ImportDma},
    },
    desktop::{
        LayerSurface, PopupManager, Space, Window, WindowSurfaceType, layer_map_for_output,
        utils::send_frames_surface_tree,
    },
    input::{
        Seat, SeatState,
        keyboard::XkbConfig,
        pointer::{CursorIcon, CursorImageStatus},
    },
    output::Output,
    reexports::{
        calloop::{EventLoop, Interest, LoopSignal, Mode, PostAction, generic::Generic},
        drm::control::crtc,
        wayland_protocols::xdg::{
            decoration::zv1::server::zxdg_toplevel_decoration_v1::Mode as DecorationMode,
            shell::server::xdg_toplevel,
        },
        wayland_server::{
            Display, DisplayHandle,
            backend::{ClientData, ClientId, DisconnectReason},
            protocol::wl_surface::WlSurface,
        },
    },
    utils::{Logical, Point, Rectangle, Size},
    wayland::{
        alpha_modifier::AlphaModifierState,
        compositor::{CompositorClientState, CompositorState, with_states},
        cursor_shape::CursorShapeManagerState,
        dmabuf::{DmabufState, ImportNotifier},
        fractional_scale::FractionalScaleManagerState,
        idle_inhibit::IdleInhibitManagerState,
        idle_notify::IdleNotifierState,
        input_method::InputMethodManagerState,
        keyboard_shortcuts_inhibit::{KeyboardShortcutsInhibitState, KeyboardShortcutsInhibitor},
        output::OutputManagerState,
        pointer_constraints::PointerConstraintsState,
        presentation::PresentationState,
        relative_pointer::RelativePointerManagerState,
        selection::{data_device::DataDeviceState, primary_selection::PrimarySelectionState},
        shell::wlr_layer::Layer,
        shell::wlr_layer::WlrLayerShellState,
        shell::xdg::decoration::XdgDecorationState,
        shell::xdg::{SurfaceCachedState, XdgShellState},
        shm::ShmState,
        single_pixel_buffer::SinglePixelBufferState,
        socket::ListeningSocketSource,
        text_input::TextInputManagerState,
        viewporter::ViewporterState,
        xdg_activation::XdgActivationState,
        xdg_foreign::XdgForeignState,
        xdg_toplevel_icon::XdgToplevelIconManager,
    },
};

use crate::{
    config::{Binding, InputSettings, OutputProfile, ThemeSettings},
    window_rules::{WindowRule, resolve as resolve_window_rules},
};

const CLOSE_ANIMATION_DURATION: Duration = Duration::from_millis(140);

#[derive(Clone, Copy, Debug, Default)]
struct ClosingAnimation {
    progress: f64,
    close_sent: bool,
}

impl ClosingAnimation {
    fn advance(&mut self, delta: Duration) -> bool {
        if self.close_sent {
            return false;
        }

        self.progress =
            (self.progress + delta.as_secs_f64() / CLOSE_ANIMATION_DURATION.as_secs_f64()).min(1.0);
        if self.progress < 1.0 {
            return false;
        }

        self.close_sent = true;
        true
    }
}

pub struct Ferese {
    pub start_time: Instant,
    pub socket_name: OsString,
    pub display_handle: DisplayHandle,
    pub loop_signal: LoopSignal,
    pub space: Space<Window>,
    pub workspaces: WorkspaceSet,
    pub output_workspaces: OutputWorkspaceMap,
    pub(crate) output_ids: HashMap<Output, OutputId>,
    output_identity_ids: HashMap<String, OutputId>,
    pub window_ids: HashMap<Window, WindowId>,
    pub window_geometry: HashMap<WindowId, WindowGeometry>,
    maximized_windows: HashSet<WindowId>,
    window_stack: crate::stacking::WindowStack,
    pub(crate) window_borders: HashMap<WindowId, crate::winit::WindowBorderBuffers>,
    pub(crate) window_shadows: HashMap<WindowId, crate::winit::WindowShadowBuffers>,
    pub(crate) rounded_clip_programs: HashMap<ErasedContextId, crate::winit::RoundedClipPrograms>,
    pub(crate) overview_scrims: HashMap<OutputId, crate::winit::OverviewScrim>,
    pub(crate) material_programs: HashMap<ErasedContextId, crate::winit::MaterialProgram>,
    pub(crate) material_buffers: HashMap<WlSurface, crate::winit::MaterialBuffers>,
    pub(crate) blur_programs: HashMap<ErasedContextId, crate::winit::BlurProgram>,
    pub(crate) backdrop_generation: u64,
    closing_windows: HashMap<WindowId, ClosingAnimation>,
    viewport_animations: HashMap<WorkspaceId, AnimatedValue>,
    scrolling_world_x: HashMap<WindowId, (WorkspaceId, AnimatedValue)>,
    viewport_coupled_widths: HashMap<WindowId, (WorkspaceId, AnimatedValue)>,
    pending_column_width_cycles: HashSet<WindowId>,
    pub focused_window: Option<WindowId>,
    column_width_presets: Vec<ColumnWidth>,
    gap_config: GapConfig,
    pub(crate) input_settings: InputSettings,
    pub(crate) bindings: Vec<Binding>,
    window_rules: Vec<WindowRule>,
    window_rules_applied: HashSet<WindowId>,
    pub(crate) theme_settings: ThemeSettings,
    animations_enabled: bool,
    animation_speed: f64,
    spring_config: SpringConfig,
    viewport_spring_config: SpringConfig,
    pub(crate) output_profiles: Vec<OutputProfile>,
    pub cursor_status: CursorImageStatus,
    // Cursor callbacks may run with Smithay's pointer mutex held. Rendering
    // reads the pointer position, so defer it until event dispatch returns.
    pub(crate) cursor_redraw_pending: bool,
    pub(crate) cursor_theme: xcursor::CursorTheme,
    pub(crate) named_cursors: HashMap<CursorIcon, crate::cursor::NamedCursor>,
    pub intercepted_keys: HashSet<smithay::input::keyboard::Keycode>,
    pub idle_inhibitors: HashMap<WlSurface, usize>,
    pub active_shortcuts_inhibitor: Option<KeyboardShortcutsInhibitor>,
    pub direct_backend: Option<crate::backends::direct::DirectBackendState>,
    _ipc_socket: Option<crate::ipc::IpcSocketGuard>,
    pending_dmabuf_imports: Vec<(Dmabuf, ImportNotifier)>,
    pub(crate) pending_screencopies: Vec<crate::handlers::screencopy::PendingScreencopy>,
    pub(crate) shell_resources: Vec<
        smithay::reexports::wayland_server::Weak<
            ferese_protocols::shell::v1::server::ferese_shell_v1::FereseShellV1,
        >,
    >,
    pub(crate) shell_snapshot_serial: u32,
    pub(crate) overview: crate::overview::OverviewState,
    next_window_id: u64,
    next_output_id: u64,
    last_animation_tick: Instant,
    pub popups: PopupManager,
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
    pub layout_mode: LayoutMode,
    pub gap_config: GapConfig,
    pub input_settings: InputSettings,
    pub bindings: Vec<Binding>,
    pub window_rules: Vec<WindowRule>,
    pub theme_settings: ThemeSettings,
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
    pub fn register_output(&mut self, output: &Output, identity: String) {
        let Some(geometry) = self.space.output_geometry(output) else {
            tracing::error!(output = %output.name(), "cannot register an unmapped output");
            return;
        };
        let output_id = *self.output_identity_ids.entry(identity).or_insert_with(|| {
            let id = OutputId(self.next_output_id);
            self.next_output_id = self.next_output_id.saturating_add(1);
            id
        });
        let fallback_workspace = (1..)
            .find_map(|index| {
                let workspace = self.workspaces.ensure_numeric(index).ok()?;
                self.output_workspaces
                    .output_for_workspace(workspace)
                    .is_none()
                    .then_some(workspace)
            })
            .expect("numeric workspaces are inexhaustible");
        let geometry = OutputGeometry::new(
            geometry.loc.x,
            geometry.loc.y,
            geometry.size.w,
            geometry.size.h,
        );

        let registration = self
            .output_workspaces
            .connect(output_id, geometry, fallback_workspace);

        match registration {
            Ok(workspace) => {
                self.output_ids.insert(output.clone(), output_id);
                if self.output_workspaces.focused_output() == Some(output_id) {
                    self.activate_output_workspace(output_id, workspace);
                }
                self.send_shell_snapshots();
            }
            Err(error) => {
                tracing::error!(%error, output = %output.name(), "failed to register output");
            }
        }
    }

    pub fn unregister_output(&mut self, output: &Output) {
        let Some(output_id) = self.output_ids.remove(output) else {
            return;
        };

        self.overview_scrims.remove(&output_id);
        self.pending_screencopies.retain(|capture| {
            if capture.output == *output {
                capture.frame.failed();
                false
            } else {
                true
            }
        });
        self.space.unmap_output(output);

        match self.output_workspaces.disconnect(output_id) {
            Ok(Some(target)) => {
                if let Some(workspace) = self.output_workspaces.active_workspace(target) {
                    self.activate_output_workspace(target, workspace);
                }
            }
            Ok(None) => {
                self.focused_window = None;
                self.restore_keyboard_focus();
            }
            Err(error) => {
                tracing::error!(%error, output = %output.name(), "failed to unregister output")
            }
        }

        self.relayout();
    }

    pub(crate) fn is_focused_output(&self, output: &Output) -> bool {
        self.output_ids.get(output).copied() == self.output_workspaces.focused_output()
    }

    fn activate_output_workspace(&mut self, output: OutputId, workspace: ferese_core::WorkspaceId) {
        if let Err(error) = self.output_workspaces.focus_output(output) {
            tracing::error!(%error, "failed to focus output");
            return;
        }

        match self.workspaces.activate(workspace) {
            Ok(focus) => self.focused_window = focus,
            Err(error) => tracing::error!(%error, "failed to activate output workspace"),
        }
    }

    pub(crate) fn focused_output(&self) -> Option<&Output> {
        let focused = self.output_workspaces.focused_output()?;
        self.output_ids
            .iter()
            .find_map(|(output, id)| (*id == focused).then_some(output))
    }

    pub(crate) fn output_id(&self, output: &Output) -> Option<OutputId> {
        self.output_ids.get(output).copied()
    }

    pub(crate) fn focus_output_at(&mut self, position: Point<f64, Logical>) {
        let output = self.space.outputs().find_map(|output| {
            let geometry = self.space.output_geometry(output)?;
            (position.x >= f64::from(geometry.loc.x)
                && position.y >= f64::from(geometry.loc.y)
                && position.x < f64::from(geometry.loc.x + geometry.size.w)
                && position.y < f64::from(geometry.loc.y + geometry.size.h))
            .then(|| output.clone())
        });
        let Some(output) = output else {
            return;
        };
        let Some(output_id) = self.output_ids.get(&output).copied() else {
            return;
        };
        if self.output_workspaces.focused_output() == Some(output_id) {
            return;
        }
        let Some(workspace) = self.output_workspaces.active_workspace(output_id) else {
            return;
        };

        self.activate_output_workspace(output_id, workspace);
        self.relayout();
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
        let compositor_state = CompositorState::new::<Self>(&display_handle);
        let cursor_shape_state = CursorShapeManagerState::new::<Self>(&display_handle);
        let xdg_shell_state = XdgShellState::new::<Self>(&display_handle);
        let decoration_state = XdgDecorationState::new::<Self>(&display_handle);
        let dmabuf_state = DmabufState::new();
        let fractional_scale_state = FractionalScaleManagerState::new::<Self>(&display_handle);
        let idle_inhibit_state = IdleInhibitManagerState::new::<Self>(&display_handle);
        let idle_notifier_state = IdleNotifierState::new(&display_handle, event_loop.handle());
        let input_method_enabled =
            std::env::var_os("FERESE_ENABLE_INPUT_METHOD").is_some_and(|value| value == "1");
        let input_method_manager_state =
            InputMethodManagerState::new::<Self, _>(&display_handle, move |_| input_method_enabled);
        let keyboard_shortcuts_inhibit_state =
            KeyboardShortcutsInhibitState::new::<Self>(&display_handle);
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
        let presentation_state =
            PresentationState::new::<Self>(&display_handle, libc::CLOCK_MONOTONIC as u32);
        let data_device_state = DataDeviceState::new::<Self>(&display_handle);
        let primary_selection_state = PrimarySelectionState::new::<Self>(&display_handle);
        let relative_pointer_state = RelativePointerManagerState::new::<Self>(&display_handle);
        let viewporter_state = ViewporterState::new::<Self>(&display_handle);
        let layer_shell_state = WlrLayerShellState::new::<Self>(&display_handle);
        let xdg_activation_state = XdgActivationState::new::<Self>(&display_handle);
        let xdg_foreign_state = XdgForeignState::new::<Self>(&display_handle);
        let xdg_toplevel_icon_manager = XdgToplevelIconManager::new::<Self>(&display_handle);
        let mut seat_state = SeatState::new();
        let mut seat = seat_state.new_wl_seat(&display_handle, "ferese-winit");
        let xkb_options = (!config.input_settings.xkb_options.is_empty())
            .then(|| config.input_settings.xkb_options.join(","));
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
        let cursor_theme = crate::cursor::cursor_theme();
        let default_cursor = crate::cursor::load_named_cursor(&cursor_theme, CursorIcon::Default);
        let named_cursors = HashMap::from([(CursorIcon::Default, default_cursor)]);

        let mut state = Self {
            start_time,
            socket_name,
            display_handle,
            loop_signal: event_loop.get_signal(),
            space: Space::default(),
            workspaces: WorkspaceSet::new(
                config.layout_mode,
                config.default_column_width,
                config.scrolling_focus_strategy,
            ),
            output_workspaces: OutputWorkspaceMap::default(),
            output_ids: HashMap::new(),
            output_identity_ids: HashMap::new(),
            window_ids: HashMap::new(),
            window_geometry: HashMap::new(),
            maximized_windows: HashSet::new(),
            window_stack: crate::stacking::WindowStack::default(),
            window_borders: HashMap::new(),
            window_shadows: HashMap::new(),
            rounded_clip_programs: HashMap::new(),
            overview_scrims: HashMap::new(),
            material_programs: HashMap::new(),
            material_buffers: HashMap::new(),
            blur_programs: HashMap::new(),
            backdrop_generation: 0,
            closing_windows: HashMap::new(),
            viewport_animations: HashMap::new(),
            scrolling_world_x: HashMap::new(),
            viewport_coupled_widths: HashMap::new(),
            pending_column_width_cycles: HashSet::new(),
            focused_window: None,
            column_width_presets: config.column_width_presets,
            gap_config: config.gap_config,
            input_settings: config.input_settings,
            bindings: config.bindings,
            window_rules: config.window_rules,
            window_rules_applied: HashSet::new(),
            theme_settings: config.theme_settings,
            animations_enabled: config.animations_enabled,
            animation_speed: config.animation_speed,
            spring_config: config.spring_config,
            viewport_spring_config: config.viewport_spring_config,
            output_profiles: config.output_profiles,
            cursor_status: CursorImageStatus::default_named(),
            cursor_redraw_pending: false,
            cursor_theme,
            named_cursors,
            intercepted_keys: HashSet::new(),
            idle_inhibitors: HashMap::new(),
            active_shortcuts_inhibitor: None,
            direct_backend: None,
            _ipc_socket: None,
            pending_dmabuf_imports: Vec::new(),
            pending_screencopies: Vec::new(),
            shell_resources: Vec::new(),
            shell_snapshot_serial: 0,
            overview: crate::overview::OverviewState::default(),
            next_window_id: 1,
            next_output_id: 1,
            last_animation_tick: start_time,
            popups: PopupManager::default(),
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
        state._ipc_socket = Some(crate::ipc::init(event_loop)?);
        Ok(state)
    }

    fn init_wayland_listener(
        display: Display<Self>,
        event_loop: &mut EventLoop<'static, Self>,
    ) -> Result<OsString, Box<dyn Error>> {
        let listening_socket = ListeningSocketSource::new_auto()?;
        let socket_name = listening_socket.socket_name().to_os_string();
        let loop_handle = event_loop.handle();

        loop_handle.insert_source(listening_socket, |client_stream, _, state| {
            if let Err(error) = state
                .display_handle
                .insert_client(client_stream, Arc::new(ClientState::default()))
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

    pub fn surface_under(
        &self,
        position: Point<f64, Logical>,
    ) -> Option<(WlSurface, Point<f64, Logical>)> {
        self.layer_surface_under(position, &[Layer::Overlay, Layer::Top])
            .map(|(_, surface, origin)| (surface, origin))
            .or_else(|| self.window_surface_under(position))
            .or_else(|| {
                self.layer_surface_under(position, &[Layer::Bottom, Layer::Background])
                    .map(|(_, surface, origin)| (surface, origin))
            })
    }

    pub fn layer_under(
        &self,
        position: Point<f64, Logical>,
    ) -> Option<(LayerSurface, WlSurface, Point<f64, Logical>)> {
        if let Some(layer) = self.layer_surface_under(position, &[Layer::Overlay, Layer::Top]) {
            return Some(layer);
        }
        if self.window_surface_under(position).is_some() {
            return None;
        }

        self.layer_surface_under(position, &[Layer::Bottom, Layer::Background])
    }

    fn window_surface_under(
        &self,
        position: Point<f64, Logical>,
    ) -> Option<(WlSurface, Point<f64, Logical>)> {
        if self.overview.is_active() {
            return None;
        }

        self.space.elements().rev().find_map(|window| {
            let id = self.window_ids.get(window)?;
            let visual = self.presented_window_rect(*id)?;
            let inside_visual = position.x >= visual.x
                && position.y >= visual.y
                && position.x < visual.x + visual.width
                && position.y < visual.y + visual.height;
            if !inside_visual {
                return None;
            }

            let (source_x, source_y) =
                self.inverse_presented_window_point(*id, position.x, position.y)?;
            let source_point = Point::from((source_x, source_y)) + window.geometry().loc.to_f64();

            window
                .surface_under(source_point, WindowSurfaceType::ALL)
                .map(|(surface, surface_location)| {
                    let surface_point = source_point - surface_location.to_f64();
                    (surface, position - surface_point)
                })
        })
    }

    fn layer_surface_under(
        &self,
        position: Point<f64, Logical>,
        layers: &[Layer],
    ) -> Option<(LayerSurface, WlSurface, Point<f64, Logical>)> {
        let output = self.space.output_under(position).next()?;
        let output_geometry = self.space.output_geometry(output)?;
        let output_position = position - output_geometry.loc.to_f64();
        let map = layer_map_for_output(output);

        for requested in layers {
            if *requested == Layer::Top && self.output_has_fullscreen(output) {
                continue;
            }
            for layer in map.layers_on(*requested).rev() {
                let geometry = map.layer_geometry(layer)?;
                let layer_position = output_position - geometry.loc.to_f64();
                let Some((surface, surface_location)) =
                    layer.surface_under(layer_position, WindowSurfaceType::ALL)
                else {
                    continue;
                };
                let origin = output_geometry.loc + geometry.loc + surface_location;

                return Some((layer.clone(), surface, origin.to_f64()));
            }
        }

        None
    }

    pub fn send_cursor_frame(&self, output: &Output) {
        let CursorImageStatus::Surface(surface) = &self.cursor_status else {
            return;
        };

        send_frames_surface_tree(
            surface,
            output,
            self.start_time.elapsed(),
            Some(Duration::ZERO),
            |_, _| Some(output.clone()),
        );
    }

    pub fn window_under_visual(&self, position: Point<f64, Logical>) -> Option<Window> {
        self.space.elements().rev().find_map(|window| {
            let id = self.window_ids.get(window)?;
            let visual = self.presented_window_rect(*id)?;

            (position.x >= visual.x
                && position.y >= visual.y
                && position.x < visual.x + visual.width
                && position.y < visual.y + visual.height)
                .then(|| window.clone())
        })
    }

    pub(crate) fn raise_window(&mut self, window: &Window, activate: bool) {
        if let Some(id) = self.window_ids.get(window) {
            self.window_stack.raise(*id);
        }
        self.space.raise_element(window, activate);
        self.sync_window_stacking();
    }

    pub(crate) fn sync_window_stacking(&mut self) {
        let mut windows = self.space.elements().cloned().collect::<Vec<_>>();
        windows.sort_by_key(|window| {
            let id = self.window_ids.get(window).copied();
            let priority = id
                .and_then(|id| self.window_geometry.get(&id))
                .is_some_and(|geometry| geometry.is_fullscreen() || geometry.is_zooming());
            (
                priority,
                id.map_or(usize::MAX, |id| self.window_stack.rank(id)),
            )
        });
        for window in windows {
            self.space.raise_element(&window, false);
        }
    }

    pub fn visual_scale_for_window(&self, window: &Window) -> Option<(f64, f64)> {
        let id = self.window_ids.get(window)?;
        let geometry = self.window_geometry.get(id)?;
        let source = geometry.client.committed_size?;
        let presented = self.presented_window_rect(*id)?;

        if source.width <= 0 || source.height <= 0 {
            return None;
        }

        Some((
            presented.width / f64::from(source.width),
            presented.height / f64::from(source.height),
        ))
    }

    pub(crate) fn visual_rect_for_window(
        &self,
        window: &Window,
    ) -> Option<Rectangle<i32, Logical>> {
        let id = self.window_ids.get(window)?;
        let rect = self.presented_window_rect(*id)?;
        let size = ClientSize::from_rect(rect);
        Some(Rectangle::new(
            (rect.x.round() as i32, rect.y.round() as i32).into(),
            (size.width, size.height).into(),
        ))
    }

    pub fn add_tiled_window(&mut self, window: Window) {
        let id = WindowId(self.next_window_id);
        self.next_window_id += 1;
        let focus_new_window = self.workspaces.active().fullscreen.is_none();

        let axis = self
            .output_bounds()
            .and_then(|bounds| {
                self.workspaces
                    .active()
                    .layout
                    .automatic_axis(self.focused_window, bounds)
                    .ok()
            })
            .unwrap_or(Axis::Horizontal);

        if let Err(error) = self.workspaces.insert_window(id, axis, 0.5) {
            tracing::error!(%error, ?id, "failed to insert window into layout");
            return;
        }

        self.window_ids.insert(window.clone(), id);
        self.window_stack.insert(id);
        if focus_new_window {
            self.focused_window = Some(id);
        }
        self.space.map_element(window, (0, 0), focus_new_window);
        self.relayout();

        if focus_new_window {
            self.restore_keyboard_focus();
        }
    }

    pub fn add_transient_window(&mut self, window: Window, parent: WindowId) {
        let Some(workspace) = self.workspaces.workspace_for_window(parent) else {
            self.add_tiled_window(window);
            return;
        };
        let Some(bounds) = self.output_bounds() else {
            self.add_tiled_window(window);
            return;
        };
        let parent_rect = self.logical_window_rect(parent, bounds).unwrap_or(bounds);
        let rect = centered_transient_rect(parent_rect);
        let id = WindowId(self.next_window_id);
        self.next_window_id += 1;

        if let Err(error) = self
            .workspaces
            .insert_floating_window(id, workspace, rect, false)
        {
            tracing::error!(%error, ?id, ?parent, "failed to insert transient window");
            return;
        }

        self.window_ids.insert(window.clone(), id);
        self.window_stack.insert(id);
        self.space.map_element(window, (0, 0), false);
        self.relayout();
    }

    pub(crate) fn apply_initial_window_rules(
        &mut self,
        window: &Window,
        app_id: Option<&str>,
        title: Option<&str>,
        transient: bool,
    ) {
        let Some(id) = self.window_ids.get(window).copied() else {
            return;
        };
        let Some(bounds) = self.output_bounds() else {
            return;
        };
        if !self.window_rules_applied.insert(id) {
            return;
        }

        let rule = resolve_window_rules(&self.window_rules, app_id, title, transient);
        if rule == Default::default() {
            return;
        }
        let axis = self
            .workspaces
            .active()
            .layout
            .automatic_axis(self.focused_window, bounds)
            .unwrap_or(Axis::Horizontal);

        if let Some(workspace) = rule.workspace
            && let Err(error) = self
                .workspaces
                .move_window_to_numeric(id, workspace, axis, 0.5)
        {
            tracing::warn!(%error, ?id, workspace, "failed to apply window workspace rule");
        }

        let should_float = rule
            .floating
            .or((rule.width.is_some() || rule.height.is_some()).then_some(true));
        if let Some(should_float) = should_float {
            let is_floating = matches!(
                self.workspaces.placement(id),
                Some(WindowPlacement::Floating { .. })
            );
            let mut rect = centered_floating_rect(bounds);
            rect.width = rule.width.unwrap_or(rect.width).min(bounds.width);
            rect.height = rule.height.unwrap_or(rect.height).min(bounds.height);
            rect.x = bounds.x + (bounds.width - rect.width) / 2.0;
            rect.y = bounds.y + (bounds.height - rect.height) / 2.0;

            let result = if should_float != is_floating {
                self.workspaces
                    .toggle_floating(id, rect, axis, 0.5)
                    .map(|_| ())
            } else if should_float && (rule.width.is_some() || rule.height.is_some()) {
                self.workspaces.set_floating_rect(id, rect)
            } else {
                Ok(())
            };
            if let Err(error) = result {
                tracing::warn!(%error, ?id, "failed to apply floating window rule");
            }
        }

        if let Some(fullscreen) = rule.fullscreen
            && let Err(error) = self.workspaces.set_fullscreen(id, fullscreen)
        {
            tracing::warn!(%error, ?id, fullscreen, "failed to apply fullscreen window rule");
        }

        if self.workspaces.workspace_for_window(id) != Some(self.workspaces.active_id())
            && self.focused_window == Some(id)
        {
            self.focused_window = self.workspaces.active().last_focused;
        }

        self.relayout();
        self.restore_keyboard_focus();
    }

    pub fn make_window_transient(&mut self, window: &Window, parent: WindowId) {
        let Some(id) = self.window_ids.get(window).copied() else {
            return;
        };
        if self.workspaces.workspace_for_window(id) != self.workspaces.workspace_for_window(parent)
        {
            tracing::warn!(
                ?id,
                ?parent,
                "ignored transient parent on another workspace"
            );
            return;
        }
        let Some(bounds) = self.output_bounds() else {
            return;
        };
        let parent_rect = self.logical_window_rect(parent, bounds).unwrap_or(bounds);
        let rect = centered_transient_rect(parent_rect);
        let result = match self.workspaces.placement(id) {
            Some(WindowPlacement::Tiled) => self
                .workspaces
                .toggle_floating(id, rect, Axis::Horizontal, 0.5)
                .map(|_| ()),
            Some(WindowPlacement::Floating { .. }) => self.workspaces.set_floating_rect(id, rect),
            None => return,
        };

        if let Err(error) = result {
            tracing::warn!(%error, ?id, ?parent, "failed to apply transient placement");
            return;
        }

        self.relayout();
    }

    pub fn remove_tiled_window(&mut self, window: &Window) {
        let Some(id) = self.window_ids.remove(window) else {
            return;
        };

        self.space.unmap_elem(window);
        self.window_geometry.remove(&id);
        self.maximized_windows.remove(&id);
        self.window_stack.remove(id);
        self.window_borders.remove(&id);
        self.window_shadows.remove(&id);
        self.closing_windows.remove(&id);
        self.window_rules_applied.remove(&id);
        self.scrolling_world_x.remove(&id);
        self.viewport_coupled_widths.remove(&id);
        self.pending_column_width_cycles.remove(&id);
        if let Err(error) = self.workspaces.remove_window(id) {
            tracing::error!(%error, ?id, "failed to remove window from layout");
        }
        if self.focused_window == Some(id) {
            self.focused_window = self.workspaces.active().last_focused;
        }

        self.relayout();
    }

    pub fn relayout(&mut self) {
        self.backdrop_generation = self.backdrop_generation.wrapping_add(1);
        self.arrange_layers();
        let mut previous_scrolling_world_x = std::mem::take(&mut self.scrolling_world_x);
        let pending_column_width_cycles = std::mem::take(&mut self.pending_column_width_cycles);
        let outputs = self.space.outputs().cloned().collect::<Vec<_>>();
        let constraints = self.window_constraints();
        let mut visible = HashSet::new();
        let mut placements = Vec::new();

        for output in outputs {
            let Some(output_id) = self.output_ids.get(&output).copied() else {
                continue;
            };
            let Some(workspace_id) = self.output_workspaces.active_workspace(output_id) else {
                continue;
            };
            let Some(bounds) = self.output_bounds_for(&output) else {
                continue;
            };
            let fullscreen_bounds = self.full_output_bounds_for(&output).unwrap_or(bounds);
            let Some(workspace) = self.workspaces.workspace(workspace_id) else {
                continue;
            };
            let workspace_fullscreen = workspace.fullscreen;
            let workspace_focus = workspace.last_focused;
            let focused = self
                .focused_window
                .filter(|window| {
                    self.workspaces.workspace_for_window(*window) == Some(workspace_id)
                })
                .or(workspace_focus);
            let Some(workspace) = self.workspaces.workspace_mut(workspace_id) else {
                continue;
            };
            let layout = match workspace.layout.geometry_with_constraints(
                bounds,
                self.gap_config,
                &constraints,
                focused,
            ) {
                Ok(layout) => layout,
                Err(error) => {
                    tracing::error!(%error, ?workspace_id, "failed to compute tiled geometry");
                    continue;
                }
            };
            let viewport_target = workspace.layout.viewport_x();
            let viewport_motion = viewport_target.map(|target| {
                let viewport = self
                    .viewport_animations
                    .entry(workspace_id)
                    .or_insert_with(|| AnimatedValue::new(target));
                let target_changed = (viewport.target - target).abs() > 0.001;
                viewport.retarget_preserving_motion(target);
                if !self.animations_enabled {
                    viewport.snap();
                }
                (viewport.current, target_changed)
            });
            let viewport_current = viewport_motion.map(|(current, _)| current);
            let viewport_target_changed = viewport_motion.is_some_and(|(_, changed)| changed);

            for warning in layout.warnings {
                tracing::warn!(
                    window = ?warning.window,
                    kind = ?warning.kind,
                    requested = warning.requested,
                    assigned = warning.assigned,
                    "window size constraint could not be satisfied exactly"
                );
            }

            for (window, id) in &self.window_ids {
                if self.workspaces.workspace_for_window(*id) != Some(workspace_id) {
                    continue;
                }

                let is_maximized =
                    self.maximized_windows.contains(id) && workspace_fullscreen != Some(*id);
                let rect = if workspace_fullscreen == Some(*id) {
                    fullscreen_bounds
                } else if is_maximized {
                    maximized_rect(bounds, self.gap_config.outer)
                } else {
                    match self.workspaces.placement(*id) {
                        Some(WindowPlacement::Tiled) => {
                            let Some(rect) = layout.geometry.get(id).copied() else {
                                continue;
                            };
                            rect
                        }
                        Some(WindowPlacement::Floating { rect }) => rect,
                        None => continue,
                    }
                };
                let is_fullscreen = workspace_fullscreen == Some(*id);
                let is_floating = matches!(
                    self.workspaces.placement(*id),
                    Some(WindowPlacement::Floating { .. })
                );

                visible.insert(*id);
                let scrolling = if !is_fullscreen && !is_maximized && !is_floating {
                    viewport_target
                        .zip(viewport_current)
                        .map(|(target, current)| (workspace_id, rect.x + target, current))
                } else {
                    None
                };

                placements.push((
                    window.clone(),
                    *id,
                    rect,
                    is_fullscreen,
                    is_maximized,
                    is_floating,
                    scrolling,
                    pending_column_width_cycles.contains(id) && viewport_target_changed,
                ));
            }
        }

        let hidden = self
            .window_ids
            .iter()
            .filter(|(_, id)| !visible.contains(id))
            .map(|(window, _)| window.clone())
            .collect::<Vec<_>>();

        for window in hidden {
            self.space.unmap_elem(&window);
        }

        let now = self.start_time.elapsed();

        let mut scrolling_world_x = HashMap::new();
        placements.sort_by_key(|(_, id, ..)| self.window_stack.rank(*id));

        for (window, id, rect, is_fullscreen, is_maximized, is_floating, scrolling, couple_width) in
            placements
        {
            // A viewport-coupled width must never override fullscreen/floating geometry.
            if scrolling.is_none() {
                self.viewport_coupled_widths.remove(&id);
            }
            let committed_size = client_size(&window);
            let had_geometry = self.window_geometry.contains_key(&id);
            let geometry = self
                .window_geometry
                .entry(id)
                .or_insert_with(|| WindowGeometry::new(rect, committed_size));
            let mode = if is_fullscreen {
                PresentationMode::Fullscreen
            } else if is_maximized {
                PresentationMode::Maximized
            } else {
                PresentationMode::Normal
            };
            let mut requested_size = geometry.set_presentation_mode(rect, mode, now);
            if !self.animations_enabled {
                geometry.advance(Duration::ZERO, self.spring_config, false);
                requested_size = geometry.presentation_size_request(now).or(requested_size);
            }
            if geometry.is_zooming() {
                self.viewport_coupled_widths.remove(&id);
            }
            if let Some((workspace, world_x, viewport_x)) = scrolling {
                let restored_world_x = restored_scrolling_world_x(
                    had_geometry,
                    geometry.visual.current.x,
                    viewport_x,
                    world_x,
                );
                let mut animated_world_x = previous_scrolling_world_x
                    .remove(&id)
                    .filter(|(previous_workspace, _)| *previous_workspace == workspace)
                    .map(|(_, world_x)| world_x)
                    .unwrap_or_else(|| AnimatedValue::new(restored_world_x));
                animated_world_x.set_target(world_x);
                if !self.animations_enabled {
                    animated_world_x.snap();
                }
                if !geometry.is_zooming() {
                    geometry.visual.current.x = animated_world_x.current - viewport_x;
                    geometry.visual.velocity.x = animated_world_x.velocity;
                }
                scrolling_world_x.insert(id, (workspace, animated_world_x));

                let coupled = !geometry.is_zooming()
                    && (couple_width
                        || self.viewport_coupled_widths.get(&id).is_some_and(
                            |(previous_workspace, _)| *previous_workspace == workspace,
                        ));
                if coupled {
                    let width = self.viewport_coupled_widths.entry(id).or_insert_with(|| {
                        (workspace, AnimatedValue::new(geometry.visual.current.width))
                    });
                    if width.0 != workspace {
                        *width = (workspace, AnimatedValue::new(geometry.visual.current.width));
                    }
                    width.1.retarget_preserving_motion(rect.width);
                    if !self.animations_enabled {
                        width.1.snap();
                    }
                    geometry.visual.current.width = width.1.current;
                    geometry.visual.velocity.width = width.1.velocity;
                } else if pending_column_width_cycles.contains(&id) {
                    self.viewport_coupled_widths.remove(&id);
                }
            }
            let visual = geometry.visual.current;
            let location = (visual.x.round() as i32, visual.y.round() as i32);

            self.space.map_element(window.clone(), location, false);
            if let Some(toplevel) = window.toplevel() {
                let state_changed = toplevel.with_pending_state(|state| {
                    if let Some(size) = requested_size {
                        state.size = Some((size.width, size.height).into());
                    }

                    let fullscreen_changed = if is_fullscreen {
                        state.states.set(xdg_toplevel::State::Fullscreen)
                    } else {
                        state.states.unset(xdg_toplevel::State::Fullscreen)
                    };

                    let maximized_changed = if is_maximized {
                        state.states.set(xdg_toplevel::State::Maximized)
                    } else {
                        state.states.unset(xdg_toplevel::State::Maximized)
                    };
                    let tiled = !is_floating && !is_fullscreen;
                    let tiled_changed = if tiled {
                        state.states.set(xdg_toplevel::State::TiledLeft)
                            | state.states.set(xdg_toplevel::State::TiledRight)
                            | state.states.set(xdg_toplevel::State::TiledTop)
                            | state.states.set(xdg_toplevel::State::TiledBottom)
                    } else {
                        state.states.unset(xdg_toplevel::State::TiledLeft)
                            | state.states.unset(xdg_toplevel::State::TiledRight)
                            | state.states.unset(xdg_toplevel::State::TiledTop)
                            | state.states.unset(xdg_toplevel::State::TiledBottom)
                    };

                    let decoration_mode = if is_floating && !is_fullscreen && !is_maximized {
                        DecorationMode::ClientSide
                    } else {
                        DecorationMode::ServerSide
                    };
                    let decoration_changed = state.decoration_mode != Some(decoration_mode);
                    state.decoration_mode = Some(decoration_mode);

                    fullscreen_changed || maximized_changed || tiled_changed || decoration_changed
                });

                if requested_size.is_some() || state_changed {
                    toplevel.send_pending_configure();
                }
            }
        }
        self.scrolling_world_x = scrolling_world_x;
        self.sync_window_stacking();
        self.retarget_overview();
        self.send_shell_snapshots();

        crate::backends::direct::render_all(self);
    }

    pub fn advance_animations(&mut self, now: Instant) -> bool {
        let delta = now.saturating_duration_since(self.last_animation_tick);
        self.last_animation_tick = now;
        self.advance_animations_by(delta)
    }

    pub fn record_drm_presentation(
        &mut self,
        node: DrmNode,
        crtc: crtc::Handle,
        time: DrmEventTime,
        sequence: u32,
    ) {
        let delta = self
            .direct_backend
            .as_mut()
            .and_then(|backend| backend.record_presentation(node, crtc, time, sequence));
        if let Some(delta) = delta {
            self.advance_animations_by(delta);
        }
    }

    fn advance_animations_by(&mut self, delta: std::time::Duration) -> bool {
        let delta = delta.mul_f64(self.animation_speed);
        let visible_workspaces = self
            .space
            .outputs()
            .filter_map(|output| self.output_ids.get(output))
            .filter_map(|output| self.output_workspaces.active_workspace(*output))
            .collect::<HashSet<_>>();
        let windows = self
            .space
            .elements()
            .filter_map(|window| {
                self.window_ids
                    .get(window)
                    .copied()
                    .map(|id| (window.clone(), id))
            })
            .collect::<Vec<_>>();
        let mut active_animation = false;

        let mut ready_to_close = Vec::new();
        for (id, animation) in &mut self.closing_windows {
            if animation.advance(delta) {
                ready_to_close.push(*id);
            } else if !animation.close_sent {
                active_animation = true;
            }
        }
        for id in ready_to_close {
            self.send_window_close(id);
        }

        for (workspace, viewport) in &mut self.viewport_animations {
            if !visible_workspaces.contains(workspace) {
                continue;
            }
            if self.animations_enabled {
                active_animation |= viewport.advance(delta, self.viewport_spring_config);
            } else {
                viewport.snap();
            }
        }

        let mut settled_coupled_widths = Vec::new();
        for (window, id) in windows {
            let Some(geometry) = self.window_geometry.get_mut(&id) else {
                continue;
            };
            let zooming = geometry.is_zooming();

            let coupled_target = self
                .viewport_coupled_widths
                .get(&id)
                .map(|(_, width)| width.target);
            if coupled_target.is_some() {
                geometry.visual.target.width = geometry.visual.current.width;
                geometry.visual.velocity.width = 0.0;
            }
            active_animation |=
                geometry.advance(delta, self.spring_config, self.animations_enabled);
            if let Some(target) = coupled_target {
                geometry.visual.target.width = target;
            }
            if let Some((workspace, world_x)) = self.scrolling_world_x.get_mut(&id)
                && let Some(viewport) = self.viewport_animations.get(workspace)
            {
                if zooming {
                    world_x.current = geometry.visual.current.x + viewport.current;
                    world_x.velocity = geometry.visual.velocity.x + viewport.velocity;
                } else if self.animations_enabled {
                    active_animation |= world_x.advance(delta, self.spring_config);
                } else {
                    world_x.snap();
                }
                if !zooming {
                    geometry.visual.current.x = world_x.current - viewport.current;
                    geometry.visual.velocity.x = world_x.velocity - viewport.velocity;
                }
            }
            if let Some((_, width)) = self.viewport_coupled_widths.get_mut(&id) {
                let width_active = if self.animations_enabled {
                    width.advance(delta, self.viewport_spring_config)
                } else {
                    width.snap();
                    false
                };
                active_animation |= width_active;
                geometry.visual.current.width = width.current;
                geometry.visual.velocity.width = width.velocity;
                if !width_active {
                    settled_coupled_widths.push(id);
                }
            }
            if let Some(size) = geometry.client.expire_wait(self.start_time.elapsed()) {
                tracing::warn!(
                    ?id,
                    configured_width = size.width,
                    configured_height = size.height,
                    committed = ?geometry.client.committed_size,
                    "client did not commit the final configured size within 500 ms"
                );
            }
            if let Some(size) = geometry.presentation_size_request(self.start_time.elapsed())
                && let Some(toplevel) = window.toplevel()
            {
                toplevel.with_pending_state(|state| {
                    state.size = Some((size.width, size.height).into());
                });
                toplevel.send_pending_configure();
            }
            let visual = geometry.visual.current;
            self.space.map_element(
                window,
                (visual.x.round() as i32, visual.y.round() as i32),
                false,
            );
        }
        for id in settled_coupled_widths {
            self.viewport_coupled_widths.remove(&id);
        }
        active_animation |=
            self.overview
                .advance(delta, self.spring_config, self.animations_enabled);
        self.sync_window_stacking();

        if active_animation {
            self.backdrop_generation = self.backdrop_generation.wrapping_add(1);
        }
        active_animation
    }

    pub(crate) fn animations_enabled(&self) -> bool {
        self.animations_enabled
    }

    pub fn record_client_commit(&mut self, window: &Window) {
        let Some(id) = self.window_ids.get(window) else {
            return;
        };
        let Some(size) = client_size(window) else {
            return;
        };
        let Some(geometry) = self.window_geometry.get_mut(id) else {
            return;
        };

        let matches_target = geometry.client.commit(size);
        tracing::debug!(
            ?id,
            ?size,
            matches_target,
            "recorded client geometry commit"
        );
    }

    pub fn focus_direction(&mut self, direction: Direction) {
        if self.focus_overview_direction(direction) {
            return;
        }

        let Some(current) = self.focused_window else {
            return;
        };
        let Some(bounds) = self.output_bounds() else {
            return;
        };
        let Ok(Some(next)) = self
            .workspaces
            .active()
            .layout
            .directional_neighbor(current, direction, bounds)
        else {
            return;
        };

        if let Err(error) = self.workspaces.focus_window(next) {
            tracing::error!(%error, ?next, "failed to update workspace focus");
            return;
        }
        let Some(window) = self
            .window_ids
            .iter()
            .find_map(|(window, id)| (*id == next).then(|| window.clone()))
        else {
            return;
        };
        let Some(surface) = window
            .toplevel()
            .map(|toplevel| toplevel.wl_surface().clone())
        else {
            return;
        };

        self.focused_window = Some(next);
        self.raise_window(&window, true);
        self.seat
            .get_keyboard()
            .expect("seat has a keyboard")
            .set_focus(
                self,
                Some(surface),
                smithay::utils::SERIAL_COUNTER.next_serial(),
            );

        for window in self.space.elements() {
            if let Some(toplevel) = window.toplevel() {
                toplevel.send_pending_configure();
            }
        }

        self.relayout();
    }

    pub fn move_direction(&mut self, direction: Direction) {
        let Some(current) = self.focused_window else {
            return;
        };
        let Some(bounds) = self.output_bounds() else {
            return;
        };

        match self
            .workspaces
            .active_mut()
            .layout
            .move_window(current, direction, bounds)
        {
            Ok(true) => self.relayout(),
            Ok(false) => {}
            Err(error) => tracing::error!(%error, ?current, "failed to move tiled window"),
        }
    }

    pub fn resize_direction(&mut self, direction: Direction) {
        const RESIZE_STEP: f64 = 0.05;

        let Some(current) = self.focused_window else {
            return;
        };

        match self
            .workspaces
            .active_mut()
            .layout
            .resize_window(current, direction, RESIZE_STEP)
        {
            Ok(true) => self.relayout(),
            Ok(false) => {}
            Err(error) => tracing::error!(%error, ?current, "failed to resize tiled window"),
        }
    }

    pub fn toggle_layout_mode(&mut self) {
        let Some(bounds) = self.output_bounds() else {
            return;
        };
        let mode = match self.workspaces.active().layout.mode() {
            LayoutMode::Scrolling => LayoutMode::Tree,
            LayoutMode::Tree => LayoutMode::Scrolling,
        };

        match self.workspaces.set_active_layout_mode(mode, bounds) {
            Ok(true) => self.relayout(),
            Ok(false) => {}
            Err(error) => tracing::error!(%error, ?mode, "failed to change layout mode"),
        }
    }

    pub fn consume_focused_window(&mut self) {
        let Some(window) = self.focused_window else {
            return;
        };
        let Some(bounds) = self.output_bounds() else {
            return;
        };
        if self.workspaces.active().layout.mode() != LayoutMode::Scrolling {
            return;
        }
        let target = self
            .workspaces
            .active()
            .layout
            .directional_neighbor(window, Direction::Left, bounds)
            .ok()
            .flatten()
            .or_else(|| {
                self.workspaces
                    .active()
                    .layout
                    .directional_neighbor(window, Direction::Right, bounds)
                    .ok()
                    .flatten()
            });
        let Some(target) = target else {
            return;
        };

        if let Err(error) = self.workspaces.stack_window(window, target) {
            tracing::error!(%error, ?window, ?target, "failed to consume window into column");
            return;
        }
        self.relayout();
    }

    pub fn expel_focused_window(&mut self) {
        let Some(window) = self.focused_window else {
            return;
        };

        match self.workspaces.extract_window(window) {
            Ok(true) => self.relayout(),
            Ok(false) => {}
            Err(error) => tracing::error!(%error, ?window, "failed to expel window from column"),
        }
    }

    pub fn cycle_focused_column_width(&mut self) {
        let Some(window) = self.focused_window else {
            return;
        };

        match self
            .workspaces
            .cycle_column_width(window, &self.column_width_presets)
        {
            Ok(true) => {
                self.pending_column_width_cycles.insert(window);
                self.relayout();
            }
            Ok(false) => {}
            Err(error) => tracing::error!(%error, ?window, "failed to cycle column width"),
        }
    }

    pub fn center_focused_column(&mut self) {
        let Some(window) = self.focused_window else {
            return;
        };
        let Some(bounds) = self.output_bounds() else {
            return;
        };
        let constraints = self.window_constraints();

        match self
            .workspaces
            .center_window(window, bounds, self.gap_config, &constraints)
        {
            Ok(true) => self.relayout(),
            Ok(false) => {}
            Err(error) => tracing::error!(%error, ?window, "failed to center column"),
        }
    }

    pub fn toggle_focused_floating(&mut self) {
        let Some(window) = self.focused_window else {
            return;
        };
        let Some(bounds) = self.output_bounds() else {
            return;
        };
        let floating_rect = match self.workspaces.placement(window) {
            Some(WindowPlacement::Tiled) => self
                .tiled_layout(bounds)
                .ok()
                .and_then(|layout| layout.geometry.get(&window).copied())
                .unwrap_or_else(|| centered_floating_rect(bounds)),
            Some(WindowPlacement::Floating { rect }) => rect,
            None => return,
        };
        let axis = self
            .workspaces
            .active()
            .layout
            .automatic_axis(Some(window), bounds)
            .unwrap_or(Axis::Horizontal);

        if let Err(error) = self
            .workspaces
            .toggle_floating(window, floating_rect, axis, 0.5)
        {
            tracing::error!(%error, ?window, "failed to toggle floating window");
            return;
        }

        self.relayout();
    }

    pub fn toggle_focused_fullscreen(&mut self) {
        let Some(window) = self.focused_window else {
            return;
        };

        if let Err(error) = self.workspaces.toggle_fullscreen(window) {
            tracing::error!(%error, ?window, "failed to toggle fullscreen window");
            return;
        }

        self.relayout();
    }

    pub fn toggle_focused_maximized(&mut self) {
        let Some(window) = self.focused_window else {
            return;
        };
        // Super+F from true fullscreen enters decorated maximization.
        let fullscreen = self
            .workspaces
            .workspace_for_window(window)
            .and_then(|workspace| self.workspaces.workspace(workspace))
            .is_some_and(|workspace| workspace.fullscreen == Some(window));
        let enabled = fullscreen || !self.maximized_windows.contains(&window);
        self.set_window_maximized(window, enabled);
    }

    pub fn set_window_maximized(&mut self, window: WindowId, enabled: bool) {
        if self.workspaces.workspace_for_window(window).is_none() {
            return;
        }
        if enabled {
            let _ = self.workspaces.set_fullscreen(window, false);
            self.maximized_windows.insert(window);
        } else {
            self.maximized_windows.remove(&window);
        }
        self.relayout();
    }

    pub(crate) fn output_has_fullscreen(&self, output: &Output) -> bool {
        !self.overview.is_presenting()
            && self
                .output_id(output)
                .and_then(|id| self.output_workspaces.active_workspace(id))
                .and_then(|workspace| self.workspaces.workspace(workspace))
                .is_some_and(|workspace| workspace.fullscreen.is_some())
    }

    pub fn close_focused_window(&mut self) {
        let Some(focused) = self.focused_window else {
            return;
        };

        if !self.animations_enabled {
            self.send_window_close(focused);
            return;
        }

        self.closing_windows.entry(focused).or_default();
        crate::backends::direct::render_all(self);
    }

    pub(crate) fn close_managed_window(&mut self, id: WindowId) -> bool {
        if !self.window_ids.values().any(|window_id| *window_id == id) {
            return false;
        }

        if self.animations_enabled {
            self.closing_windows.entry(id).or_default();
            crate::backends::direct::render_all(self);
        } else {
            self.send_window_close(id);
        }

        true
    }

    pub(crate) fn activate_managed_window(&mut self, id: WindowId) -> bool {
        let Some(window) = self
            .window_ids
            .iter()
            .find_map(|(window, window_id)| (*window_id == id).then(|| window.clone()))
        else {
            return false;
        };
        let Some(workspace) = self.workspaces.workspace_for_window(id) else {
            return false;
        };

        if let Some(output) = self.output_workspaces.output_for_workspace(workspace) {
            self.activate_output_workspace(output, workspace);
        } else if let Some(output) = self.output_workspaces.focused_output() {
            if self
                .output_workspaces
                .assign_workspace(output, workspace)
                .is_err()
            {
                return false;
            }
            self.activate_output_workspace(output, workspace);
        } else if self.workspaces.activate(workspace).is_err() {
            return false;
        }

        if self.workspaces.focus_window(id).is_err() {
            return false;
        }

        self.focused_window = Some(id);
        self.raise_window(&window, true);
        self.relayout();
        self.restore_keyboard_focus();
        true
    }

    pub(crate) fn activate_managed_workspace(&mut self, workspace: WorkspaceId) -> bool {
        if self.workspaces.workspace(workspace).is_none() {
            return false;
        }

        let output = self
            .output_workspaces
            .output_for_workspace(workspace)
            .or_else(|| self.output_workspaces.focused_output());
        let Some(output) = output else {
            return false;
        };

        if self
            .output_workspaces
            .output_for_workspace(workspace)
            .is_none()
            && self
                .output_workspaces
                .assign_workspace(output, workspace)
                .is_err()
        {
            return false;
        }

        self.activate_output_workspace(output, workspace);
        self.relayout();
        self.restore_keyboard_focus();
        true
    }

    pub(crate) fn closing_visual(&self, id: WindowId) -> (f64, f32) {
        let Some(animation) = self.closing_windows.get(&id) else {
            return (1.0, 1.0);
        };
        let eased = smoothstep(animation.progress);

        (1.0 - eased * 0.02, (1.0 - eased) as f32)
    }

    fn send_window_close(&self, id: WindowId) {
        let Some(toplevel) = self.window_ids.iter().find_map(|(window, window_id)| {
            (*window_id == id).then(|| window.toplevel()).flatten()
        }) else {
            return;
        };

        toplevel.send_close();
    }

    pub fn set_window_fullscreen(&mut self, window: WindowId, enabled: bool) {
        match self.workspaces.set_fullscreen(window, enabled) {
            Ok(true) => self.relayout(),
            Ok(false) => {}
            Err(error) => {
                tracing::error!(%error, ?window, enabled, "failed to set fullscreen window")
            }
        }
    }

    pub fn is_floating_window(&self, window: &Window) -> bool {
        self.window_ids
            .get(window)
            .and_then(|id| self.workspaces.placement(*id))
            .is_some_and(|placement| matches!(placement, WindowPlacement::Floating { .. }))
    }

    pub fn set_floating_window_geometry(
        &mut self,
        window: &Window,
        location: Point<i32, Logical>,
        size: Size<i32, Logical>,
    ) {
        let Some(id) = self.window_ids.get(window).copied() else {
            return;
        };
        let rect = Rect::new(
            location.x as f64,
            location.y as f64,
            size.w.max(1) as f64,
            size.h.max(1) as f64,
        );

        if let Err(error) = self.workspaces.set_floating_rect(id, rect) {
            tracing::error!(%error, ?id, "failed to update floating window geometry");
            return;
        }

        // Direct manipulation follows the pointer, including while the client is
        // still drawing its next buffer. Rendering and hit testing share this rect.
        let geometry = self
            .window_geometry
            .entry(id)
            .or_insert_with(|| WindowGeometry::new(rect, client_size(window)));
        if let Some(size) = geometry.follow_pointer(rect, self.start_time.elapsed())
            && let Some(toplevel) = window.toplevel()
        {
            toplevel.with_pending_state(|state| {
                state.size = Some((size.width, size.height).into());
            });
            toplevel.send_pending_configure();
        }
        self.scrolling_world_x.remove(&id);
        self.viewport_coupled_widths.remove(&id);
        self.space.map_element(window.clone(), location, false);
        self.sync_window_stacking();
        crate::backends::direct::render_all(self);
    }

    pub fn switch_workspace(&mut self, index: u32) {
        let workspace = match self.workspaces.ensure_numeric(index) {
            Ok(workspace) => workspace,
            Err(error) => {
                tracing::error!(%error, index, "failed to switch workspace");
                return;
            }
        };
        let Some(output) = self.output_workspaces.focused_output() else {
            return;
        };
        let owner = match self.output_workspaces.switch_workspace(output, workspace) {
            Ok(ferese_core::WorkspaceSwitch::Activated(output))
            | Ok(ferese_core::WorkspaceSwitch::FocusedExisting(output)) => output,
            Err(error) => {
                tracing::error!(%error, index, "failed to assign workspace to output");
                return;
            }
        };
        self.activate_output_workspace(owner, workspace);

        self.relayout();
        self.restore_keyboard_focus();
    }

    pub fn move_focused_to_workspace(&mut self, index: u32) {
        let Some(window) = self.focused_window else {
            return;
        };
        let axis = self
            .output_bounds()
            .and_then(|bounds| {
                let workspace = self.workspaces.ensure_numeric(index).ok()?;
                let target = self.workspaces.workspace(workspace)?;
                target
                    .layout
                    .automatic_axis(target.last_focused, bounds)
                    .ok()
            })
            .unwrap_or(Axis::Horizontal);

        let destination = match self
            .workspaces
            .move_window_to_numeric(window, index, axis, 0.5)
        {
            Ok(destination) => destination,
            Err(error) => {
                tracing::error!(%error, ?window, index, "failed to move window to workspace");
                return;
            }
        };
        if let Some(output) = self.output_workspaces.focused_output()
            && let Err(error) = self.output_workspaces.assign_workspace(output, destination)
        {
            tracing::error!(%error, ?destination, "failed to assign destination workspace");
        }

        self.focused_window = self.workspaces.active().last_focused;
        self.relayout();
        self.restore_keyboard_focus();
    }

    pub(crate) fn restore_keyboard_focus(&mut self) {
        let surface = self.focused_window.and_then(|focused| {
            self.window_ids.iter().find_map(|(window, id)| {
                (*id == focused)
                    .then(|| window.toplevel())
                    .flatten()
                    .map(|toplevel| toplevel.wl_surface().clone())
            })
        });

        self.seat
            .get_keyboard()
            .expect("seat has a keyboard")
            .set_focus(self, surface, smithay::utils::SERIAL_COUNTER.next_serial());
    }

    fn output_bounds(&self) -> Option<Rect> {
        let output = self
            .focused_output()
            .or_else(|| self.space.outputs().next())?;
        self.output_bounds_for(output)
    }

    pub(crate) fn output_bounds_for(&self, output: &Output) -> Option<Rect> {
        let geometry = self.space.output_geometry(output)?;
        let zone = layer_map_for_output(output).non_exclusive_zone();

        Some(Rect::new(
            (geometry.loc.x + zone.loc.x) as f64,
            (geometry.loc.y + zone.loc.y) as f64,
            zone.size.w.max(0) as f64,
            zone.size.h.max(0) as f64,
        ))
    }

    fn full_output_bounds_for(&self, output: &Output) -> Option<Rect> {
        let geometry = self.space.output_geometry(output)?;

        Some(Rect::new(
            geometry.loc.x as f64,
            geometry.loc.y as f64,
            geometry.size.w as f64,
            geometry.size.h as f64,
        ))
    }

    fn arrange_layers(&self) {
        let outputs = self.space.outputs().cloned().collect::<Vec<_>>();

        for output in outputs {
            layer_map_for_output(&output).arrange();
        }
    }

    fn logical_window_rect(&mut self, window: WindowId, bounds: Rect) -> Option<Rect> {
        let workspace = self.workspaces.workspace_for_window(window)?;
        let fullscreen = self.workspaces.workspace(workspace)?.fullscreen;

        if fullscreen == Some(window) {
            return Some(bounds);
        }

        let constraints = self.window_constraints();
        let focused = self.focused_window;
        match self.workspaces.placement(window)? {
            WindowPlacement::Tiled => self
                .workspaces
                .workspace_mut(workspace)?
                .layout
                .geometry_with_constraints(bounds, self.gap_config, &constraints, focused)
                .ok()?
                .geometry
                .get(&window)
                .copied(),
            WindowPlacement::Floating { rect } => Some(rect),
        }
    }

    fn tiled_layout(&mut self, bounds: Rect) -> Result<LayoutResult, ferese_layout::LayoutError> {
        let constraints = self.window_constraints();
        let focused = self.focused_window;
        self.workspaces
            .active_mut()
            .layout
            .geometry_with_constraints(bounds, self.gap_config, &constraints, focused)
    }

    fn window_constraints(&self) -> HashMap<WindowId, SizeConstraints> {
        self.window_ids
            .iter()
            .filter_map(|(window, id)| {
                let toplevel = window.toplevel()?;
                let (minimum, maximum) = with_states(toplevel.wl_surface(), |states| {
                    let mut cached = states.cached_state.get::<SurfaceCachedState>();
                    let state = cached.current();
                    (state.min_size, state.max_size)
                });

                Some((
                    *id,
                    SizeConstraints {
                        min_width: minimum.w.max(1) as f64,
                        min_height: minimum.h.max(1) as f64,
                        max_width: (maximum.w > 0).then_some(maximum.w as f64),
                        max_height: (maximum.h > 0).then_some(maximum.h as f64),
                    },
                ))
            })
            .collect()
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

fn smoothstep(progress: f64) -> f64 {
    let progress = progress.clamp(0.0, 1.0);

    progress * progress * (3.0 - 2.0 * progress)
}

fn restored_scrolling_world_x(
    had_geometry: bool,
    visual_x: f64,
    viewport_x: f64,
    target_world_x: f64,
) -> f64 {
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
    let size = window.geometry().size;

    (size.w > 0 && size.h > 0).then_some(ClientSize {
        width: size.w,
        height: size.h,
    })
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
    use super::*;

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

        assert!(!animation.advance(Duration::from_millis(70)));
        assert_eq!(animation.progress, 0.5);
        assert!(animation.advance(Duration::from_millis(70)));
        assert!(animation.close_sent);
        assert!(!animation.advance(Duration::from_secs(1)));
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
        assert_eq!(
            restored_scrolling_world_x(false, 10.0, 495.0, 1_000.0),
            1_000.0
        );
    }
}
