<p align="center">
  <img src="packaging/icons/ferese.svg" width="88" height="88" alt="Ferese logo">
</p>

<h1 align="center">Ferese</h1>

<p align="center">An animation-first scrolling Wayland desktop.</p>

<p align="center">
  <a href="docs/installation.md">Install</a> ·
  <a href="docs/configuration.md">Configure</a> ·
  <a href="#try-it">Try it</a> ·
  <a href="#development">Develop</a>
</p>

Ferese combines a scrolling window manager with its own menu bar, control center
and native Settings app. Built in Rust on Smithay, it brings configurable motion
and a cohesive desktop interface to a keyboard-driven workflow.

## A desktop that moves with you

- **Scrolling columns:** a horizontal window strip with configurable widths and
  focus behavior. Tree tiling is available per workspace.
- **Coordinated motion:** animated focus, resize, workspace transitions and
  overview, with adjustable speed and reduced-motion support.
- **An integrated shell:** workspace selector, status popovers, notification
  controls and a compact control center.
- **Native Settings:** appearance, wallpaper, input, motion and more, with
  automatic saving, validation and undo.
- **Your own look:** solid or translucent shell surfaces, backdrop blur,
  rounded corners, soft shadows and optional gradient focus rings.
- **Multiple displays:** per-output workspaces, fractional scaling, monitor
  hotplug and laptop-lid workspace migration.
- **Live configuration:** update TOML without restarting the desktop.
  Invalid changes keep the last working configuration.
- **Session locking:** `ext-session-lock-v1` with swaylock or swaylock-effects
  through `ferese-lock`.

## Try it

Preview Ferese inside your current Wayland desktop:

```sh
cargo build --release --locked -p ferese -p ferese-shell -p ferese-settings -p feresectl
target/release/ferese --backend nested --grant-effects --grant-shell-control -- \
  target/release/ferese-shell
```

Use a recent Rust toolchain (1.93 or newer for the shell), Smithay's native
build dependencies, and `foot` for the default terminal. Settings wallpaper
browsing uses `zenity`; entering an image path works without it.

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

These are built-in defaults; your configuration can override them.

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
| Super + Shift + E | Log out immediately—save your work first |

## Make it yours

Open **Control Center → Settings**, or run `ferese-settings`.
Advanced options live in `~/.config/ferese/config.toml` (or under
`$XDG_CONFIG_HOME`). Start with the [example config](packaging/config.toml).

```toml
[scrolling]
default_column_width = 0.5
focus_strategy = "paged"

[[window_rules]]
app_id = "dev.ferese.Settings"
floating = true
```

Saving the file reloads it automatically. Run `feresectl reload-config`
for an explicit reload with validation feedback. The
[configuration reference](docs/configuration.md) covers themes, window rules,
bindings, output profiles, session services and locking.

## Development

```sh
cargo test --workspace --locked
cargo fmt --all --check
```

Use release builds when evaluating animation and startup performance.
`RUST_LOG=ferese=debug` enables detailed logs;
`FERESE_TRACE_PERFORMANCE=1` enables per-output performance summaries.
Development probes under `tools/` are local-only and are not part of the
published Cargo workspace.

- [Architecture and window-manager specification](docs/ferese-window-manager-spec.md)
- [Shell design](docs/ferese-shell-design.md)
- [Theme and UI specification](docs/ferese-theme-ui-spec.md)
- [Animation validation](docs/animation-validation.md)
- [Nested resize testing](docs/nested-resize-testing.md)
- [Native-client soak testing](docs/native-soak-testing.md)

Test hardware behavior on your own setup, especially suspend/resume and locking.
Keep a working desktop session available while testing changes.
