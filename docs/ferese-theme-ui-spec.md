# Theme components

Ferese's native apps use [ferese-theme](../crates/ferese-theme/README.md) for
colors, fonts, controls, menus, icons, calendar elements, and dialog materials.
Use [Configuration → Appearance](configuration.md#appearance) for supported keys
and defaults.

## Shared appearance

The six theme families are Ferese Blue, Catppuccin, Gruvbox, Rosé Pine, Tokyo
Night, and Everforest. Each has a paired light and dark palette. Ferese Blue uses the
logo accent, `#3D7BE6`. Presets supply colors and materials; explicit wallpaper,
typography, geometry, and material overrides remain in place.

The interface font applies to shell text and native dialogs. Clock and note
widgets can override it. Managed-window corners and shell corners are separate
settings; small controls cap their corners to fit their size.

## Materials

Two material styles are supported:

| Style | Background |
| --- | --- |
| `solid` | Opaque theme surface |
| `translucent` | Theme tint over blurred background content |

Bars, popovers, notifications, and themed dialogs share `theme.material.opacity`
and `blur-radius`. Text and icons stay opaque. An opacity of zero hides the
material; blur radius zero disables blur. Solid mode stays opaque.

```kdl
theme {
    colors {
        accent "#3D7BE6"
    }
    material {
        style "translucent"
        opacity 0.78
        blur-radius 12
    }
    geometry {
        shell-radius 14
        window-radius 14
    }
}
```

The compositor owns blur and tint. `material::ModalMaterial` attaches native
dialogs to that rendering path. Keep the attachment alive with the window and
make the attached client background transparent. Outside Ferese, retain a client
background so the dialog remains usable.

## Components

| Module | Use |
| --- | --- |
| `palette` | Resolve configuration colors and build the native toolkit theme |
| `controls` | Surfaces, navigation, text buttons, and input styles |
| `menus` | Rows, headings, sections, separators, switches, and sliders |
| `gallery` | Split theme previews, selected badges and accessible radio navigation |
| `calendar` | Date/time labels, clock digits, and calendar grids |
| `icons` | Shared SVG assets, sizing, and tint |
| `typography` | Shared font selection and text |
| `contrast` | Readable labels on filled controls |
| `material` | Attach modal cards to compositor materials |

Applications provide data, layout, actions, and animation progress. Extend shared
components when a style is needed in multiple apps. Avoid hardcoded app-specific
fonts, foreground colors, or opacity values.

## UI checks

- Check text and icons in all twelve light and dark variants.
- Check rest, hover, pressed, focused, selected, and disabled controls.
- Keep accent-button labels readable after typing or changing focus.
- Keep controls usable with solid materials and blur disabled.
- Align material regions with the visible card at every output scale.
- Give dialogs one frame; cap their height and scroll their content, preserving actions.
- Respect reduced motion and stop requesting animation frames after settling.

Theme changes affect appearance; they do not change focus, workspace ownership,
input permissions, or authentication policy.
