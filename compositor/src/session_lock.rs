use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use smithay::output::Output;
use smithay::reexports::calloop::RegistrationToken;
use smithay::reexports::calloop::timer::{TimeoutAction, Timer};
use smithay::reexports::wayland_protocols::ext::session_lock::v1::server::ext_session_lock_manager_v1::ExtSessionLockManagerV1;
use smithay::reexports::wayland_protocols::ext::session_lock::v1::server::ext_session_lock_surface_v1::ExtSessionLockSurfaceV1;
use smithay::reexports::wayland_protocols::ext::session_lock::v1::server::ext_session_lock_v1;
use smithay::reexports::wayland_protocols::ext::session_lock::v1::server::ext_session_lock_v1::ExtSessionLockV1;
use smithay::reexports::wayland_server::protocol::wl_output::WlOutput;
use smithay::reexports::wayland_server::{Client, DataInit, Dispatch, DisplayHandle, Resource};
use smithay::utils::{Logical, Point, SERIAL_COUNTER};
use smithay::wayland::session_lock::{
    LockSurface, SessionLockHandler, SessionLockManagerState, SessionLockState, SessionLocker,
};

use crate::Ferese;

#[derive(Clone, Copy, Debug, serde::Deserialize)]
#[serde(default)]
pub(crate) struct IdleSettings {
    pub dim_after_seconds: u64,
    pub sleep_after_seconds: u64,
}

impl Default for IdleSettings {
    fn default() -> Self {
        Self {
            dim_after_seconds: 30,
            sleep_after_seconds: 120,
        }
    }
}

impl IdleSettings {
    pub(crate) fn validate(self) -> Result<Self, crate::config::ConfigError> {
        if self.dim_after_seconds > 86400 || self.sleep_after_seconds > 86400 {
            return Err(crate::config::ConfigError::InputValue {
                field: "lock_screen.idle_delay",
                value: "must be between 0 and 86400 seconds".into(),
            });
        }
        Ok(self)
    }

    fn next_deadline(self, elapsed: Duration, fade_duration: Duration) -> Option<Duration> {
        if self.sleep_after_seconds != 0 && elapsed >= Duration::from_secs(self.sleep_after_seconds) {
            return None;
        }
        let dim_at = Duration::from_secs(self.dim_after_seconds);
        if !fade_duration.is_zero()
            && self.dim_after_seconds != 0
            && elapsed >= dim_at
            && elapsed < dim_at + fade_duration
        {
            return Some(Duration::from_millis(16));
        }
        [self.dim_after_seconds, self.sleep_after_seconds]
            .into_iter()
            .filter(|delay| *delay != 0)
            .filter_map(|delay| Duration::from_secs(delay).checked_sub(elapsed))
            .filter(|delay| !delay.is_zero())
            .min()
    }

    fn appearance(self, elapsed: Duration, fade_duration: Duration) -> (f32, bool) {
        let sleeping = self.sleep_after_seconds != 0 && elapsed >= Duration::from_secs(self.sleep_after_seconds);
        let dim_at = Duration::from_secs(self.dim_after_seconds);
        let dim = if self.dim_after_seconds == 0 || elapsed < dim_at {
            0.0
        } else if fade_duration.is_zero() {
            0.65
        } else {
            (elapsed.saturating_sub(dim_at).as_secs_f32() / fade_duration.as_secs_f32()).min(1.0) * 0.65
        };
        (if sleeping { 1.0 } else { dim }, sleeping)
    }
}

#[derive(Default)]
enum Lifecycle {
    #[default]
    Unlocked,
    Acquiring(SessionLocker),
    Locked(ExtSessionLockV1),
    Orphaned,
}

impl Lifecycle {
    fn owner(&self) -> Option<&ExtSessionLockV1> {
        match self {
            Self::Acquiring(confirmation) => Some(confirmation.ext_session_lock()),
            Self::Locked(owner) => Some(owner),
            Self::Unlocked | Self::Orphaned => None,
        }
    }

    fn can_acquire(&self) -> bool {
        self.owner().is_none_or(|owner| !owner.is_alive())
    }
}

