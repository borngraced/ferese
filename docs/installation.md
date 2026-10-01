# Installation

Ferese runs on Linux as its own Wayland session, alongside your existing desktop, and
you can launch it from a login manager, a command-based greeter or a local TTY.

## Requirements

For a full session, you’ll need a Wayland-capable graphics driver and a local user
session with seat access through logind or seatd, while a nested preview runs inside the
Wayland desktop you already use.

To build Ferese, install **Rust 1.95 or newer**, a C/C++ toolchain, CMake, pkg-config
and the libraries below. The installer builds the Ferese components once those system
packages are in place.

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

Fedora is the tested installation path, and package names or versions may differ on
other distributions, where the [Smithay
dependencies](https://github.com/Smithay/smithay#system-dependencies) can help you find
equivalent libraries.

### Rust toolchain

Check your toolchain:

```sh
rustc --version
cargo --version
```

If you don’t have Rust yet, follow the instructions at [rustup.rs](https://rustup.rs/)
and reopen your terminal, then select the required toolchain for this checkout after
cloning:

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

The script builds locked release binaries as your normal user and then asks for
administrator access through `sudo` or `pkexec` to install them, so leave the build
itself unprivileged. The first build needs network access to fetch Rust dependencies.

Installation brings together the session launcher, desktop tools, portal backend, login
entry, icons, default wallpaper and example config. Releases live under
`/usr/local/lib/ferese/releases/` and commands under `/usr/local/bin/`, with `current`
and `previous` links for switching between releases during updates and rollback.

Your configuration, other desktop sessions and existing PAM policy stay in place, while
Ferese starts its authentication agent and portal services together with the session.
The authentication agent registers for the login session, so requests from desktop
apps use the Ferese dialog. Remove other polkit agents from your Ferese autostart
configuration to avoid competing session registrations.

### Initial configuration

You can start with Ferese’s built-in defaults, or copy the example config if you’d like
a file to customize:

```sh
mkdir -p "${XDG_CONFIG_HOME:-$HOME/.config}/ferese"
if [ ! -e "${XDG_CONFIG_HOME:-$HOME/.config}/ferese/config.kdl" ]; then
  cp /usr/local/lib/ferese/current/config.example.kdl \
    "${XDG_CONFIG_HOME:-$HOME/.config}/ferese/config.kdl"
fi
```

After login, open **Control Center → Settings** or run `ferese-settings` to shape your
desktop, using [Configuration](configuration.md) for themes, displays, shortcuts and
startup apps. The default terminal is `foot`, which you can replace with your preferred
terminal command in Settings.

## Start a session

### Graphical login managers

Save your work and log out, then open your login screen’s session selector, choose
**Ferese** and sign in. Login managers that support Wayland sessions can discover the
installed entry in `/usr/share/wayland-sessions/`.

If Ferese is missing, confirm that the session entry exists and its launcher is
executable:

```sh
ls -l /usr/share/wayland-sessions/ferese.desktop
test -x /usr/local/bin/ferese-session && echo "Session launcher is ready"
desktop-file-validate /usr/share/wayland-sessions/ferese.desktop
```

If the entry still doesn’t appear, check your login manager’s documentation for Wayland
session discovery, or use a Wayland-capable greeter or the TTY method below if your
manager only launches X11 sessions.

### Command-based greeters

If your greeter asks for a session command instead of reading desktop files, use:

```sh
/usr/local/bin/ferese-session
```

For greetd, set this as the session command after authentication using the [greetd
documentation](https://sr.ht/~kennylevinsen/greetd/), and the launcher will start the
compositor, shell and session services together.

### From a TTY

Log out of your graphical session and switch to a local console with **Ctrl+Alt+F2**, or
another available function key, then sign in as your normal user and run:

```sh
/usr/local/bin/ferese-session
```

Your login needs to provide `XDG_RUNTIME_DIR` and access to the active seat through
logind or seatd; if either is missing, fix your distribution’s login and session setup
rather than running Ferese as root or making device nodes writable. On non-systemd
systems, follow the distribution’s seatd and user-session instructions, and use a nested
preview whenever you’re already inside a graphical session.

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

Package names and availability vary by distribution, so use Satty’s [upstream
instructions](https://github.com/Satty-org/Satty#install) if it isn’t in your
repositories. Ferese leaves service setup to you and preserves your existing service
configuration during installation.

Use **Print Screen** to capture the active monitor, **Super+Shift+S** to select an area,
or `ferese-screenshot --all` to capture all monitors in one image.

The native locker follows your shell theme and can be personalized in **Settings → Lock
Screen**, with `ferese-lock --preview` opening an ordinary preview window. Before
enabling automatic locking, test real unlocking and read [Native locker](locking.md) for
authentication, idle locking and limits.

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

The session launcher saves logs in `${XDG_STATE_HOME:-$HOME/.local/state}/ferese/`,
while a source build launched directly writes to its terminal. Keep another desktop
available when testing scaling, monitor hotplug, suspend or locking so you have a way
back if something goes wrong.

The launcher enables screencopy for screenshot tools, allowing Wayland clients to
capture the unlocked desktop and making `feresectl screenshot` available. Set
`FERESE_ENABLE_SCREENCOPY=0` in the session environment to disable every screen-reading
path, including the built-in screenshot command, at your next login; capture is always
blocked while the session is locked.

When you launch the compositor directly, set `FERESE_ENABLE_SCREENCOPY=1` to enable
screenshots, since that variable is otherwise unset and screen capture stays disabled.

## Updating

From your existing checkout, with any local edits saved:

```sh
git pull --ff-only
./scripts/install.sh
```

Log out and back in to start using the new release, as a running session keeps its
existing processes and release paths until it restarts.

Each release records the portal configuration it ships so later upgrades can identify
it, and installation stops if it encounters modified or untracked administrator files.
To replace Ferese portal configuration from an earlier development install, build first
and run `./scripts/install.sh --skip-build --replace-portal-config`, which backs up the
existing files in the new release’s `portal-config.previous/` directory.

Installer options:

```sh
./scripts/install.sh --dry-run
./scripts/install.sh --offline --release-id my-build
./scripts/install.sh --skip-build
```

Use `--dry-run` to see the planned commands, `--offline` when dependencies are already
cached, or `--skip-build` after building every component from the revision you want to
install. The latter uses binaries already in `target/release/`, and
`./scripts/install.sh --help` lists all available options.

## Logout and recovery

Save your work before leaving: **Super+Shift+E** and `feresectl request-logout` ask for
confirmation, while `feresectl exit` ends the session immediately. If Ferese freezes,
switch to another TTY with **Ctrl+Alt+F1–F12**, sign in and find the affected compositor
process:

```sh
pgrep -a -u "$USER" -x ferese
```

Replace `PID` with the affected session’s process ID and use `kill PID` to end it,
bearing in mind that its applications will close and unsaved work may be lost. You can
then choose another desktop at the next login.

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

Log in again to use the selected binaries, with your config and PAM policy still in
place. If you also need to recover a configuration edit, Settings keeps a backup when
saving, as described in [Saving and validation](configuration.md#saving-and-validation).

### Remove the login option

To hide Ferese from login managers while keeping its releases and your config:

```sh
sudo mv /usr/share/wayland-sessions/ferese.desktop \
  /usr/local/lib/ferese/ferese.desktop.disabled
```

For a command-based greeter, remove Ferese from its session choices in the greeter’s
configuration; reinstalling Ferese restores the desktop session entry.

## Screen sharing and recording

Ferese’s native ScreenCast portal is included in the installation, which backs up the
old user service override pointing to `~/.local/libexec/ferese` so new sessions use the
installed release. Custom overrides are preserved and reported before building, and
[Screen sharing](screen-sharing.md) covers consent, PipeWire setup, supported sources
and recording limits.
