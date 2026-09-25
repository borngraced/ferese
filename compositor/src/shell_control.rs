use ferese_layout::WindowId;
use ferese_protocols::shell::v1::server::{
    ferese_shell_manager_v1::{self, FereseShellManagerV1},
    ferese_shell_v1::{self, FereseShellV1},
};
use smithay::{
    reexports::wayland_server::{
        Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New, Resource,
    },
    wayland::{compositor::with_states, shell::xdg::XdgToplevelSurfaceData},
};

use crate::{Ferese, private_client::ClientCapabilities, state::ClientState};

pub(crate) fn init_global(display: &DisplayHandle) {
    display.create_global::<Ferese, FereseShellManagerV1, _>(1, ());
}

impl GlobalDispatch<FereseShellManagerV1, ()> for Ferese {
    fn bind(
        _state: &mut Self,
        _display: &DisplayHandle,
        _client: &Client,
        resource: New<FereseShellManagerV1>,
        _global_data: &(),
        data_init: &mut DataInit<'_, Self>,
    ) {
        data_init.init(resource, ());
    }

    fn can_view(client: Client, _global_data: &()) -> bool {
        client.get_data::<ClientState>().is_some_and(|state| {
            state
                .capabilities
                .contains(ClientCapabilities::SHELL_CONTROL)
        })
    }
}

impl Dispatch<FereseShellManagerV1, ()> for Ferese {
    fn request(
        state: &mut Self,
        _client: &Client,
        _manager: &FereseShellManagerV1,
        request: ferese_shell_manager_v1::Request,
        _data: &(),
        _display: &DisplayHandle,
        data_init: &mut DataInit<'_, Self>,
    ) {
        match request {
            ferese_shell_manager_v1::Request::GetShell { id } => {
                let shell = data_init.init(id, ());

                shell.capabilities(1);
                shell.overview_state(u32::from(state.overview_active));
                state.send_shell_snapshot(&shell);
                state.shell_resources.push(shell.downgrade());
            }
            ferese_shell_manager_v1::Request::Destroy => {}
            _ => unreachable!(),
        }
    }
}

impl Dispatch<FereseShellV1, ()> for Ferese {
    fn request(
        state: &mut Self,
        _client: &Client,
        shell: &FereseShellV1,
        request: ferese_shell_v1::Request,
        _data: &(),
        _display: &DisplayHandle,
        _data_init: &mut DataInit<'_, Self>,
    ) {
        match request {
            ferese_shell_v1::Request::ActivateWindow {
                window_hi,
                window_lo,
            } => state.handle_shell_window_request(
                shell,
                ferese_shell_v1::FailedRequest::ActivateWindow,
                join_id(window_hi, window_lo),
                ShellWindowAction::Activate,
            ),
            ferese_shell_v1::Request::CloseWindow {
                window_hi,
                window_lo,
            } => state.handle_shell_window_request(
                shell,
                ferese_shell_v1::FailedRequest::CloseWindow,
                join_id(window_hi, window_lo),
                ShellWindowAction::Close,
            ),
            ferese_shell_v1::Request::EnterOverview => state.set_overview_active(true),
            ferese_shell_v1::Request::ExitOverview => state.set_overview_active(false),
            ferese_shell_v1::Request::SelectOverviewWindow {
                window_hi,
                window_lo,
            } => {
                let id = join_id(window_hi, window_lo);

                if state.activate_managed_window(id) {
                    state.set_overview_active(false);
                } else {
                    send_request_failed(
                        shell,
                        ferese_shell_v1::FailedRequest::SelectOverviewWindow,
                        id,
                    );
                }
            }
            ferese_shell_v1::Request::Destroy => {}
            _ => unreachable!(),
        }
    }
}

impl Ferese {
    pub(crate) fn send_shell_snapshots(&mut self) {
        let resources = std::mem::take(&mut self.shell_resources);

        for weak in resources {
            if let Ok(shell) = weak.upgrade() {
                self.send_shell_snapshot(&shell);
                self.shell_resources.push(shell.downgrade());
            }
        }
    }

