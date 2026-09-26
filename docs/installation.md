# Fedora / SDDM installation

From the checkout, run the installer as your normal user:

```sh
./scripts/install.sh
```

It builds all four release binaries with the locked dependencies, then uses
`sudo` (or `pkexec`) for installation. Install the Rust toolchain, native build
dependencies, `desktop-file-utils`, and D-Bus first. Cargo reports missing native
libraries during the build. The shell requires Rust 1.93 or newer.

Useful options:

```sh
./scripts/install.sh --dry-run
./scripts/install.sh --offline --release-id my-demo
./scripts/install.sh --skip-build --release-id existing-build
```

The script works from any directory and preserves your user configuration.
Use `--help` for all options. To test and install manually:

```sh
cargo test -p ferese -p feresectl -p ferese-animation -p ferese-core -p ferese-layout -p ferese-shell -p ferese-settings --locked
cargo build --release --locked -p ferese -p ferese-shell -p ferese-settings -p feresectl
sudo bash scripts/install-session.sh YOUR_RELEASE_ID
```

The installer adds **Ferese** to SDDM without changing Plasma or
the default session. It installs immutable, versioned directories in
`/usr/local/lib/ferese/releases/` and launchers in `/usr/local/bin/`.
`current` selects the installed build; `previous` records the prior selection
on upgrades. Installation refuses to replace unmanaged launchers/session files.

Keep your existing `~/.config/ferese/config.toml`. For a new configuration, copy
`/usr/local/lib/ferese/current/config.example.toml` there. The existing compositor
defaults handle connected monitors automatically; add output profiles only after
checking `feresectl get-outputs` on the actual DRM session.

Use `ferese-session --nested` for an installed-build preview. Log out of Plasma
and select Ferese in SDDM for real hardware use. `ferese-session` can also run
from a spare logged-in TTY. It must run as your normal user, never as root.
Foot and `dbus-run-session` must be installed.

Session logs are written under `${XDG_STATE_HOME:-~/.local/state}/ferese/`.
The launcher sets Wayland desktop variables, starts D-Bus if none is available,
and grants shell capabilities only to the compositor-launched shell process.
It does not enable public screencopy, privileged input methods, or shortcut
inhibition by default.

## Logout and recovery

Save your work, then press **Super+Shift+E** or run `feresectl exit` to log out.
Ferese stops its event loop and terminates its shell child during shutdown.
This is immediate logout, without a save-confirmation dialog.

If the compositor becomes unresponsive, switch to another TTY using
Ctrl+Alt+F1–F12 and terminate that specific Ferese process. Select Plasma on the
next login. Do not restart SDDM while a desktop contains unsaved work.

For a binary rollback, inspect `/usr/local/lib/ferese/previous` and select that
release with the `current` symlink while Ferese is stopped. Existing releases
are retained. To hide the login option, move
`/usr/share/wayland-sessions/ferese.desktop` out of that directory.

## First hardware session

Verify terminal/editor/browser, clipboard, fullscreen, fractional scaling,
monitor disconnect/reconnect, VT switching away/back, and logout. Suspend/resume
and session locking are not certified by the nested smoke test. Keep Plasma
available until real development sessions and these checks pass.
