use std::env;
use std::fs;
use std::io;
use std::os::fd::AsRawFd;
use std::os::unix::fs::{FileTypeExt, MetadataExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{
    Arc,
    mpsc::{SyncSender, sync_channel},
};
use std::thread;
use std::time::Duration;

use ferese_core::LayoutMode;
use ferese_ipc::{Request, Response, VERSION, read_frame, write_frame};
use ferese_layout::Direction;
use serde_json::{Value, json};
use smithay::reexports::calloop::{EventLoop, LoopSignal, channel, timer};
use smithay::utils::Transform;

use crate::handlers::screencopy;
use crate::handlers::screenshot::{
    Action, Geometry, OutputLayout, PartOutcome, PartSender, parse_geometry, plan,
};
use crate::handlers::screenshot_worker::{self, Encoded, Job, Worker};
use crate::{Ferese, config::OutputTransform};
use smithay::output::Output;

const REQUEST_QUEUE_CAPACITY: usize = 128;
const MAX_CONNECTIONS: usize = 64;
const RESULT_QUEUE_CAPACITY: usize = 8;
// The encode thread pushes one result per job, and at most QUEUE_CAPACITY jobs
// can be queued while a further one is in flight. Keeping the result queue
// larger than that is what stops the worker's send from ever blocking, which
// would otherwise wedge it whenever the event loop is busy.
const _: () = assert!(
    RESULT_QUEUE_CAPACITY > crate::handlers::screenshot_worker::QUEUE_CAPACITY + 1,
    "the result queue must outsize the job queue plus the in-flight job"
);
const SWEEP_INTERVAL: Duration = crate::handlers::screenshot_worker::SWEEP_INTERVAL;
// Only consulted when no request is outstanding, so it bounds how late a newly
// admitted request can be noticed. With a request pending the timer re-arms to
// the exact remaining time instead.
const DEADLINE_IDLE: Duration = Duration::from_secs(1);

#[derive(Debug)]
struct IpcCall {
    owner: u64,
    request: Request,
    response: SyncSender<Response>,
}

#[derive(Debug)]
enum IpcEvent {
    Call(IpcCall),
    Closed(u64),
}

struct IpcConnection {
    owner: u64,
    sender: channel::SyncSender<IpcEvent>,
}

impl Drop for IpcConnection {
    fn drop(&mut self) {
        let _ = self.sender.send(IpcEvent::Closed(self.owner));
    }
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
) -> Result<ScreenshotInit, Box<dyn std::error::Error>> {
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

    let (sender, receiver): (channel::SyncSender<IpcEvent>, channel::Channel<IpcEvent>) =
        channel::sync_channel(REQUEST_QUEUE_CAPACITY);
    event_loop
        .handle()
        .insert_source(receiver, |event, _, state| {
            if let channel::Event::Msg(event) = event {
                let call = match event {
                    IpcEvent::Call(call) => call,
                    IpcEvent::Closed(owner) => {
                        state.portal_shortcuts.remove(owner);
                        state.portal_session.remove(owner);
                        state.refresh_idle_inhibition();
                        return;
                    }
                };
                if call.request.command == "session-watch" {
                    if let Err(error) = validate_request(&call.request) {
                        let _ = call.response.send(Response::error(
                            call.request.id,
                            error.code,
                            error.message,
                        ));
                    } else if let Some(since) = call.request.args["since"].as_u64() {
                        state.portal_session.watch(
                            call.owner,
                            call.request.id,
                            since,
                            call.response,
                        );
                    } else {
                        let _ = call.response.send(Response::error(
                            call.request.id,
                            "invalid_argument",
                            "Missing session revision",
                        ));
                    }
                } else if call.request.command == "screenshot-window" {
                    state.start_window_screenshot(call.request, call.response);
                } else if call.request.command == "screenshot" {
                    // Deferred: this path answers the caller itself, exactly
                    // once, whenever the request finishes or is terminated.
                    state.start_screenshot(call.request, call.response);
                } else {
                    let response = state.handle_ipc_request(call.owner, call.request);
                    let _ = call.response.send(response);
                }
            }
        })?;

    // Screenshot readbacks publish parts here, so the loop wakes as soon as a
    // readback lands, whichever backend produced it.
    let (parts, part_events) = channel::channel::<PartOutcome>();
    event_loop
        .handle()
        .insert_source(part_events, |event, _, state| {
            if let channel::Event::Msg(outcome) = event {
                state.on_screenshot_part(outcome);
            }
        })?;

    let (encoded, encoded_events) = channel::sync_channel::<Encoded>(RESULT_QUEUE_CAPACITY);
    event_loop
        .handle()
        .insert_source(encoded_events, |event, _, state| {
            if let channel::Event::Msg(result) = event {
                state.on_screenshot_encoded(result);
            }
        })?;
    let worker = Worker::spawn(encoded);

    let signal = event_loop.get_signal();
    thread::Builder::new()
        .name("ferese-ipc-listener".to_owned())
        .spawn(move || accept_connections(listener, sender, signal))?;

    // A client that dies between receiving a screenshot path and unlinking it
    // leaves the file behind, so staged files are reclaimed at startup and then
    // periodically. The TTL keeps this from disturbing a live request.
    screenshot_worker::sweep_stale_files();
    event_loop
        .handle()
        .insert_source(timer::Timer::from_duration(SWEEP_INTERVAL), |_, _, _| {
            screenshot_worker::sweep_stale_files();
            timer::TimeoutAction::ToDuration(SWEEP_INTERVAL)
        })?;

    // Readbacks are published by the render path, so a request whose outputs
    // are not redrawn would otherwise wait forever. This re-arms to the exact
    // remaining time whenever a request is outstanding, and idles otherwise.
    event_loop.handle().insert_source(
        timer::Timer::from_duration(DEADLINE_IDLE),
        |_, _, state| {
            // Abandoned requests have already been answered, so their
            // readbacks are simply dropped: they are not failed, because
            // that would publish into a request that no longer exists.
            for request in state.screenshot.expire() {
                state
                    .pending_screencopies
                    .retain(|capture| capture.request_id() != Some(request));
            }
            let next = state.screenshot.next_deadline().unwrap_or(DEADLINE_IDLE);
            timer::TimeoutAction::ToDuration(next)
        },
    )?;

    tracing::info!(path = %path.display(), "Ferese IPC is accepting connections");

    let Some(worker) = worker else {
        return Err("Could not start the screenshot worker".into());
    };
    Ok(ScreenshotInit {
        parts,
        worker,
        _guard: guard,
    })
}

pub(crate) struct ScreenshotInit {
    pub(crate) parts: PartSender,
    pub(crate) worker: Worker,
    pub(crate) _guard: IpcSocketGuard,
}

fn accept_connections(
    listener: UnixListener,
    sender: channel::SyncSender<IpcEvent>,
    signal: LoopSignal,
) {
    let active_connections = Arc::new(AtomicUsize::new(0));
    let connection_ids = AtomicU64::new(1);

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
        let owner = connection_ids.fetch_add(1, Ordering::Relaxed);
        let signal = signal.clone();
        if let Err(error) = thread::Builder::new()
            .name("ferese-ipc-client".to_owned())
            .spawn(move || {
                let _permit = permit;
                serve_connection(stream, sender, signal, owner);
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
    sender: channel::SyncSender<IpcEvent>,
    signal: LoopSignal,
    owner: u64,
) {
    let _connection = IpcConnection {
        owner,
        sender: sender.clone(),
    };
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
        let screenshot_requested =
            matches!(request.command.as_str(), "screenshot" | "screenshot-window");
        let call = IpcEvent::Call(IpcCall {
            owner,
            request,
            response,
        });
        if sender.try_send(call).is_err() {
            tracing::warn!("disconnecting IPC client because the request queue is full");
            return;
        }
        let response = loop {
            match receiver.recv_timeout(Duration::from_secs(1)) {
                Ok(response) => break response,
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => return,
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                    let mut byte = 0_u8;
                    // SAFETY: the live stream owns this fd and byte is writable.
                    let result = unsafe {
                        libc::recv(
                            std::os::fd::AsRawFd::as_raw_fd(&stream),
                            (&mut byte as *mut u8).cast(),
                            1,
                            libc::MSG_PEEK | libc::MSG_DONTWAIT,
                        )
                    };
                    if result == 0 {
                        return;
                    }
                    if result < 0
                        && !matches!(
                            io::Error::last_os_error().kind(),
                            io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
                        )
                    {
                        return;
                    }
                }
            }
        };
        let exit_accepted = exit_requested && response.error.is_none();
        if let Err(error) = write_frame(&mut stream, &response) {
            if screenshot_requested {
                discard_undelivered_screenshot(&response);
            }
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

fn u32_arg(args: &Value, key: &str) -> Result<u32, CommandError> {
    args[key]
        .as_u64()
        .and_then(|value| u32::try_from(value).ok())
        .ok_or_else(|| CommandError::new("invalid_argument", format!("Missing or invalid {key}")))
}

fn discard_undelivered_screenshot(response: &Response) {
    if response.error.is_none()
        && let Some(path) = response
            .result
            .as_ref()
            .and_then(|result| result["path"].as_str())
        && let Err(error) = fs::remove_file(path)
        && error.kind() != io::ErrorKind::NotFound
    {
        tracing::debug!(%error, "failed to remove an undelivered screenshot");
    }
}

impl Ferese {
    fn handle_ipc_request(&mut self, owner: u64, request: Request) -> Response {
        if self.session_lock.active
            && !matches!(
                request.command.as_str(),
                "portal-shortcuts-poll"
                    | "get-session-state"
                    | "portal-inhibit"
                    | "portal-monitor-register"
                    | "portal-monitor-ack"
                    | "logind-session-ending"
                    | "cancel-session-end"
            )
        {
            return Response::error(
                request.id,
                "session_locked",
                "IPC unavailable while session is locked",
            );
        }
        if let Err(error) = validate_request(&request) {
            return Response::error(request.id, error.code, error.message);
        }

        match self.dispatch_ipc_command(owner, &request.command, &request.args) {
            Ok(result) => Response::success(request.id, result),
            Err(error) => Response::error(request.id, error.code, error.message),
        }
    }

    fn dispatch_ipc_command(
        &mut self,
        owner: u64,
        command: &str,
        args: &Value,
    ) -> Result<Value, CommandError> {
        match command {
            "portal-inhibit" => {
                let inhibition = serde_json::from_value(args.clone())
                    .map_err(|error| CommandError::new("invalid_argument", error.to_string()))?;
                self.portal_session
                    .register(owner, inhibition)
                    .map_err(|error| CommandError::new("invalid_argument", error))?;
                self.refresh_idle_inhibition();
                return Ok(json!({}));
            }
            "get-session-state" => {
                return Ok(self.portal_session.snapshot());
            }
            "portal-monitor-register" => {
                self.portal_session
                    .register_monitor(owner)
                    .map_err(|error| CommandError::new("invalid_argument", error))?;
                let mut result = self.portal_session.snapshot();
                result["monitor-owner"] = json!(owner);
                return Ok(result);
            }
            "portal-monitor-ack" => {
                let token = u32_arg(args, "token")?;
                self.portal_session
                    .acknowledge(owner, token)
                    .map_err(|error| CommandError::new("invalid_argument", error))?;
                return Ok(json!({}));
            }
            "begin-session-end" => {
                self.portal_session.begin_owned_query(owner);
                return Ok(self.portal_session.snapshot());
            }
            "cancel-session-end" => {
                self.portal_session.cancel_query(u32_arg(args, "token")?);
                return Ok(json!({}));
            }
            "validate-session-end" => {
                self.portal_session
                    .validate_end(
                        u32_arg(args, "token")?,
                        u32_arg(args, "inhibitor-revision")?,
                        args["force"].as_bool().ok_or_else(|| {
                            CommandError::new("invalid_argument", "Missing force confirmation")
                        })?,
                    )
                    .map_err(|error| CommandError::new("inhibited", error))?;
                return Ok(json!({}));
            }
            "commit-session-end" => {
                self.portal_session
                    .commit_end(
                        u32_arg(args, "token")?,
                        u32_arg(args, "inhibitor-revision")?,
                        args["force"].as_bool().ok_or_else(|| {
                            CommandError::new("invalid_argument", "Missing force confirmation")
                        })?,
                    )
                    .map_err(|error| CommandError::new("inhibited", error))?;
                return Ok(json!({}));
            }
            "logind-session-ending" => {
                let ending = args["ending"].as_bool().ok_or_else(|| {
                    CommandError::new("invalid_argument", "Missing shutdown state")
                })?;
                self.portal_session.set_phase(if ending { 3 } else { 1 });
                return Ok(json!({}));
            }
            "portal-shortcuts-register" => {
                let shortcuts = serde_json::from_value(args.clone())
                    .map_err(|error| CommandError::new("invalid_argument", error.to_string()))?;
                self.portal_shortcuts
                    .register(owner, shortcuts, &self.bindings, &self.input_settings)
                    .map_err(|error| CommandError::new("shortcut_conflict", error))?;
                return Ok(json!({}));
            }
            "portal-shortcuts-poll" => {
                return Ok(self.portal_shortcuts.poll(owner, self.session_lock.active));
            }
            "exit" => {} // The IPC worker stops the loop after writing the response.
            "request-logout" => self.request_logout_confirmation(),
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
            "get-windows" => return Ok(self.windows_json()),
            "get-keybindings" => {
                let map = crate::config::physical_keymap(&self.input_settings).ok();
                return Ok(json!(
                    self.bindings
                        .iter()
                        .filter_map(|binding| binding.guide_entry(map.as_ref()))
                        .take(256)
                        .collect::<Vec<_>>()
                ));
            }
            "has-client-surfaces" => {
                let pid = args
                    .get("pid")
                    .and_then(Value::as_u64)
                    .and_then(|pid| u32::try_from(pid).ok())
                    .ok_or_else(|| {
                        CommandError::new("invalid_argument", "pid must be an unsigned process ID")
                    })?;
                return Ok(json!(self.has_client_surfaces(pid)));
            }
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

    // Screenshot replies are deferred: this either answers the caller now, or
    // hands the reply to the coordinator, which answers exactly once.
    pub(crate) fn start_screenshot(&mut self, request: Request, response: SyncSender<Response>) {
        // A macro rather than a closure: the message may be a borrowed str or an
        // owned String, and Response::error already accepts either.
        macro_rules! reject {
            ($code:expr, $message:expr) => {
                let _ = response.try_send(Response::error(request.id, $code, $message));
            };
        }
        if self.session_lock.active {
            reject!("session_locked", "IPC unavailable while session is locked");
            return;
        }
        if !screencopy::capture_allowed() {
            reject!(
                "capture_disabled",
                "Screen capture is disabled for this session; start it with \
                 FERESE_ENABLE_SCREENCOPY=1 to enable screenshots"
            );
            return;
        }
        if let Err(error) = validate_request(&request) {
            reject!(error.code, error.message);
            return;
        }
        let geometry = match geometry_arg(&request.args) {
            Ok(geometry) => geometry,
            Err(message) => {
                reject!("invalid_argument", &message);
                return;
            }
        };
        if self.screenshot_parts.is_none() || self.screenshot_worker.is_none() {
            reject!("unavailable", "Screenshot capture is not available");
            return;
        }
        let Some(parts) = self.screenshot_parts.clone() else {
            reject!("unavailable", "Screenshot capture is not available");
            return;
        };

        // Snapshot the layout, so a later move, rescale, or transform cannot
        // change what this request captures.
        let targets: Vec<(Output, OutputLayout)> = self
            .space
            .outputs()
            .filter_map(|output| {
                let mode = output.current_mode()?;
                let geometry = self.space.output_geometry(output)?;
                Some((
                    output.clone(),
                    OutputLayout {
                        mode_size: mode.size,
                        scale: output.current_scale().fractional_scale(),
                        transform: output.current_transform(),
                        location: geometry.loc,
                    },
                ))
            })
            .collect();
        let layouts: Vec<OutputLayout> = targets.iter().map(|(_, layout)| layout.clone()).collect();
        let planned = match plan(&geometry, &layouts) {
            Ok(planned) => planned,
            Err(message) => {
                reject!("invalid_request", &message);
                return;
            }
        };
        let specs = planned
            .iter()
            .map(|part| part.spec(&layouts[part.index]))
            .collect();

        // admit stores the reply only on success, so a clone survives the
        // rejection path and every caller is answered exactly once.
        let fallback = response.clone();
        let id = match self.screenshot.admit(request.id, response, specs) {
            Ok(id) => id,
            Err(message) => {
                let _ =
                    fallback.try_send(Response::error(request.id, "screenshot_rejected", message));
                return;
            }
        };

        for (position, part) in planned.iter().enumerate() {
            let (output, _) = &targets[part.index];
            self.pending_screencopies
                .push(crate::handlers::screencopy::PendingScreencopy::owned(
                    id,
                    position,
                    parts.clone(),
                    output.clone(),
                    part.buffer,
                ));
        }

        // The nested backend redraws on its refresh timer; this drives the
        // direct backend immediately and is a no-op otherwise.
        crate::backends::direct::render_all(self);
    }

    fn start_window_screenshot(&mut self, request: Request, response: SyncSender<Response>) {
        let reject = |message: String| {
            let _ = response.try_send(Response::error(
                request.id,
                "window_capture_failed",
                message,
            ));
        };
        if self.session_lock.active || !screencopy::capture_allowed() {
            reject("Screen capture is unavailable".into());
            return;
        }
        if let Err(error) = validate_request(&request) {
            reject(error.message);
            return;
        }
        let Some(id) = request
            .args
            .get("window")
            .and_then(Value::as_u64)
            .map(ferese_layout::WindowId)
        else {
            reject("Expected a window ID".into());
            return;
        };
        let Some(window) = self
            .window_ids
            .iter()
            .find(|(_, candidate)| **candidate == id)
            .map(|(window, _)| window.clone())
        else {
            reject("Window no longer exists".into());
            return;
        };
        let Some(output) = self
            .space
            .outputs()
            .find(|output| self.window_belongs_to_output(id, output))
            .cloned()
        else {
            reject("Window output is unavailable".into());
            return;
        };
        let geometry = window.geometry();
        let scale = output.current_scale().fractional_scale();
        let size = geometry.size.to_physical_precise_round(scale);
        let spec = crate::handlers::screenshot::PartSpec {
            preserve_alpha: true,
            transform: Transform::Normal,
            scale,
            location: (0, 0),
            logical_width: geometry.size.w,
            logical_height: geometry.size.h,
            buffer_width: size.w,
            buffer_height: size.h,
        };
        let fallback = response.clone();
        let capture = match self.screenshot.admit(request.id, response, vec![spec]) {
            Ok(capture) => capture,
            Err(error) => {
                let _ =
                    fallback.try_send(Response::error(request.id, "screenshot_rejected", error));
                return;
            }
        };
        let result = if let Some(backend) = &self.nested_backend {
            match backend.try_borrow_mut() {
                Ok(mut backend) => crate::winit::capture_window_buffer(
                    backend.renderer(),
                    &window,
                    geometry,
                    scale,
                ),
                Err(_) => Err("Window renderer is busy".into()),
            }
        } else if let Some(backend) = &mut self.direct_backend {
            backend.capture_window_buffer(&window, geometry, &output)
        } else {
            Err("Window renderer is unavailable".into())
        };
        self.on_screenshot_part(PartOutcome {
            request: capture,
            part: 0,
            result,
        });
    }

    pub(crate) fn on_screenshot_part(&mut self, outcome: PartOutcome) {
        let action = self
            .screenshot
            .on_part(outcome.request, outcome.part, outcome.result);
        self.apply_screenshot_action(action);
    }

    pub(crate) fn on_screenshot_encoded(&mut self, result: Encoded) {
        let action = self.screenshot.on_encoded(result.request, result.result);
        self.apply_screenshot_action(action);
    }

    fn apply_screenshot_action(&mut self, action: Action) {
        match action {
            Action::None => {}
            Action::Encode { request, frames } => {
                let submitted = self
                    .screenshot_worker
                    .as_ref()
                    .is_some_and(|worker| worker.submit(Job { request, frames }).is_ok());
                if !submitted {
                    self.screenshot
                        .reject(request, "Screenshot encoding queue is full");
                }
            }
            Action::Delivered(path) => {
                // The caller opens and unlinks it; the path was already sent.
                tracing::debug!(path = %path.display(), "delivered a screenshot");
            }
            Action::DiscardFile(path) => {
                if let Err(error) = fs::remove_file(&path)
                    && error.kind() != io::ErrorKind::NotFound
                {
                    tracing::debug!(
                        %error,
                        path = %path.display(),
                        "failed to remove a staged screenshot"
                    );
                }
            }
        }
    }

    fn focused_window_json(&self) -> Value {
        self.focused_window
            .map(|window| json!({ "id": window.0 }))
            .unwrap_or(Value::Null)
    }

    fn has_client_surfaces(&self, pid: u32) -> bool {
        use smithay::reexports::wayland_server::Resource;
        let belongs_to =
            |surface: &smithay::reexports::wayland_server::protocol::wl_surface::WlSurface| {
                surface
                    .client()
                    .and_then(|client| client.get_credentials(&self.display_handle).ok())
                    .is_some_and(|credentials| credentials.pid as u32 == pid)
            };
        self.window_ids
            .keys()
            .filter_map(|window| window.toplevel())
            .any(|toplevel| belongs_to(toplevel.wl_surface()))
            || self.space.outputs().any(|output| {
                smithay::desktop::layer_map_for_output(output)
                    .layers()
                    .any(|layer| belongs_to(layer.wl_surface()))
            })
    }

    fn windows_json(&self) -> Value {
        use smithay::wayland::{compositor::with_states, shell::xdg::XdgToplevelSurfaceData};
        let mut windows = self
            .window_ids
            .iter()
            .filter_map(|(window, id)| {
                let toplevel = window.toplevel()?;
                let (app_id, title) = with_states(toplevel.wl_surface(), |states| {
                    let attributes = states
                        .data_map
                        .get::<XdgToplevelSurfaceData>()
                        .unwrap()
                        .lock()
                        .unwrap();
                    (
                        attributes.app_id.clone().unwrap_or_default(),
                        attributes.title.clone().unwrap_or_default(),
                    )
                });
                let rect = self.visual_rect_for_window(window);
                Some(json!({
                    "id": id.0,
                    "app_id": app_id,
                    "title": title,
                    "focused": self.focused_window == Some(*id),
                    "mapped": self.space.element_location(window).is_some(),
                    "workspace": self.workspaces.workspace_for_window(*id).map(|workspace| workspace.0),
                    "x": rect.as_ref().map(|rect| rect.loc.x),
                    "y": rect.as_ref().map(|rect| rect.loc.y),
                    "capture_width": window.geometry().size.w,
                    "capture_height": window.geometry().size.h,
                    "width": rect.as_ref().map(|rect| rect.size.w),
                    "height": rect.as_ref().map(|rect| rect.size.h),
                }))
            })
            .collect::<Vec<_>>();
        windows.sort_by_key(|window| window["id"].as_u64());
        Value::Array(windows)
    }

    fn workspaces_json(&self) -> Value {
        let active = self.workspaces.active_id();
        let workspaces = self
            .workspaces
            .ordered()
            .into_iter()
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

fn geometry_arg(args: &Value) -> Result<Geometry, String> {
    match args.get("geometry") {
        None | Some(Value::Null) => Ok(Geometry::All),
        Some(value) => {
            let text = value
                .as_str()
                .ok_or("geometry must be a string like \"x,y WxH\"")?;
            parse_geometry(text)
        }
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
        let worker = thread::spawn(move || serve_connection(server, sender, signal, 1));
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
    fn failed_screenshot_delivery_removes_only_its_staging_file() {
        let root = unique_test_directory("undelivered-screenshot");
        fs::create_dir(&root).unwrap();
        let path = root.join("capture.png");
        fs::write(&path, b"capture").unwrap();
        let response = Response::success(1, serde_json::json!({"path": path}));

        discard_undelivered_screenshot(&response);
        assert!(!path.exists());
        discard_undelivered_screenshot(&response);
        fs::remove_dir(root).unwrap();
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
