#![allow(irrefutable_let_patterns)]

mod backends;
mod config;
mod cursor;
mod daemon;
mod dimming;
mod effects;
mod floating;
mod gestures;
mod grabs;
mod handlers;
mod input;
mod input_capture;
mod ipc;
mod metrics;
mod overview;
mod portal_session;
mod portal_shortcuts;
mod presentation;
mod private_client;
mod reload;
mod render;
mod resize_transaction;
mod session_lock;
mod shell_control;
mod stacking;
mod state;
mod theme;
mod wallpaper;
mod window_rules;
mod winit;

use std::error::Error;
use std::process::{Child, Command};
use std::time::Duration;
use std::{io, thread};

use calloop::signals::{Signal, Signals};
use smithay::reexports::calloop::EventLoop;
use smithay::reexports::wayland_server::Display;
pub use state::{Ferese, RuntimeConfig};
use tracing::{info, warn};

use crate::backends::LaunchConfig;

fn main() -> Result<(), Box<dyn Error>> {
    init_logging();

    // Settings validates a candidate before atomically replacing the user's file.
    // This mode never opens a display, input device, or compositor socket.
    let args = std::env::args_os().skip(1).collect::<Vec<_>>();
    if args.first().is_some_and(|arg| arg == "--check-config") {
        if args.len() != 2 {
            return Err("usage: ferese --check-config PATH".into());
        }
        let source = std::fs::read_to_string(&args[1])?;
        theme::prepare(
            &source,
            std::path::Path::new(&args[1])
                .parent()
                .unwrap_or(std::path::Path::new(".")),
        )?;
        println!("Configuration is valid");
        return Ok(());
    }

    let launch =
        LaunchConfig::from_environment().map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))?;
    let initial_source = config::config_path()
        .filter(|path| path.exists())
        .map(|path| theme::read_source(&path))
        .transpose()?;
    let source = initial_source.as_deref().unwrap_or("");
    let directory = config::config_path()
        .and_then(|path| path.parent().map(ToOwned::to_owned))
        .unwrap_or_else(|| std::path::PathBuf::from("."));
    let (config, runtime, candidate) = theme::prepare(source, &directory)?;
    let mut event_loop = EventLoop::try_new()?;
    let signals = Signals::new(&[Signal::SIGINT, Signal::SIGTERM])?;
    event_loop
        .handle()
        .insert_source(signals, |event, _, state: &mut Ferese| {
            info!(signal = ?event.signal(), "stopping Ferese");
            state.loop_signal.stop();
        })?;
    let display = Display::new()?;
    let mut state = Ferese::new(&mut event_loop, display, runtime)?;
    state.config_source = initial_source.filter(|source| source.len() <= 60 * 1024);
    theme::init(&mut event_loop, &mut state, candidate)?;
    overview::init_font_loader(&mut event_loop, &mut state)?;
    backends::init(launch.backend, &mut event_loop, &mut state)?;
    use calloop::timer::{TimeoutAction, Timer};
    info!(socket = ?state.socket_name, backend = ?launch.backend, "Ferese is accepting Wayland clients");
    let mut child = spawn_client(&mut state, launch.client, launch.client_capabilities);
    let runner = std::rc::Rc::new(std::cell::RefCell::new(daemon::Runner::new(
        &config.autostart,
        launch.backend == backends::BackendKind::Nested,
    )));
    runner.borrow_mut().tick(&mut state);
    let services = runner.clone();
    event_loop.handle().insert_source(
        Timer::from_duration(Duration::from_secs(1)),
        move |_, _, state: &mut Ferese| {
            services.borrow_mut().tick(state);
            TimeoutAction::ToDuration(Duration::from_secs(1))
        },
    )?;
    let reload_handle = event_loop.handle();
    let result = event_loop.run(None, &mut state, |state| {
        state.poll_theme();
        // The decoder wakes the loop after publishing its result, including
        // when no output has a pending frame.
        let wallpaper_changed = state.wallpaper.poll();
        if wallpaper_changed {
            state.backdrop_generation = state.backdrop_generation.wrapping_add(1);
        }
        // All input/Wayland callbacks have returned, releasing seat locks.
        if state.focus_cycle.is_some() && (state.session_lock.active || state.input_capture.captures(1)) {
            state.cancel_focus_cycle();
        }
        // Coalesce cursor changes and redraw here, never inside cursor_image.
        if state.input_capture.restore_focus {
            state.restore_input_capture_focus();
        }
        state.apply_unlocked_pointer_hint();
        let wallpaper_retry = state.wallpaper.take_retry_wakeup();
        if std::mem::take(&mut state.cursor_redraw_pending) || wallpaper_changed || wallpaper_retry {
            backends::direct::render_all(state);
        }
        if let Err(error) = state
            .wallpaper
            .arm_retry_timer(&reload_handle, std::time::Instant::now())
        {
            warn!(%error, "could not schedule wallpaper upload retry");
        }
        // Registry/sync/configure replies must not depend on a submitted
        // frame: at startup clients need these before they can draw anything.
        // This also keeps idle DRM sessions responsive when there is no damage.
        if let Err(error) = state.display_handle.flush_clients() {
            tracing::warn!(%error, "failed to flush Wayland clients");
        }
    });

    runner.borrow_mut().stop();
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
    command.env_remove("WAYLAND_SOCKET");
    command.env_remove("FERESE_SHELL_CONTROL_SOCKET");
    command.env("FERESE_COMPOSITOR_WALLPAPER", "1");
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
    use std::io::{Read, Write};
    use std::os::unix::net::UnixStream;
    use std::sync::Arc;

    use super::*;

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
        client.set_read_timeout(Some(Duration::from_secs(1))).unwrap();
        let mut reply = [0u8; 12];
        client.read_exact(&mut reply).unwrap();
        assert_eq!(u32::from_ne_bytes(reply[0..4].try_into().unwrap()), 2);
        assert_eq!(u32::from_ne_bytes(reply[4..8].try_into().unwrap()) & 0xffff, 0);
    }
}
