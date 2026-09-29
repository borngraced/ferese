//! Session-owned services. Never grant shell/effects privileges to daemons.
use crate::{Ferese, config::DaemonConfig, private_client::ClientCapabilities};
use std::{
    process::Child,
    time::{Duration, Instant},
};

struct Service {
    config: DaemonConfig,
    child: Option<Child>,
    next_start: Instant,
    finished: bool,
}

pub(crate) struct Runner(Vec<Service>);

impl Runner {
    pub fn reconcile(&mut self, configs: &[DaemonConfig], nested: bool) {
        let mut previous = std::mem::take(&mut self.0);
        for config in configs
            .iter()
            .filter(|c| c.enabled && (!nested || c.nested))
        {
            if self
                .0
                .iter()
                .any(|service| service.config.command == config.command)
            {
                continue;
            }
            if let Some(index) = previous
                .iter()
                .position(|service| service.config.command == config.command)
            {
                let mut service = previous.remove(index);
                if config.restart && !service.config.restart {
                    service.finished = false;
                }
                service.config = config.clone();
                self.0.push(service);
            } else {
                self.0.push(Service {
                    config: config.clone(),
                    child: None,
                    next_start: Instant::now(),
                    finished: false,
                });
            }
        }

        // TERM/grace/reaping must not pause the compositor's event loop.
        for mut removed in previous {
            if let Some(mut child) = removed.child.take() {
                std::thread::spawn(move || crate::terminate_child(&mut child));
            }
        }
    }

    pub fn new(configs: &[DaemonConfig], nested: bool) -> Self {
        Self(
            configs
                .iter()
                .filter(|config| config.enabled && (!nested || config.nested))
                .map(|config| Service {
                    config: config.clone(),
                    child: None,
                    next_start: Instant::now(),
                    finished: false,
                })
                .collect(),
        )
    }

    pub fn tick(&mut self, state: &mut Ferese) {
        self.reconcile(&state.autostart, state.direct_backend.is_none());
        let now = Instant::now();
        for service in &mut self.0 {
            if let Some(child) = &mut service.child {
                match child.try_wait() {
                    Ok(Some(status)) => {
                        tracing::info!(command = ?service.config.command, %status, "session daemon exited");
                        service.child = None;
                        service.finished = !service.config.restart;
                        service.next_start = now + Duration::from_secs(5);
                    }
                    Ok(None) => continue,
                    Err(error) => {
                        tracing::warn!(%error, "cannot reap session daemon");
                        continue;
                    }
                }
            }
            if !service.finished && now >= service.next_start {
                service.child = crate::spawn_client(
                    state,
                    service.config.command.iter().map(Into::into).collect(),
                    ClientCapabilities::default(),
                );
                service.next_start = now + Duration::from_secs(5);
                if service.child.is_none() && !service.config.restart {
                    service.finished = true;
                }
            }
        }
    }

    pub fn stop(&mut self) {
        for service in &mut self.0 {
            if let Some(mut child) = service.child.take() {
                crate::terminate_child(&mut child);
            }
            service.finished = true;
        }
    }
}

impl Drop for Runner {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn live_reconcile_keeps_existing_process_and_changes_policy_without_restart() {
        let mut config = DaemonConfig {
            command: vec!["/bin/sleep".into(), "30".into()],
            enabled: true,
            restart: false,
            nested: false,
        };
        let mut runner = Runner::new(&[config.clone()], false);
        runner.0[0].child = Some(
            std::process::Command::new("/bin/sleep")
                .arg("30")
                .spawn()
                .unwrap(),
        );
        let pid = runner.0[0].child.as_ref().unwrap().id();
        config.restart = true;
        runner.reconcile(&[config.clone()], false);
        assert_eq!(runner.0[0].child.as_ref().unwrap().id(), pid);
        assert!(runner.0[0].config.restart);
        runner.reconcile(&[config.clone(), config.clone()], false);
        assert_eq!(runner.0.len(), 1);
        config.enabled = false;
        runner.reconcile(&[config], false);
        assert!(runner.0.is_empty());
        let deadline = Instant::now() + Duration::from_secs(2);
        while unsafe { libc::kill(pid as i32, 0) } == 0 {
            assert!(
                Instant::now() < deadline,
                "removed service was not stopped/reaped"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn previews_do_not_duplicate_session_daemons() {
        let configs = vec![DaemonConfig {
            command: vec!["awari".into()],
            enabled: true,
            restart: true,
            nested: false,
        }];
        assert!(Runner::new(&configs, true).0.is_empty());
        let mut direct = Runner::new(&configs, false);
        assert_eq!(direct.0.len(), 1);
        direct.stop();
        assert!(direct.0[0].finished);
    }

    #[test]
    fn disabled_login_items_never_start_on_either_backend() {
        let configs = vec![DaemonConfig {
            command: vec!["not-executed".into()],
            enabled: false,
            restart: true,
            nested: true,
        }];
        assert!(Runner::new(&configs, true).0.is_empty());
        assert!(Runner::new(&configs, false).0.is_empty());
    }
}