    fn send_shell_snapshot(&mut self, shell: &FereseShellV1) {
        self.shell_snapshot_serial = self.shell_snapshot_serial.wrapping_add(1);
        let serial = self.shell_snapshot_serial;
        let snapshots = self.managed_window_snapshots();

        shell.snapshot_begin(serial);
        for snapshot in snapshots {
            let (window_hi, window_lo) = split_id(snapshot.id.0);
            let (workspace_hi, workspace_lo) = split_id(snapshot.workspace);

            shell.window(
                window_hi,
                window_lo,
                workspace_hi,
                workspace_lo,
                snapshot.app_id,
                snapshot.title,
                snapshot.state,
            );
        }
        shell.snapshot_end(serial);
    }

    fn managed_window_snapshots(&self) -> Vec<ManagedWindowSnapshot> {
        let mut snapshots = self
            .window_ids
            .iter()
            .filter_map(|(window, id)| {
                let workspace = self.workspaces.workspace_for_window(*id)?.0;
                let toplevel = window.toplevel()?;
                let (app_id, title) = with_states(toplevel.wl_surface(), |states| {
                    let attributes = states
                        .data_map
                        .get::<XdgToplevelSurfaceData>()
                        .expect("xdg toplevel state exists")
                        .lock()
                        .expect("xdg toplevel state is not poisoned");

                    (
                        attributes.app_id.clone().unwrap_or_default(),
                        attributes.title.clone().unwrap_or_default(),
                    )
                });
                let mut state = ferese_shell_v1::WindowState::empty();

                if self.focused_window == Some(*id) {
                    state |= ferese_shell_v1::WindowState::Focused;
                }
                if self
                    .workspaces
                    .workspace_for_window(*id)
                    .and_then(|workspace| self.workspaces.workspace(workspace))
                    .is_some_and(|workspace| workspace.fullscreen == Some(*id))
                {
                    state |= ferese_shell_v1::WindowState::Fullscreen;
                }
                if self.is_floating_window(window) {
                    state |= ferese_shell_v1::WindowState::Floating;
                }

                Some(ManagedWindowSnapshot {
                    id: *id,
                    workspace,
                    app_id,
                    title,
                    state,
                })
            })
            .collect::<Vec<_>>();

        snapshots.sort_by_key(|snapshot| snapshot.id.0);
        snapshots
    }

    fn handle_shell_window_request(
        &mut self,
        shell: &FereseShellV1,
        request: ferese_shell_v1::FailedRequest,
        id: WindowId,
        action: ShellWindowAction,
    ) {
        let succeeded = match action {
            ShellWindowAction::Activate => self.activate_managed_window(id),
            ShellWindowAction::Close => self.close_managed_window(id),
        };

        if !succeeded {
            send_request_failed(shell, request, id);
        }
    }

    fn set_overview_active(&mut self, active: bool) {
        if self.overview_active == active {
            return;
        }

        self.overview_active = active;
        self.shell_resources.retain(|resource| {
            resource.upgrade().is_ok_and(|shell| {
                shell.overview_state(u32::from(active));
                true
            })
        });
        crate::backends::direct::render_all(self);
    }
}

#[derive(Clone, Copy)]
enum ShellWindowAction {
    Activate,
    Close,
}

struct ManagedWindowSnapshot {
    id: WindowId,
    workspace: u64,
    app_id: String,
    title: String,
    state: ferese_shell_v1::WindowState,
}

fn split_id(id: u64) -> (u32, u32) {
    ((id >> 32) as u32, id as u32)
}

fn join_id(hi: u32, lo: u32) -> WindowId {
    WindowId((u64::from(hi) << 32) | u64::from(lo))
}

fn send_request_failed(
    shell: &FereseShellV1,
    request: ferese_shell_v1::FailedRequest,
    id: WindowId,
) {
    let (window_hi, window_lo) = split_id(id.0);

    shell.request_failed(
        request,
        window_hi,
        window_lo,
        "unknown or unavailable managed window".to_owned(),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window_ids_round_trip_through_protocol_words() {
        for id in [0, 1, u32::MAX as u64, u64::from(u32::MAX) + 1, u64::MAX] {
            let (hi, lo) = split_id(id);

            assert_eq!(join_id(hi, lo), WindowId(id));
        }
    }
}
