# Ferese Window Manager Specification

Status: Draft for implementation  
Target: Ferese v0  
Language: Rust  
Display protocol: Wayland

## 1. Purpose

Ferese is a Wayland tiling compositor for a conventional, keyboard-friendly
desktop. It combines predictable window management with compositor-owned rounded
geometry, shadows, animation, transparency, and semantic glass materials.

This document is the implementation source of truth for Ferese v0. The terms
**MUST**, **MUST NOT**, **SHOULD**, **SHOULD NOT**, and **MAY** are normative.

## 2. Product principles

1. Window-management behavior is predictable and familiar.
2. The compositor owns composition effects; clients request semantic intent.
3. Core state has one authoritative representation.
4. Rendering and animation never block input.
5. Optional shell processes may fail without terminating the compositor.
6. Effects degrade gracefully when performance requires it.
7. The nested compositor is the primary development environment.
8. Security-sensitive operations are unavailable to untrusted clients.

## 3. Scope

### 3.1 v0 requirements

Ferese v0 MUST provide:

- native Wayland compositing;
- automatic and manually adjustable tiling;
- floating and fullscreen windows;
- multiple workspaces and outputs;
- keyboard and pointer input;
- native Wayland clipboard, primary selection, and drag-and-drop;
- client-side and server-side decoration negotiation;
- rounded clipping, borders, shadows, transparency, and backdrop blur;
- frame-timestamp-driven window and workspace animations;
- damage-aware rendering and blur caching;
- validated live configuration reload;
- authenticated local IPC and a command-line client;
- a graphical settings application;
- an overview, launcher, OSD, and basic notifications;
- a direct DRM/KMS session; and
- XWayland support, including clipboard interoperability.

The completed v0 MUST support routine software-development work without regular
compositor restarts.

### 3.2 Non-goals

The following are outside v0:

- a file manager, display manager, or lock screen;
- network, Bluetooth, or package-management UI;
- a general plugin, scripting, or theme system;
- desktop widgets;
- advanced dynamic layout algorithms;
- exhaustive support for every Wayland protocol or toolkit edge case; and
- pixel-for-pixel imitation of another operating system.

Ferese MAY display a configured color or image, but a standalone wallpaper
daemon is not part of v0.

## 4. Architecture

### 4.1 Processes

| Executable | Responsibility |
| --- | --- |
| `ferese` | Compositing, policy, input, scene construction, and rendering |
| `ferese-shell` | Overview controls, launcher, workspace UI, notifications, and OSD |
| `ferese-settings` | Graphical configuration editor |
| `feresectl` | Command-line IPC client |

`ferese` MUST remain usable if another Ferese process exits. Shell and settings
code MUST NOT run inside the compositor.

```text
Wayland/XWayland clients
          |
          v
  protocol adapters
          |
          v
 authoritative core state
          |
          +----> layout targets
          +----> animation state
          |
          v
      scene graph
          |
          v
 damage-aware renderer
          |
          v
 nested window or DRM/KMS output
```

Protocol handlers MUST translate requests into domain operations. Business
logic MUST NOT accumulate in Smithay callbacks.

### 4.2 Technology baseline

The initial implementation uses Rust, Smithay, calloop, Smithay's GLES renderer,
`glow`, targeted GLSL shaders, DRM/KMS, GBM, EGL, dma-buf, libinput, xkbcommon,
libseat, serde, TOML, serde JSON, clap, tracing, and GPUI.

Dependencies MAY change when an architectural decision record explains the
reason and migration impact.

### 4.3 Repository layout

```text
ferese/
|-- Cargo.toml
|-- crates/
|   |-- ferese-core/
|   |-- ferese-layout/
|   |-- ferese-render/
|   |-- ferese-animation/
|   |-- ferese-config/
|   |-- ferese-ipc/
|   |-- ferese-protocols/
|   `-- ferese-shell-protocol/
|-- compositor/
|-- shell/
|-- settings/
|-- feresectl/
`-- protocols/
    |-- ferese-effects-v1.xml
    `-- ferese-shell-v1.xml
