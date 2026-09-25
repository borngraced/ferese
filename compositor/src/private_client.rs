use std::{
    io,
    os::{
        fd::AsRawFd,
        unix::{net::UnixStream, process::CommandExt},
    },
    process::Command,
    sync::Arc,
};

use crate::{Ferese, state::ClientState};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct ClientCapabilities(u8);

impl ClientCapabilities {
    pub(crate) const EFFECTS: Self = Self(1 << 0);

    pub(crate) fn contains(self, capability: Self) -> bool {
        self.0 & capability.0 == capability.0
    }

    pub(crate) fn insert(&mut self, capability: Self) {
        self.0 |= capability.0;
    }

    pub(crate) fn is_empty(self) -> bool {
        self.0 == 0
    }
}

pub(crate) fn prepare_command(
    state: &mut Ferese,
    command: &mut Command,
    capabilities: ClientCapabilities,
) -> io::Result<UnixStream> {
    let (server, client) = UnixStream::pair()?;
    state.display_handle.insert_client(
        server,
        Arc::new(ClientState {
            capabilities,
            ..ClientState::default()
        }),
    )?;

    let client_fd = client.as_raw_fd();
    command.env_remove("WAYLAND_DISPLAY");
    command.env("WAYLAND_SOCKET", client_fd.to_string());
    // SAFETY: fcntl(F_SETFD) is async-signal-safe. The closure only clears
    // CLOEXEC on the socket already owned by this Command's parent process.
    unsafe {
        command.pre_exec(move || {
            if libc::fcntl(client_fd, libc::F_SETFD, 0) == -1 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        });
    }

    Ok(client)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capabilities_are_explicit_and_independent() {
        let mut capabilities = ClientCapabilities::default();
        assert!(capabilities.is_empty());
        assert!(!capabilities.contains(ClientCapabilities::EFFECTS));

        capabilities.insert(ClientCapabilities::EFFECTS);
        assert!(!capabilities.is_empty());
        assert!(capabilities.contains(ClientCapabilities::EFFECTS));
    }
}
