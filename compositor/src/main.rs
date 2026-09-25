#![allow(irrefutable_let_patterns)]

mod backends;
mod config;
mod cursor;
mod effects;
mod grabs;
mod handlers;
mod input;
mod ipc;
mod metrics;
mod overview;
mod private_client;
mod shell_control;
mod stacking;
mod state;
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
    };
    let mut state = Ferese::new(&mut event_loop, display, runtime)?;
    backends::init(launch.backend, &mut event_loop, &mut state)?;

    // SAFETY: the backend has already read the host display and Ferese is
    // single-threaded before the event loop and optional child start.
    unsafe { std::env::set_var("WAYLAND_DISPLAY", &state.socket_name) };

    info!(socket = ?state.socket_name, backend = ?launch.backend, "Ferese is accepting Wayland clients");
    let mut child = spawn_client(&mut state, launch.client, launch.client_capabilities);
    let result = event_loop.run(None, &mut state, |_| {});

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
