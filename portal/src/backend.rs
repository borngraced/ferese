//! ScreenCast portal backend. Only the portal frontend may create sessions;
//! saved monitor consent is restored only through the frontend permission store.
use crate::{
    capture::{Capture, Source},
    picker::Prompt,
    stream::Ready,
};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::{collections::HashMap, process::Stdio, time::Duration};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::{Child, Command},
    sync::{Mutex, Notify},
};
use zbus::{
    Connection,
    message::Header,
    object_server::SignalEmitter,
    zvariant::{OwnedObjectPath, OwnedValue, Value},
};

pub(crate) type Options = HashMap<String, OwnedValue>;
pub(crate) static CONFIG_TRANSACTION: std::sync::Mutex<()> = std::sync::Mutex::new(());
const FRONTEND: &str = "org.freedesktop.portal.Desktop";
const PATH: &str = "/org/freedesktop/portal/desktop";

#[derive(Debug)]
enum StartError {
    Cancelled,
    Failed(String),
}

impl From<String> for StartError {
    fn from(message: String) -> Self {
        Self::Failed(message)
    }
}

impl From<&str> for StartError {
    fn from(message: &str) -> Self {
        Self::Failed(message.into())
    }
}

#[derive(Default)]
pub(crate) struct Cancel {
    pub(crate) stopped: AtomicBool,
    changed: Notify,
}

impl Cancel {
    pub(crate) fn stop(&self) {
        self.stopped.store(true, Ordering::SeqCst);
        self.changed.notify_one();
    }

