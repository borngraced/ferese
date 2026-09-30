use cosmic::iced::Color;
use ferese_config::Document;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Palette {
    pub background: Color,
    pub sidebar: Color,
    pub card: Color,
    pub accent: Color,
    pub text: Color,
    pub muted: Color,
    pub error: Color,
    pub radius: f32,
}

impl Palette {
    pub fn from_document(document: Option<&Document>) -> Self {
        let color = |key, fallback| {
            document
                .and_then(|document| document.get(key))
                .and_then(|value| value.as_str())
                .and_then(parse_color)
                .unwrap_or_else(|| parse_color(fallback).expect("valid default theme color"))
        };
        let base = color("theme.colors.surface_base", "#111821");
        Self {
            background: mix(base, surface_shade(base), 0.025),
            sidebar: base,
            card: mix(base, surface_shade(base), 0.055),
            accent: color("theme.colors.accent", "#3D7BE6"),
            text: color("theme.colors.text_primary", "#F4F7FB"),
            muted: color("theme.colors.text_muted", "#8793A2"),
            error: mix(base, Color::from_rgb8(210, 80, 80), 0.2),
            radius: document
                .and_then(|document| {
                    document
                        .get("theme.geometry.shell_radius")
                        .or_else(|| document.get("appearance.corner_radius"))
                })
                .and_then(|value| value.as_f64())
                .filter(|value| value.is_finite())
                .unwrap_or(14.)
                .clamp(0., 64.) as f32,
        }
    }

    pub fn flat(mut self) -> Self {
        self.background = self.sidebar;
        self.card = self.sidebar;
        self
    }

    pub fn native_theme(self) -> cosmic::Theme {
        let rgba = |c: Color| cosmic::cosmic_theme::palette::Srgba::new(c.r, c.g, c.b, 1.);
        let builder = if crate::luminance(self.sidebar) > 0.5 {
            cosmic::cosmic_theme::ThemeBuilder::light()
        } else {
            cosmic::cosmic_theme::ThemeBuilder::dark()
        };
        let corners = cosmic::cosmic_theme::CornerRadii {
            radius_xs: [self.radius.min(4.); 4],
            radius_s: [self.radius.min(8.); 4],
            radius_m: [self.radius; 4],
            radius_l: [self.radius; 4],
            radius_xl: [self.radius; 4],
            radius_0: Default::default(),
        };
        let mut native = builder
            .corner_radii(corners)
            .bg_color(rgba(self.background))
            .primary_container_bg(rgba(self.card))
            .text_tint(rgba(self.text).color)
            .accent(crate::accent_color(self.accent, self.card))
            .build();
        crate::apply(&mut native, self.text);
        cosmic::Theme::custom(std::sync::Arc::new(native))
    }

    pub fn application_style(self, background: Color) -> cosmic::iced::theme::Style {
        cosmic::iced::theme::Style {
            background_color: background,
            text_color: self.text,
            icon_color: self.text,
        }
    }
}

pub fn parse_color(value: &str) -> Option<Color> {
    let hex = value.strip_prefix('#')?;
    let packed = u32::from_str_radix(hex, 16).ok()?;
    let rgba = match hex.len() {
        6 => (packed << 8) | 255,
        8 => packed,
        _ => return None,
    };
    Some(Color::from_rgba8(
        (rgba >> 24) as u8,
        (rgba >> 16) as u8,
        (rgba >> 8) as u8,
        (rgba & 255) as f32 / 255.,
    ))
}

pub fn mix(a: Color, b: Color, t: f32) -> Color {
    Color::from_rgb(a.r + (b.r - a.r) * t, a.g + (b.g - a.g) * t, a.b + (b.b - a.b) * t)
}

pub fn surface_shade(base: Color) -> Color {
    if crate::luminance(base) > 0.5 {
        Color::BLACK
    } else {
        Color::WHITE
    }
}

pub fn material_opacity(document: Option<&Document>) -> f32 {
    let style = document
        .and_then(|document| document.get("theme.material.style"))
        .and_then(|value| value.as_str())
        .unwrap_or("solid");
    if style != "translucent" {
        return 1.;
    }
    document
        .and_then(|document| document.get("theme.material.opacity"))
        .and_then(|value| value.as_f64())
        .filter(|value| value.is_finite())
        .unwrap_or(ferese_config::DEFAULT_MATERIAL_OPACITY)
        .clamp(0., 1.) as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_preset_builds_an_opaque_control_palette_over_translucent_materials() {
        for preset in crate::PRESETS {
            let document = Document::parse(&format!(
                r##"theme {{
                    colors {{
                        surface-base "{}"
                        accent "{}"
                        text-primary "{}"
                        text-muted "{}"
                    }}
                    material {{ style translucent; opacity 0.25; }}
                    geometry {{ shell-radius 0; }}
                }}"##,
                preset.base, preset.accent, preset.text, preset.muted,
            ))
            .unwrap();
            let palette = Palette::from_document(Some(&document));
            let native = palette.native_theme();
            assert_eq!(material_opacity(Some(&document)), 0.25);
            assert_eq!(native.cosmic().primary(false).base.alpha, 1.);
            assert_eq!(native.cosmic().corner_radii.radius_m, [0.; 4]);
            for component in [&native.cosmic().accent, &native.cosmic().accent_button] {
                for fill in [component.base, component.hover, component.pressed] {
                    assert!(
                        crate::contrast(fill.into(), component.on.into()) >= 4.5,
                        "{}",
                        preset.name
                    );
                }
            }
        }
    }

    #[test]
    fn solid_materials_ignore_opacity_and_translucent_materials_bound_it() {
        for (style, opacity, expected) in [
            ("solid", 0.4, 1.),
            ("translucent", 0.4, 0.4),
            ("translucent", 2., 1.),
            ("translucent", -1., 0.),
        ] {
            let document =
                Document::parse(&format!("theme {{ material {{ style {style}; opacity {opacity}; }} }}")).unwrap();
            assert_eq!(material_opacity(Some(&document)), expected);
        }
    }
}
