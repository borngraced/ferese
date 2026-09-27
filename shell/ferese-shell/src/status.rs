//! Bounded, off-UI-thread adapters. A missing service is `None`, never a fake state.
mod bluetooth;
mod network;

use std::{
    env, fs,
    path::Path,
    process::{Command, Stdio},
    sync::{
        Arc, Condvar, Mutex,
        mpsc::{self, SyncSender},
    },
    thread,
    time::Duration,
};

#[derive(Clone, Debug, Default)]
pub struct Snapshot {
    pub network: Option<Network>,
    pub bluetooth: Option<Bluetooth>,
    pub audio: Option<Audio>,
    pub battery: Option<Battery>,
    pub brightness: Option<u8>,
    pub notifications: Option<Notifications>,
    pub poweroff: bool,
    pub reboot: bool,
    pub suspend: bool,
}

#[derive(Clone, Debug)]
pub struct Network {
    pub enabled: bool,
    pub connection: Option<String>,
    pub signal: u8,
}

#[derive(Clone, Debug)]
pub struct Bluetooth {
    pub enabled: bool,
    pub devices: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct Audio {
    pub volume: u8,
    pub muted: bool,
    pub output: String,
}

#[derive(Clone, Debug)]
pub struct Battery {
    pub percent: u8,
    pub status: String,
}

#[derive(Clone, Debug)]
pub struct Notifications {
    pub count: u32,
    pub dnd: bool,
}

#[derive(Clone, Debug)]
pub enum Action {
    Wifi(bool),
    Bluetooth(bool),
    Volume(u8),
    Mute(bool),
    Brightness(u8),
    Dnd(bool),
    Notifications,
    Settings,
    Poweroff,
    Reboot,
    Suspend,
}

impl Action {
    fn key(&self) -> std::mem::Discriminant<Self> {
        std::mem::discriminant(self)
    }
}

#[derive(Clone, Debug)]
pub struct Update {
    pub snapshot: Snapshot,
    pub generation: u64,
    pub error: Option<String>,
}

pub struct Service {
    settings: Arc<Mutex<Option<Vec<String>>>>,
    tx: SyncSender<(u64, Action)>,
    updates: Updates,
    pub generation: u64,
}

// The receiver stays with the service so a subscription can start after the
// first poll. A watch channel retains the newest result without blocking workers.
#[derive(Clone)]
struct Updates(Arc<tokio::sync::watch::Receiver<Option<Update>>>);

impl std::hash::Hash for Updates {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        std::ptr::hash(Arc::as_ptr(&self.0), state);
    }
}

impl Updates {
    fn stream(&self) -> impl cosmic::iced::futures::Stream<Item = Update> + use<> {
        cosmic::iced::futures::stream::unfold(self.0.as_ref().clone(), |mut receiver| async move {
            loop {
                receiver.changed().await.ok()?;
                let update = receiver.borrow_and_update().clone();
                if let Some(update) = update {
                    return Some((update, receiver));
                }
            }
        })
    }
}

type PollState = (Mutex<(u64, bool, Option<String>, bool)>, Condvar);

fn wait_for_poll(shared: &PollState) -> Option<u64> {
    let state = shared
        .1
        .wait_while(shared.0.lock().unwrap(), |state| state.1 && !state.3)
        .unwrap();
    (!state.3).then_some(state.0)
}

