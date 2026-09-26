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
    pub fn new(configs: &[DaemonConfig], nested: bool) -> Self {
        Self(
            configs
                .iter()
                .filter(|config| !nested || config.nested)
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
    fn previews_do_not_duplicate_session_daemons() {
        let configs = vec![DaemonConfig {
            command: vec!["awari".into()],
            restart: true,
            nested: false,
        }];
        assert!(Runner::new(&configs, true).0.is_empty());
        let mut direct = Runner::new(&configs, false);
        assert_eq!(direct.0.len(), 1);
        direct.stop();
        assert!(direct.0[0].finished);
    }
}
