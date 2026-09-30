# Ferese theme

The shared native UI theme for the shell, settings, lock screen, authentication
prompt and screen-sharing/recording chooser.

- `Palette` resolves KDL colors and corners and builds the native toolkit theme.
- `PRESETS` defines twelve variants in six families.
- `font` and `text` provide shared font selection and text.
- `gallery` supplies split theme tiles, accessible radio groups and keyboard navigation.
- `controls` supplies surfaces, navigation, font-aware text buttons and input styles.
- `menus` supplies headings, rows, sections, separators, switches and slider styles.
- `calendar` supplies date/time labels, stacked clock digits and calendar grids.
  Callers supply the date and navigation controls.
- `icons` owns the shared SVG assets, outline icons, sizing and semantic tinting.
- Contrast helpers keep filled controls readable in every interaction state.
- `material::ModalMaterial` attaches a dialog to the compositor's shell material.
  Keep the attachment alive with its window and make the client background
  transparent after attachment succeeds. Other compositors retain the client fill.

Applications supply layout, data, actions, and animation progress. Extend shared
components when a style is needed across apps.

```rust
let palette = ferese_theme::Palette::from_document(Some(&document));
let native_theme = palette.native_theme();
let input_style = ferese_theme::controls::settings_input(palette);
let font = ferese_theme::font(Some("Inter"));
let label = ferese_theme::text("Appearance", font);
```

The compositor owns backdrop blur and material rendering. This crate does not
provide a client-side imitation of those effects.
