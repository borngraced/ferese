use std::time::Duration;

use smithay::{
    reexports::wayland_server::protocol::wl_surface::WlSurface,
    utils::SERIAL_COUNTER,
    wayland::xdg_activation::{
        XdgActivationHandler, XdgActivationState, XdgActivationToken, XdgActivationTokenData,
    },
};

use crate::Ferese;

const TOKEN_MAX_AGE: Duration = Duration::from_secs(10);

impl XdgActivationHandler for Ferese {
    fn activation_state(&mut self) -> &mut XdgActivationState {
        &mut self.xdg_activation_state
    }

    fn token_created(&mut self, _token: XdgActivationToken, data: XdgActivationTokenData) -> bool {
        data.serial.is_some() && data.timestamp.elapsed() <= TOKEN_MAX_AGE
    }

    fn request_activation(
        &mut self,
        token: XdgActivationToken,
        data: XdgActivationTokenData,
        surface: WlSurface,
    ) {
        let valid = data.serial.is_some() && data.timestamp.elapsed() <= TOKEN_MAX_AGE;
        self.xdg_activation_state.remove_token(&token);
        if !valid {
            tracing::debug!("ignored stale or non-interactive activation request");
            return;
        }

        let Some(window) = self
            .space
            .elements()
            .find(|window| {
                window
                    .toplevel()
                    .is_some_and(|toplevel| toplevel.wl_surface() == &surface)
            })
            .cloned()
        else {
            return;
        };

        self.focused_window = self.window_ids.get(&window).copied();
        if let Some(focused) = self.focused_window
            && let Err(error) = self.workspaces.focus_window(focused)
        {
            tracing::error!(%error, ?focused, "failed to update workspace focus");
            return;
        }
        self.raise_window(&window, true);
        let keyboard = self.seat.get_keyboard().expect("seat has a keyboard");
        keyboard.set_focus(self, Some(surface), SERIAL_COUNTER.next_serial());
        self.space.elements().for_each(|window| {
            if let Some(toplevel) = window.toplevel() {
                toplevel.send_pending_configure();
            }
        });
    }
}
