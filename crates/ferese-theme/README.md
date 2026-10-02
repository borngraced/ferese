# Ferese theme

This crate supplies the shared native UI theme used by the shell, settings, lock
screen, authentication prompt and screen-sharing/recording chooser.

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

Each application supplies its layout, data, actions and animation progress.
Add styles used by several apps to the shared components.

```rust
let palette = ferese_theme::Palette::from_document(Some(&document));
let native_theme = palette.native_theme();
let input_style = ferese_theme::controls::settings_input(palette);
let font = ferese_theme::font(Some("Inter"));
let label = ferese_theme::text("Appearance", font);
```

Backdrop blur and material rendering happen in the compositor. This crate does
not reproduce those effects on the client.
