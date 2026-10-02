<p align="center">
  <img src="docs/images/ferese-lockup.svg" width="300" alt="Ferese">
</p>

<p align="center"><strong>A configurable Wayland desktop</strong></p>

<p align="center">
  <a href="docs/installation.md">Install</a> ·
  <a href="docs/configuration.md">Configure</a> ·
  <a href="docs/screenshots.md">Screenshots</a> ·
  <a href="docs/README.md">Documentation</a>
</p>

Ferese combines a window manager and desktop shell with one configuration.

![Ferese Blue desktop with Settings and Control Center](docs/images/screenshots/ferese-main-hero.png)

[View more screenshots](docs/screenshots.md)

## Features

- Scrolling columns with window grouping, tree tiling and floating windows that remember their size and placement
- Workspace overview and recent-window switching
- Remappable keyboard shortcuts and touchpad gestures
- Multiple monitors with fractional scaling and automatic refresh-rate switching on low battery
- Themes, wallpapers and blur that update live, with Light/Dark/Auto modes and custom KDL themes
- Squircle window corners with matching borders and shadows
- Desktop widgets, including a clock and sticky notes
- Screenshots with annotation, screen sharing and built-in recording
- A built-in lock screen and authentication dialogs
- Settings and Control Center with Wi-Fi, Bluetooth and audio controls

## Install

Start with the [dependencies](docs/installation.md#requirements), then clone Ferese and
build it as your normal user:

```sh
git clone https://github.com/ferese-wm/ferese.git
cd ferese
./scripts/install.sh
```

Once it’s installed, log out and choose **Ferese** at your login screen, or run
`ferese-session` from a TTY or a greeter that accepts a command. The [installation
guide](docs/installation.md) also walks you through previews, updates and recovery.

## Configure

Configure Ferese through **Control Center → Settings** or `~/.config/ferese/config.kdl`.
Changes apply live. Invalid edits leave the last working configuration in place.

The [configuration reference](docs/configuration.md) and [example
config](packaging/config.kdl) cover everything from appearance and window layouts to
displays and startup apps.

## Default shortcuts

Use `Super`, usually the Windows key, to move around your desktop, and open **Settings →
Shortcuts** whenever you want to change a shortcut or gesture.

| Control | Action |
| --- | --- |
| Super + Enter | Open a terminal |
| Display Fn key (`XF86Display`) | Choose the display mode |
| Super + H / J / K / L | Focus left / down / up / right |
| Super + Backspace | Return to the last focused window |
| Super + Escape | Toggle between the last two visited workspaces on this monitor |
| Alt + Tab / Alt + Shift + Tab | Preview recent windows forward / backward; release Alt to select, Escape to cancel |
| Super + F | Maximize the window |
| Super + Shift + F | Toggle fullscreen |
| Super + R | Cycle column width |
| Super + Tab | Open overview |
| Super + M | Switch scrolling / tree layout |
| Print Screen | Capture the active monitor and open Satty |
| Super + Shift + S | Select a screenshot area |
| Super + Shift + E | Ask to log out |
| Super + left-button drag | Move a floating window |
| Super + right-button drag | Resize a floating window |
| Three-finger swipe up / down | Next / previous workspace |
| Three-finger swipe left / right | Focus the window to the right / left |

See [all shortcuts and mouse actions](docs/shortcuts.md) for window and workspace controls.

## Documentation

The [documentation index](docs/README.md) lists the desktop guides.
[Development instructions](docs/development.md) cover building, testing and contributing.

Ferese is written in Rust using Smithay. Development happens [on
GitHub](https://github.com/ferese-wm/ferese). If something goes wrong, [open an
issue](https://github.com/ferese-wm/ferese/issues) with the steps to reproduce it and
any relevant logs.
