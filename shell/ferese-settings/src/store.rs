use std::{
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant},
};
use toml_edit::{DocumentMut, Item, Value};

pub const LIMIT: usize = 60 * 1024;

#[derive(Clone, Debug)]
pub struct Snapshot {
    pub source: String,
    pub doc: DocumentMut,
}
impl Snapshot {
    pub fn parse(source: String) -> Result<Self, String> {
        if source.len() > LIMIT {
            return Err("Configuration exceeds the 60 KiB limit.".into());
        }
        let doc = source.parse::<DocumentMut>().map_err(|e| e.to_string())?;
        Ok(Self { source, doc })
    }
    pub fn read(path: &Path) -> Result<Self, String> {
        let source = match fs::File::open(path) {
            Ok(file) => {
                let mut source = String::new();
                file.take((LIMIT + 1) as u64)
                    .read_to_string(&mut source)
                    .map_err(|e| e.to_string())?;
                source
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(e) => return Err(e.to_string()),
        };
        Self::parse(source)
    }
    pub fn item(&self, path: &str) -> Option<&Item> {
        let mut item = self.doc.as_item();
        for key in path.split('.') {
            item = if let Ok(index) = key.parse::<usize>() {
                item.get(index)?
            } else {
                item.get(key)?
            };
        }
        Some(item)
    }
    pub fn string(&self, path: &str, fallback: &str) -> String {
        self.item(path)
            .and_then(Item::as_str)
            .unwrap_or(fallback)
            .to_owned()
    }
    pub fn number(&self, path: &str, fallback: f64) -> f64 {
        self.item(path)
            .and_then(|v| v.as_float().or_else(|| v.as_integer().map(|v| v as f64)))
            .unwrap_or(fallback)
    }
    pub fn boolean(&self, path: &str, fallback: bool) -> bool {
        self.item(path).and_then(Item::as_bool).unwrap_or(fallback)
    }
    pub fn records(&self, path: &str) -> usize {
        self.item(path)
            .and_then(Item::as_array_of_tables)
            .map_or(0, |a| a.len())
    }
    pub fn argv(&self, path: &str) -> String {
        self.item(path)
            .and_then(Item::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(Value::as_str)
                    .map(|s| {
                        shlex::try_quote(s)
                            .map(|s| s.into_owned())
                            .unwrap_or_else(|_| s.to_owned())
                    })
                    .collect::<Vec<_>>()
                    .join(" ")
            })
            .unwrap_or_default()
    }
    pub fn edit(&mut self, edit: &Edit) -> Result<(), String> {
        match edit {
            Edit::Set(path, value) => {
                let mut item = self.doc.as_item_mut();
                for key in path.split('.') {
                    if let Ok(index) = key.parse::<usize>() {
                        item = item
                            .get_mut(index)
                            .ok_or("This item no longer exists. Reload settings.")?;
                    } else {
                        if item.is_none() {
                            *item = Item::Table(toml_edit::Table::new());
                        }
                        let table = item
                            .as_table_like_mut()
                            .ok_or("This setting is not a table.")?;
                        item = table.entry(key).or_insert(Item::None);
                    }
                }
                let mut value = value.clone();
                if let Some(previous) = item.as_value() {
                    *value.decor_mut() = previous.decor().clone();
                }
                *item = Item::Value(value);
            }
            Edit::Add(table, fields) => {
                let item = nested_item_mut(self.doc.as_item_mut(), table)?;
                if item.is_none() {
                    *item = Item::ArrayOfTables(Default::default());
                }
                let array = item
                    .as_array_of_tables_mut()
                    .ok_or("Expected a list of settings.")?;
                let mut record = toml_edit::Table::new();
                for (key, value) in fields {
                    record[key] = Item::Value(value.clone());
                }
                array.push(record);
            }
            Edit::Remove(table, index) => {
                let array = nested_item_mut(self.doc.as_item_mut(), table)?
                    .as_array_of_tables_mut()
                    .ok_or("This list no longer exists.")?;
                if *index >= array.len() {
                    return Err("This item no longer exists. Reload settings.".into());
                }
                array.remove(*index);
            }
        }
        self.source = self.doc.to_string();
        Ok(())
    }
}

fn nested_item_mut<'a>(mut item: &'a mut Item, path: &str) -> Result<&'a mut Item, String> {
    for key in path.split('.') {
        if item.is_none() {
            *item = Item::Table(toml_edit::Table::new());
        }
        let table = item
            .as_table_like_mut()
            .ok_or("Expected a settings table.")?;
        item = table.entry(key).or_insert(Item::None);
    }
    Ok(item)
}

#[derive(Clone, Debug)]
pub enum Edit {
    Set(String, Value),
    Add(String, Vec<(String, Value)>),
    Remove(String, usize),
}
pub fn set(path: &str, value: impl Into<Value>) -> Edit {
    Edit::Set(path.to_owned(), value.into())
}

