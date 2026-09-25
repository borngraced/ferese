use std::env;
use std::error::Error;
use std::fmt;
use std::fs;
use std::io;
use std::path::PathBuf;

use ferese_animation::SpringConfig;
use ferese_core::LayoutMode;
use ferese_layout::ColumnWidth;
use serde::Deserialize;

#[derive(Debug, Default, Deserialize)]
pub struct Config {
    #[serde(default)]
    animations: AnimationsConfig,
    #[serde(default)]
    layout: LayoutConfig,
    #[serde(default)]
    scrolling: ScrollingConfig,
}

#[derive(Debug, Deserialize)]
struct AnimationsConfig {
    #[serde(default = "enabled_by_default")]
    enabled: bool,
    #[serde(default)]
    reduced_motion: bool,
    #[serde(default = "default_animation_speed")]
    speed: f64,
    #[serde(default)]
    spring: SpringSettings,
}

impl Default for AnimationsConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            reduced_motion: false,
            speed: 1.0,
            spring: SpringSettings::default(),
        }
    }
}

#[derive(Debug, Deserialize)]
struct SpringSettings {
    #[serde(default = "default_spring_mass")]
    mass: f64,
    #[serde(default = "default_spring_stiffness")]
    stiffness: f64,
    #[serde(default = "default_spring_damping")]
    damping: f64,
}

impl Default for SpringSettings {
    fn default() -> Self {
        Self {
            mass: default_spring_mass(),
            stiffness: default_spring_stiffness(),
            damping: default_spring_damping(),
        }
    }
}

#[derive(Debug, Default, Deserialize)]
struct LayoutConfig {
    mode: Option<LayoutModeValue>,
}

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "lowercase")]
enum LayoutModeValue {
    Scrolling,
    Tree,
}

#[derive(Debug, Default, Deserialize)]
struct ScrollingConfig {
    default_column_width: Option<ColumnWidthValue>,
    #[serde(default)]
    width_presets: Vec<ColumnWidthValue>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(untagged)]
enum ColumnWidthValue {
    Proportion(f64),
    Named(String),
}

#[derive(Debug)]
pub enum ConfigError {
    Read {
        path: PathBuf,
        source: io::Error,
    },
    Parse {
        path: PathBuf,
        source: toml::de::Error,
    },
    InvalidColumnWidth {
        field: &'static str,
        value: String,
    },
    InvalidAnimationValue {
        field: &'static str,
        value: f64,
    },
}

impl fmt::Display for ConfigError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Read { path, source } => {
                write!(formatter, "failed to read {}: {source}", path.display())
            }
            Self::Parse { path, source } => {
                write!(formatter, "failed to parse {}: {source}", path.display())
            }
            Self::InvalidColumnWidth { field, value } => write!(
                formatter,
                "invalid scrolling.{field} {value}; expected a positive number or \"full\""
            ),
            Self::InvalidAnimationValue { field, value } => {
                write!(formatter, "invalid animations.{field} value {value}")
            }
        }
    }
}

impl Error for ConfigError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Read { source, .. } => Some(source),
            Self::Parse { source, .. } => Some(source),
            Self::InvalidColumnWidth { .. } | Self::InvalidAnimationValue { .. } => None,
        }
    }
}

impl Config {
    pub fn load() -> Result<Self, ConfigError> {
        let Some(path) = config_path() else {
            return Ok(Self::default());
        };
        let source = match fs::read_to_string(&path) {
            Ok(source) => source,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Self::default()),
            Err(source) => return Err(ConfigError::Read { path, source }),
        };

        toml::from_str(&source).map_err(|source| ConfigError::Parse { path, source })
    }

    pub fn layout_mode(&self) -> LayoutMode {
        match self.layout.mode.unwrap_or(LayoutModeValue::Scrolling) {
            LayoutModeValue::Scrolling => LayoutMode::Scrolling,
            LayoutModeValue::Tree => LayoutMode::Tree,
        }
    }

    pub fn default_column_width(&self) -> Result<ColumnWidth, ConfigError> {
        parse_column_width(
            self.scrolling
                .default_column_width
                .as_ref()
                .unwrap_or(&ColumnWidthValue::Proportion(0.5)),
            "default_column_width",
        )
    }

    pub fn width_presets(&self) -> Result<Vec<ColumnWidth>, ConfigError> {
        if self.scrolling.width_presets.is_empty() {
            return Ok(vec![
                ColumnWidth::Proportion(1.0 / 3.0),
                ColumnWidth::Proportion(0.5),
                ColumnWidth::Proportion(2.0 / 3.0),
                ColumnWidth::Full,
            ]);
        }

        self.scrolling
            .width_presets
            .iter()
            .map(|value| parse_column_width(value, "width_presets"))
            .collect()
    }

    pub fn animations_enabled(&self) -> bool {
        self.animations.enabled && !self.animations.reduced_motion
    }

    pub fn animation_speed(&self) -> Result<f64, ConfigError> {
        positive_animation_value(self.animations.speed, "speed")
    }

    pub fn spring_config(&self) -> Result<SpringConfig, ConfigError> {
        let mass = positive_animation_value(self.animations.spring.mass, "spring.mass")?;
        let stiffness =
            positive_animation_value(self.animations.spring.stiffness, "spring.stiffness")?;
        let damping = self.animations.spring.damping;
        if !damping.is_finite() || damping < 0.0 {
            return Err(ConfigError::InvalidAnimationValue {
                field: "spring.damping",
                value: damping,
            });
        }

        Ok(SpringConfig {
            mass,
            stiffness,
            damping,
            ..SpringConfig::default()
        })
    }
}

