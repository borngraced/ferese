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
