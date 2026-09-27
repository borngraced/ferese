<p align="center">
  <img src="packaging/icons/ferese.svg" width="88" height="88" alt="Ferese logo">
</p>

<h1 align="center">Ferese</h1>

<p align="center">Just another Wayland window manager with sensible defaults.</p>

<p align="center">
  <a href="docs/installation.md">Install</a> ·
  <a href="docs/configuration.md">Configure</a> ·
  <a href="#try-it">Try it</a> ·
  <a href="docs/desktop-widgets.md">Widgets</a>
</p>

Ferese is built in Rust on Smithay, with its own menu bar, control center
and native Settings app.

## Features

- Scrolling columns, tree tiling and floating windows
- Workspace overview
- Configurable three-finger workspace and window navigation
- Configurable animations
- Built-in bar and control center
- Native Settings app
- Desktop widgets
- Area and full-screen screenshots with Satty annotation
- Themes, blur and gradient borders
- Multi-monitor support and fractional scaling
- Live configuration reload
- Background services and session locking

## Try it

Preview Ferese inside your current Wayland desktop:

```sh
cargo build --release --locked -p ferese -p ferese-shell -p ferese-settings -p feresectl
target/release/ferese --backend nested --grant-effects --grant-shell-control -- \
  target/release/ferese-shell
```

Requirements (Fedora package names):

- Rust 1.93+ and Cargo.
- Build tools: `gcc`, `gcc-c++`, `cmake`, `make`, `pkgconf-pkg-config`.
- Wayland and input: `wayland-devel`, `libxkbcommon-devel`, `libinput-devel`.
- Graphics: `libdrm-devel`, `mesa-libgbm-devel`, `mesa-libEGL-devel`.
- Session and fonts: `systemd-devel`, `libseat-devel`, `fontconfig-devel`.
- Installation: `desktop-file-utils`, `dbus`.
- Default terminal: `foot` (or configure another terminal).
- Optional: `zenity` for wallpaper browsing, `swayidle` for idle actions,
  and `swaylock` or `swaylock-effects` for locking.
- Screenshots: `grim`, `slurp`, `satty`, and `wl-clipboard`.

For a login-screen session alongside your existing desktop:

```sh
./scripts/install.sh
```

The installer preserves your configuration and keeps previous releases for
rollback. Log out and select **Ferese** at the login screen. An upgrade does
not replace the running compositor: log out and back in to activate it.
See [installation and recovery](docs/installation.md) for prerequisites,
session logs and rollback.

## Get around

Built-in defaults (your configuration can override them):

The bar shows workspaces 1–9 in a compact numbered strip, including empty ones.
Click a number to switch using the same behavior as Super+1–9. A filled button marks this monitor's
active workspace; an outlined button marks one active on another monitor.
Three-finger swipes up/down navigate workspaces on this monitor; left/right
navigate windows. Assign swipes to any keyboard-binding action (including
overview, move, or a launcher command) in **Settings → Shortcuts**.

| Shortcut | Action |
| --- | --- |
| Super + Enter | Open a terminal |
| Super + H / J / K / L | Focus a window |
| Super + R | Cycle column width |
| Super + C | Center the focused column |
| Super + F | Maximize, keeping the bar and decorations |
| Super + Shift + F | True fullscreen |
| Super + Tab | Open workspace and window overview |
| Super + M | Switch scrolling / tree layout |
| Super + Shift + S | Select a screenshot area |
| Print Screen / Fn + PrtSc | Capture the whole screen |
| Super + Shift + E | Log out immediately—save your work first |

Both screenshot shortcuts open Satty for annotation. Press Enter in Satty to
save to `Pictures/Screenshots` and copy the edited image to the clipboard;
Escape discards it. Escape also cancels an area selection.
Run `ferese-screenshot --full` to capture the whole screen from a terminal.
When focus follows the mouse is enabled, hovering changes keyboard focus without
raising the window; clicking raises it within its layer.

## Make it yours

Open **Control Center → Settings**, or run `ferese-settings`.
Advanced options live in `~/.config/ferese/config.kdl` (or under
`$XDG_CONFIG_HOME`). Changes reload automatically. The complete
[configuration reference](docs/configuration.md) covers every supported option;
the [example config](packaging/config.kdl) is a starting point. KDL uses compact nested sections
and bindings such as `binding "Swipe3Up" "toggle-overview"`. See the
[configuration guide](docs/configuration.md#kdl-syntax).

## Development

```sh
cargo test --workspace --locked
cargo fmt --all --check
```

Use release builds when evaluating animation and startup performance.
`RUST_LOG=ferese=debug` enables detailed logs;
`FERESE_TRACE_PERFORMANCE=1` enables per-output performance summaries.
Test hardware behavior on your own setup, especially suspend/resume and locking.
Keep a working desktop session available while testing changes.
