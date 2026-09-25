# Nested live-resize regression check

The shell enables libcosmic's `wgpu` feature. Without it, iced uses tiny-skia
to resample and rasterize the wallpaper on the CPU. In an unoptimized build,
this blocks the UI/render thread shared with the top bar for seconds. Prompt
layer configures, buffer stretching, and compositor vsync cannot fix that
client-side bottleneck. Software rendering remains available as a fallback.

## Reproduce and compare

```sh
cargo build --locked -p ferese -p ferese-shell
WAYLAND_DEBUG=1 FERESE_TRACE_PERFORMANCE=1 \
  target/debug/ferese --backend nested --grant-effects --grant-shell-control -- \
  target/debug/ferese-shell 2>/tmp/ferese-resize.log
```

Use a configured wallpaper. Drag the host window's edges in both directions,
then maximize/restore it repeatedly. Check that the wallpaper and bar follow
the window, including at fractional host scaling. Repeat with
`ICED_BACKEND=wgpu` to require GPU rendering (rather than silently accepting
fallback), and with `ICED_BACKEND=tiny-skia` to compare software rendering.
Use a release build if software fallback is needed for regular use.

Trace the chain, not just the compositor's frame rate:

1. Host `xdg_toplevel.configure` becomes Winit `Resized`.
2. Ferese updates the output, arranges layers, and sends
   `zwlr_layer_surface_v1.configure` for the new logical dimensions.
3. The shell acknowledges configure and eventually attaches/commits a new
   buffer. A fast acknowledgement does **not** imply a finished render.
4. Committed buffers change surface damage; Ferese renders the damaged output
   and submits through EGL to the host compositor.

`FERESE_TRACE_PERFORMANCE` measures Ferese's rendering/submission, not shell
rasterization. Wayland traces distinguish software SHM buffers from GPU
buffers (Mesa Vulkan on the tested system).

## Measured validation (KDE Wayland, 175% scale)

With temporary instrumentation, 24 host maximize/restore requests spaced
250 ms apart generated 26 Winit resize events including initialization. The
same debug binaries were tested with each renderer forced:

| Measurement | tiny-skia | wgpu |
| --- | ---: | ---: |
| Median configure acknowledgement | 1.35 ms | 2.27 ms |
| Median Ferese render + submit | 0.82 ms | 1.10 ms |
| Wallpaper buffer interval | 5919 ms (only two buffers) | 9.85 ms median |
| Bar buffers observed | 1 | 996 |

The CPU capture ran for 14 seconds and the GPU capture for 18 seconds;
counts are diagnostic, not equal-duration throughput benchmarks. GPU maximum
buffer intervals were about 200 ms for wallpaper and 158 ms for the bar, so
the median does not imply every frame met a refresh deadline. These figures
include protocol tracing overhead. The temporary resize/timing hooks were
removed after testing; no synthetic resizing runs in production.

The fix does not alter output coordinates, layer exclusion, damage tracking,
frame scheduling, or EGL swap settings. It also applies to the same shell
running under DRM, though this validation used the nested backend.
