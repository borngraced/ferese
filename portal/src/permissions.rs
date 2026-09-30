use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::RwLock;
use zbus::Connection;
use zbus::message::Header;
use zbus::object_server::SignalEmitter;
use zbus::zvariant::{OwnedObjectPath, OwnedValue, Value};

use crate::backend::{Options, authorize};
use crate::desktop::{Requests, consent, ipc, reply};

const PATH: &str = "/org/freedesktop/portal/desktop";

#[derive(Clone)]
pub(crate) struct Usb(pub(crate) Requests);

#[zbus::interface(name = "org.freedesktop.impl.portal.Usb")]
impl Usb {
    #[zbus(property, name = "version")]
    fn version(&self) -> u32 {
        1
    }

    async fn acquire_devices(
        &self,
        handle: OwnedObjectPath,
        parent_window: String,
        app_id: String,
        devices: Vec<(String, Options, Options)>,
        _options: Options,
        #[zbus(connection)] connection: &Connection,
        #[zbus(header)] header: Header<'_>,
    ) -> zbus::fdo::Result<(u32, Options)> {
        let cancel = self.0.begin(connection, &header, &handle).await?;
        let result = tokio::select! {
            biased;
            _ = cancel.wait() => (1, Options::new()),
            result = usb_consent(&app_id, &parent_window, devices) => reply(result),
        };
        self.0.end(connection, &handle).await;
        Ok(result)
    }
}

fn usb_label(info: &Options, id: &str) -> String {
    let properties = info
        .get("properties")
        .and_then(|value| value.try_clone().ok())
        .and_then(|value| HashMap::<String, OwnedValue>::try_from(value).ok())
        .unwrap_or_default();
    let property = |keys: &[&str]| {
        keys.iter()
            .find_map(|key| properties.get(*key).and_then(|value| <&str>::try_from(value).ok()))
    };
    let vendor = property(&["ID_VENDOR_FROM_DATABASE", "ID_VENDOR", "ID_VENDOR_ID"]);
    let model = property(&["ID_MODEL_FROM_DATABASE", "ID_MODEL", "ID_MODEL_ID"]);
    let label = [vendor, model].into_iter().flatten().collect::<Vec<_>>().join(" ");
    let label = if label.is_empty() { id } else { &label };
    label
        .chars()
        .filter(|character| !character.is_control())
        .take(256)
        .collect()
}

async fn usb_consent(
    app: &str,
    parent: &str,
    devices: Vec<(String, Options, Options)>,
) -> Result<Option<Options>, String> {
    if devices.is_empty() || devices.len() > 32 {
        return Err("Invalid USB device count".into());
    }
    let mut ids = HashSet::new();
    let mut selected = Vec::new();
    let mut description = "Allow access to these USB devices?\n".to_owned();
    for (id, info, options) in devices {
        if id.is_empty() || id.len() > 512 || !ids.insert(id.clone()) {
            return Err("Invalid or duplicate USB device identifier".into());
        }
        let writable = options
            .get("writable")
            .map(bool::try_from)
            .transpose()
            .map_err(|_| "Invalid USB access mode")?
            .unwrap_or(false);
        let label = usb_label(&info, &id);
        description.push_str(&format!(
            "\n{} — {}",
            label,
            if writable { "read and write" } else { "read only" }
        ));
        selected.push((id, HashMap::from([("writable".to_owned(), OwnedValue::from(writable))])));
    }
    if !consent(app, parent, "USB device access", &description, "Allow access", None).await? {
        return Ok(None);
    }
    Ok(Some(HashMap::from([(
        "devices".into(),
        Value::from(selected)
            .try_to_owned()
            .map_err(|error| error.to_string())?,
    )])))
}

#[derive(Clone)]
pub(crate) struct Background {
    requests: Requests,
    states: Arc<RwLock<HashMap<String, u32>>>,
}

impl Background {
    pub(crate) fn new(requests: Requests) -> Self {
        Self {
            requests,
            states: Default::default(),
        }
    }

    pub(crate) async fn watch(self, connection: Connection) {
        let emitter = SignalEmitter::new(&connection, PATH).unwrap();
        loop {
            if let Ok(windows) = ipc("get-windows", serde_json::json!({})).await {
                let next = app_states(&windows);
                let mut previous = self.states.write().await;
                if *previous != next {
                    *previous = next;
                    drop(previous);
                    let _ = Self::running_applications_changed(&emitter).await;
                }
            }
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    }
}

#[zbus::interface(name = "org.freedesktop.impl.portal.Background")]
impl Background {
    async fn get_app_state(
        &self,
        #[zbus(connection)] connection: &Connection,
        #[zbus(header)] header: Header<'_>,
    ) -> zbus::fdo::Result<Options> {
        authorize(connection, &header).await?;
        Ok(self
            .states
            .read()
            .await
            .iter()
            .map(|(app, state)| (app.clone(), (*state).into()))
            .collect())
    }

