# Ferese

Ferese is a Wayland scrolling compositor written in Rust with
Smithay. Scrolling columns are the default, tree tiling remains available as an
optional per-workspace mode, and geometry changes use timestamp-driven
animation. See the
[window-manager specification](docs/ferese-window-manager-spec.md) for the
current architecture and milestones.

For a versioned installation alongside Plasma in SDDM, see
[installation and recovery](docs/installation.md). `Super+Shift+E` or
`feresectl exit` logs out immediately; save your work first.

## Live configuration

`ferese-settings` opens the native settings application, also available from
Control Center → Settings after installation. It includes appearance presets,
wallpaper selection, menu bar geometry, window behavior, motion, keyboard/mouse,
custom shortcut editing, login items, and existing display profiles. Changes save
automatically; text fields save on Enter or when leaving the field. Sliders save
when released. Settings also refreshes after external config edits, preserving
active drafts. Display profiles, touchpad options, and login items update live.

The app validates changes with `ferese --check-config PATH` before atomically
replacing your file, preserves comments and unrelated keys, and keeps a
`config.toml.settings-backup`. Undo restores the previous save; external edits
require Reload before saving. Search filters settings categories. To preview
against a disposable config without changing your desktop config:

```sh
cargo build -p ferese -p ferese-settings --locked
target/debug/ferese-settings --config /tmp/ferese-settings-preview.toml
```

The GUI is not a replacement for every advanced config option: adding display
profiles, arranging outputs, new shortcut definitions, and window rules remain
in `config.toml`. Theme presets update palette and focus-gradient colors without
replacing custom rules, bindings, wallpaper, or layout settings.

New installations float Settings by default. To do the same in an existing
config, add a `[[window_rules]]` entry with `app_id = "dev.ferese.Settings"`
and `floating = true`.

Saving `~/.config/ferese/config.toml` (or `$XDG_CONFIG_HOME/ferese/config.toml`)
reloads it automatically. Atomic editor saves are supported and bursts are
debounced. Invalid edits keep the last working compositor and shell config;
the rejection is logged. Idle watching does not reread the file.

Use `feresectl reload-config` to reload manually and report validation errors.
Live sources are limited to 60 KiB for safe delivery over the private protocol.

Appearance, fonts, wallpaper, animations, keybindings, keyboard layout/repeat,
gaps, scrolling focus policy/default widths, and bar geometry reload live.
Wallpaper decoding stays off the render thread and keeps the old image until
the replacement is ready. Layout-mode changes convert existing workspaces;
default-width changes update columns using the previous default, preserving
custom widths. Changed window rules are reapplied to existing matching windows.
Output profiles and touchpad changes apply immediately. Display changes retain
output/workspace identity; configurations with no usable output are rejected.
The compositor and shell need one initial update/restart to enable this feature.

## Session services and locking

`[[autostart]]` entries run after the compositor's public socket is ready:

```toml
[[autostart]]
command = ["awari"]
enabled = true
restart = true

[[autostart]]
command = ["swayidle", "-w", "timeout", "600", "ferese-lock", "before-sleep", "ferese-lock", "lock", "ferese-lock"]
restart = true
```

Services are session-owned, reaped, and restarted with a five-second retry delay.
They receive the correct public Wayland display without private shell privileges.
Nested previews skip them unless `nested = true` is explicitly configured.
Autostart changes reconcile live: unchanged commands keep their processes,
new/enabled items start, and disabled/removed commands stop without blocking rendering.
Run foreground daemons here, not commands that fork themselves into the background.

Ferese implements `ext-session-lock-v1`. While locked, only lock surfaces over an
opaque fallback are rendered, normal input/shortcuts and IPC are blocked, and
screencopy is unavailable. DRM confirmation waits for safe page flips on active
outputs. A missing/crashed locker leaves the session locked; it is not an unlock.
`ferese-lock` runs swaylock/swaylock-effects with optional
`~/.config/ferese/swaylock.conf`, waiting for lock confirmation before daemonizing.
The compositor authenticates no passwords itself; authentication belongs to the locker.
Validate password unlock in a disposable nested session before enabling automatic
locking on a primary session. Crash recovery currently requires ending that session
from another VT; there is deliberately no unauthenticated IPC unlock.

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

