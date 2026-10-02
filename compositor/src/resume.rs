//! System wake is shared by hardware reconciliation and time-dependent UI state.
use std::error::Error;
use std::time::Duration;

use calloop::EventLoop;
use calloop::channel::{Event, channel};

use crate::Ferese;

pub(crate) fn init(event_loop: &mut EventLoop<Ferese>) -> Result<(), Box<dyn Error>> {
    let (sender, receiver) = channel();
    event_loop.handle().insert_source(receiver, |event, _, state| {
        if let Event::Msg(()) = event {
            crate::backends::direct::system_resumed(state);
            state.refresh_theme();
        }
    })?;

    std::thread::Builder::new()
        .name("ferese-system-resume".into())
        .spawn(move || {
            let mut warned = false;
            loop {
                let monitor = || -> zbus::Result<bool> {
                    let connection = zbus::blocking::connection::Builder::system()?
                        .method_timeout(Duration::from_secs(2))
                        .build()?;
                    let proxy = zbus::blocking::Proxy::new(
                        &connection,
                        "org.freedesktop.login1",
                        "/org/freedesktop/login1",
                        "org.freedesktop.login1.Manager",
                    )?;
                    for message in proxy.receive_signal("PrepareForSleep")? {
                        let (sleeping,): (bool,) = message.body().deserialize()?;
                        if !sleeping && sender.send(()).is_err() {
                            return Ok(false);
                        }
                    }
                    Ok(true)
                };

                match monitor() {
                    Ok(false) => break,
                    Ok(true) => warned = false,
                    Err(error) if !warned => {
                        tracing::warn!(%error, "system resume monitor unavailable; retrying connection");
                        warned = true;
                    }
                    Err(_) => {}
                }
                // Recovery only: a healthy listener blocks on DBus signals.
                std::thread::sleep(Duration::from_secs(5));
            }
        })?;
    Ok(())
}
