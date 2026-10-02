//! Desired device identities survive temporary loss of their DRM resources.
use std::path::PathBuf;

use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Availability {
    Ready(DrmNode),
    Degraded(DrmNode),
    Missing,
    Unavailable,
}

pub(super) struct DeviceState {
    pub availability: Availability,
    failures: u32,
    retry_at: Option<Instant>,
}

impl Default for DeviceState {
    fn default() -> Self {
        Self {
            availability: Availability::Missing,
            failures: 0,
            retry_at: None,
        }
    }
}

impl DeviceState {
    pub fn failed(&mut self, now: Instant) {
        self.availability = Availability::Unavailable;
        self.failures = self.failures.saturating_add(1);
        self.retry_at = Some(now + Duration::from_secs(1u64 << self.failures.saturating_sub(1).min(5)));
    }

    pub fn ready(&mut self, node: DrmNode) {
        self.availability = Availability::Ready(node);
        self.failures = 0;
        self.retry_at = None;
    }

    pub fn missing(&mut self) {
        *self = Self::default();
    }
}

#[derive(Default)]
pub(super) struct Topology {
    pub devices: HashMap<PathBuf, DeviceState>,
    pub invalidated: HashSet<DrmNode>,
    pub reconciling: bool,
    pub dirty: bool,
    pub retry_timer: Option<RegistrationToken>,
}

impl Topology {
    pub fn request(&mut self, active: bool) -> bool {
        self.dirty = true;
        active && !self.reconciling
    }

