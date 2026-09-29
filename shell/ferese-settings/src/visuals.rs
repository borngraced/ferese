use crate::{
    Message,
    schema::Page,
    store::{Edit, Snapshot, set},
};
use cosmic::{
    Element,
    iced::{Color, Length},
    widget::icon as svg_icon,
};

pub use ferese_theme::controls::{
    button_style, navigation_style, settings_input as input_style, surface,
};
pub use ferese_theme::{Palette, mix, surface_shade};

pub fn color(value: &str, fallback: Color) -> Color {
    ferese_theme::parse_color(value).unwrap_or(fallback)
}

fn hex(c: Color) -> String {
    format!(
        "#{:02x}{:02x}{:02x}",
        (c.r * 255.) as u8,
        (c.g * 255.) as u8,
        (c.b * 255.) as u8
    )
}

pub fn native_theme(snapshot: Option<&Snapshot>) -> cosmic::Theme {
    Palette::from_document(snapshot.map(|snapshot| &snapshot.doc)).native_theme()
}

pub fn configured_font(snapshot: &Snapshot) -> cosmic::font::Font {
    ferese_theme::font(Some(
        &snapshot.string("theme.typography.font_family", "Inter"),
    ))
}

pub fn icon(page: Page, tint: Color) -> svg_icon::Icon {
    ferese_theme::icons::outline(page.icon(), tint, 19)
}

pub fn brand_icon() -> svg_icon::Icon {
    cosmic::widget::icon::from_svg_bytes(ferese_theme::icons::APPLICATION)
        .symbolic(false)
        .icon()
        .size(32)
}

pub fn action_icon(path: &str, tint: Color) -> svg_icon::Icon {
    ferese_theme::icons::outline(path, tint, 16)
}

pub use ferese_theme::{PRESETS, Preset};

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

fn preset_preview_handle(preset: &Preset) -> svg_icon::Handle {
    let base = preset.base;
    let accent = preset.accent;
    let end = preset.gradient_end;
    let text = preset.text;
    let muted = preset.muted;
    let surface = color(base, Color::BLACK);
    let card = hex(mix(surface, surface_shade(surface), 0.055));
    let svg = format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="240" height="132" viewBox="0 0 240 132">
    <defs>
        <linearGradient id="wall" x2="1" y2="1">
            <stop stop-color="{accent}" stop-opacity=".24"/>
            <stop offset="1" stop-color="{end}" stop-opacity=".06"/>
        </linearGradient>
    </defs>
    <rect width="240" height="132" rx="10" fill="{base}"/>
    <rect width="240" height="132" rx="10" fill="url(#wall)"/>
    <path d="M0 112Q52 48 120 93T240 67V132H0Z" fill="{accent}" opacity=".09"/>
    <rect x="8" y="8" width="224" height="12" rx="4" fill="{base}"/>
    <rect x="14" y="12" width="6" height="4" rx="1" fill="{accent}"/>
    <path d="M26 14h4m4 0h4m164 0h6m4 0h6" stroke="{muted}" stroke-width="2" stroke-linecap="round"/>
    <rect x="16" y="34" width="121" height="82" rx="7" fill="{card}"/>
    <path d="M26 44h102" stroke="{muted}" opacity=".16"/>
    <rect x="23" y="50" width="25" height="59" rx="3" fill="{base}"/>
    <rect x="26" y="57" width="19" height="7" rx="2" fill="{accent}" opacity=".6"/>
    <path d="M29 73h11m-11 9h9m-9 9h12" stroke="{muted}" opacity=".6" stroke-width="2" stroke-linecap="round"/>
    <path d="M56 59h38m-38 10h63m-63 8h49m-49 8h58" stroke="{text}" opacity=".55" stroke-width="2" stroke-linecap="round"/>
    <rect x="55" y="94" width="29" height="9" rx="3" fill="{accent}"/>
    <rect x="145" y="37" width="78" height="70" rx="7" fill="{card}" stroke="{accent}" stroke-width="1.2"/>
    <circle cx="154" cy="45" r="1.5" fill="{accent}"/>
    <path d="M161 45h17" stroke="{muted}" stroke-width="2" stroke-linecap="round"/>
    <path d="M155 58h27m-27 9h56m-56 9h37m-37 9h49m-49 9h30" stroke="{text}" opacity=".55" stroke-width="2" stroke-linecap="round"/>
</svg>"##,
    );
    svg_icon::from_svg_bytes(svg.into_bytes()).symbolic(false)
}

pub fn preset_preview(index: usize) -> Element<'static, Message> {
    static HANDLES: std::sync::OnceLock<Vec<svg_icon::Handle>> = std::sync::OnceLock::new();
    HANDLES.get_or_init(|| PRESETS.iter().map(preset_preview_handle).collect())[index]
        .clone()
        .icon()
        .width(Length::Fill)
        .height(Length::Fixed(90.))
        .content_fit(cosmic::iced::ContentFit::Contain)
        .into()
}

pub fn preview(snapshot: &Snapshot) -> Element<'static, Message> {
    let p = Palette::from_document(Some(&snapshot.doc));
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
    let bar_radius = snapshot.number("theme.geometry.shell_radius", 14.) * 0.65;
    let opacity = if snapshot.string("theme.material.style", "solid") == "translucent" {
        snapshot
            .number(
                "theme.material.opacity",
                ferese_config::DEFAULT_MATERIAL_OPACITY,
            )
            .clamp(0., 1.)
    } else {
        1.
    };
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
        ferese_theme::contrast(a, b)
    }

    #[test]
    fn presets_are_distinct_and_readable_on_settings_surfaces() {
        for (index, preset) in PRESETS.iter().enumerate() {
            let snapshot = configured_preset(index);
            let palette = Palette::from_document(Some(&snapshot.doc));
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

        for (index, item) in PRESETS.iter().enumerate() {
            let snapshot = configured_preset(index);
            assert_eq!(
                native_theme(Some(&snapshot)).cosmic().is_dark,
                item.name != "Ayu Light"
            );
        }
    }
}