    async fn notify_background(
        &self,
        handle: OwnedObjectPath,
        app_id: String,
        name: String,
        #[zbus(connection)] connection: &Connection,
        #[zbus(header)] header: Header<'_>,
    ) -> zbus::fdo::Result<(u32, Options)> {
        let cancel = self.requests.begin(connection, &header, &handle).await?;
        let result = tokio::select! {
            biased;
            _ = cancel.wait() => (1, Options::new()),
            result = background_consent(&app_id, &name) => reply(result),
        };
        self.requests.end(connection, &handle).await;
        Ok(result)
    }

    async fn enable_autostart(
        &self,
        app_id: String,
        enable: bool,
        commandline: Vec<String>,
        flags: u32,
        #[zbus(connection)] connection: &Connection,
        #[zbus(header)] header: Header<'_>,
    ) -> zbus::fdo::Result<bool> {
        authorize(connection, &header).await?;
        let directory = dirs::config_dir()
            .ok_or_else(|| zbus::fdo::Error::Failed("Missing config directory".into()))?
            .join("autostart");
        set_autostart(&directory, &app_id, enable, &commandline, flags).map_err(zbus::fdo::Error::InvalidArgs)
    }

    #[zbus(signal)]
    async fn running_applications_changed(emitter: &SignalEmitter<'_>) -> zbus::Result<()>;
}

async fn background_consent(app: &str, name: &str) -> Result<Option<Options>, String> {
    if name.len() > 512 {
        return Err("Background application name is too long".into());
    }
    let allow = consent(
        app,
        "",
        "Background activity",
        &format!("{name} wants to keep running after its windows close."),
        "Allow this time",
        None,
    )
    .await?;
    Ok(Some(HashMap::from([(
        "result".into(),
        OwnedValue::from(if allow { 2u32 } else { 0u32 }),
    )])))
}

fn app_states(windows: &serde_json::Value) -> HashMap<String, u32> {
    let mut apps: HashMap<String, u32> = HashMap::new();
    for window in windows.as_array().into_iter().flatten() {
        let Some(app) = window["app_id"].as_str().filter(|app| !app.is_empty()) else {
            continue;
        };
        let app = app.strip_suffix(".desktop").unwrap_or(app).to_owned();
        let state = if window["focused"] == true { 2 } else { 1 };
        apps.entry(app)
            .and_modify(|old| *old = (*old).max(state))
            .or_insert(state);
    }
    apps
}

fn set_autostart(directory: &Path, app: &str, enable: bool, argv: &[String], flags: u32) -> Result<bool, String> {
    if app.is_empty()
        || app.len() > 255
        || !app
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte))
        || app.starts_with('.')
        || flags & !1 != 0
    {
        return Err("Invalid autostart application or flags".into());
    }
    let path = directory.join(format!("{app}.desktop"));
    let previous = read_managed_entry(&path)?;
    if previous
        .as_ref()
        .is_some_and(|source| !source.lines().any(|line| line == "X-Ferese-Portal=true"))
    {
        return Err("Existing autostart entry is not managed by Ferese".into());
    }
    if !enable {
        match std::fs::remove_file(path) {
            Ok(()) => (),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
            Err(error) => return Err(error.to_string()),
        }
        return Ok(true);
    }
    if argv.len() > 128 || (argv.first().is_none_or(|argument| argument.is_empty()) && flags & 1 == 0) {
        return Err("Invalid autostart command".into());
    }
    let exec = argv
        .iter()
        .map(|argument| desktop_argument(argument))
        .collect::<Result<Vec<_>, _>>()?
        .join(" ");
    let entry = format!(
        "[Desktop Entry]\nType=Application\nName={app}\nExec={exec}\nDBusActivatable={}\nX-Ferese-Portal=true\n",
        flags & 1 != 0
    );
    std::fs::create_dir_all(directory).map_err(|error| error.to_string())?;
    let mut file = tempfile::NamedTempFile::new_in(directory).map_err(|error| error.to_string())?;
    std::io::Write::write_all(&mut file, entry.as_bytes()).map_err(|error| error.to_string())?;
    if read_managed_entry(&path)? != previous {
        return Err("Autostart entry changed; please try again".into());
    }
    file.persist(path).map_err(|error| error.to_string())?;
    Ok(true)
}

