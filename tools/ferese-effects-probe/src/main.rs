use std::error::Error;
use std::io::{Seek, SeekFrom, Write};
use std::os::fd::AsFd;
use std::time::Duration;

use ferese_protocols::effects::v1::client::{
    ferese_effects_manager_v1::FereseEffectsManagerV1,
    ferese_surface_effects_v1::{self, FereseSurfaceEffectsV1},
};
use wayland_client::{
    Connection, Dispatch, QueueHandle, delegate_noop,
    protocol::{wl_buffer, wl_compositor, wl_registry, wl_shm, wl_shm_pool, wl_surface},
};
use wayland_protocols_wlr::layer_shell::v1::client::{
    zwlr_layer_shell_v1::{self, ZwlrLayerShellV1},
    zwlr_layer_surface_v1::{self, ZwlrLayerSurfaceV1},
};

const WIDTH: u32 = 640;
const HEIGHT: u32 = 64;

fn main() -> Result<(), Box<dyn Error>> {
    let arguments = std::env::args_os().collect::<Vec<_>>();
    let expect_hidden = arguments
        .iter()
        .any(|argument| argument == "--expect-hidden");
    let preview = arguments.iter().any(|argument| argument == "--preview");
    let connection = Connection::connect_to_env()?;
    let mut queue = connection.new_event_queue();
    let qh = queue.handle();
    connection.display().get_registry(&qh, ());
    let mut state = ProbeState {
        preview,
        ..ProbeState::default()
    };

    if expect_hidden {
        queue.roundtrip(&mut state)?;
        if state.effects_manager.is_some() {
            return Err("public client could see the private effects global".into());
        }
        println!("PASS public client cannot see ferese_effects_manager_v1");
        return Ok(());
    }

    while state.effects.is_none() || !state.configured {
        queue.blocking_dispatch(&mut state)?;
        state.try_initialize(&qh)?;
        if state.closed {
            return Err("layer surface was closed before its first configure".into());
        }
    }

    let effects = state.effects.clone().expect("effects initialized");
    let initial_configures = state.configure_count;
    for role in [
        ferese_surface_effects_v1::Role::PanelElevated,
        ferese_surface_effects_v1::Role::Popover,
        ferese_surface_effects_v1::Role::Menu,
        ferese_surface_effects_v1::Role::Notification,
        ferese_surface_effects_v1::Role::Hud,
        ferese_surface_effects_v1::Role::Modal,
        ferese_surface_effects_v1::Role::Panel,
    ] {
        effects.set_role(role);
        queue.roundtrip(&mut state)?;
        // Exercise each role in an actual presented frame, including changes
        // to the cached backdrop allocation and shadow bounds.
        std::thread::sleep(Duration::from_millis(50));
        if state.configure_count != initial_configures {
            return Err(format!("semantic role {role:?} changed layer geometry").into());
        }
    }

    if preview {
        effects.set_role(ferese_surface_effects_v1::Role::Panel);
        queue.roundtrip(&mut state)?;
        println!("PREVIEW glass panel is mapped over the patterned card; press Ctrl+C to stop");

        while !state.closed {
            queue.blocking_dispatch(&mut state)?;
        }

        return Ok(());
    }

    // Keep the final glass role mapped long enough for the compositor to run
    // at least one presentation. This turns shader and capture failures into
    // probe failures instead of disconnecting before the first rendered frame.
    std::thread::sleep(Duration::from_millis(100));

    effects.clear_role();
    queue.roundtrip(&mut state)?;
    if state.configure_count != initial_configures {
        return Err("clearing the semantic role changed layer geometry".into());
    }

    println!(
        "PASS surface.panel and all semantic roles preserved {}x{} layer geometry",
        state.width, state.height
    );
    Ok(())
}

#[derive(Default)]
struct ProbeState {
    compositor: Option<wl_compositor::WlCompositor>,
    shm: Option<wl_shm::WlShm>,
    layer_shell: Option<ZwlrLayerShellV1>,
    effects_manager: Option<FereseEffectsManagerV1>,
    surface: Option<wl_surface::WlSurface>,
    layer_surface: Option<ZwlrLayerSurfaceV1>,
    effects: Option<FereseSurfaceEffectsV1>,
    buffer: Option<wl_buffer::WlBuffer>,
    preview_backdrop_surface: Option<wl_surface::WlSurface>,
    preview_backdrop_layer: Option<ZwlrLayerSurfaceV1>,
    preview_backdrop_buffer: Option<wl_buffer::WlBuffer>,
    preview: bool,
    configured: bool,
    closed: bool,
    configure_count: u32,
    width: u32,
    height: u32,
}

