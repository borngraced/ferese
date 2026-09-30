use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use ferese_config::Document;
use ferese_theme::Palette;
use tokio::sync::RwLock;
use zbus::Connection;
use zbus::object_server::SignalEmitter;
use zbus::zvariant::{OwnedValue, Value};

const APPEARANCE: &str = "org.freedesktop.appearance";
const PATH: &str = "/org/freedesktop/portal/desktop";
type Values = HashMap<String, HashMap<String, OwnedValue>>;

#[derive(Clone, Copy, Debug, PartialEq)]
struct Appearance {
    scheme: u32,
    accent: (f64, f64, f64),
    reduced_motion: u32,
}

impl Appearance {
    fn from_document(document: Option<&Document>) -> Self {
        let palette = Palette::from_document(document);
        let animations = document
            .and_then(|document| document.get("animations.enabled"))
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(true);
        Self {
            scheme: if ferese_theme::luminance(palette.sidebar) > 0.5 {
                2
            } else {
                1
            },
            accent: (
                palette.accent.r as f64,
                palette.accent.g as f64,
                palette.accent.b as f64,
            ),
            reduced_motion: u32::from(
                !animations
                    || document
                        .and_then(|doc| doc.get("animations.reduced_motion"))
                        .and_then(serde_json::Value::as_bool)
                        .unwrap_or(false),
            ),
        }
    }

    fn values(self) -> HashMap<String, OwnedValue> {
        HashMap::from([
            ("color-scheme".into(), self.scheme.into()),
            ("accent-color".into(), Value::from(self.accent).try_to_owned().unwrap()),
            ("contrast".into(), 0u32.into()),
            ("reduced-motion".into(), self.reduced_motion.into()),
        ])
    }
}

#[derive(Clone)]
pub(crate) struct Settings(Arc<RwLock<Appearance>>);

impl Settings {
    pub(crate) fn new() -> Self {
        Self(Arc::new(RwLock::new(
            load().unwrap_or_else(|| Appearance::from_document(None)),
        )))
    }

    pub(crate) async fn watch(self, connection: Connection) {
        let emitter = SignalEmitter::new(&connection, PATH).unwrap();
        loop {
            tokio::time::sleep(Duration::from_secs(1)).await;
            let Some(next) = load() else { continue };
            let mut current = self.0.write().await;
            if *current == next {
                continue;
            }
            let previous = current.values();
            *current = next;
            drop(current);
            for (key, value) in next.values() {
                if previous.get(&key) != Some(&value) {
                    let _ = Self::setting_changed(&emitter, APPEARANCE, &key, value).await;
                }
            }
        }
    }
}

#[zbus::interface(name = "org.freedesktop.impl.portal.Settings")]
impl Settings {
    #[zbus(property, name = "version")]
    fn version(&self) -> u32 {
        1
    }

    async fn read_all(&self, namespaces: Vec<String>) -> Values {
        if matches_namespace(&namespaces, APPEARANCE) {
            HashMap::from([(APPEARANCE.into(), self.0.read().await.values())])
        } else {
            HashMap::new()
        }
    }

    async fn read(&self, namespace: &str, key: &str) -> zbus::fdo::Result<OwnedValue> {
        if namespace == APPEARANCE {
            if let Some(value) = self.0.read().await.values().remove(key) {
                return Ok(value);
            }
        }
        Err(zbus::fdo::Error::InvalidArgs("Unknown setting".into()))
    }

    #[zbus(signal)]
    async fn setting_changed(
        emitter: &SignalEmitter<'_>,
        namespace: &str,
        key: &str,
        value: OwnedValue,
    ) -> zbus::Result<()>;
}

fn matches_namespace(filters: &[String], namespace: &str) -> bool {
    filters.is_empty()
        || filters.iter().any(|filter| {
            filter.is_empty()
                || filter == namespace
                || filter.strip_suffix(".*").is_some_and(|prefix| {
                    namespace
                        .strip_prefix(prefix)
                        .is_some_and(|suffix| suffix.starts_with('.'))
                })
        })
}

fn load() -> Option<Appearance> {
    let path = ferese_config::config_path()?;
    let source = match std::fs::read_to_string(path) {
        Ok(source) if source.len() <= 1024 * 1024 => source,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Some(Appearance::from_document(None));
        }
        _ => return None,
    };
    let document = Document::parse(&source).ok()?;
    Some(Appearance::from_document(Some(&document)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn namespaces_support_only_trailing_section_wildcards() {
        for filter in ["", "org.freedesktop.appearance", "org.freedesktop.*", "org.*"] {
            assert!(matches_namespace(&[filter.into()], APPEARANCE));
        }
        for filter in [
            "org.freedesktop*",
            "org.*.appearance",
            "org.freedesktop.appearance.*",
            "other",
        ] {
            assert!(!matches_namespace(&[filter.into()], APPEARANCE));
        }
        assert!(matches_namespace(&[], APPEARANCE));
    }

    #[test]
    fn reports_ferese_theme_and_reduced_motion() {
        let doc = Document::parse(
            "theme { colors { surface-base \"#ffffff\"; accent \"#ff8000\"; }; }; animations { enabled #false; }",
        )
        .unwrap();
        let appearance = Appearance::from_document(Some(&doc));
        assert_eq!(appearance.scheme, 2);
        assert_eq!(appearance.reduced_motion, 1);
        assert_eq!(appearance.accent.0, 1.);
        assert_eq!(appearance.accent.2, 0.);
        assert_eq!(
            u32::try_from(appearance.values().remove("contrast").unwrap()).unwrap(),
            0
        );
    }
}