```

`ferese-layout` MUST be pure Rust and MUST NOT depend on Smithay or Wayland
objects.

### 4.4 Domain operations

Wayland callbacks, input handlers, IPC handlers, configuration reload, and shell
requests MUST enter core policy through explicit domain operations rather than
mutating unrelated managers directly. A representative shape is:

```rust
pub enum Action {
    WindowMapped(WindowId),
    WindowUnmapped(WindowId),
    Focus(Direction),
    Move(Direction),
    Resize(Direction, f32),
    SwitchWorkspace(WorkspaceId),
    MoveToWorkspace(WindowId, WorkspaceId),
    ToggleFloating(WindowId),
    ToggleFullscreen(WindowId),
    SetConfig(ConfigChange),
}
```

An action is applied as one state transaction. The transaction MUST either
complete with all Section 5 invariants restored or leave authoritative state
unchanged. Rendering, protocol configure requests, IPC events, and persistence
are consequences emitted after the state transition; they MUST NOT become a
second source of policy state.

## 5. Authoritative state

The compositor MUST maintain one state root:

```rust
pub struct FereseState {
    pub outputs: OutputManager,
    pub workspaces: WorkspaceManager,
    pub windows: WindowManager,
    pub seat: SeatState,
    pub layout: LayoutManager,
    pub animations: AnimationManager,
    pub config: Config,
}
```

Smithay objects SHOULD remain behind adapter types rather than become the domain
model.

### 5.1 Identifiers

Outputs, workspaces, windows, layout nodes, and the logical seat MUST have stable
internal identifiers. An identifier MUST NOT be reused during one compositor
lifetime.

```rust
pub struct WindowId(u64);
pub struct WorkspaceId(u64);
pub struct OutputId(u64);
pub struct NodeId(u64);
pub struct SeatId(u64);
pub struct SurfaceId(u64);
```

Ferese v0 exposes exactly one logical seat. Multiple physical keyboards, mice,
and touch devices MAY feed that seat. Multi-seat policy is outside v0 and MUST
NOT be implied by the domain model.

### 5.2 Windows

```rust
pub struct Window {
    pub id: WindowId,
    pub app_id: Option<String>,
    pub title: Option<String>,
    pub workspace: WorkspaceId,
    pub placement: WindowPlacement,
    pub fullscreen: bool,
    pub constraints: SizeConstraints,
    pub urgent: bool,
    pub material: WindowMaterial,
}

pub enum WindowPlacement {
    Tiled,
    Floating { rect: RectF },
}
```

Fullscreen is a presentation state layered over the preserved tiled or floating
placement. Entering fullscreen MUST NOT remove a tiled window from its layout
tree or a floating window from its floating list. At most one window may be
fullscreen on a workspace. While fullscreen is active, that window is the sole
normal managed-window content presented for the workspace, although permitted
shell overlays and popups may still appear. Leaving fullscreen restores the
preserved placement without reconstructing it.

A window MUST NOT independently store an output. Its output is derived from its
workspace, preventing stale ownership after workspace migration. Authoritative
keyboard focus belongs to `SeatState` and MUST NOT be duplicated on every
window or workspace. Layout hierarchy MUST NOT be embedded in `Window`.

### 5.3 Workspaces

```rust
pub struct Workspace {
    pub id: WorkspaceId,
    pub name: String,
    pub output: Option<OutputId>,
    pub root: Option<NodeId>,
    pub floating: Vec<WindowId>,
    pub last_focused: Option<WindowId>,
}

