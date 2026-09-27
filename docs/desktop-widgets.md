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

```kdl
desktop-widgets {
    clock enabled=#true anchor="top_left" margin-x=64 margin-y=80 {
        style "pixel"                   // or "minimal"
        outputs                         // every output; or outputs "DP-2"
        width 440
        height 320
        font-family ""                  // style default; or "Comfortaa"
        bold #false
        time-size 128.0
        date-size 18.0
        time-format "%-I:%M %p"          // use "%H:%M" for 24-hour time
        date-format "%a, %b %-d"
        time-zone ""                    // system zone; or "Africa/Lagos"
        show-date #true
        lowercase #true
        color ""                        // theme primary text
        date-color ""                   // theme muted text
        opacity 0.9
        alignment "center"              // left, center, right
        gap 4.0
        padding 12.0
        background ""                   // transparent; or "#10101080"
        radius 16.0
    }
}
```

Positions: `top_left`, `top_center`, `top_right`, `center_left`, `center`,
`center_right`, `bottom_left`, `bottom_center`, `bottom_right`.
Margins affect anchored axes only: a centered horizontal position ignores
`margin_x`; a centered vertical position ignores `margin_y`.
All geometry and text sizes use logical pixels and follow the output's scale.
Increase the widget width/height for longer formats or larger text.

Pixel is the default clock style: heavy hours above minutes, alternating theme colors, and a small plain date above. Its bundled Cantarell display font can be replaced with the font selector. Minimal retains the plain time/date layout. Pixel zero-pads hours and hides am/pm for the stacked display. Formats with seconds or additional text retain their full formatted label on one line. Both styles shrink time text to fit smaller widget dimensions; an explicit text color overrides the alternating colors.

Saving reloads appearance and labels live. Placement, size, output filters and
enable/disable changes recreate only clock surfaces; normal windows and the bar
remain intact. Disconnecting an output destroys its clock; reconnecting creates
one if its name matches the filter. Invalid configuration keeps the last working
configuration. Theme changes apply to colors/fonts that have no explicit override.

Formatting uses [Jiff's strftime-compatible formats](https://docs.rs/jiff/latest/jiff/fmt/strtime/index.html).
Seconds update on the shell's existing timer. Without seconds, unchanged labels
produce no additional clock buffer damage; there is no widget animation loop.
Widget placement can be changed by dragging or through Settings and KDL.
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
Choose each note’s font in Settings using the installed-font dropdown or enter a family name manually. Default font inherits the interface font. In KDL, use `font-family "Comfortaa"` inside the `note` node.

These are plain-text notes, not Markdown or HTML.

```kdl
desktop-widgets {
    note id="today" title="Today" text="Review the pull request\nTake a break\n" anchor="top_right" margin-x=48 margin-y=80
}
```

Repeat the `note` node for additional notes; IDs must be unique. Cards stay behind windows
and take keyboard focus only when clicked. Set `interactive #false` for a fully
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
