# Ferese handbook

Ferese is a Linux Wayland desktop with scrolling columns, tree tiling, floating
windows, and a shared theme for its desktop tools.

## Get started

1. [Install Ferese](installation.md), or try a [nested preview](installation.md#preview-and-logs).
2. Open **Control Center → Settings** to choose a theme and configure your desktop.
3. Use the [configuration reference](configuration.md) for shortcuts, gestures, window rules, and displays.

## Default shortcuts

`Super` is usually the Windows key. All these shortcuts can be changed in Settings.

| Shortcut | Action |
| --- | --- |
| Super + Enter | Open a terminal |
| Super + H / J / K / L | Focus left / down / up / right |
| Super + F | Maximize the window |
| Super + Shift + F | Toggle fullscreen |
| Super + Tab | Open overview |
| Super + M | Switch scrolling / tree layout |
| Print Screen | Capture the active monitor and open Satty |
| Super + Shift + S | Select a screenshot area |
| Super + Shift + E | Ask to log out |

See [all shortcuts and mouse actions](shortcuts.md), including Super-drag to move
and Super-right-drag to resize floating windows.

Three-finger swipes up/down switch workspaces; left/right focus windows.
Change gestures in **Settings → Shortcuts**. The login shortcut guide can be
disabled there too.

## Desktop guides

- [Desktop widgets](desktop-widgets.md): clocks and sticky notes.
- [Notifications](notifications.md): history and Do Not Disturb.
- [Lock screen](locking.md): appearance, automatic locking, and display sleep.
- [Screen sharing and recording](screen-sharing.md): share an app or display, or save a video.
- [Desktop portals](portals.md): integration with applications.

Use Settings or edit `~/.config/ferese/config.kdl`. Changes reload automatically;
invalid edits keep the last working configuration.

## Troubleshooting

See [logs and recovery](installation.md#logout-and-recovery). `feresectl exit`
ends the session immediately; `feresectl request-logout` asks for confirmation.

Report problems in the [issue tracker](https://github.com/ferese-wm/ferese/issues).
Include reproduction steps, whether Ferese ran as a nested preview or a direct
session, and relevant logs. Contributors can start with [Development](development.md).
