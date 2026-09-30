# Screen sharing and recording

Share a monitor or window from a Wayland-compatible recording or conferencing app,
or save a video using the top bar. [Installation](installation.md) includes the
portal and PipeWire dependencies.

## Record from the top bar

1. Click the recording icon.
2. Select a monitor and click **Share**.
3. Click the stop square to finish.

Videos are saved as VP8 WebM files in your XDG Videos folder under `Ferese`.
A notification shows the saved path. Audio and region recording are not supported
by the built-in recorder; other recording apps can handle audio separately.
If recording fails, a notification reports the error and any unfinished
`.webm.part` file.

## Start sharing

Select a Wayland screen-sharing source in your application. In Ferese's picker,
choose a display or window and click **Share**. Nothing is selected initially.
Apps requesting multiple sources can select up to eight.

A shared display includes everything visible on it, including notifications.
A shared window includes only that app's content and subsurfaces, even when
covered or on another workspace. Separate popups and compositor decorations are
excluded. Resizing a shared window updates its video size; closing it ends sharing.

The picker follows your shell colors, font, corners, opacity, and blur.

## Stop sharing

Click **Stop sharing** in the sharing window, or close that window. The built-in
recorder uses the top-bar stop button instead. Sharing also ends when the app
closes or Ferese locks. Disconnecting a shared monitor or changing its capture
size ends its stream; start sharing again afterward.

## Remember permissions

When an app requests saved display permissions, the picker offers **Allow without
asking**, off by default. Approval lasts for the duration requested by the app.
The portal manages stored permissions and revocation.

A different app, changed display, or changed cursor option needs approval again.
Nested previews and displays without a reliable identity always ask. Window
permissions are not saved. Stopping a stream does not erase a saved permission.

## Requirements and limits

Install PipeWire, WirePlumber, `xdg-desktop-portal`, and `xdg-desktop-portal-gtk`.
The built-in recorder also needs GStreamer's PipeWire, base, and good plugins;
see the [package lists](installation.md#requirements).

The installer supplies Ferese's portal backend and selects it for screen sharing.
Log into a new Ferese session after installation.

Streams run at up to 30 fps and support hidden or embedded cursors. Capture copies
frames through CPU-accessible buffers, so large or multiple streams can use
substantial CPU and memory bandwidth.

## Troubleshooting

- Confirm PipeWire and WirePlumber are running.
- Confirm `XDG_CURRENT_DESKTOP` includes `Ferese`.
- Check that `FERESE_ENABLE_SCREENCOPY` was not set to `0` before session startup.
- Check for portal overrides in `~/.config/xdg-desktop-portal/`.

The installed preference is `/usr/share/xdg-desktop-portal/ferese-portals.conf`.
Its ScreenCast entry should select Ferese:

```ini
[preferred]
default=gtk;
org.freedesktop.impl.portal.ScreenCast=ferese;
```

`xdg-desktop-portal-ferese --sources` lists displays on the current
`WAYLAND_DISPLAY` without starting a stream. Nested previews do not replace the
host desktop's portal environment. See [Development](development.md#portal-checks)
for isolated sharing tests.
