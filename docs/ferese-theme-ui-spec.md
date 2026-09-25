# Ferese Theme and UI Specification

Status: Draft for implementation  
Target: Ferese desktop tier  
Normative owner: visual tokens and component geometry

Companion documents:
- `ferese-window-manager-spec.md` — compositor mechanism and policy
- `ferese-shell-design.md` — shell behavior and interaction

## 1. Purpose

This document defines the Ferese visual language.

It owns:

- desktop background treatment;
- color tokens;
- semantic surface tokens;
- typography;
- iconography;
- spacing;
- exact top-bar geometry;
- launcher geometry;
- popover/menu/modal geometry;
- notification geometry;
- OSD geometry;
- settings UI primitives;
- motion tokens; and
- glass, translucent, and solid visual profiles.

It does not define shell focus, fullscreen behavior, workspace policy, launcher
behavior, notification policy, or compositor security.

Ferese intentionally has no persistent dock or bottom application bar.

## 2. Design language

Ferese UI should feel:

```text
quiet
precise
soft
material
low-latency
high-contrast
spatially consistent
```

The shell MUST avoid:

- excessive glow;
- RGB/gamer styling;
- ornamental gradients;
- gratuitous transparency;
- oversized shadows;
- arbitrary component styling;
- large chrome that steals workspace area; and
- pixel-for-pixel imitation of another operating system.

Ferese must still look complete with blur disabled.

## 3. Theme model

Components request semantic surface roles:

```text
surface.panel
surface.panel_elevated
surface.popover
surface.menu
surface.notification
surface.hud
surface.modal
```

The active profile maps each role to material parameters.

Themes change appearance, not component structure or behavior.

## 4. Material styles

```rust
pub enum MaterialStyle {
    Glass,
    Translucent,
    Solid,
}
```

### Glass

Backdrop blur plus restrained tint, saturation, brightness, and optional noise.

### Translucent

Alpha-composited semantic tint with no backdrop blur.

### Solid

Opaque semantic surface with identical geometry.

Changing material style MUST NOT move, resize, or restructure shell components.

## 5. Built-in profiles

```text
ferese-glass
ferese-translucent
ferese-solid
```

All share:

```text
component geometry
spacing
typography
icon metrics
interaction states
motion semantics
```

## 6. Background

Supported:

```text
solid color
image fill
image fit
```

```toml
[theme.background]
path = "/path/to/wallpaper.png"
mode = "fill" # fill or fit
```

Ferese SHOULD NOT globally blur the wallpaper.

Optional restrained adjustments MAY include:

```text
brightness
dimming
saturation
```

Default fallback:

```text
#0B0F14
```

## 7. Color tokens

Representative dark profile:

```toml
[theme.colors]
desktop_background = "#0B0F14"

surface_base = "#111821"
surface_raised = "#18212C"
surface_elevated = "#202B37"

text_primary = "#F4F7FB"
text_secondary = "#B7C0CB"
text_muted = "#7F8A98"
text_disabled = "#58616D"

accent = "#5B8CFF"
accent_soft = "#8EAFFF"
accent_text = "#FFFFFF"

success = "#4CC38A"
warning = "#F0B45B"
danger = "#F06A6A"

border = "#FFFFFF18"
border_strong = "#FFFFFF2A"

shadow = "#00000055"
scrim = "#05070AB8"
```

## 8. Surface tokens

Representative glass profile:

```toml
[theme.surface.panel]
style = "glass"
opacity = 0.78
blur = 24.0
saturation = 1.08
brightness = 1.02
noise = 0.012
border_opacity = 0.10
shadow = "soft"

[theme.surface.panel_elevated]
style = "glass"
opacity = 0.84
blur = 28.0
saturation = 1.10
brightness = 1.03
noise = 0.014
border_opacity = 0.12
shadow = "elevated"

[theme.surface.popover]
style = "glass"
opacity = 0.84
blur = 28.0
saturation = 1.10
brightness = 1.03
noise = 0.014
border_opacity = 0.12
shadow = "elevated"

[theme.surface.hud]
style = "glass"
opacity = 0.88
blur = 30.0
saturation = 1.06
brightness = 1.04
noise = 0.010
border_opacity = 0.12
shadow = "elevated"
```

## 9. Geometry tokens

