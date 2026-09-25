use std::env;
use std::fs;
use std::path::PathBuf;

use serde::Deserialize;

const DEFAULT_BACKGROUND: [u8; 3] = [11, 15, 20];

#[derive(Clone, Debug, Default)]
pub(crate) struct ShellConfig {
    pub(crate) font_family: Option<String>,
    pub(crate) wallpaper: WallpaperConfig,
    pub(crate) theme: ShellTheme,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct ShellTheme {
    pub(crate) surface_base: [u8; 4],
    pub(crate) text_primary: [u8; 4],
    pub(crate) text_muted: [u8; 4],
    pub(crate) accent: [u8; 4],
    pub(crate) border: [u8; 4],
    pub(crate) shadow: [u8; 4],
    pub(crate) bar_height: f32,
    pub(crate) bar_margin_top: i32,
    pub(crate) bar_margin_horizontal: i32,
    pub(crate) bar_radius: f32,
    pub(crate) panel_padding: f32,
    pub(crate) control_gap: f32,
    pub(crate) shadow_offset_y: f32,
    pub(crate) shadow_blur: f32,
    pub(crate) shadow_opacity: f32,
}

impl Default for ShellTheme {
    fn default() -> Self {
        Self {
            surface_base: [17, 24, 33, 255],
            text_primary: [244, 247, 251, 255],
            text_muted: [127, 138, 152, 255],
            accent: [91, 140, 255, 255],
            border: [255, 255, 255, 24],
            shadow: [0, 0, 0, 85],
            bar_height: 38.0,
            bar_margin_top: 4,
            bar_margin_horizontal: 10,
            bar_radius: 19.0,
            panel_padding: 12.0,
            control_gap: 8.0,
            shadow_offset_y: 4.0,
            shadow_blur: 18.0,
            shadow_opacity: 0.20,
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize)]
pub(crate) struct WallpaperConfig {
    pub(crate) path: Option<PathBuf>,
    #[serde(default)]
    pub(crate) mode: WallpaperMode,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum WallpaperMode {
    #[default]
    Fill,
    Fit,
}

#[derive(Debug, Default, Deserialize)]
struct FereseConfig {
    #[serde(default)]
    theme: ThemeConfig,
}

#[derive(Debug, Default, Deserialize)]
struct ThemeConfig {
    #[serde(default)]
    colors: ThemeColorsConfig,
    #[serde(default)]
    geometry: ThemeGeometryConfig,
    #[serde(default)]
    shadow: ThemeShadowConfig,
    #[serde(default)]
    typography: TypographyConfig,
    #[serde(default)]
    background: WallpaperConfig,
}

#[derive(Debug, Default, Deserialize)]
struct TypographyConfig {
    font_family: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ThemeColorsConfig {
    #[serde(default = "default_surface_base")]
    surface_base: String,
    #[serde(default = "default_text_primary")]
    text_primary: String,
    #[serde(default = "default_text_muted")]
    text_muted: String,
    #[serde(default = "default_accent")]
    accent: String,
    #[serde(default = "default_border")]
    border: String,
    #[serde(default = "default_shadow")]
    shadow: String,
}

impl Default for ThemeColorsConfig {
    fn default() -> Self {
        Self {
            surface_base: default_surface_base(),
            text_primary: default_text_primary(),
            text_muted: default_text_muted(),
            accent: default_accent(),
            border: default_border(),
            shadow: default_shadow(),
        }
    }
}

#[derive(Debug, Deserialize)]
struct ThemeGeometryConfig {
    #[serde(default = "default_bar_height")]
    top_bar_height: f32,
    #[serde(default = "default_bar_margin_top")]
    top_bar_margin_top: i32,
    #[serde(default = "default_bar_margin_horizontal")]
    top_bar_margin_horizontal: i32,
    #[serde(default = "default_bar_radius")]
    top_bar_radius: f32,
    #[serde(default = "default_panel_padding")]
    panel_padding: f32,
    #[serde(default = "default_control_gap")]
    control_gap: f32,
}

impl Default for ThemeGeometryConfig {
    fn default() -> Self {
        Self {
            top_bar_height: default_bar_height(),
            top_bar_margin_top: default_bar_margin_top(),
            top_bar_margin_horizontal: default_bar_margin_horizontal(),
            top_bar_radius: default_bar_radius(),
            panel_padding: default_panel_padding(),
            control_gap: default_control_gap(),
        }
    }
}

#[derive(Debug, Default, Deserialize)]
struct ThemeShadowConfig {
    #[serde(default)]
    soft: SoftShadowConfig,
}

#[derive(Debug, Deserialize)]
struct SoftShadowConfig {
    #[serde(default = "default_shadow_offset_y")]
    offset_y: f32,
    #[serde(default = "default_shadow_blur")]
    blur: f32,
    #[serde(default = "default_shadow_opacity")]
    opacity: f32,
}

impl Default for SoftShadowConfig {
    fn default() -> Self {
        Self {
            offset_y: default_shadow_offset_y(),
            blur: default_shadow_blur(),
            opacity: default_shadow_opacity(),
        }
    }
}

pub(crate) fn load() -> ShellConfig {
    let Some(path) = config_path() else {
        return ShellConfig::default();
    };
    let Ok(source) = fs::read_to_string(&path) else {
        return ShellConfig::default();
    };

    match toml::from_str::<FereseConfig>(&source) {
        Ok(config) => {
            let theme = shell_theme(&config.theme);

            ShellConfig {
                font_family: config.theme.typography.font_family,
                wallpaper: config.theme.background,
                theme,
            }
        }
        Err(error) => {
            eprintln!("ferese-shell: failed to read {}: {error}", path.display());
            ShellConfig::default()
        }
    }
}

fn shell_theme(theme: &ThemeConfig) -> ShellTheme {
    let defaults = ShellTheme::default();

    ShellTheme {
        surface_base: parse_color(&theme.colors.surface_base).unwrap_or(defaults.surface_base),
        text_primary: parse_color(&theme.colors.text_primary).unwrap_or(defaults.text_primary),
        text_muted: parse_color(&theme.colors.text_muted).unwrap_or(defaults.text_muted),
        accent: parse_color(&theme.colors.accent).unwrap_or(defaults.accent),
        border: parse_color(&theme.colors.border).unwrap_or(defaults.border),
        shadow: parse_color(&theme.colors.shadow).unwrap_or(defaults.shadow),
        bar_height: positive_or(theme.geometry.top_bar_height, defaults.bar_height),
        bar_margin_top: theme.geometry.top_bar_margin_top.max(0),
        bar_margin_horizontal: theme.geometry.top_bar_margin_horizontal.max(0),
        bar_radius: nonnegative_or(theme.geometry.top_bar_radius, defaults.bar_radius),
        panel_padding: nonnegative_or(theme.geometry.panel_padding, defaults.panel_padding),
        control_gap: nonnegative_or(theme.geometry.control_gap, defaults.control_gap),
        shadow_offset_y: finite_or(theme.shadow.soft.offset_y, defaults.shadow_offset_y),
        shadow_blur: nonnegative_or(theme.shadow.soft.blur, defaults.shadow_blur),
        shadow_opacity: theme.shadow.soft.opacity.clamp(0.0, 1.0),
    }
}

fn parse_color(value: &str) -> Option<[u8; 4]> {
    let value = value.strip_prefix('#')?;
    if value.len() != 6 && value.len() != 8 {
        return None;
    }

    let red = u8::from_str_radix(&value[0..2], 16).ok()?;
    let green = u8::from_str_radix(&value[2..4], 16).ok()?;
    let blue = u8::from_str_radix(&value[4..6], 16).ok()?;
    let alpha = if value.len() == 8 {
        u8::from_str_radix(&value[6..8], 16).ok()?
    } else {
        255
    };

    Some([red, green, blue, alpha])
}

fn positive_or(value: f32, fallback: f32) -> f32 {
    if value.is_finite() && value > 0.0 {
        value
    } else {
        fallback
    }
}

fn nonnegative_or(value: f32, fallback: f32) -> f32 {
    if value.is_finite() && value >= 0.0 {
        value
    } else {
        fallback
    }
}

fn finite_or(value: f32, fallback: f32) -> f32 {
    if value.is_finite() { value } else { fallback }
}

fn default_surface_base() -> String {
    "#111821".to_owned()
}
fn default_text_primary() -> String {
    "#F4F7FB".to_owned()
}
fn default_text_muted() -> String {
    "#7F8A98".to_owned()
}
fn default_accent() -> String {
    "#5B8CFF".to_owned()
}
fn default_border() -> String {
    "#FFFFFF18".to_owned()
}
fn default_shadow() -> String {
    "#00000055".to_owned()
}
const fn default_bar_height() -> f32 {
    38.0
}
const fn default_bar_margin_top() -> i32 {
    4
}
const fn default_bar_margin_horizontal() -> i32 {
    10
}
const fn default_bar_radius() -> f32 {
    19.0
}
const fn default_panel_padding() -> f32 {
    12.0
}
const fn default_control_gap() -> f32 {
    8.0
}
const fn default_shadow_offset_y() -> f32 {
    4.0
}
const fn default_shadow_blur() -> f32 {
    18.0
}
const fn default_shadow_opacity() -> f32 {
    0.20
}

pub(crate) const fn default_background() -> [u8; 3] {
    DEFAULT_BACKGROUND
}

fn config_path() -> Option<PathBuf> {
    if let Some(directory) = env::var_os("XDG_CONFIG_HOME").filter(|value| !value.is_empty()) {
        return Some(PathBuf::from(directory).join("ferese/config.toml"));
    }

    env::var_os("HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .map(|home| home.join(".config/ferese/config.toml"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_theme_without_rejecting_compositor_sections() {
        let config: FereseConfig = toml::from_str(
            r#"
                [layout]
                mode = "scrolling"

                [theme.typography]
                font_family = "JetBrainsMono Nerd Font"

                [theme.background]
                path = "/tmp/wallpaper.png"
                mode = "fit"
            "#,
        )
        .unwrap();

        assert_eq!(
            config.theme.typography.font_family.as_deref(),
            Some("JetBrainsMono Nerd Font")
        );
        assert_eq!(
            config.theme.background.path,
            Some(PathBuf::from("/tmp/wallpaper.png"))
        );
        assert_eq!(config.theme.background.mode, WallpaperMode::Fit);
    }

    #[test]
    fn defaults_to_the_ferese_visual_profile() {
        let config: FereseConfig = toml::from_str("").unwrap();

        assert_eq!(config.theme.typography.font_family, None);
        assert_eq!(config.theme.background.path, None);
        assert_eq!(config.theme.background.mode, WallpaperMode::Fill);
    }
}
