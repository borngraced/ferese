use std::env;
use std::fs;
use std::io;
use std::os::fd::AsRawFd;
use std::os::unix::fs::{FileTypeExt, MetadataExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{
    Arc,
    mpsc::{SyncSender, sync_channel},
};
use std::thread;

use ferese_core::LayoutMode;
use ferese_ipc::{Request, Response, VERSION, read_frame, write_frame};
use ferese_layout::Direction;
use serde_json::{Value, json};
use smithay::reexports::calloop::{EventLoop, LoopSignal, channel};
use smithay::utils::Transform;

use crate::{Ferese, config::OutputTransform};

const REQUEST_QUEUE_CAPACITY: usize = 128;
const MAX_CONNECTIONS: usize = 64;

#[derive(Debug)]
struct IpcCall {
    request: Request,
    response: SyncSender<Response>,
}

struct ConnectionPermit {
    active: Arc<AtomicUsize>,
}

impl Drop for ConnectionPermit {
    fn drop(&mut self) {
        self.active.fetch_sub(1, Ordering::AcqRel);
    }
}

#[derive(Debug)]
pub(crate) struct IpcSocketGuard {
    path: PathBuf,
    device: u64,
    inode: u64,
}

impl Drop for IpcSocketGuard {
    fn drop(&mut self) {
        if self.path.symlink_metadata().is_ok_and(|metadata| {
            metadata.file_type().is_socket()
                && metadata.dev() == self.device
                && metadata.ino() == self.inode
        }) {
            let _ = fs::remove_file(&self.path);
        }
    }
}

pub(crate) fn init(
    event_loop: &mut EventLoop<'static, Ferese>,
) -> Result<IpcSocketGuard, Box<dyn std::error::Error>> {
    let path = socket_path()?;
    prepare_parent(&path)?;
    let listener = bind_listener(&path)?;
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;
    let metadata = fs::symlink_metadata(&path)?;
    let guard = IpcSocketGuard {
        path: path.clone(),
        device: metadata.dev(),
        inode: metadata.ino(),
    };

    let (sender, receiver): (channel::SyncSender<IpcCall>, channel::Channel<IpcCall>) =
        channel::sync_channel(REQUEST_QUEUE_CAPACITY);
    event_loop
        .handle()
        .insert_source(receiver, |event, _, state| {
            if let channel::Event::Msg(call) = event {
                let response = state.handle_ipc_request(call.request);
                let _ = call.response.send(response);
            }
        })?;

    let signal = event_loop.get_signal();
    thread::Builder::new()
        .name("ferese-ipc-listener".to_owned())
        .spawn(move || accept_connections(listener, sender, signal))?;

    tracing::info!(path = %path.display(), "Ferese IPC is accepting connections");
    Ok(guard)
}

fn accept_connections(
    listener: UnixListener,
    sender: channel::SyncSender<IpcCall>,
    signal: LoopSignal,
) {
    let active_connections = Arc::new(AtomicUsize::new(0));

    loop {
        let (stream, _) = match listener.accept() {
            Ok(connection) => connection,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => {
                tracing::error!(%error, "IPC listener stopped");
                return;
            }
        };

        match peer_uid(&stream) {
            Ok(uid) if uid == effective_uid() => {}
            Ok(uid) => {
                tracing::warn!(uid, "rejected IPC connection from a different user");
                continue;
            }
            Err(error) => {
                tracing::warn!(%error, "could not authenticate IPC peer");
                continue;
            }
        }

        let Some(permit) = try_acquire_connection(&active_connections) else {
            tracing::warn!(
                limit = MAX_CONNECTIONS,
                "rejected IPC connection at worker limit"
            );
            continue;
        };
        let sender = sender.clone();
        let signal = signal.clone();
        if let Err(error) = thread::Builder::new()
            .name("ferese-ipc-client".to_owned())
            .spawn(move || {
                let _permit = permit;
                serve_connection(stream, sender, signal);
            })
        {
            tracing::warn!(%error, "could not start IPC connection worker");
        }
    }
}

fn try_acquire_connection(active: &Arc<AtomicUsize>) -> Option<ConnectionPermit> {
    let mut count = active.load(Ordering::Acquire);
    loop {
        if count >= MAX_CONNECTIONS {
            return None;
        }
        match active.compare_exchange_weak(count, count + 1, Ordering::AcqRel, Ordering::Acquire) {
            Ok(_) => break,
            Err(actual) => count = actual,
        }
    }

    Some(ConnectionPermit {
        active: active.clone(),
    })
}

fn serve_connection(
    mut stream: UnixStream,
    sender: channel::SyncSender<IpcCall>,
    signal: LoopSignal,
) {
    loop {
        let request: Request = match read_frame(&mut stream) {
            Ok(request) => request,
            Err(ferese_ipc::FrameError::Io(error))
                if matches!(
                    error.kind(),
                    io::ErrorKind::UnexpectedEof
                        | io::ErrorKind::ConnectionReset
                        | io::ErrorKind::BrokenPipe
                ) =>
            {
                return;
            }
            Err(error) => {
                tracing::warn!(%error, "closing invalid IPC connection");
                return;
            }
        };
        let (response, receiver) = sync_channel(1);
        let exit_requested = request.command == "exit";
        let call = IpcCall { request, response };
        if sender.try_send(call).is_err() {
            tracing::warn!("disconnecting IPC client because the request queue is full");
            return;
        }
        let Ok(response) = receiver.recv() else {
            return;
        };
        let exit_accepted = exit_requested && response.error.is_none();
        if let Err(error) = write_frame(&mut stream, &response) {
            tracing::debug!(%error, "IPC client disconnected before receiving its response");
            return;
        }
        if exit_accepted {
            // Acknowledge before stopping so feresectl never races process exit.
            signal.stop();
            signal.wakeup();
            return;
        }
    }
}

impl Ferese {
    fn handle_ipc_request(&mut self, request: Request) -> Response {
        if self.session_lock.active {
            return Response::error(
                request.id,
                "session_locked",
                "IPC unavailable while session is locked",
            );
        }
        if let Err(error) = validate_request(&request) {
            return Response::error(request.id, error.code, error.message);
        }

        match self.dispatch_ipc_command(&request.command, &request.args) {
            Ok(result) => Response::success(request.id, result),
            Err(error) => Response::error(request.id, error.code, error.message),
        }
    }

    fn dispatch_ipc_command(&mut self, command: &str, args: &Value) -> Result<Value, CommandError> {
        match command {
            "exit" => {} // The IPC worker stops the loop after writing the response.
            "reload-config" => self
                .reload_config()
                .map_err(|e| CommandError::new("invalid_config", e))?,
            "focus" => self.focus_direction(direction_arg(args)?),
            "move" => self.move_direction(direction_arg(args)?),
            "resize" => self.resize_direction(direction_arg(args)?),
            "workspace" => self.switch_workspace(workspace_arg(args)?),
            "move-to-workspace" => self.move_focused_to_workspace(workspace_arg(args)?),
            "toggle-floating" => self.toggle_focused_floating(),
            "toggle-fullscreen" => self.toggle_focused_fullscreen(),
            "toggle-maximized" => self.toggle_focused_maximized(),
            "toggle-layout" => self.toggle_layout_mode(),
            "toggle-overview" => self.toggle_overview(),
            "cycle-column-width" => self.cycle_focused_column_width(),
            "center-column" => self.center_focused_column(),
            "consume" => self.consume_focused_window(),
            "expel" => self.expel_focused_window(),
            "close" => self.close_focused_window(),
            "get-focused-window" => return Ok(self.focused_window_json()),
            "get-workspaces" => return Ok(self.workspaces_json()),
            "get-outputs" => return Ok(self.outputs_json()),
            _ => {
                return Err(CommandError::new(
                    "unknown_command",
                    format!("unknown command {command:?}"),
                ));
            }
        }

        Ok(json!({}))
    }

    fn focused_window_json(&self) -> Value {
        self.focused_window
            .map(|window| json!({ "id": window.0 }))
            .unwrap_or(Value::Null)
    }

    fn workspaces_json(&self) -> Value {
        let active = self.workspaces.active_id();
        let mut workspaces = self
            .workspaces
            .iter()
            .map(|workspace| {
                let mode = match workspace.layout.mode() {
                    LayoutMode::Scrolling => "scrolling",
                    LayoutMode::Tree => "tree",
                };
                json!({
                    "id": workspace.id.0,
                    "name": workspace.name,
                    "active": workspace.id == active,
                    "layout": mode,
                    "focused_window": workspace.last_focused.map(|window| window.0),
                    "fullscreen_window": workspace.fullscreen.map(|window| window.0),
                })
            })
            .collect::<Vec<_>>();
        workspaces.sort_by_key(|workspace| workspace["id"].as_u64());
        Value::Array(workspaces)
    }

    fn outputs_json(&self) -> Value {
        if let Some(backend) = self.direct_backend.as_ref() {
            let focused = self.output_workspaces.focused_output();
            let mut outputs = backend
                .connected_outputs
                .iter()
                .map(|info| {
                    let mapped = self
                        .space
                        .outputs()
                        .find(|output| output.name() == info.connector);
                    let id = mapped.and_then(|output| self.output_id(output));
                    let geometry = mapped.and_then(|output| self.space.output_geometry(output));
                    let workspace = id.and_then(|id| self.output_workspaces.active_workspace(id));
                    let position = geometry
                        .map(|geometry| [geometry.loc.x, geometry.loc.y])
                        .or(info.configured_position);
                    let scale = mapped
                        .map(|output| output.current_scale().fractional_scale())
                        .unwrap_or(info.scale);
                    let transform = mapped
                        .map(|output| transform_name(output.current_transform()))
                        .unwrap_or_else(|| configured_transform_name(info.transform));
                    let modes = info
                        .available_modes
                        .iter()
                        .map(|mode| {
                            json!({
                                "width": mode.width,
                                "height": mode.height,
                                "refresh_millihertz": mode.refresh_millihertz,
                                "refresh_hz": f64::from(mode.refresh_millihertz) / 1_000.0,
                                "preferred": mode.preferred,
                            })
                        })
                        .collect::<Vec<_>>();
                    let current_mode = info.current_mode.map(|mode| {
                        json!({
                            "width": mode.width,
                            "height": mode.height,
                            "refresh_millihertz": mode.refresh_millihertz,
                            "refresh_hz": f64::from(mode.refresh_millihertz) / 1_000.0,
                        })
                    });

                    json!({
                        "id": id.map(|id| id.0),
                        "name": info.connector,
                        "connector": info.connector,
                        "identity": info.identity,
                        "connected": true,
                        "enabled": info.enabled && mapped.is_some(),
                        "profile": info.profile,
                        "focused": id.is_some() && id == focused,
                        "workspace": workspace.map(|workspace| workspace.0),
                        "x": position.map(|position| position[0]),
                        "y": position.map(|position| position[1]),
                        "width": geometry.map(|geometry| geometry.size.w),
                        "height": geometry.map(|geometry| geometry.size.h),
                        "physical_width_mm": info.physical_size.map(|size| size.0),
                        "physical_height_mm": info.physical_size.map(|size| size.1),
                        "scale": scale,
                        "transform": transform,
                        "current_mode": current_mode,
                        "available_modes": modes,
                    })
                })
                .collect::<Vec<_>>();
            outputs.sort_by(|left, right| {
                left["connector"].as_str().cmp(&right["connector"].as_str())
            });
            return Value::Array(outputs);
        }

        let focused = self.output_workspaces.focused_output();
        let mut outputs = self
            .space
            .outputs()
            .filter_map(|output| {
                let id = self.output_id(output)?;
                let geometry = self.space.output_geometry(output)?;
                let workspace = self.output_workspaces.active_workspace(id)?;
                let mode = output.current_mode();
                let physical = output.physical_properties().size;
                Some(json!({
                    "id": id.0,
                    "name": output.name(),
                    "connector": output.name(),
                    "identity": Value::Null,
                    "connected": true,
                    "enabled": true,
                    "focused": Some(id) == focused,
                    "workspace": workspace.0,
                    "x": geometry.loc.x,
                    "y": geometry.loc.y,
                    "width": geometry.size.w,
                    "height": geometry.size.h,
                    "physical_width_mm": physical.w,
                    "physical_height_mm": physical.h,
                    "scale": output.current_scale().fractional_scale(),
                    "transform": transform_name(output.current_transform()),
                    "current_mode": mode.map(|mode| json!({
                        "width": mode.size.w,
                        "height": mode.size.h,
                        "refresh_millihertz": mode.refresh,
                        "refresh_hz": f64::from(mode.refresh) / 1_000.0,
                    })),
                    "available_modes": output.modes().into_iter().map(|mode| json!({
                        "width": mode.size.w,
                        "height": mode.size.h,
                        "refresh_millihertz": mode.refresh,
                        "refresh_hz": f64::from(mode.refresh) / 1_000.0,
                        "preferred": Some(mode) == output.preferred_mode(),
                    })).collect::<Vec<_>>(),
                }))
            })
            .collect::<Vec<_>>();
        outputs.sort_by_key(|output| output["id"].as_u64());
        Value::Array(outputs)
    }
}

fn validate_request(request: &Request) -> Result<(), CommandError> {
    if request.version != VERSION {
        return Err(CommandError::new(
            "unsupported_version",
            format!("supported version is {VERSION}"),
        ));
    }
    if request.kind != "command" {
        return Err(CommandError::new(
            "invalid_type",
            "expected type \"command\"",
        ));
    }

    Ok(())
}

fn transform_name(transform: Transform) -> &'static str {
    match transform {
        Transform::Normal => "normal",
        Transform::_90 => "rotate_90",
        Transform::_180 => "rotate_180",
        Transform::_270 => "rotate_270",
        Transform::Flipped => "flipped",
        Transform::Flipped90 => "flipped_90",
        Transform::Flipped180 => "flipped_180",
        Transform::Flipped270 => "flipped_270",
    }
}

