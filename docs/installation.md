# Installation

## Build and install

On Fedora with SDDM, install a Rust toolchain (1.95 or newer), native build
dependencies, `desktop-file-utils`, D-Bus and `foot`. Wallpaper browsing also
needs `zenity`; locking needs swaylock or swaylock-effects.
Screenshots need `grim`, `slurp`, `satty`, `wl-clipboard`, and Python 3.
Print Screen captures the active monitor; Super+Shift+S selects an area.
Use `ferese-screenshot --all` to capture all monitors in one image.

Clone the repository, then run the installer as your normal user:

```sh
git clone https://github.com/borngraced/ferese.git
cd ferese
./scripts/install.sh
```

The script builds locked release binaries, then requests elevation to install.
It preserves your configuration and other desktop sessions. Log out and select
**Ferese** at the login screen. Upgrades require logging out and back in too.

```sh
./scripts/install.sh --dry-run
./scripts/install.sh --offline --release-id my-build
./scripts/install.sh --skip-build
```

Use `--help` for all options. New configs can be copied from
`/usr/local/lib/ferese/current/config.example.kdl` to
`~/.config/ferese/config.kdl`. See [Configuration](configuration.md).

## Preview and logs

To preview a source build before installing, run these commands from the checkout
inside an existing Wayland desktop:

```sh
cargo build --release --locked -p ferese -p ferese-shell -p ferese-settings -p feresectl
target/release/ferese --backend nested --grant-effects --grant-shell-control -- \
  target/release/ferese-shell
```


Run `ferese-session --nested` inside an existing Wayland desktop. For a hardware
session, use the login screen or a spare TTY; never run Ferese as root or start
its DRM backend inside an active graphical session.

Logs are stored in `${XDG_STATE_HOME:-~/.local/state}/ferese/`. Keep an existing
desktop available while testing scaling, monitor hotplug, suspend and locking.

The session launcher enables the screencopy protocol for screenshot tools.
Set `FERESE_ENABLE_SCREENCOPY=0` in the session environment to disable it.
When enabled, Wayland clients can capture the unlocked desktop. Capture remains
blocked while the session is locked. Launcher changes take effect at next login.

## Logout and recovery

Save your work before **Super+Shift+E** or `feresectl exit`: logout is immediate.
If frozen, switch to another TTY with Ctrl+Alt+F1–F12, terminate the specific
Ferese process, and select your other desktop on the next login. Restarting SDDM
also ends other sessions, so avoid it with unsaved work.

Installed binaries live in `/usr/local/lib/ferese/releases/`. While Ferese is
stopped, select a retained release with the `current` symlink; `previous` points
to the prior installation. To hide the session option, move
`/usr/share/wayland-sessions/ferese.desktop` out of that directory.
