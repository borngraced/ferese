use std::error::Error;

use ferese_protocols::shell::v1::client::{
    ferese_shell_manager_v1::FereseShellManagerV1,
    ferese_shell_v1::{self, FereseShellV1},
};
use wayland_client::{
    Connection, Dispatch, QueueHandle, WEnum, delegate_noop, protocol::wl_registry,
};

fn main() -> Result<(), Box<dyn Error>> {
    let expect_hidden = std::env::args_os().any(|argument| argument == "--expect-hidden");
    let connection = Connection::connect_to_env()?;
    let mut queue = connection.new_event_queue();
    let qh = queue.handle();
    connection.display().get_registry(&qh, ());
    let mut state = ProbeState::default();

    queue.roundtrip(&mut state)?;
    if expect_hidden {
        if state.manager.is_some() {
            return Err("public client could see ferese_shell_manager_v1".into());
        }

        println!("PASS public client cannot see ferese_shell_manager_v1");
        return Ok(());
    }

    let manager = state
        .manager
        .clone()
        .ok_or("trusted client could not see ferese_shell_manager_v1")?;
    state.shell = Some(manager.get_shell(&qh, ()));

    while !state.initialized() {
        queue.blocking_dispatch(&mut state)?;
    }

    let shell = state.shell.clone().expect("shell initialized");
    shell.enter_overview();
    while state.overview_active != Some(true) {
        queue.blocking_dispatch(&mut state)?;
    }

    shell.exit_overview();
    while state.overview_active != Some(false) || state.overview_events < 3 {
        queue.blocking_dispatch(&mut state)?;
    }

    shell.activate_window(u32::MAX, u32::MAX);
    while !state.invalid_window_rejected {
        queue.blocking_dispatch(&mut state)?;
    }

    println!(
        "PASS shell-control snapshot={} windows={} overview round-trip and stale-id rejection",
        state.snapshot_serial.unwrap_or_default(),
        state.window_count
    );
    Ok(())
}

#[derive(Default)]
struct ProbeState {
    manager: Option<FereseShellManagerV1>,
    shell: Option<FereseShellV1>,
    capabilities: Option<u32>,
    snapshot_serial: Option<u32>,
    snapshot_complete: bool,
    window_count: usize,
    overview_active: Option<bool>,
    overview_events: usize,
    invalid_window_rejected: bool,
}

impl ProbeState {
    fn initialized(&self) -> bool {
        self.capabilities == Some(1)
            && self.snapshot_complete
            && self.overview_active == Some(false)
    }
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

        if interface == "ferese_shell_manager_v1" {
            state.manager = Some(registry.bind(name, version.min(1), qh, ()));
        }
    }
}

impl Dispatch<FereseShellV1, ()> for ProbeState {
    fn event(
        state: &mut Self,
        _shell: &FereseShellV1,
        event: ferese_shell_v1::Event,
        _data: &(),
        _connection: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        match event {
            ferese_shell_v1::Event::Capabilities { version } => {
                state.capabilities = Some(version);
            }
            ferese_shell_v1::Event::SnapshotBegin { serial } => {
                state.snapshot_serial = Some(serial);
                state.snapshot_complete = false;
                state.window_count = 0;
            }
            ferese_shell_v1::Event::Window { .. } => {
                state.window_count += 1;
            }
            ferese_shell_v1::Event::SnapshotEnd { serial } => {
                state.snapshot_complete = state.snapshot_serial == Some(serial);
            }
            ferese_shell_v1::Event::OverviewState { active } => {
                state.overview_active = Some(active != 0);
                state.overview_events += 1;
            }
            ferese_shell_v1::Event::RequestFailed {
                request,
                window_hi,
                window_lo,
                ..
            } => {
                state.invalid_window_rejected = request
                    == WEnum::Value(ferese_shell_v1::FailedRequest::ActivateWindow)
                    && window_hi == u32::MAX
                    && window_lo == u32::MAX;
            }
            _ => {}
        }
    }
}

delegate_noop!(ProbeState: ignore FereseShellManagerV1);
