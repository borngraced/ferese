# Installation

## Build and install

On Fedora with SDDM, install a Rust toolchain (1.93 or newer), native build
dependencies, `desktop-file-utils`, D-Bus and `foot`. Wallpaper browsing also
needs `zenity`; locking needs swaylock or swaylock-effects.
Area screenshots need `grim`, `slurp`, and `wl-clipboard`.

From the checkout, run as your normal user:

```sh
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
`/usr/local/lib/ferese/current/config.example.toml` to
`~/.config/ferese/config.toml`. See [Configuration](configuration.md).

## Preview and logs

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
