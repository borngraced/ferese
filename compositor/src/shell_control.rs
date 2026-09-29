use ferese_core::{OutputId, WorkspaceId};
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
    display.create_global::<Ferese, FereseShellManagerV1, _>(3, ());
}

pub(crate) fn config_chunks(source: &str) -> Vec<&str> {
    let mut remaining = source;
    let mut chunks = Vec::new();
    while !remaining.is_empty() {
        let mut boundary = remaining.len().min(1024);
        while !remaining.is_char_boundary(boundary) {
            boundary -= 1;
        }
        chunks.push(&remaining[..boundary]);
        remaining = &remaining[boundary..];
    }
    chunks
}

pub(crate) fn send_shell_config(shell: &FereseShellV1, source: &str) {
    shell.config_begin();
    for chunk in config_chunks(source) {
        shell.config_chunk(chunk.to_owned());
    }
    shell.config_end();
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
                if shell.version() >= 2
                    && let Some(source) = &state.config_source
                {
                    send_shell_config(&shell, source);
                }
                shell.overview_state(u32::from(state.overview.is_active()));
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
        if state.session_lock.active {
            return;
        }
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
            ferese_shell_v1::Request::ActivateWorkspace {
                workspace_hi,
                workspace_lo,
            } => {
                let workspace = WorkspaceId(join_u64(workspace_hi, workspace_lo));

                if !state.activate_managed_workspace(workspace) {
                    send_request_failed(
                        shell,
                        ferese_shell_v1::FailedRequest::ActivateWorkspace,
                        workspace.0,
                    );
                }
            }
            ferese_shell_v1::Request::ConfirmLogout { serial } => {
                if state.logout_owner.as_ref() == Some(&shell.id())
                    && consume_logout_confirmation(&mut state.pending_logout, serial)
                {
                    state.logout_owner = None;
                    state.loop_signal.stop();
                }
            }
            ferese_shell_v1::Request::CancelLogout { serial } => {
                if state.logout_owner.as_ref() == Some(&shell.id())
                    && state.pending_logout == Some(serial)
                {
                    state.cancel_logout_confirmation();
                }
            }
            ferese_shell_v1::Request::EnterOverview => state.set_overview_active(true),
            ferese_shell_v1::Request::ExitOverview => state.set_overview_active(false),
            ferese_shell_v1::Request::SelectOverviewWindow {
                window_hi,
                window_lo,
            } => {
                let id = join_id(window_hi, window_lo);

                if !state.select_overview_window(id) {
                    send_request_failed(
                        shell,
                        ferese_shell_v1::FailedRequest::SelectOverviewWindow,
                        id.0,
                    );
                }
            }
            ferese_shell_v1::Request::Destroy => {
                if state.logout_owner.as_ref() == Some(&shell.id()) {
                    state.cancel_logout_confirmation();
                }
            }
            _ => unreachable!(),
        }
    }

    fn destroyed(
        state: &mut Self,
        _client: smithay::reexports::wayland_server::backend::ClientId,
        shell: &FereseShellV1,
        _data: &(),
    ) {
        if state.logout_owner.as_ref() == Some(&shell.id()) {
            state.pending_logout = None;
            state.logout_owner = None;
        }
    }
}

fn consume_logout_confirmation(pending: &mut Option<u32>, serial: u32) -> bool {
    if *pending == Some(serial) {
        *pending = None;
        true
    } else {
        false
    }
}

impl Ferese {
    pub(crate) fn request_logout_confirmation(&mut self) {
        if self.session_lock.active {
            return;
        }
        let shells = self
            .shell_resources
            .iter()
            .filter_map(|shell| shell.upgrade().ok())
            .filter(|shell| shell.version() >= 3)
            .collect::<Vec<_>>();
        if shells.is_empty() {
            tracing::warn!("logout confirmation requires the updated Ferese shell");
            return;
        }
        self.cancel_logout_confirmation();
        let serial = u32::from(smithay::utils::SERIAL_COUNTER.next_serial());
        let shell = &shells[0];
        self.pending_logout = Some(serial);
        self.logout_owner = Some(shell.id());
        let output_name = self
            .focused_output()
            .map_or_else(String::new, |output| output.name());
        shell.logout_requested(serial, output_name);
    }