fn configured_transform_name(transform: OutputTransform) -> &'static str {
    match transform {
        OutputTransform::Normal => "normal",
        OutputTransform::Rotate90 => "rotate_90",
        OutputTransform::Rotate180 => "rotate_180",
        OutputTransform::Rotate270 => "rotate_270",
        OutputTransform::Flipped => "flipped",
        OutputTransform::Flipped90 => "flipped_90",
        OutputTransform::Flipped180 => "flipped_180",
        OutputTransform::Flipped270 => "flipped_270",
    }
}

#[derive(Debug)]
struct CommandError {
    code: &'static str,
    message: String,
}

impl CommandError {
    fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

fn direction_arg(args: &Value) -> Result<Direction, CommandError> {
    match args.get("direction").and_then(Value::as_str) {
        Some("left") => Ok(Direction::Left),
        Some("right") => Ok(Direction::Right),
        Some("up") => Ok(Direction::Up),
        Some("down") => Ok(Direction::Down),
        _ => Err(CommandError::new(
            "invalid_argument",
            "direction must be left, right, up, or down",
        )),
    }
}

fn workspace_arg(args: &Value) -> Result<u32, CommandError> {
    args.get("index")
        .and_then(Value::as_u64)
        .and_then(|index| u32::try_from(index).ok())
        .filter(|index| *index > 0)
        .ok_or_else(|| {
            CommandError::new(
                "invalid_argument",
                "index must be a positive 32-bit integer",
            )
        })
}

fn socket_path() -> Result<PathBuf, io::Error> {
    env::var_os("XDG_RUNTIME_DIR")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .map(|directory| directory.join("ferese/control.sock"))
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "XDG_RUNTIME_DIR is not set"))
}

