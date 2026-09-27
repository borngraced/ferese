use crate::{
    Message,
    schema::Page,
    store::{Edit, Snapshot, set},
};
use cosmic::{
    Element,
    iced::{Background, Border, Color, Length, Vector},
    theme,
    widget::{self, button, container, icon as svg_icon},
};

#[derive(Clone, Copy)]
pub struct Palette {
    pub background: Color,
    pub sidebar: Color,
    pub card: Color,
    pub accent: Color,
    pub text: Color,
    pub muted: Color,
    pub error: Color,
}

pub fn color(value: &str, fallback: Color) -> Color {
    let Some(value) = value.strip_prefix('#') else {
        return fallback;
    };

    if value.len() != 6 {
        return fallback;
    }

    u32::from_str_radix(value, 16)
        .map(|rgb| Color::from_rgb8((rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8))
        .unwrap_or(fallback)
}

impl Palette {
    pub fn from(snapshot: &Snapshot) -> Self {
        let base = color(
            &snapshot.string("theme.colors.surface_base", "#111821"),
            Color::from_rgb8(15, 16, 20),
        );
        Self {
            background: mix(base, surface_shade(base), 0.025),
            sidebar: base,
            card: mix(base, surface_shade(base), 0.055),
            accent: color(
                &snapshot.string("theme.colors.accent", "#3D7BE6"),
                Color::from_rgb8(61, 123, 230),
            ),
            text: color(
                &snapshot.string("theme.colors.text_primary", "#F4F7FB"),
                Color::WHITE,
            ),
            muted: color(
                &snapshot.string("theme.colors.text_muted", "#8793A2"),
                Color::from_rgb8(150, 149, 159),
            ),
            error: mix(base, Color::from_rgb8(210, 80, 80), 0.2),
        }
    }
}

fn luminance(color: Color) -> f32 {
    let channel = |value: f32| {
        if value <= 0.04045 {
            value / 12.92
        } else {
            ((value + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * channel(color.r) + 0.7152 * channel(color.g) + 0.0722 * channel(color.b)
}

fn surface_shade(base: Color) -> Color {
    if luminance(base) > 0.5 {
        Color::BLACK
    } else {
        Color::WHITE
    }
}

fn cosmic_color(color: Color) -> cosmic::theme::CosmicColor {
    cosmic::theme::CosmicColor::new(color.r, color.g, color.b, 1.)
}

fn mix(a: Color, b: Color, t: f32) -> Color {
    Color::from_rgb(
        a.r + (b.r - a.r) * t,
        a.g + (b.g - a.g) * t,
        a.b + (b.b - a.b) * t,
    )
}

fn hex(c: Color) -> String {
    format!(
        "#{:02x}{:02x}{:02x}",
        (c.r * 255.) as u8,
        (c.g * 255.) as u8,
        (c.b * 255.) as u8
    )
}

pub fn surface(background: Color, radius: f32) -> theme::Container<'static> {
    theme::Container::custom(move |_| container::Style {
        background: Some(Background::Color(background)),
        border: Border {
            radius: radius.into(),
            ..Default::default()
        },
        ..Default::default()
    })
}

pub fn button_style(p: Palette, selected: bool) -> theme::Button {
    styled_button(p, selected, false)
}

pub fn navigation_style(p: Palette, selected: bool) -> theme::Button {
    styled_button(p, selected, true)
}

pub fn input_style(p: Palette) -> theme::TextInput {
    let appearance = move |focused: bool, hovered: bool| cosmic::widget::text_input::Appearance {
        background: mix(p.sidebar, p.card, if hovered { 0.65 } else { 0.4 }).into(),
        border_radius: 7.into(),
        border_width: if focused { 1. } else { 0. },
        border_offset: None,
        border_color: p.accent,
        icon_color: Some(p.muted),
        text_color: Some(p.text),
        placeholder_color: p.muted,
        selected_text_color: p.sidebar,
        selected_fill: p.accent,
        label_color: p.muted,
    };
    theme::TextInput::Custom {
        active: Box::new(move |_| appearance(false, false)),
        hovered: Box::new(move |_| appearance(false, true)),
        focused: Box::new(move |_| appearance(true, true)),
        error: Box::new(move |_| appearance(true, false)),
        disabled: Box::new(move |_| appearance(false, false)),
    }
}

fn styled_button(p: Palette, selected: bool, navigation: bool) -> theme::Button {
    let style = move |hover: bool| button::Style {
        background: Some(Background::Color(if selected {
            mix(p.sidebar, p.accent, if hover { 0.24 } else { 0.17 })
        } else if hover {
            mix(p.card, surface_shade(p.sidebar), 0.06)
        } else if navigation {
            p.sidebar
        } else {
            p.card
        })),
        text_color: Some(p.text),
        icon_color: None,
        border_radius: 9.into(),
        border_width: if selected { 1. } else { 0. },
        border_color: if selected {
            p.accent
        } else {
            Color::TRANSPARENT
        },
        outline_width: 0.,
        outline_color: Color::TRANSPARENT,
        overlay: None,
        shadow_offset: Vector::ZERO,
    };

    theme::Button::Custom {
        active: Box::new(move |_, _| style(false)),
        hovered: Box::new(move |_, _| style(true)),
        pressed: Box::new(move |_, _| style(true)),
        disabled: Box::new(move |_| style(false)),
    }
}

pub fn native_theme(snapshot: Option<&Snapshot>) -> cosmic::Theme {
    let Some(snapshot) = snapshot else {
        return cosmic::Theme::custom(std::sync::Arc::new(cosmic::theme::COSMIC_DARK.clone()));
    };
    let palette = Palette::from(snapshot);
    let builder = if luminance(palette.sidebar) > 0.5 {
        cosmic::cosmic_theme::ThemeBuilder::light()
    } else {
        cosmic::cosmic_theme::ThemeBuilder::dark()
    };
    let theme = builder
        .bg_color(cosmic_color(palette.background))
        .primary_container_bg(cosmic_color(palette.card))
        .text_tint(cosmic_color(palette.text).color)
        .accent(cosmic_color(palette.accent).color)
        .build();
    cosmic::Theme::custom(std::sync::Arc::new(theme))
}

pub fn configured_font(snapshot: &Snapshot) -> cosmic::font::Font {
    static FONTS: std::sync::OnceLock<
        std::sync::Mutex<std::collections::HashMap<String, cosmic::font::Font>>,
    > = std::sync::OnceLock::new();
    let family = snapshot.string("theme.typography.font_family", "Inter");
    let mut fonts = FONTS.get_or_init(Default::default).lock().unwrap();
    if let Some(font) = fonts.get(&family) {
        return *font;
    }
    if fonts.len() >= 32 {
        return cosmic::font::default();
    }
    let font = cosmic::font::Font::with_name(Box::leak(family.clone().into_boxed_str()));
    fonts.insert(family, font);
    font
}

pub fn icon(page: Page, tint: Color) -> svg_icon::Icon {
    let svg = format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24">
    <path d="{}" fill="none" stroke="{}" stroke-width="1.6"
        stroke-linecap="round" stroke-linejoin="round"/>
</svg>"##,
        page.icon(),
        hex(tint),
    );
    svg_icon::from_svg_bytes(svg.into_bytes())
        .symbolic(false)
        .icon()
        .size(19)
}

pub fn brand_icon() -> svg_icon::Icon {
    svg_icon::from_svg_bytes(include_bytes!("../../../packaging/icons/ferese.svg").as_slice())
        .symbolic(false)
        .icon()
        .size(32)
}

pub fn action_icon(path: &str, tint: Color) -> svg_icon::Icon {
    let svg = format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24">
    <path d="{path}" fill="none" stroke="{}" stroke-width="1.6"
        stroke-linecap="round" stroke-linejoin="round"/>
</svg>"##,
        hex(tint),
    );
    svg_icon::from_svg_bytes(svg.into_bytes())
        .symbolic(false)
        .icon()
        .size(16)
}

pub struct Preset {
    pub name: &'static str,
    pub description: &'static str,
    accent: &'static str,
    base: &'static str,
    text: &'static str,
    muted: &'static str,
    border: &'static str,
    gradient_end: &'static str,
}

pub const PRESETS: [Preset; 6] = [
    Preset {
        name: "Ferese Blue",
        description: "Default · logo blue",
        accent: "#3D7BE6",
        base: "#111821",
        text: "#F4F7FB",
        muted: "#8793A2",
        border: "#FFFFFF18",
        gradient_end: "#3D7BE6",
    },
    Preset {
        name: "Monochrome",
        description: "Dark · grayscale",
        accent: "#E5E5E5",
        base: "#101012",
        text: "#EDEDF0",
        muted: "#97979F",
        border: "#FFFFFF18",
        gradient_end: "#696973",
    },
    Preset {
        name: "Gruvbox",
        description: "Dark · warm orange",
        accent: "#FE8019",
        base: "#282828",
        text: "#EBDBB2",
        muted: "#BDAE93",
        border: "#504945",
        gradient_end: "#FABD2F",
    },
    Preset {
        name: "Dracula",
        description: "Dark · purple",
        accent: "#BD93F9",
        base: "#282A36",
        text: "#F8F8F2",
        muted: "#A4ADCD",
        border: "#44475A",
        gradient_end: "#FF79C6",
    },
    Preset {
        name: "Ayu Light",
        description: "Light · paper",
        accent: "#8F5300",
        base: "#F8F9FA",
        text: "#5C6166",
        muted: "#596574",
        border: "#C8CDD3",
        gradient_end: "#996000",
    },
    Preset {
        name: "Monokai",
        description: "Dark · warm green",
        accent: "#A6E22E",
        base: "#272822",
        text: "#F8F8F2",
        muted: "#B2B29F",
        border: "#414339",
        gradient_end: "#E6DB74",
    },
];

pub fn preset(index: usize) -> Vec<Edit> {
    let Some(preset) = PRESETS.get(index) else {
        return Vec::new();
    };
    vec![
        set("theme.colors.accent", preset.accent),
        set("theme.colors.surface_base", preset.base),
        set("theme.colors.text_primary", preset.text),
        set("theme.colors.text_muted", preset.muted),
        set("theme.colors.border", preset.border),
        set("theme.colors.shadow", "#00000055"),
        set("theme.surface.bar.background", preset.base),
        set("theme.surface.bar.text_primary", preset.text),
        set("theme.surface.bar.text_muted", preset.muted),
        set("theme.focus_ring.gradient.from", preset.accent),
        set("theme.focus_ring.gradient.to", preset.gradient_end),
        set("theme.focus_ring.gradient.angle", 0.0),
    ]
}

pub fn preset_selected(snapshot: &Snapshot, index: usize) -> bool {
    let Some(preset) = PRESETS.get(index) else {
        return false;
    };
    [
        ("theme.colors.accent", preset.accent, "#3D7BE6"),
        ("theme.colors.surface_base", preset.base, "#111821"),
        ("theme.colors.text_primary", preset.text, "#F4F7FB"),
        ("theme.colors.text_muted", preset.muted, "#8793A2"),
        ("theme.colors.border", preset.border, "#FFFFFF18"),
        ("theme.colors.shadow", "#00000055", "#00000055"),
        ("theme.surface.bar.background", preset.base, "#1C202EF2"),
        ("theme.surface.bar.text_primary", preset.text, "#F0F3FA"),
        ("theme.surface.bar.text_muted", preset.muted, "#AAB4C7"),
        ("theme.focus_ring.gradient.from", preset.accent, "#3D7BE6"),
        (
            "theme.focus_ring.gradient.to",
            preset.gradient_end,
            "#3D7BE6",
        ),
    ]
    .into_iter()
    .all(|(path, expected, fallback)| {
        if index == 0 && snapshot.item(path).is_none() {
            return true;
        }
        snapshot
            .string(path, fallback)
            .eq_ignore_ascii_case(expected)
    }) && snapshot.number("theme.focus_ring.gradient.angle", 0.) == 0.
}

pub fn swatches(index: usize) -> Element<'static, Message> {
    let preset = &PRESETS[index];
    let mut row = widget::row([]).spacing(3);
    for value in [preset.base, preset.gradient_end, preset.accent] {
        row = row.push(
            container(widget::Space::new().width(12).height(22))
                .class(surface(color(value, Color::WHITE), 4.)),
        );
    }
    row.into()
}

pub fn preview(snapshot: &Snapshot) -> Element<'static, Message> {
    let p = Palette::from(snapshot);
    let base = hex(p.sidebar);
    let card = hex(p.card);
    let accent = hex(p.accent);
    let muted = hex(p.muted);
    let gap = snapshot.number("layout.inner_gap", 8.).clamp(0., 32.);
    let radius = snapshot
        .number("theme.geometry.window_radius", 14.)
        .clamp(0., 28.);
    let bar_y = snapshot.number("theme.geometry.top_bar_margin_top", 0.) * 0.5 + 12.;
    let bar_margin = snapshot.number("theme.geometry.top_bar_margin_horizontal", 0.) * 0.5 + 14.;
    let bar_height = snapshot.number("theme.geometry.top_bar_height", 30.) * 0.65;
    let bar_radius = snapshot.number("theme.geometry.top_bar_radius", 0.) * 0.65;
    let opacity = snapshot.number("theme.surface.bar.opacity", 0.78);
    let right = 340. + gap / 2.;
    let left_width = 288. - gap / 2.;
    let svg = format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="680" height="210" viewBox="0 0 680 210">
      <defs><linearGradient id="wall" x2="1" y2="1"><stop stop-color="{accent}" stop-opacity=".20"/><stop offset="1" stop-color="{base}"/></linearGradient></defs>
      <rect width="680" height="210" rx="14" fill="{base}"/><rect width="680" height="210" rx="14" fill="url(#wall)"/>
      <path d="M0 180Q160 30 350 145T680 100V210H0Z" fill="{accent}" opacity=".055"/>
      <rect x="{bar_margin}" y="{bar_y}" width="{}" height="{bar_height}" rx="{bar_radius}" fill="{base}" opacity="{opacity}"/>
      <rect x="30" y="{}" width="12" height="5" rx="2.5" fill="{accent}"/><circle cx="51" cy="{}" r="2.5" fill="{muted}"/>
      <path d="M595 {}h10m10 0h10m10 0h10" stroke="{muted}" stroke-width="3" stroke-linecap="round"/>
      <rect x="44" y="60" width="{left_width}" height="132" rx="{radius}" fill="{card}" stroke="{accent}" stroke-width="1.2"/>
      <rect x="{right}" y="60" width="{left_width}" height="132" rx="{radius}" fill="{card}"/>
      <path d="M64 80h65M64 100h170M64 115h120M64 130h152" stroke="{muted}" opacity=".45" stroke-width="4" stroke-linecap="round"/>
      <rect x="{}" y="78" width="50" height="94" rx="5" fill="{base}"/>
      <path d="M420 82h85M420 103h175M420 121h150M420 139h160" stroke="{muted}" opacity=".32" stroke-width="4" stroke-linecap="round"/>
    </svg>"##,
        680. - 2. * bar_margin,
        bar_y + bar_height / 2. - 2.5,
        bar_y + bar_height / 2.,
        bar_y + bar_height / 2.,
        right + 16.
    );
    svg_icon::from_svg_bytes(svg.into_bytes())
        .symbolic(false)
        .icon()
        .width(Length::Fill)
        .height(Length::Fixed(132.))
        .content_fit(cosmic::iced::ContentFit::Contain)
        .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn configured_preset(index: usize) -> Snapshot {
        let mut snapshot = Snapshot::parse(String::new()).unwrap();
        for edit in preset(index) {
            snapshot.edit(&edit).unwrap();
        }
        snapshot
    }

    fn contrast(a: Color, b: Color) -> f32 {
        let a = luminance(a);
        let b = luminance(b);
        (a.max(b) + 0.05) / (a.min(b) + 0.05)
    }

    #[test]
    fn presets_are_distinct_and_readable_on_settings_surfaces() {
        for (index, preset) in PRESETS.iter().enumerate() {
            let snapshot = configured_preset(index);
            let palette = Palette::from(&snapshot);
            for background in [palette.background, palette.sidebar, palette.card] {
                assert!(
                    contrast(palette.text, background) >= 4.5,
                    "{} text",
                    preset.name
                );
                assert!(
                    contrast(palette.muted, background) >= 4.5,
                    "{} muted text",
                    preset.name
                );
            }
            for other in PRESETS.iter().skip(index + 1) {
                assert_ne!(preset.accent, other.accent);
                assert_ne!(preset.base, other.base);
            }
        }
    }

    #[test]
    fn selection_tracks_all_preset_colors_and_gradient_changes() {
        for index in 0..PRESETS.len() {
            let mut snapshot = configured_preset(index);
            assert!(preset_selected(&snapshot, index));
            snapshot
                .edit(&set("theme.surface.bar.text_primary", "#123456"))
                .unwrap();
            assert!(!preset_selected(&snapshot, index));
            let mut snapshot = configured_preset(index);
            snapshot
                .edit(&set("theme.focus_ring.gradient.to", "#123456"))
                .unwrap();
            assert!(!preset_selected(&snapshot, index));
        }
        assert!(preset_selected(&Snapshot::parse(String::new()).unwrap(), 0));
        assert!(preset(usize::MAX).is_empty());
        assert!(!preset_selected(
            &Snapshot::parse(String::new()).unwrap(),
            usize::MAX
        ));
    }

    #[test]
    fn default_uses_logo_blue_and_native_controls_follow_lightness() {
        let logo = include_str!("../../../packaging/icons/ferese.svg").to_ascii_uppercase();
        assert!(logo.contains(PRESETS[0].accent));
        for index in 0..PRESETS.len() {
            let snapshot = configured_preset(index);
            assert_eq!(
                native_theme(Some(&snapshot)).cosmic().is_dark,
                PRESETS[index].name != "Ayu Light"
            );
        }
    }
}
