use smithay::{
    desktop::{
        PopupKeyboardGrab, PopupKind, PopupManager, PopupPointerGrab, Space, Window,
        find_popup_root_surface, get_popup_toplevel_coords,
    },
    input::{
        Seat,
        pointer::{Focus, GrabStartData},
    },
    reexports::wayland_protocols::xdg::decoration::zv1::server::zxdg_toplevel_decoration_v1::Mode,
    reexports::wayland_server::{
        Resource,
        protocol::{wl_seat, wl_surface::WlSurface},
    },
    utils::{Rectangle, Serial},
    wayland::{
        compositor::with_states,
        shell::xdg::decoration::XdgDecorationHandler,
        shell::xdg::{
            PopupSurface, PositionerState, ToplevelSurface, XdgShellHandler, XdgShellState,
            XdgToplevelSurfaceData,
        },
    },
};

use crate::{
    Ferese,
    grabs::{MoveSurfaceGrab, ResizeEdge, ResizeSurfaceGrab, handle_resize_commit},
};

impl XdgShellHandler for Ferese {
    fn xdg_shell_state(&mut self) -> &mut XdgShellState {
        &mut self.xdg_shell_state
    }

    fn new_toplevel(&mut self, surface: ToplevelSurface) {
        self.add_tiled_window(Window::new_wayland_window(surface));
    }

    fn toplevel_destroyed(&mut self, surface: ToplevelSurface) {
        let Some(window) = self
            .space
            .elements()
            .find(|window| {
                window
                    .toplevel()
                    .is_some_and(|toplevel| toplevel.wl_surface() == surface.wl_surface())
            })
            .cloned()
        else {
            return;
        };

        self.remove_tiled_window(&window);
    }

    fn new_popup(&mut self, surface: PopupSurface, _positioner: PositionerState) {
        self.unconstrain_popup(&surface);
        if let Err(error) = self.popups.track_popup(PopupKind::Xdg(surface)) {
            tracing::warn!(?error, "failed to track xdg popup");
        }
    }

    fn reposition_request(
        &mut self,
        surface: PopupSurface,
        positioner: PositionerState,
        token: u32,
    ) {
        surface.with_pending_state(|state| {
            state.geometry = positioner.get_geometry();
            state.positioner = positioner;
        });
        self.unconstrain_popup(&surface);
        surface.send_repositioned(token);
    }

    fn move_request(&mut self, surface: ToplevelSurface, seat: wl_seat::WlSeat, serial: Serial) {
        let Some(seat) = Seat::from_resource(&seat) else {
            return;
        };
        let Some(start_data) = check_grab(&seat, surface.wl_surface(), serial) else {
            return;
        };
        let Some(pointer) = seat.get_pointer() else {
            return;
        };
        let Some(window) = self
            .space
            .elements()
            .find(|window| {
                window
                    .toplevel()
                    .is_some_and(|toplevel| toplevel.wl_surface() == surface.wl_surface())
            })
            .cloned()
        else {
            return;
        };
        let Some(initial_location) = self.space.element_location(&window) else {
            return;
        };
        pointer.set_grab(
            self,
            MoveSurfaceGrab {
                start_data,
                window,
                initial_location,
            },
            serial,
            Focus::Clear,
        );
    }

    fn resize_request(
        &mut self,
        surface: ToplevelSurface,
        seat: wl_seat::WlSeat,
        serial: Serial,
        edges: smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel::ResizeEdge,
    ) {
        let Some(seat) = Seat::from_resource(&seat) else {
            return;
        };
        let Some(start_data) = check_grab(&seat, surface.wl_surface(), serial) else {
            return;
        };
        let Some(pointer) = seat.get_pointer() else {
            return;
        };
        let Some(window) = self
            .space
            .elements()
            .find(|window| {
                window
                    .toplevel()
                    .is_some_and(|toplevel| toplevel.wl_surface() == surface.wl_surface())
            })
            .cloned()
        else {
            return;
        };
        let Some(location) = self.space.element_location(&window) else {
            return;
        };
        let rect = Rectangle::new(location, window.geometry().size);
        pointer.set_grab(
            self,
            ResizeSurfaceGrab::new(start_data, window, ResizeEdge::from(edges), rect),
            serial,
            Focus::Clear,
        );
    }

