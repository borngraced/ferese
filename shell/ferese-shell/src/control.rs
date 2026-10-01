use std::error::Error;
use std::os::fd::{FromRawFd, RawFd};
use std::os::unix::net::UnixStream;
use std::sync::mpsc;
use std::sync::mpsc::{Receiver, Sender, TryRecvError};
use std::thread;

use ferese_protocols::shell::v1::client::ferese_shell_manager_v1::FereseShellManagerV1;
use ferese_protocols::shell::v1::client::ferese_shell_v1;
use ferese_protocols::shell::v1::client::ferese_shell_v1::FereseShellV1;
use wayland_client::globals::{GlobalListContents, registry_queue_init};
use wayland_client::protocol::wl_registry;
use wayland_client::{Connection, Dispatch, Proxy, QueueHandle, WEnum, delegate_noop};

#[derive(Clone, Debug, Default)]
pub(crate) struct ShellSnapshot {
    pub(crate) outputs: Vec<OutputSnapshot>,
    pub(crate) workspaces: Vec<WorkspaceSnapshot>,
    pub(crate) windows: Vec<WindowSnapshot>,
}

impl ShellSnapshot {
    pub(crate) fn workspaces_for_output(&self, output: Option<u64>) -> impl Iterator<Item = &WorkspaceSnapshot> {
        self.workspaces
            .iter()
            .filter(move |workspace| output.is_some() && workspace.output == output)
    }
}

#[derive(Clone, Debug)]
pub(crate) struct OutputSnapshot {
    pub(crate) id: u64,
    pub(crate) name: String,
    pub(crate) active_workspace: u64,
    pub(crate) focused: bool,
}

#[derive(Clone, Debug)]
pub(crate) struct WorkspaceSnapshot {
    pub(crate) id: u64,
    pub(crate) name: String,
    pub(crate) output: Option<u64>,
    pub(crate) index: u32,
    pub(crate) window_count: u32,
    pub(crate) visible: bool,
    pub(crate) focused: bool,
}

#[derive(Clone, Debug)]
pub(crate) struct WindowSnapshot {
    pub(crate) workspace: u64,
    pub(crate) app_id: String,
    pub(crate) title: String,
    pub(crate) focused: bool,
    pub(crate) fullscreen: bool,
}

pub(crate) struct ShellControl {
    connection: Connection,
    _manager: FereseShellManagerV1,
    shell: FereseShellV1,
    updates: Receiver<ControlUpdate>,
}

pub(crate) struct ControlPoll {
    pub(crate) config: Option<String>,
    pub(crate) snapshot: Option<ShellSnapshot>,
    pub(crate) overview_active: Option<bool>,
    pub(crate) disconnected: bool,
    pub(crate) logout: Option<(u32, String)>,
    pub(crate) logout_cancelled: Vec<u32>,
    pub(crate) guide_toggles: Vec<String>,
}

enum ControlUpdate {
    Config(String),
    Snapshot(ShellSnapshot),
    OverviewState(bool),
    Disconnected,
    Logout(u32, String),
    LogoutCancelled(u32),
    ToggleGuide(String),
}

impl ShellControl {
    pub(crate) fn connect() -> Result<Self, Box<dyn Error>> {
        let connection = control_connection()?;
        let (globals, mut queue) = registry_queue_init::<ControlState>(&connection)?;
        let qh = queue.handle();
        let manager = globals.bind::<FereseShellManagerV1, _, _>(&qh, 4..=5, ())?;
        let shell = manager.get_shell(&qh, ());
        let (sender, updates) = mpsc::channel();
        let mut state = ControlState::new(sender);

        connection.flush()?;
        thread::Builder::new()
            .name("ferese-shell-control".to_owned())
            .spawn(move || {
                while queue.blocking_dispatch(&mut state).is_ok() {}
                let _ = state.sender.send(ControlUpdate::Disconnected);
            })?;

        Ok(Self {
            connection,
            _manager: manager,
            shell,
            updates,
        })
    }

