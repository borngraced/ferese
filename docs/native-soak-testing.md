# Native-client soak testing

The M6 soak runner repeatedly creates and destroys native Wayland clients while
exercising focus, resize, column width, fullscreen, floating, and workspace
paths. Every tenth iteration also terminates a client abruptly and opens three
isolated connections that send malformed Wayland messages. The runner requires
Ferese to disconnect each malformed client and then checks that the compositor
and authenticated IPC socket remain responsive. It also records compositor RSS
growth.

Build Ferese and `feresectl`, start the compositor, and run:

```bash
FERESE_TRACE_PERFORMANCE=1 cargo run -p ferese -- foot
```

From a terminal connected to Ferese, or from the host with the Ferese socket
selected, run:

```bash
WAYLAND_DISPLAY=wayland-1 \
FERESE_PID="$(pgrep -n -x ferese)" \
./scripts/soak-native.sh
```

The default duration is 24 hours. A shorter validation run is:

```bash
WAYLAND_DISPLAY=wayland-1 \
FERESE_PID="$(pgrep -n -x ferese)" \
FERESE_SOAK_SECONDS=300 \
./scripts/soak-native.sh
```

The malformed-client check can also be run independently against a disposable
Ferese session:

```bash
WAYLAND_DISPLAY=wayland-1 cargo run -p ferese-malformed-client
target/debug/feresectl get-outputs
```

Do not point the malformed-client probe at the host compositor socket.

Environment controls:

- `FERESE_SOAK_SECONDS`: run duration, default `86400`;
- `FERESE_SOAK_DELAY`: delay between operations, default `0.15` seconds;
- `FERESE_SOAK_CLIENT`: native client executable, default `foot`;
- `FERESECTL`: path to `feresectl`, default `target/debug/feresectl`;
- `FERESE_MALFORMED_CLIENT`: path to the malformed protocol probe, default
  `target/debug/ferese-malformed-client`;
- `FERESE_PID`: compositor PID; automatic lookup is only a fallback;
- `FERESE_SOAK_LOG`: result log, default `/tmp/ferese-soak-<pid>.log`.
- `FERESE_SOAK_MAX_RSS_GROWTH_KIB`: maximum RSS growth, default `131072`
  (128 MiB);
- `FERESE_SOAK_MAX_FD_GROWTH`: maximum open-file-descriptor growth, default
  `32`;
- `FERESE_SOAK_MAX_THREAD_GROWTH`: maximum thread growth, default `8`.

Run the same workload once with the nested backend and once from a direct DRM
session. Samples are taken after spawned clients are reaped. A passing run
requires the compositor to remain alive, IPC queries to succeed, and RSS, file
descriptor, and thread growth to stay within the declared budgets. Tighten the
defaults for release hardware when its normal cache behavior is known. Preserve
the soak log and the `ferese::render` summaries with the release artifacts.