impl Service {
    pub fn start(settings: Option<Vec<String>>) -> Self {
        let live_settings = Arc::new(Mutex::new(settings));
        let settings = live_settings.clone();
        let (tx, commands) = mpsc::sync_channel::<(u64, Action)>(64);
        let (updates, rx) = tokio::sync::watch::channel(None);
        // Polling and writes have separate workers: a missing D-Bus service must
        // never hold up volume/brightness changes. Publish only coherent polls.
        let shared = Arc::new((
            Mutex::new((0u64, false, None::<String>, false)),
            Condvar::new(),
        ));
        let polling = shared.clone();
        thread::spawn(move || {
            let mut system_bus = StatusBus::new(true);
            let mut session_bus = StatusBus::new(false);
            loop {
                // A poll overlapping a write is discarded below. Wait for the
                // write to finish before retrying instead of launching commands
                // repeatedly while a slow control operation is still running.
                let Some(before) = wait_for_poll(&polling) else {
                    break;
                };
                let snapshot = poll(&mut system_bus, &mut session_bus);
                let state = polling.0.lock().unwrap();
                if state.3 {
                    break;
                }
                if state.0 == before && !state.1 {
                    updates.send_replace(Some(Update {
                        snapshot,
                        generation: state.0,
                        error: state.2.clone(),
                    }));
                    let _ = polling.1.wait_timeout(state, Duration::from_secs(2));
                }
            }
        });
        thread::spawn(move || {
            loop {
                let first = match commands.recv() {
                    Ok(command) => command,
                    Err(_) => break,
                };
                let mut pending = vec![first];
                for command in commands.try_iter() {
                    pending.retain(|(_, action)| action.key() != command.1.key());
                    pending.push(command);
                }
                for (id, action) in pending {
                    {
                        let mut state = shared.0.lock().unwrap();
                        state.0 = id;
                        state.1 = true;
                    }
                    let settings = settings.lock().unwrap().clone();
                    let error = execute(&action, settings.as_deref()).err();
                    let mut state = shared.0.lock().unwrap();
                    state.1 = false;
                    state.2 = error;
                    shared.1.notify_one();
                }
            }
            shared.0.lock().unwrap().3 = true;
            shared.1.notify_one();
        });
        Self {
            settings: live_settings,
            tx,
            updates: Updates(Arc::new(rx)),
            generation: 0,
        }
    }

    pub fn send(&mut self, action: Action) -> Result<(), String> {
        let generation = self.generation + 1;
        self.tx
            .try_send((generation, action))
            .map_err(|_| "Controls are busy; please try again".to_owned())?;
        self.generation = generation;
        Ok(())
    }

    pub fn update_settings(&self, settings: Option<Vec<String>>) {
        *self.settings.lock().unwrap() = settings;
    }

    pub fn subscription(&self) -> cosmic::iced::Subscription<Update> {
        cosmic::iced::Subscription::run_with(self.updates.clone(), Updates::stream)
    }
}

pub fn available(program: &str) -> bool {
    use std::os::unix::fs::PermissionsExt;
    let executable = |p: &Path| {
        p.metadata()
            .is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
    };
    if program.contains('/') {
        return executable(Path::new(program));
    }
    env::var_os("PATH")
        .is_some_and(|paths| env::split_paths(&paths).any(|path| executable(&path.join(program))))
}