#[derive(Default)]
pub(crate) struct Lock {
    lifecycle: Lifecycle,
    pub surfaces: HashMap<Output, LockSurface>,
    pub backgrounds: HashMap<Output, smithay::backend::renderer::element::solid::SolidColorBuffer>,
    presented: HashSet<Output>,
    idle_since: Option<Instant>,
    pub(crate) idle_opacity: f32,
    pub(crate) sleeping: bool,
    pub(crate) idle_overlays: HashMap<Output, smithay::backend::renderer::element::solid::SolidColorBuffer>,
    idle_timer: Option<RegistrationToken>,
}

impl Lock {
    pub(crate) fn active(&self) -> bool {
        !matches!(self.lifecycle, Lifecycle::Unlocked)
    }

    fn ready_for_idle<'a>(&self, outputs: impl Iterator<Item = &'a Output>) -> bool {
        self.active() && outputs.into_iter().all(|output| self.presented.contains(output))
    }

    pub(crate) fn output_added(&mut self, output: &Output) {
        self.presented.remove(output);
    }

    pub(crate) fn output_removed(&mut self, output: &Output) {
        self.presented.remove(output);
        self.idle_overlays.remove(output);
    }

    fn activity(&mut self, now: Instant) -> bool {
        if !self.active() {
            return false;
        }
        let changed = self.idle_opacity != 0.0 || self.sleeping;
        self.idle_since = Some(now);
        self.idle_opacity = 0.0;
        self.sleeping = false;
        changed
    }
}

impl Ferese {
    pub(crate) fn refresh_lock_outputs(&mut self) {
        self.confirm_lock_if_ready();
        self.refresh_lock_idle_policy();
    }

    fn lock_outputs_protected(&self) -> bool {
        if let Some(backend) = self.direct_backend.as_ref() {
            // Reconciliation temporarily removes devices while staging changes.
            // Never interpret that partial inventory as protected displays.
            if backend.reconciling() {
                return false;
            }
            return self.session_lock.ready_for_idle(backend.physical_outputs());
        }
        self.session_lock.ready_for_idle(self.space.outputs())
    }

    pub(crate) fn lock_input_activity(&mut self) {
        if self.session_lock.activity(Instant::now()) {
            crate::backends::direct::wake_locked_outputs(self);
            self.cursor_redraw_pending = true;
        }
        self.rearm_lock_idle();
    }

    pub(crate) fn refresh_lock_idle_policy(&mut self) {
        self.update_lock_idle(Instant::now());
        self.rearm_lock_idle();
    }

    fn rearm_lock_idle(&mut self) {
        if let Some(token) = self.session_lock.idle_timer.take() {
            self.loop_handle.remove(token);
        }
        if !self.lock_outputs_protected() {
            return;
        }

        let elapsed = self
            .session_lock
            .idle_since
            .map_or(Duration::ZERO, |since| since.elapsed());
        let Some(delay) = self
            .lock_idle
            .next_deadline(elapsed, self.animation_duration(Duration::from_millis(500)))
        else {
            return;
        };
        match self
            .loop_handle
            .insert_source(Timer::from_duration(delay), |_, _, state| {
                state.session_lock.idle_timer = None;
                state.refresh_lock_idle_policy();
                TimeoutAction::Drop
            }) {
            Ok(token) => self.session_lock.idle_timer = Some(token),
            Err(error) => tracing::warn!(%error, "could not arm lock idle deadline"),
        }
    }

    fn update_lock_idle(&mut self, now: Instant) {
        if !self.lock_outputs_protected() {
            return;
        }
        let fade_duration = self.animation_duration(Duration::from_millis(500));
        let since = self.session_lock.idle_since.get_or_insert(now);
        let (opacity, sleeping) = self
            .lock_idle
            .appearance(now.saturating_duration_since(*since), fade_duration);
        let changed = opacity != self.session_lock.idle_opacity || sleeping != self.session_lock.sleeping;
        let was_sleeping = self.session_lock.sleeping;
        self.session_lock.idle_opacity = opacity;
        self.session_lock.sleeping = sleeping;
        if changed {
            if was_sleeping && !sleeping {
                crate::backends::direct::wake_locked_outputs(self);
            } else if sleeping {
                crate::backends::direct::sleep_locked_outputs(self);
            } else {
                crate::backends::direct::render_all(self);
            }
            self.cursor_redraw_pending = true;
        }
    }