```toml
[theme.geometry]
base_unit = 8.0

window_radius = 14.0

top_bar_height = 32.0
top_bar_margin_top = 4.0
top_bar_margin_horizontal = 10.0
top_bar_radius = 16.0

launcher_width = 560.0
launcher_radius = 20.0

popover_radius = 16.0
menu_radius = 14.0
modal_radius = 18.0
hud_radius = 18.0

panel_padding = 12.0
control_gap = 8.0
section_gap = 16.0
```

Only the top bar consumes persistent shell space.

## 10. Typography

```toml
[theme.typography]
font_family = "sans-serif"
```

Initial roles:

```text
caption       11-12 px
body          13-14 px
body-strong   13-14 px medium
label         12-13 px medium
title         16-18 px semibold
display       22-28 px semibold
```

Use the system UI sans-serif by default.

The configured family applies consistently to shell text. Icon assets use the
theme icon set and are sized from the same scale tokens rather than text glyph
metrics.

Clocks and changing numeric values SHOULD use tabular numerals where available.

## 11. Iconography

System icons are monochrome vectors.

```text
visual box      16-20 px
stroke          approximately 1.5-2 px
style           simple and optically balanced
```

Application icons retain original artwork.

System icons MUST NOT use arbitrary gradients.

## 12. Top bar

Default composition:

```text
╭──────────────────────────────────────────────────────────────╮
│ F   1  2  3   4      Zed              09:41        🔊   ◉  │
╰──────────────────────────────────────────────────────────────╯
```

Surface role:

```text
surface.panel
```

### Workspace state

Inactive workspaces use muted text.

Active workspace uses a compact filled or accent-backed indicator.

### Focused application

May display:

```text
application icon
application name
```

Full changing window titles are not part of the default bar.

### Clock

Compact:

```text
09:41
```

Optional wide:

```text
Mon, Sep 23   09:41
```

## 13. Launcher

The launcher is the primary application-selection surface.

Default placement:

```text
centered horizontally
upper-middle or center of usable workspace
```

Surface role:

```text
surface.panel_elevated
```

Default geometry:

```text
width       theme.geometry.launcher_width
max height  70% of usable output
radius      theme.geometry.launcher_radius
```

Initial content:

```text
╭──────────────────────────────────────╮
│ Search apps…                         │
├──────────────────────────────────────┤
│                                      │
│  Zed      Firefox      Foot          │
│  Files    Settings     Music         │
│                                      │
╰──────────────────────────────────────╯
```

Initial implementation is apps-only.

## 14. Launcher search field

```text
height      44-48 px
radius      12 px
```

Placeholder:

```text
Search apps…
```

The search field is the visual focus.

## 15. Launcher application grid

Recommended:

```text
4-6 columns depending on launcher width
```

Entries show:

```text
application icon
one-line application name
```

Long names truncate with ellipsis.

## 16. Top-bar popovers

Surface:

```text
surface.popover
```

Recommended:

```text
width            280-360 px
padding          12 px
radius           theme.geometry.popover_radius
anchor gap       8 px
```

They visually align with the invoking top-bar control.

## 17. Control center

Representative:

```text
╭──────────────────────────────╮
│ Sound                        │
│ ━━━━━━━━━━━●━━━━━━          │
│                              │
│ Battery              92%     │
│ Do Not Disturb        Off    │
│ Notifications          3     │
│                              │
│ Ferese Settings              │
╰──────────────────────────────╯
```

Network/Bluetooth rows are not part of the initial visual contract.

## 18. Menus

Menus use:

```text
surface.menu
```

Row:

```text
icon   label                         shortcut
```

Disabled items use `text_disabled`.

## 19. Modals

Modals use:

```text
surface.modal
```

Default width:

```text
360-480 px
```

Background scrim uses `theme.colors.scrim`.

Modal visuals do not imply a Ferese authentication agent.

## 20. Notifications

Toast surface:

```text
surface.notification
```

Recommended width:

```text
320-360 px
```

Structure:

```text
icon
summary
body
time
optional actions
```

Newest appears nearest the top bar.

## 21. OSD

Surface:

```text
surface.hud
```

Example:

```text
╭──────────────────────╮
│ 🔊  ━━━━━━━●━━━━━   │
╰──────────────────────╯
```

Default placement is centered or lower-middle.

## 22. Overview chrome

Managed windows remain live compositor surfaces.

Theme/UI owns only:

```text
selection ring
labels
workspace strip
scrim
overview control surfaces
```

Selected preview uses an accent ring, not a large glow.