    fn grab(&mut self, surface: PopupSurface, seat: wl_seat::WlSeat, serial: Serial) {
        let Some(seat) = Seat::from_resource(&seat) else {
            return;
        };
        let popup = PopupKind::Xdg(surface);
        let Ok(root) = find_popup_root_surface(&popup) else {
            return;
        };
        let popup_grab = match self.popups.grab_popup(root, popup, &seat, serial) {
            Ok(grab) => grab,
            Err(error) => {
                tracing::debug!(?error, "rejected xdg popup grab");
                return;
            }
        };

        if let Some(pointer) = seat.get_pointer() {
            pointer.set_grab(
                self,
                PopupPointerGrab::new(&popup_grab),
                serial,
                Focus::Keep,
            );
        }
        if let Some(keyboard) = seat.get_keyboard() {
            keyboard.set_grab(self, PopupKeyboardGrab::new(&popup_grab), serial);
        }
    }
}

fn check_grab(
    seat: &Seat<Ferese>,
    surface: &WlSurface,
    serial: Serial,
) -> Option<GrabStartData<Ferese>> {
    let pointer = seat.get_pointer()?;
    if !pointer.has_grab(serial) {
        return None;
    }
    let start_data = pointer.grab_start_data()?;
    let (focus, _) = start_data.focus.as_ref()?;
    focus
        .id()
        .same_client_as(&surface.id())
        .then_some(start_data)
}

impl XdgDecorationHandler for Ferese {
    fn new_decoration(&mut self, toplevel: ToplevelSurface) {
        set_decoration_mode(&toplevel, Mode::ClientSide);
    }

    fn request_mode(&mut self, toplevel: ToplevelSurface, mode: Mode) {
        // Phase 1 negotiates decorations but does not yet draw a server frame.
        // Honor CSD and safely fall back to CSD for premature SSD requests.
        let mode = match mode {
            Mode::ClientSide | Mode::ServerSide => Mode::ClientSide,
            _ => Mode::ClientSide,
        };
        set_decoration_mode(&toplevel, mode);
    }

    fn unset_mode(&mut self, toplevel: ToplevelSurface) {
        set_decoration_mode(&toplevel, Mode::ClientSide);
    }
}

fn set_decoration_mode(toplevel: &ToplevelSurface, mode: Mode) {
    toplevel.with_pending_state(|state| state.decoration_mode = Some(mode));
    toplevel.send_pending_configure();
}

pub fn handle_commit(popups: &mut PopupManager, space: &mut Space<Window>, surface: &WlSurface) {
    handle_resize_commit(space, surface);
    if let Some(window) = space
        .elements()
        .find(|window| {
            window
                .toplevel()
                .is_some_and(|toplevel| toplevel.wl_surface() == surface)
        })
        .cloned()
    {
        let configured = with_states(surface, |states| {
            states
                .data_map
                .get::<XdgToplevelSurfaceData>()
                .expect("xdg toplevel state exists")
                .lock()
                .expect("xdg toplevel state is not poisoned")
                .initial_configure_sent
        });
        if !configured {
            window
                .toplevel()
                .expect("mapped window has a toplevel")
                .send_configure();
        }
    }

    popups.commit(surface);
    if let Some(PopupKind::Xdg(popup)) = popups.find_popup(surface)
        && !popup.is_initial_configure_sent()
    {
        popup
            .send_configure()
            .expect("initial popup configure is valid");
    }
}

impl Ferese {
    fn unconstrain_popup(&self, popup: &PopupSurface) {
        let Ok(root) = find_popup_root_surface(&PopupKind::Xdg(popup.clone())) else {
            return;
        };
        let Some(window) = self.space.elements().find(|window| {
            window
                .toplevel()
                .is_some_and(|toplevel| toplevel.wl_surface() == &root)
        }) else {
            return;
        };
        let Some(output) = self.space.outputs().next() else {
            return;
        };
        let Some(output_geometry) = self.space.output_geometry(output) else {
            return;
        };
        let Some(window_geometry) = self.space.element_geometry(window) else {
            return;
        };

        let mut target = output_geometry;
        target.loc -= get_popup_toplevel_coords(&PopupKind::Xdg(popup.clone()));
        target.loc -= window_geometry.loc;
        popup.with_pending_state(|state| {
            state.geometry = state.positioner.get_unconstrained_geometry(target);
        });
    }
}