    pub(crate) fn lock_surface_under(
        &self,
        position: Point<f64, Logical>,
    ) -> Option<(
        smithay::reexports::wayland_server::protocol::wl_surface::WlSurface,
        Point<f64, Logical>,
    )> {
        let output = self.space.output_under(position).next()?;
        let surface = self.session_lock.surfaces.get(output)?;
        surface.alive().then(|| {
            (
                surface.wl_surface().clone(),
                self.space.output_geometry(output).unwrap().loc.to_f64(),
            )
        })
    }

    pub(crate) fn focus_lock_surface(&mut self) {
        let surface = self
            .focused_output()
            .and_then(|output| self.session_lock.surfaces.get(output))
            .or_else(|| self.session_lock.surfaces.values().find(|surface| surface.alive()))
            .filter(|surface| surface.alive())
            .map(|surface| surface.wl_surface().clone());
        self.seat
            .get_keyboard()
            .unwrap()
            .set_focus(self, surface, SERIAL_COUNTER.next_serial());
    }

    pub(crate) fn lock_frame_presented(&mut self, output: &Output) {
        if !self.session_lock.active() {
            return;
        }
        let newly_presented = self.session_lock.presented.insert(output.clone());
        self.confirm_lock_if_ready();
        if newly_presented {
            self.refresh_lock_idle_policy();
        }
    }

    pub(crate) fn confirm_lock_if_ready(&mut self) {
        // With no outputs there is no visible content to protect. Re-evaluate
        // here after acquisition and output removal as well as presentation.
        if !self.lock_outputs_protected() {
            return;
        }

        if matches!(self.session_lock.lifecycle, Lifecycle::Acquiring(_)) {
            let Lifecycle::Acquiring(confirmation) =
                std::mem::replace(&mut self.session_lock.lifecycle, Lifecycle::Orphaned)
            else {
                unreachable!()
            };

            let owner = confirmation.ext_session_lock().clone();
            if owner.is_alive() {
                confirmation.lock();
                self.session_lock.lifecycle = Lifecycle::Locked(owner);
                tracing::info!("session lock confirmed after safe output frames");
            }
        }
    }

    pub(crate) fn configure_lock_surfaces(&mut self) {
        for (output, surface) in &self.session_lock.surfaces {
            crate::handlers::set_surface_tree_output(surface.wl_surface(), output);
            if let Some(geometry) = self.space.output_geometry(output) {
                surface.with_pending_state(|state| {
                    state.size = Some((geometry.size.w as u32, geometry.size.h as u32).into())
                });
                surface.send_configure();
            }
        }
    }

    pub(crate) fn lock_frame_callbacks(&self, output: &Output) {
        let eligible = self.callback_outputs();
        if let Some(surface) = self.session_lock.surfaces.get(output).filter(|surface| surface.alive()) {
            smithay::desktop::utils::send_frames_surface_tree(
                surface.wl_surface(),
                output,
                self.start_time.elapsed(),
                None,
                |surface, _| eligible.get(&surface.into()).cloned(),
            );
        }
    }
}

impl SessionLockHandler for Ferese {
    fn lock_state(&mut self) -> &mut SessionLockManagerState {
        &mut self.session_lock_state
    }

    fn lock(&mut self, confirmation: SessionLocker) {
        if !self.session_lock.lifecycle.can_acquire() {
            return;
        }

        // Replacing an orphan never passes through Unlocked. Existing protected
        // presentation facts remain valid, but old surfaces must not own input.
        self.session_lock.lifecycle = Lifecycle::Acquiring(confirmation);
        self.capture_epoch = self.capture_epoch.wrapping_add(1);
        self.session_lock.surfaces.clear();
        self.cancel_logout_confirmation();
        self.refresh_idle_inhibition();
        self.session_lock.idle_since = Some(Instant::now());
        self.input_capture.disable_all();
        self.portal_session.set_locked(true);
        for capture in self.pending_screencopies.drain(..) {
            capture.fail();
        }
        // Draining only covers requests that had not read back yet. One that is
        // already encoding has no pending capture left, so it is terminated
        // here and its staged file is discarded when the worker reports back.
        for request in self.screenshot.terminate_all() {
            tracing::debug!(request, "cancelled an in-flight screenshot for the session lock");
        }
        self.set_overview_active(false);
        let serial = SERIAL_COUNTER.next_serial();
        if let Some(pointer) = self.seat.get_pointer() {
            pointer.unset_grab(self, serial, 0);
            pointer.motion(
                self,
                None,
                &smithay::input::pointer::MotionEvent {
                    location: pointer.current_location(),
                    serial,
                    time: 0,
                },
            );
            pointer.frame(self);
        }
        if let Some(touch) = self.seat.get_touch() {
            touch.unset_grab(self);
            touch.cancel(self);
        }
        let keyboard = self.seat.get_keyboard().unwrap();
        keyboard.unset_grab(self);
        keyboard.set_focus(self, None, serial);
        self.intercepted_keys.clear();
        self.lock_input_activity();
        self.confirm_lock_if_ready();
        crate::backends::direct::render_all(self);
    }

