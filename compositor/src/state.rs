use std::{
    collections::{HashMap, HashSet},
    error::Error,
    ffi::OsString,
    sync::Arc,
    time::Instant,
};

use ferese_core::WorkspaceSet;
use ferese_layout::{Axis, Direction, GapConfig, Rect, WindowId};

use smithay::{
    desktop::{PopupManager, Space, Window, WindowSurfaceType},
    input::{Seat, SeatState},
    reexports::{
        calloop::{EventLoop, Interest, LoopSignal, Mode, PostAction, generic::Generic},
        wayland_server::{
            Display, DisplayHandle,
            backend::{ClientData, ClientId, DisconnectReason},
            protocol::wl_surface::WlSurface,
        },
    },
    utils::{Logical, Point},
    wayland::{
        compositor::{CompositorClientState, CompositorState},
        fractional_scale::FractionalScaleManagerState,
        output::OutputManagerState,
        pointer_constraints::PointerConstraintsState,
        presentation::PresentationState,
        relative_pointer::RelativePointerManagerState,
        selection::{data_device::DataDeviceState, primary_selection::PrimarySelectionState},
        shell::xdg::XdgShellState,
        shell::xdg::decoration::XdgDecorationState,
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
    pub focused_window: Option<WindowId>,
    pub intercepted_keys: HashSet<smithay::input::keyboard::Keycode>,
    next_window_id: u64,
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
        let xdg_activation_state = XdgActivationState::new::<Self>(&display_handle);
        let mut seat_state = SeatState::new();
        let mut seat = seat_state.new_wl_seat(&display_handle, "ferese-winit");
        seat.add_keyboard(Default::default(), 200, 25)?;
        seat.add_pointer();
        let socket_name = Self::init_wayland_listener(display, event_loop)?;

        Ok(Self {
            start_time: Instant::now(),
            socket_name,
            display_handle,
            loop_signal: event_loop.get_signal(),
            space: Space::default(),
            workspaces: WorkspaceSet::default(),
            window_ids: HashMap::new(),
            focused_window: None,
            intercepted_keys: HashSet::new(),
            next_window_id: 1,
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
        self.space
            .element_under(position)
            .and_then(|(window, location)| {
                window
                    .surface_under(position - location.to_f64(), WindowSurfaceType::ALL)
                    .map(|(surface, surface_location)| {
                        (surface, (surface_location + location).to_f64())
                    })
            })
    }

    pub fn add_tiled_window(&mut self, window: Window) {
        let id = WindowId(self.next_window_id);
        self.next_window_id += 1;

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
        self.focused_window = Some(id);
        self.space.map_element(window, (0, 0), true);
        self.relayout();
    }

    pub fn remove_tiled_window(&mut self, window: &Window) {
        let Some(id) = self.window_ids.remove(window) else {
            return;
        };

        self.space.unmap_elem(window);
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
        let active = self.workspaces.active_id();
        let inactive = self
            .window_ids
            .iter()
            .filter(|(_, id)| self.workspaces.workspace_for_window(**id) != Some(active))
            .map(|(window, _)| window.clone())
            .collect::<Vec<_>>();

        for window in inactive {
            self.space.unmap_elem(&window);
        }

        let geometry = match self
            .workspaces
            .active()
            .layout
            .geometry_with_gaps(bounds, GapConfig::default())
        {
            Ok(geometry) => geometry,
            Err(error) => {
                tracing::error!(%error, "failed to compute tiled geometry");
                return;
            }
        };
        let placements = self
            .window_ids
            .iter()
            .filter_map(|(window, id)| geometry.get(id).map(|rect| (window.clone(), *rect)))
            .collect::<Vec<_>>();

        for (window, rect) in placements {
            let location = (rect.x.round() as i32, rect.y.round() as i32);
            let size = (
                (rect.width.round() as i32).max(1),
                (rect.height.round() as i32).max(1),
            );

            self.space.map_element(window.clone(), location, false);
            if let Some(toplevel) = window.toplevel() {
                toplevel.with_pending_state(|state| state.size = Some(size.into()));
                toplevel.send_pending_configure();
            }
        }
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

    fn restore_keyboard_focus(&mut self) {
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
}

#[derive(Default)]
pub struct ClientState {
    pub compositor_state: CompositorClientState,
}

impl ClientData for ClientState {
    fn initialized(&self, _client_id: ClientId) {}
    fn disconnected(&self, _client_id: ClientId, _reason: DisconnectReason) {}
}
