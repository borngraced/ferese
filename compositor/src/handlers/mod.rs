mod activation;
mod compositor;
mod input_method;
mod layer_shell;
pub(crate) mod screencopy;
mod xdg_shell;

use smithay::{
    input::{
        Seat, SeatHandler, SeatState,
        pointer::{CursorImageStatus, PointerHandle},
    },
    reexports::wayland_server::{Resource, protocol::wl_surface::WlSurface},
    utils::Point,
    wayland::{
        compositor::{TraversalAction, with_states, with_surface_tree_downward},
        fractional_scale::{FractionalScaleHandler, with_fractional_scale},
        idle_inhibit::IdleInhibitHandler,
        idle_notify::{IdleNotifierHandler, IdleNotifierState},
        keyboard_shortcuts_inhibit::{
            KeyboardShortcutsInhibitHandler, KeyboardShortcutsInhibitState,
            KeyboardShortcutsInhibitor, KeyboardShortcutsInhibitorSeat,
        },
        output::OutputHandler,
        pointer_constraints::{
            PointerConstraint, PointerConstraintsHandler, with_pointer_constraint,
        },
        selection::{
            SelectionHandler,
            data_device::{
                ClientDndGrabHandler, DataDeviceHandler, DataDeviceState, ServerDndGrabHandler,
                set_data_device_focus,
            },
            primary_selection::{
                PrimarySelectionHandler, PrimarySelectionState, set_primary_focus,
            },
        },
        tablet_manager::TabletSeatHandler,
        xdg_foreign::{XdgForeignHandler, XdgForeignState},
        xdg_toplevel_icon::XdgToplevelIconHandler,
    },
};

use crate::Ferese;

impl SeatHandler for Ferese {
    type KeyboardFocus = WlSurface;
    type PointerFocus = WlSurface;
    type TouchFocus = WlSurface;

    fn seat_state(&mut self) -> &mut SeatState<Self> {
        &mut self.seat_state
    }

    fn cursor_image(&mut self, _seat: &Seat<Self>, image: CursorImageStatus) {
        if let CursorImageStatus::Named(icon) = &image {
            self.named_cursors
                .entry(*icon)
                .or_insert_with(|| crate::cursor::load_named_cursor(&self.cursor_theme, *icon));
        }
        self.cursor_status = image;
        crate::backends::direct::render_all(self);
    }

    fn focus_changed(&mut self, seat: &Seat<Self>, focused: Option<&WlSurface>) {
        if let Some(inhibitor) = self.active_shortcuts_inhibitor.take()
            && inhibitor.is_active()
        {
            inhibitor.inactivate();
        }
        if let Some(inhibitor) =
            focused.and_then(|surface| seat.keyboard_shortcuts_inhibitor_for_surface(surface))
        {
            inhibitor.activate();
            self.active_shortcuts_inhibitor = Some(inhibitor);
        }

        let client = focused.and_then(|surface| self.display_handle.get_client(surface.id()).ok());
        set_data_device_focus(&self.display_handle, seat, client.clone());
        set_primary_focus(&self.display_handle, seat, client);
    }
}

impl OutputHandler for Ferese {}

impl FractionalScaleHandler for Ferese {
    fn new_fractional_scale(&mut self, surface: WlSurface) {
        let scale = self
            .space
            .outputs()
            .next()
            .map(|output| output.current_scale().fractional_scale())
            .unwrap_or(1.0);

        with_states(&surface, |states| {
            with_fractional_scale(states, |surface_scale| {
                surface_scale.set_preferred_scale(scale);
            });
        });
    }
}

impl IdleNotifierHandler for Ferese {
    fn idle_notifier_state(&mut self) -> &mut IdleNotifierState<Self> {
        &mut self.idle_notifier_state
    }
}

impl IdleInhibitHandler for Ferese {
    fn inhibit(&mut self, surface: WlSurface) {
        let count = self.idle_inhibitors.entry(surface).or_default();
        *count = count.saturating_add(1);
        self.idle_notifier_state.set_is_inhibited(true);
    }

    fn uninhibit(&mut self, surface: WlSurface) {
        if let Some(count) = self.idle_inhibitors.get_mut(&surface) {
            *count = count.saturating_sub(1);
            if *count == 0 {
                self.idle_inhibitors.remove(&surface);
            }
        }
        self.idle_notifier_state
            .set_is_inhibited(!self.idle_inhibitors.is_empty());
    }
}

impl KeyboardShortcutsInhibitHandler for Ferese {
    fn keyboard_shortcuts_inhibit_state(&mut self) -> &mut KeyboardShortcutsInhibitState {
        &mut self.keyboard_shortcuts_inhibit_state
    }