`Super+F` toggles decorated maximization inside the workspace (bar and outer
gaps remain visible). In scrolling mode this expands the column to full width,
keeps it in the strip, and restores its previous width when toggled off. New
windows open to its right and scroll into focus.
`Super+Shift+F` toggles true fullscreen (no bar or window
decorations). Leaving fullscreen restores the previous maximized/tiled state.

Set `FERESE_TRACE_PERFORMANCE=1` to emit five-second per-output summaries under
the `ferese::render` tracing target. Summaries include rendered frames, damaged
pixels, average and longest render time, and DRM missed-deadline totals.

The native-client soak runner and its nested/DRM procedure are documented in
[docs/native-soak-testing.md](docs/native-soak-testing.md).

The M7 semantic-effects probe must use a compositor-created private connection.
Build it, then grant only the `effects` capability to that launched process:

```bash
cargo build -p ferese-effects-probe
cargo run -p ferese -- --grant-effects -- target/debug/ferese-effects-probe
```

Add `--preview` to keep a solid popover visible in the nested compositor:

```bash
cargo build -p ferese-effects-probe
cargo run -p ferese -- --backend nested --grant-effects -- \
  target/debug/ferese-effects-probe --preview
```

Press `Super+Enter` in the Ferese window to open `foot`, then move or update the
terminal beneath the popover to inspect stacking, rounded clipping, and shadows. Stop the preview with `Ctrl+C` in the host terminal.

To verify that the private global is absent from the public Wayland socket:

```bash
cargo run -p ferese -- target/debug/ferese-effects-probe --expect-hidden
```

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
outer_gap = 4.0
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

[theme.material]
style = "solid" # solid or translucent
blur_radius = 12.0 # logical pixels; translucent only, 0 disables blur

[appearance.inactive_dim]
enabled = true # opt-in; disabled by default
amount = 0.15 # dark tint over unfocused windows, 0–1; 0 disables
duration_ms = 150 # 0 snaps immediately; reduced motion also snaps
```

Inactive dimming follows window focus with a smooth, interruption-safe fade.
Focus-ring paint (including gradients) and width use the same `duration_ms`,
even when dimming is disabled. Focus changes never scale or move the window.
Shell buttons fade their hover tint over 120 ms. Both transitions respect
`animations.speed`, `animations.enabled`, and `animations.reduced_motion`;
click actions remain immediate and settled hover states do not animate.
It keeps window opacity intact, respects animated geometry and rounded corners,
and leaves shell bars/popovers untouched. Overview temporarily removes dimming.

Material roles use the configured surface color and role-specific shadows.
Solid is the default. Translucent adds backdrop blur behind the same surface
color, without grain, glossy lighting, saturation boosts, or inset highlights.
The radius is clamped to 32 logical pixels. A shader/allocation failure falls
back to a plain translucent fill. Shell popovers report rounded card regions;
their outer padding and gaps stay transparent. Each surface shares one capture
across its cards, so cards do not sample already blurred neighboring cards.
Use `--preview --regions` with the effects probe to inspect two cards over a
patterned background, including the sharp gap between them.

```toml
[appearance]
corner_radius = 14.0
```

Shell shadows use restrained logical-pixel presets: panel `1/5/4%`, popover
and menu `2/8/7%`, HUD `1/4/4%`, and notification/modal `3/10/9%`
(vertical offset / blur / opacity). Private effects capabilities
remain required; public background-effect requests cannot select these materials.

Fullscreen windows are borderless and shadowless. Other managed windows use GPU
rounded clipping, matching rounded focus rings, and an analytic soft shadow.
These effects overlay or surround the allocation without adding client padding.

Scrolling keeps windows in stable workspace coordinates and animates the
viewport only when focus must be revealed. Set
`focus_strategy = "center_on_focus"` under `[scrolling]` to center focused
columns instead.

Use `focus_strategy = "paged"` to pack consecutive columns into viewport-sized
pages. Half-width columns form pairs, thirds form triples, and full-width
columns occupy their own page. Mixed widths and client minimum sizes use their
actual allocated widths, including gaps. Focusing within a page keeps it still;
crossing a page boundary scrolls to that page. Explicit centering remains in
effect until focus changes or the output resizes.

Scrolling controls use `Super+R` to cycle configured width presets, `Super+C`
to center the focused column, `Super+[` to consume the focused window into an
adjacent column, and `Super+]` to expel it. `Super+Ctrl+H/J/K/L` resizes in the
corresponding direction.

`Super+Tab` toggles the compositor overview. Its live window previews retain
their client surfaces, `Super+H/J/K/L` moves the selection using the presented
preview geometry, and clicking a preview activates it without forwarding the
selection click to the application.

The shell provides an edge-to-edge libcosmic menu bar backed
by the private shell-control connection. Build and preview it nested with:

```bash
cargo build --release -p ferese -p ferese-shell
target/release/ferese --backend nested --grant-effects --grant-shell-control -- \
  target/release/ferese-shell
