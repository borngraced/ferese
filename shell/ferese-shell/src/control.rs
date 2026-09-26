use std::{
    error::Error,
    os::{
        fd::{FromRawFd, RawFd},
        unix::net::UnixStream,
    },
    sync::mpsc::{self, Receiver, Sender, TryRecvError},
    thread,
};

use ferese_protocols::shell::v1::client::{
    ferese_shell_manager_v1::FereseShellManagerV1,
    ferese_shell_v1::{self, FereseShellV1},
};
use wayland_client::{
    Connection, Dispatch, QueueHandle, WEnum, delegate_noop,
    globals::{GlobalListContents, registry_queue_init},
    protocol::wl_registry,
};

#[derive(Clone, Debug, Default)]
pub(crate) struct ShellSnapshot {
    pub(crate) outputs: Vec<OutputSnapshot>,
    pub(crate) workspaces: Vec<WorkspaceSnapshot>,
    pub(crate) windows: Vec<WindowSnapshot>,
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
    pub(crate) output: Option<u64>,
    pub(crate) active: bool,
}

#[derive(Clone, Debug)]
pub(crate) struct WindowSnapshot {
    pub(crate) id: u64,
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
    pub(crate) snapshot: Option<ShellSnapshot>,
    pub(crate) overview_active: Option<bool>,
    pub(crate) disconnected: bool,
}

enum ControlUpdate {
    Snapshot(ShellSnapshot),
    OverviewState(bool),
    Disconnected,
}

impl ShellControl {
    pub(crate) fn connect() -> Result<Self, Box<dyn Error>> {
        let connection = control_connection()?;
        let (globals, mut queue) = registry_queue_init::<ControlState>(&connection)?;
        let qh = queue.handle();
        let manager = globals.bind::<FereseShellManagerV1, _, _>(&qh, 1..=1, ())?;
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
            snapshot: None,
            overview_active: None,
            disconnected: false,
        };

        loop {
            match self.updates.try_recv() {
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

    pub(crate) fn activate_window(&self, id: u64) {
        let (hi, lo) = split_id(id);
        self.shell.activate_window(hi, lo);
        let _ = self.connection.flush();
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
    let raw_fd = std::env::var_os("FERESE_SHELL_CONTROL_SOCKET")
        .ok_or("Ferese did not provide a shell-control connection")?;
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
    sender: Sender<ControlUpdate>,
    pending: ShellSnapshot,
    serial: Option<u32>,
}

impl ControlState {
    fn new(sender: Sender<ControlUpdate>) -> Self {
        Self {
            sender,
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
                name: _,
                active,
            } => {
                let output = join_id(output_hi, output_lo);

                state.pending.workspaces.push(WorkspaceSnapshot {
                    id: join_id(workspace_hi, workspace_lo),
                    output: (output != 0).then_some(output),
                    active: active != 0,
                });
            }
            ferese_shell_v1::Event::Window {
                window_hi,
                window_lo,
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
                    id: join_id(window_hi, window_lo),
                    workspace: join_id(workspace_hi, workspace_lo),
                    app_id,
                    title,
                    focused,
                    fullscreen,
                });
            }
            ferese_shell_v1::Event::SnapshotEnd { serial }
                if state.serial.take() == Some(serial) =>
            {
                let _ = state
                    .sender
                    .send(ControlUpdate::Snapshot(state.pending.clone()));
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
