# Desktop portals

Applications call the standard `org.freedesktop.portal.Desktop` frontend. Ferese's
native backend implements desktop integration; GTK remains the fallback for
standard file, print and application dialogs. Install both backends.

## Native services

- **Settings:** Ferese's color scheme, accent color, contrast preference and
  reduced-motion setting. Changes are signalled after valid KDL appearance
  changes; malformed saves leave the last valid preferences intact.
- **Screenshot:** whole-screen and selected-area PNG captures, plus a screen
  color picker. Every capture asks for consent. Interactive requests offer area
  selection. Selection uses `slurp`; captures use Ferese's bounded screenshot IPC.
  The helper must disappear from the compositor before capture starts.
- **Wallpaper:** a preview and confirmation, then an atomic configuration update.
  Desktop and lock-screen wallpaper can be set independently, or together.
  Ferese copies accepted local images into its own private data directory so
  temporary portal files can disappear safely. Image bytes and decoded dimensions
  are bounded. `theme.background.lock-path` overrides the lock-screen image;
  without it the lock screen follows `theme.background.path`.
- **ScreenCast:** monitor sharing with a native picker and PipeWire streams.
  See [screen sharing](screen-sharing.md) for lifetime and recording details.

Native request dialogs use Ferese's theme and are floating windows. Screenshot,
PickColor and Wallpaper consent dialogs use Wayland parent identifiers to establish
a transient relationship with the requesting window.
Cancelling a request or losing the portal frontend terminates its pending helper.

The packaged `ferese-portals.conf` selects these interfaces explicitly. Existing
user overrides in `~/.config/xdg-desktop-portal/` may take precedence. Install the
new compositor, backend and preference files together; screenshot synchronization
requires the updated compositor IPC.

## GNOME compatibility audit

The audit compares the [GNOME backend's registered interfaces](https://github.com/GNOME/xdg-desktop-portal-gnome/blob/main/data/meson.build)
with the [standard backend D-Bus contracts](https://flatpak.github.io/xdg-desktop-portal/docs/impl-dbus-interfaces.html).
Matching D-Bus signatures does not establish complete feature parity.

| Interface | Ferese integration |
| --- | --- |
| Settings | Native, including appearance change signals |
| Screenshot | Native screen/area capture and PickColor; true window capture remains missing |
| Wallpaper | Native desktop, lock-screen and combined targets |
| ScreenCast | Native monitors; window sources, persistence and richer metadata remain missing |
| Access, Account, AppChooser, DynamicLauncher, FileChooser, Notification, Print | Delegated to GTK |
| Background | Missing native application state and background consent |
| Usb | Missing native device consent |
| GlobalShortcuts | Missing session-scoped compositor registration and activation signals |
| RemoteDesktop | Missing authorized input injection and EIS transport |
| Clipboard | Missing remote-session clipboard transfer |
| InputCapture | Missing zones, pointer barriers and EIS transport |
| Lockdown | GTK availability depends on the installed build; no native policy provider |
| Inhibit | GTK's GNOME-session/ScreenSaver integration does not provide complete Ferese session inhibition |

The frontend supplies additional APIs such as OpenURI, network monitoring,
document export and permission storage. Adding a similarly named backend service
is neither required nor sufficient for those APIs.

Do not advertise window capture or input services until the compositor provides
the corresponding operation. Screen crops include occluding windows and cannot
serve as isolated window captures. GNOME's remote-input features require native
compositor/session work, rather than substituting successful empty replies.

## Validation

Build the backend and run unit tests, then check its contracts on a private bus:

```sh
cargo build --locked -p xdg-desktop-portal-ferese
cargo test --locked -p xdg-desktop-portal-ferese
python3 scripts/tests/test_portal_contracts.py
```

The contract test compares introspection against the installed official backend
XML, verifies appearance changes and invalid-save retention, and checks that
sensitive methods reject callers outside the portal frontend. It does not open
consent dialogs or capture the host desktop. Interactive consent, fractional-scale
capture, polkit and hotplug still require a live session test.