pub struct SeatState {
    pub id: SeatId,
    pub keyboard_focus: Option<WindowId>,
    pub pointer_focus: Option<SurfaceId>,
    pub focused_output: Option<OutputId>,
    pub focused_workspace: Option<WorkspaceId>,
}
```

`Workspace::last_focused` is restoration/history state only. It MUST NOT be
interpreted as current keyboard focus.

These invariants MUST hold after every state transaction:

1. Every managed window belongs to exactly one workspace.
2. Every attached workspace belongs to exactly one connected output; only a
   workspace awaiting an output may contain `None`.
3. An output exposes exactly one active workspace.
4. A window with `WindowPlacement::Tiled` appears exactly once in its workspace
   tree and not in the floating list.
5. A window with `WindowPlacement::Floating` appears exactly once in the
   floating list and not in the tree.
6. Fullscreen does not alter the window's underlying placement membership.
7. `seat.keyboard_focus`, when present, refers to a managed window on
   `seat.focused_workspace`.
8. `Workspace::last_focused`, when present, refers to a window belonging to that
   workspace.
9. At most one window is fullscreen on a workspace at a time.

Workspace migration MUST be atomic. If an output disappears, its workspaces move
to a deterministic remaining output. If none remains, they stay detached until
an output appears.

Numeric workspace names are unbounded, but the default bindings directly address
`1` through `9` and create them lazily. Named workspaces MAY be added after v0.

### 5.4 Focus semantics

The single v0 seat owns authoritative keyboard focus, pointer focus, focused
workspace, and focused output. A workspace stores only `last_focused` so focus
can be restored when that workspace becomes active.

When switching to a workspace that is already visible on another output, Ferese
MUST focus that output and workspace instead of moving or swapping the workspace.
Explicit workspace-move commands are responsible for relocating workspaces.

## 6. Layout

```rust
pub enum Node {
    Window(WindowId),
    Split {
        axis: Axis,
        ratio: f32,
        first: NodeId,
        second: NodeId,
    },
    Stack {
        children: Vec<NodeId>,
        active: usize,
    },
}

pub enum Axis {
    Horizontal,
    Vertical,
}
```

Split ratios MUST be finite and clamped so descendants satisfy their minimum
sizes where possible. If all constraints cannot be met, the focused window takes
priority and the conflict MUST be logged.

The engine consumes a layout tree, usable logical geometry, size constraints,
and tiling configuration. It returns a deterministic `WindowId -> RectF` map and
constraint warnings. Identical input MUST produce identical output.

### 6.1 Automatic insertion

1. The first tiled window becomes the root.
2. A later window splits the focused leaf.
3. For `default_split = "automatic"`, use a horizontal split when the focused
   rectangle is wider than tall and a vertical split otherwise.
4. The new window becomes focused after its first mapped commit.

### 6.2 Required operations

Ferese MUST support directional focus, movement and resizing; explicit split
direction; floating and fullscreen toggles; tiled-divider resize; floating
move/resize; and tiled drag-and-drop.

Drag targets are `left`, `right`, `top`, `bottom`, and `center`. A drag MUST show
a live preview. Center creates or joins a stack; an edge creates a split.

### 6.3 Gaps

Gaps belong exclusively to `[tiling]`. `inner_gap` separates tiles and
`outer_gap` separates tiles from output bounds. With `smart_gaps = true`, a lone
tiled window has no outer gap.

## 7. Geometry and animation

Ferese MUST distinguish buffer, surface-local, logical-output, and physical
coordinates. Layout and input policy use logical coordinates. Rendering performs
physical conversion. Fractional scaling MUST avoid cumulative rounding errors
between adjacent tiles.

A managed window has three geometry domains that MUST remain distinct:

1. **logical geometry:** the authoritative layout destination;
2. **visual geometry:** where the compositor currently presents the window while
   an animation is in progress; and
3. **client geometry:** the most recently configured and committed client content
   size.

A representative model is:

```rust
pub struct AnimatedRect {
    pub current: RectF,
    pub target: RectF,
    pub velocity: RectF,
}

pub struct ClientGeometry {
    pub last_configured_size: Size<i32>,
    pub committed_size: Size<i32>,
}