    pub(crate) fn poll(&self) -> ControlPoll {
        let mut poll = ControlPoll {
            config: None,
            snapshot: None,
            overview_active: None,
            disconnected: false,
            logout: None,
            logout_cancelled: Vec::new(),
            guide_toggles: Vec::new(),
        };

        loop {
            match self.updates.try_recv() {
                Ok(ControlUpdate::Logout(serial, output)) => poll.logout = Some((serial, output)),
                Ok(ControlUpdate::ToggleGuide(output)) => poll.guide_toggles.push(output),
                Ok(ControlUpdate::LogoutCancelled(serial)) => {
                    if poll.logout.as_ref().is_some_and(|(pending, _)| *pending == serial) {
                        poll.logout = None;
                    }
                    poll.logout_cancelled.push(serial);
                }
                Ok(ControlUpdate::Config(source)) => poll.config = Some(source),
                Ok(ControlUpdate::Snapshot(snapshot)) => poll.snapshot = Some(snapshot),
                Ok(ControlUpdate::OverviewState(active)) => poll.overview_active = Some(active),
                Ok(ControlUpdate::Disconnected) | Err(TryRecvError::Disconnected) => {
                    poll.disconnected = true;
                    return poll;
                }
                Err(TryRecvError::Empty) => return poll,
            }
        }
    }

    pub(crate) fn activate_workspace(&self, id: u64) {
        let (hi, lo) = split_id(id);

        self.shell.activate_workspace(hi, lo);
        let _ = self.connection.flush();
    }

    pub(crate) fn confirm_logout(&self, serial: u32, revision: u32, token: u32, force: bool) {
        self.shell
            .confirm_logout_with_inhibitors(serial, revision, token, u32::from(force));
        let _ = self.connection.flush();
    }

    pub(crate) fn cancel_logout(&self, serial: u32) {
        if self.shell.version() >= 3 {
            self.shell.cancel_logout(serial);
            let _ = self.connection.flush();
        }
    }

    pub(crate) fn set_overview_active(&self, active: bool) {
        if active {
            self.shell.enter_overview();
        } else {
            self.shell.exit_overview();
        }
        let _ = self.connection.flush();
    }
}

fn control_connection() -> Result<Connection, Box<dyn Error>> {
    let raw_fd =
        std::env::var_os("FERESE_SHELL_CONTROL_SOCKET").ok_or("Ferese did not provide a shell-control connection")?;
    let fd = raw_fd
        .to_str()
        .ok_or("FERESE_SHELL_CONTROL_SOCKET is not valid UTF-8")?
        .parse::<RawFd>()?;
    // SAFETY: Ferese passes ownership of this inherited descriptor to the
    // shell. This function is called once and consumes the descriptor.
    let socket = unsafe { UnixStream::from_raw_fd(fd) };
    // Rust's cloned stream is close-on-exec. Do not leak shell-control authority
    // into status helpers or applications launched from the menu.
    let connection_socket = socket.try_clone()?;
    drop(socket);
    Ok(Connection::from_socket(connection_socket)?)
}

struct ControlState {
    config: Option<String>,
    sender: Sender<ControlUpdate>,
    pending: ShellSnapshot,
    serial: Option<u32>,
}

