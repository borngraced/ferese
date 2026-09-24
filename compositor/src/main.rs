#![allow(irrefutable_let_patterns)]

mod grabs;
mod handlers;
mod input;
mod state;
mod winit;

use std::{error::Error, process::Command};

use smithay::reexports::{calloop::EventLoop, wayland_server::Display};
pub use state::Ferese;
use tracing::{info, warn};

fn main() -> Result<(), Box<dyn Error>> {
    init_logging();

    let mut event_loop: EventLoop<Ferese> = EventLoop::try_new()?;
    let display: Display<Ferese> = Display::new()?;
    let mut state = Ferese::new(&mut event_loop, display)?;
    winit::init(&mut event_loop, &mut state)?;

    // SAFETY: the backend has already read the host display and Ferese is
    // single-threaded before the event loop and optional child start.
    unsafe { std::env::set_var("WAYLAND_DISPLAY", &state.socket_name) };

    info!(socket = ?state.socket_name, "Ferese is accepting Wayland clients");
    spawn_client_from_args();
    event_loop.run(None, &mut state, |_| {})?;
    Ok(())
}

fn init_logging() {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("ferese=info"));
    tracing_subscriber::fmt().with_env_filter(filter).init();
}

fn spawn_client_from_args() {
    let mut args = std::env::args_os().skip(1);
    let Some(program) = args.next() else {
        info!("no client requested; pass one after `--`, for example `-- foot`");
        return;
    };

    match Command::new(&program).args(args).spawn() {
        Ok(child) => info!(program = ?program, pid = child.id(), "spawned nested client"),
        Err(error) => warn!(program = ?program, %error, "failed to spawn nested client"),
    }
}