fn read_managed_entry(path: &Path) -> Result<Option<String>, String> {
    use std::io::Read;
    match std::fs::symlink_metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Ok(metadata) if metadata.is_file() && metadata.len() <= 1024 * 1024 => (),
        Ok(_) => return Err("Existing autostart entry is not a regular file".into()),
        Err(error) => return Err(error.to_string()),
    }
    let mut source = String::new();
    std::fs::File::open(path)
        .map_err(|error| error.to_string())?
        .take(1024 * 1024 + 1)
        .read_to_string(&mut source)
        .map_err(|error| error.to_string())?;
    if source.len() > 1024 * 1024 {
        return Err("Existing autostart entry is too large".into());
    }
    Ok(Some(source))
}

fn desktop_argument(argument: &str) -> Result<String, String> {
    if argument.len() > 4096 || argument.chars().any(|character| character.is_control()) {
        return Err("Invalid autostart argument".into());
    }
    let mut result = "\"".to_owned();
    for character in argument.chars() {
        match character {
            '%' => result.push_str("%%"),
            '\\' => result.push_str("\\\\\\\\"),
            '$' | '`' | '"' => {
                result.push_str("\\\\");
                result.push(character);
            }
            _ => result.push(character),
        }
    }
    result.push('"');
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn application_state_groups_windows_and_keeps_inactive_workspaces_running() {
        let windows = serde_json::json!([
            {"app_id":"org.test.App.desktop","focused":false,"mapped":false},
            {"app_id":"org.test.App","focused":true,"mapped":true},
            {"app_id":"org.test.Other","focused":false,"mapped":true},
            {"app_id":"","focused":false}
        ]);
        assert_eq!(
            app_states(&windows),
            HashMap::from([("org.test.App".into(), 2), ("org.test.Other".into(), 1)])
        );
        assert!(app_states(&serde_json::json!([])).is_empty());
    }

    #[test]
    fn usb_labels_use_vendor_and_model_from_udev_properties() {
        let properties = HashMap::from([
            (
                "ID_VENDOR_FROM_DATABASE".to_owned(),
                Value::from("Example Vendor").try_to_owned().unwrap(),
            ),
            ("ID_MODEL".to_owned(), Value::from("Gamepad").try_to_owned().unwrap()),
        ]);
        let info = HashMap::from([("properties".into(), Value::from(properties).try_to_owned().unwrap())]);
        assert_eq!(usb_label(&info, "opaque-id"), "Example Vendor Gamepad");
        assert_eq!(usb_label(&Options::new(), "fallback-id"), "fallback-id");
    }

    #[test]
    fn autostart_preserves_existing_links_directories_and_invalid_text() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("org.test.App.desktop");
        std::os::unix::fs::symlink("missing", &path).unwrap();
        assert!(set_autostart(dir.path(), "org.test.App", false, &[], 0).is_err());
        assert!(path.symlink_metadata().unwrap().file_type().is_symlink());
        std::fs::remove_file(&path).unwrap();
        std::fs::create_dir(&path).unwrap();
        assert!(set_autostart(dir.path(), "org.test.App", false, &[], 0).is_err());
        std::fs::remove_dir(&path).unwrap();
        std::fs::write(&path, [255u8]).unwrap();
        assert!(set_autostart(dir.path(), "org.test.App", false, &[], 0).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), [255]);
    }

    #[test]
    fn autostart_is_scoped_to_managed_files_and_literal_argv() {
        let dir = tempfile::tempdir().unwrap();
        let argv = vec!["program".into(), "a $x `y` 50% \\\"".into()];
        assert!(set_autostart(dir.path(), "org.test.App", true, &argv, 0).unwrap());
        let path = dir.path().join("org.test.App.desktop");
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("50%%"));
        assert!(text.contains("X-Ferese-Portal=true"));
        assert!(set_autostart(dir.path(), "org.test.App", false, &[], 0).unwrap());
        assert!(!path.exists());
        std::fs::write(&path, "[Desktop Entry]\nName=My custom file\n").unwrap();
        assert!(set_autostart(dir.path(), "org.test.App", true, &argv, 0).is_err());
        for app in ["../escape", ".hidden", "bad/name", "bad\nname"] {
            assert!(set_autostart(dir.path(), app, true, &argv, 0).is_err());
        }
        assert!(desktop_argument("bad\nargument").is_err());
    }
}
