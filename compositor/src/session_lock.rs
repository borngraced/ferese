use crate::Ferese;
use smithay::{
    output::Output,
    reexports::{
        wayland_protocols::ext::session_lock::v1::server::{
            ext_session_lock_manager_v1::ExtSessionLockManagerV1,
            ext_session_lock_surface_v1::ExtSessionLockSurfaceV1,
            ext_session_lock_v1::{self, ExtSessionLockV1},
        },
        wayland_server::{
            Client, DataInit, Dispatch, DisplayHandle, Resource, protocol::wl_output::WlOutput,
        },
    },
    utils::{Logical, Point, SERIAL_COUNTER},
    wayland::session_lock::{
        LockSurface, SessionLockHandler, SessionLockManagerState, SessionLockState, SessionLocker,
    },
};
use std::collections::{HashMap, HashSet};

#[derive(Default)]
pub(crate) struct Lock {
    pub active: bool,
    owner: Option<ExtSessionLockV1>,
    confirmation: Option<SessionLocker>,
    pub surfaces: HashMap<Output, LockSurface>,
    pub backgrounds: HashMap<Output, smithay::backend::renderer::element::solid::SolidColorBuffer>,
    presented: HashSet<Output>,
    confirmed: bool,
}

impl Ferese {
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
            .or_else(|| {
                self.session_lock
                    .surfaces
                    .values()
                    .find(|surface| surface.alive())
            })
            .filter(|surface| surface.alive())
            .map(|surface| surface.wl_surface().clone());
        self.seat
            .get_keyboard()
            .unwrap()
            .set_focus(self, surface, SERIAL_COUNTER.next_serial());
    }

    pub(crate) fn lock_frame_presented(&mut self, output: &Output) {
        if !self.session_lock.active {
            return;
        }
        self.session_lock.presented.insert(output.clone());
        if self
            .space
            .outputs()
            .all(|output| self.session_lock.presented.contains(output))
            && let Some(confirmation) = self.session_lock.confirmation.take()
            && confirmation.ext_session_lock().is_alive()
        {
            confirmation.lock();
            self.session_lock.confirmed = true;
            tracing::info!("session lock confirmed after safe output frames");
        }
    }

    pub(crate) fn configure_lock_surfaces(&mut self) {
        for (output, surface) in &self.session_lock.surfaces {
            if let Some(geometry) = self.space.output_geometry(output) {
                surface.with_pending_state(|state| {
                    state.size = Some((geometry.size.w as u32, geometry.size.h as u32).into())
                });
                surface.send_configure();
            }
        }
    }

    pub(crate) fn lock_frame_callbacks(&self, output: &Output) {
        if let Some(surface) = self
            .session_lock
            .surfaces
            .get(output)
            .filter(|surface| surface.alive())
        {
            smithay::desktop::utils::send_frames_surface_tree(
                surface.wl_surface(),
                output,
                self.start_time.elapsed(),
                Some(std::time::Duration::ZERO),
                |_, _| Some(output.clone()),
            );
        }
    }
}

impl SessionLockHandler for Ferese {
    fn lock_state(&mut self) -> &mut SessionLockManagerState {
        &mut self.session_lock_state
    }

    fn lock(&mut self, confirmation: SessionLocker) {
        if self.session_lock.active {
            return;
        }
        self.session_lock.active = true;
        self.session_lock.owner = Some(confirmation.ext_session_lock().clone());
        self.session_lock.confirmation = Some(confirmation);
        for capture in self.pending_screencopies.drain(..) {
            capture.frame.failed();
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
        crate::backends::direct::render_all(self);
    }

    fn unlock(&mut self) {
        self.session_lock = Lock::default();
        self.restore_keyboard_focus();
        crate::backends::direct::render_all(self);
        tracing::info!("session unlocked by lock owner");
    }

    fn new_surface(&mut self, surface: LockSurface, output: WlOutput) {
        if let Some(output) = Output::from_resource(&output)
            && let Some(geometry) = self.space.output_geometry(&output)
        {
            surface.with_pending_state(|state| {
                state.size = Some((geometry.size.w as u32, geometry.size.h as u32).into())
            });
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
        let owner = state.session_lock.owner.as_ref() == Some(lock);
        if matches!(request, ext_session_lock_v1::Request::UnlockAndDestroy)
            && (!owner || !state.session_lock.confirmed)
        {
            lock.post_error(
                ext_session_lock_v1::Error::InvalidUnlock,
                "lock not confirmed or not the active owner",
            );
            return;
        }
        if matches!(request, ext_session_lock_v1::Request::GetLockSurface { .. }) && !owner {
            lock.post_error(
                ext_session_lock_v1::Error::InvalidUnlock,
                "not the active lock owner",
            );
            return;
        }
        <SessionLockManagerState as Dispatch<ExtSessionLockV1, SessionLockState, Ferese>>::request(
            state, client, lock, request, data, display, data_init,
        );
    }
}
smithay::reexports::wayland_server::delegate_global_dispatch!(Ferese: [ExtSessionLockManagerV1: smithay::wayland::session_lock::SessionLockManagerGlobalData] => SessionLockManagerState);
smithay::reexports::wayland_server::delegate_dispatch!(Ferese: [ExtSessionLockManagerV1: ()] => SessionLockManagerState);
smithay::reexports::wayland_server::delegate_dispatch!(Ferese: [ExtSessionLockSurfaceV1: smithay::wayland::session_lock::ExtLockSurfaceUserData] => SessionLockManagerState);
