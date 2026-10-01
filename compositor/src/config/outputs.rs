use super::*;

impl Config {
    pub fn output_profiles(&self) -> Result<Vec<OutputProfile>, ConfigError> {
        let mut names = HashSet::new();

        self.output_profiles
            .iter()
            .map(|profile| {
                let name = profile.name.trim();
                if name.is_empty() {
                    return Err(ConfigError::InvalidOutputProfile(
                        "profile names cannot be empty".to_owned(),
                    ));
                }
                if !names.insert(name.to_owned()) {
                    return Err(ConfigError::InvalidOutputProfile(format!(
                        "duplicate profile name {name:?}"
                    )));
                }
                if profile.outputs.is_empty() {
                    return Err(ConfigError::InvalidOutputProfile(format!(
                        "profile {name:?} has no outputs"
                    )));
                }

                let mut matchers = HashSet::new();
                let outputs = profile
                    .outputs
                    .iter()
                    .map(|output| {
                        let matcher = output.matcher.trim();
                        if matcher.is_empty() {
                            return Err(ConfigError::InvalidOutputProfile(format!(
                                "profile {name:?} contains an empty output matcher"
                            )));
                        }
                        if !matchers.insert(matcher.to_owned()) {
                            return Err(ConfigError::InvalidOutputProfile(format!(
                                "profile {name:?} repeats output matcher {matcher:?}"
                            )));
                        }
                        if !output.scale.is_finite() || output.scale <= 0.0 {
                            return Err(ConfigError::InvalidOutputProfile(format!(
                                "profile {name:?} output {matcher:?} has invalid scale {}",
                                output.scale
                            )));
                        }

                        Ok(OutputSettings {
                            auto_refresh: output.auto_refresh,
                            matcher: matcher.to_owned(),
                            enabled: output.enabled,
                            mode: output
                                .mode
                                .as_deref()
                                .map(parse_output_mode)
                                .transpose()
                                .map_err(ConfigError::InvalidOutputProfile)?,
                            scale: output.scale,
                            transform: output.transform,
                            position: output.position,
                        })
                    })
                    .collect::<Result<Vec<_>, ConfigError>>()?;

                Ok(OutputProfile {
                    name: name.to_owned(),
                    outputs,
                })
            })
            .collect()
    }
}

pub(super) fn parse_output_mode(value: &str) -> Result<OutputModeRequest, String> {
    let (size, refresh) = value
        .trim()
        .split_once('@')
        .map_or((value.trim(), None), |(size, refresh)| (size, Some(refresh)));
    let (width, height) = size
        .split_once('x')
        .ok_or_else(|| format!("mode {value:?} must use WIDTHxHEIGHT or WIDTHxHEIGHT@REFRESH"))?;
    let width = width
        .parse::<u16>()
        .map_err(|_| format!("mode {value:?} has an invalid width"))?;
    let height = height
        .parse::<u16>()
        .map_err(|_| format!("mode {value:?} has an invalid height"))?;
    if width == 0 || height == 0 {
        return Err(format!("mode {value:?} dimensions must be positive"));
    }
    let refresh_millihertz = refresh
        .map(|refresh| {
            let hertz = refresh
                .parse::<f64>()
                .map_err(|_| format!("mode {value:?} has an invalid refresh rate"))?;
            if !hertz.is_finite() || hertz <= 0.0 || hertz > u32::MAX as f64 / 1_000.0 {
                return Err(format!("mode {value:?} has an invalid refresh rate"));
            }

            Ok((hertz * 1_000.0).round() as u32)
        })
        .transpose()?;

    Ok(OutputModeRequest {
        width,
        height,
        refresh_millihertz,
    })
}

pub(super) const fn default_output_scale() -> f64 {
    1.0
}

#[derive(Clone, Debug, PartialEq)]
pub struct OutputProfile {
    pub name: String,
    pub outputs: Vec<OutputSettings>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct OutputSettings {
    pub auto_refresh: bool,
    pub matcher: String,
    pub enabled: bool,
    pub mode: Option<OutputModeRequest>,
    pub scale: f64,
    pub transform: OutputTransform,
    pub position: Option<[i32; 2]>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct OutputModeRequest {
    pub width: u16,
    pub height: u16,
    pub refresh_millihertz: Option<u32>,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum OutputTransform {
    #[default]
    Normal,
    #[serde(rename = "rotate_90")]
    Rotate90,
    #[serde(rename = "rotate_180")]
    Rotate180,
    #[serde(rename = "rotate_270")]
    Rotate270,
    Flipped,
    #[serde(rename = "flipped_90")]
    Flipped90,
    #[serde(rename = "flipped_180")]
    Flipped180,
    #[serde(rename = "flipped_270")]
    Flipped270,
}

#[derive(Clone, Debug, Deserialize)]
pub(super) struct OutputProfileConfig {
    pub(super) name: String,
    #[serde(default)]
    pub(super) outputs: Vec<OutputConfig>,
}

#[derive(Clone, Debug, Deserialize)]
pub(super) struct OutputConfig {
    #[serde(default)]
    pub(super) auto_refresh: bool,
    #[serde(rename = "match")]
    pub(super) matcher: String,
    #[serde(default = "default_true")]
    pub(super) enabled: bool,
    pub(super) mode: Option<String>,
    #[serde(default = "default_output_scale")]
    pub(super) scale: f64,
    #[serde(default)]
    pub(super) transform: OutputTransform,
    pub(super) position: Option<[i32; 2]>,
}
