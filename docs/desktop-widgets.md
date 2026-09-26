# Desktop widgets

The shell's desktop clock sits above the wallpaper and behind normal windows.
Drag the clock to reposition it; it reserves no workspace space.
All desktop widgets share dragging: grab the clock or a note's title. Release
to save the position, or press Escape to cancel.
Dragging stops at other visible widgets on the same output; edges can slide
along each other. Disabled widgets and widgets on other outputs do not block it.
Each matching output gets a small, transparent surface—not a full-screen canvas.
Lock-screen rendering remains separate and does not expose the desktop clock.

Enable it in Settings → Desktop widgets or in your Ferese config:

```toml
[desktop_widgets.clock]
enabled = true
outputs = []                  # every output; or ["DP-2"] using your own connector
anchor = "top_left"
margin_x = 64                 # distance from anchored left/right edge
margin_y = 80                 # distance from anchored top/bottom edge
width = 440
height = 160
font_family = ""              # empty/omitted follows your desktop font
bold = false
time_size = 72.0
date_size = 18.0
time_format = "%-I:%M %p"     # 12-hour; "%H:%M" for 24-hour, "%H:%M:%S" for seconds
date_format = "%A, %-d %B"
time_zone = ""                # system zone; or e.g. "Africa/Lagos", "UTC"
show_date = true
lowercase = true
color = ""                    # theme primary text; or "#ffffff", "#ffffffcc"
date_color = ""               # theme muted text
opacity = 0.9                 # multiplies text and optional background alpha
alignment = "center"          # left, center, right
gap = 4.0
padding = 12.0
background = ""               # transparent; or e.g. "#10101080" for a solid tint
radius = 16.0                 # optional background corner radius
```

Positions: `top_left`, `top_center`, `top_right`, `center_left`, `center`,
`center_right`, `bottom_left`, `bottom_center`, `bottom_right`.
Margins affect anchored axes only: a centered horizontal position ignores
`margin_x`; a centered vertical position ignores `margin_y`.
All geometry and text sizes use logical pixels and follow the output's scale.
Increase the widget width/height for longer formats or larger text.

Saving reloads appearance and labels live. Placement, size, output filters and
enable/disable changes recreate only clock surfaces; normal windows and the bar
remain intact. Disconnecting an output destroys its clock; reconnecting creates
one if its name matches the filter. Invalid configuration keeps the last working
configuration. Theme changes apply to colors/fonts that have no explicit override.

Formatting uses [Jiff's strftime-compatible formats](https://docs.rs/jiff/latest/jiff/fmt/strtime/index.html).
Seconds update on the shell's existing timer. Without seconds, unchanged labels
produce no additional clock buffer damage; there is no widget animation loop.
Widget placement can be changed by dragging or through Settings and TOML.
Sticky notes also support editing directly on the desktop.

## Clock limits

All clock keys are optional. The example enables the clock (disabled by default);
the other values illustrate defaults or their empty-string equivalents.
`outputs` is an array of strings; `enabled`, `bold`, `show_date`, `lowercase` are
booleans; `width`, `height`, and margins are integers. Other sizes/opacity are numbers.

| Fields | Accepted range |
| --- | --- |
| `width`, `height` | 64–1600, 32–800 |
| `margin_x`, `margin_y` | 0–8192 |
| `time_size`, `date_size` | 8–240, 8–96 |
| `opacity` | 0–1 |
| `gap`, `padding`, `radius` | 0–64, 0–64, 0–128 |
| `font_family`, `time_format`, `date_format` | At most 128 bytes each |
| `outputs` | At most 32 exact output names, each 1–128 bytes |

Formats must be valid; `time_zone` must be empty or a recognized IANA name.
Colors are empty (inherit), `#RRGGBB` or `#RRGGBBAA`; all numeric values must be finite.

## Sticky notes

Settings → Desktop widgets → Add note creates a card. Drag its title to move it;
click Edit for its multiline editor, then Done to finish. Text autosaves after
500 ms without typing. Escape cancels a drag or finishes editing. Position saves
on release; dragging changes the anchor to `top_left` and saves pixel margins.
Title, size and style are also editable in Settings. Notes persist in the config.
These are plain-text notes, not Markdown or HTML.

```toml
[[desktop_widgets.notes]]
id = "today"
title = "Today"
text = """
Review the pull request
Take a break
"""
anchor = "top_right"
margin_x = 48
margin_y = 80
```

Repeat the table for additional notes; IDs must be unique. Cards stay behind windows
and take keyboard focus only when clicked. Set `interactive = false` for a fully
click-through card. Long text wraps; increase height if needed. Notes have no idle
animation loop. Clock and notes share a compact drag preview moved by the
compositor, not an output-sized canvas. It is released when dragging ends.

| Key | Type | Default / accepted values |
| --- | --- | --- |
| `id` | string | `"note"`; unique, 1–64 ASCII letters/digits/`-`/`_` |
| `enabled` | boolean | `true` |
| `interactive` | boolean | `true`; drag/edit controls; `false` is click-through |
| `title` | string | `"Note"`; empty hides it; maximum 256 bytes |
| `text` | string | `""`; multiline plain text, maximum 16 KiB |
| `outputs` | string array | `[]` means all; at most 32 exact names, each 1–128 bytes |
| `anchor` | string | `"top_right"`; same nine positions as clock |
| `margin_x`, `margin_y` | integer | `48`, `80`; 0–8192; anchored edges only |
| `width`, `height` | integer | `320`, `240`; 120–1200, 80–1200 |
| `font_family` | string | omitted/empty follows desktop font; maximum 128 bytes |
| `text_size`, `title_size` | number | `16`, `18`; 8–96 |
| `color` | string | omitted/empty/`"theme"` follows primary text; or hex color |
| `background` | string | omitted/`"theme"` follows surface base; empty is transparent; or hex color |
| `opacity` | number | `0.9`; 0–1, multiplies text/background alpha |
| `padding`, `gap` | number | `20`, `8`; 0–64 |
| `radius` | number | `16`; 0–128 |
| `alignment` | string | `"left"`; `left`, `center`, `right` |

At most 32 notes; the entire live config is limited to 60 KiB.