fn run(program: &str, args: &[&str]) -> Result<String, String> {
    if !available(program) {
        return Err(format!("{program} is not installed"));
    }
    // Coreutils timeout bounds disconnected D-Bus services, too. Never invoke a shell.
    let output = Command::new("timeout")
        .args(["--kill-after=1s", "2s", program])
        .args(args)
        .env("LC_ALL", "C")
        .stdin(Stdio::null())
        .output()
        .map_err(|e| e.to_string())?;
    if !output.status.success() {
        let message = String::from_utf8_lossy(&output.stderr)
            .trim()
            .chars()
            .take(180)
            .collect::<String>();
        return Err(if message.is_empty() {
            format!("{program} did not complete successfully")
        } else {
            message
        });
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

// Connections live only on the polling worker. Each method still runs on every
// poll with a bounded reply timeout; capability values are never cached.
struct StatusBus {
    system: bool,
    connection: Option<zbus::blocking::Connection>,
}

impl StatusBus {
    fn new(system: bool) -> Self {
        Self {
            system,
            connection: None,
        }
    }

    fn query<T>(
        &mut self,
        query: impl FnOnce(&zbus::blocking::Connection) -> zbus::Result<T>,
    ) -> Option<T> {
        if self.connection.is_none() {
            let builder = if self.system {
                zbus::blocking::connection::Builder::system()
            } else {
                zbus::blocking::connection::Builder::session()
            };
            self.connection = builder
                .ok()?
                .method_timeout(Duration::from_secs(2))
                .build()
                .ok();
        }
        match query(self.connection.as_ref()?) {
            Ok(value) => Some(value),
            Err(_) => {
                // A disconnected/restarted bus must be rediscovered next poll.
                self.connection = None;
                None
            }
        }
    }

    fn can_power(&mut self, method: &str) -> bool {
        self.query(|connection| {
            connection
                .call_method(
                    Some("org.freedesktop.login1"),
                    "/org/freedesktop/login1",
                    Some("org.freedesktop.login1.Manager"),
                    method,
                    &(),
                )?
                .body()
                .deserialize::<String>()
        })
        .is_some_and(|value| value == "yes" || value == "challenge")
    }

    fn notification_service_owned(&mut self) -> bool {
        self.query(|connection| {
            connection
                .call_method(
                    Some("org.freedesktop.DBus"),
                    "/org/freedesktop/DBus",
                    Some("org.freedesktop.DBus"),
                    "NameHasOwner",
                    &("org.erikreider.swaync",),
                )?
                .body()
                .deserialize::<bool>()
        })
        .unwrap_or(false)
    }
}

fn poll(system_bus: &mut StatusBus, session_bus: &mut StatusBus) -> Snapshot {
    Snapshot {
        network: system_bus.query(network::read).flatten(),
        bluetooth: system_bus.query(bluetooth::read).flatten(),
        audio: audio(),
        battery: battery(),
        brightness: brightness(),
        notifications: notifications(session_bus),
        poweroff: system_bus.can_power("CanPowerOff"),
        reboot: system_bus.can_power("CanReboot"),
        suspend: system_bus.can_power("CanSuspend"),
    }
}

pub fn parse_audio(value: &str) -> Option<(u8, bool)> {
    let volume = value
        .strip_prefix("Volume: ")?
        .split_whitespace()
        .next()?
        .parse::<f32>()
        .ok()?;
    if !volume.is_finite() || volume < 0.0 {
        return None;
    }
    Some((
        (volume * 100.0).round().clamp(0.0, 100.0) as u8,
        value.contains("[MUTED]"),
    ))
}

fn audio() -> Option<Audio> {
    let (volume, muted) =
        parse_audio(&run("wpctl", &["get-volume", "@DEFAULT_AUDIO_SINK@"]).ok()?)?;
    let info = run("wpctl", &["inspect", "@DEFAULT_AUDIO_SINK@"]).ok()?;
    let output = info
        .lines()
        .find_map(|line| line.trim().strip_prefix("node.description = "))
        .map(|s| s.trim_matches('"').to_owned())
        .unwrap_or_else(|| "Default output".into());
    Some(Audio {
        volume,
        muted,
        output,
    })
}

fn read(path: &Path, name: &str) -> Option<String> {
    fs::read_to_string(path.join(name))
        .ok()
        .map(|s| s.trim().to_owned())
}

fn battery() -> Option<Battery> {
    let entries = fs::read_dir("/sys/class/power_supply").ok()?;
    // Prefer a system battery, not a mouse/headset battery.
    let mut batteries = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            read(p, "type").as_deref() == Some("Battery")
                && read(p, "scope").as_deref() != Some("Device")
        })
        .collect::<Vec<_>>();
    batteries.sort();
    batteries.iter().find_map(|path| {
        Some(Battery {
            percent: read(path, "capacity")?.parse::<u8>().ok()?.min(100),
            status: read(path, "status")?,
        })
    })
}

fn brightness() -> Option<u8> {
    parse_brightness(
        &run(
            "brightnessctl",
            &["--class=backlight", "--machine-readable", "info"],
        )
        .ok()?,
    )
}

fn parse_brightness(value: &str) -> Option<u8> {
    // device,class,current,percentage,max. Compute the ratio ourselves to
    // preserve the previous rounding rather than using the CLI's percentage.
    let mut fields = value.trim().split(',');
    let _device = fields.next()?;
    if fields.next()? != "backlight" {
        return None;
    }
    let current = fields.next()?.parse::<f64>().ok()?;
    let _percentage = fields.next()?;
    let max = fields.next()?.parse::<f64>().ok()?;
    (fields.next().is_none()
        && max.is_finite()
        && max > 0.0
        && current.is_finite()
        && current >= 0.0)
        .then(|| (100.0 * current / max).round().clamp(0.0, 100.0) as u8)
}