fn prepare_parent(socket: &Path) -> Result<(), io::Error> {
    let parent = socket
        .parent()
        .expect("the Ferese control socket always has a parent");
    match fs::symlink_metadata(parent) {
        Ok(metadata) => {
            if !metadata.is_dir() || metadata.file_type().is_symlink() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("{} is not a real directory", parent.display()),
                ));
            }
            if metadata.uid() != effective_uid() {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    format!("{} is owned by another user", parent.display()),
                ));
            }
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => fs::create_dir(parent)?,
        Err(error) => return Err(error),
    }

    fs::set_permissions(parent, fs::Permissions::from_mode(0o700))
}

fn bind_listener(path: &Path) -> Result<UnixListener, io::Error> {
    match UnixListener::bind(path) {
        Ok(listener) => Ok(listener),
        Err(error) if error.kind() == io::ErrorKind::AddrInUse => {
            if UnixStream::connect(path).is_ok() {
                return Err(io::Error::new(
                    io::ErrorKind::AddrInUse,
                    "another Ferese IPC server is already running",
                ));
            }
            let metadata = fs::symlink_metadata(path)?;
            if !metadata.file_type().is_socket() || metadata.uid() != effective_uid() {
                return Err(error);
            }
            fs::remove_file(path)?;
            UnixListener::bind(path)
        }
        Err(error) => Err(error),
    }
}