    fn unlock(&mut self) {
        if let Some(timer) = self.session_lock.idle_timer.take() {
            self.loop_handle.remove(timer);
        }
        let sleeping = self.session_lock.sleeping;
        self.session_lock = Lock::default();
        self.refresh_idle_inhibition();
        if sleeping {
            crate::backends::direct::wake_locked_outputs(self);
        }
        self.portal_session.set_locked(false);
        self.restore_keyboard_focus();
        crate::backends::direct::render_all(self);
        tracing::info!("session unlocked by lock owner");
    }

    fn new_surface(&mut self, surface: LockSurface, output: WlOutput) {
        if let Some(output) = Output::from_resource(&output)
            && let Some(geometry) = self.space.output_geometry(&output)
        {
            surface
                .with_pending_state(|state| state.size = Some((geometry.size.w as u32, geometry.size.h as u32).into()));
            crate::handlers::set_surface_tree_output(surface.wl_surface(), &output);
            self.session_lock.surfaces.insert(output, surface);
            self.focus_lock_surface();
        }
    }
}

// Smithay delegates unlock even after posting InvalidUnlock. Check ownership
// and confirmation first so a rejected/unfinished lock cannot unlock the owner.
impl Dispatch<ExtSessionLockV1, SessionLockState> for Ferese {
    fn request(
        state: &mut Self,
        client: &Client,
        lock: &ExtSessionLockV1,
        request: ext_session_lock_v1::Request,
        data: &SessionLockState,
        display: &DisplayHandle,
        data_init: &mut DataInit<'_, Self>,
    ) {
        let owner = state.session_lock.lifecycle.owner() == Some(lock);
        if matches!(request, ext_session_lock_v1::Request::UnlockAndDestroy)
            && (!owner || !matches!(state.session_lock.lifecycle, Lifecycle::Locked(_)))
        {
            lock.post_error(
                ext_session_lock_v1::Error::InvalidUnlock,
                "lock not confirmed or not the active owner",
            );
            return;
        }
        if matches!(request, ext_session_lock_v1::Request::GetLockSurface { .. }) && !owner {
            lock.post_error(ext_session_lock_v1::Error::InvalidUnlock, "not the active lock owner");
            return;
        }
        <SessionLockManagerState as Dispatch<ExtSessionLockV1, SessionLockState, Ferese>>::request(
            state, client, lock, request, data, display, data_init,
        );
    }