    pub(crate) async fn wait(&self) {
        if !self.stopped.load(Ordering::SeqCst) {
            self.changed.notified().await;
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Phase {
    Created,
    Selected,
    Starting,
    Running,
}

struct Session {
    owner: String,
    app: String,
    phase: Phase,
    multiple: bool,
    cursor: bool,
    source_types: u32,
    bar_controlled: bool,
    persist_mode: u32,
    restore: Option<crate::restore::Restore>,
    cancel: Arc<Cancel>,
}

#[derive(Clone, Default)]
pub struct Backend {
    sessions: Arc<Mutex<HashMap<String, Session>>>,
}

// This private interface only transfers stop controls to the shell's own recorder.
// The ordinary portal picker and all session revocation rules still apply.
struct RecorderControl(Backend);

#[zbus::interface(name = "org.ferese.ScreenRecorder")]
impl RecorderControl {
    async fn use_bar_controls(
        &self,
        session_handle: OwnedObjectPath,
        #[zbus(connection)] connection: &Connection,
        #[zbus(header)] header: Header<'_>,
    ) -> zbus::fdo::Result<()> {
        let sender = header
            .sender()
            .ok_or_else(|| error("Missing recorder caller"))?;
        if !owns_session(sender.as_str(), session_handle.as_str()) {
            return Err(zbus::fdo::Error::AccessDenied(
                "Not your recording session".into(),
            ));
        }
        let dbus = zbus::fdo::DBusProxy::new(connection).await?;
        let pid = dbus
            .get_connection_unix_process_id(sender.clone().into())
            .await?;
        if !shell_recorder(pid) {
            return Err(zbus::fdo::Error::AccessDenied(
                "The shell must own the recording controls".into(),
            ));
        }
        let mut sessions = self.0.sessions.lock().await;
        let session = sessions
            .get_mut(session_handle.as_str())
            .ok_or_else(|| error("Unknown session"))?;
        if session.phase != Phase::Created {
            return Err(error("Recording already configured"));
        }
        session.bar_controlled = true;
        Ok(())
    }
}

fn owns_session(sender: &str, path: &str) -> bool {
    let Some(sender) = sender.strip_prefix(':') else {
        return false;
    };
    path.starts_with(&format!("{PATH}/session/{}/", sender.replace('.', "_")))
}

fn shell_recorder(pid: u32) -> bool {
    use std::os::unix::fs::MetadataExt;
    let matches_binary = |pid: u32, name: &str| -> Option<bool> {
        let expected = std::env::current_exe().ok()?.parent()?.join(name);
        let expected = std::fs::metadata(expected).ok()?;
        let actual = std::fs::metadata(format!("/proc/{pid}/exe")).ok()?;
        Some(expected.dev() == actual.dev() && expected.ino() == actual.ino())
    };
    if matches_binary(pid, "ferese-record") != Some(true) {
        return false;
    }
    let parent = std::fs::read_to_string(format!("/proc/{pid}/status"))
        .ok()
        .and_then(|status| {
            status.lines().find_map(|line| {
                line.strip_prefix("PPid:")
                    .and_then(|s| s.trim().parse::<u32>().ok())
            })
        });
    parent.is_some_and(|pid| matches_binary(pid, "ferese-shell") == Some(true))
}

pub(crate) async fn authorize(
    connection: &Connection,
    header: &Header<'_>,
) -> zbus::fdo::Result<String> {
    let sender = header
        .sender()
        .ok_or_else(|| zbus::fdo::Error::AccessDenied("Missing caller".into()))?;
    let dbus = zbus::fdo::DBusProxy::new(connection).await?;
    let owner = dbus
        .get_name_owner(FRONTEND.try_into().unwrap())
        .await
        .map_err(|_| zbus::fdo::Error::AccessDenied("Desktop portal is unavailable".into()))?;
    if sender.as_str() != owner.as_str() {
        return Err(zbus::fdo::Error::AccessDenied(
            "Use the desktop portal".into(),
        ));
    }

    Ok(sender.to_string())
}

fn error(message: &str) -> zbus::fdo::Error {
    zbus::fdo::Error::InvalidArgs(message.into())
}

fn source_options(options: &Options) -> zbus::fdo::Result<(bool, bool, u32)> {
    let number = |key: &str, default| {
        options
            .get(key)
            .map(u32::try_from)
            .transpose()
            .map(|v| v.unwrap_or(default))
            .map_err(|_| error("Invalid option type"))
    };
    let types = number("types", 1)?;
    let cursor = number("cursor_mode", 1)?;
    if types == 0 || types & !3 != 0 {
        return Err(error("Unsupported source types"));
    }
    if cursor != 1 && cursor != 2 {
        return Err(error("Unsupported cursor mode"));
    }
    let multiple = options
        .get("multiple")
        .map(bool::try_from)
        .transpose()
        .map_err(|_| error("Invalid multiple option"))?
        .unwrap_or(false);

    Ok((multiple, cursor == 2, types))
}

impl Backend {
    async fn end(&self, connection: &Connection, path: &str) {
        let session = self.sessions.lock().await.remove(path);
        if let Some(session) = session {
            session.cancel.stop();
            if let Ok(emitter) = SignalEmitter::new(connection, path) {
                let _ = SessionObject::closed(&emitter).await;
            }
            // A Close method may still hold the interface read lock.
            let conn = connection.clone();
            let path = path.to_owned();
            tokio::spawn(async move {
                let _ = conn
                    .object_server()
                    .remove::<SessionObject, _>(path.as_str())
                    .await;
            });
        }
    }
}

#[zbus::interface(name = "org.freedesktop.impl.portal.ScreenCast")]
impl Backend {
    #[zbus(property, name = "version")]
    fn version(&self) -> u32 {
        4
    }

    #[zbus(property)]
    fn available_source_types(&self) -> u32 {
        3
    }

    #[zbus(property)]
    fn available_cursor_modes(&self) -> u32 {
        3
    }

    async fn create_session(
        &self,
        _handle: OwnedObjectPath,
        session_handle: OwnedObjectPath,
        app_id: String,
        _options: Options,
        #[zbus(connection)] connection: &Connection,
        #[zbus(header)] header: Header<'_>,
    ) -> zbus::fdo::Result<(u32, Options)> {
        let owner = authorize(connection, &header).await?;
        if !session_handle
            .as_str()
            .starts_with("/org/freedesktop/portal/desktop/session/")
            || app_id.len() > 512
        {
            return Err(error("Invalid session"));
        }
        let mut sessions = self.sessions.lock().await;
        if sessions.len() >= 8 || sessions.contains_key(session_handle.as_str()) {
            return Err(zbus::fdo::Error::LimitsExceeded(
                "Session limit reached or duplicate session".into(),
            ));
        }
        let inserted = connection
            .object_server()
            .at(
                session_handle.clone(),
                SessionObject {
                    backend: self.clone(),
                    path: session_handle.to_string(),
                    owner: owner.clone(),
                },
            )
            .await?;
        if !inserted {
            return Err(error("Session object already exists"));
        }
        sessions.insert(
            session_handle.to_string(),
            Session {
                owner,
                app: app_id,
                phase: Phase::Created,
                multiple: false,
                cursor: false,
                source_types: 1,
                bar_controlled: false,
                persist_mode: 0,
                restore: None,
                cancel: Arc::default(),
            },
        );
        Ok((0, Options::new()))
    }

    async fn select_sources(
        &self,
        _handle: OwnedObjectPath,
        session_handle: OwnedObjectPath,
        app_id: String,
        options: Options,
        #[zbus(connection)] connection: &Connection,
        #[zbus(header)] header: Header<'_>,
    ) -> zbus::fdo::Result<(u32, Options)> {
        let owner = authorize(connection, &header).await?;
        let settings = source_options(&options).and_then(|(multiple, cursor, types)| {
            Ok((
                multiple,
                cursor,
                types,
                crate::restore::persist_mode(&options)?,
            ))
        });
        let mut sessions = self.sessions.lock().await;
        let session = sessions
            .get_mut(session_handle.as_str())
            .ok_or_else(|| error("Unknown session"))?;

        if session.owner != owner
            || session.app != app_id
            || !matches!(session.phase, Phase::Created | Phase::Selected)
        {
            return Err(error("Invalid session state"));
        }

        match settings {
            Ok((multiple, cursor, types, persist_mode)) => {
                session.persist_mode = persist_mode;
                session.restore = crate::restore::Restore::read(&options);
                session.multiple = multiple;
                session.cursor = cursor;
                session.source_types = types;
                session.phase = Phase::Selected;
                Ok((0, Options::new()))
            }
            Err(e) => {
                drop(sessions);
                self.end(connection, session_handle.as_str()).await;
                Err(e)
            }
        }
    }

    #[allow(clippy::too_many_arguments)] // ScreenCast D-Bus method signature.
    async fn start(
        &self,
        handle: OwnedObjectPath,
        session_handle: OwnedObjectPath,
        app_id: String,
        parent_window: String,
        _options: Options,
        #[zbus(connection)] connection: &Connection,
        #[zbus(header)] header: Header<'_>,
    ) -> zbus::fdo::Result<(u32, Options)> {
        let owner = authorize(connection, &header).await?;

        if !handle
            .as_str()
            .starts_with("/org/freedesktop/portal/desktop/request/")
        {
            return Err(error("Invalid request path"));
        }

        let (multiple, cursor, source_types, bar_controlled, persist_mode, restore, cancel) = {
            let mut sessions = self.sessions.lock().await;
            let session = sessions
                .get_mut(session_handle.as_str())
                .ok_or_else(|| error("Unknown session"))?;

            if session.owner != owner || session.app != app_id || session.phase != Phase::Selected {
                return Err(error("Select sources before starting a session"));
            }

            session.phase = Phase::Starting;
            (
                session.multiple,
                session.cursor,
                session.source_types,
                session.bar_controlled,
                session.persist_mode,
                session.restore.clone(),
                session.cancel.clone(),
            )
        };
        let inserted = connection
            .object_server()
            .at(
                handle.clone(),
                Request {
                    cancel: cancel.clone(),
                    owner,
                },
            )
            .await;

        match inserted {
            Ok(true) => (),
            result => {
                self.end(connection, session_handle.as_str()).await;
                return Err(match result {
                    Err(e) => e.into(),
                    _ => error("Request object already exists"),
                });
            }
        }

        let sharing = start_streams(
            &app_id,
            &parent_window,
            multiple,
            cursor,
            source_types,
            persist_mode,
            restore,
            bar_controlled,
        );
        let result = tokio::select! {
            _ = cancel.wait() => Err(StartError::Cancelled),
            result = tokio::time::timeout(Duration::from_secs(120), sharing) => {
                result.unwrap_or_else(|_| Err(StartError::Failed("Sharing request timed out".into())))
            }
        };
        let _ = connection
            .object_server()
            .remove::<Request, _>(handle)
            .await;

        match result {
            Ok((children, streams, mut reply)) if !cancel.stopped.load(Ordering::SeqCst) => {
                if let Some(session) = self.sessions.lock().await.get_mut(session_handle.as_str()) {
                    session.phase = Phase::Running;
                }
                let backend = self.clone();
                let connection = connection.clone();
                let path = session_handle.to_string();

                tokio::spawn(async move {
                    supervise(children, cancel).await;
                    backend.end(&connection, &path).await;
                });
                reply.insert(
                    "streams".into(),
                    OwnedValue::try_from(Value::from(streams))
                        .map_err(|e| error(&e.to_string()))?,
                );

                Ok((0, reply))
            }
            result => {
                let code = match result {
                    Err(StartError::Failed(message)) => {
                        eprintln!("ferese portal: {message}");
                        2
                    }
                    _ => 1,
                };
                self.end(connection, session_handle.as_str()).await;
                Ok((code, Options::new()))
            }
        }
    }
}

pub(crate) struct Request {
    pub(crate) cancel: Arc<Cancel>,
    pub(crate) owner: String,
}

#[zbus::interface(name = "org.freedesktop.impl.portal.Request")]
impl Request {
    fn close(&self, #[zbus(header)] header: Header<'_>) -> zbus::fdo::Result<()> {
        if header.sender().map(|s| s.as_str()) != Some(self.owner.as_str()) {
            return Err(zbus::fdo::Error::AccessDenied("Wrong request owner".into()));
        }

        self.cancel.stop();
        Ok(())
    }
}

struct SessionObject {
    backend: Backend,
    path: String,
    owner: String,
}

#[zbus::interface(name = "org.freedesktop.impl.portal.Session")]
impl SessionObject {
    #[zbus(property, name = "version")]
    fn version(&self) -> u32 {
        1
    }

    async fn close(
        &self,
        #[zbus(connection)] connection: &Connection,
        #[zbus(header)] header: Header<'_>,
    ) -> zbus::fdo::Result<()> {
        if header.sender().map(|s| s.as_str()) != Some(self.owner.as_str()) {
            return Err(zbus::fdo::Error::AccessDenied("Wrong session owner".into()));
        }

        self.backend.end(connection, &self.path).await;
        Ok(())
    }

    #[zbus(signal)]
    async fn closed(emitter: &SignalEmitter<'_>) -> zbus::Result<()>;
}

// All helpers die with the service, including a picker awaiting user input.
pub(crate) fn child_command() -> Result<Command, String> {
    child_command_for(std::env::current_exe().map_err(|e| e.to_string())?)
}

pub(crate) fn child_command_for(program: impl AsRef<std::ffi::OsStr>) -> Result<Command, String> {
    let mut command = Command::new(program);
    let parent = std::process::id() as libc::pid_t;
    // SAFETY: only async-signal-safe libc calls run between fork and exec.
    unsafe {
        command.pre_exec(move || {
            if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            if libc::getppid() != parent {
                libc::_exit(1);
            }
            Ok(())
        });
    }

    command.kill_on_drop(true);
    Ok(command)
}

async fn picker(prompt: &Prompt) -> Result<Child, String> {
    let mut child = child_command()?
        .arg("--picker")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| e.to_string())?;
    let mut input = child.stdin.take().unwrap();
    input
        .write_all(&serde_json::to_vec(prompt).unwrap())
        .await
        .map_err(|e| e.to_string())?;
    drop(input);

    Ok(child)
}

async fn start_streams(
    app: &str,
    parent: &str,
    multiple: bool,
    cursor: bool,
    source_types: u32,
    requested_persistence: u32,
    restore: Option<crate::restore::Restore>,
    bar_controlled: bool,
) -> Result<(Vec<Child>, Vec<(u32, Options)>, Options), StartError> {
    let mut discovery = tokio::task::spawn_blocking(|| Capture::connect(&AtomicBool::new(false)))
        .await
        .map_err(|e| e.to_string())??;
    let mut sources = if source_types & 1 != 0 {
        discovery.sources()
    } else {
        Vec::new()
    };
    if source_types & 2 != 0 && discovery.supports_windows() {
        let windows = crate::desktop::ipc("get-windows", serde_json::json!({})).await?;
        sources.extend(crate::desktop::sharing_window_sources(&windows)?);
    }
    let outputs = crate::desktop::ipc("get-outputs", serde_json::json!({})).await?;
    let rememberable = sources
        .iter()
        .filter(|source| {
            crate::restore::Restore::create(app, cursor, std::slice::from_ref(*source), &outputs, 1)
                .is_some()
        })
        .map(|source| source.name.clone())
        .collect::<Vec<_>>();
    let restored = restore.and_then(|restore| {
        let selected = restore.resolve(app, cursor, multiple, &sources, &outputs)?;
        let selected = validate_selection(
            &sources,
            &selected
                .iter()
                .map(|source| source.name.clone())
                .collect::<Vec<_>>(),
            multiple,
        )
        .ok()?;
        Some((selected, restore.effective_mode(requested_persistence)))
    });
    let (selected, persist_mode) = if let Some(restored) = restored {
        restored
    } else {
        let prompt = Prompt {
            app: if bar_controlled { "Ferese" } else { app }.to_owned(),
            sources,
            multiple,
            rememberable: rememberable.clone(),
            parent: parent.into(),
            window_capture: false,
            persist_mode: if rememberable.is_empty() {
                0
            } else {
                requested_persistence
            },
        };
        let picker = picker(&prompt).await?;
        let picker_pid = picker.id().ok_or("Missing picker process ID")?;
        let selection = picker.wait_with_output().await.map_err(|e| e.to_string())?;
        if !selection.status.success() {
            return Err(StartError::Cancelled);
        }
        let selection: crate::picker::Selection =
            serde_json::from_slice(&selection.stdout).map_err(|_| StartError::Cancelled)?;
        if selection.persist_mode > prompt.persist_mode {
            return Err("Invalid persistence selection".into());
        }
        let selected = validate_selection(&prompt.sources, &selection.names, multiple)?;
        crate::desktop::wait_for_surface_removal(picker_pid).await?;

        (selected, selection.persist_mode)
    };

    let mut reply = Options::new();
    let fresh_outputs = crate::desktop::ipc("get-outputs", serde_json::json!({})).await?;

    let monitors = selected
        .iter()
        .filter(|source| source.window_id().is_none())
        .cloned()
        .collect::<Vec<_>>();
    if !crate::restore::selection_unchanged(&monitors, &outputs, &fresh_outputs) {
        return Err("A selected display changed while approval was pending".into());
    }

    let approved = crate::restore::Restore::create(app, cursor, &selected, &outputs, persist_mode);
    let effective_persistence = if let Some(approved) = approved.filter(|_| persist_mode > 0) {
        reply.insert("restore_data".into(), approved.value()?);
        persist_mode
    } else {
        0
    };
    reply.insert("persist_mode".into(), effective_persistence.into());

    let generations = monitors
        .iter()
        .map(|source| {
            discovery
                .generation(&source.name)
                .map(|generation| (source.name.clone(), generation))
                .ok_or("A selected display disconnected before startup")
        })
        .collect::<Result<Vec<_>, _>>()?;
    let mut children = Vec::new();
    let mut streams = Vec::new();

    for source in &selected {
        let mut command = child_command()?;
        if let Some(id) = source.window_id() {
            command.args([
                "--stream-window",
                &id.to_string(),
                if cursor { "embedded" } else { "hidden" },
            ]);
        } else {
            let generation = generations
                .iter()
                .find(|(name, _)| name == &source.name)
                .ok_or("Missing display generation")?
                .1;
            command.args([
                "--stream",
                &source.name,
                if cursor { "embedded" } else { "hidden" },
                &generation.to_string(),
            ]);
        }
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| e.to_string())?;
        let mut line = String::new();
        let output = child.stdout.take().unwrap();
        tokio::time::timeout(
            Duration::from_secs(10),
            BufReader::new(output).read_line(&mut line),
        )
        .await
        .map_err(|_| "PipeWire stream startup timed out")?
        .map_err(|e| e.to_string())?;
        let ready: Ready =
            serde_json::from_str(&line).map_err(|_| "PipeWire stream could not start")?;

        if ready.node == u32::MAX {
            return Err("Invalid PipeWire node".into());
        }

        streams.push((source.name.clone(), ready));
        children.push(child);
    }

    tokio::task::spawn_blocking(move || discovery.validate_sources(&generations))
        .await
        .map_err(|error| error.to_string())??;

    let final_outputs = crate::desktop::ipc("get-outputs", serde_json::json!({})).await?;
    if !crate::restore::selection_unchanged(&monitors, &outputs, &final_outputs) {
        return Err("A selected display changed during stream startup".into());
    }

    if children
        .iter_mut()
        .any(|child| !matches!(child.try_wait(), Ok(None)))
    {
        return Err("A selected stream ended during startup".into());
    }

    let final_windows = crate::desktop::ipc("get-windows", serde_json::json!({})).await?;
    let streams = streams
        .into_iter()
        .map(|(name, ready)| {
            let metadata = if name.starts_with("window:") {
                let source = selected
                    .iter()
                    .find(|source| source.name == name)
                    .ok_or("Unknown window stream")?;
                let id = source.window_id().ok_or("Invalid selected window")?;
                if !final_windows.as_array().is_some_and(|windows| {
                    windows
                        .iter()
                        .any(|window| window["id"].as_u64() == Some(id))
                }) {
                    return Err("A selected window closed during startup".to_owned());
                }
                window_metadata(source, &ready)
            } else {
                monitor_metadata(&final_outputs, &name, &ready)
            };
            metadata.map(|metadata| (ready.node, metadata))
        })
        .collect::<Result<Vec<_>, _>>()?;

    Ok((children, streams, reply))
}

fn window_metadata(source: &Source, ready: &Ready) -> Result<Options, String> {
    if source.window_id().is_none() || ready.width == 0 || ready.height == 0 {
        return Err("Invalid window stream".into());
    }
    let (width, height) = ready
        .logical_size
        .ok_or("Missing captured window geometry")?;
    let size = (
        i32::try_from(width).map_err(|_| "Invalid window width")?,
        i32::try_from(height).map_err(|_| "Invalid window height")?,
    );
    if size.0 <= 0 || size.1 <= 0 {
        return Err("Invalid captured window geometry".into());
    }
    Ok(Options::from([
        ("source_type".into(), 2u32.into()),
        (
            "size".into(),
            Value::from(size)
                .try_to_owned()
                .map_err(|e| e.to_string())?,
        ),
    ]))
}

fn monitor_metadata(
    outputs: &serde_json::Value,
    name: &str,
    ready: &Ready,
) -> Result<Options, String> {
    let output = outputs
        .as_array()
        .and_then(|outputs| {
            outputs
                .iter()
                .find(|output| output["name"].as_str() == Some(name))
        })
        .ok_or("Selected monitor is unavailable")?;
    let integer = |key: &str| {
        output[key]
            .as_i64()
            .and_then(|value| i32::try_from(value).ok())
            .ok_or_else(|| format!("Invalid output {key}"))
    };
    let position = (integer("x")?, integer("y")?);
    let size = (integer("width")?, integer("height")?);
    if size.0 <= 0 || size.1 <= 0 || output["enabled"] != true {
        return Err("Selected monitor is disabled".into());
    }
    let mode = &output["current_mode"];
    let mut physical = (
        mode["width"]
            .as_u64()
            .and_then(|value| u32::try_from(value).ok())
            .ok_or("Invalid physical output width")?,
        mode["height"]
            .as_u64()
            .and_then(|value| u32::try_from(value).ok())
            .ok_or("Invalid physical output height")?,
    );

    if matches!(
        output["transform"].as_str(),
        Some("rotate_90" | "rotate_270" | "flipped_90" | "flipped_270")
    ) {
        physical = (physical.1, physical.0);
    }

    if (ready.width, ready.height) != physical {
        return Err("Selected monitor changed mode during startup".into());
    }

    Ok(HashMap::from([
        ("source_type".into(), 1u32.into()),
        (
            "position".into(),
            Value::from(position)
                .try_to_owned()
                .map_err(|error| error.to_string())?,
        ),
        (
            "size".into(),
            Value::from(size)
                .try_to_owned()
                .map_err(|error| error.to_string())?,
        ),
    ]))
}

fn validate_selection(
    sources: &[Source],
    names: &[String],
    multiple: bool,
) -> Result<Vec<Source>, String> {
    if names.is_empty()
        || names.len() > 8
        || names.len() > sources.len()
        || (!multiple && names.len() != 1)
    {
        return Err("Invalid display selection".into());
    }

    let mut selected = Vec::new();
    for name in names {
        if selected.iter().any(|s: &Source| &s.name == name) {
            return Err("Duplicate display selection".into());
        }
        selected.push(
            sources
                .iter()
                .find(|s| &s.name == name)
                .ok_or("Unknown display selection")?
                .clone(),
        );
    }

    Ok(selected)
}

async fn supervise(mut children: Vec<Child>, cancel: Arc<Cancel>) {
    loop {
        if cancel.stopped.load(Ordering::SeqCst)
            || children
                .iter_mut()
                .any(|c| !matches!(c.try_wait(), Ok(None)))
        {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    cancel.stop();

    for child in &mut children {
        let _ = child.kill().await;
    }
}

pub async fn run() -> Result<(), Box<dyn std::error::Error>> {
    let backend = Backend::default();
    let settings = crate::settings::Settings::new();
    let requests = crate::desktop::Requests::default();
    let background = crate::permissions::Background::new(requests.clone());
    let lockdown = crate::lockdown::Lockdown::new();
    let shortcuts = crate::shortcuts::GlobalShortcuts::new(requests.clone());
    let inhibit = crate::inhibit::Inhibit::default();
    let connection = zbus::connection::Builder::session()?
        .name("org.freedesktop.impl.portal.desktop.ferese")?
        .serve_at(PATH, backend.clone())?
        .serve_at(PATH, settings.clone())?
        .serve_at(PATH, crate::desktop::Screenshot(requests.clone()))?
        .serve_at(PATH, crate::desktop::Wallpaper(requests.clone()))?
        .serve_at(PATH, crate::permissions::Usb(requests.clone()))?
        .serve_at(PATH, background.clone())?
        .serve_at(PATH, lockdown.clone())?
        .serve_at(PATH, shortcuts.clone())?
        .serve_at(PATH, inhibit.clone())?
        .serve_at(
            "/org/ferese/ScreenRecorder",
            RecorderControl(backend.clone()),
        )?
        .build()
        .await?;

    tokio::spawn(settings.watch(connection.clone()));
    tokio::spawn(background.watch(connection.clone()));
    tokio::spawn(lockdown.watch(connection.clone()));
    tokio::spawn(inhibit.clone().watch(connection.clone()));
    tokio::spawn(inhibit.clone().watch_logind());

    // Frontend death must revoke every stream, even if Session.Close never arrives.
    loop {
        tokio::time::sleep(Duration::from_secs(1)).await;
        let owner = match zbus::fdo::DBusProxy::new(&connection).await {
            Ok(proxy) => proxy
                .get_name_owner(FRONTEND.try_into().unwrap())
                .await
                .ok()
                .map(|name| name.to_string()),
            Err(_) => None,
        };
        requests.revoke_stale(owner.as_deref()).await;
        shortcuts.revoke_stale(&connection, owner.as_deref()).await;
        inhibit.revoke_stale(&connection, owner.as_deref()).await;

        let stale: Vec<_> = backend
            .sessions
            .lock()
            .await
            .iter()
            .filter(|(_, session)| owner.as_deref() != Some(session.owner.as_str()))
            .map(|(path, _)| path.clone())
            .collect();
        for path in stale {
            backend.end(&connection, &path).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bar_control_claims_are_bound_to_the_recorder_connection() {
        assert!(owns_session(
            ":1.42",
            "/org/freedesktop/portal/desktop/session/1_42/record"
        ));
        assert!(!owns_session(
            ":1.4",
            "/org/freedesktop/portal/desktop/session/1_42/record"
        ));
        assert!(!owns_session(
            ":1.43",
            "/org/freedesktop/portal/desktop/session/1_42/record"
        ));
        assert!(!shell_recorder(std::process::id()));
    }

    #[test]
    fn rejects_unimplemented_sources_and_cursor_modes() {
        assert_eq!(source_options(&Options::new()).unwrap(), (false, false, 1));
        for bits in [0u32, 4, 5, 6, 7] {
            assert!(source_options(&HashMap::from([("types".into(), bits.into())])).is_err());
        }
        for bits in [1u32, 2, 3] {
            assert_eq!(
                source_options(&HashMap::from([("types".into(), bits.into())])).unwrap(),
                (false, false, bits)
            );
        }
        assert!(source_options(&HashMap::from([("cursor_mode".into(), 4u32.into())])).is_err());
    }

    #[test]
    fn window_metadata_uses_captured_logical_geometry_and_omits_position() {
        let source = Source {
            name: "window:17".into(),
            label: "Editor".into(),
            width: 640,
            height: 480,
            x: 0,
            y: 0,
            scale: 1,
        };
        let ready = Ready {
            node: 1,
            width: 1200,
            height: 800,
            logical_size: Some((600, 400)),
        };
        let metadata = window_metadata(&source, &ready).unwrap();
        assert_eq!(u32::try_from(&metadata["source_type"]).unwrap(), 2);
        assert_eq!(
            <(i32, i32)>::try_from(metadata["size"].try_clone().unwrap()).unwrap(),
            (600, 400)
        );
        assert!(!metadata.contains_key("position"));
        assert!(
            window_metadata(
                &source,
                &Ready {
                    logical_size: None,
                    ..ready
                }
            )
            .is_err()
        );
    }

    #[test]
    fn multiple_selection_is_bounded_and_accepts_displays_with_windows() {
        let sources = (0..9)
            .map(|id| Source {
                name: format!("window:{id}"),
                label: "Editor".into(),
                width: 640,
                height: 480,
                x: 0,
                y: 0,
                scale: 1,
            })
            .collect::<Vec<_>>();
        let names = sources
            .iter()
            .map(|source| source.name.clone())
            .collect::<Vec<_>>();
        assert!(validate_selection(&sources, &names[..8], true).is_ok());
        assert!(validate_selection(&sources, &names, true).is_err());
        assert!(validate_selection(&sources, &names[..2], false).is_err());
        assert!(validate_selection(&sources, &[names[0].clone(), names[0].clone()], true).is_err());
    }

    #[test]
    fn monitor_metadata_uses_logical_fractional_scale_geometry() {
        let outputs = serde_json::json!([{"name":"display","enabled":true,"x":-1440,"y":20,"width":2560,"height":1440,"scale":1.5,"current_mode":{"width":3840,"height":2160}}]);
        let metadata = monitor_metadata(
            &outputs,
            "display",
            &Ready {
                logical_size: None,
                node: 1,
                width: 3840,
                height: 2160,
            },
        )
        .unwrap();
        assert_eq!(
            <(i32, i32)>::try_from(metadata["position"].try_clone().unwrap()).unwrap(),
            (-1440, 20)
        );
        assert_eq!(
            <(i32, i32)>::try_from(metadata["size"].try_clone().unwrap()).unwrap(),
            (2560, 1440)
        );
        assert!(
            monitor_metadata(
                &outputs,
                "missing",
                &Ready {
                    logical_size: None,
                    node: 1,
                    width: 3840,
                    height: 2160
                }
            )
            .is_err()
        );
    }

    #[test]
    fn multiple_selection_can_include_more_than_four_displays() {
        let sources = (0..6)
            .map(|index| Source {
                name: format!("display{index}"),
                label: String::new(),
                width: 100,
                height: 100,
                x: 0,
                y: 0,
                scale: 1,
            })
            .collect::<Vec<_>>();
        let names = sources
            .iter()
            .map(|source| source.name.clone())
            .collect::<Vec<_>>();
        assert_eq!(validate_selection(&sources, &names, true).unwrap().len(), 6);
        assert!(validate_selection(&sources, &names, false).is_err());
    }

    #[test]
    fn rejects_empty_unknown_duplicate_and_excess_selections() {
        let source = Source {
            name: "test".into(),
            label: "Test".into(),
            width: 640,
            height: 480,
            x: 0,
            y: 0,
            scale: 1,
        };
        assert!(validate_selection(std::slice::from_ref(&source), &[], false).is_err());
        assert!(
            validate_selection(std::slice::from_ref(&source), &["other".into()], false).is_err()
        );
        assert!(
            validate_selection(
                std::slice::from_ref(&source),
                &["test".into(), "test".into()],
                true
            )
            .is_err()
        );
        assert_eq!(
            validate_selection(&[source], &["test".into()], false)
                .unwrap()
                .len(),
            1
        );
    }
}