pub fn config_path() -> PathBuf {
    let directory = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
        .unwrap_or_else(|| PathBuf::from("."));
    let path = directory.join("ferese/config.toml");
    path.canonicalize().unwrap_or(path)
}

pub fn sibling(name: &str) -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|p| p.join(name)))
        .filter(|p| p.is_file())
        .unwrap_or_else(|| name.into())
}

fn validate(path: &Path) -> Result<(), String> {
    let errors = tempfile::NamedTempFile::new().map_err(|e| e.to_string())?;
    let mut child = Command::new(sibling("ferese"))
        .arg("--check-config")
        .arg(path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(errors.reopen().map_err(|e| e.to_string())?)
        .spawn()
        .map_err(|e| format!("Cannot validate configuration: {e}"))?;
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(status) = child.try_wait().map_err(|e| e.to_string())? {
            return if status.success() {
                Ok(())
            } else {
                Err(fs::read_to_string(errors.path())
                    .unwrap_or_else(|_| "Configuration was rejected.".into())
                    .chars()
                    .take(700)
                    .collect())
            };
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err("Configuration validation timed out.".into());
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

// Compare-and-replace refuses stale drafts. A second check after validation also
// catches edits made while the validation process was running.
pub fn save(path: &Path, expected: &str, desired: &str) -> Result<Snapshot, String> {
    save_with(path, expected, desired, validate)
}
fn save_with(
    path: &Path,
    expected: &str,
    desired: &str,
    validate: impl FnOnce(&Path) -> Result<(), String>,
) -> Result<Snapshot, String> {
    let snapshot = Snapshot::parse(desired.to_owned())?;
    let unchanged = || -> Result<(), String> {
        if Snapshot::read(path)?.source != expected {
            return Err(
                "Your config changed in another app. Reload settings before saving.".into(),
            );
        }
        Ok(())
    };
    unchanged()?;
    let parent = path.parent().ok_or("Invalid configuration path.")?;
    fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    let mut staged = tempfile::NamedTempFile::new_in(parent).map_err(|e| e.to_string())?;
    if let Ok(metadata) = fs::metadata(path) {
        staged
            .as_file()
            .set_permissions(metadata.permissions())
            .map_err(|e| e.to_string())?;
    }
    staged
        .write_all(desired.as_bytes())
        .map_err(|e| e.to_string())?;
    staged.as_file().sync_all().map_err(|e| e.to_string())?;
    validate(staged.path())?;
    unchanged()?;
    // Keep the last working source, including comments and unknown keys.
    let mut backup = tempfile::NamedTempFile::new_in(parent).map_err(|e| e.to_string())?;
    backup
        .write_all(expected.as_bytes())
        .map_err(|e| e.to_string())?;
    backup.as_file().sync_all().map_err(|e| e.to_string())?;
    backup
        .persist(path.with_extension("toml.settings-backup"))
        .map_err(|e| e.to_string())?;
    staged.persist(path).map_err(|e| e.to_string())?;
    Ok(snapshot)
}

pub fn reload_running() -> bool {
    use ferese_ipc::{Request, Response, VERSION, read_frame, write_frame};
    let Some(runtime) = std::env::var_os("XDG_RUNTIME_DIR") else {
        return false;
    };
    let Ok(mut socket) =
        std::os::unix::net::UnixStream::connect(PathBuf::from(runtime).join("ferese/control.sock"))
    else {
        return false;
    };
    let _ = socket.set_read_timeout(Some(Duration::from_secs(2)));
    let _ = socket.set_write_timeout(Some(Duration::from_secs(2)));
    let request = Request {
        version: VERSION,
        id: 1,
        kind: "command".into(),
        command: "reload-config".into(),
        args: serde_json::json!({}),
    };
    write_frame(&mut socket, &request).is_ok()
        && read_frame::<Response>(&mut socket).is_ok_and(|response| response.error.is_none())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn edits_preserve_comments_custom_keys_and_other_rules() {
        let source = "# personal theme\n[animations]\nspeed = 0.75 # keep this\n[custom]\nfuture = true\n[[window_rules]]\napp_id = 'mine'\nfloating = true\n";
        let mut snapshot = Snapshot::parse(source.into()).unwrap();
        snapshot.edit(&set("animations.speed", 0.8)).unwrap();
        assert!(snapshot.source.contains("# keep this"));
        assert!(snapshot.source.contains("# personal theme"));
        assert_eq!(snapshot.string("window_rules.0.app_id", ""), "mine");
        assert!(snapshot.boolean("custom.future", false));
    }
    #[test]
    fn rejects_conflicts_and_failed_validation_without_overwriting() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        fs::write(&path, "# external edit\n").unwrap();
        assert!(save_with(&path, "", "[animations]\nspeed=1.0", |_| Ok(())).is_err());
        assert!(
            save_with(
                &path,
                "# external edit\n",
                "[animations]\nspeed=1.0",
                |_| Err("invalid".into())
            )
            .is_err()
        );
        assert_eq!(fs::read_to_string(&path).unwrap(), "# external edit\n");
    }
    #[test]
    fn catches_edits_during_validation_and_keeps_a_backup_on_success() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        fs::write(&path, "# initial\n").unwrap();
        assert!(
            save_with(&path, "# initial\n", "# desired\n", |_| {
                fs::write(&path, "# another editor\n").unwrap();
                Ok(())
            })
            .is_err()
        );
        save_with(&path, "# another editor\n", "# desired\n", |_| Ok(())).unwrap();
        assert_eq!(
            fs::read_to_string(path.with_extension("toml.settings-backup")).unwrap(),
            "# another editor\n"
        );
        assert_eq!(fs::read_to_string(path).unwrap(), "# desired\n");
    }
    #[test]
    fn nested_array_edits_and_login_items_roundtrip() {
        let mut snapshot = Snapshot::parse("[[output_profiles]]\nname='desk'\n[[output_profiles.outputs]]\nmatch='HDMI-A-1'\nscale=1.5\n".into()).unwrap();
        snapshot
            .edit(&set("output_profiles.0.outputs.0.scale", 1.75))
            .unwrap();
        assert_eq!(
            snapshot.number("output_profiles.0.outputs.0.scale", 1.),
            1.75
        );
        snapshot
            .edit(&Edit::Add(
                "autostart".into(),
                vec![
                    (
                        "command".into(),
                        ["program", "argument with space"]
                            .into_iter()
                            .collect::<toml_edit::Array>()
                            .into(),
                    ),
                    ("enabled".into(), false.into()),
                ],
            ))
            .unwrap();
        assert_eq!(
            shlex::split(&snapshot.argv("autostart.0.command")).unwrap(),
            vec!["program", "argument with space"]
        );
        assert!(!snapshot.boolean("autostart.0.enabled", true));
        snapshot.edit(&Edit::Remove("autostart".into(), 0)).unwrap();
        assert_eq!(snapshot.records("autostart"), 0);
    }
    #[test]
    fn nested_note_lists_keep_existing_clock_settings() {
        let mut snapshot =
            Snapshot::parse("[desktop_widgets.clock]\nenabled = true\n".into()).unwrap();
        snapshot
            .edit(&Edit::Add(
                "desktop_widgets.notes".into(),
                vec![
                    ("id".into(), "first".into()),
                    ("text".into(), "one\ntwo".into()),
                ],
            ))
            .unwrap();
        assert_eq!(snapshot.records("desktop_widgets.notes"), 1);
        assert!(snapshot.boolean("desktop_widgets.clock.enabled", false));
        assert_eq!(
            snapshot.string("desktop_widgets.notes.0.text", ""),
            "one\ntwo"
        );
        snapshot
            .edit(&Edit::Remove("desktop_widgets.notes".into(), 0))
            .unwrap();
        assert_eq!(snapshot.records("desktop_widgets.notes"), 0);
    }

    #[test]
    #[ignore = "requires a built compositor; run with FERESE_TEST_BINARY=target/debug/ferese"]
    fn all_gui_controls_and_presets_pass_compositor_validation() {
        let binary = std::env::var_os("FERESE_TEST_BINARY").expect("Set FERESE_TEST_BINARY");
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.toml");
        let validate = |snapshot: &Snapshot| {
            fs::write(&path, &snapshot.source).unwrap();
            let result = Command::new(&binary)
                .arg("--check-config")
                .arg(&path)
                .output()
                .unwrap();
            assert!(
                result.status.success(),
                "{}\n{}",
                snapshot.source,
                String::from_utf8_lossy(&result.stderr)
            );
        };
        for page in crate::schema::Page::ALL {
            let mut fields = crate::schema::fields(page);
            if page == crate::schema::Page::Desktop {
                fields.extend(crate::schema::note_fields(0));
            }
            for field in fields {
                use crate::schema::Kind;
                let values: Vec<Value> = match field.kind {
                    Kind::Toggle(_) => vec![true.into(), false.into()],
                    Kind::Range {
                        min, max, integer, ..
                    } => {
                        if integer {
                            vec![(min as i64).into(), (max as i64).into()]
                        } else {
                            vec![min.into(), max.into()]
                        }
                    }
                    Kind::Text { default, .. } => vec![default.into()],
                    Kind::Choice { choices, .. } => {
                        choices.iter().map(|(v, _)| (*v).into()).collect()
                    }
                };
                for value in values {
                    let mut snapshot = Snapshot::parse(String::new()).unwrap();
                    if field.path.starts_with("desktop_widgets.notes.") {
                        snapshot
                            .edit(&Edit::Add(
                                "desktop_widgets.notes".into(),
                                vec![("id".into(), "test".into())],
                            ))
                            .unwrap();
                    }
                    snapshot.edit(&set(&field.path, value)).unwrap();
                    validate(&snapshot);
                }
            }
        }
        for preset in 0..3 {
            let mut snapshot = Snapshot::parse(String::new()).unwrap();
            for edit in crate::visuals::preset(preset) {
                snapshot.edit(&edit).unwrap();
            }
            validate(&snapshot);
        }
    }
}
