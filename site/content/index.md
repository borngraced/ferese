# Ferese handbook

Ferese is a window manager and desktop shell built together and configured in one place,
for people who want their desktop to be their own. This handbook helps you get settled,
shape the desktop around the way you work and understand what you can change.

[Browse screenshots](screenshots.md) to see the themes, layouts and desktop controls in
use

## Get started

1. [Install Ferese](installation.md), or try a [nested preview](installation.md#preview-and-logs).
2. Choose Light, Dark, or Auto in **Control Center**. Open **Settings** for theme files and presets.
3. Use the [configuration reference](configuration.md) for shortcuts, gestures, window rules, and displays.

## Default shortcuts

Use `Super`, usually the Windows key, for the shortcuts below, and make them your own in
**Settings → Shortcuts**.

| Control | Action |
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
| Super + left-button drag | Move a floating window |
| Super + right-button drag | Resize a floating window |
| Three-finger swipe up / down | Next / previous workspace |
| Three-finger swipe left / right | Focus the window to the right / left |
| Login shortcut guide | Appears at login; disable in Settings → Shortcuts |

The [shortcuts and mouse actions guide](shortcuts.md) covers the rest, including how to
group windows, resize them and move between workspaces.

## Desktop guides

- [Lock screen](locking.md): appearance, automatic locking, and display sleep.
- [Screen sharing and recording](screen-sharing.md): share an app or display, or save a video.
- [Desktop portals](portals.md): integration with applications.

Personalize the desktop in Settings or edit `~/.config/ferese/config.kdl` directly, with
changes applying live and the last working configuration staying in place if an edit is
invalid.

## Troubleshooting

If you need to end a session, `feresectl request-logout` asks for confirmation and
`feresectl exit` exits immediately, so save your work first. The [logs and recovery
guide](installation.md#logout-and-recovery) helps when something stops responding.

You can [report a problem](https://github.com/ferese-wm/ferese/issues) with the steps to
reproduce it, relevant logs and whether you were using a nested preview or a direct
session, or start with [Development](development.md) if you’d like to contribute.
