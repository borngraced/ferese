# Ferese Shell Design Specification

Status: Draft for implementation  
Target: Ferese desktop tier  
Normative owner: shell behavior and interaction

Companion documents:
- `ferese-window-manager-spec.md` — compositor mechanism and policy
- `ferese-theme-ui-spec.md` — visual tokens and exact component geometry

## 1. Purpose

`ferese-shell` provides the integrated desktop interface around Ferese-managed
windows.

The initial shell consists of:

- one floating top bar per output;
- an apps-only launcher;
- overview controls;
- top-bar popovers and a compact control center;
- notifications and notification history; and
- volume/brightness OSD.

Ferese deliberately provides no persistent dock or bottom application bar.

The shell MUST remain optional. If it crashes, managed windows, layout trees,
workspaces, focus topology, and client sessions remain valid.

## 2. Document boundaries

This document owns:

```text
shell visibility
layer-shell roles
exclusive-zone behavior
focus acquisition/restoration
launcher behavior
top-bar actions
notification/DND behavior
modal interaction
fullscreen shell behavior
shell restart behavior
```

It MUST NOT define raw colors, pixel heights, radii, blur amounts, icon sizes,
font sizes, or shadows. Those belong to `ferese-theme-ui-spec.md`.

## 3. Surface architecture

The shell uses public layer-shell for normal shell surfaces.

```text
background(output)         background layer, optional
top_bar(output)            top layer, exclusive zone
launcher(output)           overlay layer, exclusive_zone = 0
notification_stack(output) overlay layer, exclusive_zone = 0
osd(output)                overlay layer, exclusive_zone = 0
popover(output)            overlay/top, exclusive_zone = 0
modal(output)              overlay layer, exclusive_zone = 0
```

`ferese-shell-v1` is a private control protocol only. It MUST NOT duplicate
layer-shell geometry or roles.

Its initial responsibilities are:

```text
window list and lifecycle
application identity metadata
focused / urgent state
activate managed window
request close
enter / exit overview
select overview window / workspace
```

## 4. Top bar

A top bar exists on every active output unless disabled.

Its conceptual regions are:

```text
LEFT
    Ferese mark
    workspace switcher
    focused application

CENTER
    clock / date

RIGHT
    notifications
    sound
    power
    control-center entry
```

The top bar is the only persistent shell surface that reserves workspace space.

Workspace controls MUST use compositor workspace truth and ordinary Ferese
workspace policy.

The top bar SHOULD remain non-keyboard-interactive during ordinary use.

## 5. Fullscreen behavior

Entering fullscreen hides the top bar visually.

The underlying tiled workspace layout region MUST remain unchanged while
fullscreen is active.

A temporary top-bar reveal over fullscreen:

```text
uses overlay presentation
uses exclusive_zone = 0
does not resize the fullscreen client
does not mutate the underlying workspace layout
```

Leaving fullscreen restores the ordinary top-bar presentation without
reconfiguring hidden tiled windows merely because the bar reappears.

## 6. Shell reservation lease

If `ferese-shell` disconnects unexpectedly, Ferese SHOULD retain the most recent
valid Ferese-owned top-bar reservation for a short bounded grace period.

If the shell reconnects before expiry with a compatible reservation, tiled
windows MUST NOT reflow.

If the grace period expires, Ferese removes the reservation and performs at most
one ordinary layout reflow.

Default:

```text
reservation_grace_ms = 2000
```

## 7. Launcher

The initial launcher is apps-only.

Default invocation:

```text
Super + Space
```

The launcher MAY also be opened by a Ferese control in the top bar.

The launcher:

- receives keyboard focus immediately;
- discovers XDG desktop applications;
- searches by application name and normalized desktop identity;
- supports pointer and keyboard navigation;
- launches selected applications with XDG activation;
- dismisses on Escape; and
- restores the previous valid managed-window focus on dismissal.

Initial placeholder:

```text
Search apps…
```

The launcher MUST NOT expose dead tabs for files, settings, commands, or recents.

Future search providers MAY extend it without changing the base interaction
model.

## 8. Application identity and launch state

The launcher consumes the compositor's normalized application identity and launch
association model.

It MUST NOT assume strings such as:

```text
firefox
org.mozilla.firefox
Mozilla Firefox
```

are the same application unless desktop-entry or activation metadata establishes
that relationship.

Ferese does not maintain a persistent running-application strip.

## 9. Top-bar popovers

A top-bar control may own one anchored popover.

Only one primary top-bar popover SHOULD be open per output at a time.

Initial popovers MAY include:

```text
calendar / time
sound
battery / power status
notifications
control center
```

