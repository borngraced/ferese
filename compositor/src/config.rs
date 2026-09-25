use std::env;
use std::error::Error;
use std::fmt;
use std::fs;
use std::io;
use std::path::PathBuf;

use ferese_core::LayoutMode;
use ferese_layout::ColumnWidth;
use serde::Deserialize;

#[derive(Debug, Default, Deserialize)]
pub struct Config {
    #[serde(default)]
    layout: LayoutConfig,
    #[serde(default)]
    scrolling: ScrollingConfig,
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
    InvalidColumnWidth(String),
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
            Self::InvalidColumnWidth(value) => write!(
                formatter,
                "invalid scrolling.default_column_width {value}; expected a positive number or \"full\""
            ),
        }
    }
}

impl Error for ConfigError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Read { source, .. } => Some(source),
            Self::Parse { source, .. } => Some(source),
            Self::InvalidColumnWidth(_) => None,
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
        match self
            .scrolling
            .default_column_width
            .as_ref()
            .unwrap_or(&ColumnWidthValue::Proportion(0.5))
        {
            ColumnWidthValue::Proportion(value) if value.is_finite() && *value > 0.0 => {
                Ok(ColumnWidth::Proportion(*value))
            }
            ColumnWidthValue::Named(value) if value.eq_ignore_ascii_case("full") => {
                Ok(ColumnWidth::Full)
            }
            value => Err(ConfigError::InvalidColumnWidth(match value {
                ColumnWidthValue::Proportion(value) => value.to_string(),
                ColumnWidthValue::Named(value) => format!("{value:?}"),
            })),
        }
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
    fn rejects_non_positive_or_unknown_column_widths() {
        let zero = parse("[scrolling]\ndefault_column_width = 0.0");
        let unknown = parse("[scrolling]\ndefault_column_width = \"wide\"");

        assert!(zero.default_column_width().is_err());
        assert!(unknown.default_column_width().is_err());
    }
}
