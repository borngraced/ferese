# The Ferese handbook

Ferese is a complete Wayland desktop built around personalization. Its custom shell, native Settings, live themes, and flexible layouts work together as one cohesive experience.

This handbook covers installing Ferese, finding your way around, and making the desktop your own.

[Why Ferese](why-ferese.md) explains why the desktop is designed as one integrated system rather than assembled from separately configured tools.

## Start with your first session

1. Read [Installation](installation.md) for dependencies, building, login-manager or TTY startup, and recovery.
2. Open **Control Center → Settings** to choose a theme and configure your desktop.
3. Explore the [configuration reference](configuration.md) for layouts, shortcuts, window rules, and displays.

Want to try it before switching desktops? The [preview instructions](installation.md#preview-and-logs) cover running Ferese inside an existing Wayland session.

## Find your way around

Ferese supports scrolling columns, tree tiling, floating windows, and a workspace overview. These are the built-in keyboard shortcuts; all can be changed in Settings.

| Shortcut | Action |
| --- | --- |
| Super + Enter | Open a terminal |
| Super + H / J / K / L | Focus a window |
| Super + R | Cycle column width |
| Super + Tab | Open workspace and window overview |
| Super + M | Switch scrolling / tree layout |
| Super + Shift + S | Select a screenshot area |

Three-finger swipes move between workspaces and windows. Open **Control Center → Settings → Shortcuts** to change the defaults or assign gestures to other actions.

## Make it feel like yours

Use **Settings** for everyday customization. Choose an appearance preset, change the wallpaper, adjust shell and window corners, or add a clock and sticky notes through [Desktop widgets](desktop-widgets.md).

[Native locker](locking.md) covers lock-screen appearance, authentication, and idle locking.

[Notifications](notifications.md) explains popup cards, history, Do Not Disturb, and app actions.

If you prefer a text editor, your configuration lives at:

```text
~/.config/ferese/config.kdl
```

Ferese reloads changes as you make them. An invalid edit keeps the last working configuration. Settings also preserves a backup of your previous save. See [Saving and validation](configuration.md#saving-and-validation).

## Keep a recovery path

Keep another desktop session available while testing hardware behavior. Save your work before logging out: `feresectl exit` and **Super + Shift + E** end the current session immediately.

The [recovery guide](installation.md#logout-and-recovery) explains logs, installed releases, and rollback.

## Work on Ferese

Ferese is built in Rust on Smithay. Use release builds when evaluating motion and startup performance. From the repository checkout:

```sh
cargo test --workspace --locked
cargo fmt --all --check
```

The [source repository](https://github.com/ferese-wm/ferese) is the place to explore implementation details and report issues. When reporting a problem, include how to reproduce it, whether you used the nested or DRM backend, and relevant logs.
