# Ferese libcosmic shell spike

This disposable M8 probe validates libcosmic's Wayland lifecycle before Ferese
uses it for production shell components. It creates one top layer-shell surface
on the active output and reports:

- output discovery and updates;
- the initial configure and later resizes;
- keyboard focus changes; and
- compositor close or application shutdown.

Run it inside Ferese with:

```sh
cargo run -p ferese -- --backend nested -- cargo run -p ferese-shell-spike
```

Click the surface to exercise on-demand keyboard focus. Press Escape or click
`Exit spike` to exercise clean shutdown.

For an unattended shutdown check, set an exit delay in milliseconds:

```sh
FERESE_SHELL_SPIKE_EXIT_AFTER_MS=1000 \
  FERESE_SHELL_SPIKE_EXCLUSIVE_FOCUS=1 \
  cargo run -p ferese -- --backend nested --grant-effects -- \
  target/debug/ferese-shell-spike
```

The spike intentionally uses public layer-shell. The production shell must use
Ferese's private protocols only for control and semantic effects, never as a
replacement surface role.

The spike can attach the semantic `panel` role to libcosmic's own `wl_surface`
through a supported `wayland-client` connection handle. Run with
`--grant-effects` and `FERESE_SHELL_SPIKE_EXCLUSIVE_FOCUS=1` to make libcosmic
expose the surface deterministically through its public focus event. An
attachment failure is reported without replacing the public layer-shell role.

Source and runtime testing found that libcosmic does not currently forward its
layer-surface creation event to the application. A non-focusable top bar
therefore cannot attach the private effect before mapping through this API.
Production must add a supported upstream creation hook or use the thin native
surface adapter described in the toolkit ADR.
