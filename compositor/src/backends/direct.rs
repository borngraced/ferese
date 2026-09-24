use std::{collections::HashMap, error::Error, io, path::Path, time::Duration};

use smithay::{
    backend::{
        allocator::{
            Fourcc,
            gbm::{GbmAllocator, GbmBufferFlags, GbmDevice},
        },
        drm::{DrmDevice, DrmDeviceFd, DrmEvent, DrmEventTime, DrmNode, GbmBufferedSurface},
        egl::{EGLContext, EGLDisplay},
        libinput::{LibinputInputBackend, LibinputSessionInterface},
        renderer::{Bind, ImportDma, ImportMemWl, damage::OutputDamageTracker, gles::GlesRenderer},
        session::{Event as SessionEvent, Session, libseat::LibSeatSession},
        udev::{UdevBackend, UdevEvent, primary_gpu},
    },
    desktop::utils::OutputPresentationFeedback,
    output::{Mode as OutputMode, Output, PhysicalProperties, Scale, Subpixel},
    reexports::{
        calloop::EventLoop,
        drm::control::{Device as ControlDevice, Mode as DrmMode, ModeTypeFlags, connector, crtc},
        input::Libinput,
        rustix::fs::OFlags,
        wayland_protocols::wp::presentation_time::server::wp_presentation_feedback::Kind,
    },
    utils::{DeviceFd, Monotonic, Time, Transform},
    wayland::presentation::Refresh,
};