impl ProbeState {
    fn try_initialize(&mut self, qh: &QueueHandle<Self>) -> Result<(), Box<dyn Error>> {
        if self.surface.is_some() {
            return Ok(());
        }
        let (Some(compositor), Some(_shm), Some(layer_shell), Some(effects_manager)) = (
            self.compositor.as_ref(),
            self.shm.as_ref(),
            self.layer_shell.as_ref(),
            self.effects_manager.as_ref(),
        ) else {
            return Ok(());
        };

        let surface = compositor.create_surface(qh, ());
        let layer = if self.preview {
            zwlr_layer_shell_v1::Layer::Overlay
        } else {
            zwlr_layer_shell_v1::Layer::Top
        };
        let layer_surface = layer_shell.get_layer_surface(
            &surface,
            None,
            layer,
            "ferese-effects-probe".to_owned(),
            qh,
            (),
        );
        if self.preview {
            layer_surface.set_anchor(
                zwlr_layer_surface_v1::Anchor::Top | zwlr_layer_surface_v1::Anchor::Right,
            );
            layer_surface.set_margin(32, 32, 0, 0);
            layer_surface.set_size(480, 220);
            layer_surface.set_exclusive_zone(0);
        } else {
            layer_surface.set_anchor(
                zwlr_layer_surface_v1::Anchor::Top
                    | zwlr_layer_surface_v1::Anchor::Left
                    | zwlr_layer_surface_v1::Anchor::Right,
            );
            layer_surface.set_size(WIDTH, HEIGHT);
            layer_surface.set_exclusive_zone(HEIGHT as i32);
        }

        let effects = effects_manager.get_surface_effects(&surface, qh, ());
        effects.set_role(ferese_surface_effects_v1::Role::Panel);

        surface.commit();

        if self.preview {
            let backdrop_surface = compositor.create_surface(qh, ());
            let backdrop_layer = layer_shell.get_layer_surface(
                &backdrop_surface,
                None,
                zwlr_layer_shell_v1::Layer::Top,
                "ferese-effects-preview-backdrop".to_owned(),
                qh,
                (),
            );
            backdrop_layer.set_anchor(
                zwlr_layer_surface_v1::Anchor::Top | zwlr_layer_surface_v1::Anchor::Right,
            );
            backdrop_layer.set_margin(12, 12, 0, 0);
            backdrop_layer.set_size(520, 260);
            backdrop_layer.set_exclusive_zone(0);
            backdrop_surface.commit();

            self.preview_backdrop_surface = Some(backdrop_surface);
            self.preview_backdrop_layer = Some(backdrop_layer);
        }

        self.surface = Some(surface);
        self.layer_surface = Some(layer_surface);
        self.effects = Some(effects);
        Ok(())
    }
}

fn create_buffer(
    shm: &wl_shm::WlShm,
    qh: &QueueHandle<ProbeState>,
    width: u32,
    height: u32,
    preview: bool,
) -> Result<wl_buffer::WlBuffer, Box<dyn Error>> {
    let stride = width * 4;
    let size = stride * height;
    let mut file = tempfile::tempfile()?;
    file.set_len(u64::from(size))?;
    let pixel = if preview {
        [0_u8; 4]
    } else {
        [0x28, 0x24, 0x20, 0xd8]
    };
    for _ in 0..width * height {
        file.write_all(&pixel)?;
    }
    file.seek(SeekFrom::Start(0))?;

    let pool = shm.create_pool(file.as_fd(), size as i32, qh, ());
    let buffer = pool.create_buffer(
        0,
        width as i32,
        height as i32,
        stride as i32,
        wl_shm::Format::Argb8888,
        qh,
        (),
    );
    pool.destroy();
    Ok(buffer)
}

