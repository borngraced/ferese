use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use ferese_config::theme::{Appearance as ThemeAppearance, ResolvedTheme};
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
    contrast: u32,
}

impl Appearance {
    fn from_resolved(theme: &ResolvedTheme) -> Self {
        let accent = ferese_config::theme::rgba(&theme.tokens.colors.accent).expect("validated accent");
        Self {
            scheme: if theme.appearance == ThemeAppearance::Light {
                2
            } else {
                1
            },
            accent: (accent[0], accent[1], accent[2]),
            reduced_motion: u32::from(theme.reduced_motion),
            contrast: u32::from(theme.accessibility.increase_contrast),
        }
    }

    fn values(self) -> HashMap<String, OwnedValue> {
        HashMap::from([
            ("color-scheme".into(), self.scheme.into()),
            ("accent-color".into(), Value::from(self.accent).try_to_owned().unwrap()),
            ("contrast".into(), self.contrast.into()),
            ("reduced-motion".into(), self.reduced_motion.into()),
        ])
    }
}

#[derive(Clone)]
pub(crate) struct Settings(Arc<RwLock<Appearance>>);

impl Settings {
    pub(crate) fn new() -> Self {
        Self(Arc::new(RwLock::new(Appearance::from_resolved(
            &ferese_ipc::theme::current().theme,
        ))))
    }

    pub(crate) async fn watch(self, connection: Connection) {
        let emitter = SignalEmitter::new(&connection, PATH).unwrap();
        loop {
            let connection = tokio::task::spawn_blocking(ferese_ipc::theme::Connection::connect).await;
            let Ok(Ok(mut connection)) = connection else {
                tokio::time::sleep(Duration::from_secs(1)).await;
                continue;
            };
            let Ok(cancellation) = connection.cancellation() else {
                return;
            };
            let (send, mut receive) = tokio::sync::mpsc::channel(1);
            tokio::task::spawn_blocking(move || {
                let Ok(mut snapshot) = connection.get() else { return };
                loop {
                    let revision = snapshot.revision;
                    if send.blocking_send(snapshot).is_err() {
                        return;
                    }
                    match connection.watch(revision) {
                        Ok(next) => snapshot = next,
                        Err(_) => return,
                    }
                }
            });
            while let Some(snapshot) = receive.recv().await {
                let next = Appearance::from_resolved(&snapshot.theme);
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
            drop(cancellation);
            tokio::time::sleep(Duration::from_secs(1)).await;
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
        let mut resolved = ResolvedTheme::default();
        resolved.appearance = ThemeAppearance::Light;
        resolved.tokens.colors.accent = "#ff8000".into();
        resolved.reduced_motion = true;
        resolved.accessibility.increase_contrast = true;
        let appearance = Appearance::from_resolved(&resolved);
        assert_eq!(appearance.scheme, 2);
        assert_eq!(appearance.reduced_motion, 1);
        assert_eq!(appearance.accent.0, 1.);
        assert_eq!(appearance.accent.2, 0.);
        assert_eq!(
            u32::try_from(appearance.values().remove("contrast").unwrap()).unwrap(),
            1
        );
    }
}
