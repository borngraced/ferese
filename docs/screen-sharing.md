# Screen sharing and recording

Ferese includes a ScreenCast portal backend, `xdg-desktop-portal-ferese`.
Applications use the standard desktop portal to request a display, then receive
video through PipeWire. A recorder or conferencing application handles encoding,
saving files, or sending video; those tasks do not run inside the compositor.

## Record from the top bar

1. Click the screen-recording icon in the top bar to open the display picker.
2. Choose a monitor in the native picker and click **Share**.
3. The icon changes to a stop square and shows elapsed time. Click it to stop.

The recorder finishes the video before sending a notification with its saved
path. Videos are saved as VP8 WebM files under
`Videos/Ferese` (using your configured XDG Videos folder). Audio is off.
The icon uses the same theme accent, hover effect, and corner radius as the
other bar controls.

Encoding runs in a separate native `ferese-record` process using GStreamer.
The shell remains responsive while a file is recorded or finalized. If saving
fails, an error appears in the notification center; an unfinished `.webm.part` file
may remain at the path shown in the error.

## Start sharing

Use a Wayland screen-sharing or recording source in your application. Ferese
opens a display picker. Select a display and press **Share**. Nothing is selected
by default. Applications that request multiple displays can share up to four.
The picker uses your Ferese theme colors, font, and shell corner radius. Its
background follows `theme.material.style`: opaque in solid mode, or translucent
using `theme.surface.popover.opacity`. Text and icons remain opaque.

Other applications keep a **screen sharing** window with a **Stop sharing** button.
Ferese’s built-in recorder uses the top-bar stop control instead, leaving your
windows accessible after display selection. The command-line recorder keeps the
sharing window when it is not launched by the shell. Press **Stop sharing** or
close the sharing window to revoke an ordinary application’s session. Closing the requesting application,
locking Ferese, disconnecting the selected display, or changing its capture size
also ends sharing. A new request needs fresh consent; selections are not saved.
Everything visible on a shared display, including notifications, is included.

## Installation

The [standard installer](installation.md) builds and installs the backend, its
D-Bus activation entry, and a Ferese-specific portal preference file. Install
`pipewire`, a PipeWire session manager such as `wireplumber`,
`xdg-desktop-portal`, and `xdg-desktop-portal-gtk` using your distribution's package
manager. The GTK backend supplies other portal interfaces such as file selection.
The built-in recorder also needs GStreamer’s PipeWire, base, and good plugins
(`pipewiresrc`, `videoconvert`, `vp8enc`, and `webmmux`). The installation guide
includes these packages.

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

Window-only sharing, region selection, and audio capture in Ferese’s recorder
are not implemented yet. Other recording applications can handle audio separately.
Video currently uses CPU copies through the compositor's shared-memory screencopy path, not DMA-BUF zero-copy. High-resolution
or multiple-display recording can therefore use significant CPU and memory
bandwidth. A paused stream checks capture availability once per second so locking
also ends sessions without an active consumer.

## Development checks

Build the backend after installing the native dependencies in the
[installation guide](installation.md#requirements):

```sh
cargo build --release --locked -p xdg-desktop-portal-ferese
cargo test --locked -p xdg-desktop-portal-ferese
# Exercise real video encoding and playback (requires the plugins above):
cargo test --locked -p xdg-desktop-portal-ferese --bin ferese-record encoder_finishes_a_real_webm -- --ignored
```

The opt-in integration test opens a temporary nested compositor and uses a
private D-Bus session. It requires built Ferese and locker binaries, Python
GI/GStreamer with `pipewiresrc`, `dbus-daemon`, `xdg-desktop-portal`, and `bwrap`.
It checks capture delivery and timestamps, cancellation, frontend loss, and lock
revocation:

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
