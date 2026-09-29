# Configuration

Edit `~/.config/ferese/config.kdl` (or `$XDG_CONFIG_HOME/ferese/config.kdl`),
or open **Control Center → Settings**. Start with the [example](../packaging/config.kdl).
All supported configuration sections are listed below; [desktop widgets](desktop-widgets.md)
covers the complete clock and sticky-note fields separately.

## KDL syntax

Sections use nested braces, values follow their names, and booleans are `#true`
or `#false`. Multiple values represent arrays. Hyphenated keys are preferred;
underscore spellings remain accepted. Enum values retain their documented spelling.
A newline or semicolon ends each node. Comments use `//` or `/* ... */`.
For example:

```kdl
input {
    repeat-rate 30
    touchpad {
        swipe-threshold 80
    }
}

commands {
    terminal "foot"
}

binding "Super+Enter" "spawn" "terminal"
binding "Swipe3Up" "toggle-overview"
binding "Swipe3Down" disabled=#true
output-profile "docked" {
    output "HDMI-A-1" {
        scale 1.5
        position 0 0
    }
}
```

Repeated `binding`, `window-rule`, `output-profile`, `output`, `note` and
`autostart` nodes describe lists of settings. Bindings accept positional
keys/action/argument; profiles, outputs and notes accept a positional name,
match or ID. Other fields can be properties or child nodes. Autostart accepts
its command as positional arguments. Duplicate fields are rejected.
The dotted paths in the reference tables below describe nested sections.

Settings preserves comments and custom fields when editing, while normalizing
indentation. Runtime configuration uses KDL only. KDL parsing requires Rust 1.95
or newer to build.

## Saving and validation

Settings saves automatically: text on Enter/focus loss, sliders when released.
Undo restores the previous save; Reload picks up external edits without merging
unfinished drafts. Wallpaper browsing needs `zenity`; a path can also be entered.
Settings keeps `config.kdl.settings-backup` before saving.

File edits reload automatically, including atomic editor saves. Invalid changes
keep the last working configuration. Appearance, wallpaper, motion, input,
bindings, rules, layouts, displays, widgets and login items update live.

```sh
feresectl reload-config          # reload with error feedback
ferese --check-config PATH       # validate without applying
ferese-settings --config PATH   # edit a separate preview config
```

Defaults below are built-in defaults, not your personal or packaged overrides.
Dimensions are logical pixels unless noted; numbers must be finite. Optional
values are omitted, not `null`. Colors use `#RRGGBB` or `#RRGGBBAA`.
Live config is limited to 60 KiB.

## Layout

| `layout` key | Type / values | Default | Meaning |
| --- | --- | --- | --- |
| `mode` | `"scrolling"`, `"tree"` | `"scrolling"` | Workspace layout |
| `inner_gap` | number ≥ 0 | `10` | Between windows |
| `outer_gap` | number ≥ 0 | `4` | Around workspace edges |
| `smart_gaps` | boolean | `false` | Remove outer gaps for one tiled window |

| `scrolling` key | Type / values | Default | Meaning |
| --- | --- | --- | --- |
| `default_column_width` | proportion > 0; or `"full"` | `0.5` | Width of new columns |
| `focus_strategy` | `"minimal"`, `"center_on_focus"`, `"paged"` | `"minimal"` | Viewport movement on focus |
| `width_presets` | array of widths | `[0.3333333333333333, 0.5, 0.6666666666666666, "full"]` | Super+R cycle; empty uses defaults |

`minimal` scrolls only to reveal the focused window. `center_on_focus` centers
it. `paged` packs columns into viewport-sized pages: halves form pairs, thirds
form triples; mixed widths and client minimum sizes determine actual boundaries.
Changing default width preserves manually resized columns.

## Motion

| Section / key | Type | Default | Meaning |
| --- | --- | --- | --- |
| `animations.enabled` | boolean | `true` | Enable motion |
| `animations.reduced_motion` | boolean | `false` | Disable animated motion |
| `animations.speed` | number > 0 | `1` | Higher is faster; `0.75` is slower |
| `animations.spring.mass` | number > 0 | `1` | Window-motion spring mass |
| `animations.spring.stiffness` | number > 0 | `700` | Spring stiffness |
| `animations.spring.damping` | number ≥ 0 | `53` | Spring damping |
| `animations.viewport_spring.mass` | number > 0 | `1` | Scrolling spring mass |
| `animations.viewport_spring.stiffness` | number > 0 | `320` | Scrolling stiffness |
| `animations.viewport_spring.damping_ratio` | number > 0 | `1` | `1` is critically damped |

