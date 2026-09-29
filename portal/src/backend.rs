//! ScreenCast portal backend. Only the portal frontend may create sessions;
//! consent never survives a session, a process restart, or a capture failure.
use crate::{
    capture::{Capture, Source},
    picker::Prompt,
    stream::Ready,
};
use std::{
    collections::HashMap,
    process::Stdio,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
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
type Options = HashMap<String, OwnedValue>;
type Reply = (u32, Options);
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
const FRONTEND: &str = "org.freedesktop.portal.Desktop";
const PATH: &str = "/org/freedesktop/portal/desktop";
#[derive(Default)]
struct Cancel {
    stopped: AtomicBool,
    changed: Notify,
}
impl Cancel {
    fn stop(&self) {
        self.stopped.store(true, Ordering::SeqCst);
        self.changed.notify_one();
    }
    async fn wait(&self) {
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
    bar_controlled: bool,
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

async fn authorize(connection: &Connection, header: &Header<'_>) -> zbus::fdo::Result<String> {
    let sender = header
        .sender()
        .ok_or_else(|| zbus::fdo::Error::AccessDenied("Missing caller".into()))?;
    let dbus = zbus::fdo::DBusProxy::new(connection).await?;
    let owner = dbus.get_name_owner(FRONTEND.try_into().unwrap()).await?;
    if sender.as_str() != owner.as_str() {
        return Err(zbus::fdo::Error::AccessDenied(
            "Use the ScreenCast desktop portal".into(),
        ));
    }
    Ok(sender.to_string())
}
fn error(message: &str) -> zbus::fdo::Error {
    zbus::fdo::Error::InvalidArgs(message.into())
}
fn source_options(options: &Options) -> zbus::fdo::Result<(bool, bool)> {
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
    if types & 1 == 0 {
        return Err(error("Only monitor sharing is supported"));
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
    Ok((multiple, cursor == 2))
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
        3
    }
    #[zbus(property)]
    fn available_source_types(&self) -> u32 {
        1
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
    ) -> zbus::fdo::Result<Reply> {
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
                bar_controlled: false,
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
    ) -> zbus::fdo::Result<Reply> {
        let owner = authorize(connection, &header).await?;
        let settings = source_options(&options);
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
            Ok((multiple, cursor)) => {
                session.multiple = multiple;
                session.cursor = cursor;
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
        _parent_window: String,
        _options: Options,
        #[zbus(connection)] connection: &Connection,
        #[zbus(header)] header: Header<'_>,
    ) -> zbus::fdo::Result<Reply> {
        let owner = authorize(connection, &header).await?;
        if !handle
            .as_str()
            .starts_with("/org/freedesktop/portal/desktop/request/")
        {
            return Err(error("Invalid request path"));
        }
        let (multiple, cursor, bar_controlled, cancel) = {
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
                session.bar_controlled,
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
        let result = tokio::select! {
            _ = cancel.wait() => Err(StartError::Cancelled),
            result = tokio::time::timeout(Duration::from_secs(120), start_streams(if bar_controlled { "Ferese" } else { &app_id }, multiple, cursor)) => result.unwrap_or_else(|_| Err(StartError::Failed("Sharing request timed out".into()))),
        };
        let _ = connection
            .object_server()
            .remove::<Request, _>(handle)
            .await;
        match result {
            Ok((children, streams)) if !cancel.stopped.load(Ordering::SeqCst) => {
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
                let streams: Vec<(u32, Options)> = streams
                    .into_iter()
                    .map(|ready| {
                        (
                            ready.node,
                            HashMap::from([("source_type".into(), OwnedValue::from(1u32))]),
                        )
                    })
                    .collect();
                let mut reply = Options::new();
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
struct Request {
    cancel: Arc<Cancel>,
    owner: String,
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
fn child_command() -> Result<Command, String> {
    let mut command = Command::new(std::env::current_exe().map_err(|e| e.to_string())?);
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
    multiple: bool,
    cursor: bool,
) -> Result<(Vec<Child>, Vec<Ready>), StartError> {
    let sources = tokio::task::spawn_blocking(|| {
        Capture::connect(&AtomicBool::new(false)).map(|capture| capture.sources())
    })
    .await
    .map_err(|e| e.to_string())??;
    let prompt = Prompt {
        app: app.to_owned(),
        sources,
        multiple,
    };
    let selection = picker(&prompt)
        .await?
        .wait_with_output()
        .await
        .map_err(|e| e.to_string())?;
    if !selection.status.success() {
        return Err(StartError::Cancelled);
    }
    let names: Vec<String> =
        serde_json::from_slice(&selection.stdout).map_err(|_| StartError::Cancelled)?;
    let selected = validate_selection(&prompt.sources, &names, multiple)?;
    let mut children = Vec::new();
    let mut streams = Vec::new();
    for source in &selected {
        let mut child = child_command()?
            .args([
                "--stream",
                &source.name,
                if cursor { "embedded" } else { "hidden" },
            ])
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
        streams.push(ready);
        children.push(child);
    }
    Ok((children, streams))
}
fn validate_selection(
    sources: &[Source],
    names: &[String],
    multiple: bool,
) -> Result<Vec<Source>, String> {
    if names.is_empty() || names.len() > 4 || (!multiple && names.len() != 1) {
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
    let connection = zbus::connection::Builder::session()?
        .name("org.freedesktop.impl.portal.desktop.ferese")?
        .serve_at(PATH, backend.clone())?
        .serve_at(
            "/org/ferese/ScreenRecorder",
            RecorderControl(backend.clone()),
        )?
        .build()
        .await?;
    // Frontend death must revoke every stream, even if Session.Close never arrives.
    loop {
        tokio::time::sleep(Duration::from_secs(1)).await;
        if backend.sessions.lock().await.is_empty() {
            continue;
        }
        let owner = match zbus::fdo::DBusProxy::new(&connection).await {
            Ok(proxy) => proxy
                .get_name_owner(FRONTEND.try_into().unwrap())
                .await
                .ok()
                .map(|name| name.to_string()),
            Err(_) => None,
        };
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
        assert_eq!(source_options(&Options::new()).unwrap(), (false, false));
        for bits in [0u32, 2, 4, 6] {
            assert!(source_options(&HashMap::from([("types".into(), bits.into())])).is_err());
        }
        for bits in [1u32, 3, 5, 7] {
            assert_eq!(
                source_options(&HashMap::from([("types".into(), bits.into())])).unwrap(),
                (false, false)
            );
        }
        assert!(source_options(&HashMap::from([("cursor_mode".into(), 4u32.into())])).is_err());
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
