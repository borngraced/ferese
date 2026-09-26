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
            background: mix(base, Color::WHITE, 0.025),
            sidebar: base,
            card: mix(base, Color::WHITE, 0.055),
            accent: color(
                &snapshot.string("theme.colors.accent", "#5B8CFF"),
                Color::from_rgb8(229, 200, 144),
            ),
            text: color(
                &snapshot.string("theme.colors.text_primary", "#e2e0e6"),
                Color::WHITE,
            ),
            muted: color(
                &snapshot.string("theme.colors.text_muted", "#96959f"),
                Color::from_rgb8(150, 149, 159),
            ),
            error: mix(base, Color::from_rgb8(210, 80, 80), 0.2),
        }
    }
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
            mix(p.card, Color::WHITE, 0.06)
        } else if navigation {
            p.sidebar
        } else {
            p.card
        })),
        text_color: Some(if selected { p.accent } else { p.text }),
        icon_color: None,
        border_radius: 9.into(),
        border_width: 0.,
        border_color: Color::TRANSPARENT,
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
    let mut theme = cosmic::theme::COSMIC_DARK.clone();
    if let Some(snapshot) = snapshot {
        let p = Palette::from(snapshot);
        let accent = cosmic::theme::CosmicColor::new(p.accent.r, p.accent.g, p.accent.b, 1.);
        theme.accent.base = accent;
        theme.accent_button.base = accent;
        theme.text_button.on = accent;
    }
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
    svg_icon::from_svg_bytes(format!(r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24"><path d="{}" fill="none" stroke="{}" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round"/></svg>"##, page.icon(),hex(tint)).into_bytes()).symbolic(false).icon().size(19)
}
pub fn brand_icon() -> svg_icon::Icon {
    svg_icon::from_svg_bytes(include_bytes!("../../../packaging/icons/ferese.svg").as_slice())
        .symbolic(false)
        .icon()
        .size(32)
}
const PRESETS: [(&str, &str, &str, &str, &str); 3] = [
    ("#e5c890", "#0f1014", "#c9c7cd", "#9998a8", "#795e40"),
    ("#e5e5e5", "#101012", "#ededf0", "#97979f", "#696973"),
    ("#cba6f7", "#1e1e2e", "#cdd6f4", "#a6adc8", "#89b4fa"),
];
pub fn preset(index: usize) -> Vec<Edit> {
    let (accent, base, text, muted, end) = PRESETS[index.min(2)];
    vec![
        set("theme.colors.accent", accent),
        set("theme.colors.surface_base", base),
        set("theme.colors.text_primary", text),
        set("theme.colors.text_muted", muted),
        set("theme.surface.bar.background", base),
        set("theme.surface.bar.text_primary", text),
        set("theme.surface.bar.text_muted", muted),
        set("theme.focus_ring.gradient.from", accent),
        set("theme.focus_ring.gradient.to", end),
    ]
}
pub fn preset_selected(snapshot: &Snapshot, index: usize) -> bool {
    let (accent, base, text, muted, _) = PRESETS[index];
    [
        ("accent", accent),
        ("surface_base", base),
        ("text_primary", text),
        ("text_muted", muted),
    ]
    .into_iter()
    .all(|(key, value)| {
        snapshot
            .string(&format!("theme.colors.{key}"), "")
            .eq_ignore_ascii_case(value)
    })
}
pub fn swatches(index: usize) -> Element<'static, Message> {
    let (accent, base, _, _, end) = PRESETS[index];
    let mut row = widget::row([]).spacing(3);
    for value in [base, end, accent] {
        row = row.push(
            container(widget::Space::new().width(12).height(22))
                .class(surface(color(value, Color::WHITE), 4.)),
        );
    }
    row.into()
}
// A deliberately stylized preview, built as vector geometry at the actual
// output scale. No full-resolution wallpaper buffers or screenshot polling.
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
