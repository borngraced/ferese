mod compositor;
mod xdg_shell;

use smithay::{
    input::{Seat, SeatHandler, SeatState, pointer::CursorImageStatus},
    reexports::wayland_server::protocol::wl_surface::WlSurface,
    wayland::output::OutputHandler,
};

use crate::Ferese;

impl SeatHandler for Ferese {
    type KeyboardFocus = WlSurface;
    type PointerFocus = WlSurface;
    type TouchFocus = WlSurface;

    fn seat_state(&mut self) -> &mut SeatState<Self> {
        &mut self.seat_state
    }

    fn cursor_image(&mut self, _seat: &Seat<Self>, _image: CursorImageStatus) {}
}

impl OutputHandler for Ferese {}

smithay::delegate_compositor!(Ferese);
smithay::delegate_output!(Ferese);
smithay::delegate_seat!(Ferese);
smithay::delegate_shm!(Ferese);
smithay::delegate_xdg_shell!(Ferese);