fn create_preview_backdrop_buffer(
    shm: &wl_shm::WlShm,
    qh: &QueueHandle<ProbeState>,
    width: u32,
    height: u32,
) -> Result<wl_buffer::WlBuffer, Box<dyn Error>> {
    let stride = width * 4;
    let size = stride * height;
    let mut file = tempfile::tempfile()?;
    file.set_len(u64::from(size))?;

    for y in 0..height {
        for x in 0..width {
            let checker = (x / 11 + y / 11) % 2 == 0;
            let stripe = (x / 53) % 3;
            let pixel = match (checker, stripe) {
                (true, 0) => [0xf0, 0xf0, 0xf0, 0xff],
                (false, 0) => [0x08, 0x08, 0x08, 0xff],
                (true, 1) => [0x30, 0xe8, 0x40, 0xff],
                (false, 1) => [0x10, 0x18, 0x08, 0xff],
                (true, _) => [0x30, 0x60, 0xf0, 0xff],
                (false, _) => [0x18, 0x08, 0x20, 0xff],
            };
            file.write_all(&pixel)?;
        }
    }
    file.seek(SeekFrom::Start(0))?;

    let pool = shm.create_pool(file.as_fd(), size as i32, qh, ());
    let buffer = pool.create_buffer(
        0,
        width as i32,
        height as i32,
        stride as i32,
        wl_shm::Format::Argb8888,
        qh,
        (),
    );
    pool.destroy();
    Ok(buffer)
}

impl Dispatch<wl_registry::WlRegistry, ()> for ProbeState {
    fn event(
        state: &mut Self,
        registry: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        _data: &(),
        _connection: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        let wl_registry::Event::Global {
            name,
            interface,
            version,
        } = event
        else {
            return;
        };

        match interface.as_str() {
            "wl_compositor" => state.compositor = Some(registry.bind(name, version.min(6), qh, ())),
            "wl_shm" => state.shm = Some(registry.bind(name, 1, qh, ())),
            "zwlr_layer_shell_v1" => {
                state.layer_shell = Some(registry.bind(name, version.min(4), qh, ()))
            }
            "ferese_effects_manager_v1" => {
                state.effects_manager = Some(registry.bind(name, 1, qh, ()))
            }
            _ => {}
        }
    }
}

impl Dispatch<ZwlrLayerSurfaceV1, ()> for ProbeState {
    fn event(
        state: &mut Self,
        layer_surface: &ZwlrLayerSurfaceV1,
        event: zwlr_layer_surface_v1::Event,
        _data: &(),
        _connection: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        match event {
            zwlr_layer_surface_v1::Event::Configure {
                serial,
                width,
                height,
            } => {
                layer_surface.ack_configure(serial);

                if state.preview_backdrop_layer.as_ref() == Some(layer_surface) {
                    let width = if width == 0 { 520 } else { width };
                    let height = if height == 0 { 260 } else { height };
                    let surface = state
                        .preview_backdrop_surface
                        .clone()
                        .expect("preview backdrop surface initialized");
                    let shm = state.shm.as_ref().expect("shm initialized");
                    let buffer = create_preview_backdrop_buffer(shm, qh, width, height)
                        .expect("create preview backdrop buffer");
                    surface.attach(Some(&buffer), 0, 0);
                    surface.damage_buffer(0, 0, width as i32, height as i32);
                    surface.commit();
                    state.preview_backdrop_buffer = Some(buffer);
                    return;
                }

                state.configure_count = state.configure_count.saturating_add(1);
                state.width = if width == 0 { WIDTH } else { width };
                state.height = if height == 0 { HEIGHT } else { height };
                if !state.configured {
                    let surface = state.surface.clone().expect("surface initialized");
                    let shm = state.shm.as_ref().expect("shm initialized");
                    let buffer = create_buffer(shm, qh, state.width, state.height, state.preview)
                        .expect("create configured shared-memory buffer");
                    surface.attach(Some(&buffer), 0, 0);
                    surface.damage_buffer(0, 0, state.width as i32, state.height as i32);
                    surface.commit();
                    state.buffer = Some(buffer);
                    state.configured = true;
                }
            }
            zwlr_layer_surface_v1::Event::Closed => state.closed = true,
            _ => {}
        }
    }
}

delegate_noop!(ProbeState: ignore wl_compositor::WlCompositor);
delegate_noop!(ProbeState: ignore wl_shm::WlShm);
delegate_noop!(ProbeState: ignore wl_shm_pool::WlShmPool);
delegate_noop!(ProbeState: ignore wl_buffer::WlBuffer);
delegate_noop!(ProbeState: ignore wl_surface::WlSurface);
delegate_noop!(ProbeState: ignore ZwlrLayerShellV1);
delegate_noop!(ProbeState: ignore FereseEffectsManagerV1);
delegate_noop!(ProbeState: ignore FereseSurfaceEffectsV1);
