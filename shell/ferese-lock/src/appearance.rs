use cosmic::{iced::Color, widget::image};
use ferese_config::Document;
#[derive(Clone)]
pub struct Appearance {
    pub dim: f32,
    pub show_clock: bool,
    pub show_date: bool,
    pub twelve_hour: bool,
    pub panel: Color,
    pub text: Color,
    pub accent: Color,
    pub radius: f32,
    pub font: cosmic::font::Font,
    pub wallpaper: image::Handle,
    pub avatar: Option<image::Handle>,
}
impl Appearance {
    pub fn load(user: &str, path: Option<&std::path::Path>) -> Self {
        let doc = path
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|s| Document::parse(&s).ok());
        let string = |key: &str, fallback: &str| {
            doc.as_ref()
                .and_then(|d| d.get(key))
                .and_then(|v| v.as_str())
                .unwrap_or(fallback)
                .to_owned()
        };
        let color = |key, fallback| {
            parse_color(&string(key, fallback)).unwrap_or_else(|| parse_color(fallback).unwrap())
        };
        let font = string("theme.typography.font_family", "Inter");
        let path = string("theme.background.path", ferese_config::default_wallpaper());
        let path = if let Some(tail) = path.strip_prefix("~/") {
            std::env::var("HOME")
                .map(|home| format!("{home}/{tail}"))
                .unwrap_or(path)
        } else {
            path
        };
        let wallpaper = image::Handle::from_path(path);
        // Soften a bounded thumbnail once, before requesting the lock. The
        // renderer scales this shared image across outputs; no per-frame blur.
        let pixels = cosmic::iced::advanced::graphics::image::load(&wallpaper)
            .or_else(|_| {
                cosmic::iced::advanced::graphics::image::load(&image::Handle::from_bytes(
                    include_bytes!("../../../assets/wallpapers/ferese.png").as_slice(),
                ))
            })
            .expect("bundled wallpaper is valid");
        let blur = doc
            .as_ref()
            .and_then(|d| d.get("lock_screen.background_blur"))
            .and_then(|v| v.as_f64())
            .unwrap_or(18.)
            .clamp(0., 40.) as f32;
        let wallpaper = if blur > 0. {
            let thumbnail = ::image::imageops::thumbnail(&pixels, 640, 640);
            let softened = ::image::imageops::blur(&thumbnail, blur);
            image::Handle::from_rgba(softened.width(), softened.height(), softened.into_raw())
        } else {
            image::Handle::from_rgba(pixels.width(), pixels.height(), pixels.into_raw())
        };
        let boolean = |key, fallback| {
            doc.as_ref()
                .and_then(|d| d.get(key))
                .and_then(|v| v.as_bool())
                .unwrap_or(fallback)
        };
        Self {
            dim: doc
                .as_ref()
                .and_then(|d| d.get("lock_screen.background_dim"))
                .and_then(|v| v.as_f64())
                .unwrap_or(0.48)
                .clamp(0., 0.9) as f32,
            show_clock: boolean("lock_screen.show_clock", true),
            show_date: boolean("lock_screen.show_date", true),
            twelve_hour: string("lock_screen.clock_format", "24h") == "12h",
            panel: color("theme.colors.surface_base", "#111821"),
            text: color("theme.colors.text_primary", "#F4F7FB"),
            accent: color("theme.colors.accent", "#3D7BE6"),
            radius: doc
                .as_ref()
                .and_then(|d| {
                    d.get("theme.geometry.shell_radius")
                        .or_else(|| d.get("appearance.corner_radius"))
                })
                .and_then(|v| v.as_f64())
                .unwrap_or(14.)
                .clamp(0., 64.) as f32,
            font: cosmic::font::Font::with_name(Box::leak(font.into_boxed_str())),
            wallpaper,
            avatar: account_picture(user),
        }
    }
    pub fn clock_format(&self) -> &'static str {
        if self.twelve_hour {
            "%I:%M %p"
        } else {
            "%H:%M"
        }
    }
    pub fn theme(&self) -> cosmic::Theme {
        use cosmic::cosmic_theme::{ThemeBuilder, palette::Srgba};
        let color = |c: Color| Srgba::new(c.r, c.g, c.b, c.a);
        let builder = if self.panel.r + self.panel.g + self.panel.b > 1.5 {
            ThemeBuilder::light()
        } else {
            ThemeBuilder::dark()
        };
        let mut corners = cosmic::cosmic_theme::CornerRadii::default();
        corners.radius_xs = [self.radius.min(4.); 4];
        corners.radius_s = [self.radius.min(8.); 4];
        corners.radius_m = [self.radius; 4];
        corners.radius_l = [self.radius; 4];
        corners.radius_xl = [self.radius; 4];
        let mut native = builder
            .corner_radii(corners)
            .bg_color(color(self.panel))
            .primary_container_bg(color(self.panel))
            .text_tint(color(self.text).color)
            .accent(ferese_theme::accent_color(self.accent, self.panel))
            .build();
        ferese_theme::apply(&mut native, self.text);
        cosmic::Theme::custom(std::sync::Arc::new(native))
    }
}

fn parse_color(value: &str) -> Option<Color> {
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

fn account_picture(user: &str) -> Option<image::Handle> {
    let mut candidates =
        vec![std::path::PathBuf::from("/var/lib/AccountsService/icons").join(user)];
    if let Some(home) = std::env::var_os("HOME") {
        candidates.push(std::path::PathBuf::from(home).join(".face"));
    }
    candidates.into_iter().find_map(|path| {
        let pixels =
            cosmic::iced::advanced::graphics::image::load(&image::Handle::from_path(path)).ok()?;
        let side = pixels.width().min(pixels.height());
        if side == 0 {
            return None;
        }
        let square = ::image::imageops::crop_imm(
            &pixels,
            (pixels.width() - side) / 2,
            (pixels.height() - side) / 2,
            side,
            side,
        )
        .to_image();
        let mut pixels =
            ::image::imageops::resize(&square, 256, 256, ::image::imageops::FilterType::Lanczos3);
        // Clip the decoded thumbnail, so the avatar stays circular even on
        // renderers that do not support rounded image clipping.
        for (x, y, pixel) in pixels.enumerate_pixels_mut() {
            let distance =
                ((x as f32 + 0.5 - 128.).powi(2) + (y as f32 + 0.5 - 128.).powi(2)).sqrt();
            let coverage = (128. - distance).clamp(0., 1.);
            pixel[3] = (pixel[3] as f32 * coverage).round() as u8;
        }
        Some(image::Handle::from_rgba(
            pixels.width(),
            pixels.height(),
            pixels.into_raw(),
        ))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn lock_options_share_shell_style_without_changing_security() {
        let directory = tempfile::tempdir().unwrap();
        let config = directory.path().join("config.kdl");
        std::fs::write(
            &config,
            r##"
            theme {
                geometry { shell-radius 0; }
                colors { accent "#FF5500"; }
            }
            lock-screen {
                show-clock #false
                show-date #false
                clock-format "12h"
                background-dim 0.7
            }
        "##,
        )
        .unwrap();
        let appearance = Appearance::load("no-such-test-user", Some(&config));
        assert_eq!(appearance.radius, 0.);
        assert_eq!(appearance.accent, Color::from_rgb8(255, 85, 0));
        assert!(!appearance.show_clock && !appearance.show_date);
        assert_eq!(appearance.clock_format(), "%I:%M %p");
        assert_eq!(appearance.dim, 0.7);
        assert_eq!(appearance.theme().cosmic().corner_radii.radius_m, [0.; 4]);
    }
}
