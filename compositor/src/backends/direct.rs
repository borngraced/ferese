use std::{
    collections::{HashMap, HashSet},
    error::Error,
    io,
    path::Path,
    time::{Duration, Instant},
};

use smithay::{
    backend::{
        allocator::{
            Fourcc,
            gbm::{GbmAllocator, GbmBufferFlags, GbmDevice},
        },
        drm::{
            DrmDevice, DrmDeviceFd, DrmEvent, DrmEventTime, DrmNode, GbmBufferedSurface, NodeType,
        },
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
    wayland::{dmabuf::DmabufFeedbackBuilder, presentation::Refresh},
};

use crate::{
    Ferese,
    config::{OutputModeRequest, OutputProfile, OutputSettings, OutputTransform},
    metrics::RenderMetrics,
    winit::{
        animated_window_elements, cursorless_window_elements, frame_effect_metrics, redraw_output,
    },
};

pub struct DirectBackendState {
    pub session: LibSeatSession,
    pub active: bool,
    lid_closed: bool,
    devices: HashMap<DrmNode, DirectDevice>,
    input_devices: Vec<LibinputDevice>,
    presentation: HashMap<(DrmNode, crtc::Handle), PresentationClock>,
    pub(crate) connected_outputs: Vec<ConnectedOutputInfo>,
}

impl DirectBackendState {
    pub(crate) fn capture_resize_snapshot(
        &mut self,
        window: &smithay::desktop::Window,
        geometry: smithay::utils::Rectangle<i32, smithay::utils::Logical>,
        output: &Output,
        remaining: usize,
    ) -> Result<Option<crate::winit::ResizeSnapshot>, smithay::backend::renderer::gles::GlesError>
    {
        let Some(device) = self.devices.values_mut().find(|device| {
            device
                .outputs
                .values()
                .any(|candidate| &candidate.output == output)
        }) else {
            return Ok(None);
        };
        crate::winit::capture_resize_snapshot(
            &mut device.renderer,
            window,
            geometry,
            output.current_scale().fractional_scale(),
            remaining,
        )
    }

    pub(crate) fn capture_window_buffer(
        &mut self,
        window: &smithay::desktop::Window,
        geometry: smithay::utils::Rectangle<i32, smithay::utils::Logical>,
        output: &Output,
    ) -> Result<crate::handlers::screenshot::CaptureBuffer, String> {
        let device = self
            .devices
            .values_mut()
            .find(|device| {
                device
                    .outputs
                    .values()
                    .any(|candidate| &candidate.output == output)
            })
            .ok_or("Window output is unavailable")?;
        crate::winit::capture_window_buffer(
            &mut device.renderer,
            window,
            geometry,
            output.current_scale().fractional_scale(),
        )
    }
    pub(crate) fn capture_window_frame(
        &mut self,
        state: &Ferese,
        window: &smithay::desktop::Window,
        output: &Output,
        cursor: Option<smithay::utils::Rectangle<i32, smithay::utils::Logical>>,
    ) -> Result<crate::handlers::screenshot::CaptureBuffer, String> {
        let device = self
            .devices
            .values_mut()
            .find(|device| {
                device
                    .outputs
                    .values()
                    .any(|candidate| &candidate.output == output)
            })
            .ok_or("Window output is unavailable")?;
        crate::winit::capture_window_frame(
            &mut device.renderer,
            window,
            window.geometry(),
            output.current_scale().fractional_scale(),
            cursor.map(|rect| (state, rect)),
        )
    }
}

#[derive(Clone, Debug)]
pub(crate) struct ConnectedOutputInfo {
    pub connector: String,
    pub identity: String,
    pub enabled: bool,
    pub profile: Option<String>,
    pub physical_size: Option<(u32, u32)>,
    pub current_mode: Option<ConnectedModeInfo>,
    pub available_modes: Vec<ConnectedModeInfo>,
    pub scale: f64,
    pub transform: OutputTransform,
    pub configured_position: Option<[i32; 2]>,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct ConnectedModeInfo {
    pub width: u16,
    pub height: u16,
    pub refresh_millihertz: i32,
    pub preferred: bool,
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
    internal: bool,
    connector: connector::Handle,
    mode: DrmMode,
    settings: OutputSettings,
    output: Output,
    global: GlobalId,
    surface: GbmBufferedSurface<GbmAllocator<DrmDeviceFd>, OutputPresentationFeedback>,
    damage_tracker: OutputDamageTracker,
    render_metrics: RenderMetrics,
    frame_pending: bool,
    lock_frame_pending: bool,
}

struct OutputSelection {
    connector: connector::Info,
    crtc: crtc::Handle,
    mode: DrmMode,
    settings: OutputSettings,
}

struct OutputScan {
    selections: Vec<OutputSelection>,
    connected_outputs: Vec<ConnectedOutputInfo>,
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
        lid_closed: initial_lid_closed(),
        devices: HashMap::new(),
        input_devices: Vec::new(),
        presentation: HashMap::new(),
        connected_outputs: Vec::new(),
    });
    open_primary_device(event_loop, state, &primary_path)?;

    event_loop
        .handle()
        .insert_source(libinput_backend, |event, _, state| match event {
            InputEvent::DeviceAdded { mut device } => {
                configure_libinput_device(state, &mut device);
                if let Some(backend) = state.direct_backend.as_mut() {
                    backend.input_devices.push(device);
                }
            }
            InputEvent::DeviceRemoved { device } => {
                if let Some(backend) = state.direct_backend.as_mut() {
                    backend
                        .input_devices
                        .retain(|candidate| candidate != &device);
                }
            }
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

pub(crate) fn reload_input_devices(state: &mut Ferese) {
    let Some(backend) = state.direct_backend.as_ref() else {
        return;
    };
    // Device clones refer to the same live libinput objects.
    let mut devices = backend.input_devices.clone();
    for device in &mut devices {
        configure_libinput_device(state, device);
    }
}

pub(crate) fn reload_outputs(state: &mut Ferese) {
    let nodes = state
        .direct_backend
        .as_ref()
        .map(|backend| backend.devices.keys().copied().collect::<Vec<_>>())
        .unwrap_or_default();
    for node in nodes {
        rescan_device(state, node);
    }
}

pub(crate) fn validate_live_outputs(
    state: &Ferese,
    profiles: &[OutputProfile],
) -> Result<(), String> {
    let Some(backend) = state.direct_backend.as_ref() else {
        return Ok(());
    };
    let mut usable = 0;
    for device in backend.devices.values() {
        let mut scan = select_outputs(&device.drm, profiles).map_err(|e| e.to_string())?;
        apply_lid_policy(&mut scan, backend.lid_closed);
        usable += scan.selections.len();
    }
    if usable == 0 {
        return Err("display configuration would leave no usable output".into());
    }
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
    let dmabuf_formats = renderer.dmabuf_formats();
    let display_handle = state.display_handle.clone();
    // Mesa's Wayland EGL path needs the main device, not just a v3 format
    // list, to choose a hardware render node. Without feedback, nested EGL
    // clients can fall back to llvmpipe despite a GPU-backed DRM compositor.
    let render_node = node
        .node_with_type(NodeType::Render)
        .and_then(Result::ok)
        .unwrap_or(node);
    let feedback = DmabufFeedbackBuilder::new(render_node.dev_id(), dmabuf_formats).build()?;
    state
        .dmabuf_state
        .create_global_with_default_feedback::<Ferese>(&display_handle, &feedback);
    let mut scan = select_outputs(&drm, &state.output_profiles)?;
    apply_lid_policy(
        &mut scan,
        state
            .direct_backend
            .as_ref()
            .is_some_and(|backend| backend.lid_closed),
    );
    if scan.selections.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            "no connected enabled DRM output with a usable CRTC",
        )
        .into());
    }
    state
        .direct_backend
        .as_mut()
        .expect("direct backend state remains initialized")
        .connected_outputs = scan.connected_outputs;
    let mut outputs = HashMap::new();
    for selection in scan.selections {
        let OutputSelection {
            connector,
            crtc,
            mode,
            settings,
        } = selection;
        let output = create_direct_output(
            state, &mut drm, &gbm, &renderer, connector, crtc, mode, &settings,
        )?;
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
                    let mut retired = false;
                    let feedback = state
                        .direct_backend
                        .as_mut()
                        .and_then(|backend| backend.devices.get_mut(&node))
                        .and_then(|device| device.outputs.get_mut(&crtc))
                        .and_then(|output| match output.surface.frame_submitted() {
                            Ok(feedback) => {
                                retired = true;
                                feedback
                            }
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
                    let lock_output = state
                        .direct_backend
                        .as_mut()
                        .and_then(|backend| backend.devices.get_mut(&node))
                        .and_then(|device| device.outputs.get_mut(&crtc))
                        .and_then(|output| {
                            if retired && std::mem::take(&mut output.lock_frame_pending) {
                                Some(output.output.clone())
                            } else {
                                None
                            }
                        });
                    if let Some(output) = lock_output {
                        state.lock_frame_presented(&output);
                    }
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

pub(crate) fn set_lid_closed(state: &mut Ferese, closed: bool) {
    let Some(backend) = state.direct_backend.as_mut() else {
        return;
    };
    if backend.lid_closed == closed {
        return;
    }
    backend.lid_closed = closed;
    tracing::info!(closed, "laptop lid state changed");
    let nodes = backend.devices.keys().copied().collect::<Vec<_>>();
    for node in nodes {
        rescan_device(state, node);
    }
}

fn initial_lid_closed() -> bool {
    std::fs::read_dir("/proc/acpi/button/lid")
        .ok()
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .filter_map(|entry| std::fs::read_to_string(entry.path().join("state")).ok())
        .any(|state| state.split_whitespace().last() == Some("closed"))
}

fn internal_connector(interface: connector::Interface) -> bool {
    matches!(
        interface,
        connector::Interface::EmbeddedDisplayPort
            | connector::Interface::LVDS
            | connector::Interface::DSI
    )
}

fn apply_lid_policy(scan: &mut OutputScan, closed: bool) {
    let external_available = scan
        .selections
        .iter()
        .any(|selection| !internal_connector(selection.connector.interface()));
    if !lid_hides_panel(closed, external_available) {
        return;
    }
    let internal = scan
        .selections
        .iter()
        .filter(|selection| internal_connector(selection.connector.interface()))
        .map(|selection| selection.connector.to_string())
        .collect::<HashSet<_>>();
    scan.selections
        .retain(|selection| !internal_connector(selection.connector.interface()));
    for output in &mut scan.connected_outputs {
        if internal.contains(&output.connector) {
            output.enabled = false;
            output.current_mode = None;
        }
    }
}

fn lid_hides_panel(closed: bool, external_available: bool) -> bool {
    closed && external_available
}

fn render_output(state: &mut Ferese, node: DrmNode, crtc: crtc::Handle) {
    let missed_deadlines = state
        .direct_backend
        .as_ref()
        .and_then(|backend| backend.presentation.get(&(node, crtc)))
        .map_or(0, |clock| clock.missed_deadlines);
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

    let render_started = Instant::now();
    let rendered = (|| -> Result<bool, Box<dyn Error>> {
        state.process_dmabuf_imports(&mut device.renderer);
        let (mut buffer, age) = output.surface.next_buffer()?;
        let elements = animated_window_elements(state, &mut device.renderer, &output.output);
        let effects =
            frame_effect_metrics(&elements, output.output.current_scale().fractional_scale());
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
            output
                .render_metrics
                .record_no_damage(render_started.elapsed(), missed_deadlines);
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
            .queue_buffer(Some(result.sync), Some(damage.clone()), presentation)?;
        output.render_metrics.record_frame(
            render_started.elapsed(),
            &damage,
            missed_deadlines,
            effects,
        );
        output.frame_pending = true;
        output.lock_frame_pending = state.session_lock.active;
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

    let mut scan = match select_outputs(&device.drm, &state.output_profiles) {
        Ok(scan) => scan,
        Err(error) => {
            tracing::error!(?node, %error, "failed to scan DRM connectors");
            restore_device(state, node, device);
            return;
        }
    };
    let lid_closed = state
        .direct_backend
        .as_ref()
        .is_some_and(|backend| backend.lid_closed);
    apply_lid_policy(&mut scan, lid_closed);
    if let Some(backend) = state.direct_backend.as_mut() {
        backend.connected_outputs = scan.connected_outputs;
    }
    let mut selections = scan
        .selections
        .into_iter()
        .map(|selection| (selection.crtc, selection))
        .collect::<HashMap<_, _>>();
    let existing = device.outputs.keys().copied().collect::<Vec<_>>();
    let mut deferred_removals = Vec::new();

    for crtc in existing {
        if let Some(selection) = selections.get(&crtc)
            && let Some(output) = device.outputs.get_mut(&crtc)
            && selection.connector.handle() == output.connector
            && (selection.mode != output.mode || selection.settings != output.settings)
        {
            // Keep the wl_output/global and workspace ownership alive. A mode
            // change is tested by DRM before updating client-visible geometry.
            let result = if selection.mode != output.mode {
                output.surface.use_mode(selection.mode)
            } else {
                Ok(())
            };
            match result {
                Ok(()) => {
                    let old_geometry = state.space.output_geometry(&output.output);
                    output.mode = selection.mode;
                    output.settings = selection.settings.clone();
                    let position = selection.settings.position.unwrap_or_else(|| {
                        state
                            .space
                            .output_geometry(&output.output)
                            .map(|g| [g.loc.x, g.loc.y])
                            .unwrap_or([0, 0])
                    });
                    output.output.change_current_state(
                        Some(OutputMode::from(output.mode)),
                        Some(output_transform(output.settings.transform)),
                        Some(Scale::Fractional(output.settings.scale)),
                        Some((position[0], position[1]).into()),
                    );
                    state
                        .space
                        .map_output(&output.output, (position[0], position[1]));
                    if let Some(id) = state.output_id(&output.output)
                        && let Some(g) = state.space.output_geometry(&output.output)
                    {
                        state.output_workspaces.update_geometry(
                            id,
                            ferese_core::OutputGeometry::new(g.loc.x, g.loc.y, g.size.w, g.size.h),
                        );
                        if let Some(old) = old_geometry {
                            state.reposition_output_floats(
                                id,
                                ferese_layout::Rect::new(
                                    old.loc.x as f64,
                                    old.loc.y as f64,
                                    old.size.w as f64,
                                    old.size.h as f64,
                                ),
                                ferese_layout::Rect::new(
                                    g.loc.x as f64,
                                    g.loc.y as f64,
                                    g.size.w as f64,
                                    g.size.h as f64,
                                ),
                            );
                        }
                    }
                    output.damage_tracker = OutputDamageTracker::from_output(&output.output);
                    if let Some(backend) = state.direct_backend.as_mut() {
                        let clock = backend.presentation.entry((node, crtc)).or_default();
                        clock.set_refresh(OutputMode::from(output.mode).refresh);
                        clock.reset_timing();
                    }
                }
                Err(error) => {
                    tracing::warn!(?crtc, %error, "display change failed; retaining current output")
                }
            }
            selections.remove(&crtc);
            continue;
        }
        let unchanged = device.outputs.get(&crtc).is_some_and(|output| {
            selections.get(&crtc).is_some_and(|selection| {
                selection.connector.handle() == output.connector
                    && selection.mode == output.mode
                    && selection.settings == output.settings
            })
        });
        if unchanged {
            selections.remove(&crtc);
            continue;
        }

        // Add replacements on free CRTCs before removing vanished outputs so
        // workspace evacuation always has a real destination. Reused CRTCs
        // still need their old surface released first.
        if !selections.contains_key(&crtc) {
            deferred_removals.push(crtc);
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

    for (_, selection) in selections {
        let OutputSelection {
            connector,
            crtc,
            mode,
            settings,
        } = selection;
        match create_direct_output(
            state,
            &mut device.drm,
            &device.gbm,
            &device.renderer,
            connector,
            crtc,
            mode,
            &settings,
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

    for crtc in deferred_removals {
        let keep_internal = device
            .outputs
            .get(&crtc)
            .is_some_and(|output| output.internal)
            && lid_closed
            && state.direct_backend.as_ref().is_some_and(|backend| {
                device.outputs.get(&crtc).is_some_and(|output| {
                    backend
                        .connected_outputs
                        .iter()
                        .any(|info| info.connector == output.output.name())
                })
            })
            && !device.outputs.values().any(|output| !output.internal);
        if keep_internal {
            tracing::warn!(
                ?crtc,
                "keeping laptop panel enabled: external output activation failed"
            );
            if let Some(output) = device.outputs.get(&crtc)
                && let Some(backend) = state.direct_backend.as_mut()
                && let Some(info) = backend
                    .connected_outputs
                    .iter_mut()
                    .find(|info| info.connector == output.output.name())
            {
                info.enabled = true;
                info.current_mode = Some(connected_mode_info(output.mode));
            }
            continue;
        }
        if let Some(output) = device.outputs.remove(&crtc) {
            state.display_handle.disable_global::<Ferese>(output.global);
            state.unregister_output(&output.output);
            if let Some(backend) = state.direct_backend.as_mut() {
                backend.presentation.remove(&(node, crtc));
            }
        }
    }
    if let Some(backend) = state.direct_backend.as_mut() {
        for output in device.outputs.values() {
            if let Some(info) = backend
                .connected_outputs
                .iter_mut()
                .find(|info| info.connector == output.output.name())
            {
                info.enabled = true;
                info.current_mode = Some(connected_mode_info(output.mode));
                info.scale = output.settings.scale;
                info.transform = output.settings.transform;
                info.configured_position = output.settings.position;
            }
        }
    }
    restore_device(state, node, device);
    state.restore_output_focus();
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

    let context = device.renderer.context_id().erased();
    state.wallpaper.forget_context(&context);
    state
        .resize_snapshots
        .retain(|_, snapshot| snapshot.context != context);
    for (crtc, output) in device.outputs {
        state.display_handle.disable_global::<Ferese>(output.global);
        state.unregister_output(&output.output);
        if let Some(backend) = state.direct_backend.as_mut() {
            backend.presentation.remove(&(node, crtc));
        }
    }
    if let Some(backend) = state.direct_backend.as_mut() {
        backend.connected_outputs.clear();
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
    if state.session_lock.active {
        state.lock_frame_callbacks(output);
        return;
    }
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

fn select_outputs(drm: &DrmDevice, profiles: &[OutputProfile]) -> io::Result<OutputScan> {
    let resources = drm.resource_handles()?;
    let connected = resources
        .connectors()
        .iter()
        .map(|handle| drm.get_connector(*handle, true))
        .collect::<io::Result<Vec<_>>>()?
        .into_iter()
        .filter(|connector| connector.state() == connector::State::Connected)
        .collect::<Vec<_>>();
    let identities = connected
        .iter()
        .map(|connector| connector_identity(drm, connector))
        .collect::<Vec<_>>();
    let active_profile = profiles.iter().find(|profile| {
        profile.outputs.iter().all(|settings| {
            connected
                .iter()
                .zip(&identities)
                .any(|(connector, identity)| output_matches(settings, connector, identity))
        })
    });
    if let Some(profile) = active_profile {
        tracing::info!(profile = %profile.name, "selected output profile");
    }

    let mut selections = Vec::new();
    let mut connected_outputs = Vec::new();
    let mut used_crtcs = HashSet::new();

    for (connector, identity) in connected.into_iter().zip(identities) {
        let settings = active_profile
            .and_then(|profile| {
                profile
                    .outputs
                    .iter()
                    .find(|settings| output_matches(settings, &connector, &identity))
            })
            .cloned()
            .unwrap_or_else(|| default_output_settings(connector.to_string()));
        let selected_mode = settings
            .enabled
            .then(|| select_mode(&connector, settings.mode))
            .flatten();
        connected_outputs.push(ConnectedOutputInfo {
            connector: connector.to_string(),
            identity: identity.clone(),
            enabled: settings.enabled,
            profile: active_profile.map(|profile| profile.name.clone()),
            physical_size: connector.size(),
            current_mode: selected_mode.map(connected_mode_info),
            available_modes: connector
                .modes()
                .iter()
                .copied()
                .map(connected_mode_info)
                .collect(),
            scale: settings.scale,
            transform: settings.transform,
            configured_position: settings.position,
        });
        if !settings.enabled {
            tracing::info!(output = %connector, "output disabled by active profile");
            continue;
        }
        let Some(mode) = selected_mode else {
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
            selections.push(OutputSelection {
                connector,
                crtc,
                mode,
                settings,
            });
        }
    }

    selections.sort_by_key(|selection| selection.settings.position.is_none());
    Ok(OutputScan {
        selections,
        connected_outputs,
    })
}

fn connected_mode_info(mode: DrmMode) -> ConnectedModeInfo {
    let (width, height) = mode.size();
    ConnectedModeInfo {
        width,
        height,
        refresh_millihertz: OutputMode::from(mode).refresh,
        preferred: mode.mode_type().contains(ModeTypeFlags::PREFERRED),
    }
}

fn output_matches(settings: &OutputSettings, connector: &connector::Info, identity: &str) -> bool {
    settings.matcher == connector.to_string() || settings.matcher == identity
}

fn default_output_settings(matcher: String) -> OutputSettings {
    OutputSettings {
        matcher,
        enabled: true,
        mode: None,
        scale: 1.0,
        transform: OutputTransform::Normal,
        position: None,
    }
}

fn select_mode(
    connector: &connector::Info,
    requested: Option<OutputModeRequest>,
) -> Option<DrmMode> {
    let fallback = || {
        connector
            .modes()
            .iter()
            .find(|mode| mode.mode_type().contains(ModeTypeFlags::PREFERRED))
            .or_else(|| connector.modes().first())
            .copied()
    };
    let Some(requested) = requested else {
        return fallback();
    };

    let matching = connector
        .modes()
        .iter()
        .filter(|mode| mode.size() == (requested.width, requested.height));
    let selected = if let Some(refresh) = requested.refresh_millihertz {
        matching
            .min_by_key(|mode| {
                OutputMode::from(**mode)
                    .refresh
                    .unsigned_abs()
                    .abs_diff(refresh)
            })
            .filter(|mode| {
                OutputMode::from(**mode)
                    .refresh
                    .unsigned_abs()
                    .abs_diff(refresh)
                    <= 1_000
            })
            .copied()
    } else {
        matching
            .max_by_key(|mode| OutputMode::from(**mode).refresh)
            .copied()
    };
    if selected.is_none() {
        tracing::warn!(
            output = %connector,
            width = requested.width,
            height = requested.height,
            refresh_millihertz = ?requested.refresh_millihertz,
            "requested output mode is unavailable; using preferred mode"
        );
    }

    selected.or_else(fallback)
}

fn create_direct_output(
    state: &mut Ferese,
    drm: &mut DrmDevice,
    gbm: &GbmDevice<DrmDeviceFd>,
    renderer: &GlesRenderer,
    connector: connector::Info,
    crtc: crtc::Handle,
    mode: DrmMode,
    settings: &OutputSettings,
) -> Result<DirectOutput, Box<dyn Error>> {
    let identity = connector_identity(drm, &connector);
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
    // Publish only after DRM/GBM creation succeeds; failed activation must not
    // leave a phantom output/workspace that could receive evacuated windows.
    let (output, global) = create_output(state, &connector, mode, identity, settings);
    let damage_tracker = OutputDamageTracker::from_output(&output);
    let render_metrics = RenderMetrics::from_environment(output.name());

    tracing::info!(?crtc, connector = %connector, "initialized DRM output");
    Ok(DirectOutput {
        internal: internal_connector(connector.interface()),
        connector: connector.handle(),
        mode,
        settings: settings.clone(),
        output,
        global,
        surface,
        damage_tracker,
        render_metrics,
        frame_pending: false,
        lock_frame_pending: false,
    })
}

fn create_output(
    state: &mut Ferese,
    connector: &connector::Info,
    mode: DrmMode,
    identity: String,
    settings: &OutputSettings,
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
    for mode in connector.modes().iter().copied().map(OutputMode::from) {
        output.add_mode(mode);
    }
    output.set_preferred(output_mode);
    output.change_current_state(
        Some(output_mode),
        Some(output_transform(settings.transform)),
        Some(Scale::Fractional(settings.scale)),
        Some({
            let [x, y] = settings.position.unwrap_or([0, 0]);
            (x, y).into()
        }),
    );
    let position = settings.position.unwrap_or_else(|| {
        let x = state
            .space
            .outputs()
            .filter_map(|output| state.space.output_geometry(output))
            .map(|geometry| geometry.loc.x + geometry.size.w)
            .max()
            .unwrap_or(0);
        [x, 0]
    });
    state.space.map_output(&output, (position[0], position[1]));
    state.register_output(&output, identity);
    (output, global)
}

fn output_transform(transform: OutputTransform) -> Transform {
    match transform {
        OutputTransform::Normal => Transform::Normal,
        OutputTransform::Rotate90 => Transform::_90,
        OutputTransform::Rotate180 => Transform::_180,
        OutputTransform::Rotate270 => Transform::_270,
        OutputTransform::Flipped => Transform::Flipped,
        OutputTransform::Flipped90 => Transform::Flipped90,
        OutputTransform::Flipped180 => Transform::Flipped180,
        OutputTransform::Flipped270 => Transform::Flipped270,
    }
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
        Some(edid) if !edid.is_empty() => format!("drm-edid:{:016x}", stable_hash(&edid)),
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
    fn lid_only_hides_panels_with_a_usable_external_output() {
        assert!(lid_hides_panel(true, true));
        assert!(!lid_hides_panel(false, true));
        assert!(!lid_hides_panel(true, false));
        assert!(!lid_hides_panel(false, false));
    }

    #[test]
    fn panel_detection_uses_connector_type_not_monitor_name() {
        assert!(internal_connector(
            connector::Interface::EmbeddedDisplayPort
        ));
        assert!(internal_connector(connector::Interface::LVDS));
        assert!(internal_connector(connector::Interface::DSI));
        assert!(!internal_connector(connector::Interface::HDMIA));
        assert!(!internal_connector(connector::Interface::DisplayPort));
    }

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
