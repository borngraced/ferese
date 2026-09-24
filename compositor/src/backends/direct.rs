use std::{collections::HashMap, error::Error, io, path::Path, time::Duration};

use smithay::{
    backend::{
        allocator::gbm::GbmDevice,
        drm::{DrmDevice, DrmDeviceFd, DrmEvent, DrmEventTime, DrmNode},
        egl::{EGLContext, EGLDisplay},
        libinput::{LibinputInputBackend, LibinputSessionInterface},
        renderer::gles::GlesRenderer,
        session::{Event as SessionEvent, Session, libseat::LibSeatSession},
        udev::{UdevBackend, UdevEvent, primary_gpu},
    },
    output::{Mode as OutputMode, Output, PhysicalProperties, Scale, Subpixel},
    reexports::{
        calloop::EventLoop,
        drm::control::{Device as ControlDevice, Mode as DrmMode, ModeTypeFlags, connector, crtc},
        input::Libinput,
        rustix::fs::OFlags,
    },
    utils::{DeviceFd, Transform},
};

use crate::Ferese;

pub struct DirectBackendState {
    pub session: LibSeatSession,
    pub active: bool,
    devices: HashMap<DrmNode, DirectDevice>,
    presentation: PresentationClock,
}

#[derive(Default)]
struct PresentationClock {
    last_presentation: Option<Duration>,
    presented_frames: u64,
}

struct DirectDevice {
    drm: DrmDevice,
    #[allow(dead_code)]
    gbm: GbmDevice<DrmDeviceFd>,
    #[allow(dead_code)]
    renderer: GlesRenderer,
    #[allow(dead_code)]
    connector: connector::Handle,
    #[allow(dead_code)]
    crtc: crtc::Handle,
    #[allow(dead_code)]
    mode: DrmMode,
    #[allow(dead_code)]
    output: Output,
}