fn notifications(bus: &mut StatusBus) -> Option<Notifications> {
    // Check service ownership first: swaync-client otherwise waits indefinitely.
    if !bus.notification_service_owned() {
        return None;
    }
    let count = run("swaync-client", &["--count"]).ok()?.parse().ok()?;
    let dnd = match run("swaync-client", &["--get-dnd"]).ok()?.as_str() {
        "true" => true,
        "false" => false,
        _ => return None,
    };
    Some(Notifications { count, dnd })
}

fn execute(action: &Action, settings: Option<&[String]>) -> Result<(), String> {
    match action {
        Action::Wifi(on) => run("nmcli", &["radio", "wifi", if *on { "on" } else { "off" }]),
        Action::Bluetooth(on) => run("bluetoothctl", &["power", if *on { "on" } else { "off" }]),
        Action::Volume(value) => run(
            "wpctl",
            &[
                "set-volume",
                "-l",
                "1.0",
                "@DEFAULT_AUDIO_SINK@",
                &format!("{}%", value.min(&100)),
            ],
        ),
        Action::Mute(on) => run(
            "wpctl",
            &[
                "set-mute",
                "@DEFAULT_AUDIO_SINK@",
                if *on { "1" } else { "0" },
            ],
        ),
        Action::Brightness(value) => run(
            "brightnessctl",
            &[
                "--class=backlight",
                "set",
                &format!("{}%", value.clamp(&1, &100)),
            ],
        ),
        Action::Dnd(on) => run(
            "swaync-client",
            &[if *on { "--dnd-on" } else { "--dnd-off" }],
        ),
        Action::Notifications => run("swaync-client", &["--open-panel"]),
        Action::Poweroff => run("systemctl", &["poweroff"]),
        Action::Reboot => run("systemctl", &["reboot"]),
        Action::Suspend => run("systemctl", &["suspend"]),
        Action::Settings => {
            let argv = settings
                .filter(|argv| !argv.is_empty())
                .ok_or("No settings application configured")?;
            let mut command = Command::new(&argv[0]);
            command
                .args(&argv[1..])
                .stdin(Stdio::null())
                .env_remove("WAYLAND_SOCKET")
                .env_remove("FERESE_SHELL_CONTROL_SOCKET");
            if let Some(display) = env::var_os("FERESE_PUBLIC_WAYLAND_DISPLAY") {
                command.env("WAYLAND_DISPLAY", display);
            }
            command.env_remove("FERESE_PUBLIC_WAYLAND_DISPLAY");
            let mut child = command.spawn().map_err(|e| e.to_string())?;
            thread::spawn(move || {
                let _ = child.wait();
            });
            return Ok(());
        }
    }
    .map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(super) struct TestBus {
        daemon: std::process::Child,
        pub(super) address: String,
    }
    impl TestBus {
        pub(super) fn new() -> Self {
            use std::io::{BufRead, BufReader};
            let mut daemon = Command::new("dbus-daemon")
                .args(["--session", "--nofork", "--nopidfile", "--print-address=1"])
                .stdout(Stdio::piped())
                .spawn()
                .unwrap();
            let mut address = String::new();
            BufReader::new(daemon.stdout.take().unwrap())
                .read_line(&mut address)
                .unwrap();
            Self {
                daemon,
                address: address.trim().to_owned(),
            }
        }
        pub(super) fn connect(&self) -> zbus::blocking::Connection {
            zbus::blocking::connection::Builder::address(self.address.as_str())
                .unwrap()
                .method_timeout(Duration::from_millis(200))
                .build()
                .unwrap()
        }
    }
    impl Drop for TestBus {
        fn drop(&mut self) {
            let _ = self.daemon.kill();
            let _ = self.daemon.wait();
        }
    }

    struct MockLogin1(Arc<std::sync::atomic::AtomicU8>);

    #[zbus::interface(name = "org.freedesktop.login1.Manager")]
    impl MockLogin1 {
        fn can_power_off(&self) -> zbus::fdo::Result<String> {
            use std::sync::atomic::Ordering;
            match self.0.load(Ordering::Relaxed) {
                0 => Ok("yes".into()),
                1 => Ok("no".into()),
                3 => {
                    thread::sleep(Duration::from_millis(500));
                    Ok("yes".into())
                }
                _ => Err(zbus::fdo::Error::Failed("test failure".into())),
            }
        }
        fn can_reboot(&self) -> String {
            "challenge".into()
        }
        fn can_suspend(&self) -> String {
            "na".into()
        }
    }

    #[test]
    #[ignore = "requires dbus-daemon; uses a private test bus"]
    fn native_bus_queries_track_live_changes_and_fail_closed() {
        use std::io::{BufRead, BufReader};
        use std::sync::atomic::{AtomicU8, Ordering};
        struct Daemon(std::process::Child);
        impl Drop for Daemon {
            fn drop(&mut self) {
                let _ = self.0.kill();
                let _ = self.0.wait();
            }
        }
        let mut daemon = Daemon(
            Command::new("dbus-daemon")
                .args(["--session", "--nofork", "--nopidfile", "--print-address=1"])
                .stdout(Stdio::piped())
                .spawn()
                .unwrap(),
        );
        let mut address = String::new();
        BufReader::new(daemon.0.stdout.take().unwrap())
            .read_line(&mut address)
            .unwrap();
        let mode = Arc::new(AtomicU8::new(0));
        let server = zbus::blocking::connection::Builder::address(address.trim())
            .unwrap()
            .name("org.freedesktop.login1")
            .unwrap()
            .serve_at("/org/freedesktop/login1", MockLogin1(mode.clone()))
            .unwrap()
            .build()
            .unwrap();
        let connect = || {
            zbus::blocking::connection::Builder::address(address.trim())
                .unwrap()
                .method_timeout(Duration::from_millis(200))
                .build()
                .unwrap()
        };
        let mut bus = StatusBus {
            system: false,
            connection: Some(connect()),
        };
        assert!(bus.can_power("CanPowerOff"));
        assert!(bus.can_power("CanReboot"));
        assert!(!bus.can_power("CanSuspend"));
        mode.store(1, Ordering::Relaxed);
        assert!(
            !bus.can_power("CanPowerOff"),
            "capabilities must not be cached"
        );
        assert!(bus.connection.is_some());
        assert!(!bus.notification_service_owned());
        server.request_name("org.erikreider.swaync").unwrap();
        assert!(bus.notification_service_owned());
        server.release_name("org.erikreider.swaync").unwrap();
        assert!(!bus.notification_service_owned());
        mode.store(2, Ordering::Relaxed);
        assert!(!bus.can_power("CanPowerOff"));
        assert!(
            bus.connection.is_none(),
            "failed connections must be eligible for reconnection"
        );
        bus.connection = Some(connect());
        assert!(!bus.can_power("MissingMethod"));
        assert!(bus.connection.is_none());
        bus.connection = Some(connect());
        mode.store(3, Ordering::Relaxed);
        assert!(
            !bus.can_power("CanPowerOff"),
            "a reply beyond the method deadline must fail closed"
        );
        assert!(bus.connection.is_none());
    }

    #[test]
    fn brightness_info_preserves_ratio_rounding_and_rejects_bad_devices() {
        assert_eq!(parse_brightness("intel,backlight,72,18%,400"), Some(18));
        assert_eq!(parse_brightness("panel,backlight,2,66%,3\n"), Some(67));
        assert_eq!(parse_brightness("panel,backlight,0,0%,400"), Some(0));
        for value in [
            "",
            "panel,leds,1,1%,100",
            "panel,backlight,1,0%,0",
            "panel,backlight,NaN,0%,100",
            "panel,backlight,1,0%,inf",
            "panel,backlight,-1,0%,100",
            "panel,backlight,1,0%,100,extra",
        ] {
            assert_eq!(parse_brightness(value), None, "{value}");
        }
    }

    #[test]
    fn status_stream_retains_latest_result_wakes_and_closes() {
        use cosmic::iced::futures::{
            Stream,
            task::{ArcWake, waker},
        };
        use std::{
            sync::atomic::{AtomicUsize, Ordering},
            task::{Context, Poll},
        };

        #[derive(Default)]
        struct WakeCount(AtomicUsize);
        impl ArcWake for WakeCount {
            fn wake_by_ref(arc_self: &Arc<Self>) {
                arc_self.0.fetch_add(1, Ordering::Relaxed);
            }
        }
        let result = |generation| {
            Some(Update {
                snapshot: Snapshot {
                    brightness: Some(42),
                    ..Snapshot::default()
                },
                generation,
                error: Some("service unavailable".into()),
            })
        };
        let (sender, receiver) = tokio::sync::watch::channel(None);
        let updates = Updates(Arc::new(receiver));
        // Results produced before the UI subscribes remain available; a slow
        // consumer gets the newest generation instead of a stale queued result.
        sender.send_replace(result(1));
        sender.send_replace(result(2));
        let mut stream = Box::pin(updates.stream());
        let wakes = Arc::new(WakeCount::default());
        let waker = waker(wakes.clone());
        let mut context = Context::from_waker(&waker);
        let Poll::Ready(Some(update)) = stream.as_mut().poll_next(&mut context) else {
            panic!("startup result missing");
        };
        assert_eq!(update.generation, 2);
        assert_eq!(update.snapshot.brightness, Some(42));
        assert_eq!(update.error.as_deref(), Some("service unavailable"));
        assert!(stream.as_mut().poll_next(&mut context).is_pending());
        let before = wakes.0.load(Ordering::Relaxed);
        thread::spawn(move || {
            sender.send_replace(result(3));
            // Closing still delivers the last unseen update before ending.
        })
        .join()
        .unwrap();
        assert!(wakes.0.load(Ordering::Relaxed) > before);
        let Poll::Ready(Some(update)) = stream.as_mut().poll_next(&mut context) else {
            panic!("worker result missing");
        };
        assert_eq!(update.generation, 3);
        assert!(matches!(
            stream.as_mut().poll_next(&mut context),
            Poll::Ready(None)
        ));
    }

    #[test]
    fn polling_waits_for_a_write_and_uses_the_completed_generation() {
        let shared = Arc::new((Mutex::new((7, true, None, false)), Condvar::new()));
        let worker_state = shared.clone();
        let (sender, receiver) = mpsc::channel();
        let worker = thread::spawn(move || {
            sender.send(wait_for_poll(&worker_state)).unwrap();
        });
        assert!(matches!(
            receiver.recv_timeout(Duration::from_millis(30)),
            Err(mpsc::RecvTimeoutError::Timeout)
        ));
        {
            let mut state = shared.0.lock().unwrap();
            state.0 = 8;
            state.1 = false;
            shared.1.notify_one();
        }
        assert_eq!(
            receiver.recv_timeout(Duration::from_secs(2)).unwrap(),
            Some(8)
        );
        worker.join().unwrap();
    }

    #[test]
    fn shutdown_unblocks_polling_even_during_a_write() {
        let shared = Arc::new((Mutex::new((7, true, None, false)), Condvar::new()));
        let worker_state = shared.clone();
        let (sender, receiver) = mpsc::channel();
        let worker = thread::spawn(move || {
            sender.send(wait_for_poll(&worker_state)).unwrap();
        });
        {
            let mut state = shared.0.lock().unwrap();
            state.3 = true;
            shared.1.notify_one();
        }
        assert_eq!(receiver.recv_timeout(Duration::from_secs(2)).unwrap(), None);
        worker.join().unwrap();
    }
    #[test]
    fn audio_parser_handles_mute_and_rejects_invalid_values() {
        assert_eq!(parse_audio("Volume: 0.42 [MUTED]"), Some((42, true)));
        assert_eq!(parse_audio("Volume: 1.2"), Some((100, false)));
        for value in ["", "Volume: NaN", "Volume: -1", "error"] {
            assert_eq!(parse_audio(value), None);
        }
    }

    #[test]
    fn default_snapshot_does_not_invent_services() {
        let s = Snapshot::default();
        assert!(s.network.is_none() && s.audio.is_none() && s.notifications.is_none());
        assert!(!s.poweroff && !s.reboot && !s.suspend);
    }
}
