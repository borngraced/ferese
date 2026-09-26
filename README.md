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
- Configurable animations
- Built-in bar and control center
- Native Settings app
- Desktop widgets
- Area screenshots saved to disk and copied to the clipboard
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
- Screenshots: `grim`, `slurp`, and `wl-clipboard`.

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
| Super + Shift + E | Log out immediately—save your work first |

In an installed session, screenshots are saved to `Pictures/Screenshots` and
copied to the clipboard. Press Escape to cancel the selection.
When focus follows the mouse is enabled, hovering changes keyboard focus without
raising the window; clicking raises it within its layer.

## Make it yours

Open **Control Center → Settings**, or run `ferese-settings`.
Advanced options live in `~/.config/ferese/config.toml` (or under
`$XDG_CONFIG_HOME`). Changes reload automatically. The complete
[configuration reference](docs/configuration.md) covers every supported option;
the [example config](packaging/config.toml) is a starting point.

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
