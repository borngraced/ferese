use std::{
    collections::{HashMap, HashSet},
    error::Error,
    ffi::OsString,
    sync::Arc,
    time::{Duration, Instant},
};

use ferese_animation::{ClientSize, SpringConfig, WindowGeometry};
use ferese_core::{WindowPlacement, WorkspaceSet};
use ferese_layout::{Axis, Direction, GapConfig, LayoutResult, Rect, SizeConstraints, WindowId};

use smithay::{
    backend::drm::DrmEventTime,
    desktop::{
        LayerSurface, PopupManager, Space, Window, WindowSurfaceType, layer_map_for_output,
        utils::send_frames_surface_tree,
    },
    input::{Seat, SeatState, pointer::CursorImageStatus},
    output::Output,
    reexports::{
        calloop::{EventLoop, Interest, LoopSignal, Mode, PostAction, generic::Generic},
        wayland_protocols::xdg::shell::server::xdg_toplevel,
        wayland_server::{
            Display, DisplayHandle,
            backend::{ClientData, ClientId, DisconnectReason},
            protocol::wl_surface::WlSurface,
        },
    },
    utils::{Logical, Point, Size},
    wayland::{
        compositor::{CompositorClientState, CompositorState, with_states},
        fractional_scale::FractionalScaleManagerState,
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
        socket::ListeningSocketSource,
        viewporter::ViewporterState,
        xdg_activation::XdgActivationState,
    },
};

pub struct Ferese {
    pub start_time: Instant,
    pub socket_name: OsString,
    pub display_handle: DisplayHandle,
    pub loop_signal: LoopSignal,
    pub space: Space<Window>,
    pub workspaces: WorkspaceSet,
    pub window_ids: HashMap<Window, WindowId>,
    pub window_geometry: HashMap<WindowId, WindowGeometry>,
    pub focused_window: Option<WindowId>,
    pub cursor_status: CursorImageStatus,
    pub intercepted_keys: HashSet<smithay::input::keyboard::Keycode>,
    pub direct_backend: Option<crate::backends::direct::DirectBackendState>,
    next_window_id: u64,
    last_animation_tick: Instant,
    pub popups: PopupManager,
    pub seat: Seat<Self>,
    pub compositor_state: CompositorState,
    pub data_device_state: DataDeviceState,
    pub decoration_state: XdgDecorationState,
    pub fractional_scale_state: FractionalScaleManagerState,
    pub output_manager_state: OutputManagerState,
    pub pointer_constraints_state: PointerConstraintsState,
    pub presentation_state: PresentationState,
    pub primary_selection_state: PrimarySelectionState,
    pub relative_pointer_state: RelativePointerManagerState,
    pub seat_state: SeatState<Self>,
    pub shm_state: ShmState,
    pub viewporter_state: ViewporterState,
    pub layer_shell_state: WlrLayerShellState,
    pub xdg_activation_state: XdgActivationState,
    pub xdg_shell_state: XdgShellState,
}

impl Ferese {
    pub fn new(
        event_loop: &mut EventLoop<Self>,
        display: Display<Self>,
    ) -> Result<Self, Box<dyn Error>> {
        let display_handle = display.handle();
        let compositor_state = CompositorState::new::<Self>(&display_handle);
        let xdg_shell_state = XdgShellState::new::<Self>(&display_handle);
        let decoration_state = XdgDecorationState::new::<Self>(&display_handle);
        let fractional_scale_state = FractionalScaleManagerState::new::<Self>(&display_handle);
        let shm_state = ShmState::new::<Self>(&display_handle, Vec::new());
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
        let mut seat_state = SeatState::new();
        let mut seat = seat_state.new_wl_seat(&display_handle, "ferese-winit");
        seat.add_keyboard(Default::default(), 200, 25)?;
        seat.add_pointer();
        let socket_name = Self::init_wayland_listener(display, event_loop)?;

        let start_time = Instant::now();

        Ok(Self {
            start_time,
            socket_name,
            display_handle,
            loop_signal: event_loop.get_signal(),
            space: Space::default(),
            workspaces: WorkspaceSet::default(),
            window_ids: HashMap::new(),
            window_geometry: HashMap::new(),
            focused_window: None,
            cursor_status: CursorImageStatus::default_named(),
            intercepted_keys: HashSet::new(),
            direct_backend: None,
            next_window_id: 1,
            last_animation_tick: start_time,
            popups: PopupManager::default(),
            seat,
            compositor_state,
            data_device_state,
            decoration_state,
            fractional_scale_state,
            output_manager_state,
            pointer_constraints_state,
            presentation_state,
            primary_selection_state,
            relative_pointer_state,
            seat_state,
            shm_state,
            viewporter_state,
            layer_shell_state,
            xdg_activation_state,
            xdg_shell_state,
        })
    }

