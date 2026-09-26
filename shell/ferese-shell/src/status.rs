//! Bounded, off-UI-thread adapters. A missing service is `None`, never a fake state.
use std::{
    env, fs,
    path::Path,
    process::{Command, Stdio},
    sync::{
        Arc, Condvar, Mutex,
        mpsc::{self, Receiver, SyncSender},
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
pub struct Update {
    pub snapshot: Snapshot,
    pub generation: u64,
    pub error: Option<String>,
}
pub struct Service {
    tx: SyncSender<(u64, Action)>,
    rx: Receiver<Update>,
    pub generation: u64,
}
impl Service {
    pub fn start(settings: Option<Vec<String>>) -> Self {
        let (tx, commands) = mpsc::sync_channel::<(u64, Action)>(64);
        let (updates, rx) = mpsc::sync_channel(1);
        // Polling and writes have separate workers: a missing D-Bus service must
        // never hold up volume/brightness changes. Publish only coherent polls.
        let shared = Arc::new((
            Mutex::new((0u64, false, None::<String>, false)),
            Condvar::new(),
        ));
        let polling = shared.clone();
        thread::spawn(move || {
            loop {
                let before = {
                    let state = polling.0.lock().unwrap();
                    if state.3 {
                        break;
                    }
                    state.0
                };
                let snapshot = poll();
                let state = polling.0.lock().unwrap();
                if state.3 {
                    break;
                }
                if state.0 == before && !state.1 {
                    let _ = updates.try_send(Update {
                        snapshot,
                        generation: state.0,
                        error: state.2.clone(),
                    });
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
            tx,
            rx,
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
    pub fn poll(&self) -> Option<Update> {
        self.rx.try_iter().last()
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

fn poll() -> Snapshot {
    Snapshot {
        network: network(),
        bluetooth: bluetooth(),
        audio: audio(),
        battery: battery(),
        brightness: brightness(),
        notifications: notifications(),
        poweroff: can_power("CanPowerOff"),
        reboot: can_power("CanReboot"),
        suspend: can_power("CanSuspend"),
    }
}
fn network() -> Option<Network> {
    // No Wi-Fi device means no Wi-Fi control (wired connectivity isn't called Wi-Fi).
    let devices = run("nmcli", &["-t", "-f", "TYPE", "device", "status"]).ok()?;
    if !devices.lines().any(|line| line == "wifi") {
        return None;
    }
    let enabled = run("nmcli", &["radio", "wifi"]).ok()? == "enabled";
    let mut connection = None;
    let mut signal = 0;
    if enabled {
        let rows = run(
            "nmcli",
            &[
                "-t",
                "-f",
                "IN-USE,SIGNAL,SSID",
                "device",
                "wifi",
                "list",
                "--rescan",
                "no",
            ],
        )
        .ok()?;
        for line in rows.lines() {
            if let Some((strength, ssid)) = line.strip_prefix("*:").and_then(|s| s.split_once(':'))
            {
                signal = strength.parse::<u8>().ok()?.min(100);
                connection = Some(ssid.replace("\\:", ":").replace("\\\\", "\\"));
                break;
            }
        }
    }
    Some(Network {
        enabled,
        connection,
        signal,
    })
}
fn bluetooth() -> Option<Bluetooth> {
    let info = run("bluetoothctl", &["show"]).ok()?;
    let enabled = info
        .lines()
        .find_map(|line| line.trim().strip_prefix("Powered: "))?
        == "yes";
    let devices = if enabled {
        run("bluetoothctl", &["devices", "Connected"])
            .ok()?
            .lines()
            .filter_map(|line| {
                line.strip_prefix("Device ")
                    .and_then(|s| s.split_once(' '))
                    .map(|(_, name)| name.to_owned())
            })
            .collect()
    } else {
        Vec::new()
    };
    Some(Bluetooth { enabled, devices })
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
    let max = run("brightnessctl", &["--class=backlight", "max"])
        .ok()?
        .parse::<f64>()
        .ok()?;
    let current = run("brightnessctl", &["--class=backlight", "get"])
        .ok()?
        .parse::<f64>()
        .ok()?;
    (max > 0.0 && current.is_finite())
        .then(|| (100.0 * current / max).round().clamp(0.0, 100.0) as u8)
}
fn notifications() -> Option<Notifications> {
    // Check service ownership first: swaync-client otherwise waits indefinitely.
    let owner = run(
        "busctl",
        &[
            "--user",
            "call",
            "org.freedesktop.DBus",
            "/org/freedesktop/DBus",
            "org.freedesktop.DBus",
            "NameHasOwner",
            "s",
            "org.erikreider.swaync",
        ],
    )
    .ok()?;
    if owner != "b true" {
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
fn can_power(method: &str) -> bool {
    run(
        "busctl",
        &[
            "--system",
            "call",
            "org.freedesktop.login1",
            "/org/freedesktop/login1",
            "org.freedesktop.login1.Manager",
            method,
        ],
    )
    .is_ok_and(|s| s == "s \"yes\"" || s == "s \"challenge\"")
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
