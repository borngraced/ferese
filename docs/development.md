# Development

Install the [build dependencies](installation.md#requirements), then run:

```sh
cargo build --release --locked --workspace
cargo test --workspace --locked
cargo fmt --all --check
```

Use release builds for performance and animation checks. Preview changes in a
[nested session](installation.md#preview-and-logs) before testing hardware behavior.
Keep another desktop available for direct-session recovery.

## Test guides

- [Nested resize checks](nested-resize-testing.md): wallpaper and bar responsiveness.
- [Client soak testing](native-soak-testing.md): repeated client lifecycle and resource growth.
- [Performance measurements](performance-baseline.md): sampling and historical results.

## Portal checks

Run backend unit tests and compare D-Bus contracts against installed portal XML:

```sh
cargo test --locked -p xdg-desktop-portal-ferese
python3 scripts/tests/test_portal_contracts.py
```

The contract test uses a private bus. It checks signatures, appearance changes,
invalid-config retention, and caller authorization. It does not prove complete
compatibility with every app or open consent dialogs.

With release binaries built, run isolated integration checks:

```sh
FERESE_TEST_SHORTCUTS=1 python3 scripts/tests/test_shortcuts_isolated.py
FERESE_TEST_WINDOW_CAPTURE=1 python3 scripts/tests/test_window_capture_isolated.py
FERESE_TEST_INHIBIT=1 python3 scripts/tests/test_inhibit_isolated.py
FERESE_TEST_RESTORE=1 python3 scripts/tests/test_restore_isolated.py
FERESE_TEST_WINDOW_STREAM=1 python3 scripts/tests/test_window_stream_isolated.py
```

These cover shortcut cleanup, window capture, inhibitors, restored permissions,
and live window streams. Check each script's dependencies before running it.
They use disposable nested sessions or private buses.

For monitor streaming and lock revocation, install Python GI/GStreamer with
`pipewiresrc`, `dbus-daemon`, `xdg-desktop-portal`, and `bwrap`, then run:

```sh
FERESE_TEST_PORTAL=1 \
FERESE_TEST_PORTAL_BINARY=target/release/xdg-desktop-portal-ferese \
python3 scripts/tests/test_portal_isolated.py
```

Add `FERESE_TEST_PORTAL_CONSENT=1` to exercise the picker. Choose the temporary
display and click **Share**. Tests must target the nested preview, not the host desktop.

Check real encoding and the private InputCapture transport:

```sh
cargo test --locked -p xdg-desktop-portal-ferese --bin ferese-record encoder_finishes_a_real_webm -- --ignored
cargo test --locked -p ferese input_capture::tests
cargo test --locked -p xdg-desktop-portal-ferese eis::tests -- --ignored
```

Encoding needs the recorder's GStreamer plugins. The transport test needs libei
and uses synthetic input over a private socket pair. Hardware pointer barriers,
multiple monitors, display sleep, and live authentication still need session tests.

## Report a bug

Include the commit or installed release, reproduction steps, backend (nested or
direct), monitor scales/transforms, and relevant logs. Avoid publishing passwords,
access tokens, or private window content.

## Resume recovery

The direct backend retains known GPU identities while their DRM resources are
unavailable. Hotplug, configuration and lid changes reconcile through one path;
inactive sessions defer that work. Activation reads the lid asynchronously, then
re-enumerates devices and connectors before allowing presentation. A logind
system-wake signal follows the same path, including when seat ownership did not
change. Newer libinput observations supersede in-flight lid reads. A failed read
preserves the last observation; a three-second deadline prevents a stalled bus
from blocking recovery indefinitely.

Device failures retire their output globals and retry with exponential backoff
capped at 32 seconds. Working devices remain usable. Known hardware can return
under a different `cardN` path. This does not expand support to previously
unmanaged secondary GPUs. Connected-output reports mark a mode active only when
output creation succeeded. Lock ownership is retained throughout recovery.

Run the ordering and private D-Bus checks with:

```sh
cargo test --locked -p ferese backends::direct::topology
cargo test --locked -p ferese backends::direct::lid -- --include-ignored
```

The D-Bus test requires `dbus-daemon` and permission to create private sockets.
Hardware validation still needs a direct session: change monitors while on
another VT, suspend/resume with a dock disconnected, change the lid while
suspended, and unplug/reconnect the managed GPU where supported. Verify current
output geometry, idle notifications, recovery after failures and continued lock
protection. Nested sessions do not validate DRM reacquisition.