pub struct WindowGeometry {
    pub visual: AnimatedRect,
    pub client: ClientGeometry,
}
```

`visual.target` is the authoritative logical layout rectangle. The target becomes
authoritative immediately after the state transaction even if the client has not
yet committed a buffer matching its new size.

For a non-interactive layout transition Ferese MUST:

1. compute and store the final target geometry;
2. send a configure for the final client size rather than one configure per
   animation frame;
3. continue presenting the most recent committed client content while visual
   geometry interpolates toward the target;
4. map pointer coordinates through the inverse visual transform before
   delivering surface-local input; and
5. accept a newly committed target-sized buffer without restarting the visual
   transition.

The renderer MAY use a short-lived snapshot of the previously committed content
to avoid a visible source-size discontinuity. Whether using a live committed
buffer or snapshot, client content MUST remain aligned with the visual geometry
used for hit testing.

During an interactive resize, configure events MAY follow pointer motion subject
to normal coalescing and client responsiveness. Keyboard navigation MUST use the
target layout topology, not transient visual geometry.

When animation is disabled or reduced motion is active, `visual.current` snaps
to `visual.target`. Geometry uses spring animation; opacity, dimming, blur
intensity, and overlays use bounded easing. Updates MUST use presentation or
monotonic frame timestamps and MUST NOT assume a fixed refresh rate.

Animations MUST stop within configured tolerances. Hidden animations SHOULD stop
requesting frames. Opening, closing, or moving a window MUST never block input.

## 8. Wayland behavior

### 8.1 Required native protocols

Before the native desktop is usable, Ferese MUST support the stable protocols
needed for:

- compositor, subcompositor, shared memory, and outputs;
- xdg-shell;
- xdg-activation;
- seats, keyboard, pointer, and available touch devices;
- data-device clipboard and drag-and-drop;
- primary selection;
- xdg-decoration;
- relative pointer and pointer constraints;
- presentation timing;
- viewporter and fractional scaling;
- idle inhibition; and
- Linux dma-buf where supported.

Layer-shell, screencopy, virtual input, and session-lock MAY be added after their
security policy is specified. Their absence does not block v0 unless a Ferese
component requires them.

### 8.2 Decorations

Ferese MUST support client-side decorations. If a client negotiates server-side
decorations, Ferese MUST provide usable move, resize, and close interactions.
Shadows, masks, and focus borders are compositor effects and may apply in either
mode.

### 8.3 XWayland

XWayland windows use the same `Window` abstraction as native surfaces. The
layout engine MUST remain protocol-agnostic.

XWayland support includes startup, mapping, focus, tiled geometry, floating
hints, selection bridging, and correct stacking for popups and override-redirect
windows.

## 9. Scene and rendering

Ferese MUST build an explicit scene from authoritative state and current
animation values. Nodes SHOULD cover surfaces, popups, decorations, shadows,
masks, borders, materials, shell UI, drag icons, and cursors.

Scene order is background, windows back-to-front, shell/overlays, drag icons,
then cursor. Within a window, the shadow is behind clipped background and client
content; the border is above. Rounded clipping MUST be established before
drawing clipped content or through an equivalent offscreen pass. Client buffers
MUST NOT be modified to implement effects.

Rendering MUST be damage-driven. Damage includes commits, movement, animation,
shadows, borders, material invalidation, shell, cursor, and output changes.
Moving or removing a node MUST damage both old and new bounds. Expansion MUST
include shadow, blur-kernel, and sampling extents.

Direct scanout SHOULD be possible when one compatible fullscreen surface covers
an output and no transform, overlay, capture, conversion, or effect requires
composition.

Rounded corners use a GPU mask or equivalent. Fullscreen windows use zero radius.
Shadows SHOULD use analytic shaders, cached textures, or reduced-resolution
intermediates rather than full-resolution per-frame blur.

## 10. Material system

Initial semantic materials are:

```text
normal
glass.panel
glass.popover
glass.menu
glass.hud
```

The compositor maps semantic names to theme-controlled parameters. Clients
SHOULD request names rather than shader values.

`ferese-effects-v1` is private to trusted Ferese components. It MAY set or clear
a semantic surface material. It MUST NOT expose shader code, unrestricted
capture, or unbounded effect values. Its global MUST be advertised only to
Wayland clients granted the `effects` capability by the private-client mechanism
in Section 13.

Glass renders as:

```text
lower scene content
  -> capture expanded region
  -> downsample
  -> dual Kawase blur
  -> upsample
  -> saturation and brightness
  -> tint and noise
  -> glass foreground
