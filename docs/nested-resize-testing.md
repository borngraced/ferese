# Nested resize checks

Build and launch a preview with protocol and render logging:

```sh
cargo build --release --locked -p ferese -p ferese-shell
WAYLAND_DEBUG=1 FERESE_TRACE_PERFORMANCE=1 \
  target/release/ferese --backend nested --grant-effects --grant-shell-control -- \
  target/release/ferese-shell 2>/tmp/ferese-resize.log
```

Use a wallpaper, drag each edge of the host window, and repeatedly maximize and
restore it. Check that the wallpaper and bar follow without long stalls, including
at fractional scaling.

## Compare renderers

Repeat with `ICED_BACKEND=wgpu` and `ICED_BACKEND=tiny-skia`. The shell enables
wgpu; software fallback can be slower when resampling wallpaper.

Trace the whole sequence: host resize, layer configure, client acknowledgement,
new buffer commit, then compositor rendering. A quick acknowledgement does not
mean the client has finished drawing. `FERESE_TRACE_PERFORMANCE` measures compositor
work, not shell rasterization.

## Historical check

A KDE Wayland check at 175% scale used debug binaries and protocol tracing.
Median configure acknowledgement was 1.35 ms with tiny-skia and 2.27 ms with
wgpu. Wallpaper buffer intervals were 5919 ms with tiny-skia (only two buffers)
and 9.85 ms median with wgpu. Capture durations differed, so these are diagnostic
observations rather than throughput benchmarks. GPU intervals also had stalls
up to about 200 ms.

Repeat on the current build. A nested result does not establish direct-session
resize or frame-pacing behavior.
