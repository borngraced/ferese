//! Atomic, conflict-checked note edits; always called from a worker thread.
use std::{fs, io::Write, path::Path};

#[derive(Clone, Debug)]
pub enum Edit {
    Text(String, String),
    Position(String, i32, i32),
    ClockPosition(i32, i32),
}

pub fn save(path: &Path, edits: &[Edit]) -> Result<(), String> {
    let canonical = path.canonicalize().map_err(|e| e.to_string())?;
    let path = canonical.as_path();
    let source = fs::read_to_string(path).map_err(|e| e.to_string())?;
    let mut doc: toml_edit::DocumentMut = source
        .parse()
        .map_err(|e: toml_edit::TomlError| e.to_string())?;
    for edit in edits {
        if let Edit::ClockPosition(x, y) = edit {
            let clock = doc
                .get_mut("desktop_widgets")
                .and_then(|item| item.get_mut("clock"))
                .ok_or("Clock was removed from the config.")?;
            clock["anchor"] = toml_edit::value("top_left");
            clock["margin_x"] = toml_edit::value(i64::from(*x));
            clock["margin_y"] = toml_edit::value(i64::from(*y));
            continue;
        }
        let notes = doc
            .get_mut("desktop_widgets")
            .and_then(|item| item.get_mut("notes"))
            .and_then(toml_edit::Item::as_array_of_tables_mut)
            .ok_or("Notes were removed from the config.")?;
        let id = match edit {
            Edit::Text(id, _) | Edit::Position(id, _, _) => id,
            Edit::ClockPosition(..) => unreachable!(),
        };
        let note = notes
            .iter_mut()
            .find(|note| {
                note.get("id")
                    .and_then(toml_edit::Item::as_str)
                    .unwrap_or("note")
                    == id
            })
            .ok_or("This note no longer exists in the config.")?;
        match edit {
            Edit::Text(_, value) => {
                note["text"] = toml_edit::value(value);
            }
            Edit::Position(_, x, y) => {
                note["anchor"] = toml_edit::value("top_left");
                note["margin_x"] = toml_edit::value(i64::from(*x));
                note["margin_y"] = toml_edit::value(i64::from(*y));
            }
            Edit::ClockPosition(..) => unreachable!(),
        }
    }
    let updated = doc.to_string();
    if updated.len() > 60 * 1024 {
        return Err("Configuration exceeds 60 KiB.".into());
    }
    crate::config::parse_source(&updated).map_err(|e| e.to_string())?;
    let parent = path.parent().ok_or("Config has no parent directory.")?;
    let mut file = tempfile::NamedTempFile::new_in(parent).map_err(|e| e.to_string())?;
    file.as_file()
        .set_permissions(fs::metadata(path).map_err(|e| e.to_string())?.permissions())
        .map_err(|e| e.to_string())?;
    file.write_all(updated.as_bytes())
        .map_err(|e| e.to_string())?;
    file.as_file().sync_all().map_err(|e| e.to_string())?;
    if fs::read_to_string(path).map_err(|e| e.to_string())? != source {
        return Err(
            "Config changed while saving; your note remains open. Edit again to retry.".into(),
        );
    }
    file.persist(path).map_err(|e| e.to_string())?;
    fs::File::open(parent)
        .and_then(|dir| dir.sync_all())
        .map_err(|e| e.to_string())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn clock_position_saves_without_a_notes_table() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        fs::write(
            &path,
            "[desktop_widgets.clock]\nenabled = true\ntime_size = 80\n",
        )
        .unwrap();
        save(&path, &[Edit::ClockPosition(120, 240)]).unwrap();
        let config = crate::config::parse_source(&fs::read_to_string(path).unwrap()).unwrap();
        assert_eq!(
            (
                config.desktop_widgets.clock.margin_x,
                config.desktop_widgets.clock.margin_y
            ),
            (120, 240)
        );
        assert_eq!(config.desktop_widgets.clock.time_size, 80.);
    }
    #[test]
    fn note_edits_preserve_unrelated_config_and_multiline_content() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        fs::write(&path, "# keep this\n[theme.colors]\naccent = '#123456'\n[[desktop_widgets.notes]]\nid = 'a'\ntext = 'old'\n").unwrap();
        save(
            &path,
            &[
                Edit::Text("a".into(), "one\ntwo".into()),
                Edit::Position("a".into(), 50, 60),
            ],
        )
        .unwrap();
        let source = fs::read_to_string(&path).unwrap();
        assert!(source.contains("# keep this"));
        assert!(source.contains("#123456"));
        let notes = crate::config::parse_source(&source)
            .unwrap()
            .desktop_widgets
            .notes;
        assert_eq!(notes[0].text, "one\ntwo");
        assert_eq!((notes[0].margin_x, notes[0].margin_y), (50, 60));
        assert_eq!(notes[0].anchor, ferese_core::desktop::Anchor::TopLeft);
    }
}
