# Installation

Ferese runs on Linux and installs as a standalone Wayland session. Use any login
manager that can launch Wayland sessions, a command-based greeter, or a local
TTY. You do not need to replace your existing login manager or desktop.

## Requirements

Use a Wayland-capable graphics driver and a local user session with seat access
through logind or seatd. A nested preview runs inside an existing Wayland desktop.

Building requires **Rust 1.95 or newer**, a C/C++ toolchain, CMake, pkg-config,
and the libraries below. The installer builds all Ferese components; install
system packages separately.

### Fedora

```sh
sudo dnf install git curl gcc gcc-c++ make cmake pkgconf-pkg-config \
  wayland-devel libxkbcommon-devel libinput-devel systemd-devel libseat-devel \
  mesa-libgbm-devel mesa-libEGL-devel libdrm-devel fontconfig-devel \
  freetype-devel expat-devel dbus-daemon dbus-tools desktop-file-utils foot pam \
  clang clang-devel pipewire-devel pipewire wireplumber polkit polkit-libs \
  xdg-desktop-portal xdg-desktop-portal-gtk \
  gstreamer1-devel gstreamer1-plugins-base gstreamer1-plugins-good pipewire-gstreamer
```

### Arch Linux

```sh
sudo pacman -S --needed base-devel git curl cmake pkgconf wayland libxkbcommon \
  libinput systemd seatd mesa libdrm fontconfig freetype2 expat dbus \
  desktop-file-utils foot pam clang pipewire libpipewire wireplumber polkit \
  xdg-desktop-portal xdg-desktop-portal-gtk \
  gstreamer gst-plugins-base gst-plugins-good gst-plugin-pipewire
```

### Debian and Ubuntu

```sh
sudo apt update
sudo apt install build-essential git curl cmake pkg-config libwayland-dev \
  libxkbcommon-dev libinput-dev libudev-dev libseat-dev libgbm-dev libegl-dev \
  libdrm-dev libfontconfig1-dev libfreetype-dev libexpat1-dev dbus-bin \
  dbus-user-session desktop-file-utils foot libpam0g clang libclang-dev \
  libpipewire-0.3-dev libspa-0.2-dev pipewire wireplumber polkitd libpolkit-agent-1-0 \
  xdg-desktop-portal xdg-desktop-portal-gtk \
  libgstreamer1.0-dev gstreamer1.0-pipewire gstreamer1.0-plugins-base \
  gstreamer1.0-plugins-good
```