## Appearance

Settings → Appearance offers six presets: **Ferese Blue** (default),
**Monochrome**, **Gruvbox**, **Dracula**, **Ayu Light**, and **Monokai**.
The two-row gallery previews each palette on a miniature desktop; a checkmark
identifies the active preset.
Ferese Blue uses the README logo's `#3D7BE6`. Each preset coordinates surfaces,
text, menu-bar colors, borders and the focus ring. Ayu Light also switches native
Settings controls to light styling. Presets preserve wallpaper, typography,
geometry and opacity settings; colors remain individually editable.

The community palettes are desktop adaptations with text shades adjusted for
readability. Sources: [Gruvbox](https://github.com/morhetz/gruvbox),
[Dracula](https://github.com/dracula/visual-studio-code),
[Ayu](https://github.com/ayu-theme/ayu-colors), and
[classic Monokai](https://github.com/microsoft/vscode/blob/main/extensions/theme-monokai/themes/monokai-color-theme.json).
Marketplace install counts informed the shortlist; they are not a measurement
of desktop users. Monochrome and Gruvbox replace similar blue palettes to keep
these six visually distinct.


| Section / key | Type | Default | Meaning |
| --- | --- | --- | --- |
| `appearance.corner_radius` | number ≥ 0 | unset | Legacy fallback for shell radius |
| `appearance.inactive_dim.enabled` | boolean | `false` | Dim unfocused windows |
| `appearance.inactive_dim.amount` | number 0–1 | `0.15` | Darkening strength |
| `appearance.inactive_dim.duration_ms` | number ≥ 0 | `150` | Dimming and focus-ring transition; 0 snaps |
| `theme.typography.font_family` | string | system sans-serif | Shell, Settings and overview font |
| `theme.background.path` | string | bundled Ferese wallpaper | Wallpaper image path; an existing selection overrides the default |
| `theme.background.mode` | `"fill"`, `"fit"` | `"fill"` | Crop or letterbox |
| `theme.material.style` | `"solid"`, `"translucent"` | `"solid"` | Shell background material |
| `theme.material.opacity` | number 0–1 | `0.78` | Shared shell background opacity for bars, menus, popovers, notifications and the sharing dialog. Text/icons stay opaque; 0 and 1 skip blur. Solid mode is always opaque |
| `theme.material.blur_radius` | number ≥ 0 | `12` | Translucent backdrop blur, capped at 32; 0 disables |

| `theme.colors` key | Default | Meaning |
| --- | --- | --- |
| `surface_base` | `"#111821"` | Material/card base color |
| `text_primary` | `"#F4F7FB"` | Main text |
| `text_muted` | `"#8793A2"` | Secondary text |
| `accent` | `"#3D7BE6"` | Active controls and solid focus ring |
| `border` | `"#FFFFFF18"` | Unfocused window border |
| `shadow` | `"#00000055"` | Window shadow color |

Ferese derives the text and icon color for filled accent controls from the
configured theme. It keeps the configured text color and, when necessary, shades
the accent fill slightly to reach a 4.5:1 contrast ratio. If that would require a
large color change, it chooses black or white instead. The same rule is shared by
native dialogs, settings selections, and notification badges. Hover, pressed,
and disabled accent-button states use the same policy. Small filled controls
resolve accent transparency against their theme surface so wallpaper cannot
make their labels unreadable. No separate per-application color setting is needed.
The target follows [normal-text contrast guidance](https://www.w3.org/WAI/WCAG22/Understanding/contrast-minimum.html).

| `theme.geometry` key | Type | Default | Meaning |
| --- | --- | --- | --- |
| `border_width` | number ≥ 0 | `1` | Window border thickness |
| `focus_ring_width` | number ≥ 0 | `2` | Focused border thickness |
| `window_radius` | number ≥ 0 | `14` | Managed-window corners, independent of the shell |
| `shell_radius` | number ≥ 0 | `14` | All shell surfaces, cards, widgets and interaction backgrounds; 0 makes them square |
| `top_bar_height` | number > 0 | `28` | Menu-bar height |
| `top_bar_margin_top` | integer ≥ 0 | `0` | Space above bar |
| `top_bar_window_gap` | integer ≥ 0 | `0` | Clearance below bar |
| `top_bar_margin_horizontal` | integer ≥ 0 | `0` | Bar side margins |
| `top_bar_radius` | number ≥ 0 | unset | Legacy fallback for shell radius |
| `panel_padding` | number ≥ 0 | `12` | Bar inner padding |
| `control_gap` | number ≥ 0 | `12` | Right-side control spacing |

| Section / key | Type | Default | Meaning |
| --- | --- | --- | --- |
| `theme.surface.bar.background` | color | `"#1C202EF2"` | Shell fallback background; compositor materials use `surface_base` |
| `theme.surface.bar.text_primary` | color | `"#F0F3FA"` | Bar text/icons |
| `theme.surface.bar.text_muted` | color | `"#AAB4C7"` | Inactive bar foreground |
| `theme.shadow.soft.offset_y` | number | `4` | Window shadow vertical offset |
| `theme.shadow.soft.blur` | number ≥ 0 | `18` | Window shadow softness |
| `theme.shadow.soft.opacity` | number 0–1 | `0.20` | Window shadow strength |

Solid materials omit blur; translucent materials blur behind the surface color.
True fullscreen removes decorations. Focus transitions never scale the content.

Optional `theme.focus_ring.gradient` and `theme.border.gradient` use the same
keys: required `from` and `to` colors, and `angle` (number, default `0`). Angles
are clockwise: 0 is left-to-right, 90 top-to-bottom. Omit the gradient section
for solid `accent` or `border` colors.

```kdl
theme {
    focus-ring {
        gradient {
            from "#e5c890"
            to "#b98d58"
            angle 135.0
        }
    }
}
```

## Input

| `input` key | Type | Default | Meaning |
| --- | --- | --- | --- |
| `focus_follows_mouse` | boolean | `false` | Focus visible windows under the pointer without raising them or scrolling; click or use keyboard focus to reveal a window |
| `xkb_layout` | nonempty string | `"us"` | XKB layout |
| `xkb_variant` | string | `""` | XKB variant |
| `xkb_options` | string array | `[]` | XKB options |
| `repeat_rate` | integer > 0 | `25` | Repeats per second |
| `repeat_delay_ms` | integer ≥ 0 | `600` | Delay before repeat |

With focus-follows-mouse enabled, hovering a visible part of a window changes
keyboard focus without raising it or moving the scrolling viewport. Clicking or
using directional keyboard focus brings the selected window into view.

In Overview (`Super+Tab`), click a window preview or a miniature inside a workspace
card to activate that exact window. Hover highlights the target; directional
focus keys move the selection, Enter activates it, and Escape dismisses Overview.
Clicking a workspace card's background switches workspaces while keeping Overview
open. Previews follow layout order and use the configured shell font and colors.

The touchpad device keys are booleans, all defaulting to `true`: `tap`,
`natural_scroll` and `disable_while_typing`. These apply to DRM devices; nested
previews use the host's physical input settings.

Three-finger swipes navigate on release: up goes to the next workspace on the
current monitor, down to the previous; left focuses the window to the right,
and right focuses the window to the left. Workspace swipes skip workspaces owned
by other monitors and stop at the first/last workspace. Cancelled, short and
diagonal swipes do nothing. Navigation pauses while locked, during
window grabs, or when an application inhibits shortcuts.

Assign swipes in **Settings → Shortcuts** using the same actions and arguments as
keyboard bindings. Choose a "Customize swipe" button to add an override. The
gesture keys are `Swipe3Up`, `Swipe3Down`, `Swipe3Left`, and `Swipe3Right`; 4 or 5
fingers are also supported. Changes reload live:

```kdl
binding keys="Swipe3Up" action="toggle-overview"
binding keys="Swipe3Left" action="move" argument="left"
binding keys="Swipe3Down" disabled=#true
```

`spawn` uses a named entry from `commands`, just like keyboard shortcuts. To
disable a direction in Settings, set its action to `none` and leave Argument
empty. In KDL, `disabled=#true` removes the default binding. Swipes do not use
keyboard modifiers or physical key matching. The trigger is automatically
recognized from its name. Gesture navigation uses native touchpad events in a
hardware session.

Adjust recognition distance in **Settings → Keyboard & mouse**, or with
`swipe-threshold 80` under `input.touchpad` (integer 16–1000 logical pixels).

## Commands and bindings

`commands` maps arbitrary names to nonempty argument arrays. The built-in
`terminal "foot"` can be overridden. Commands run directly, without a shell.

| `binding` key | Type / values | Default |
| --- | --- | --- |
| `keys` | chord or gesture, e.g. `"Super+Enter"`, `"Swipe3Up"` | required |
| `match` | `"keysym"`, `"physical"` | `"keysym"` |
| `action` | action name below | required unless disabled |
| `argument` | string | required only for actions listed below |
| `disabled` | boolean | `false` |

Modifiers: `Super`/`Logo`/`Mod4`, `Ctrl`/`Control`, `Alt`, `Shift`.
Keys use XKB keysyms (`Enter`, letters, `[`/`]`) or physical XKB names (`AD06`).
A binding replaces the same normalized chord/match mode. Duplicate bindings are
invalid. A disabled binding must omit `action` and `argument`.

| Actions | Argument |
| --- | --- |
| `spawn` | Name in `commands` |
| `focus`, `move`, `resize` | `"left"`, `"right"`, `"up"`, `"down"` |
| `workspace`, `move-to-workspace` | Workspace number string, 1–255 |
| `workspace-next`, `workspace-previous` | None; next/previous workspace on this monitor |
| `none` | None; ignore this trigger |
| `close`, `exit`, `toggle-maximized`, `toggle-fullscreen`, `toggle-layout`, `cycle-column-width`, `center-column`, `consume`, `expel`, `toggle-floating`, `toggle-overview` | None |

```kdl
commands {
    terminal "foot"
}
binding keys="Super+Enter" action="spawn" argument="terminal"
```

Defaults: Super+Enter terminal; Super+Q close; Super+H/J/K/L focus;
Super+Shift+H/J/K/L move; Super+Ctrl+H/J/K/L resize; Super+1–9 workspace;
Super+Shift+1–9 move to workspace; Super+R width cycle; Super+C center;
Super+[/] consume/expel; Super+F maximize; Super+Shift+F fullscreen;
Super+M layout; Super+Shift+Space floating; Super+Tab overview;
Super+Shift+S area screenshot; Print Screen whole-screen screenshot;
Super+Shift+E immediate logout.

Hold Super and drag a floating window with the left mouse button to move it.
Hold Super and drag with the right mouse button to resize it from the nearest
corner. These work even when an app has no title bar or resize border.

The screenshot shortcut runs `ferese-screenshot`: drag to select an area, or
press Escape to cancel. Captures open in Satty for annotation. Press Enter to
save the edited PNG under your Pictures directory in `Screenshots` and copy it
to the clipboard; Escape discards the capture. It requires `slurp`, `satty`, and
`wl-copy` (from `wl-clipboard`); the capture itself is done by the compositor
through `feresectl screenshot`. Override the `screenshot` command to use another
screenshot tool.
Print Screen runs `ferese-screenshot --full` and captures the active monitor
without a selector, falling back to the sole enabled output when focus is
unavailable. It never silently captures every output. Fn+PrtSc works when the
keyboard emits the Print Screen key; Fn is handled by the keyboard firmware.
Override `screenshot-full` to customize this command. Both modes open Satty with
the same save and copy workflow.

`ferese-screenshot --all` captures every enabled output as one image.

The editor window is sized from the captured image rather than the output, so a
small area selection does not open a large window. Sizing is advisory: the
compositor floats the editor at whatever size Satty requests, clamped to the
workspace.

A capture is assembled from the readback each output produces when it is next
redrawn, so a request is abandoned if an output has not been redrawn within 30
seconds. That is reported as an error rather than a partial or empty image, and
the request holds no resources once it is abandoned. Nothing is left in the
runtime directory: `feresctl screenshot` writes a private file that the caller
opens and unlinks, and any file a dead client leaves behind is reclaimed after
an hour.


## Window rules

Native authentication and display-sharing dialogs float by default. Explicit
window rules can override that behavior.

| `window-rule` key | Type | Default / meaning |
| --- | --- | --- |
| `app_id` | nonempty string | Optional exact match; case-insensitive, `.desktop` suffix ignored |
| `title` | nonempty string | Optional exact, case-sensitive match |
| `transient` | boolean | Optional parent-dialog match |
| `workspace` | integer > 0 | Leave placement unchanged |
| `floating` | boolean | Leave placement unchanged |
| `width`, `height` | numbers > 0 | Application-chosen floating size |
| `fullscreen` | boolean | Leave fullscreen state unchanged |

At least one matcher is required; supplied matchers must all match. Rules apply
in order, with later fields overriding earlier ones. Dimensions imply floating
when `floating` is omitted. Floating apps without dimensions choose their own
size and open centered on the output. Title changes do not trigger new rules;
config rule changes apply to existing windows.

```kdl
window-rule app-id="dev.ferese.Settings" floating=#true
```

## Displays

`output-profile` has a required unique nonempty `name` and one or more
`output-profile → output` entries. The first profile whose listed monitors
are connected wins. Find connector names and persistent identities using
`feresectl get-outputs`.

| `output-profile → output` key | Type | Default |
| --- | --- | --- |
| `match` | nonempty connector or persistent identity string | required; unique within profile |
| `enabled` | boolean | `true` |
| `mode` | `"WIDTHxHEIGHT"` or `"WIDTHxHEIGHT@HZ"` | Preferred mode |
| `scale` | number > 0 | `1` |
| `transform` | enum below | `"normal"` |
| `position` | `[integer, integer]` | Automatic horizontal placement |

Transforms: `normal`, `rotate_90`, `rotate_180`, `rotate_270`, `flipped`,
`flipped_90`, `flipped_180`, `flipped_270`. Unspecified connected displays stay
enabled with defaults. Disabling every usable output is rejected.

```kdl
output-profile name="docked" {
    output match="HDMI-A-1" scale=1.5 {
        position 0 0
    }
}
```

With a usable external display, closing the lid disables internal panels.
Workspaces move to a remaining display and return when their original output
returns. Ferese does not change the system's suspend policy.

## Status controls

| `status` key | Type | Default | Meaning |
| --- | --- | --- | --- |
| `window_title` | boolean | `true` | Focused window title in the bar center when space allows |
| `battery_percentage` | boolean | `true` | Show percentage beside icon |
| `low_battery_threshold` | integer 0–100 | `20` | Warning-color threshold |
| `settings_command` | argument array | `["ferese-settings"]` | Settings launcher; `[]` hides the action |

## Login items and locking

| `autostart` key | Type | Default | Meaning |
| --- | --- | --- | --- |
| `command` | nonempty argument array | required | Foreground process, not a self-daemonizing command |
| `enabled` | boolean | `true` | Start service |
| `restart` | boolean | `true` | Restart after exit, with a five-second retry delay |
| `nested` | boolean | `false` | Also run in nested previews |

Unchanged services keep running on reload; removed/disabled items stop. To lock
after five minutes and before sleep:

```kdl
autostart {
    command "swayidle" "-w" "timeout" "300" "ferese-lock" "before-sleep" "ferese-lock" "lock" "ferese-lock"
}
```

`ferese-lock` is the native PAM-authenticated locker. It follows your wallpaper,
colors, font and shell corner radius. Its command returns successfully only after
the compositor confirms the lock; the UI keeps running until authentication
succeeds. `--foreground` keeps the command attached to its UI process.

Use `ferese-lock --preview` for an ordinary window that never locks or checks a
password. See [Native locker](locking.md) for setup and verification. Test real
unlock in a nested compositor before enabling automatic locking. A crashed
locker leaves the session locked; recovery requires ending that session from
another TTY.

## Optional session protocols

These are launch-time environment switches, not KDL keys. Enable only for
trusted clients: `FERESE_ENABLE_INPUT_METHOD=1`,
`FERESE_ENABLE_SHORTCUT_INHIBIT=1`, `FERESE_ENABLE_SCREENCOPY=1`.
The installed session launcher enables screencopy for screenshots unless
`FERESE_ENABLE_SCREENCOPY=0` is explicitly set in its environment. Setting it
to `0` disables every path that can read the screen, including the built-in
screenshot command, not just the Wayland protocol. A session launched by hand
rather than through `ferese-session` needs `FERESE_ENABLE_SCREENCOPY=1` in its
environment for screenshots to work.
Text input is available by default; Ctrl+Alt+Escape releases an active shortcut
inhibitor. Use `feresectl --help` for runtime control commands.

## Notifications

Settings → Notifications controls popups, Do Not Disturb, and the default timeout. See [Notifications](notifications.md) for KDL options and app behavior.

A single-output session starts with workspace 1. Numbered workspace shortcuts create
workspaces on demand; switching forward past the last workspace also creates the
next one when the current workspace contains windows. Empty workspaces are
cleaned up automatically, keeping at most one empty workspace in addition to
any empty workspace currently shown on another display. Additional displays
receive their own workspace.

Shell rounding is controlled in Settings → Appearance → Shell corner radius.
Small controls cap the radius to fit their size. Window rounding remains under
Settings → Windows. `theme.geometry.shell_radius` takes precedence over the old
`appearance.corner_radius` and then `theme.geometry.top_bar_radius` keys; when
none are set, the shell uses 14 px. Legacy clock/note radius fields no longer
override shell rounding.
