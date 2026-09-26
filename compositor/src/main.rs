#![allow(irrefutable_let_patterns)]

mod backends;
mod config;
mod cursor;
mod dimming;
mod effects;
mod grabs;
mod handlers;
mod input;
mod ipc;
mod metrics;
mod overview;
mod presentation;
mod private_client;
mod resize_transaction;
mod shell_control;
mod stacking;
mod state;
mod wallpaper;
mod window_rules;
mod winit;

use std::{
    error::Error,
    io,
    process::{Child, Command},
    thread,
    time::Duration,
};

use calloop::signals::{Signal, Signals};
use smithay::reexports::{calloop::EventLoop, wayland_server::Display};
pub use state::{Ferese, RuntimeConfig};
use tracing::{info, warn};

use crate::backends::LaunchConfig;
use crate::config::Config;

fn main() -> Result<(), Box<dyn Error>> {
    init_logging();

    let launch = LaunchConfig::from_environment()
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))?;
    let config = Config::load()?;
    let input_settings = config.input_settings()?;
    let bindings = config.bindings(&input_settings)?;
    let window_rules = config.window_rules()?;
    let output_profiles = config.output_profiles()?;
    let mut event_loop = EventLoop::try_new()?;
    let signals = Signals::new(&[Signal::SIGINT, Signal::SIGTERM])?;
    event_loop
        .handle()
        .insert_source(signals, |event, _, state: &mut Ferese| {
            info!(signal = ?event.signal(), "stopping Ferese");
            state.loop_signal.stop();
        })?;
    let display = Display::new()?;
    let runtime = RuntimeConfig {
        layout_mode: config.layout_mode(),
        gap_config: config.gap_config()?,
        input_settings,
        bindings,
        window_rules,
        theme_settings: config.theme_settings()?,
        default_column_width: config.default_column_width()?,
        scrolling_focus_strategy: config.scrolling_focus_strategy(),
        column_width_presets: config.width_presets()?,
        animations_enabled: config.animations_enabled(),
        animation_speed: config.animation_speed()?,
        spring_config: config.spring_config()?,
        viewport_spring_config: config.viewport_spring_config()?,
        output_profiles,
        wallpaper: config.wallpaper_settings(),
        overview_font_family: config.overview_font_family(),
    };
    let mut state = Ferese::new(&mut event_loop, display, runtime)?;
    backends::init(launch.backend, &mut event_loop, &mut state)?;

    info!(socket = ?state.socket_name, backend = ?launch.backend, "Ferese is accepting Wayland clients");
    let mut child = spawn_client(&mut state, launch.client, launch.client_capabilities);
    let result = event_loop.run(None, &mut state, |state| {
        // All input/Wayland callbacks have returned, releasing seat locks.
        // Coalesce cursor changes and redraw here, never inside cursor_image.
        if std::mem::take(&mut state.cursor_redraw_pending) {
            backends::direct::render_all(state);
        }
        // Registry/sync/configure replies must not depend on a submitted
        // frame: at startup clients need these before they can draw anything.
        // This also keeps idle DRM sessions responsive when there is no damage.
        if let Err(error) = state.display_handle.flush_clients() {
            tracing::warn!(%error, "failed to flush Wayland clients");
        }
    });

    if let Some(child) = &mut child {
        terminate_child(child);
    }

    result.map_err(Into::into)
}

fn init_logging() {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("ferese=info"));
    tracing_subscriber::fmt().with_env_filter(filter).init();
}

fn spawn_client(
    state: &mut Ferese,
    mut args: Vec<std::ffi::OsString>,
    capabilities: private_client::ClientCapabilities,
) -> Option<Child> {
    if args.is_empty() {
        info!("no client requested; pass one after `--`, for example `-- foot`");
        return None;
    }
    let program = args.remove(0);

    let mut command = Command::new(&program);
    command.args(args);
    // Wallpaper decoding and drivers may already have worker threads. Set
    // the child's display explicitly instead of mutating the process env.
    command.env("WAYLAND_DISPLAY", &state.socket_name);
    if state.wallpaper.owns_background() {
        command.env("FERESE_COMPOSITOR_WALLPAPER", "1");
    }
    let private_connection = if capabilities.is_empty() {
        None
    } else {
        match private_client::prepare_command(state, &mut command, capabilities) {
            Ok(connection) => Some(connection),
            Err(error) => {
                warn!(program = ?program, %error, "failed to create private Wayland connection");
                return None;
            }
        }
    };

    let child = match command.spawn() {
        Ok(child) => {
            info!(program = ?program, pid = child.id(), "spawned Wayland client");
            Some(child)
        }
        Err(error) => {
            warn!(program = ?program, %error, "failed to spawn Wayland client");
            None
        }
    };
    drop(private_connection);
    child
}

fn terminate_child(child: &mut Child) {
    if child.try_wait().ok().flatten().is_some() {
        return;
    }

    // SAFETY: the process ID comes from the live Child owned by Ferese.
    unsafe {
        libc::kill(child.id() as i32, libc::SIGTERM);
    }

    for _ in 0..20 {
        if child.try_wait().ok().flatten().is_some() {
            return;
        }
        thread::sleep(Duration::from_millis(10));
    }

    let _ = child.kill();
    let _ = child.wait();
}

#[cfg(test)]
mod startup_tests {
    use super::*;
    use std::{
        io::{Read, Write},
        os::unix::net::UnixStream,
        sync::Arc,
    };

    #[test]
    fn deferred_cursor_redraw_runs_after_input_lock_is_released() {
        use std::sync::Mutex;

        let mut event_loop = EventLoop::<bool>::try_new().unwrap();
        let pointer_lock = Arc::new(Mutex::new(()));
        let callback_lock = pointer_lock.clone();
        event_loop.handle().insert_idle(move |pending| {
            // Mirrors Smithay's cursor_image callback during pointer motion.
            let _guard = callback_lock.lock().unwrap();
            *pending = true;
            *pending = true; // Multiple cursor changes must coalesce.
            assert!(callback_lock.try_lock().is_err());
        });

        let signal = event_loop.get_signal();
        let mut pending = false;
        let mut redraws = 0;
        event_loop
            .run(Some(Duration::ZERO), &mut pending, |pending| {
                if std::mem::take(pending) {
                    // Rendering reads the pointer location at this point.
                    let _guard = pointer_lock.try_lock().expect("input lock still held");
                    redraws += 1;
                }
                assert!(!std::mem::take(pending));
                signal.stop();
            })
            .unwrap();
        assert_eq!(redraws, 1);
    }

    #[test]
    fn wayland_roundtrip_completes_without_a_rendered_frame() {
        let mut display = Display::<()>::new().unwrap();
        let (server, mut client) = UnixStream::pair().unwrap();
        display
            .handle()
            .insert_client(server, Arc::new(state::ClientState::default()))
            .unwrap();
        // wl_display.sync(new_id=2), using native-endian Wayland wire format.
        for word in [1u32, 12u32 << 16, 2u32] {
            client.write_all(&word.to_ne_bytes()).unwrap();
        }
        display.dispatch_clients(&mut ()).unwrap();
        display.handle().flush_clients().unwrap();
        client
            .set_read_timeout(Some(Duration::from_secs(1)))
            .unwrap();
        let mut reply = [0u8; 12];
        client.read_exact(&mut reply).unwrap();
        assert_eq!(u32::from_ne_bytes(reply[0..4].try_into().unwrap()), 2);
        assert_eq!(
            u32::from_ne_bytes(reply[4..8].try_into().unwrap()) & 0xffff,
            0
        );
    }
}