```

Blur MUST exclude the glass surface and all content above it. Sampling bounds
MUST include the kernel radius.

A blur cache entry MUST identify its output and scale, visible and sample
regions, material parameters, contributing lower-layer generations, and result
texture. Changes to content, stacking, geometry, scale, transform, material,
workspace, or output MUST invalidate affected entries. A global generation MAY
be used initially; region-aware invalidation SHOULD replace it before performance
tuning is complete.

### 10.1 Built-in visual profile

v0 provides one restrained dark profile rather than a theme engine. Its defaults
are a subtle blue focus accent, 14 logical-pixel window radius, 10 logical-pixel
tiling gaps, soft shadows, 24 logical-pixel backdrop blur, and mildly increased
glass saturation. Focused and foreground content MUST remain distinguishable
without requiring transparency. Blur, glow, borders, and shadows SHOULD establish
hierarchy rather than compete for attention. Settings, overview, notifications,
launcher, and OSD MUST use the same semantic materials and spacing system.

## 11. Shell and overview

`ferese-shell` owns overview controls, launcher, workspace strip, notifications,
and volume and brightness OSD. Screenshot UI MAY be added after capture policy
and implementation are complete. The shell uses a capability-scoped private
Wayland connection plus authenticated IPC.

Overview MUST NOT require copying each managed window into the shell process.
The compositor owns the live window presentation and enters an overview scene
mode in which existing window scene nodes receive temporary visual transforms.
`ferese-shell` supplies controls such as labels, selection affordances, workspace
UI, and later search.

The overview binding is `Super+Tab`. Entering overview MUST animate windows from
their current visible geometry into overview geometry. Pointer and keyboard
selection MUST target the compositor-owned live window nodes. Dismissing overview
MUST animate those same nodes back to the current layout targets. Workspace
switching from overview MUST obey Section 5.4. Search MAY wait for the launcher
milestone.

### 11.1 Private shell surfaces

Because layer-shell is not required for v0, Ferese MUST provide a private
`ferese-shell-v1` protocol to clients with the `shell-surfaces` capability. A
shell surface is not a managed application window and MUST NOT enter tiling,
workspace focus history, or normal application task switching.

Initial semantic roles are:

```text
launcher
overview-controls
workspace-ui
notification
osd
```

The compositor owns placement, z-order bounds, input region enforcement, and
output association for these roles. The shell provides content and requests a
role; it MUST NOT receive unrestricted arbitrary z-order or geometry authority.
Interactive shell surfaces MAY receive keyboard/pointer focus while active, but
Ferese MUST restore the prior managed-window focus when the interaction ends.
Destroying or disconnecting the shell client MUST remove its shell surfaces
without changing managed-window state.

### 11.2 Application launching and notifications

The launcher MUST execute configured argument vectors directly and MUST NOT pass
them through a shell. Before launch, it MUST obtain an xdg-activation token tied
to the triggering user interaction and make the token available to the child.
The child receives the public Wayland display and normal session environment.
It MUST NOT inherit a private Wayland descriptor, IPC capability, or bearer
secret. A private descriptor deliberately inherited when the compositor starts a
trusted component is the sole exception during that component's initial exec;
the component MUST mark it close-on-exec before it can launch any child.

`ferese-shell` MUST implement the standard `org.freedesktop.Notifications` D-Bus
service for basic notifications. v0 MUST support summary, body, icon metadata,
expiry, replacement, close, and actions. Notification content is untrusted:
markup, image paths, and action identifiers MUST be parsed and rendered without
command execution. If the shell restarts, it MUST reacquire the service and
continue without affecting managed windows.

## 12. Configuration

The primary file is `$XDG_CONFIG_HOME/ferese/config.toml`, falling back to
`~/.config/ferese/config.toml`.

```toml
[appearance]
glass = true
blur_radius = 24.0
glass_opacity = 0.82
corner_radius = 14.0

[appearance.shadow]
enabled = true
radius = 32.0
offset_y = 8.0
opacity = 0.28

[appearance.active_border]
style = "subtle"
width = 2.0

[animations]
enabled = true
reduced_motion = false
speed = 1.0

[animations.spring]
stiffness = 320.0
damping = 28.0
mass = 1.0

[tiling]
inner_gap = 10.0
outer_gap = 10.0
smart_gaps = true
default_split = "automatic"

[input]
focus_follows_mouse = false

[commands]
terminal = ["foot"]

[[bindings]]
keys = "Super+Enter"
action = "spawn"
argument = "terminal"

[[bindings]]
keys = "Super+Q"
action = "close"

[[bindings]]
keys = "Super+Tab"
action = "show-overview"