    pub(crate) fn cancel_logout_confirmation(&mut self) {
        if let Some(serial) = self.pending_logout.take() {
            for shell in self
                .shell_resources
                .iter()
                .filter_map(|shell| shell.upgrade().ok())
            {
                if self.logout_owner.as_ref() == Some(&shell.id()) && shell.version() >= 3 {
                    shell.logout_cancelled(serial);
                }
            }
        }
        self.logout_owner = None;
    }

    pub(crate) fn send_shell_snapshots(&mut self) {
        self.shell_resources
            .retain(|resource| resource.upgrade().is_ok());
        if self.shell_resources.is_empty() {
            self.last_shell_snapshot = None;
            return;
        }
        let snapshot = self.shell_snapshot();
        if self.last_shell_snapshot.as_ref() == Some(&snapshot) {
            return;
        }
        for resource in &self.shell_resources {
            if let Ok(shell) = resource.upgrade() {
                self.shell_snapshot_serial = self.shell_snapshot_serial.wrapping_add(1);
                snapshot.send(&shell, self.shell_snapshot_serial);
            }
        }
        self.last_shell_snapshot = Some(snapshot);
    }

    fn shell_snapshot(&self) -> ShellSnapshot {
        ShellSnapshot {
            outputs: self.output_snapshots(),
            workspaces: self.workspace_snapshots(),
            windows: self.managed_window_snapshots(),
        }
    }

    fn send_shell_snapshot(&mut self, shell: &FereseShellV1) {
        // New subscribers always receive a complete snapshot. Do not update the
        // broadcast cache here: existing subscribers may still need this state.
        self.shell_snapshot_serial = self.shell_snapshot_serial.wrapping_add(1);
        self.shell_snapshot()
            .send(shell, self.shell_snapshot_serial);
    }

    fn output_snapshots(&self) -> Vec<OutputSnapshot> {
        let focused = self.output_workspaces.focused_output();
        let mut outputs = self
            .output_ids
            .iter()
            .filter_map(|(output, id)| {
                Some(OutputSnapshot {
                    id: *id,
                    name: output.name(),
                    active_workspace: self.output_workspaces.active_workspace(*id)?,
                    focused: focused == Some(*id),
                })
            })
            .collect::<Vec<_>>();

        outputs.sort_by_key(|output| output.id.0);
        outputs
    }

    fn workspace_snapshots(&self) -> Vec<WorkspaceSnapshot> {
        self.workspaces
            .ordered()
            .into_iter()
            .map(|workspace| {
                let output = self.output_workspaces.output_for_workspace(workspace.id);
                let active = output.is_some_and(|output| {
                    self.output_workspaces.active_workspace(output) == Some(workspace.id)
                });

                WorkspaceSnapshot {
                    id: workspace.id,
                    output,
                    name: workspace.name.clone(),
                    active,
                }
            })
            .collect()
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
            send_request_failed(shell, request, id.0);
        }
    }

    pub(crate) fn notify_overview_state(&mut self) {
        let active = self.overview.is_active();

        self.shell_resources.retain(|resource| {
            resource.upgrade().is_ok_and(|shell| {
                shell.overview_state(u32::from(active));
                true
            })
        });
    }
}

