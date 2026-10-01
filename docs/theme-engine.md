# Themes

The compositor resolves theme files and publishes a versioned snapshot. Native
apps consume that snapshot; Settings edits references and requests isolated previews.

## Preset pairs

| Family | Dark preset | Light preset |
| --- | --- | --- |
| Ferese Blue (default) | `ferese-blue` | `ferese-blue-light` |
| Catppuccin | `catppuccin-mocha` | `catppuccin-latte` |
| Gruvbox | `gruvbox` | `gruvbox-light` |
| Rosé Pine | `rose-pine-moon` | `rose-pine-dawn` |
| Tokyo Night | `tokyo-night` | `tokyo-night-day` |
| Everforest | `everforest-dark` | `everforest-light` |

Each variant supplies its own surfaces, tint strength, opacity, blur, borders,
and shadows. Shared material overrides apply to both appearances; appearance
specific overrides take precedence. [Palette sources and adjustments](theme-palette-sources.md)
record the upstream colors and contrast tuning.

## Configuration

```kdl
theme {
    mode "auto"
    family "catppuccin"
    split #false
    file "themes/custom_theme.kdl"
    light {
        preset "ferese-blue-light"
        file "themes/custom_theme_light.kdl"
        material { opacity 0.94; tint-strength 1; }
    }
    dark { preset "ferese-blue"; file "themes/custom_theme_dark.kdl"; }
    schedule { source "schedule"; timezone "system"; light-at "07:00"; dark-at "19:00"; }
    accessibility { increase-contrast #false; reduce-transparency #false; }
    accent "#3D7BE6"
}
```

Paths are relative to the configuration directory. Partial theme files contain
`theme { colors { accent "#3D7BE6"; } }`, or the token sections directly.
Merge order is preset, shared file, appearance file, shared inline tokens,
appearance inline tokens, then accent and accessibility adjustments. Unknown tokens warn; invalid known values reject
an entire update and retain the last working snapshot.

## Snapshot

`ResolvedTheme` contains effective appearance and typed colors, surfaces, material,
geometry, typography, wallpaper, border paints, and shadow tokens. Base, raised,
and application backgrounds are separate colors. Selected mode,
accessibility inputs, requested accent, adjusted accent, warnings, schema version,
and revision accompany it. Consumers do not infer appearance from brightness.

Material tokens differ by appearance. Light translucent surfaces use full tint strength and
0.94 default opacity to stay pale over dark wallpapers; dark surfaces use a softer
0.5 tint. Shared material overrides apply to both unless the appearance overrides
them. Reduce Transparency resolves to solid
material with zero blur. Increase Contrast adjusts text, borders, and translucent
surface tint for 7:1 text contrast, and enables native high-contrast controls.
Accents are adjusted against surfaces independently for each appearance, then
on-accent text is selected for readability.

## Scheduling

Auto selects the most recent light or dark boundary in the configured timezone.
The interval is circular, so light may start after dark. Equal boundaries are
invalid. Repeated hours use their first occurrence; skipped hours shift forward
by the timezone gap. The next transition arms a realtime timer. Resume, timezone
replacement, and realtime clock jumps trigger recalculation without polling.

## Transitions

Prepare the incoming wallpaper before starting a 250 ms eased transition. Decode
or upload failures and a bounded readiness timeout retain the old wallpaper and
allow the theme change. Reduced motion and accessibility changes apply immediately.
Transitions preserve their exact resolved endpoints. Changes within one appearance
keep the resolved material settings instead of adding temporary tint corrections.

Surface, tint, border, shadow, and accent colors can transition. Fonts and geometry
switch atomically. Foregrounds remain readable across dark and light backdrops. Near the neutral
midpoint, the transition temporarily strengthens material tint instead of fading
text through the background color. Tests sample primary, muted,
and on-accent text throughout the transition. Wallpaper changes use prepared
images rather than a placeholder.

Publish effective appearance at transition start. The portal emits changed
`color-scheme`, `accent-color`, `contrast`, and `reduced-motion` values then.

## Commands

| Command | Action |
| --- | --- |
| `feresectl theme get` | Read the current mode, effective appearance, and colors |
| `feresectl theme status` | Read warnings and the last reload error |
| `feresectl theme subscribe` | Stream changes as JSON; no polling |
| `feresectl theme mode light` | Save Light mode; also accepts `dark` and `auto` |
| `feresectl theme preview PATH` | Resolve a candidate config without applying it |

Settings shows six paired previews, with Light on the left and Dark on the right.
Auto uses a switch in the page header. The adjacent button shows its source and,
when enabled, the effective appearance. It opens source and schedule controls
below the gallery, which can be edited before enabling Auto. Enabling Auto
immediately follows the chosen source; disabling it keeps the current appearance.
Manual appearance controls are in Control Center. Selecting a family
applies the matching variant live and offers Undo for five
seconds. Hovering does not change the desktop. Arrow keys navigate the gallery.
Enable **Use different themes for light and dark** for two independent galleries.
Disabling it uses the active family and retains the stored pair. A missing
variant uses Ferese Blue for that appearance and displays a fallback note.

**Follow system** reads `org.gnome.desktop.interface color-scheme` and monitors
changes. `default` and `prefer-light` mean Light; `prefer-dark` means Dark. If the
setting is unavailable, Ferese uses Dark and displays a note. This source does
not read Ferese's own appearance portal, avoiding a feedback loop. Sunrise/sunset
is deferred.

The gallery’s Import button adds a family to the gallery without selecting it. Give each
variant an explicit section; omitted tokens inherit Ferese Blue for that
appearance. For example:

```kdl
theme {
    name "My theme"
    light { colors { accent "#8F5300"; }; }
    dark { colors { accent "#CBA6F7"; }; }
}
```

Settings stores the file reference under `theme.custom-themes.<id>.file`.
The compositor validates it and publishes its palettes alongside built-ins.
Omit a variant to make a single-variant family. Token-only partial files remain
supported as shared/per-appearance overrides; imports require explicit variants
so appearance is never guessed from brightness. Imported and override files
reload after a short debounce, including saves that replace the file. Invalid
updates retain the last working theme and produce one desktop notification.

High contrast, Reduce Transparency and Reduce Motion are in Accessibility.
Material, accent, font and geometry controls stay visible below the gallery. Theme
overrides are under Advanced customization, in one file-picker row with a
Shared/Light/Dark selector; each stored
reference is retained when selecting another scope. Settings and shell switches
share the same palette. Switch and slider thumbs stay circular regardless of
shell corner radius.

When an older config first adopts mode or preset selection, Ferese moves its
inline palette into the dark selection and preserves comments. Shared wallpaper,
font, and geometry settings continue to apply to both appearances. For a wallpaper
pair, set `background { path "..."; }` inside each appearance or its theme file.
