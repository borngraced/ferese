# Desktop widgets

Clock and sticky-note widgets sit above the wallpaper and behind windows.
Drag the clock or a note's title to move it. Release to save, or press Escape to
cancel. Widgets stop at each other's edges on the same display.

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
`margin-x`; a centered vertical position ignores `margin-y`.
All geometry and text sizes use logical pixels and follow the output's scale.
Increase the widget width/height for longer formats or larger text.

Choose **Pixel** for stacked hours and minutes with alternating theme colors,
or **Minimal** for a plain time/date layout. Pixel zero-pads hours and hides am/pm.
Formats containing seconds or extra text use a single line. Both styles shrink
time text to fit. Select a different font in Settings, or set `font-family`.

Changes apply live. Colors and fonts follow the theme unless overridden.
Formatting uses [strftime-compatible formats](https://docs.rs/jiff/latest/jiff/fmt/strtime/index.html).
Use `%H:%M` for 24-hour time or `%-I:%M %p` for 12-hour time.

## Clock limits

Clock and note corners follow the shared shell radius. Their `radius` fields
are accepted for compatibility but do not override it.

All clock keys are optional. The example enables the clock (disabled by default);
the other values illustrate defaults or their empty-string equivalents.
`outputs` is an array of strings; `enabled`, `bold`, `show-date`, `lowercase` are
booleans; `width`, `height`, and margins are integers. Other sizes/opacity are numbers.

| Fields | Accepted range |
| --- | --- |
| `width`, `height` | 64–1600, 32–800 |
| `margin-x`, `margin-y` | 0–8192 |
| `time-size`, `date-size` | 8–240, 8–96 |
| `opacity` | 0–1 |
| `gap`, `padding`, `radius` | 0–64, 0–64, 0–128 |
| `font-family`, `time-format`, `date-format` | At most 128 bytes each |
| `outputs` | At most 32 exact output names, each 1–128 bytes |

Formats must be valid; `time-zone` must be empty or a recognized IANA name.
Colors are empty (inherit), `#RRGGBB` or `#RRGGBBAA`; all numeric values must be finite.

## Sticky notes

Settings → Desktop widgets → Add note creates a card. Drag its title to move it;
click Edit for its multiline editor, then Done to finish. Text autosaves after
500 ms without typing. Escape cancels a drag or finishes editing. Position saves
on release; dragging changes the anchor to `top_left` and saves pixel margins.
Title, size and style are also editable in Settings. Notes persist in the config.
Choose a note’s font in Settings or set `font-family "Comfortaa"` in its `note`
node. Without an override, notes use the interface font.

These are plain-text notes, not Markdown or HTML.

```kdl
desktop-widgets {
    note id="today" title="Today" text="Review the pull request\nTake a break\n" anchor="top_right" margin-x=48 margin-y=80
}
```

Repeat the `note` node for additional notes; IDs must be unique. Cards stay behind windows
and take keyboard focus only when clicked. Set `interactive #false` for a fully
click-through card. Long text wraps; increase height if needed.

| Key | Type | Default / accepted values |
| --- | --- | --- |
| `id` | string | `"note"`; unique, 1–64 ASCII letters/digits/`-`/`_` |
| `enabled` | boolean | `true` |
| `interactive` | boolean | `true`; drag/edit controls; `false` is click-through |
| `title` | string | `"Note"`; empty hides it; maximum 256 bytes |
| `text` | string | `""`; multiline plain text, maximum 16 KiB |
| `outputs` | string array | `[]` means all; at most 32 exact names, each 1–128 bytes |
| `anchor` | string | `"top_right"`; same nine positions as clock |
| `margin-x`, `margin-y` | integer | `48`, `80`; 0–8192; anchored edges only |
| `width`, `height` | integer | `320`, `240`; 120–1200, 80–1200 |
| `font-family` | string | omitted/empty follows desktop font; maximum 128 bytes |
| `text-size`, `title-size` | number | `16`, `18`; 8–96 |
| `color` | string | omitted/empty/`"theme"` follows primary text; or hex color |
| `background` | string | omitted/`"theme"` follows surface base; empty is transparent; or hex color |
| `opacity` | number | `0.9`; 0–1, multiplies text/background alpha |
| `padding`, `gap` | number | `20`, `8`; 0–64 |
| `radius` | number | `16`; 0–128 |
| `alignment` | string | `"left"`; `left`, `center`, `right` |

At most 32 notes; the entire live config is limited to 60 KiB.
