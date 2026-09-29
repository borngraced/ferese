use std::{borrow::Cow, collections::HashMap, sync::Arc, time::Duration};
use tokio::sync::RwLock;
use zbus::{Connection, message::Header, object_server::SignalEmitter, zvariant::Value};

const PATH: &str = "/org/freedesktop/portal/desktop";
const KEYS: [&str; 7] = [
    "disable-printing",
    "disable-save-to-disk",
    "disable-application-handlers",
    "disable-location",
    "disable-camera",
    "disable-microphone",
    "disable-sound-output",
];

#[derive(Clone)]
pub(crate) struct Lockdown(Arc<RwLock<[bool; 7]>>);

impl Lockdown {
    pub(crate) fn new() -> Self {
        Self(Arc::new(RwLock::new(load().unwrap_or_default())))
    }

    pub(crate) async fn watch(self, connection: Connection) {
        let emitter = SignalEmitter::new(&connection, PATH).unwrap();
        loop {
            tokio::time::sleep(Duration::from_secs(1)).await;
            let mut current = self.0.write().await;
            let Some(next) = load() else { continue };
            if *current == next {
                continue;
            }
            let changed = KEYS
                .iter()
                .enumerate()
                .filter(|(index, _)| current[*index] != next[*index])
                .map(|(index, key)| (*key, Value::from(next[index])))
                .collect::<HashMap<_, _>>();
            *current = next;
            drop(current);
            let _ = zbus::fdo::Properties::properties_changed(
                &emitter,
                "org.freedesktop.impl.portal.Lockdown".try_into().unwrap(),
                changed,
                Cow::Borrowed(&[]),
            )
            .await;
        }
    }

    async fn set(
        &self,
        index: usize,
        value: bool,
        connection: &Connection,
        header: Option<Header<'_>>,
    ) -> zbus::fdo::Result<()> {
        let header =
            header.ok_or_else(|| zbus::fdo::Error::AccessDenied("Missing policy caller".into()))?;
        crate::backend::authorize(connection, &header).await?;
        let mut current = self.0.write().await;
        store(index, value).map_err(zbus::fdo::Error::Failed)?;
        current[index] = value;
        Ok(())
    }
}

#[zbus::interface(name = "org.freedesktop.impl.portal.Lockdown")]
impl Lockdown {
    #[zbus(property, name = "disable-printing")]
    async fn disable_printing(&self) -> bool {
        self.0.read().await[0]
    }

    #[zbus(property, name = "disable-printing")]
    async fn set_disable_printing(
        &self,
        value: bool,
        #[zbus(connection)] connection: &Connection,
        #[zbus(header)] header: Option<Header<'_>>,
    ) -> zbus::fdo::Result<()> {
        self.set(0, value, connection, header).await
    }

    #[zbus(property, name = "disable-save-to-disk")]
    async fn disable_save_to_disk(&self) -> bool {
        self.0.read().await[1]
    }

    #[zbus(property, name = "disable-save-to-disk")]
    async fn set_disable_save_to_disk(
        &self,
        value: bool,
        #[zbus(connection)] connection: &Connection,
        #[zbus(header)] header: Option<Header<'_>>,
    ) -> zbus::fdo::Result<()> {
        self.set(1, value, connection, header).await
    }

    #[zbus(property, name = "disable-application-handlers")]
    async fn disable_application_handlers(&self) -> bool {
        self.0.read().await[2]
    }

    #[zbus(property, name = "disable-application-handlers")]
    async fn set_disable_application_handlers(
        &self,
        value: bool,
        #[zbus(connection)] connection: &Connection,
        #[zbus(header)] header: Option<Header<'_>>,
    ) -> zbus::fdo::Result<()> {
        self.set(2, value, connection, header).await
    }

    #[zbus(property, name = "disable-location")]
    async fn disable_location(&self) -> bool {
        self.0.read().await[3]
    }

    #[zbus(property, name = "disable-location")]
    async fn set_disable_location(
        &self,
        value: bool,
        #[zbus(connection)] connection: &Connection,
        #[zbus(header)] header: Option<Header<'_>>,
    ) -> zbus::fdo::Result<()> {
        self.set(3, value, connection, header).await
    }

    #[zbus(property, name = "disable-camera")]
    async fn disable_camera(&self) -> bool {
        self.0.read().await[4]
    }

