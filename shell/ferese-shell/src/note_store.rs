//! Atomic, conflict-checked note edits; always called from a worker thread.
use std::fs;
use std::io::Write;
use std::path::Path;

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
    let mut doc = ferese_config::Document::parse(&source).map_err(|e| e.to_string())?;
    for edit in edits {
        if let Edit::ClockPosition(x, y) = edit {
            if doc.get("desktop_widgets.clock").is_none() {
                return Err("Clock was removed from the config.".into());
            }
            for (field, value) in [
                ("anchor", "top_left".into()),
                ("margin_x", i64::from(*x).into()),
                ("margin_y", i64::from(*y).into()),
            ] {
                doc.set(&format!("desktop_widgets.clock.{field}"), value)
                    .map_err(|e| e.to_string())?;
            }
            continue;
        }
        let id = match edit {
            Edit::Text(id, _) | Edit::Position(id, _, _) => id,
            Edit::ClockPosition(..) => unreachable!(),
        };
        let index = doc
            .get("desktop_widgets.notes")
            .and_then(|v| v.as_array())
            .ok_or("Notes were removed from the config.")?
            .iter()
            .position(|note| note.get("id").and_then(|v| v.as_str()).unwrap_or("note") == id)
            .ok_or("This note no longer exists in the config.")?;
        let fields = match edit {
            Edit::Text(_, value) => vec![("text", value.clone().into())],
            Edit::Position(_, x, y) => vec![
                ("anchor", "top_left".into()),
                ("margin_x", i64::from(*x).into()),
                ("margin_y", i64::from(*y).into()),
            ],
            Edit::ClockPosition(..) => unreachable!(),
        };
        for (field, value) in fields {
            doc.set(&format!("desktop_widgets.notes.{index}.{field}"), value)
                .map_err(|e| e.to_string())?;
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
    file.write_all(updated.as_bytes()).map_err(|e| e.to_string())?;
    file.as_file().sync_all().map_err(|e| e.to_string())?;
    if fs::read_to_string(path).map_err(|e| e.to_string())? != source {
        return Err("Config changed while saving; your note remains open. Edit again to retry.".into());
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
        let path = dir.path().join("config.kdl");
        fs::write(
            &path,
            "desktop-widgets {\n    clock {\n        enabled #true\n        time-size 80\n    }\n}\n",
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
        let path = dir.path().join("config.kdl");
        fs::write(
            &path,
            r##"// keep this
theme {
    colors {
        accent "#123456"
    }
}
desktop-widgets {
    note id="a" text="old"
}
"##,
        )
        .unwrap();
        save(
            &path,
            &[
                Edit::Text("a".into(), "one\ntwo".into()),
                Edit::Position("a".into(), 50, 60),
            ],
        )
        .unwrap();
        let source = fs::read_to_string(&path).unwrap();
        assert!(source.contains("// keep this"));
        assert!(source.contains("#123456"));
        let notes = crate::config::parse_source(&source).unwrap().desktop_widgets.notes;
        assert_eq!(notes[0].text, "one\ntwo");
        assert_eq!((notes[0].margin_x, notes[0].margin_y), (50, 60));
        assert_eq!(notes[0].anchor, ferese_core::desktop::Anchor::TopLeft);
    }
}