    pub fn retry_deadline(&self, active: bool) -> Option<Instant> {
        active
            .then(|| self.devices.values().filter_map(|device| device.retry_at).min())
            .flatten()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inactive_changes_coalesce_and_reconciliation_cannot_reenter() {
        let mut topology = Topology::default();
        assert!(!topology.request(false));
        assert!(!topology.request(false));
        assert!(topology.dirty);
        assert!(topology.request(true));
        topology.reconciling = true;
        assert!(!topology.request(true));
    }

    #[test]
    fn unavailable_devices_retry_independently_with_capped_backoff() {
        let now = Instant::now();
        let mut topology = Topology::default();
        topology.devices.insert("/dev/dri/card0".into(), DeviceState::default());
        let healthy = DrmNode::from_dev_id(libc::makedev(226, 1)).unwrap();
        let mut second = DeviceState::default();
        second.ready(healthy);
        topology.devices.insert("/dev/dri/card1".into(), second);
        let device = topology.devices.get_mut(Path::new("/dev/dri/card0")).unwrap();
        device.failed(now);
        assert_eq!(device.retry_at, Some(now + Duration::from_secs(1)));
        for _ in 0..20 {
            device.failed(now);
        }
        assert_eq!(device.retry_at, Some(now + Duration::from_secs(32)));
        assert_eq!(
            topology.devices[Path::new("/dev/dri/card1")].availability,
            Availability::Ready(healthy)
        );
        assert!(topology.retry_deadline(false).is_none());
        assert_eq!(topology.retry_deadline(true), Some(now + Duration::from_secs(32)));
        topology.devices.get_mut(Path::new("/dev/dri/card0")).unwrap().missing();
        assert!(topology.retry_deadline(true).is_none());
    }
}

/// The only entry point for applying the current output configuration to DRM.
pub(super) fn reconcile_outputs(state: &mut Ferese, reactivate: bool) {
    let Some(backend) = state.direct_backend.as_mut() else {
        return;
    };
    if !backend.topology.request(backend.active) {
        return;
    }

    backend.topology.reconciling = true;
    backend.topology.dirty = false;
    if let Some(token) = backend.topology.retry_timer.take() {
        state.loop_handle.remove(token);
    }
    let paths = backend.topology.devices.keys().cloned().collect::<Vec<_>>();
    let invalidated = std::mem::take(&mut backend.topology.invalidated);
    let observed = all_gpus(backend.session.seat());
    let automatic = state
        .output_profiles
        .iter()
        .any(|profile| profile.outputs.iter().any(|output| output.auto_refresh));
    backend.low_power =
        automatic && super::super::power::low_power(backend.low_power, super::super::power::battery_percent());

    for node in invalidated {
        remove_device(state, node);
    }

    for path in paths {
        let existing = DrmNode::from_path(&path)
            .ok()
            .filter(|node| state.direct_backend.as_ref().unwrap().devices.contains_key(node));
        // If the path vanished, its retained identity still identifies the old fd.
        let previous = match state.direct_backend.as_ref().unwrap().topology.devices[&path].availability {
            Availability::Ready(node) | Availability::Degraded(node) => Some(node),
            _ => existing,
        };
        let found = observed.as_ref().map(|paths| paths.contains(&path));
        if !matches!(found, Ok(true)) {
            if let Some(node) = previous {
                remove_device(state, node);
            }
            let entry = state
                .direct_backend
                .as_mut()
                .unwrap()
                .topology
                .devices
                .get_mut(&path)
                .unwrap();
            match found {
                Ok(false) => entry.missing(),
                Err(error) => {
                    tracing::warn!(%error, ?path, "could not enumerate DRM topology");
                    entry.failed(Instant::now());
                }
                Ok(true) => unreachable!(),
            }
            continue;
        }

        if let Some(node) = previous.filter(|node| Some(*node) != existing) {
            remove_device(state, node);
        }
        let node = match existing {
            Some(node) => node,
            None => match open_device(state, &path) {
                Ok(node) => node,
                Err(error) => {
                    tracing::warn!(%error, ?path, "DRM device unavailable; will retry");
                    state
                        .direct_backend
                        .as_mut()
                        .unwrap()
                        .topology
                        .devices
                        .get_mut(&path)
                        .unwrap()
                        .failed(Instant::now());
                    continue;
                }
            },
        };

        let mut failed_outputs = Vec::new();
        let activation = if reactivate && existing.is_some() {
            let device = state.direct_backend.as_mut().unwrap().devices.get_mut(&node).unwrap();
            device.drm.activate(true).map(|()| {
                for (crtc, output) in &mut device.outputs {
                    if let Err(error) = output.surface.clear().and_then(|()| output.surface.reset_state()) {
                        tracing::warn!(%error, ?node, ?crtc, "output reset failed; recreating output");
                        failed_outputs.push(*crtc);
                    }
                }
            })
        } else {
            Ok(())
        };

        if let Err(error) = activation {
            tracing::warn!(%error, ?node, "DRM reactivation failed; retiring unavailable outputs");
            remove_device(state, node);
            state
                .direct_backend
                .as_mut()
                .unwrap()
                .topology
                .devices
                .get_mut(&path)
                .unwrap()
                .failed(Instant::now());
            continue;
        }

        for crtc in failed_outputs {
            let mut output = state
                .direct_backend
                .as_mut()
                .unwrap()
                .devices
                .get_mut(&node)
                .unwrap()
                .outputs
                .remove(&crtc)
                .unwrap();
            cancel_output_timers(&state.loop_handle, &mut output);
            state.display_handle.disable_global::<Ferese>(output.global);
            state.unregister_output(&output.output);
            state
                .direct_backend
                .as_mut()
                .unwrap()
                .presentation
                .remove(&(node, crtc));
        }

        let complete = rescan_device(state, node);
        if complete.is_err() {
            remove_device(state, node);
        }
        let entry = state
            .direct_backend
            .as_mut()
            .unwrap()
            .topology
            .devices
            .get_mut(&path)
            .unwrap();
        if complete == Ok(true) {
            entry.ready(node);
        } else {
            entry.failed(Instant::now());
            // A partial connector failure must not take down working outputs.
            if complete == Ok(false) {
                entry.availability = Availability::Degraded(node);
            }
        }
    }

    let backend = state.direct_backend.as_mut().unwrap();
    backend.connected_outputs = backend
        .devices
        .values()
        .flat_map(|device| device.connected_outputs.clone())
        .collect();
    backend.topology.reconciling = false;
    let retry = backend.topology.retry_deadline(backend.active);
    if let Some(deadline) = retry {
        match state
            .loop_handle
            .insert_source(Timer::from_deadline(deadline), |_, _, state| {
                if let Some(backend) = state.direct_backend.as_mut() {
                    backend.topology.retry_timer = None;
                }
                reconcile_outputs(state, false);
                TimeoutAction::Drop
            }) {
            Ok(token) => state.direct_backend.as_mut().unwrap().topology.retry_timer = Some(token),
            Err(error) => tracing::warn!(%error, "could not arm DRM recovery deadline"),
        }
    }

    sync_battery_timer(state);
    state.restore_output_focus();
    state.relayout();
    render_all(state);
}