fn positive_animation_value(value: f64, field: &'static str) -> Result<f64, ConfigError> {
    if value.is_finite() && value > 0.0 {
        Ok(value)
    } else {
        Err(ConfigError::InvalidAnimationValue { field, value })
    }
}

const fn enabled_by_default() -> bool {
    true
}

const fn default_animation_speed() -> f64 {
    1.0
}

const fn default_spring_mass() -> f64 {
    1.0
}

const fn default_spring_stiffness() -> f64 {
    700.0
}

const fn default_spring_damping() -> f64 {
    53.0
}

fn parse_column_width(
    value: &ColumnWidthValue,
    field: &'static str,
) -> Result<ColumnWidth, ConfigError> {
    match value {
        ColumnWidthValue::Proportion(value) if value.is_finite() && *value > 0.0 => {
            Ok(ColumnWidth::Proportion(*value))
        }
        ColumnWidthValue::Named(value) if value.eq_ignore_ascii_case("full") => {
            Ok(ColumnWidth::Full)
        }
        value => Err(ConfigError::InvalidColumnWidth {
            field,
            value: match value {
                ColumnWidthValue::Proportion(value) => value.to_string(),
                ColumnWidthValue::Named(value) => format!("{value:?}"),
            },
        }),
    }
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

    fn parse(source: &str) -> Config {
        toml::from_str(source).unwrap()
    }

    #[test]
    fn defaults_to_half_width_scrolling_columns() {
        let config = parse("");

        assert_eq!(config.layout_mode(), LayoutMode::Scrolling);
        assert_eq!(
            config.default_column_width().unwrap(),
            ColumnWidth::Proportion(0.5)
        );
    }

    #[test]
    fn accepts_full_and_numeric_column_widths() {
        let full = parse("[scrolling]\ndefault_column_width = \"full\"");
        let numeric = parse("[scrolling]\ndefault_column_width = 1.0");

        assert_eq!(full.default_column_width().unwrap(), ColumnWidth::Full);
        assert_eq!(
            numeric.default_column_width().unwrap(),
            ColumnWidth::Proportion(1.0)
        );
    }

    #[test]
    fn parses_width_presets_and_supplies_defaults() {
        let defaults = parse("");
        let configured = parse("[scrolling]\nwidth_presets = [0.5, 1.0, \"full\"]");

        assert_eq!(defaults.width_presets().unwrap().len(), 4);
        assert_eq!(
            configured.width_presets().unwrap(),
            vec![
                ColumnWidth::Proportion(0.5),
                ColumnWidth::Proportion(1.0),
                ColumnWidth::Full
            ]
        );
    }

    #[test]
    fn rejects_non_positive_or_unknown_column_widths() {
        let zero = parse("[scrolling]\ndefault_column_width = 0.0");
        let unknown = parse("[scrolling]\ndefault_column_width = \"wide\"");

        assert!(zero.default_column_width().is_err());
        assert!(unknown.default_column_width().is_err());
    }

    #[test]
    fn parses_animation_policy_and_reduced_motion() {
        let config = parse(
            "[animations]\nspeed = 1.5\nreduced_motion = true\n\n[animations.spring]\nmass = 2.0\nstiffness = 500.0\ndamping = 40.0",
        );

        assert!(!config.animations_enabled());
        assert_eq!(config.animation_speed().unwrap(), 1.5);
        assert_eq!(
            config.spring_config().unwrap(),
            SpringConfig {
                mass: 2.0,
                stiffness: 500.0,
                damping: 40.0,
                ..SpringConfig::default()
            }
        );
    }

    #[test]
    fn rejects_invalid_animation_numbers() {
        let speed = parse("[animations]\nspeed = 0.0");
        let damping = parse("[animations.spring]\ndamping = -1.0");

        assert!(speed.animation_speed().is_err());
        assert!(damping.spring_config().is_err());
    }
}
