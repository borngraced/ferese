# Ferese

Ferese is an experimental Wayland scrolling compositor written in Rust with
Smithay. Scrolling columns are the default, tree tiling remains available as an
optional per-workspace mode, and geometry changes use timestamp-driven
animation. See the
[window-manager specification](docs/ferese-window-manager-spec.md) for the
current architecture and milestones.

## Development

Install the native dependencies required by Smithay's winit and GLES backends,
then run:

```bash
cargo run -p ferese
```

Pass a client and its arguments after `--` to launch it inside Ferese:

```bash
cargo run -p ferese -- foot
```

Set `RUST_LOG=ferese=debug` for detailed compositor logging.

Set `FERESE_TRACE_PERFORMANCE=1` to emit five-second per-output summaries under
the `ferese::render` tracing target. Summaries include rendered frames, damaged
pixels, average and longest render time, and DRM missed-deadline totals.

The native-client soak runner and its nested/DRM procedure are documented in
[docs/native-soak-testing.md](docs/native-soak-testing.md).

Ferese reads `$XDG_CONFIG_HOME/ferese/config.toml`, or
`~/.config/ferese/config.toml` when `XDG_CONFIG_HOME` is unset. New scrolling
columns use half the viewport by default. To make every newly opened column use
the full viewport width:

```toml
[scrolling]
default_column_width = "full"
```

`default_column_width = 1.0` is equivalent. `Super+M` switches the active
workspace between scrolling and tree modes.

Outer gaps remain visible with one window by default. To intentionally remove
them for a lone tiled window:

```toml
[layout]
smart_gaps = true
```

Managed-window borders use theme tokens and overlay the window edge without
adding inner padding or changing client geometry:

```toml
[theme.colors]
border = "#FFFFFF18"
accent = "#5B8CFF"
shadow = "#00000055"

[theme.geometry]
border_width = 1.0
focus_ring_width = 2.0
window_radius = 14.0

[theme.shadow.soft]
offset_y = 4.0
blur = 18.0
opacity = 0.20
```

Fullscreen windows are borderless and shadowless. Other managed windows use GPU
rounded clipping, matching rounded focus rings, and an analytic soft shadow.
These effects overlay or surround the allocation without adding client padding.

Scrolling keeps windows in stable workspace coordinates and animates the
viewport only when focus must be revealed. Set
`focus_strategy = "center_on_focus"` under `[scrolling]` to center focused
columns instead.

Scrolling controls use `Super+R` to cycle configured width presets, `Super+C`
to center the focused column, `Super+[` to consume the focused window into an
adjacent column, and `Super+]` to expel it. `Super+Ctrl+H/J/K/L` resizes in the
corresponding direction.

Keyboard and direct-session touchpad settings are applied at startup:

```toml
[input]
focus_follows_mouse = false
xkb_layout = "us"
xkb_variant = ""
xkb_options = []
repeat_rate = 25
repeat_delay_ms = 600

[input.touchpad]
tap = true
natural_scroll = true
disable_while_typing = true
```

Touchpad settings apply to libinput devices in the direct DRM session. In the
nested backend, the host compositor remains responsible for physical touchpad
configuration.

The direct DRM backend selects the first output profile whose listed monitors
are all connected. Match either the connector name reported by `get-outputs`
or its persistent EDID-derived identity:

```toml
[[output_profiles]]
name = "docked"

[[output_profiles.outputs]]
match = "HDMI-A-1"
mode = "3840x2160@119.998"
scale = 1.6
position = [0, 0]

[[output_profiles.outputs]]
match = "eDP-1"
enabled = false

[[output_profiles]]
name = "laptop"

[[output_profiles.outputs]]
match = "eDP-1"
mode = "2880x1800@120"
scale = 1.6
position = [0, 0]
```

Unspecified connected outputs remain enabled with their preferred mode, scale
`1.0`, normal transform, and automatic horizontal placement. Supported
transforms are `normal`, `rotate_90`, `rotate_180`, `rotate_270`, `flipped`,
and their rotated flipped variants. Output configuration is applied at startup
and on hotplug; live configuration reload remains deferred.