fn peer_uid(stream: &UnixStream) -> Result<u32, io::Error> {
    let mut credentials = libc::ucred {
        pid: 0,
        uid: 0,
        gid: 0,
    };
    let mut length = std::mem::size_of::<libc::ucred>() as libc::socklen_t;

    // SAFETY: `credentials` and `length` point to initialized, correctly sized
    // storage for Linux SO_PEERCRED, and the stream owns a live file descriptor.
    let result = unsafe {
        libc::getsockopt(
            stream.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            (&raw mut credentials).cast(),
            &raw mut length,
        )
    };
    if result == -1 {
        Err(io::Error::last_os_error())
    } else {
        Ok(credentials.uid)
    }
}

fn effective_uid() -> u32 {
    // SAFETY: `geteuid` has no preconditions and cannot fail.
    unsafe { libc::geteuid() }
}

#[cfg(test)]
mod tests {
    use std::io::{Read, Write};
    use std::net::Shutdown;
    use std::os::unix::fs::PermissionsExt;

    use super::*;

    fn request(version: u32, kind: &str) -> Request {
        Request {
            version,
            id: 7,
            kind: kind.to_owned(),
            command: "get-outputs".to_owned(),
            args: json!({}),
        }
    }

    #[test]
    fn parses_direction_arguments() {
        assert_eq!(
            direction_arg(&json!({ "direction": "left" })).unwrap(),
            Direction::Left
        );
        assert!(direction_arg(&json!({ "direction": "diagonal" })).is_err());
    }

