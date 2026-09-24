use std::{error::Error, io};

use smithay::{
    backend::{
        libinput::{LibinputInputBackend, LibinputSessionInterface},
        session::{Event as SessionEvent, Session, libseat::LibSeatSession},
        udev::{UdevBackend, UdevEvent},
    },
    reexports::{calloop::EventLoop, input::Libinput},
};

use crate::Ferese;

pub fn init(event_loop: &mut EventLoop<Ferese>, state: &mut Ferese) -> Result<(), Box<dyn Error>> {
    let (session, notifier) = LibSeatSession::new()?;
    let seat_name = session.seat();
    let udev_backend = UdevBackend::new(&seat_name)?;
    let mut libinput_context =
        Libinput::new_with_udev::<LibinputSessionInterface<LibSeatSession>>(session.clone().into());
    libinput_context
        .udev_assign_seat(&seat_name)
        .map_err(|()| io::Error::other(format!("failed to assign libinput seat {seat_name}")))?;
    let libinput_backend = LibinputInputBackend::new(libinput_context.clone());

    state.direct_session = Some(session);
    state.session_active = true;

    event_loop
        .handle()
        .insert_source(libinput_backend, |event, _, state| {
            state.process_input_event(event);
        })?;
    event_loop
        .handle()
        .insert_source(notifier, move |event, _, state| match event {
            SessionEvent::PauseSession => {
                state.session_active = false;
                libinput_context.suspend();
                tracing::info!("direct session paused");
            }
            SessionEvent::ActivateSession => {
                if libinput_context.resume().is_err() {
                    tracing::error!("failed to resume libinput");
                }
                state.session_active = true;
                tracing::info!("direct session activated");
            }
        })?;
    event_loop
        .handle()
        .insert_source(udev_backend, |event, _, _state| match event {
            UdevEvent::Added { device_id, path } => {
                tracing::info!(?device_id, ?path, "DRM device added");
            }
            UdevEvent::Changed { device_id } => {
                tracing::debug!(?device_id, "DRM device changed");
            }
            UdevEvent::Removed { device_id } => {
                tracing::info!(?device_id, "DRM device removed");
            }
        })?;

    tracing::info!(seat = %seat_name, "initialized direct session input and device discovery");
    Ok(())
}
