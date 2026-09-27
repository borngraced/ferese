//! Startup handshake: the invoking command exits successfully only after the
//! compositor's confirmation. The UI child remains alive for authentication.
use std::{
    io::{BufRead, BufReader},
    process::{Command, Stdio},
    time::Duration,
};
use wayland_client::{
    Connection, Dispatch, QueueHandle,
    globals::{GlobalListContents, registry_queue_init},
    protocol::wl_registry,
};
struct Registry;
impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for Registry {
    fn event(
        _: &mut Self,
        _: &wl_registry::WlRegistry,
        _: wl_registry::Event,
        _: &GlobalListContents,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}
pub fn check_protocol() -> Result<(), String> {
    let connection =
        Connection::connect_to_env().map_err(|e| format!("Wayland connection: {e}"))?;
    let (globals, _) = registry_queue_init::<Registry>(&connection)
        .map_err(|e| format!("Wayland registry: {e}"))?;
    if globals
        .contents()
        .clone_list()
        .iter()
        .any(|g| g.interface == "ext_session_lock_manager_v1")
    {
        Ok(())
    } else {
        Err("the compositor does not support secure session locking".into())
    }
}
pub fn daemonize() -> Result<(), String> {
    let executable = std::env::current_exe().map_err(|e| format!("Locker executable: {e}"))?;
    let mut child = Command::new(&executable)
        .arg("--foreground")
        .env("FERESE_LOCK_READY", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .map_err(|e| format!("Starting locker {}: {e}", executable.display()))?;
    let stdout = child.stdout.take().unwrap();
    let (tx, rx) = std::sync::mpsc::sync_channel(1);
    std::thread::spawn(move || {
        let mut reader = BufReader::new(stdout);
        let mut line = String::new();
        let ready = reader.read_line(&mut line).is_ok() && line.trim_end() == "FERESE_LOCKED";
        let _ = tx.send(ready);
    });
    match rx.recv_timeout(Duration::from_secs(15)) {
        Ok(true) => Ok(()),
        Ok(false) => {
            let _ = child.wait();
            Err("locker exited without confirming protection".into())
        }
        Err(_) => Err("lock confirmation timed out; the locker was left running".into()),
    }
}

/// libcosmic revision 03d7dcb emits one synthetic Locked on the request and
/// another from SCTK's actual protocol callback. Requiring both is essential:
/// never release swayidle's delay inhibitor on the synthetic notification.
/// Re-audit iced/winit's session_lock Action::Lock and SessionLockHandler when
/// updating libcosmic. A missing callback times out closed, never signals ready.
#[derive(Default)]
pub struct Confirmation {
    events: u8,
}
impl Confirmation {
    pub fn observe(&mut self) -> bool {
        self.events = self.events.saturating_add(1);
        self.events == 2
    }
    pub fn confirmed(&self) -> bool {
        self.events >= 2
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn synthetic_event_does_not_confirm_a_lock() {
        let mut c = Confirmation::default();
        assert!(!c.confirmed());
        assert!(!c.observe());
        assert!(!c.confirmed());
        assert!(c.observe());
        assert!(c.confirmed());
        assert!(!c.observe());
    }
}
