//! Deliberately has no password authentication. Nested test sessions only.
use std::{error::Error, time::Duration};
use wayland_client::{Connection, Dispatch, QueueHandle, protocol::wl_registry};
use wayland_protocols::ext::session_lock::v1::client::{
    ext_session_lock_manager_v1::ExtSessionLockManagerV1,
    ext_session_lock_v1::{self, ExtSessionLockV1},
};

#[derive(Default)]
struct Probe {
    manager: Option<ExtSessionLockManagerV1>,
    locked: bool,
    rejected: bool,
}
impl Dispatch<wl_registry::WlRegistry, ()> for Probe {
    fn event(
        state: &mut Self,
        registry: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if let wl_registry::Event::Global {
            name, interface, ..
        } = event
        {
            if interface == "ext_session_lock_manager_v1" {
                state.manager = Some(registry.bind(name, 1, qh, ()));
            }
        }
    }
}
wayland_client::delegate_noop!(Probe: ignore ExtSessionLockManagerV1);
impl Dispatch<ExtSessionLockV1, bool> for Probe {
    fn event(
        state: &mut Self,
        _: &ExtSessionLockV1,
        event: ext_session_lock_v1::Event,
        competing: &bool,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            ext_session_lock_v1::Event::Locked if !competing => state.locked = true,
            ext_session_lock_v1::Event::Finished if *competing => state.rejected = true,
            ext_session_lock_v1::Event::Finished => panic!("lock rejected"),
            _ => {}
        }
    }
}

fn main() -> Result<(), Box<dyn Error>> {
    if std::env::var_os("FERESE_LOCK_TEST").is_none() {
        return Err(
            "set FERESE_LOCK_TEST=1; this probe is only for disposable nested sessions".into(),
        );
    }
    let mode = std::env::args().nth(1).unwrap_or_else(|| "unlock".into());
    let connection = Connection::connect_to_env()?;
    let mut queue = connection.new_event_queue();
    let qh = queue.handle();
    connection.display().get_registry(&qh, ());
    let mut state = Probe::default();
    queue.roundtrip(&mut state)?;
    let manager = state
        .manager
        .clone()
        .ok_or("session lock protocol unavailable")?;
    let lock = manager.lock(&qh, false);
    if mode == "invalid-unlock" {
        lock.unlock_and_destroy();
        assert!(
            queue.roundtrip(&mut state).is_err(),
            "premature unlock accepted"
        );
        println!("PASS premature unlock disconnected");
        return Ok(());
    }
    while !state.locked {
        queue.blocking_dispatch(&mut state)?;
    }
    println!("PASS locked event received after safe frame");
    match mode.as_str() {
        "crash" => return Ok(()),
        "hold" => std::thread::sleep(Duration::from_secs(10)),
        "competing" | "attack" => {
            let other = manager.lock(&qh, true);
            queue.roundtrip(&mut state)?;
            assert!(state.rejected, "competing lock not rejected");
            if mode == "attack" {
                other.unlock_and_destroy();
                assert!(
                    queue.roundtrip(&mut state).is_err(),
                    "rejected lock unlocked owner"
                );
                println!("PASS rejected owner cannot unlock");
                return Ok(());
            }
            other.destroy();
        }
        "unlock" => {}
        _ => return Err("use unlock, hold, crash, competing, attack, or invalid-unlock".into()),
    }
    lock.unlock_and_destroy();
    queue.roundtrip(&mut state)?;
    println!("PASS owner unlocked");
    Ok(())
}
