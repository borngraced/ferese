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
    identity: Option<PathBuf>,
    failures: u32,
    retry_at: Option<Instant>,
}

impl Default for DeviceState {
    fn default() -> Self {
        Self {
            availability: Availability::Missing,
            identity: None,
            failures: 0,
            retry_at: None,
        }
    }
}

fn device_identity(path: &Path) -> Option<PathBuf> {
    // The parent hardware identity survives cardN renumbering after unplug.
    std::fs::canonicalize(Path::new("/sys/class/drm").join(path.file_name()?).join("device")).ok()
}

impl DeviceState {
    pub fn known(path: &Path) -> Self {
        Self {
            identity: device_identity(path),
            ..Self::default()
        }
    }

    fn current_path<'a>(&self, original: &Path, observed: &'a [(PathBuf, Option<PathBuf>)]) -> Option<&'a Path> {
        observed
            .iter()
            .find(|(path, identity)| match &self.identity {
                Some(expected) => identity.as_ref() == Some(expected),
                None => path == original,
            })
            .map(|(path, _)| path.as_path())
    }

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
        self.availability = Availability::Missing;
        self.failures = 0;
        self.retry_at = None;
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
    fn replug_matches_hardware_even_when_card_numbers_change() {
        let original = Path::new("/dev/dri/card0");
        let identity = PathBuf::from("/sys/devices/pci0000:00/0000:00:02.0");
        let mut slot = DeviceState {
            identity: Some(identity.clone()),
            ..DeviceState::default()
        };
        slot.failed(Instant::now());
        slot.missing();
        let observed = vec![
            (
                PathBuf::from("/dev/dri/card0"),
                Some(PathBuf::from("/sys/devices/other-gpu")),
            ),
            (PathBuf::from("/dev/dri/card2"), Some(identity)),
        ];
        assert_eq!(
            slot.current_path(original, &observed),
            Some(Path::new("/dev/dri/card2"))
        );
        assert!(slot.current_path(original, &observed[..1]).is_none());
        assert!(slot.current_path(original, &[]).is_none());
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

fn slot_mut<'a>(state: &'a mut Ferese, path: &Path) -> &'a mut DeviceState {
    state
        .direct_backend
        .as_mut()
        .unwrap()
        .topology
        .devices
        .get_mut(path)
        .unwrap()
}

/// The only entry point for applying the current output configuration to DRM.
pub(super) fn reconcile_outputs(state: &mut Ferese, reactivate: bool) {
    let Some(backend) = state.direct_backend.as_mut() else {
        return;
    };
    if !backend.topology.request(backend.active && !backend.lid.pending()) {
        return;
    }

    backend.topology.reconciling = true;
    backend.topology.dirty = false;
    if reactivate {
        backend
            .presentation
            .values_mut()
            .for_each(PresentationClock::reset_timing);
    }
    if let Some(token) = backend.topology.retry_timer.take() {
        state.loop_handle.remove(token);
    }
    let paths = backend.topology.devices.keys().cloned().collect::<Vec<_>>();
    let invalidated = std::mem::take(&mut backend.topology.invalidated);
    let observed = all_gpus(backend.session.seat()).map(|paths| {
        paths
            .into_iter()
            .map(|path| {
                let identity = device_identity(&path);
                (path, identity)
            })
            .collect::<Vec<_>>()
    });
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
        let slot = &state.direct_backend.as_ref().unwrap().topology.devices[&path];
        let current_path = observed
            .as_ref()
            .ok()
            .and_then(|devices| slot.current_path(&path, devices));
        let existing = current_path
            .and_then(|path| DrmNode::from_path(path).ok())
            .filter(|node| state.direct_backend.as_ref().unwrap().devices.contains_key(node));
        let previous = match slot.availability {
            Availability::Ready(node) | Availability::Degraded(node) => Some(node),
            _ => existing,
        };
        let found = observed.as_ref().map(|_| current_path.is_some());
        if !matches!(found, Ok(true)) {
            if let Some(node) = previous {
                remove_device(state, node);
            }
            let entry = slot_mut(state, &path);
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
            None => match open_device(state, current_path.unwrap()) {
                Ok(node) => node,
                Err(error) => {
                    tracing::warn!(%error, ?path, "DRM device unavailable; will retry");
                    slot_mut(state, &path).failed(Instant::now());
                    continue;
                }
            },
        };

        let mut failed_outputs = Vec::new();
        let activation = if reactivate && existing.is_some() {
            let device = state.direct_backend.as_mut().unwrap().devices.get_mut(&node).unwrap();
            device.drm.activate(true).map(|()| {
                for (crtc, output) in &mut device.outputs {
                    cancel_output_timers(&state.loop_handle, output);
                    output.surface.reset_buffers();
                    output.primary_commit = None;
                    output.capture_texture = None;
                    output.frame_pending = false;
                    output.lock_frame_pending = false;
                    output.power.suspended();
                    state.display_presentation.remove_output(&output.output);

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
            slot_mut(state, &path).failed(Instant::now());
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
        let entry = slot_mut(state, &path);
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

    state.refresh_idle_inhibition();
    sync_battery_timer(state);
    state.restore_output_focus();
    state.relayout();
    render_all(state);
}