    #[zbus(property, name = "disable-camera")]
    async fn set_disable_camera(
        &self,
        value: bool,
        #[zbus(connection)] connection: &Connection,
        #[zbus(header)] header: Option<Header<'_>>,
    ) -> zbus::fdo::Result<()> {
        self.set(4, value, connection, header).await
    }

    #[zbus(property, name = "disable-microphone")]
    async fn disable_microphone(&self) -> bool {
        self.0.read().await[5]
    }

    #[zbus(property, name = "disable-microphone")]
    async fn set_disable_microphone(
        &self,
        value: bool,
        #[zbus(connection)] connection: &Connection,
        #[zbus(header)] header: Option<Header<'_>>,
    ) -> zbus::fdo::Result<()> {
        self.set(5, value, connection, header).await
    }

    #[zbus(property, name = "disable-sound-output")]
    async fn disable_sound_output(&self) -> bool {
        self.0.read().await[6]
    }

    #[zbus(property, name = "disable-sound-output")]
    async fn set_disable_sound_output(
        &self,
        value: bool,
        #[zbus(connection)] connection: &Connection,
        #[zbus(header)] header: Option<Header<'_>>,
    ) -> zbus::fdo::Result<()> {
        self.set(6, value, connection, header).await
    }
}

fn values(document: &ferese_config::Document) -> Option<[bool; 7]> {
    for path in ["portals", "portals.lockdown"] {
        if document.get(path).is_some_and(|value| !value.is_object()) {
            return None;
        }
    }
    let mut values = [false; 7];
    for (index, key) in KEYS.iter().enumerate() {
        if let Some(value) = document.get(&format!("portals.lockdown.{}", key.replace('-', "_"))) {
            values[index] = value.as_bool()?;
        }
    }
    Some(values)
}

fn load() -> Option<[bool; 7]> {
    let path = ferese_config::config_path()?;
    let source = match std::fs::read_to_string(path) {
        Ok(source) if source.len() <= 1024 * 1024 => source,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        _ => return None,
    };
    values(&ferese_config::Document::parse(&source).ok()?)
}

fn store(index: usize, value: bool) -> Result<(), String> {
    let _transaction = crate::backend::CONFIG_TRANSACTION
        .lock()
        .map_err(|_| "Configuration transaction failed")?;
    let path = ferese_config::config_path().ok_or("Missing config directory")?;
    let source = match std::fs::read_to_string(&path) {
        Ok(source) if source.len() <= 1024 * 1024 => source,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(error) => return Err(error.to_string()),
        _ => return Err("Configuration is too large".into()),
    };
    let mut document =
        ferese_config::Document::parse(&source).map_err(|error| error.to_string())?;
    if values(&document).is_none() {
        return Err("Invalid lockdown policy".into());
    }
    document
        .set(
            &format!("portals.lockdown.{}", KEYS[index].replace('-', "_")),
            value.into(),
        )
        .map_err(|error| error.to_string())?;
    let directory = path.parent().ok_or("Invalid config path")?;
    std::fs::create_dir_all(directory).map_err(|error| error.to_string())?;
    let mut file = tempfile::NamedTempFile::new_in(directory).map_err(|error| error.to_string())?;
    std::io::Write::write_all(&mut file, document.to_string().as_bytes())
        .map_err(|error| error.to_string())?;
    file.as_file()
        .sync_all()
        .map_err(|error| error.to_string())?;
    if std::fs::read_to_string(&path).unwrap_or_default() != source {
        return Err("Configuration changed; please try again".into());
    }
    file.persist(path).map_err(|error| error.to_string())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_policy_is_unrestricted_and_wrong_types_are_rejected() {
        let empty = ferese_config::Document::parse("").unwrap();
        assert_eq!(values(&empty), Some([false; 7]));
        let enabled = ferese_config::Document::parse(
            "portals { lockdown { disable-camera #true; disable-save-to-disk #true; }; }",
        )
        .unwrap();
        let values = values(&enabled).unwrap();
        assert!(values[1] && values[4]);
        assert!(!values[0]);
        let invalid =
            ferese_config::Document::parse("portals { lockdown { disable-camera \"yes\"; }; }")
                .unwrap();
        assert!(super::values(&invalid).is_none());
        for source in ["portals #false", "portals { lockdown \"bad\"; }"] {
            let invalid = ferese_config::Document::parse(source).unwrap();
            assert!(super::values(&invalid).is_none());
        }
    }
}