    fn init_wayland_listener(
        display: Display<Self>,
        event_loop: &mut EventLoop<Self>,
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
        self.space.elements().rev().find_map(|window| {
            let id = self.window_ids.get(window)?;
            let geometry = self.window_geometry.get(id)?;
            let visual = geometry.visual.current;
            let inside_visual = position.x >= visual.x
                && position.y >= visual.y
                && position.x < visual.x + visual.width
                && position.y < visual.y + visual.height;
            if !inside_visual {
                return None;
            }

            let (source_x, source_y) = geometry.inverse_visual_point(position.x, position.y)?;
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
            let visual = self.window_geometry.get(id)?.visual.current;

            (position.x >= visual.x
                && position.y >= visual.y
                && position.x < visual.x + visual.width
                && position.y < visual.y + visual.height)
                .then(|| window.clone())
        })
    }

    pub fn visual_scale_for_window(&self, window: &Window) -> Option<f64> {
        let id = self.window_ids.get(window)?;
        self.window_geometry.get(id)?.visual_scale()
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
        if focus_new_window {
            self.focused_window = Some(id);
        }
        self.space.map_element(window, (0, 0), focus_new_window);
        self.relayout();
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
        self.space.map_element(window, (0, 0), false);
        self.relayout();
    }

    pub fn remove_tiled_window(&mut self, window: &Window) {
        let Some(id) = self.window_ids.remove(window) else {
            return;
        };

        self.space.unmap_elem(window);
        self.window_geometry.remove(&id);
        if let Err(error) = self.workspaces.remove_window(id) {
            tracing::error!(%error, ?id, "failed to remove window from layout");
        }
        if self.focused_window == Some(id) {
            self.focused_window = self.workspaces.active().last_focused;
        }

        self.relayout();
    }

    pub fn relayout(&mut self) {
        let Some(bounds) = self.output_bounds() else {
            return;
        };
        let layout = match self.tiled_layout(bounds) {
            Ok(layout) => layout,
            Err(error) => {
                tracing::error!(%error, "failed to compute tiled geometry");
                return;
            }
        };
        let tiled_geometry = layout.geometry;

        for warning in layout.warnings {
            tracing::warn!(
                window = ?warning.window,
                kind = ?warning.kind,
                requested = warning.requested,
                assigned = warning.assigned,
                "window size constraint could not be satisfied exactly"
            );
        }
        let active = self.workspaces.active_id();
        let fullscreen = self.workspaces.active().fullscreen;
        let hidden = self
            .window_ids
            .iter()
            .filter(|(_, id)| {
                self.workspaces.workspace_for_window(**id) != Some(active)
                    || fullscreen.is_some_and(|fullscreen| fullscreen != **id)
                    || (fullscreen.is_none()
                        && self.workspaces.placement(**id) == Some(WindowPlacement::Tiled)
                        && !tiled_geometry.contains_key(id))
            })
            .map(|(window, _)| window.clone())
            .collect::<Vec<_>>();

        for window in hidden {
            self.space.unmap_elem(&window);
        }

        let placements = self
            .window_ids
            .iter()
            .filter_map(|(window, id)| {
                if self.workspaces.workspace_for_window(*id) != Some(active) {
                    return None;
                }

                let rect = if fullscreen == Some(*id) {
                    bounds
                } else if fullscreen.is_some() {
                    return None;
                } else {
                    match self.workspaces.placement(*id)? {
                        WindowPlacement::Tiled => *tiled_geometry.get(id)?,
                        WindowPlacement::Floating { rect } => rect,
                    }
                };

                Some((window.clone(), *id, rect, fullscreen == Some(*id)))
            })
            .collect::<Vec<_>>();

        let now = self.start_time.elapsed();

        for (window, id, rect, is_fullscreen) in placements {
            let committed_size = client_size(&window);
            let geometry = self
                .window_geometry
                .entry(id)
                .or_insert_with(|| WindowGeometry::new(rect, committed_size));
            let requested_size = geometry.set_logical_target(rect, now);
            let visual = geometry.visual.current;
            let location = (visual.x.round() as i32, visual.y.round() as i32);

            self.space.map_element(window.clone(), location, false);
            if let Some(toplevel) = window.toplevel() {
                let state_changed = toplevel.with_pending_state(|state| {
                    if let Some(size) = requested_size {
                        state.size = Some((size.width, size.height).into());
                    }

                    if is_fullscreen {
                        state.states.set(xdg_toplevel::State::Fullscreen)
                    } else {
                        state.states.unset(xdg_toplevel::State::Fullscreen)
                    }
                });

                if requested_size.is_some() || state_changed {
                    toplevel.send_pending_configure();
                }
            }
        }

        crate::backends::direct::render_all(self);
    }