    fn destroyed(
        state: &mut Self,
        _: smithay::reexports::wayland_server::backend::ClientId,
        lock: &ExtSessionLockV1,
        _: &SessionLockState,
    ) {
        if state.session_lock.lifecycle.owner() == Some(lock) {
            state.session_lock.lifecycle = Lifecycle::Orphaned;
            state.session_lock.surfaces.clear();
            state.focus_lock_surface();
            crate::backends::direct::render_all(state);
        }
    }
}
smithay::reexports::wayland_server::delegate_global_dispatch!(Ferese: [ExtSessionLockManagerV1: smithay::wayland::session_lock::SessionLockManagerGlobalData] => SessionLockManagerState);
smithay::reexports::wayland_server::delegate_dispatch!(Ferese: [ExtSessionLockManagerV1: ()] => SessionLockManagerState);
smithay::reexports::wayland_server::delegate_dispatch!(Ferese: [ExtSessionLockSurfaceV1: smithay::wayland::session_lock::ExtLockSurfaceUserData] => SessionLockManagerState);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "requires a private XDG_RUNTIME_DIR and permission to bind test sockets"]
    fn zero_outputs_confirm_and_dead_owner_can_be_replaced_without_unlocking() {
        use std::io::{Read, Write};
        use std::os::unix::net::UnixStream;
        use std::sync::Arc;

        use smithay::reexports::calloop::EventLoop;
        use smithay::reexports::wayland_server::Display;

        let runtime = std::path::PathBuf::from(std::env::var_os("XDG_RUNTIME_DIR").unwrap());
        assert!(runtime.starts_with(std::env::temp_dir()));
        let mut event_loop = EventLoop::try_new().unwrap();
        let config = crate::config::Config::default().runtime_config().unwrap();
        let mut state = Ferese::new(&mut event_loop, Display::new().unwrap(), config).unwrap();
        let capture_epoch = state.capture_epoch;

        let acquire = |state: &mut Ferese| {
            let (server, mut wire) = UnixStream::pair().unwrap();
            let client = state
                .display_handle
                .insert_client(server, Arc::new(crate::state::ClientState::default()))
                .unwrap();
            let manager = client
                .create_resource::<ExtSessionLockManagerV1, (), Ferese>(&state.display_handle, 1, ())
                .unwrap();
            // ext_session_lock_manager_v1.lock(new_id=2).
            for word in [manager.id().protocol_id(), (12u32 << 16) | 1, 2] {
                wire.write_all(&word.to_ne_bytes()).unwrap();
            }
            wire
        };
        let dispatch = |event_loop: &mut EventLoop<'static, Ferese>, state: &mut Ferese| {
            event_loop.dispatch(Duration::from_millis(20), state).unwrap();
            state.display_handle.flush_clients().unwrap();
        };

        let mut first = acquire(&mut state);
        dispatch(&mut event_loop, &mut state);
        assert_ne!(
            state.capture_epoch, capture_epoch,
            "lock revokes deferred window snapshots"
        );
        assert!(
            matches!(state.session_lock.lifecycle, Lifecycle::Locked(_)),
            "no outputs must confirm immediately"
        );
        first.set_read_timeout(Some(Duration::from_secs(1))).unwrap();
        let mut event = [0u8; 8];
        first.read_exact(&mut event).unwrap();
        assert_eq!(u32::from_ne_bytes(event[..4].try_into().unwrap()), 2);
        assert_eq!(
            u32::from_ne_bytes(event[4..].try_into().unwrap()),
            8u32 << 16,
            "protocol locked event"
        );

        let owner = state.session_lock.lifecycle.owner().cloned();

        let mut competitor = acquire(&mut state);
        dispatch(&mut event_loop, &mut state);
        assert_eq!(
            state.session_lock.lifecycle.owner(),
            owner.as_ref(),
            "a live owner cannot be replaced"
        );
        // A rejected client must not unlock the confirmed owner.
        for word in [2u32, (8u32 << 16) | 2] {
            competitor.write_all(&word.to_ne_bytes()).unwrap();
        }
        dispatch(&mut event_loop, &mut state);
        assert_eq!(state.session_lock.lifecycle.owner(), owner.as_ref());

        drop(competitor);
        drop(first);
        dispatch(&mut event_loop, &mut state);
        assert!(matches!(state.session_lock.lifecycle, Lifecycle::Orphaned));
        assert!(state.session_lock.active());

        let replacement = acquire(&mut state);
        dispatch(&mut event_loop, &mut state);
        assert!(matches!(state.session_lock.lifecycle, Lifecycle::Locked(_)));
        assert_ne!(state.session_lock.lifecycle.owner(), owner.as_ref());
        drop(replacement);
        dispatch(&mut event_loop, &mut state);
        assert!(state.session_lock.active());

        let output = Output::new(
            "last-output".into(),
            smithay::output::PhysicalProperties {
                size: (0, 0).into(),
                subpixel: smithay::output::Subpixel::Unknown,
                make: "test".into(),
                model: "test".into(),
            },
        );
        output.change_current_state(
            Some(smithay::output::Mode {
                size: (800, 600).into(),
                refresh: 60_000,
            }),
            None,
            None,
            None,
        );
        state.space.map_output(&output, (0, 0));
        state.register_output(&output, "last-output".into());

        let pending = acquire(&mut state);
        dispatch(&mut event_loop, &mut state);
        assert!(
            matches!(state.session_lock.lifecycle, Lifecycle::Acquiring(_)),
            "a newly added output requires a protected frame"
        );
        drop(pending);
        dispatch(&mut event_loop, &mut state);
        assert!(matches!(state.session_lock.lifecycle, Lifecycle::Orphaned));

        let replacement = acquire(&mut state);
        dispatch(&mut event_loop, &mut state);
        assert!(matches!(state.session_lock.lifecycle, Lifecycle::Acquiring(_)));
        state.unregister_output(&output);
        assert!(
            matches!(state.session_lock.lifecycle, Lifecycle::Locked(_)),
            "removing the last pending output must confirm"
        );

        drop(replacement);
        dispatch(&mut event_loop, &mut state);
        state.space.map_output(&output, (0, 0));
        state.session_lock.output_added(&output);

        let mut pending = acquire(&mut state);
        dispatch(&mut event_loop, &mut state);
        assert!(matches!(state.session_lock.lifecycle, Lifecycle::Acquiring(_)));
        assert!(state.session_lock.idle_timer.is_none());

        // Remove the final display without delivering a frame or confirming during removal.
        state.space.unmap_output(&output);
        state.session_lock.output_removed(&output);
        assert!(matches!(state.session_lock.lifecycle, Lifecycle::Acquiring(_)));

        state.refresh_lock_outputs();
        assert!(matches!(state.session_lock.lifecycle, Lifecycle::Locked(_)));
        assert!(state.session_lock.idle_timer.is_some());
        state.display_handle.flush_clients().unwrap();
        pending.set_read_timeout(Some(Duration::from_secs(1))).unwrap();
        pending.read_exact(&mut event).unwrap();
        assert_eq!(u32::from_ne_bytes(event[..4].try_into().unwrap()), 2);
        assert_eq!(u32::from_ne_bytes(event[4..].try_into().unwrap()), 8u32 << 16);
    }

    #[test]
    fn dim_fade_respects_speed_without_changing_idle_deadlines() {
        let policy = IdleSettings::default();
        for (duration, half_at) in [(250, 30_125), (1000, 30_500)] {
            let fade = Duration::from_millis(duration);
            assert_eq!(policy.appearance(Duration::from_secs(29), fade), (0.0, false));
            let halfway = policy.appearance(Duration::from_millis(half_at), fade);
            assert!((halfway.0 - 0.325).abs() < 0.001 && !halfway.1);
            assert_eq!(
                policy.next_deadline(Duration::from_millis(half_at), fade),
                Some(Duration::from_millis(16))
            );
            assert_eq!(policy.appearance(Duration::from_secs(120), fade), (1.0, true));
        }
        assert_eq!(
            policy.appearance(Duration::from_secs(30), Duration::ZERO),
            (0.65, false)
        );
        assert_eq!(policy.appearance(Duration::from_secs(120), Duration::ZERO), (1.0, true));
    }

    #[test]
    fn idle_policy_fades_then_sleeps_and_activity_resets_it() {
        let policy = IdleSettings::default();
        assert_eq!(
            policy.appearance(Duration::from_secs(29), Duration::from_millis(500)),
            (0.0, false)
        );
        let halfway = policy.appearance(Duration::from_millis(30_250), Duration::from_millis(500));
        assert!((halfway.0 - 0.325).abs() < 0.001 && !halfway.1);
        assert_eq!(
            policy.appearance(Duration::from_secs(31), Duration::from_millis(500)),
            (0.65, false)
        );
        assert_eq!(
            policy.appearance(Duration::from_secs(120), Duration::from_millis(500)),
            (1.0, true)
        );
        assert_eq!(
            policy.appearance(Duration::ZERO, Duration::from_millis(500)),
            (0.0, false)
        );
        assert_eq!(
            policy.next_deadline(Duration::from_millis(29_998), Duration::from_millis(500)),
            Some(Duration::from_millis(2))
        );
        assert_eq!(
            policy.next_deadline(Duration::from_secs(30), Duration::from_millis(500)),
            Some(Duration::from_millis(16))
        );
        assert_eq!(
            policy.next_deadline(Duration::from_secs(30), Duration::ZERO),
            Some(Duration::from_secs(90))
        );
        assert_eq!(policy.next_deadline(Duration::from_secs(120), Duration::ZERO), None);
        assert_eq!(
            IdleSettings {
                dim_after_seconds: 0,
                sleep_after_seconds: 0
            }
            .next_deadline(Duration::ZERO, Duration::ZERO),
            None
        );
    }

    #[test]
    fn mirror_must_present_protected_frame_before_lock_is_ready() {
        let make_output = |name: &str| {
            Output::new(
                name.into(),
                smithay::output::PhysicalProperties {
                    size: (0, 0).into(),
                    subpixel: smithay::output::Subpixel::Unknown,
                    make: "test".into(),
                    model: "test".into(),
                },
            )
        };
        let source = make_output("source");
        let mirror = make_output("mirror");
        let mut lock = Lock {
            lifecycle: Lifecycle::Orphaned,
            ..Lock::default()
        };
        lock.presented.insert(source.clone());
        let displays = [&source, &mirror];
        assert!(
            !lock.ready_for_idle(displays.into_iter()),
            "failed mirror rendering must block confirmation"
        );
        lock.presented.insert(mirror.clone());
        assert!(lock.ready_for_idle(displays.into_iter()));
        lock.output_added(&mirror);
        assert!(
            !lock.ready_for_idle(displays.into_iter()),
            "reconfiguration invalidates physical protection"
        );
        lock.output_removed(&mirror);
        assert!(lock.ready_for_idle(std::iter::once(&source)));
    }

    #[test]
    fn protected_fallback_frames_allow_idle_after_early_locker_crash() {
        let output = Output::new(
            "test".into(),
            smithay::output::PhysicalProperties {
                size: (0, 0).into(),
                subpixel: smithay::output::Subpixel::Unknown,
                make: "test".into(),
                model: "test".into(),
            },
        );
        let mut lock = Lock {
            lifecycle: Lifecycle::Orphaned,
            ..Lock::default()
        };
        assert!(!lock.ready_for_idle(std::iter::once(&output)));
        lock.presented.insert(output.clone());
        assert!(lock.ready_for_idle(std::iter::once(&output)));
        assert!(matches!(lock.lifecycle, Lifecycle::Orphaned));
        lock.output_added(&output);
        assert!(!lock.ready_for_idle(std::iter::once(&output)));
        assert!(matches!(lock.lifecycle, Lifecycle::Orphaned));
    }

    #[test]
    fn input_wakes_a_sleeping_orphan_without_unlocking() {
        let mut lock = Lock {
            lifecycle: Lifecycle::Orphaned,
            sleeping: true,
            idle_opacity: 1.0,
            ..Lock::default()
        };
        let now = Instant::now();
        assert!(lock.activity(now));
        assert!(lock.active() && matches!(lock.lifecycle, Lifecycle::Orphaned));
        assert!(!lock.sleeping);
        assert_eq!(lock.idle_opacity, 0.0);
        assert_eq!(lock.idle_since, Some(now));
        assert!(!lock.activity(now));
        let mut unlocked = Lock::default();
        assert!(!unlocked.activity(now));
        assert!(unlocked.idle_since.is_none());
    }

    #[test]
    fn idle_disabled_options_are_independent() {
        assert_eq!(
            IdleSettings {
                dim_after_seconds: 0,
                sleep_after_seconds: 0
            }
            .appearance(Duration::from_secs(9999), Duration::from_millis(500)),
            (0.0, false)
        );
        assert_eq!(
            IdleSettings {
                dim_after_seconds: 0,
                sleep_after_seconds: 10
            }
            .appearance(Duration::from_secs(10), Duration::from_millis(500)),
            (1.0, true)
        );
        assert_eq!(
            IdleSettings {
                dim_after_seconds: 1,
                sleep_after_seconds: 0
            }
            .appearance(Duration::from_secs(9999), Duration::from_millis(500)),
            (0.65, false)
        );
        assert!(
            IdleSettings {
                dim_after_seconds: 86401,
                sleep_after_seconds: 0
            }
            .validate()
            .is_err()
        );
        assert!(crate::config::Config::parse_source("lock-screen { dim-after-seconds -1; }").is_err());
        assert!(
            crate::config::Config::parse_source("lock-screen { sleep-after-seconds 90000; }")
                .unwrap()
                .runtime_config()
                .is_err()
        );
    }
}