    fn new_inhibitor(&mut self, inhibitor: KeyboardShortcutsInhibitor) {
        let focused = self
            .seat
            .get_keyboard()
            .and_then(|keyboard| keyboard.current_focus());

        if focused.as_ref() == Some(inhibitor.wl_surface()) {
            inhibitor.activate();
            self.active_shortcuts_inhibitor = Some(inhibitor);
        }
    }

    fn inhibitor_destroyed(&mut self, inhibitor: KeyboardShortcutsInhibitor) {
        if self.active_shortcuts_inhibitor.as_ref() == Some(&inhibitor) {
            self.active_shortcuts_inhibitor = None;
        }
    }
}

impl Ferese {
    pub fn update_fractional_scale(&self, scale: f64) {
        for window in self.space.elements() {
            let Some(toplevel) = window.toplevel() else {
                continue;
            };

            with_surface_tree_downward(
                toplevel.wl_surface(),
                (),
                |_, _, &()| TraversalAction::DoChildren(()),
                |_, states, &()| {
                    with_fractional_scale(states, |surface_scale| {
                        surface_scale.set_preferred_scale(scale);
                    });
                },
                |_, _, &()| true,
            );
        }
    }
}

impl PointerConstraintsHandler for Ferese {
    fn new_constraint(&mut self, _surface: &WlSurface, pointer: &PointerHandle<Self>) {
        self.activate_focused_pointer_constraint(pointer);
    }

    fn cursor_position_hint(
        &mut self,
        surface: &WlSurface,
        pointer: &PointerHandle<Self>,
        location: smithay::utils::Point<f64, smithay::utils::Logical>,
    ) {
        let locked = with_pointer_constraint(surface, pointer, |constraint| {
            constraint.is_some_and(|constraint| {
                constraint.is_active() && matches!(&*constraint, PointerConstraint::Locked(_))
            })
        });
        if !locked || pointer.current_focus().as_ref() != Some(surface) {
            return;
        }

        let Some((focused_surface, origin)) = self.surface_under(pointer.current_location()) else {
            return;
        };
        if focused_surface == *surface {
            let current = pointer.current_location();
            let (scale_x, scale_y) = self
                .window_under_visual(current)
                .and_then(|window| self.visual_scale_for_window(&window))
                .unwrap_or((1.0, 1.0));
            let current_surface_location = current - origin;
            let offset = location - current_surface_location;

            pointer.set_location(current + Point::from((offset.x * scale_x, offset.y * scale_y)));
        }
    }
}

impl SelectionHandler for Ferese {
    type SelectionUserData = ();
}

impl ClientDndGrabHandler for Ferese {}
impl ServerDndGrabHandler for Ferese {}
impl TabletSeatHandler for Ferese {}

impl DataDeviceHandler for Ferese {
    fn data_device_state(&self) -> &DataDeviceState {
        &self.data_device_state
    }
}

impl PrimarySelectionHandler for Ferese {
    fn primary_selection_state(&self) -> &PrimarySelectionState {
        &self.primary_selection_state
    }
}

impl XdgForeignHandler for Ferese {
    fn xdg_foreign_state(&mut self) -> &mut XdgForeignState {
        &mut self.xdg_foreign_state
    }
}

impl XdgToplevelIconHandler for Ferese {}

smithay::delegate_compositor!(Ferese);
smithay::delegate_cursor_shape!(Ferese);
smithay::delegate_data_device!(Ferese);
smithay::delegate_dmabuf!(Ferese);
smithay::delegate_fractional_scale!(Ferese);
smithay::delegate_idle_inhibit!(Ferese);
smithay::delegate_idle_notify!(Ferese);
smithay::delegate_input_method_manager!(Ferese);
smithay::delegate_keyboard_shortcuts_inhibit!(Ferese);
smithay::delegate_layer_shell!(Ferese);
smithay::delegate_output!(Ferese);
smithay::delegate_pointer_constraints!(Ferese);
smithay::delegate_presentation!(Ferese);
smithay::delegate_primary_selection!(Ferese);
smithay::delegate_relative_pointer!(Ferese);
smithay::delegate_seat!(Ferese);
smithay::delegate_shm!(Ferese);
smithay::delegate_single_pixel_buffer!(Ferese);
smithay::delegate_text_input_manager!(Ferese);
smithay::delegate_viewporter!(Ferese);
smithay::delegate_xdg_activation!(Ferese);
smithay::delegate_xdg_decoration!(Ferese);
smithay::delegate_xdg_foreign!(Ferese);
smithay::delegate_xdg_shell!(Ferese);
smithay::delegate_xdg_toplevel_icon!(Ferese);
