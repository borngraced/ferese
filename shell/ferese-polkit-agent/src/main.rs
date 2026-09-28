mod prompt;
mod session;

use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};
use zbus::zvariant::{OwnedValue, Value};

const AGENT_PATH: &str = "/dev/ferese/PolkitAgent";
const AUTHORITY: &str = "org.freedesktop.PolicyKit1";
const AUTHORITY_PATH: &str = "/org/freedesktop/PolicyKit1/Authority";
const AUTHORITY_INTERFACE: &str = "org.freedesktop.PolicyKit1.Authority";

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum PromptEvent {
    Start { message: String, user: String },
    Request { prompt: String, echo: bool },
    Info { text: String },
    Error { text: String },
    Response { value: String },
    Cancel,
}

type Identity = (String, HashMap<String, OwnedValue>);

#[derive(Default)]
struct Agent {
    active: Mutex<HashMap<String, Arc<AtomicBool>>>,
}

#[zbus::interface(name = "org.freedesktop.PolicyKit1.AuthenticationAgent")]
impl Agent {
    async fn begin_authentication(
        &self,
        _action_id: String,
        message: String,
        _icon_name: String,
        _details: HashMap<String, String>,
        cookie: String,
        identities: Vec<Identity>,
    ) -> zbus::fdo::Result<()> {
        let uid = choose_identity(&identities).ok_or_else(|| {
            zbus::fdo::Error::Failed("No supported authentication identity".into())
        })?;
        let cancelled = Arc::new(AtomicBool::new(false));

        {
            let mut active = self.active.lock().unwrap();
            if active.contains_key(&cookie) {
                return Err(zbus::fdo::Error::Failed("Duplicate request".into()));
            }
            active.insert(cookie.clone(), cancelled.clone());
        }

        let result = tokio::task::spawn_blocking({
            let cookie = cookie.clone();
            move || {
                session::authenticate(uid, message.chars().take(240).collect(), cookie, cancelled)
            }
        })
        .await
        .map_err(|error| zbus::fdo::Error::Failed(error.to_string()))?;

        self.active.lock().unwrap().remove(&cookie);
        match result {
            Ok(true) => Ok(()),
            Ok(false) => Err(zbus::fdo::Error::Failed("Authentication cancelled".into())),
            Err(error) => Err(zbus::fdo::Error::Failed(error)),
        }
    }

    fn cancel_authentication(&self, cookie: String) {
        if let Some(cancelled) = self.active.lock().unwrap().get(&cookie) {
            cancelled.store(true, Ordering::Release);
        }
    }
}

fn choose_identity(identities: &[Identity]) -> Option<u32> {
    let available: Vec<_> = identities
        .iter()
        .filter(|(kind, _)| kind == "unix-user")
        .filter_map(|(_, details)| {
            details
                .get("uid")
                .and_then(|uid| u32::try_from(uid.clone()).ok())
                .filter(|uid| *uid <= i32::MAX as u32)
        })
        .collect();
    let current = unsafe { libc::geteuid() };

    available
        .iter()
        .copied()
        .find(|uid| *uid == current)
        .or_else(|| available.first().copied())
}

fn session_subject() -> Result<(String, HashMap<String, Value<'static>>), String> {
    let stat = std::fs::read_to_string("/proc/self/stat").map_err(|error| error.to_string())?;
    let suffix = stat
        .rsplit_once(") ")
        .map(|(_, suffix)| suffix)
        .ok_or("Invalid /proc/self/stat")?;
    let started = suffix
        .split_whitespace()
        .nth(19)
        .ok_or("Missing process start time")?
        .parse::<u64>()
        .map_err(|error| error.to_string())?;
    let mut details = HashMap::new();
    details.insert("pid".to_owned(), Value::from(std::process::id()));
    details.insert(
        "uid".to_owned(),
        Value::from(unsafe { libc::geteuid() } as i32),
    );
    details.insert("start-time".to_owned(), Value::from(started));
    Ok(("unix-process".into(), details))
}

async fn run_agent() -> Result<(), Box<dyn std::error::Error>> {
    let subject = session_subject()?;
    let connection = zbus::connection::Builder::system()?
        .serve_at(AGENT_PATH, Agent::default())?
        .build()
        .await?;
    let authority =
        zbus::Proxy::new(&connection, AUTHORITY, AUTHORITY_PATH, AUTHORITY_INTERFACE).await?;
    let locale = std::env::var("LANG").unwrap_or_default();

    authority
        .call::<_, _, ()>(
            "RegisterAuthenticationAgent",
            &(subject, locale, AGENT_PATH),
        )
        .await?;
    std::future::pending::<()>().await;

    Ok(())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    unsafe {
        libc::prctl(libc::PR_SET_DUMPABLE, 0);
    }

    match std::env::args().nth(1).as_deref() {
        None => tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()?
            .block_on(run_agent()),
        Some("--prompt") => prompt::run(),
        Some("--check") => session::check().map_err(Into::into),
        _ => Err("Usage: ferese-polkit-agent [--check]".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chooses_current_identity_when_available() {
        let current = unsafe { libc::geteuid() };
        let identity = |uid: u32| {
            (
                "unix-user".to_owned(),
                HashMap::from([("uid".to_owned(), OwnedValue::from(uid))]),
            )
        };
        assert_eq!(
            choose_identity(&[identity(0), identity(current)]),
            Some(current)
        );
    }
}