pub fn init(event_loop: &mut EventLoop<Ferese>, state: &mut Ferese) -> Result<(), Box<dyn Error>> {
    let (session, notifier) = LibSeatSession::new()?;
    let seat_name = session.seat();
    let udev_backend = UdevBackend::new(&seat_name)?;
    let primary_path = primary_gpu(&seat_name)?
        .or_else(|| {
            udev_backend
                .device_list()
                .next()
                .map(|(_, path)| path.to_owned())
        })
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no DRM device found"))?;
    let mut libinput_context =
        Libinput::new_with_udev::<LibinputSessionInterface<LibSeatSession>>(session.clone().into());
    libinput_context
        .udev_assign_seat(&seat_name)
        .map_err(|()| io::Error::other(format!("failed to assign libinput seat {seat_name}")))?;
    let libinput_backend = LibinputInputBackend::new(libinput_context.clone());

    state.direct_backend = Some(DirectBackendState {
        session,
        active: true,
        devices: HashMap::new(),
        presentation: PresentationClock::default(),
    });
    open_primary_device(event_loop, state, &primary_path)?;

    event_loop
        .handle()
        .insert_source(libinput_backend, |event, _, state| {
            state.process_input_event(event);
        })?;
    event_loop
        .handle()
        .insert_source(notifier, move |event, _, state| match event {
            SessionEvent::PauseSession => {
                if let Some(backend) = state.direct_backend.as_mut() {
                    backend.active = false;
                    backend
                        .devices
                        .values_mut()
                        .for_each(|device| device.drm.pause());
                }
                libinput_context.suspend();
                tracing::info!("direct session paused");
            }
            SessionEvent::ActivateSession => {
                if libinput_context.resume().is_err() {
                    tracing::error!("failed to resume libinput");
                }
                if let Some(backend) = state.direct_backend.as_mut() {
                    backend.active = true;
                    for device in backend.devices.values_mut() {
                        if let Err(error) = device.drm.activate(false) {
                            tracing::error!(%error, "failed to reactivate DRM device");
                        }
                    }
                }
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

fn open_primary_device(
    event_loop: &mut EventLoop<Ferese>,
    state: &mut Ferese,
    path: &Path,
) -> Result<(), Box<dyn Error>> {
    let mut session = state
        .direct_backend
        .as_ref()
        .expect("direct backend state is initialized before the DRM device")
        .session
        .clone();
    let node = DrmNode::from_path(path)?;
    let fd = session.open(
        path,
        OFlags::RDWR | OFlags::CLOEXEC | OFlags::NOCTTY | OFlags::NONBLOCK,
    )?;
    let fd = DrmDeviceFd::new(DeviceFd::from(fd));
    let (drm, notifier) = DrmDevice::new(fd.clone(), true)?;
    let gbm = GbmDevice::new(fd)?;
    // SAFETY: GBM owns a valid DRM descriptor for the lifetime of the EGL display.
    let egl_display = unsafe { EGLDisplay::new(gbm.clone())? };
    let egl_context = EGLContext::new(&egl_display)?;
    // SAFETY: the new context is not current on another thread and remains renderer-owned.
    let renderer = unsafe { GlesRenderer::new(egl_context)? };
    let (connector, crtc, mode) = select_output(&drm)?;
    let output = create_output(state, &connector, mode);

    event_loop
        .handle()
        .insert_source(notifier, move |event, metadata, state| match event {
            DrmEvent::VBlank(crtc) => {
                if let Some(metadata) = metadata {
                    state.record_drm_presentation(metadata.time, metadata.sequence);
                    tracing::trace!(?node, ?crtc, sequence = metadata.sequence, "page flip");
                }
            }
            DrmEvent::Error(error) => {
                tracing::error!(?node, %error, "DRM event error");
            }
        })?;

    state
        .direct_backend
        .as_mut()
        .expect("direct backend state remains initialized")
        .devices
        .insert(
            node,
            DirectDevice {
                drm,
                gbm,
                renderer,
                connector: connector.handle(),
                crtc,
                mode,
                output,
            },
        );
    state.relayout();
    tracing::info!(?node, ?path, ?crtc, connector = %connector, "initialized primary DRM/GBM device");
    Ok(())
}

fn select_output(drm: &DrmDevice) -> io::Result<(connector::Info, crtc::Handle, DrmMode)> {
    let resources = drm.resource_handles()?;

    for handle in resources.connectors() {
        let connector = drm.get_connector(*handle, true)?;
        if connector.state() != connector::State::Connected {
            continue;
        }
        let Some(mode) = connector
            .modes()
            .iter()
            .find(|mode| mode.mode_type().contains(ModeTypeFlags::PREFERRED))
            .or_else(|| connector.modes().first())
            .copied()
        else {
            continue;
        };

        let current_crtc = connector
            .current_encoder()
            .and_then(|handle| drm.get_encoder(handle).ok())
            .and_then(|encoder| encoder.crtc());
        let compatible_crtc = connector.encoders().iter().find_map(|handle| {
            let encoder = drm.get_encoder(*handle).ok()?;
            resources
                .filter_crtcs(encoder.possible_crtcs())
                .into_iter()
                .next()
        });
        if let Some(crtc) = current_crtc.or(compatible_crtc) {
            return Ok((connector, crtc, mode));
        }
    }

    Err(io::Error::new(
        io::ErrorKind::NotFound,
        "no connected desktop DRM connector with a usable CRTC",
    ))
}

fn create_output(state: &mut Ferese, connector: &connector::Info, mode: DrmMode) -> Output {
    let name = connector.to_string();
    let physical_size = connector.size().unwrap_or((0, 0));
    let output = Output::new(
        name,
        PhysicalProperties {
            size: (physical_size.0 as i32, physical_size.1 as i32).into(),
            subpixel: Subpixel::from(connector.subpixel()),
            make: "Unknown".into(),
            model: "Unknown".into(),
        },
    );
    let output_mode = OutputMode::from(mode);

    output.create_global::<Ferese>(&state.display_handle);
    output.set_preferred(output_mode);
    output.change_current_state(
        Some(output_mode),
        Some(Transform::Normal),
        Some(Scale::Integer(1)),
        Some((0, 0).into()),
    );
    state.space.map_output(&output, (0, 0));
    output
}

impl DirectBackendState {
    pub fn record_presentation(&mut self, time: DrmEventTime, sequence: u32) -> Option<Duration> {
        self.presentation.record(time, sequence)
    }
}

impl PresentationClock {
    fn record(&mut self, time: DrmEventTime, sequence: u32) -> Option<Duration> {
        let DrmEventTime::Monotonic(time) = time else {
            tracing::warn!(
                sequence,
                "DRM driver reported a realtime page-flip timestamp"
            );
            return None;
        };
        let delta = self
            .last_presentation
            .map(|previous| time.saturating_sub(previous));

        self.last_presentation = Some(time);
        self.presented_frames = self.presented_frames.saturating_add(1);
        delta
    }
}

#[cfg(test)]
mod tests {
    use std::time::SystemTime;

    use super::*;

    #[test]
    fn presentation_clock_uses_measured_monotonic_deltas() {
        let mut clock = PresentationClock::default();

        assert_eq!(
            clock.record(DrmEventTime::Monotonic(Duration::from_millis(100)), 1),
            None
        );
        assert_eq!(
            clock.record(DrmEventTime::Monotonic(Duration::from_millis(116)), 2),
            Some(Duration::from_millis(16))
        );
        assert_eq!(clock.presented_frames, 2);
    }

    #[test]
    fn presentation_clock_rejects_realtime_timestamps() {
        let mut clock = PresentationClock::default();

        assert_eq!(
            clock.record(DrmEventTime::Realtime(SystemTime::now()), 1),
            None
        );
        assert_eq!(clock.presented_frames, 0);
    }
}