[[bindings]]
keys = "Super+Space"
action = "show-launcher"
```

Command values are argument arrays, never shell command strings. Binding records
use a normalized key chord, action, and optional argument. The complete built-in
set is listed in Section 14; when a user supplies a record for a chord it
replaces that default. Duplicate normalized chords, unknown actions, invalid
arguments, and references to missing commands MUST fail validation. Every
built-in binding MUST be expressible and replaceable through this schema.

Unknown fields SHOULD warn. Invalid types, non-finite values, negative sizes,
and out-of-range opacity MUST fail validation.

Reload MUST debounce duplicate events, read the entire file, parse, validate,
construct, and apply the difference as one transaction. Invalid input MUST NOT
replace active configuration. Errors MUST be logged and emitted through IPC.

Settings and IPC MUST use the same validation path. Persistent changes MUST be
written atomically using a temporary file and rename.

## 13. IPC and security

### 13.1 Transport and trust

The user-control socket is `$XDG_RUNTIME_DIR/ferese/control.sock`. Its parent
directory MUST be owned by the user with mode `0700`; the socket MUST use `0600`.
The compositor MUST reject peers whose effective UID, read from OS peer
credentials, differs from its own.

Same-UID checks authorize user control only; they do not identify a trusted UI
client. Privileged Wayland access is capability-scoped per client. Initial
capabilities are:

```text
effects
shell-surfaces
global-metadata
capture
virtual-input
```

Trusted UI processes MUST receive a private Wayland connection created by the
compositor. Ferese creates a socket pair, registers the server end as a Wayland
client, and passes the client end only through an inherited descriptor such as
`WAYLAND_SOCKET`. Privilege MUST NOT depend on PID, executable name, command-line
arguments, environment secrets, or a discoverable private socket path.

The compositor associates an explicit capability set with each private Wayland
client and filters private globals and requests against that set. In particular,
`ferese-effects-v1` requires `effects` and `ferese-shell-v1` requires
`shell-surfaces`. `ferese-shell` receives only the capabilities required by
enabled shell features.
`ferese-settings` receives `effects` when spawned as a trusted Ferese component;
its configuration changes still use the normal validated IPC path.

If a trusted component launches another privileged Ferese component, it MUST ask
the compositor to create and pass a new capability-scoped private connection;
it MUST NOT forward its own Wayland descriptor or capabilities.

### 13.2 Framing

IPC v0 uses unsigned 32-bit big-endian length-prefixed UTF-8 JSON:

```text
4-byte payload length | JSON payload bytes
```

The maximum payload is 1 MiB. Oversized, truncated, invalid UTF-8, or invalid
JSON frames close the connection after a rate-limited diagnostic.

```json
{
  "version": 1,
  "id": 42,
  "type": "command",
  "command": "focus",
  "args": { "direction": "left" }
}
```

Responses echo `id` and contain exactly one of `result` or `error`:

```json
{
  "version": 1,
  "id": 42,
  "result": {}
}
```

Errors have stable codes. Unknown fields MAY be ignored; unknown commands and
unsupported versions MUST return errors.

### 13.3 Commands and events

User-control commands are:

```text
focus                 move
resize                workspace
move-to-workspace     toggle-floating
toggle-fullscreen     close
reload-config         get-focused-window
get-workspaces        get-outputs
get-config            set-config
show-overview         hide-overview
show-launcher         hide-launcher
```

Enumerating all window titles or capturing pixels requires shell privilege.

Events include `window-opened`, `window-closed`, `window-focused`,
`window-moved`, `workspace-changed`, `output-added`, `output-removed`,
`config-changed`, and `config-error`. Payloads MUST omit titles and application
IDs without shell privilege.

Subscribers have bounded queues. The server MUST disconnect a subscriber whose
queue overflows rather than block the compositor.

## 14. Input

Ferese MUST support click-to-focus, configurable focus-follows-pointer, floating
move/resize, tiled rearrangement and divider resizing, pointer constraints,
relative pointer, and configurable bindings.

```text
Super+Enter             terminal
Super+Q                 close window
Super+H/J/K/L           focus left/down/up/right
Super+Shift+H/J/K/L     move left/down/up/right
Super+1..9              switch workspace
Super+Shift+1..9        move window to workspace
Super+F                 toggle fullscreen
Super+Shift+Space       toggle floating
Super+Space             launcher
Super+Tab               overview
```

Bare `Super` MUST NOT activate overview. Interactive grabs MUST have explicit
cancellation. Losing a device, seat, surface, or output during a grab MUST end it
safely.

## 15. Outputs

Each connected output has a mode, scale, transform, logical position, usable
region, and one active workspace. v0 MAY choose preferred modes automatically
but MUST handle hotplug without losing windows.

On startup, the first output creates and activates workspace `1`. Selecting an
uncreated numeric workspace from `1` through `9` creates it on the focused
output. A newly connected output receives the lowest unused numeric workspace
name and activates it; this MUST NOT silently move an existing workspace.

Output removal selects a target from the remaining outputs in this order:

1. the v0 seat's focused output, if it remains connected;
2. nearest output to the removed output by logical-center distance;
3. lowest stable `OutputId` as a tie-breaker.

A workspace MAY move between outputs but MUST NOT appear on two outputs at once.
Switching to a workspace already visible on another output changes the seat's
focused output/workspace to that existing presentation and MUST NOT implicitly
move or swap workspaces.

When an output is removed, the migration target keeps its current active
workspace unless the removed output held the seat focus. In that case, the
removed output's active workspace becomes active on the target and the seat's
managed-window focus is restored there where possible.

## 16. Performance and observability

The target is sustained output refresh at 60 Hz on integrated graphics during
ordinary use. Animation MUST also work at higher and variable refresh rates.

Ferese SHOULD track frame, render, layout and blur duration; damaged pixels;
missed presentation deadlines; surface and glass-region counts; and active
animations. Performance tracing MUST be optional. Suggested targets are
`ferese::wayland`, `ferese::layout`, `ferese::render`, `ferese::input`,
`ferese::ipc`, and `ferese::xwayland`.

## 17. Testing

### 17.1 Layout

Unit tests MUST cover insertion, deletion, focus, movement, ratios, constraints,
resize, stacks, floating transitions, fullscreen placement preservation,
workspace migration, gaps, and deterministic geometry. They MUST NOT require
Wayland. Property tests
SHOULD prove arbitrary operation sequences do not duplicate or lose windows.

### 17.2 Configuration

Tests MUST cover defaults, partial files, unknown fields, invalid and non-finite
values, duplicate key chords, unknown binding actions, missing command
references, reload rollback, serialization, and atomic updates.

### 17.3 IPC and security

Tests MUST cover partial reads, multiple frames per read, oversized frames,
unsupported versions, subscriptions, slow clients, peer-UID rejection, and
denial of privileged commands without capability. Launch tests MUST prove that
children receive the public display and activation token but cannot inherit any
private Wayland or privileged IPC descriptor.

### 17.4 Rendering and integration

Pure tests MUST cover clipping, shadow bounds, damage expansion, blur sampling,
cache invalidation, transforms, animated hit testing, and refresh-rate-independent
interpolation. Geometry tests MUST cover mismatches between logical target,
visual geometry, configured client size, and committed client size. Nested
integration tests SHOULD cover configure/commit behavior, inverse-transformed
input during animation, clipboard, popups, drag-and-drop, focus, fullscreen,
xdg-activation, output removal, private-client capability filtering, overview
scene transitions, notification replacement/actions, and shell restart.

## 18. Milestones

A milestone is complete only when its acceptance criteria pass.

### M0: Native window on screen

Implement the nested backend, public Wayland socket, output, seat, xdg-shell,
basic renderer, and commit handling.

Acceptance: `foot` opens and redraws; keyboard, pointer, and popups work; closing
a client leaves the compositor running.

### M1: Essential native protocols

Implement clipboard, primary selection, drag-and-drop, decoration negotiation,
xdg-activation, relative pointer, pointer constraints, presentation timing, and
scaling.

Acceptance: text copies between native clients; drag-and-drop completes;
decorated and undecorated clients can move and resize; scaled hit testing matches
rendered content; a user-initiated test launch receives a valid activation token.

### M2: Tiling and workspaces

Implement identifiers, layout trees, insertion, focus, movement, resize, gaps,
floating, fullscreen, and workspaces.

Acceptance: five terminals tile without overlap; navigation works in all
directions; close reflows layout; modes restore correctly; randomized tests
preserve all invariants.

### M3: Animated geometry

Implement logical, visual, and client geometry separation plus timestamp-driven
animation.

Acceptance: layout changes do not jump; one non-interactive reflow does not send
per-frame configure events; a client may commit its final size mid-animation
without restarting the transition; inverse-transformed hit testing matches the
visible surface; input stays responsive.

### M4: Visual foundation

Implement rounded clipping, shadows, active borders, transparency, background,
and damage expansion.

Acceptance: masks clip correctly; old and new movement bounds repaint;
fullscreen has no unintended corners or gaps.

### M5: Glass materials

Implement semantic materials, capture, dual Kawase blur, tint, saturation, noise,
caching, invalidation, and the private effects protocol.

Acceptance: a Ferese panel blurs only content behind it; backdrop changes
invalidate correctly; the effects global is visible only to a client with the
`effects` capability; a public Wayland client cannot bind it; the demo stays
smooth on the reference integrated GPU.

### M6: IPC and CLI

Implement framing, authorization, events, and `feresectl`.

Acceptance: `feresectl focus left`, `feresectl workspace 2`, and
`feresectl get-workspaces` work reliably. Every request receives a response or
bounded-time error; malformed clients do not affect availability.

### M7: Settings

Build Appearance, Tiling, Shortcuts, Input, Workspaces, Displays, and About.

Acceptance: controls preview live; invalid changes do not replace active config;
committed changes survive restart; shortcut editing uses the canonical binding
schema and detects conflicts; the v0 Displays page accurately reports automatic
mode, scale, transform, and placement state but MAY be read-only; when spawned
as a trusted component, Settings receives only the private Wayland capabilities
it requires.

### M8: Overview and shell

Implement compositor overview scene mode, `ferese-shell-v1`, launcher, workspace
strip, OSD, and the standard desktop notifications D-Bus service.

Acceptance: `Super+Tab` controls overview; managed windows remain compositor-
owned live scene nodes rather than copied shell thumbnails; selection follows
the transformed visual geometry; applications launch without shell parsing or
privileged-descriptor inheritance; notification replacement and actions work;
shell restart does not disrupt windows; each trusted UI process receives only
its assigned capabilities.

### M9: Hardware session

Implement libseat lifecycle, DRM, GBM, devices, hotplug, and VT/session state.

Acceptance: Ferese starts from a TTY; deactivation releases resources correctly;
output removal migrates workspaces deterministically.

### M10: XWayland and stabilization

Implement XWayland startup, mapping, hints, focus, selection bridging, and
stacking. Complete crash, soak, and performance testing.

Acceptance: representative X11 applications tile and float; clipboard works both
ways between X11 and Wayland; a 24-hour mixed-client soak has no compositor crash
or unbounded resource growth; all Section 3.1 requirements pass.

## 19. First implementation slice

```text
nested Smithay compositor
  -> two xdg-shell terminals
  -> deterministic two-window tiling
  -> logical/visual/client geometry separation
  -> spring reflow with coalesced final configure
  -> rounded clipping
  -> one capability-scoped trusted glass panel
```

The slice succeeds when A and B tile automatically; closing B immediately updates
the logical layout while A expands visually; A receives no per-frame configure
flood; pointer input remains aligned through inverse transforms; a private client
with only the `effects` capability can display a panel that blurs A; a public
client cannot bind that protocol; terminal changes refresh the backdrop; and all
interaction remains responsive and damage-driven.

Settings, XWayland, hardware multi-output, notifications, and theming MUST NOT
delay this slice.

## 20. Definition of v0

Ferese v0 is complete when a user can:

1. start a direct Ferese session;
2. run ordinary Wayland and XWayland applications;
3. copy, paste, drag, and use application popups;
4. tile, focus, move, resize, float, fullscreen, and close windows;
5. use multiple workspaces and outputs;
6. configure gaps, materials, corners, borders, shadows, and animations live;
7. use overview, launcher, OSD, and basic notifications;
8. control permitted behavior through `feresectl`;
9. recover from shell or settings crashes without losing client sessions; and
10. perform routine programming work without regular compositor restarts.

The intended result is a low-latency tiling environment with a restrained,
coherent system shell. Visual effects support hierarchy and usability; they do
not replace correctness, responsiveness, or security.
