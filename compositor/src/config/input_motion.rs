use super::*;

impl Config {
    pub fn input_settings(&self) -> Result<InputSettings, ConfigError> {
        if self.input.xkb_layout.trim().is_empty() {
            return Err(ConfigError::InvalidInputValue {
                field: "xkb_layout",
                value: self.input.xkb_layout.clone(),
            });
        }
        if self.input.repeat_rate <= 0 {
            return Err(ConfigError::InvalidInputValue {
                field: "repeat_rate",
                value: self.input.repeat_rate.to_string(),
            });
        }
        if self.input.repeat_delay_ms < 0 {
            return Err(ConfigError::InvalidInputValue {
                field: "repeat_delay_ms",
                value: self.input.repeat_delay_ms.to_string(),
            });
        }

        if !(16..=1000).contains(&self.input.touchpad.swipe_threshold) {
            return Err(ConfigError::InvalidInputValue {
                field: "touchpad.swipe_threshold",
                value: self.input.touchpad.swipe_threshold.to_string(),
            });
        }
        Ok(InputSettings {
            focus_follows_mouse: self.input.focus_follows_mouse,
            xkb_layout: self.input.xkb_layout.clone(),
            xkb_variant: self.input.xkb_variant.clone(),
            xkb_options: self.input.xkb_options.clone(),
            repeat_rate: self.input.repeat_rate,
            repeat_delay_ms: self.input.repeat_delay_ms,
            touchpad: TouchpadSettings {
                tap: self.input.touchpad.tap,
                natural_scroll: self.input.touchpad.natural_scroll,
                disable_while_typing: self.input.touchpad.disable_while_typing,
                swipe_threshold: self.input.touchpad.swipe_threshold,
            },
        })
    }

    pub fn animations_enabled(&self) -> bool {
        self.animations.enabled && !self.animations.reduced_motion
    }

    pub fn animation_speed(&self) -> Result<f64, ConfigError> {
        positive_animation_value(self.animations.speed, "speed")
    }

    pub fn spring_config(&self) -> Result<SpringConfig, ConfigError> {
        let mass = positive_animation_value(self.animations.spring.mass, "spring.mass")?;
        let stiffness = positive_animation_value(self.animations.spring.stiffness, "spring.stiffness")?;
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
        let mass = positive_animation_value(self.animations.viewport_spring.mass, "viewport_spring.mass")?;
        let stiffness =
            positive_animation_value(self.animations.viewport_spring.stiffness, "viewport_spring.stiffness")?;
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

pub(super) fn positive_animation_value(value: f64, field: &'static str) -> Result<f64, ConfigError> {
    if value.is_finite() && value > 0.0 {
        Ok(value)
    } else {
        Err(ConfigError::InvalidAnimationValue { field, value })
    }
}

pub(super) fn default_xkb_layout() -> String {
    "us".to_owned()
}

#[derive(Clone, Debug, PartialEq)]
pub struct InputSettings {
    pub focus_follows_mouse: bool,
    pub xkb_layout: String,
    pub xkb_variant: String,
    pub xkb_options: Vec<String>,
    pub repeat_rate: i32,
    pub repeat_delay_ms: i32,
    pub touchpad: TouchpadSettings,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TouchpadSettings {
    pub tap: bool,
    pub natural_scroll: bool,
    pub disable_while_typing: bool,
    pub swipe_threshold: u16,
}

fn default_swipe_threshold() -> u16 {
    80
}

#[derive(Debug, Deserialize)]
pub(super) struct InputConfig {
    #[serde(default)]
    pub(super) focus_follows_mouse: bool,
    #[serde(default = "default_xkb_layout")]
    pub(super) xkb_layout: String,
    #[serde(default)]
    pub(super) xkb_variant: String,
    #[serde(default)]
    pub(super) xkb_options: Vec<String>,
    #[serde(default = "default_repeat_rate")]
    pub(super) repeat_rate: i32,
    #[serde(default = "default_repeat_delay")]
    pub(super) repeat_delay_ms: i32,
    #[serde(default)]
    pub(super) touchpad: TouchpadConfig,
}

impl Default for InputConfig {
    fn default() -> Self {
        Self {
            focus_follows_mouse: false,
            xkb_layout: default_xkb_layout(),
            xkb_variant: String::new(),
            xkb_options: Vec::new(),
            repeat_rate: default_repeat_rate(),
            repeat_delay_ms: default_repeat_delay(),
            touchpad: TouchpadConfig::default(),
        }
    }
}

#[derive(Debug, Deserialize)]
pub(super) struct TouchpadConfig {
    #[serde(default = "enabled_by_default")]
    pub(super) tap: bool,
    #[serde(default = "enabled_by_default")]
    pub(super) natural_scroll: bool,
    #[serde(default = "enabled_by_default")]
    pub(super) disable_while_typing: bool,
    #[serde(default = "default_swipe_threshold")]
    pub(super) swipe_threshold: u16,
}

impl Default for TouchpadConfig {
    fn default() -> Self {
        Self {
            tap: true,
            natural_scroll: true,
            disable_while_typing: true,
            swipe_threshold: default_swipe_threshold(),
        }
    }
}

#[derive(Debug, Deserialize)]
pub(super) struct AnimationsConfig {
    #[serde(default = "enabled_by_default")]
    pub(super) enabled: bool,
    #[serde(default)]
    pub(super) reduced_motion: bool,
    #[serde(default = "default_animation_speed")]
    pub(super) speed: f64,
    #[serde(default)]
    pub(super) spring: SpringSettings,
    #[serde(default)]
    pub(super) viewport_spring: ViewportSpringSettings,
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
pub(super) struct ViewportSpringSettings {
    #[serde(default = "default_viewport_mass")]
    pub(super) mass: f64,
    #[serde(default = "default_viewport_stiffness")]
    pub(super) stiffness: f64,
    #[serde(default = "default_viewport_damping_ratio")]
    pub(super) damping_ratio: f64,
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
pub(super) struct SpringSettings {
    #[serde(default = "default_spring_mass")]
    pub(super) mass: f64,
    #[serde(default = "default_spring_stiffness")]
    pub(super) stiffness: f64,
    #[serde(default = "default_spring_damping")]
    pub(super) damping: f64,
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
