use ferese_ipc::theme::Snapshot;
use ferese_theme_client::service::fallback;
use serde_json::{Value, json};

fn fixture() -> Value {
    serde_json::from_str(include_str!("fixtures/theme-v2.json")).unwrap()
}

#[test]
fn default_snapshot_preserves_wire_values_and_environment_wallpaper() {
    let mut snapshot = fallback();
    let wallpaper = Some(ferese_config::default_wallpaper().into());
    assert_eq!(snapshot.theme.tokens.background.path, wallpaper);
    assert_eq!(snapshot.presented.tokens.background.path, wallpaper);
    snapshot.theme.tokens.background.path = Some("<wallpaper>".into());
    snapshot.presented.tokens.background.path = Some("<wallpaper>".into());
    assert_eq!(serde_json::to_value(snapshot).unwrap(), fixture());
}

#[test]
fn older_snapshots_supply_catalog_but_explicit_empty_catalog_stays_empty() {
    let mut value = fixture();
    value.as_object_mut().unwrap().remove("families");
    value.as_object_mut().unwrap().remove("fallback_note");
    let snapshot: Snapshot = Snapshot::decode(value.clone(), ferese_config::families::builtins).unwrap();
    assert_eq!(snapshot.families, ferese_config::families::builtins());
    assert_eq!(snapshot.fallback_note, None);
    value["families"] = json!([]);
    assert!(
        Snapshot::decode(value.clone(), ferese_config::families::builtins)
            .unwrap()
            .families
            .is_empty()
    );
    value["families"] = Value::Null;
    assert!(Snapshot::decode(value, ferese_config::families::builtins).is_err());
}

#[test]
fn offline_preview_resolves_without_changing_published_values() {
    let before = fallback();
    let document = ferese_config::Document::parse("theme { mode \"light\"; family \"gruvbox\"; }\n").unwrap();
    let candidate = ferese_config::theme::resolve(
        &document,
        std::path::Path::new("/unused"),
        "2026-10-02T12:00:00Z".parse().unwrap(),
        |_| panic!("built-in preview must not read theme files"),
    )
    .unwrap();
    assert_eq!(candidate.theme.appearance, ferese_config::theme::Appearance::Light);
    assert_eq!(candidate.theme.tokens.colors.surface_base, "#FBF1C7");
    assert_eq!(before, fallback());
}

#[test]
fn complete_snapshots_do_not_resolve_fallbacks_and_future_versions_are_rejected() {
    let value = fixture();
    let snapshot = Snapshot::decode(value.clone(), || panic!("unexpected catalog resolution")).unwrap();
    assert_eq!(serde_json::to_value(snapshot).unwrap(), value);
    let mut future = value;
    future["version"] = json!(ferese_ipc::theme::SCHEMA_VERSION + 1);
    assert_eq!(
        Snapshot::decode(future, Vec::new).unwrap_err(),
        "Unsupported theme snapshot version"
    );
}