Fedora is the tested installation path. Package names and versions may differ
on other distributions. See [Smithay dependencies](https://github.com/Smithay/smithay#system-dependencies)
for equivalent libraries.

### Rust toolchain

Check your toolchain:

```sh
rustc --version
cargo --version
```

If Rust is missing, install it using the instructions at
[rustup.rs](https://rustup.rs/), then reopen your terminal. With rustup installed,
you can select the required version for this checkout after cloning:

```sh
rustup toolchain install 1.95.0
rustup override set 1.95.0
```

## Build and install

Run these commands as your normal user:

```sh
git clone https://github.com/ferese-wm/ferese.git
cd ferese
./scripts/install.sh
```

The script builds locked release binaries, then requests administrator access
through `sudo` or `pkexec` for installation. Do not run the whole build as root.
The first build needs network access to fetch Rust dependencies.

The installer adds the session launcher, desktop tools, portal backend, login
entry, icons, default wallpaper, and example config. Releases live under
`/usr/local/lib/ferese/releases/`; commands live under `/usr/local/bin/`.
The `current` and `previous` links support updates and rollback.

Your configuration, other desktop sessions, and existing PAM policy are preserved.
Ferese starts its authentication agent and portal services with the session.

### Initial configuration

Ferese has built-in defaults. To start from the example without replacing your config:

```sh
mkdir -p "${XDG_CONFIG_HOME:-$HOME/.config}/ferese"
if [ ! -e "${XDG_CONFIG_HOME:-$HOME/.config}/ferese/config.kdl" ]; then
  cp /usr/local/lib/ferese/current/config.example.kdl \
    "${XDG_CONFIG_HOME:-$HOME/.config}/ferese/config.kdl"
fi
```

Open **Control Center → Settings** after login, or run `ferese-settings`.
See [Configuration](configuration.md) for themes, displays, shortcuts, and startup
applications. The default terminal shortcut uses `foot`; change the terminal
command in Settings if you prefer another terminal.

## Start a session

### Graphical login managers

Save your work and log out. Open your login screen's session selector, choose
**Ferese**, and sign in. Any manager that supports Wayland sessions and reads
`/usr/share/wayland-sessions/` can discover the installed entry.

If Ferese is missing, confirm that the session entry exists and its launcher is
executable:

```sh
ls -l /usr/share/wayland-sessions/ferese.desktop
test -x /usr/local/bin/ferese-session && echo "Session launcher is ready"
desktop-file-validate /usr/share/wayland-sessions/ferese.desktop
```

Check your manager's documentation for enabling Wayland session discovery. A
manager that only launches X11 sessions cannot start Ferese through that path;
use a Wayland-capable greeter or the TTY method below.

### Command-based greeters

If your greeter asks for a session command instead of reading desktop files, use:

```sh
/usr/local/bin/ferese-session
```

For greetd, use this as the session command after authentication; see the
[greetd documentation](https://sr.ht/~kennylevinsen/greetd/).
The launcher starts the compositor, shell, and session services together.

### From a TTY

Log out of the graphical session, switch to a local console with
**Ctrl+Alt+F2** (or another available function key), and sign in as your normal
user. Then run:

```sh
/usr/local/bin/ferese-session
```

Your login must provide `XDG_RUNTIME_DIR` and access to the active seat through
logind or seatd. If either is missing, fix the distribution's login/session setup;
do not work around it by running Ferese as root or making device nodes writable.
For non-systemd systems, follow your distribution's seatd and user-session setup.
Do not start the DRM backend inside an existing graphical session; use a nested
preview there.

## Optional desktop tools

Install these through your distribution when you want the corresponding features:

| Feature | Tools or services |
| --- | --- |
| Wallpaper file picker | `zenity` |
| Screenshots and editing | `slurp`, `satty`, `wl-clipboard`, Python 3 |
| Idle locking | `swayidle` with the included `ferese-lock` |
| Network controls | NetworkManager and its running service |
| Audio controls | PipeWire, WirePlumber, and `wpctl` |
| Bluetooth controls | BlueZ, `bluetoothctl`, and its running service |

Package names and availability vary by distribution. In particular, install
Satty using its [upstream instructions](https://github.com/Satty-org/Satty#install)
if your repositories do not provide it. Ferese does not enable these services
or replace your existing service configuration during installation.

Print Screen captures the active monitor; **Super+Shift+S** selects an area.
Use `ferese-screenshot --all` to capture all monitors in one image.

The native locker follows your shell theme. Open **Settings → Lock Screen** to
customize it, or run `ferese-lock --preview` to see an ordinary preview window.
Test real unlocking before enabling automatic locking; see
[Native locker](locking.md) for authentication, idle locking, and limitations.

## Preview and logs

To preview a source build before installing, run these commands from the checkout
inside an existing Wayland desktop:

```sh
cargo build --release --locked -p ferese -p ferese-shell -p ferese-settings -p feresectl -p ferese-lock -p xdg-desktop-portal-ferese
FERESE_ENABLE_SCREENCOPY=1 target/release/ferese --backend nested \
  --grant-effects --grant-shell-control -- \
  target/release/ferese-shell
```

After installation, the equivalent launcher is:

```sh
ferese-session --nested
```

The session launcher stores logs in `${XDG_STATE_HOME:-$HOME/.local/state}/ferese/`.
A directly launched source build writes to its terminal. Keep another desktop
available while testing scaling, monitor hotplug, suspend, and locking.

The launcher enables the screencopy protocol for screenshot tools. Set
`FERESE_ENABLE_SCREENCOPY=0` in the session environment to disable it. This
turns off every path that can read the screen, including the built-in
screenshot command, not only the Wayland protocol. When enabled, Wayland clients
can capture the unlocked desktop, and `feresectl screenshot` is available.
Capture remains blocked while the session is locked. Launcher changes take
effect at next login.

Starting the compositor directly instead of through `ferese-session` leaves
`FERESE_ENABLE_SCREENCOPY` unset, which disables screenshots. Export
`FERESE_ENABLE_SCREENCOPY=1` to use them.

## Updating

From your existing checkout, with any local edits saved:

```sh
git pull --ff-only
./scripts/install.sh
```

Log out and back in to use the new release. Running sessions keep their existing
processes and release paths until restarted.

The installer records shipped portal configuration with each release for future
upgrades. Modified or untracked administrator files stop installation. To replace
Ferese portal configuration from an earlier development install, use
`./scripts/install.sh --skip-build --replace-portal-config` after building. Existing
files are backed up under the new release's `portal-config.previous/` directory.

Installer options:

```sh
./scripts/install.sh --dry-run
./scripts/install.sh --offline --release-id my-build
./scripts/install.sh --skip-build
```

`--dry-run` prints the planned commands. `--offline` requires dependencies to be
cached already. `--skip-build` installs the binaries already in `target/release/`;
use it only after building all components from the intended revision.
Run `./scripts/install.sh --help` for all options.

## Logout and recovery

Use **Super+Shift+E** or `feresectl request-logout` to ask for logout confirmation.
`feresectl exit` ends the session immediately; save your work first.
If Ferese freezes, switch to another TTY with **Ctrl+Alt+F1–F12**, sign in, and
identify the affected compositor process:

```sh
pgrep -a -u "$USER" -x ferese
```

Terminate only that session's process with `kill PID`, replacing `PID` with the
correct process ID. This ends its applications and can lose unsaved work. You
can then select your other desktop at the next login.

### Roll back a release

While Ferese is stopped, inspect the retained releases:

```sh
readlink /usr/local/lib/ferese/current
readlink /usr/local/lib/ferese/previous
ls /usr/local/lib/ferese/releases/
```

If `previous` exists and points to the release you want, switch back:

```sh
sudo sh -eu -c '
  cd /usr/local/lib/ferese
  previous_release=$(readlink previous)
  test -x "$previous_release/ferese-session"
  ln -sfn -- "$previous_release" current-rollback
  mv -Tf -- current-rollback current
'
```

Then log in again. This changes the selected binaries; it does not restore your
config or replace the PAM policy. Settings keeps a backup when saving config
changes; see [Saving and validation](configuration.md#saving-and-validation).

### Remove the login option

To hide Ferese from login managers while keeping its releases and your config:

```sh
sudo mv /usr/share/wayland-sessions/ferese.desktop \
  /usr/local/lib/ferese/ferese.desktop.disabled
```

For a command-based greeter, remove Ferese from its session choices using that
greeter's configuration. Reinstalling restores the desktop session entry.

## Screen sharing and recording

The installer includes Ferese’s native ScreenCast portal. See
[Screen sharing](screen-sharing.md) for consent, PipeWire setup, supported sources,
and current recording limitations.