#[derive(Clone, Copy)]
enum ShellWindowAction {
    Activate,
    Close,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ShellSnapshot {
    outputs: Vec<OutputSnapshot>,
    workspaces: Vec<WorkspaceSnapshot>,
    windows: Vec<ManagedWindowSnapshot>,
}

impl ShellSnapshot {
    fn send(&self, shell: &FereseShellV1, serial: u32) {
        shell.snapshot_begin(serial);
        for output in &self.outputs {
            let (output_hi, output_lo) = split_id(output.id.0);
            let (workspace_hi, workspace_lo) = split_id(output.active_workspace.0);

            shell.output(
                output_hi,
                output_lo,
                output.name.clone(),
                workspace_hi,
                workspace_lo,
                u32::from(output.focused),
            );
        }
        for workspace in &self.workspaces {
            let (workspace_hi, workspace_lo) = split_id(workspace.id.0);
            let (output_hi, output_lo) = workspace
                .output
                .map(|output| split_id(output.0))
                .unwrap_or_default();

            shell.workspace(
                workspace_hi,
                workspace_lo,
                output_hi,
                output_lo,
                workspace.name.clone(),
                u32::from(workspace.active),
            );
        }
        for snapshot in &self.windows {
            let (window_hi, window_lo) = split_id(snapshot.id.0);
            let (workspace_hi, workspace_lo) = split_id(snapshot.workspace);

            shell.window(
                window_hi,
                window_lo,
                workspace_hi,
                workspace_lo,
                snapshot.app_id.clone(),
                snapshot.title.clone(),
                snapshot.state,
            );
        }
        shell.snapshot_end(serial);
    }
}

#[derive(Clone, Debug, PartialEq)]
struct ManagedWindowSnapshot {
    id: WindowId,
    workspace: u64,
    app_id: String,
    title: String,
    state: ferese_shell_v1::WindowState,
}

#[derive(Clone, Debug, PartialEq)]
struct OutputSnapshot {
    id: OutputId,
    name: String,
    active_workspace: WorkspaceId,
    focused: bool,
}

#[derive(Clone, Debug, PartialEq)]
struct WorkspaceSnapshot {
    id: WorkspaceId,
    output: Option<OutputId>,
    name: String,
    active: bool,
}

fn split_id(id: u64) -> (u32, u32) {
    ((id >> 32) as u32, id as u32)
}

fn join_id(hi: u32, lo: u32) -> WindowId {
    WindowId(join_u64(hi, lo))
}

fn join_u64(hi: u32, lo: u32) -> u64 {
    (u64::from(hi) << 32) | u64::from(lo)
}

fn send_request_failed(shell: &FereseShellV1, request: ferese_shell_v1::FailedRequest, id: u64) {
    let (object_hi, object_lo) = split_id(id);

    shell.request_failed(
        request,
        object_hi,
        object_lo,
        "unknown or unavailable shell object".to_owned(),
    );
}

#[cfg(test)]
mod tests {
    #[test]
    fn logout_requires_the_current_confirmation_and_can_be_cancelled() {
        let mut pending = Some(42);
        assert!(!super::consume_logout_confirmation(&mut pending, 41));
        assert_eq!(pending, Some(42));
        assert!(super::consume_logout_confirmation(&mut pending, 42));
        assert_eq!(pending, None);
        assert!(!super::consume_logout_confirmation(&mut pending, 42));
    }

    use super::*;

    #[test]
    fn snapshot_deduplication_preserves_shell_visible_changes() {
        let original = ShellSnapshot {
            outputs: vec![OutputSnapshot {
                id: OutputId(1),
                name: "display".into(),
                active_workspace: WorkspaceId(1),
                focused: true,
            }],
            workspaces: vec![WorkspaceSnapshot {
                id: WorkspaceId(1),
                output: Some(OutputId(1)),
                name: "1".into(),
                active: true,
            }],
            windows: vec![ManagedWindowSnapshot {
                id: WindowId(1),
                workspace: 1,
                app_id: "terminal".into(),
                title: "shell".into(),
                state: ferese_shell_v1::WindowState::Focused,
            }],
        };
        assert_eq!(original, original.clone());
        let changes: &[fn(&mut ShellSnapshot)] = &[
            |s| s.outputs[0].focused = false,
            |s| s.outputs[0].active_workspace = WorkspaceId(2),
            |s| s.outputs.clear(),
            |s| s.workspaces[0].output = None,
            |s| s.workspaces[0].active = false,
            |s| s.workspaces[0].name = "renamed".into(),
            |s| s.windows[0].workspace = 2,
            |s| s.windows[0].title = "new title".into(),
            |s| s.windows[0].app_id = "new app".into(),
            |s| s.windows[0].state |= ferese_shell_v1::WindowState::Fullscreen,
            |s| s.windows.clear(),
        ];
        for change in changes {
            let mut updated = original.clone();
            change(&mut updated);
            assert_ne!(original, updated);
        }
    }

    #[test]
    fn config_chunks_are_bounded_and_preserve_unicode() {
        let source = "abc🌲é".repeat(9000);
        let chunks = config_chunks(&source);
        assert!(
            chunks
                .iter()
                .all(|chunk| !chunk.is_empty() && chunk.len() <= 1024)
        );
        assert_eq!(chunks.concat(), source);
        assert!(config_chunks("").is_empty());
    }

    #[test]
    fn window_ids_round_trip_through_protocol_words() {
        for id in [0, 1, u32::MAX as u64, u64::from(u32::MAX) + 1, u64::MAX] {
            let (hi, lo) = split_id(id);

            assert_eq!(join_id(hi, lo), WindowId(id));
        }
    }
}