    #[test]
    fn validates_workspace_arguments() {
        assert_eq!(workspace_arg(&json!({ "index": 9 })).unwrap(), 9);
        assert!(workspace_arg(&json!({ "index": 0 })).is_err());
        assert!(workspace_arg(&json!({ "index": -1 })).is_err());
    }

    #[test]
    fn rejects_unsupported_versions_and_request_types_with_stable_codes() {
        let version = validate_request(&request(VERSION + 1, "command")).unwrap_err();
        let kind = validate_request(&request(VERSION, "event")).unwrap_err();

        assert_eq!(version.code, "unsupported_version");
        assert_eq!(kind.code, "invalid_type");
        assert!(validate_request(&request(VERSION, "command")).is_ok());
    }

    #[test]
    fn reads_same_user_peer_credentials() {
        let (left, right) = UnixStream::pair().unwrap();

        assert_eq!(peer_uid(&left).unwrap(), effective_uid());
        assert_eq!(peer_uid(&right).unwrap(), effective_uid());
    }

    #[test]
    fn malformed_ipc_connection_is_closed_without_reaching_the_request_queue() {
        let (mut client, server) = UnixStream::pair().unwrap();
        let (sender, _receiver) = channel::sync_channel(1);
        let event_loop = EventLoop::<()>::try_new().unwrap();
        let signal = event_loop.get_signal();
        let worker = thread::spawn(move || serve_connection(server, sender, signal));
        let invalid = b"not-json";

        client
            .write_all(&(invalid.len() as u32).to_be_bytes())
            .unwrap();
        client.write_all(invalid).unwrap();
        client.shutdown(Shutdown::Write).unwrap();
        client
            .set_read_timeout(Some(std::time::Duration::from_secs(1)))
            .unwrap();
        let mut byte = [0_u8; 1];

        assert_eq!(client.read(&mut byte).unwrap(), 0);
        worker.join().unwrap();
    }

    #[test]
    fn runtime_socket_directory_is_private() {
        let root = unique_test_directory("private-parent");
        let parent = root.join("ferese");
        let socket = parent.join("control.sock");

        fs::create_dir(&root).unwrap();
        prepare_parent(&socket).unwrap();
        let metadata = fs::metadata(&parent).unwrap();

        assert_eq!(metadata.permissions().mode() & 0o777, 0o700);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn listener_reclaims_only_a_stale_owned_socket() {
        let root = unique_test_directory("stale-socket");
        fs::create_dir(&root).unwrap();
        let socket = root.join("control.sock");
        let first = bind_listener(&socket).unwrap();

        assert_eq!(
            bind_listener(&socket).unwrap_err().kind(),
            io::ErrorKind::AddrInUse
        );
        drop(first);
        let replacement = bind_listener(&socket).unwrap();

        drop(replacement);
        fs::remove_file(socket).unwrap();
        fs::remove_dir(root).unwrap();
    }

    #[test]
    fn listener_never_replaces_a_non_socket_path() {
        let root = unique_test_directory("non-socket");
        fs::create_dir(&root).unwrap();
        let socket = root.join("control.sock");
        fs::write(&socket, b"keep").unwrap();

        assert_eq!(
            bind_listener(&socket).unwrap_err().kind(),
            io::ErrorKind::AddrInUse
        );
        assert_eq!(fs::read(&socket).unwrap(), b"keep");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn ipc_worker_count_is_bounded_and_permits_are_reusable() {
        let active = Arc::new(AtomicUsize::new(0));
        let mut permits = (0..MAX_CONNECTIONS)
            .map(|_| try_acquire_connection(&active).unwrap())
            .collect::<Vec<_>>();

        assert!(try_acquire_connection(&active).is_none());
        permits.pop();
        assert_eq!(active.load(Ordering::Acquire), MAX_CONNECTIONS - 1);
        assert!(try_acquire_connection(&active).is_some());
    }

    fn unique_test_directory(label: &str) -> PathBuf {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        env::temp_dir().join(format!("ferese-ipc-{label}-{}-{nonce}", std::process::id()))
    }
}