```

The Ferese mark enters overview, where workspace indicators become available.
The active workspace's window list appears on the left, with click-to-focus
and a highlighted active window; status controls and day/time sit on the
right. The 28px bar has square screen edges, 19px icons, and no border or drop
shadow. Window spacing uses the workspace outer gap; `top_bar_window_gap` under
`[theme.geometry]` adds optional extra space (default 0). Solid and translucent
shell surfaces also omit outlines. The dark startup fallback matches the material palette.

The compositor decodes wallpaper in parallel with startup. Small shell surfaces prefer the lightweight
software renderer, with GPU rendering available as a fallback. Set `ICED_BACKEND=wgpu` to explicitly use GPU
rendering for shell content. Use release builds when evaluating startup and
animation performance; debug image processing is considerably slower.

Resize presentation configures the destination once and coordinates the
matching client commits with the workspace motion (300 ms maximum wait).
Application content stays at native pixel size; animated bounds crop/reveal
it rather than stretching text. A resize-only previous-frame snapshot fades
out over 80 ms, with a 64 MiB total snapshot budget and automatic release.
All monitors share one elapsed-time animation clock, and window content,
clips, borders and shadows use the same physical-pixel edges.

Window borders and focus rings use solid colors by default. Optional linear
gradients can be configured independently:

```toml
[theme.focus_ring.gradient]
from = "#e5c890"
to = "#b98d58"
angle = 135.0

# Optional unfocused-window gradient:
[theme.border.gradient]
from = "#3e3e43"
to = "#242429"
angle = 90.0
```

Angles are clockwise in window coordinates: 0 is left-to-right and 90 is
top-to-bottom. Both endpoints accept `#RRGGBB` or `#RRGGBBAA`. Remove either
gradient section to restore its solid `theme.colors.accent` or `border` color.
Gradients share the existing rounded-border shader and add no animation or
extra render pass. Configuration changes reload live.

Overview has a horizontally scrollable workspace strip with live window
thumbnails and an accent outline on the workspace shown by each output.
Click a workspace to change the window grid without leaving overview; click
a window to focus it and exit, or press Escape to dismiss. Only the workspace
strip has a tinted surface; the window grid sits directly over the wallpaper.
Strip labels use `[theme.typography].font_family` and rasterize at output scale.
The strip fades independently and preserves its opacity on rapid reversals,
including when the selected workspace is empty.

Ferese renders `[theme.background]` wallpapers directly: `path` selects the
image and `mode = "fill"` or `"fit"` controls crop/letterboxing. One decoded
image and one texture per GPU context serve every output; resizing requires
no image reload or full-screen shell buffers. The launched shell receives
`FERESE_COMPOSITOR_WALLPAPER=1` to disable its redundant wallpaper surfaces.
When no valid image path is configured, the shell's existing fallback remains.

Keyboard and direct-session touchpad settings reload live:

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
and their rotated flipped variants. Output configuration is applied at startup,
on hotplug, and live reload, including modes, scale, transform, and position.

On DRM, closing the laptop lid disables internal panels when a usable external
output is available. With no usable external output, the panel stays enabled;
Ferese does not change the system's suspend policy. Workspaces are evacuated
to a remaining output without merging their windows, and return to their
original output when the lid reopens or a disconnected monitor returns.
Normal use of an evacuated workspace does not cancel its automatic return.

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

With `floating = true` and no explicit dimensions, the application chooses its
initial size and Ferese centers that size on its workspace's output. Small
dialogs are not forced into a workspace-sized container.

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
`toggle-floating`, `toggle-maximized`, `toggle-fullscreen`, `toggle-layout`, `cycle-column-width`,
`center-column`, `consume`, `expel`, `close`, `get-focused-window`, and
`get-outputs`. Requests use versioned, 1 MiB-limited length-prefixed JSON. The
runtime directory and socket are restricted to modes `0700` and `0600`, and the
server rejects peers whose effective UID differs from the compositor's. IPC has
a bounded request queue and accepts at most 64 concurrent client connections.
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
