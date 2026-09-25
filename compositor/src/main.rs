#![allow(irrefutable_let_patterns)]

mod backends;
mod config;
mod cursor;
mod grabs;
mod handlers;
mod input;
mod state;
mod winit;

use std::{error::Error, io, process::Command};

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
    let mut event_loop = EventLoop::try_new()?;
    let display = Display::new()?;
    let runtime = RuntimeConfig {
        layout_mode: config.layout_mode(),
        gap_config: config.gap_config()?,
        default_column_width: config.default_column_width()?,
        scrolling_focus_strategy: config.scrolling_focus_strategy(),
        column_width_presets: config.width_presets()?,
        animations_enabled: config.animations_enabled(),
        animation_speed: config.animation_speed()?,
        spring_config: config.spring_config()?,
        viewport_spring_config: config.viewport_spring_config()?,
    };
    let mut state = Ferese::new(&mut event_loop, display, runtime)?;
    backends::init(launch.backend, &mut event_loop, &mut state)?;

    // SAFETY: the backend has already read the host display and Ferese is
    // single-threaded before the event loop and optional child start.
    unsafe { std::env::set_var("WAYLAND_DISPLAY", &state.socket_name) };

    info!(socket = ?state.socket_name, backend = ?launch.backend, "Ferese is accepting Wayland clients");
    spawn_client(launch.client);
    event_loop.run(None, &mut state, |_| {})?;
    Ok(())
}

fn init_logging() {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("ferese=info"));
    tracing_subscriber::fmt().with_env_filter(filter).init();
}

fn spawn_client(mut args: Vec<std::ffi::OsString>) {
    if args.is_empty() {
        info!("no client requested; pass one after `--`, for example `-- foot`");
        return;
    }
    let program = args.remove(0);

    match Command::new(&program).args(args).spawn() {
        Ok(child) => info!(program = ?program, pid = child.id(), "spawned Wayland client"),
        Err(error) => warn!(program = ?program, %error, "failed to spawn Wayland client"),
    }
}