    pub fn advance_animations(&mut self, now: Instant) -> bool {
        let delta = now.saturating_duration_since(self.last_animation_tick);
        self.last_animation_tick = now;
        self.advance_animations_by(delta)
    }

    pub fn record_drm_presentation(&mut self, time: DrmEventTime, sequence: u32) {
        let delta = self
            .direct_backend
            .as_mut()
            .and_then(|backend| backend.record_presentation(time, sequence));
        if let Some(delta) = delta {
            self.advance_animations_by(delta);
        }
    }

    fn advance_animations_by(&mut self, delta: std::time::Duration) -> bool {
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

        for (window, id) in windows {
            let Some(geometry) = self.window_geometry.get_mut(&id) else {
                continue;
            };

            active_animation |= geometry.advance(delta, SpringConfig::default(), true);
            if let Some(size) = geometry.client.expire_wait(self.start_time.elapsed()) {
                tracing::warn!(
                    ?id,
                    configured_width = size.width,
                    configured_height = size.height,
                    committed = ?geometry.client.committed_size,
                    "client did not commit the final configured size within 500 ms"
                );
            }
            let visual = geometry.visual.current;
            self.space.map_element(
                window,
                (visual.x.round() as i32, visual.y.round() as i32),
                false,
            );
        }

        active_animation
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
        self.space.raise_element(&window, true);
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
        }
    }

    pub fn switch_workspace(&mut self, index: u32) {
        let focus = match self.workspaces.switch_to_numeric(index) {
            Ok(focus) => focus,
            Err(error) => {
                tracing::error!(%error, index, "failed to switch workspace");
                return;
            }
        };

        self.focused_window = focus;
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

        if let Err(error) = self
            .workspaces
            .move_window_to_numeric(window, index, axis, 0.5)
        {
            tracing::error!(%error, ?window, index, "failed to move window to workspace");
            return;
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
        let output = self.space.outputs().next()?;
        let geometry = self.space.output_geometry(output)?;

        Some(Rect::new(
            geometry.loc.x as f64,
            geometry.loc.y as f64,
            geometry.size.w as f64,
            geometry.size.h as f64,
        ))
    }

    fn logical_window_rect(&self, window: WindowId, bounds: Rect) -> Option<Rect> {
        let workspace = self.workspaces.workspace_for_window(window)?;
        let workspace = self.workspaces.workspace(workspace)?;

        if workspace.fullscreen == Some(window) {
            return Some(bounds);
        }

        match self.workspaces.placement(window)? {
            WindowPlacement::Tiled => workspace
                .layout
                .geometry_with_constraints(
                    bounds,
                    GapConfig::default(),
                    &self.window_constraints(),
                    self.focused_window,
                )
                .ok()?
                .geometry
                .get(&window)
                .copied(),
            WindowPlacement::Floating { rect } => Some(rect),
        }
    }

    fn tiled_layout(&self, bounds: Rect) -> Result<LayoutResult, ferese_layout::LayoutError> {
        self.workspaces.active().layout.geometry_with_constraints(
            bounds,
            GapConfig::default(),
            &self.window_constraints(),
            self.focused_window,
        )
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
}

impl ClientData for ClientState {
    fn initialized(&self, _client_id: ClientId) {}
    fn disconnected(&self, _client_id: ClientId, _reason: DisconnectReason) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transient_geometry_is_centered_and_bounded_by_its_parent() {
        let parent = Rect::new(100.0, 50.0, 1_000.0, 800.0);
        let transient = centered_transient_rect(parent);

        assert_eq!(transient, Rect::new(280.0, 210.0, 640.0, 480.0));
    }
}
