use std::env;
use std::error::Error;
use std::fmt;
use std::fs;
use std::io;
use std::path::PathBuf;

use ferese_animation::SpringConfig;
use ferese_core::LayoutMode;
use ferese_layout::{ColumnWidth, GapConfig, ViewportFocusStrategy};
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
    #[serde(default)]
    viewport_spring: ViewportSpringSettings,
}

impl Default for AnimationsConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            reduced_motion: false,
            speed: 1.0,
            spring: SpringSettings::default(),
            viewport_spring: ViewportSpringSettings::default(),
        }
    }
}

#[derive(Debug, Deserialize)]
struct ViewportSpringSettings {
    #[serde(default = "default_viewport_mass")]
    mass: f64,
    #[serde(default = "default_viewport_stiffness")]
    stiffness: f64,
    #[serde(default = "default_viewport_damping_ratio")]
    damping_ratio: f64,
}

impl Default for ViewportSpringSettings {
    fn default() -> Self {
        Self {
            mass: default_viewport_mass(),
            stiffness: default_viewport_stiffness(),
            damping_ratio: default_viewport_damping_ratio(),
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

#[derive(Debug, Deserialize)]
struct LayoutConfig {
    mode: Option<LayoutModeValue>,
    #[serde(default = "default_inner_gap")]
    inner_gap: f64,
    #[serde(default = "default_outer_gap")]
    outer_gap: f64,
    #[serde(default)]
    smart_gaps: bool,
}

impl Default for LayoutConfig {
    fn default() -> Self {
        Self {
            mode: None,
            inner_gap: default_inner_gap(),
            outer_gap: default_outer_gap(),
            smart_gaps: false,
        }
    }
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
    focus_strategy: Option<FocusStrategyValue>,
    #[serde(default)]
    width_presets: Vec<ColumnWidthValue>,
}

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
enum FocusStrategyValue {
    Minimal,
    CenterOnFocus,
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
    InvalidLayoutValue {
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
            Self::InvalidLayoutValue { field, value } => {
                write!(formatter, "invalid layout.{field} value {value}")
            }
        }
    }
}

impl Error for ConfigError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Read { source, .. } => Some(source),
            Self::Parse { source, .. } => Some(source),
            Self::InvalidColumnWidth { .. }
            | Self::InvalidAnimationValue { .. }
            | Self::InvalidLayoutValue { .. } => None,
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

    pub fn gap_config(&self) -> Result<GapConfig, ConfigError> {
        let inner = nonnegative_layout_value(self.layout.inner_gap, "inner_gap")?;
        let outer = nonnegative_layout_value(self.layout.outer_gap, "outer_gap")?;

        Ok(GapConfig {
            inner,
            outer,
            smart: self.layout.smart_gaps,
        })
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

    pub fn scrolling_focus_strategy(&self) -> ViewportFocusStrategy {
        match self
            .scrolling
            .focus_strategy
            .unwrap_or(FocusStrategyValue::Minimal)
        {
            FocusStrategyValue::Minimal => ViewportFocusStrategy::Minimal,
            FocusStrategyValue::CenterOnFocus => ViewportFocusStrategy::Center,
        }
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

    pub fn viewport_spring_config(&self) -> Result<SpringConfig, ConfigError> {
        let mass =
            positive_animation_value(self.animations.viewport_spring.mass, "viewport_spring.mass")?;
        let stiffness = positive_animation_value(
            self.animations.viewport_spring.stiffness,
            "viewport_spring.stiffness",
        )?;
        let damping_ratio = positive_animation_value(
            self.animations.viewport_spring.damping_ratio,
            "viewport_spring.damping_ratio",
        )?;

        Ok(SpringConfig {
            mass,
            stiffness,
            damping: 2.0 * damping_ratio * (stiffness * mass).sqrt(),
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

fn nonnegative_layout_value(value: f64, field: &'static str) -> Result<f64, ConfigError> {
    if value.is_finite() && value >= 0.0 {
        Ok(value)
    } else {
        Err(ConfigError::InvalidLayoutValue { field, value })
    }
}

const fn enabled_by_default() -> bool {
    true
}

const fn default_inner_gap() -> f64 {
    10.0
}

const fn default_outer_gap() -> f64 {
    10.0
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

const fn default_viewport_mass() -> f64 {
    1.0
}

const fn default_viewport_stiffness() -> f64 {
    320.0
}

const fn default_viewport_damping_ratio() -> f64 {
    1.0
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
        assert_eq!(config.gap_config().unwrap(), GapConfig::default());
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
    fn parses_scrolling_focus_strategy() {
        let minimal = parse("");
        let centered = parse("[scrolling]\nfocus_strategy = \"center_on_focus\"");

        assert_eq!(
            minimal.scrolling_focus_strategy(),
            ViewportFocusStrategy::Minimal
        );
        assert_eq!(
            centered.scrolling_focus_strategy(),
            ViewportFocusStrategy::Center
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

    #[test]
    fn parses_and_validates_layout_gaps() {
        let configured = parse("[layout]\ninner_gap = 6.0\nouter_gap = 14.0\nsmart_gaps = true");
        let invalid = parse("[layout]\nouter_gap = -1.0");

        assert_eq!(
            configured.gap_config().unwrap(),
            GapConfig {
                inner: 6.0,
                outer: 14.0,
                smart: true,
            }
        );
        assert!(invalid.gap_config().is_err());
    }

    #[test]
    fn derives_critical_viewport_damping() {
        let config = parse("");
        let spring = config.viewport_spring_config().unwrap();

        assert_eq!(spring.mass, 1.0);
        assert_eq!(spring.stiffness, 320.0);
        assert!((spring.damping - 35.777_087_64).abs() < 0.000_001);
    }
}