Network and Bluetooth configuration are not part of the initial desktop tier and
MUST NOT appear as required or placeholder rows.

Opening another primary popover closes the previous one on that output.

## 10. Control center

The initial control center MAY expose only services backed by real providers:

```text
sound
battery / power status
Do Not Disturb
notification state
Ferese Settings
```

Unavailable capabilities are omitted.

## 11. Do Not Disturb

When DND is off:

```text
eligible notifications may create transient toasts
notifications enter history according to normal policy
```

When DND is on:

```text
normal transient toasts are suppressed
notifications still enter history
unread state still updates
```

A future explicitly defined critical-urgency policy MAY bypass DND.

No application-name heuristic may bypass DND.

DND state persists across shell restart.

## 12. Notifications

`ferese-shell` implements `org.freedesktop.Notifications`.

Initial support:

```text
summary
body
icon metadata
expiry
replacement
close
actions
urgency metadata
```

Notification content is untrusted.

Transient toasts:

- do not reserve workspace space;
- appear beneath the top bar;
- obey DND;
- do not take keyboard focus by default.

## 13. Notification center

Notification history opens from the top bar.

The initial implementation SHOULD use a large anchored popover rather than a
permanent side drawer.

The center may take shell keyboard focus while interactive and MUST restore the
previous managed-window focus when dismissed.

## 14. OSD

OSD handles transient state such as:

```text
volume
brightness
microphone mute
keyboard backlight
```

OSD surfaces:

```text
use overlay layer
use exclusive_zone = 0
never take keyboard focus
dismiss automatically
```

## 15. Overview

`Super+Tab` enters overview.

Managed-window previews remain compositor-owned live scene nodes.

The shell provides:

```text
selection affordances
application/window labels
workspace controls
optional later search UI
```

The shell MUST NOT implement overview by continuously copying each managed
window into the shell process.

Window activation and close actions use `ferese-shell-v1`.

## 16. Modal interaction

Shell modals are reserved for:

```text
destructive confirmation
security-sensitive confirmation
irreversible shell action
```

Ordinary settings MUST NOT use shell modals.

When a shell modal opens:

```text
SeatState.keyboard_focus -> FocusTarget::Shell(modal)
```

The compositor presents the modal above a scrim.

Input outside the modal is consumed by the scrim and MUST NOT reach managed
applications.

On dismissal, the previous valid managed-window focus is restored.

Ferese does not ship a polkit authentication agent in the initial desktop tier.

## 17. Shell restart

On restart:

```text
managed windows remain unchanged
workspace assignments remain unchanged
layout trees remain unchanged
managed-window focus remains valid where possible
```

The restarted shell reconstructs:

```text
top bars
workspace indicators
focused-app metadata
notification service/state
DND state
```

from compositor truth and persistent shell state.

## 18. Shell configuration

Representative behavior-only configuration:

```toml
[shell]
enabled = true
reservation_grace_ms = 2000

[shell.top_bar]
enabled = true
hide_in_fullscreen = true
show_workspaces = true
show_focused_app = true
show_clock = true

[shell.launcher]
enabled = true
binding = "Super+Space"

[shell.overview]
enabled = true
binding = "Super+Tab"

[shell.notifications]
enabled = true
do_not_disturb = false
```

Visual measurements do not belong here.

## 19. First shell implementation slice

```text
top bar
launcher
one top-bar popover
OSD
```

Demo:

```text
1. Start Ferese.
2. Top bar appears.
3. Workspace and focused-app state are correct.
4. Press Super+Space.
5. Launcher opens and receives focus.
6. Search for Foot and launch it.
7. Foot enters the current workspace through normal tiling policy.
8. Open Sound from the top bar.
9. Sound popover anchors to its invoking control.
10. Enter fullscreen.
11. Top bar hides without changing the underlying tiled layout region.
12. Reveal top bar temporarily as an overlay.
13. Exit fullscreen.
14. Top bar returns to normal reserved presentation.
15. Restart ferese-shell.
16. Tiled windows do not reflow during the reservation grace period.
```

## 20. Completion criteria

The initial shell is complete when:

1. every active output has a functional top bar;
2. launcher search and application launching work;
3. workspace indicators reflect compositor truth;
4. focused application state is accurate;
5. top-bar popovers anchor correctly;
6. notifications and DND work;
7. OSD works without taking focus;
8. overview uses compositor-owned live windows;
9. fullscreen hide/reveal does not alter underlying layout geometry;
10. shell restart does not disturb managed windows; and
11. all visual presentation is supplied through the theme/UI token system.
