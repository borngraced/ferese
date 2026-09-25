use smithay::{
    desktop::{
        PopupKeyboardGrab, PopupKind, PopupManager, PopupPointerGrab, Space, Window,
        WindowSurfaceType, find_popup_root_surface, get_popup_toplevel_coords,
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
    utils::Serial,
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
        let parent = surface.parent().and_then(|parent| {
            self.window_ids.iter().find_map(|(window, id)| {
                window
                    .toplevel()
                    .is_some_and(|toplevel| toplevel.wl_surface() == &parent)
                    .then_some(*id)
            })
        });
        let window = Window::new_wayland_window(surface);

        if let Some(parent) = parent {
            self.add_transient_window(window, parent);
        } else {
            self.add_tiled_window(window);
        }
    }

    fn parent_changed(&mut self, surface: ToplevelSurface) {
        let Some(parent) = surface.parent() else {
            return;
        };
        let parent = self.window_ids.iter().find_map(|(window, id)| {
            window
                .toplevel()
                .is_some_and(|toplevel| toplevel.wl_surface() == &parent)
                .then_some(*id)
        });
        let child = self
            .space
            .elements()
            .find(|window| {
                window
                    .toplevel()
                    .is_some_and(|toplevel| toplevel.wl_surface() == surface.wl_surface())
            })
            .cloned();

        if let (Some(parent), Some(child)) = (parent, child) {
            self.make_window_transient(&child, parent);
        }
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
        let Some(initial_rect) = self.visual_rect_for_window(&window) else {
            return;
        };
        if !self.is_floating_window(&window) {
            return;
        }
        pointer.set_grab(
            self,
            MoveSurfaceGrab {
                start_data,
                window,
                initial_location: initial_rect.loc,
                initial_size: initial_rect.size,
                finished: false,
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
        let Some(rect) = self.visual_rect_for_window(&window) else {
            return;
        };
        if !self.is_floating_window(&window) {
            return;
        }
        pointer.set_grab(
            self,
            ResizeSurfaceGrab::new(start_data, window, ResizeEdge::from(edges), rect),
            serial,
            Focus::Clear,
        );
    }

    fn fullscreen_request(
        &mut self,
        surface: ToplevelSurface,
        _output: Option<smithay::reexports::wayland_server::protocol::wl_output::WlOutput>,
    ) {
        if let Some(window) = self.window_ids.iter().find_map(|(window, id)| {
            window
                .toplevel()
                .is_some_and(|toplevel| toplevel.wl_surface() == surface.wl_surface())
                .then_some(*id)
        }) {
            self.set_window_fullscreen(window, true);
        }
    }

    fn unfullscreen_request(&mut self, surface: ToplevelSurface) {
        if let Some(window) = self.window_ids.iter().find_map(|(window, id)| {
            window
                .toplevel()
                .is_some_and(|toplevel| toplevel.wl_surface() == surface.wl_surface())
                .then_some(*id)
        }) {
            self.set_window_fullscreen(window, false);
        }
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
        let mode = self.decoration_mode_for(&toplevel);
        toplevel.with_pending_state(|state| state.decoration_mode = Some(mode));

        // This must be unconditional: the initial xdg-toplevel configure may
        // already contain the same mode before the decoration object exists.
        // `send_pending_configure` would then see no state change and omit the
        // decoration configure, leaving clients to fall back to CSD.
        toplevel.send_configure();
    }

    fn request_mode(&mut self, toplevel: ToplevelSurface, _mode: Mode) {
        let mode = self.decoration_mode_for(&toplevel);
        set_decoration_mode(&toplevel, mode);
    }

    fn unset_mode(&mut self, toplevel: ToplevelSurface) {
        let mode = self.decoration_mode_for(&toplevel);
        set_decoration_mode(&toplevel, mode);
    }
}

impl Ferese {
    fn decoration_mode_for(&self, toplevel: &ToplevelSurface) -> Mode {
        self.space
            .elements()
            .find(|window| {
                window
                    .toplevel()
                    .is_some_and(|candidate| candidate.wl_surface() == toplevel.wl_surface())
            })
            .filter(|window| self.is_floating_window(window))
            .map_or(Mode::ServerSide, |_| Mode::ClientSide)
    }
}

fn set_decoration_mode(toplevel: &ToplevelSurface, mode: Mode) {
    toplevel.with_pending_state(|state| state.decoration_mode = Some(mode));
    toplevel.send_pending_configure();
}

pub fn handle_commit(popups: &mut PopupManager, space: &mut Space<Window>, surface: &WlSurface) {
    handle_resize_commit(surface);
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

pub fn apply_initial_window_rules(state: &mut Ferese, window: &Window) {
    let Some(toplevel) = window.toplevel() else {
        return;
    };
    let (app_id, title, transient) = with_states(toplevel.wl_surface(), |states| {
        let attributes = states
            .data_map
            .get::<XdgToplevelSurfaceData>()
            .expect("xdg toplevel state exists")
            .lock()
            .expect("xdg toplevel state is not poisoned");

        (
            attributes.app_id.clone(),
            attributes.title.clone(),
            attributes.parent.is_some(),
        )
    });

    state.apply_initial_window_rules(window, app_id.as_deref(), title.as_deref(), transient);
}

impl Ferese {
    pub(crate) fn unconstrain_popup(&self, popup: &PopupSurface) {
        let Ok(root) = find_popup_root_surface(&PopupKind::Xdg(popup.clone())) else {
            return;
        };
        let window = self.space.elements().find(|window| {
            window
                .toplevel()
                .is_some_and(|toplevel| toplevel.wl_surface() == &root)
        });
        let geometry = (|| {
            if let Some(window) = window {
                let output = self.space.outputs().next()?;
                let output_geometry = self.space.output_geometry(output)?;
                let window_geometry = self.space.element_geometry(window)?;

                Some((output_geometry, window_geometry.loc))
            } else {
                let layer = self
                    .space
                    .layer_for_surface(&root, WindowSurfaceType::TOPLEVEL)?;

                self.space.outputs().find_map(|output| {
                    let output_geometry = self.space.output_geometry(output)?;
                    let layer_geometry =
                        smithay::desktop::layer_map_for_output(output).layer_geometry(&layer)?;

                    Some((output_geometry, output_geometry.loc + layer_geometry.loc))
                })
            }
        })();
        let Some((mut target, root_location)) = geometry else {
            return;
        };

        target.loc -= get_popup_toplevel_coords(&PopupKind::Xdg(popup.clone()));
        target.loc -= root_location;
        popup.with_pending_state(|state| {
            state.geometry = state.positioner.get_unconstrained_geometry(target);
        });
    }
}
