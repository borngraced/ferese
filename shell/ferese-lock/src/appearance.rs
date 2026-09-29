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
        let palette = ferese_theme::Palette::from_document(doc.as_ref());
        let font = string("theme.typography.font_family", "Inter");
        let background = string("theme.background.path", ferese_config::default_wallpaper());
        let path = string("theme.background.lock_path", &background);
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
            panel: palette.sidebar,
            text: palette.text,
            accent: palette.accent,
            radius: palette.radius,
            font: ferese_theme::font(Some(&font)),
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
        ferese_theme::Palette {
            background: self.panel,
            sidebar: self.panel,
            card: self.panel,
            text: self.text,
            muted: self.text.scale_alpha(0.55),
            accent: self.accent,
            radius: self.radius,
            error: Color::from_rgb8(235, 98, 98),
        }
        .native_theme()
    }
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