Commands are argv arrays and are started directly, without a shell. The
default `Super+Enter` binding runs the built-in `terminal = ["foot"]` command:

```toml
[commands]
terminal = ["foot", "--app-id", "terminal"]

[[bindings]]
keys = "Super+Enter"
action = "spawn"
argument = "terminal"
```

A configured binding replaces the built-in binding with the same normalized
chord and match mode. Disable a default explicitly with no action:

```toml
[[bindings]]
keys = "Super+Q"
disabled = true
```

Bindings follow the active layout by default. To bind a keyboard position
instead, set `match = "physical"` and use an XKB physical key name such as
`AD06`. Invalid chords, actions, arguments, duplicate bindings, and missing
command references stop startup with an error.

Initial window rules match `app_id`, exact title, and/or transient status. All
matching rules are applied in declaration order; later rules override only the
fields they specify. Width or height implies floating placement when `floating`
is omitted:

```toml
[[window_rules]]
app_id = "org.example.Editor"
workspace = 3
floating = true
width = 900.0
height = 600.0
fullscreen = false
```

Rules are finalized on the first toplevel commit, before its buffer is
presented, and are not re-applied when an application later changes its title.

`feresectl` controls the running compositor through the same-user socket at
`$XDG_RUNTIME_DIR/ferese/control.sock`. For example:

```bash
cargo run -p feresectl -- focus left
cargo run -p feresectl -- workspace 2
cargo run -p feresectl -- get-workspaces
cargo run -p feresectl -- get-outputs
```

The v0 IPC slice also supports `move`, `resize`, `move-to-workspace`,
`toggle-floating`, `toggle-fullscreen`, `toggle-layout`, `cycle-column-width`,
`center-column`, `consume`, `expel`, `close`, `get-focused-window`, and
`get-outputs`. Requests use versioned, 1 MiB-limited length-prefixed JSON. The
runtime directory and socket are restricted to modes `0700` and `0600`, and the
server rejects peers whose effective UID differs from the compositor's.
`get-outputs` includes disabled-but-connected monitors, connector and persistent
identity matchers, current and available modes with refresh rates, fractional
scale, transform, logical position and size, and physical dimensions.

Text-input-v3 is available to applications by default. Input-method-v2 is
hidden unless the session explicitly opts in, because an input method can
request privileged keyboard access. Enable it only when launching a trusted
IME for the session:

```bash
FERESE_ENABLE_INPUT_METHOD=1 cargo run -p ferese
```

`Ctrl+Alt+Escape` always releases an active client shortcut inhibitor.

Keyboard-shortcut inhibition is hidden unless a trusted VM or remote-desktop
session explicitly enables it:

```bash
FERESE_ENABLE_SHORTCUT_INHIBIT=1 cargo run -p ferese
```

The emergency `Ctrl+Alt+Escape` binding remains available while inhibition is
active.

Screen capture is hidden by default. A session that has already made an
explicit capture-policy decision can expose screencopy-v1 to its clients with:

```bash
FERESE_ENABLE_SCREENCOPY=1 cargo run -p ferese
```

The current M5 implementation supports one-shot output and region capture into
ARGB8888 shared-memory buffers. Cursor overlays are included only when the
capture client requests them.

### Direct DRM session

Run the hardware backend from a spare TTY, outside another graphical session:

```bash
RUST_LOG=ferese=debug cargo run -p ferese -- --backend=drm -- foot
```

Do not run that command from a terminal inside the active desktop: the direct
backend requests control of the seat and primary DRM device. Use
Ctrl+Alt+F1–F12 to switch virtual terminals.

M4 validation requires the direct session to:

- start `foot` and render the same animated tiling workload as the nested path;
- retain keyboard and pointer input;
- pause when switching away and redraw after switching back; and
- log page-flip timing and any missed presentation deadlines on the reference
  integrated GPU.

The direct path creates an independent render pipeline per connected output and
rescans connectors on hotplug. Physical unplug/replug acceptance testing on the
reference hardware is still pending.