impl ControlState {
    fn new(sender: Sender<ControlUpdate>) -> Self {
        Self {
            sender,
            config: None,
            pending: ShellSnapshot::default(),
            serial: None,
        }
    }
}

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for ControlState {
    fn event(
        _state: &mut Self,
        _registry: &wl_registry::WlRegistry,
        _event: wl_registry::Event,
        _data: &GlobalListContents,
        _connection: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<FereseShellV1, ()> for ControlState {
    fn event(
        state: &mut Self,
        _shell: &FereseShellV1,
        event: ferese_shell_v1::Event,
        _data: &(),
        _connection: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        match event {
            ferese_shell_v1::Event::ConfigBegin => state.config = Some(String::new()),
            ferese_shell_v1::Event::ConfigChunk { source } => {
                append_config_chunk(&mut state.config, &source);
            }
            ferese_shell_v1::Event::ConfigEnd => {
                if let Some(source) = state.config.take() {
                    let _ = state.sender.send(ControlUpdate::Config(source));
                }
            }
            ferese_shell_v1::Event::SnapshotBegin { serial } => {
                state.serial = Some(serial);
                state.pending = ShellSnapshot::default();
            }
            ferese_shell_v1::Event::Output {
                output_hi,
                output_lo,
                name,
                active_workspace_hi,
                active_workspace_lo,
                focused,
            } => state.pending.outputs.push(OutputSnapshot {
                id: join_id(output_hi, output_lo),
                name,
                active_workspace: join_id(active_workspace_hi, active_workspace_lo),
                focused: focused != 0,
            }),
            ferese_shell_v1::Event::Workspace {
                workspace_hi,
                workspace_lo,
                output_hi,
                output_lo,
                name,
                index,
                window_count,
                visible,
                focused,
            } => {
                let output = join_id(output_hi, output_lo);

                state.pending.workspaces.push(WorkspaceSnapshot {
                    id: join_id(workspace_hi, workspace_lo),
                    name,
                    output: (output != 0).then_some(output),
                    index,
                    window_count,
                    visible: visible != 0,
                    focused: focused != 0,
                });
            }
            ferese_shell_v1::Event::Window {
                window_hi: _,
                window_lo: _,
                workspace_hi,
                workspace_lo,
                app_id,
                title,
                state: window_state,
            } => {
                let (focused, fullscreen) = match window_state {
                    WEnum::Value(flags) => (
                        flags.contains(ferese_shell_v1::WindowState::Focused),
                        flags.contains(ferese_shell_v1::WindowState::Fullscreen),
                    ),
                    WEnum::Unknown(_) => (false, false),
                };

                state.pending.windows.push(WindowSnapshot {
                    workspace: join_id(workspace_hi, workspace_lo),
                    app_id,
                    title,
                    focused,
                    fullscreen,
                });
            }
            ferese_shell_v1::Event::SnapshotEnd { serial } if state.serial.take() == Some(serial) => {
                let _ = state.sender.send(ControlUpdate::Snapshot(state.pending.clone()));
            }
            ferese_shell_v1::Event::LogoutRequested { serial, output_name } => {
                let _ = state.sender.send(ControlUpdate::Logout(serial, output_name));
            }
            ferese_shell_v1::Event::ToggleKeybindingGuide { output_name } => {
                let _ = state.sender.send(ControlUpdate::ToggleGuide(output_name));
            }
            ferese_shell_v1::Event::LogoutCancelled { serial } => {
                let _ = state.sender.send(ControlUpdate::LogoutCancelled(serial));
            }
            ferese_shell_v1::Event::OverviewState { active } => {
                let _ = state.sender.send(ControlUpdate::OverviewState(active != 0));
            }
            _ => {}
        }
    }
}

delegate_noop!(ControlState: ignore FereseShellManagerV1);

fn split_id(id: u64) -> (u32, u32) {
    ((id >> 32) as u32, id as u32)
}

fn join_id(hi: u32, lo: u32) -> u64 {
    (u64::from(hi) << 32) | u64::from(lo)
}

fn append_config_chunk(config: &mut Option<String>, source: &str) {
    if let Some(buffer) = config {
        if buffer.len() + source.len() > 60 * 1024 {
            *config = None;
        } else {
            buffer.push_str(source);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_transfer_is_bounded_and_requires_begin() {
        let mut config = None;
        append_config_chunk(&mut config, "ignored");
        assert!(config.is_none());
        config = Some(String::new());
        append_config_chunk(&mut config, "🌲");
        append_config_chunk(&mut config, "theme");
        assert_eq!(config.take().as_deref(), Some("🌲theme"));
        config = Some("a".repeat(60 * 1024));
        append_config_chunk(&mut config, "overflow");
        assert!(config.is_none());
        append_config_chunk(&mut config, "ignored after overflow");
        assert!(config.is_none());
    }
}

#[cfg(test)]
mod workspace_tests {
    use super::*;

    #[test]
    fn each_bar_uses_its_own_order_and_authoritative_occupancy() {
        let workspace = |id, output, index, window_count| WorkspaceSnapshot {
            id,
            output: Some(output),
            index,
            name: index.to_string(),
            window_count,
            visible: index == 1,
            focused: output == 1 && index == 1,
        };
        let snapshot = ShellSnapshot {
            workspaces: vec![workspace(7, 1, 1, 2), workspace(9, 1, 2, 0), workspace(3, 2, 1, 0)],
            ..Default::default()
        };

        let left = snapshot.workspaces_for_output(Some(1)).collect::<Vec<_>>();
        assert_eq!(
            left.iter().map(|workspace| workspace.id).collect::<Vec<_>>(),
            vec![7, 9]
        );
        assert_eq!(left[0].window_count, 2);
        assert_eq!(
            snapshot
                .workspaces_for_output(Some(2))
                .map(|workspace| workspace.id)
                .collect::<Vec<_>>(),
            vec![3]
        );
        assert_eq!(snapshot.workspaces_for_output(None).count(), 0);
        assert_eq!(snapshot.workspaces_for_output(Some(99)).count(), 0);
    }
}
