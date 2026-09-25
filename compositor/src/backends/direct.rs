use std::{
    collections::{HashMap, HashSet},
    error::Error,
    io,
    path::Path,
    time::Duration,
};

use smithay::{
    backend::{
        allocator::{
            Fourcc,
            gbm::{GbmAllocator, GbmBufferFlags, GbmDevice},
        },
        drm::{DrmDevice, DrmDeviceFd, DrmEvent, DrmEventTime, DrmNode, GbmBufferedSurface},
        egl::{EGLContext, EGLDisplay},
        input::InputEvent,
        libinput::{LibinputInputBackend, LibinputSessionInterface},
        renderer::{
            Bind, Frame, ImportDma, ImportMemWl, Renderer, damage::OutputDamageTracker,
            gles::GlesRenderer,
        },
        session::{Event as SessionEvent, Session, libseat::LibSeatSession},
        udev::{UdevBackend, UdevEvent, primary_gpu},
    },
    desktop::{layer_map_for_output, utils::OutputPresentationFeedback},
    output::{Mode as OutputMode, Output, PhysicalProperties, Scale, Subpixel},
    reexports::{
        calloop::EventLoop,
        drm::control::{
            Device as ControlDevice, Mode as DrmMode, ModeTypeFlags, connector, crtc, property,
        },
        input::{Device as LibinputDevice, Libinput},
        rustix::fs::OFlags,
        wayland_protocols::wp::presentation_time::server::wp_presentation_feedback::Kind,
        wayland_server::backend::GlobalId,
    },
    utils::{DeviceFd, Monotonic, Time, Transform},
    wayland::presentation::Refresh,
};

use crate::{
    Ferese,
    winit::{animated_window_elements, cursorless_window_elements, redraw_output},
};

pub struct DirectBackendState {
    pub session: LibSeatSession,
    pub active: bool,
    devices: HashMap<DrmNode, DirectDevice>,
    presentation: HashMap<(DrmNode, crtc::Handle), PresentationClock>,
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
    gbm: GbmDevice<DrmDeviceFd>,
    renderer: GlesRenderer,
    outputs: HashMap<crtc::Handle, DirectOutput>,
}

