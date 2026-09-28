# Screen sharing and recording

Ferese includes a ScreenCast portal backend, `xdg-desktop-portal-ferese`.
Applications use the standard desktop portal to request a display, then receive
video through PipeWire. A recorder or conferencing application handles encoding,
saving files, or sending video; those tasks do not run inside the compositor.

There is no built-in Record button or video-file recorder in the shell yet.
Start recording from an application that supports the Wayland ScreenCast portal.

## Start sharing

Use a Wayland screen-sharing or recording source in your application. Ferese
opens a display picker. Select a display and press **Share**. Nothing is selected
by default. Applications that request multiple displays can share up to four.
The picker uses your Ferese theme colors, font, and shell corner radius.

A **screen sharing** window remains visible while sharing. Press **Stop sharing**
or close that window to revoke the session. Closing the requesting application,
locking Ferese, disconnecting the selected display, or changing its capture size
also ends sharing. A new request needs fresh consent; selections are not saved.
Everything visible on a shared display, including notifications, is included.

## Installation

The [standard installer](installation.md) builds and installs the backend, its
D-Bus activation entry, and a Ferese-specific portal preference file. Install
`pipewire`, a PipeWire session manager such as `wireplumber`,
`xdg-desktop-portal`, and `xdg-desktop-portal-gtk` using your distribution's package
manager. The GTK backend supplies other portal interfaces such as file selection.

Log into a new Ferese session after installation. The session launcher imports
its Wayland display into the D-Bus activation environment. Nested previews leave
the host activation environment alone.

Existing custom window rules can float the picker and sharing indicator:

```kdl
window-rule app-id="dev.ferese.ScreenShare" floating=#true
```

The packaged example configuration already includes this rule. Existing user
configuration is preserved during installation.

## Current capabilities

- Monitor capture, one PipeWire stream per selected display, at up to 30 fps.
- Hidden or embedded pointer, as requested by the application.
- Bounded frame storage and reusable shared-memory buffers.
- Cancellation during selection, an explicit stop control, and process cleanup.
- ScreenCast backend version 3, without persistent or automatic consent.

Window-only sharing, region selection, audio capture, and a built-in recorder
control in the shell are not implemented yet. Audio can be handled separately by
the recording application. Video currently uses CPU copies through the
compositor's shared-memory screencopy path, not DMA-BUF zero-copy. High-resolution
or multiple-display recording can therefore use significant CPU and memory
bandwidth. A paused stream checks capture availability once per second so locking
also ends sessions without an active consumer.

## Development checks

Build the backend after installing the native dependencies in the
[installation guide](installation.md#requirements):

```sh
cargo build --release --locked -p xdg-desktop-portal-ferese
cargo test --locked -p xdg-desktop-portal-ferese
```

The opt-in integration test opens a temporary nested compositor and uses a
private D-Bus session. It requires built Ferese and locker binaries, Python
GI/GStreamer with `pipewiresrc`, `dbus-daemon`, `xdg-desktop-portal`, and `bwrap`.
It checks capture delivery, cancellation, frontend loss, and lock revocation:

```sh
FERESE_TEST_PORTAL=1 \
FERESE_TEST_PORTAL_BINARY=target/release/xdg-desktop-portal-ferese \
python3 scripts/tests/test_portal_isolated.py
```

Add `FERESE_TEST_PORTAL_CONSENT=1` to exercise the native picker and restricted
PipeWire connection. Select the temporary display and click **Share** when
prompted. Tests capture only the nested preview and leave host PAM policies and
D-Bus activation unchanged.

## Troubleshooting

Confirm that `XDG_CURRENT_DESKTOP` contains `Ferese`, PipeWire is running, and
`FERESE_ENABLE_SCREENCOPY` was not set to `0` before launching the session.

The installed preference is
`/usr/share/xdg-desktop-portal/ferese-portals.conf`. A user override under
`~/.config/xdg-desktop-portal/` can take precedence. Its ScreenCast preference
should name `ferese`:

```ini
[preferred]
default=gtk;
org.freedesktop.impl.portal.ScreenCast=ferese;
```

If capture dimensions change, stop and start sharing again. Ferese ends the old
session instead of feeding incorrectly sized frames to the application.

For development, `xdg-desktop-portal-ferese --sources` lists displays available
on the current `WAYLAND_DISPLAY`. This command does not start a stream.
