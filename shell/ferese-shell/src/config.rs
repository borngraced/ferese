use std::env;
use std::fs;
use std::path::PathBuf;

use serde::Deserialize;

const DEFAULT_BACKGROUND: [u8; 3] = [11, 15, 20];

#[derive(Clone, Debug, Default)]
pub(crate) struct ShellConfig {
    pub(crate) font_family: Option<String>,
    pub(crate) wallpaper: WallpaperConfig,
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
    typography: TypographyConfig,
    #[serde(default)]
    background: WallpaperConfig,
}

#[derive(Debug, Default, Deserialize)]
struct TypographyConfig {
    font_family: Option<String>,
}

pub(crate) fn load() -> ShellConfig {
    let Some(path) = config_path() else {
        return ShellConfig::default();
    };
    let Ok(source) = fs::read_to_string(&path) else {
        return ShellConfig::default();
    };

    match toml::from_str::<FereseConfig>(&source) {
        Ok(config) => ShellConfig {
            font_family: config.theme.typography.font_family,
            wallpaper: config.theme.background,
        },
        Err(error) => {
            eprintln!("ferese-shell: failed to read {}: {error}", path.display());
            ShellConfig::default()
        }
    }
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