struct DirectOutput {
    connector: connector::Handle,
    mode: DrmMode,
    output: Output,
    global: GlobalId,
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
        presentation: HashMap::new(),
    });
    open_primary_device(event_loop, state, &primary_path)?;

    event_loop
        .handle()
        .insert_source(libinput_backend, |event, _, state| match event {
            InputEvent::DeviceAdded { mut device } => configure_libinput_device(state, &mut device),
            event => state.process_input_event(event),
        })?;
    event_loop
        .handle()
        .insert_source(notifier, move |event, _, state| match event {
            SessionEvent::PauseSession => {
                if let Some(backend) = state.direct_backend.as_mut() {
                    backend.active = false;
                    backend
                        .presentation
                        .values_mut()
                        .for_each(PresentationClock::reset_timing);
                    backend.devices.values_mut().for_each(|device| {
                        device.drm.pause();
                        device.outputs.values_mut().for_each(|output| {
                            output.surface.reset_buffers();
                            output.damage_tracker =
                                OutputDamageTracker::from_output(&output.output);
                            output.frame_pending = false;
                        });
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
        .insert_source(udev_backend, |event, _, state| match event {
            UdevEvent::Added { device_id, path } => {
                if let Some(node) = direct_node_for_device(state, device_id) {
                    rescan_device(state, node);
                } else {
                    tracing::info!(?device_id, ?path, "additional DRM device discovered");
                }
            }
            UdevEvent::Changed { device_id } => {
                if let Some(node) = direct_node_for_device(state, device_id) {
                    rescan_device(state, node);
                }
            }
            UdevEvent::Removed { device_id } => {
                if let Some(node) = direct_node_for_device(state, device_id) {
                    remove_device(state, node);
                }
            }
        })?;

    tracing::info!(seat = %seat_name, "initialized direct session input and device discovery");
    Ok(())
}

fn configure_libinput_device(state: &Ferese, device: &mut LibinputDevice) {
    let settings = state.input_settings.touchpad;

    if device.config_tap_finger_count() > 0
        && let Err(error) = device.config_tap_set_enabled(settings.tap)
    {
        tracing::warn!(
            device = device.name(),
            ?error,
            "failed to configure tap-to-click"
        );
    }
    if device.config_scroll_has_natural_scroll()
        && let Err(error) = device.config_scroll_set_natural_scroll_enabled(settings.natural_scroll)
    {
        tracing::warn!(
            device = device.name(),
            ?error,
            "failed to configure natural scrolling"
        );
    }
    if device.config_dwt_is_available()
        && let Err(error) = device.config_dwt_set_enabled(settings.disable_while_typing)
    {
        tracing::warn!(
            device = device.name(),
            ?error,
            "failed to configure disable-while-typing"
        );
    }
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
    let dmabuf_formats = renderer.dmabuf_formats();
    let display_handle = state.display_handle.clone();
    state
        .dmabuf_state
        .create_global::<Ferese>(&display_handle, dmabuf_formats);
    let selections = select_outputs(&drm)?;
    let mut outputs = HashMap::new();
    for (connector, crtc, mode) in selections {
        let output = create_direct_output(state, &mut drm, &gbm, &renderer, connector, crtc, mode)?;
        state
            .direct_backend
            .as_mut()
            .expect("direct backend state remains initialized")
            .presentation
            .entry((node, crtc))
            .or_default()
            .set_refresh(OutputMode::from(mode).refresh);
        outputs.insert(crtc, output);
    }

    event_loop
        .handle()
        .insert_source(notifier, move |event, metadata, state| match event {
            DrmEvent::VBlank(crtc) => {
                if let Some(metadata) = metadata {
                    let feedback = state
                        .direct_backend
                        .as_mut()
                        .and_then(|backend| backend.devices.get_mut(&node))
                        .and_then(|device| device.outputs.get_mut(&crtc))
                        .and_then(|output| match output.surface.frame_submitted() {
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
                            Refresh::fixed(refresh_duration(state, node, crtc)),
                            u64::from(metadata.sequence),
                            Kind::Vsync | Kind::HwClock | Kind::HwCompletion,
                        );
                    }
                    let focused = state
                        .direct_backend
                        .as_ref()
                        .and_then(|backend| backend.devices.get(&node))
                        .and_then(|device| device.outputs.get(&crtc))
                        .is_some_and(|output| state.is_focused_output(&output.output));
                    if let Some(output) = state
                        .direct_backend
                        .as_mut()
                        .and_then(|backend| backend.devices.get_mut(&node))
                        .and_then(|device| device.outputs.get_mut(&crtc))
                    {
                        output.frame_pending = false;
                    }
                    if focused {
                        state.record_drm_presentation(node, crtc, metadata.time, metadata.sequence);
                    }
                    tracing::trace!(?node, ?crtc, sequence = metadata.sequence, "page flip");
                    render_output(state, node, crtc);
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
                outputs,
            },
        );
    state.relayout();
    render_all(state);
    tracing::info!(?node, ?path, "initialized DRM/GBM device outputs");
    Ok(())
}

pub fn render_all(state: &mut Ferese) {
    let outputs = state
        .direct_backend
        .as_ref()
        .map(|backend| {
            backend
                .devices
                .iter()
                .flat_map(|(node, device)| device.outputs.keys().map(|crtc| (*node, *crtc)))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    for (node, crtc) in outputs {
        render_output(state, node, crtc);
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

fn render_output(state: &mut Ferese, node: DrmNode, crtc: crtc::Handle) {
    let Some(mut device) = state
        .direct_backend
        .as_mut()
        .and_then(|backend| backend.devices.remove(&node))
    else {
        return;
    };
    let Some(mut output) = device.outputs.remove(&crtc) else {
        restore_device(state, node, device);
        return;
    };
    if !device.drm.is_active() || output.frame_pending {
        device.outputs.insert(crtc, output);
        restore_device(state, node, device);
        return;
    }

    let rendered = (|| -> Result<bool, Box<dyn Error>> {
        state.process_dmabuf_imports(&mut device.renderer);
        let (mut buffer, age) = output.surface.next_buffer()?;
        let elements = animated_window_elements(state, &mut device.renderer, &output.output);
        let mut framebuffer = device.renderer.bind(&mut buffer)?;
        let result = output.damage_tracker.render_output(
            &mut device.renderer,
            &mut framebuffer,
            usize::from(age),
            &elements,
            [0.035, 0.04, 0.055, 1.0],
        )?;
        let cursorless_capture = state.has_pending_screencopy(&output.output, false);
        let captured_with_cursor =
            state.process_screencopies(&mut device.renderer, &framebuffer, &output.output, true);

        if cursorless_capture {
            let cursorless_elements =
                cursorless_window_elements(state, &mut device.renderer, &output.output);
            redraw_output(
                &mut device.renderer,
                &mut framebuffer,
                &output.output,
                &cursorless_elements,
            )?;
            state.process_screencopies(&mut device.renderer, &framebuffer, &output.output, false);
            redraw_output(
                &mut device.renderer,
                &mut framebuffer,
                &output.output,
                &elements,
            )?;
        } else if captured_with_cursor {
            let _ = device
                .renderer
                .render(
                    &mut framebuffer,
                    output
                        .output
                        .current_mode()
                        .expect("output has a mode")
                        .size,
                    output.output.current_transform(),
                )?
                .finish()?;
        }
        let Some(damage) = result.damage.cloned() else {
            return Ok(false);
        };

        let mut presentation = OutputPresentationFeedback::new(&output.output);
        state.space.elements().for_each(|window| {
            window.take_presentation_feedback(
                &mut presentation,
                |_, _| Some(output.output.clone()),
                |_, _| Kind::Vsync | Kind::HwClock | Kind::HwCompletion,
            );
        });
        let layers = layer_map_for_output(&output.output)
            .layers()
            .cloned()
            .collect::<Vec<_>>();
        layers.iter().for_each(|layer| {
            layer.take_presentation_feedback(
                &mut presentation,
                |_, _| Some(output.output.clone()),
                |_, _| Kind::Vsync | Kind::HwClock | Kind::HwCompletion,
            );
        });
        output
            .surface
            .queue_buffer(Some(result.sync), Some(damage), presentation)?;
        output.frame_pending = true;
        Ok(true)
    })();

    let queued = match rendered {
        Ok(true) => {
            tracing::trace!(?node, ?crtc, "queued DRM frame");
            true
        }
        Ok(false) => false,
        Err(error) => {
            tracing::error!(?node, ?crtc, %error, "failed to render DRM frame");
            false
        }
    };
    let callback_output = output.output.clone();
    device.outputs.insert(crtc, output);
    restore_device(state, node, device);
    if queued {
        send_frame_callbacks(state, &callback_output);
    }
}

fn restore_device(state: &mut Ferese, node: DrmNode, device: DirectDevice) {
    if let Some(backend) = state.direct_backend.as_mut() {
        backend.devices.insert(node, device);
    }
}

fn direct_node_for_device(state: &Ferese, device_id: libc::dev_t) -> Option<DrmNode> {
    state
        .direct_backend
        .as_ref()?
        .devices
        .keys()
        .copied()
        .find(|node| node.dev_id() == device_id)
}

fn rescan_device(state: &mut Ferese, node: DrmNode) {
    let Some(mut device) = state
        .direct_backend
        .as_mut()
        .and_then(|backend| backend.devices.remove(&node))
    else {
        return;
    };

    let selections = match select_outputs(&device.drm) {
        Ok(selections) => selections,
        Err(error) if error.kind() == io::ErrorKind::NotFound => Vec::new(),
        Err(error) => {
            tracing::error!(?node, %error, "failed to scan DRM connectors");
            restore_device(state, node, device);
            return;
        }
    };
    let mut selections = selections
        .into_iter()
        .map(|selection @ (_, crtc, _)| (crtc, selection))
        .collect::<HashMap<_, _>>();
    let existing = device.outputs.keys().copied().collect::<Vec<_>>();

    for crtc in existing {
        let unchanged = device.outputs.get(&crtc).is_some_and(|output| {
            selections.get(&crtc).is_some_and(|(connector, _, mode)| {
                connector.handle() == output.connector && *mode == output.mode
            })
        });
        if unchanged {
            selections.remove(&crtc);
            continue;
        }

        if let Some(output) = device.outputs.remove(&crtc) {
            state.display_handle.disable_global::<Ferese>(output.global);
            state.unregister_output(&output.output);
            if let Some(backend) = state.direct_backend.as_mut() {
                backend.presentation.remove(&(node, crtc));
            }
            tracing::info!(?node, ?crtc, "removed DRM output");
        }
    }

    for (_, (connector, crtc, mode)) in selections {
        match create_direct_output(
            state,
            &mut device.drm,
            &device.gbm,
            &device.renderer,
            connector,
            crtc,
            mode,
        ) {
            Ok(output) => {
                state
                    .direct_backend
                    .as_mut()
                    .expect("direct backend state remains initialized")
                    .presentation
                    .entry((node, crtc))
                    .or_default()
                    .set_refresh(OutputMode::from(mode).refresh);
                device.outputs.insert(crtc, output);
            }
            Err(error) => tracing::error!(?node, ?crtc, %error, "failed to add DRM output"),
        }
    }

    restore_device(state, node, device);
    state.relayout();
    render_all(state);
}

fn remove_device(state: &mut Ferese, node: DrmNode) {
    let Some(device) = state
        .direct_backend
        .as_mut()
        .and_then(|backend| backend.devices.remove(&node))
    else {
        return;
    };

    for (crtc, output) in device.outputs {
        state.display_handle.disable_global::<Ferese>(output.global);
        state.unregister_output(&output.output);
        if let Some(backend) = state.direct_backend.as_mut() {
            backend.presentation.remove(&(node, crtc));
        }
    }
    tracing::info!(?node, "removed DRM device");
}

fn refresh_duration(state: &Ferese, node: DrmNode, crtc: crtc::Handle) -> Duration {
    state
        .direct_backend
        .as_ref()
        .and_then(|backend| backend.devices.get(&node))
        .and_then(|device| device.outputs.get(&crtc))
        .and_then(|output| output.output.current_mode())
        .filter(|mode| mode.refresh > 0)
        .map(|mode| Duration::from_nanos(1_000_000_000_000_u64 / mode.refresh as u64))
        .unwrap_or_else(|| Duration::from_millis(16))
}

fn send_frame_callbacks(state: &mut Ferese, output: &Output) {
    state.space.elements().for_each(|window| {
        window.send_frame(
            output,
            state.start_time.elapsed(),
            Some(Duration::ZERO),
            |_, _| Some(output.clone()),
        );
    });
    let layers = layer_map_for_output(output)
        .layers()
        .cloned()
        .collect::<Vec<_>>();
    layers.iter().for_each(|layer| {
        layer.send_frame(
            output,
            state.start_time.elapsed(),
            Some(Duration::ZERO),
            |_, _| Some(output.clone()),
        );
    });
    state.send_cursor_frame(output);
    state.space.refresh();
    state.popups.cleanup();
    layer_map_for_output(output).cleanup();
    if let Err(error) = state.display_handle.flush_clients() {
        tracing::debug!(%error, "failed to flush clients after DRM frame");
    }
}

fn select_outputs(drm: &DrmDevice) -> io::Result<Vec<(connector::Info, crtc::Handle, DrmMode)>> {
    let resources = drm.resource_handles()?;
    let mut selections = Vec::new();
    let mut used_crtcs = HashSet::new();

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
            .and_then(|encoder| encoder.crtc())
            .filter(|crtc| !used_crtcs.contains(crtc));
        let compatible_crtc = connector.encoders().iter().find_map(|handle| {
            let encoder = drm.get_encoder(*handle).ok()?;
            resources
                .filter_crtcs(encoder.possible_crtcs())
                .into_iter()
                .find(|crtc| !used_crtcs.contains(crtc))
        });
        if let Some(crtc) = current_crtc.or(compatible_crtc) {
            used_crtcs.insert(crtc);
            selections.push((connector, crtc, mode));
        }
    }

    if selections.is_empty() {
        Err(io::Error::new(
            io::ErrorKind::NotFound,
            "no connected desktop DRM connector with a usable CRTC",
        ))
    } else {
        Ok(selections)
    }
}

fn create_direct_output(
    state: &mut Ferese,
    drm: &mut DrmDevice,
    gbm: &GbmDevice<DrmDeviceFd>,
    renderer: &GlesRenderer,
    connector: connector::Info,
    crtc: crtc::Handle,
    mode: DrmMode,
) -> Result<DirectOutput, Box<dyn Error>> {
    let identity = connector_identity(drm, &connector);
    let (output, global) = create_output(state, &connector, mode, identity);
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

    tracing::info!(?crtc, connector = %connector, "initialized DRM output");
    Ok(DirectOutput {
        connector: connector.handle(),
        mode,
        output,
        global,
        surface,
        damage_tracker,
        frame_pending: false,
    })
}

fn create_output(
    state: &mut Ferese,
    connector: &connector::Info,
    mode: DrmMode,
    identity: String,
) -> (Output, GlobalId) {
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

    let global = output.create_global::<Ferese>(&state.display_handle);
    output.set_preferred(output_mode);
    output.change_current_state(
        Some(output_mode),
        Some(Transform::Normal),
        Some(Scale::Integer(1)),
        Some((0, 0).into()),
    );
    let x = state
        .space
        .outputs()
        .filter_map(|output| state.space.output_geometry(output))
        .map(|geometry| geometry.loc.x + geometry.size.w)
        .max()
        .unwrap_or(0);
    state.space.map_output(&output, (x, 0));
    state.register_output(&output, identity);
    (output, global)
}

fn connector_identity(drm: &DrmDevice, connector: &connector::Info) -> String {
    let edid = drm
        .get_properties(connector.handle())
        .ok()
        .and_then(|properties| {
            properties.iter().find_map(|(handle, raw)| {
                let info = drm.get_property(*handle).ok()?;
                if info.name().to_bytes() != b"EDID" {
                    return None;
                }
                match info.value_type().convert_value(*raw) {
                    property::Value::Blob(blob) if blob != 0 => drm.get_property_blob(blob).ok(),
                    _ => None,
                }
            })
        });

    match edid {
        Some(edid) if !edid.is_empty() => {
            format!("drm:{}:{:016x}", connector, stable_hash(&edid))
        }
        _ => format!("drm:{connector}"),
    }
}

fn stable_hash(bytes: &[u8]) -> u64 {
    const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

    bytes.iter().fold(FNV_OFFSET, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(FNV_PRIME)
    })
}

impl DirectBackendState {
    pub fn record_presentation(
        &mut self,
        node: DrmNode,
        crtc: crtc::Handle,
        time: DrmEventTime,
        sequence: u32,
    ) -> Option<Duration> {
        self.presentation
            .get_mut(&(node, crtc))
            .and_then(|clock| clock.record(time, sequence))
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
    fn connector_identity_hash_is_stable() {
        assert_eq!(stable_hash(b"hello"), 0xa430_d846_80aa_bd0b);
    }

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