## 23. Interaction states

Controls support:

```text
rest
hover
pressed
focused
selected
disabled
urgent where applicable
```

Hover changes color/contrast only.

Pressed controls MAY use a tiny local scale response around `0.98`.

There is no dock-style magnification behavior.

## 24. Focus rings

Keyboard focus:

```text
2 px accent ring
2 px visual separation where needed
```

Focus rings MUST NOT alter component geometry.

## 25. Shadows

Levels:

```text
none
soft
elevated
```

Representative:

```toml
[theme.shadow.soft]
offset_y = 4.0
blur = 18.0
opacity = 0.20

[theme.shadow.elevated]
offset_y = 10.0
blur = 36.0
opacity = 0.28
```

Top bar uses `soft`.

Launcher, popovers, notifications, modals, and HUDs typically use `elevated`.

## 26. Borders

Default:

```text
1 logical px
```

Glass uses subtle highlight borders.

Solid may use slightly stronger borders.

Borders MUST NOT resemble neon outlines.

## 27. Motion tokens

```text
spatial spring
bounded easing
```

Approximate ranges:

```text
micro feedback      80-120 ms
popover/menu        140-200 ms
launcher            180-240 ms
notification        160-220 ms
```

Managed-window and overview motion remains compositor spring-driven.

## 28. Reduced motion

Reduced motion:

```text
removes decorative scale
shortens/removes large shell translation
uses opacity where spatial context is unnecessary
does not animate blur intensity
```

## 29. Fullscreen visual presentation

When fullscreen is active, the top bar is visually absent.

Temporary reveal uses the same visual tokens without implying any layout change.

## 30. Settings application

Ferese Settings consumes the same tokens.

Representative layout:

```text
╭──────────────────────────────────────────────╮
│ sidebar         │ content                    │
│                 │                            │
│ General         │ Appearance                 │
│ Appearance      │                            │
│ Tiling          │ live preview               │
│ Workspaces      │ controls                   │
│ Shortcuts       │                            │
│ Input           │                            │
╰──────────────────────────────────────────────╯
```

## 31. Appearance settings

Expose:

```text
material style
accent
corner radius
shadow intensity
glass blur
glass opacity
animation speed
reduced motion
```

Material style:

```text
Glass
Translucent
Solid
```

Changing style MUST preserve geometry.

## 32. Theme configuration

```toml
[theme]
profile = "ferese-glass"
mode = "dark"

[theme.colors]
accent = "#5B8CFF"

[theme.material]
style = "glass"

[theme.material.glass]
blur = 24.0
opacity = 0.80
saturation = 1.08
noise = 0.012

[theme.geometry]
window_radius = 14.0
top_bar_height = 32.0
top_bar_radius = 16.0
launcher_width = 560.0
launcher_radius = 20.0

[theme.motion]
speed = 1.0
reduced_motion = false
```

## 33. Validation

Reject:

```text
non-finite dimensions
negative radii
negative blur
opacity outside 0..1
invalid colors
unknown material style
invalid font sizes
invalid motion factors
```

Theme changes use the same transactional Ferese configuration path.

## 34. Custom theme scope

Themes MAY change:

```text
colors
material parameters
radii
shadows
typography
accent
icon tint
motion scale
validated component geometry tokens
```

Themes MUST NOT change:

```text
top-bar behavior
launcher behavior
focus behavior
workspace behavior
notification policy
Wayland roles
security behavior
```

Ferese intentionally does not expose arbitrary CSS.

## 35. Performance degradation

Priority:

```text
1. geometry correctness
2. input latency
3. text/icon clarity
4. animation continuity
5. shadow quality
6. blur quality
7. decorative effects
```

Allowed degradation:

```text
glass
    -> lower-resolution glass
    -> translucent
    -> solid
```

Component geometry remains unchanged.

## 36. Visual acceptance criteria

Implementation succeeds when:

1. top bar, launcher, popovers, notifications, OSD, overview chrome, and Settings
   share one spacing/material/typography system;
2. glass, translucent, and solid profiles preserve geometry;
3. typography remains readable on bright and dark backgrounds;
4. icons have consistent optical weight;
5. launcher and popovers look anchored and intentional;
6. focus/selection/urgent states remain distinct without relying on glow;
7. disabling blur does not make the desktop look unfinished;
8. reduced-motion mode remains coherent; and
9. the desktop feels complete without a persistent dock or bottom application bar.