use crate::{Ferese, winit::animated_window_elements};

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
    refresh_interval: Option<Duration>,
    missed_deadlines: u64,
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
    surface: GbmBufferedSurface<GbmAllocator<DrmDeviceFd>, OutputPresentationFeedback>,
    damage_tracker: OutputDamageTracker,
    frame_pending: bool,
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
    let session_active = session.is_active();
    if !session_active {
        libinput_context.suspend();
    }

    state.direct_backend = Some(DirectBackendState {
        session,
        active: session_active,
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
                    backend.presentation.reset_timing();
                    backend.devices.values_mut().for_each(|device| {
                        device.drm.pause();
                        device.surface.reset_buffers();
                        device.damage_tracker = OutputDamageTracker::from_output(&device.output);
                        device.frame_pending = false;
                    });
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
                        if let Err(error) = device.drm.activate(true) {
                            tracing::error!(%error, "failed to reactivate DRM device");
                        }
                    }
                }
                tracing::info!("direct session activated");
                render_all(state);
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
    let (mut drm, notifier) = DrmDevice::new(fd.clone(), true)?;
    let gbm = GbmDevice::new(fd)?;
    // SAFETY: GBM owns a valid DRM descriptor for the lifetime of the EGL display.
    let egl_display = unsafe { EGLDisplay::new(gbm.clone())? };
    let egl_context = EGLContext::new(&egl_display)?;
    // SAFETY: the new context is not current on another thread and remains renderer-owned.
    let renderer = unsafe { GlesRenderer::new(egl_context)? };
    state.shm_state.update_formats(renderer.shm_formats());
    let (connector, crtc, mode) = select_output(&drm)?;
    let output = create_output(state, &connector, mode);
    if let Some(backend) = state.direct_backend.as_mut() {
        backend
            .presentation
            .set_refresh(OutputMode::from(mode).refresh);
    }
    let drm_surface = drm.create_surface(crtc, mode, &[connector.handle()])?;
    let allocator = GbmAllocator::new(
        gbm.clone(),
        GbmBufferFlags::RENDERING | GbmBufferFlags::SCANOUT,
    );
    let surface = GbmBufferedSurface::new(
        drm_surface,
        allocator,
        &[Fourcc::Argb8888, Fourcc::Abgr8888],
        renderer.dmabuf_formats(),
    )?;
    let damage_tracker = OutputDamageTracker::from_output(&output);

    event_loop
        .handle()
        .insert_source(notifier, move |event, metadata, state| match event {
            DrmEvent::VBlank(crtc) => {
                if let Some(metadata) = metadata {
                    let feedback = state
                        .direct_backend
                        .as_mut()
                        .and_then(|backend| backend.devices.get_mut(&node))
                        .and_then(|device| match device.surface.frame_submitted() {
                            Ok(feedback) => feedback,
                            Err(error) => {
                                tracing::error!(?node, ?crtc, %error, "failed to retire DRM frame");
                                None
                            }
                        });
                    if let (Some(mut feedback), DrmEventTime::Monotonic(time)) =
                        (feedback, metadata.time)
                    {
                        feedback.presented(
                            Time::<Monotonic>::from(time),
                            Refresh::fixed(refresh_duration(state, node)),
                            u64::from(metadata.sequence),
                            Kind::Vsync | Kind::HwClock | Kind::HwCompletion,
                        );
                    }
                    if let Some(device) = state
                        .direct_backend
                        .as_mut()
                        .and_then(|backend| backend.devices.get_mut(&node))
                    {
                        device.frame_pending = false;
                    }
                    state.record_drm_presentation(metadata.time, metadata.sequence);
                    tracing::trace!(?node, ?crtc, sequence = metadata.sequence, "page flip");
                    render_device(state, node);
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
                surface,
                damage_tracker,
                frame_pending: false,
            },
        );
    state.relayout();
    render_device(state, node);
    tracing::info!(?node, ?path, ?crtc, connector = %connector, "initialized primary DRM/GBM device");
    Ok(())
}

pub fn render_all(state: &mut Ferese) {
    let nodes = state
        .direct_backend
        .as_ref()
        .map(|backend| backend.devices.keys().copied().collect::<Vec<_>>())
        .unwrap_or_default();

    for node in nodes {
        render_device(state, node);
    }
}

pub fn switch_vt(state: &mut Ferese, vt: i32) {
    let Some(backend) = state.direct_backend.as_mut() else {
        return;
    };

    if let Err(error) = backend.session.change_vt(vt) {
        tracing::error!(vt, %error, "failed to switch virtual terminal");
    }
}

fn render_device(state: &mut Ferese, node: DrmNode) {
    let Some(mut device) = state
        .direct_backend
        .as_mut()
        .and_then(|backend| backend.devices.remove(&node))
    else {
        return;
    };
    if !device.drm.is_active() || device.frame_pending {
        restore_device(state, node, device);
        return;
    }

    let rendered = (|| -> Result<bool, Box<dyn Error>> {
        let (mut buffer, age) = device.surface.next_buffer()?;
        let elements = animated_window_elements(state, &mut device.renderer, &device.output);
        let mut framebuffer = device.renderer.bind(&mut buffer)?;
        let result = device.damage_tracker.render_output(
            &mut device.renderer,
            &mut framebuffer,
            usize::from(age),
            &elements,
            [0.035, 0.04, 0.055, 1.0],
        )?;
        let Some(damage) = result.damage.cloned() else {
            return Ok(false);
        };

        let mut presentation = OutputPresentationFeedback::new(&device.output);
        state.space.elements().for_each(|window| {
            window.take_presentation_feedback(
                &mut presentation,
                |_, _| Some(device.output.clone()),
                |_, _| Kind::Vsync | Kind::HwClock | Kind::HwCompletion,
            );
        });
        device
            .surface
            .queue_buffer(Some(result.sync), Some(damage), presentation)?;
        device.frame_pending = true;
        Ok(true)
    })();

    let queued = match rendered {
        Ok(true) => {
            tracing::trace!(?node, "queued DRM frame");
            true
        }
        Ok(false) => false,
        Err(error) => {
            tracing::error!(?node, %error, "failed to render DRM frame");
            false
        }
    };
    restore_device(state, node, device);
    if queued {
        send_frame_callbacks(state);
    }
}

fn restore_device(state: &mut Ferese, node: DrmNode, device: DirectDevice) {
    if let Some(backend) = state.direct_backend.as_mut() {
        backend.devices.insert(node, device);
    }
}

fn refresh_duration(state: &Ferese, node: DrmNode) -> Duration {
    state
        .direct_backend
        .as_ref()
        .and_then(|backend| backend.devices.get(&node))
        .and_then(|device| device.output.current_mode())
        .filter(|mode| mode.refresh > 0)
        .map(|mode| Duration::from_nanos(1_000_000_000_000_u64 / mode.refresh as u64))
        .unwrap_or_else(|| Duration::from_millis(16))
}

fn send_frame_callbacks(state: &mut Ferese) {
    let Some(output) = state
        .direct_backend
        .as_ref()
        .and_then(|backend| backend.devices.values().next())
        .map(|device| device.output.clone())
    else {
        return;
    };

    state.space.elements().for_each(|window| {
        window.send_frame(
            &output,
            state.start_time.elapsed(),
            Some(Duration::ZERO),
            |_, _| Some(output.clone()),
        );
    });
    state.space.refresh();
    state.popups.cleanup();
    if let Err(error) = state.display_handle.flush_clients() {
        tracing::debug!(%error, "failed to flush clients after DRM frame");
    }
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
    fn reset_timing(&mut self) {
        self.last_presentation = None;
    }

    fn set_refresh(&mut self, refresh_millihertz: i32) {
        if refresh_millihertz > 0 {
            self.refresh_interval = Some(Duration::from_nanos(
                1_000_000_000_000_u64 / refresh_millihertz as u64,
            ));
        }
    }

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
        if let (Some(delta), Some(refresh)) = (delta, self.refresh_interval)
            && delta > refresh + refresh / 2
        {
            let elapsed_frames = (delta.as_nanos() / refresh.as_nanos()).max(1) as u64;
            let missed = elapsed_frames.saturating_sub(1);
            self.missed_deadlines = self.missed_deadlines.saturating_add(missed);
            tracing::warn!(
                sequence,
                ?delta,
                ?refresh,
                missed,
                total = self.missed_deadlines,
                "missed DRM presentation deadline"
            );
        }
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
        clock.set_refresh(60_000);

        assert_eq!(
            clock.record(DrmEventTime::Monotonic(Duration::from_millis(100)), 1),
            None
        );
        assert_eq!(
            clock.record(DrmEventTime::Monotonic(Duration::from_millis(116)), 2),
            Some(Duration::from_millis(16))
        );
        assert_eq!(clock.presented_frames, 2);
        assert_eq!(clock.missed_deadlines, 0);
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

    #[test]
    fn presentation_clock_counts_missed_deadlines() {
        let mut clock = PresentationClock::default();
        clock.set_refresh(60_000);

        clock.record(DrmEventTime::Monotonic(Duration::ZERO), 1);
        clock.record(DrmEventTime::Monotonic(Duration::from_millis(50)), 2);

        assert_eq!(clock.missed_deadlines, 2);
    }

    #[test]
    fn presentation_clock_excludes_inactive_session_time() {
        let mut clock = PresentationClock::default();
        clock.set_refresh(60_000);
        clock.record(DrmEventTime::Monotonic(Duration::from_millis(100)), 1);

        clock.reset_timing();

        assert_eq!(
            clock.record(DrmEventTime::Monotonic(Duration::from_secs(10)), 2),
            None
        );
        assert_eq!(clock.presented_frames, 2);
        assert_eq!(clock.missed_deadlines, 0);
    }
}
