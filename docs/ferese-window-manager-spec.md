# Compositor architecture

This is a maintainer reference. For supported configuration and shortcuts, use
[Configuration](configuration.md); for builds and checks, use [Development](development.md).

## Processes and crates

| Component | Responsibility |
| --- | --- |
| `ferese` | Wayland clients, windows, workspaces, input, outputs, and rendering |
| `ferese-shell` | Bar, popovers, notifications, widgets, and system confirmations |
| `ferese-settings` | Configuration editor |
| `feresectl` | Command-line control |
| `ferese-lock` | Lock-screen UI and PAM authentication |
| `ferese-polkit-agent` | System authentication prompts |
| `xdg-desktop-portal-ferese` | Desktop portal requests and PipeWire streams |
| `ferese-record` | Video encoding and saving |

The compositor uses Smithay and calloop. The direct backend renders through
DRM/KMS; the nested backend renders into a window on another Wayland desktop.
Native UIs use libcosmic. Layout, animation, configuration, IPC, protocols, and
[theme components](../crates/ferese-theme/README.md) live in separate crates.

## State and ownership

The event loop owns compositor state and Wayland objects. Background workers
return results through bounded channels; results must be checked against current
window, output, and request identities before application.

Keep these relationships valid after a state change:

- Each window belongs to one workspace and appears once in its layout or floating list.
- Each attached workspace belongs to one output; each output has one active workspace.
- A window's output follows workspace ownership.
- Fullscreen preserves the underlying layout or floating placement.
- Focus points to a live window or authorized shell surface.
- Workspace focus history is distinct from current keyboard focus.

Monitor removal migrates workspaces to a remaining output. Returning monitors
restore eligible workspaces; explicit user moves take priority. Selecting a
workspace visible on another monitor focuses that monitor.

## Layout and presentation

Scrolling layouts arrange columns horizontally. Windows within a column divide
its height. A workspace owns its viewport; presentation subtracts that viewport
from window positions. Tree layouts use nested splits and stacks. Layout changes
preserve window membership and floating placement.

Keep layout targets, animated rectangles, and committed client sizes separate.
A layout transition requests the destination client size rather than sending
one configure per animation frame. Available content is scaled and clipped until
the matching client buffer arrives. Hit testing uses the same presented geometry.
Interrupted animations continue from their current position.

Input and layout use logical pixels. Rendering converts them using the output's
scale and transform. Application popups use their parent's output and presented
position when calculating bounds.

## Rendering and frame delivery

Render damage from client commits, animations, effects, cursor motion, and output
changes. Movement damages both old and new bounds, including shadows and blur.
Output-specific callbacks follow window ownership. Presentation feedback is
collected only for surfaces the renderer actually displayed on that output.
An unchanged frame still permits paced client callbacks without submitting a
new display buffer.

Animations use elapsed time across outputs, with a timer while transitions remain
active. An idle focused monitor must not stall another monitor's animation.

The compositor renders blur from content behind the requesting surface. Material
regions and client content must share coordinates during placement, scaling,
and animation. See [Theme components](ferese-theme-ui-spec.md).

## Input

Use milliseconds for keyboard, pointer, touch, and gesture protocol events;
relative-motion timestamps retain microseconds. Keep keyboard focus, pointer
focus, and window stacking separate. Hover focus does not scroll or raise windows.

Pointer grabs and constraints must end cleanly when their surface disappears.
A locked pointer's position hint is stored while locked, then resolved against
current surface geometry and clamped to an output when the lock is released.

## IPC and private protocols

User control uses `$XDG_RUNTIME_DIR/ferese/control.sock`, restricted to the
session user. The parent directory is private and peers are checked by UID.
IPC frames are length-prefixed JSON with bounded message sizes, workers, and queues.
See `feresectl --help` for commands.

Private Wayland capabilities grant shell control or material effects to trusted
components. Public layer-shell access alone grants neither. Trusted connections
and descriptors must not leak to launched applications. Capture paths check
session policy and reject capture while locked.

Session locking must protect every output, including hotplug, and remain secure
if the lock UI crashes. [Portal integration](portals.md) documents application
consent and the currently supported services.

## Review and testing

Check output ownership, presented geometry, focus restoration, request lifetimes,
and bounded work whenever changing a protocol or rendering path. Test both
backends; a nested result does not establish hardware behavior.

Use the [animation](animation-validation.md), [resize](nested-resize-testing.md),
and [soak](native-soak-testing.md) checks alongside the workspace tests.
