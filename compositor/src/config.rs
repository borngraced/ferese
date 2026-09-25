use std::collections::{HashMap, HashSet};
use std::env;
use std::error::Error;
use std::fmt;
use std::fs;
use std::io;
use std::path::PathBuf;

use ferese_animation::SpringConfig;
use ferese_core::LayoutMode;
use ferese_layout::{ColumnWidth, Direction, GapConfig, ViewportFocusStrategy};
use serde::Deserialize;
use smithay::input::keyboard::{Keycode, keysyms, xkb};

use crate::window_rules::{self, WindowRule, WindowRuleConfig};

#[derive(Debug, Default, Deserialize)]
pub struct Config {
    #[serde(default)]
    animations: AnimationsConfig,
    #[serde(default)]
    layout: LayoutConfig,
    #[serde(default)]
    input: InputConfig,
    #[serde(default)]
    theme: ThemeConfig,
    #[serde(default)]
    commands: HashMap<String, Vec<String>>,
    #[serde(default)]
    bindings: Vec<BindingConfig>,
    #[serde(default)]
    window_rules: Vec<WindowRuleConfig>,
    #[serde(default)]
    scrolling: ScrollingConfig,
    #[serde(default)]
    output_profiles: Vec<OutputProfileConfig>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct OutputProfile {
    pub name: String,
    pub outputs: Vec<OutputSettings>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct OutputSettings {
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
struct OutputProfileConfig {
    name: String,
    #[serde(default)]
    outputs: Vec<OutputConfig>,
}

#[derive(Clone, Debug, Deserialize)]
struct OutputConfig {
    #[serde(rename = "match")]
    matcher: String,
    #[serde(default = "default_true")]
    enabled: bool,
    mode: Option<String>,
    #[serde(default = "default_output_scale")]
    scale: f64,
    #[serde(default)]
    transform: OutputTransform,
    position: Option<[i32; 2]>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RgbaColor(pub [f32; 4]);

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ThemeSettings {
    pub border_width: f64,
    pub focus_ring_width: f64,
    pub border_color: RgbaColor,
    pub accent_color: RgbaColor,
    pub shadow_color: RgbaColor,
    pub window_radius: f64,
    pub shadow_offset_y: f64,
    pub shadow_blur: f64,
    pub shadow_opacity: f64,
}

#[derive(Debug, Default, Deserialize)]
struct ThemeConfig {
    #[serde(default)]
    colors: ThemeColorsConfig,
    #[serde(default)]
    geometry: ThemeGeometryConfig,
    #[serde(default)]
    shadow: ThemeShadowConfig,
}

#[derive(Debug, Deserialize)]
struct ThemeColorsConfig {
    #[serde(default = "default_border_color")]
    border: String,
    #[serde(default = "default_accent_color")]
    accent: String,
    #[serde(default = "default_shadow_color")]
    shadow: String,
}

impl Default for ThemeColorsConfig {
    fn default() -> Self {
        Self {
            border: default_border_color(),
            accent: default_accent_color(),
            shadow: default_shadow_color(),
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
    offset_y: f64,
    #[serde(default = "default_shadow_blur")]
    blur: f64,
    #[serde(default = "default_shadow_opacity")]
    opacity: f64,
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

#[derive(Debug, Deserialize)]
struct ThemeGeometryConfig {
    #[serde(default = "default_border_width")]
    border_width: f64,
    #[serde(default = "default_focus_ring_width")]
    focus_ring_width: f64,
    #[serde(default = "default_window_radius")]
    window_radius: f64,
}

impl Default for ThemeGeometryConfig {
    fn default() -> Self {
        Self {
            border_width: default_border_width(),
            focus_ring_width: default_focus_ring_width(),
            window_radius: default_window_radius(),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Binding {
    modifiers: BindingModifiers,
    trigger: BindingTrigger,
    pub action: BindingAction,
}

#[derive(Clone, Debug, PartialEq)]
enum BindingTrigger {
    Keysym(u32),
    Physical(Keycode),
}

#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
struct BindingModifiers {
    logo: bool,
    ctrl: bool,
    alt: bool,
    shift: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub enum BindingAction {
    Spawn(Vec<String>),
    Close,
    Focus(ferese_layout::Direction),
    Move(ferese_layout::Direction),
    Resize(ferese_layout::Direction),
    SwitchWorkspace(u8),
    MoveToWorkspace(u8),
    ToggleFullscreen,
    ToggleLayout,
    CycleColumnWidth,
    CenterColumn,
    Consume,
    Expel,
    ToggleFloating,
}

impl Binding {
    pub fn matches(
        &self,
        keycode: Keycode,
        keysyms: &[u32],
        logo: bool,
        ctrl: bool,
        alt: bool,
        shift: bool,
    ) -> bool {
        self.modifiers
            == BindingModifiers {
                logo,
                ctrl,
                alt,
                shift,
            }
            && match self.trigger {
                BindingTrigger::Keysym(expected) => keysyms.contains(&expected),
                BindingTrigger::Physical(expected) => expected == keycode,
            }
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, Hash, PartialEq)]
#[serde(rename_all = "lowercase")]
enum BindingMatch {
    #[default]
    Keysym,
    Physical,
}

#[derive(Clone, Debug, Deserialize)]
struct BindingConfig {
    keys: String,
    #[serde(default, rename = "match")]
    match_mode: BindingMatch,
    action: Option<String>,
    argument: Option<String>,
    #[serde(default)]
    disabled: bool,
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
}

#[derive(Debug, Deserialize)]
struct InputConfig {
    #[serde(default)]
    focus_follows_mouse: bool,
    #[serde(default = "default_xkb_layout")]
    xkb_layout: String,
    #[serde(default)]
    xkb_variant: String,
    #[serde(default)]
    xkb_options: Vec<String>,
    #[serde(default = "default_repeat_rate")]
    repeat_rate: i32,
    #[serde(default = "default_repeat_delay")]
    repeat_delay_ms: i32,
    #[serde(default)]
    touchpad: TouchpadConfig,
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
struct TouchpadConfig {
    #[serde(default = "enabled_by_default")]
    tap: bool,
    #[serde(default = "enabled_by_default")]
    natural_scroll: bool,
    #[serde(default = "enabled_by_default")]
    disable_while_typing: bool,
}

impl Default for TouchpadConfig {
    fn default() -> Self {
        Self {
            tap: true,
            natural_scroll: true,
            disable_while_typing: true,
        }
    }
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
    InvalidInputValue {
        field: &'static str,
        value: String,
    },
    InvalidBinding(String),
    InvalidWindowRule(String),
    InvalidThemeValue {
        field: &'static str,
        value: String,
    },
    InvalidOutputProfile(String),
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
            Self::InvalidInputValue { field, value } => {
                write!(formatter, "invalid input.{field} value {value}")
            }
            Self::InvalidBinding(message) => write!(formatter, "invalid binding: {message}"),
            Self::InvalidWindowRule(message) => write!(formatter, "invalid window rule: {message}"),
            Self::InvalidThemeValue { field, value } => {
                write!(formatter, "invalid theme.{field} value {value}")
            }
            Self::InvalidOutputProfile(message) => {
                write!(formatter, "invalid output profile: {message}")
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
            | Self::InvalidLayoutValue { .. }
            | Self::InvalidInputValue { .. }
            | Self::InvalidBinding(_)
            | Self::InvalidWindowRule(_)
            | Self::InvalidThemeValue { .. }
            | Self::InvalidOutputProfile(_) => None,
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
            },
        })
    }

    pub fn bindings(&self, input: &InputSettings) -> Result<Vec<Binding>, ConfigError> {
        let mut commands = HashMap::from([("terminal".to_owned(), vec!["foot".to_owned()])]);
        commands.extend(self.commands.clone());
        validate_commands(&commands)?;

        let keymap = physical_keymap(input)?;
        let mut bindings = default_bindings()
            .into_iter()
            .map(|binding| parse_binding(&binding, &commands, &keymap))
            .collect::<Result<Vec<_>, _>>()?;
        let mut supplied = HashSet::new();

        for configured in &self.bindings {
            let identity = binding_identity(configured, &keymap)?;
            if !supplied.insert(identity.clone()) {
                return Err(ConfigError::InvalidBinding(format!(
                    "duplicate binding {:?}",
                    configured.keys
                )));
            }

            bindings.retain(|binding| binding_identity_for_runtime(binding) != identity);
            if configured.disabled {
                if configured.action.is_some() || configured.argument.is_some() {
                    return Err(ConfigError::InvalidBinding(format!(
                        "disabled binding {:?} cannot have an action or argument",
                        configured.keys
                    )));
                }
            } else {
                bindings.push(parse_binding(configured, &commands, &keymap)?);
            }
        }

        Ok(bindings)
    }

    pub fn window_rules(&self) -> Result<Vec<WindowRule>, ConfigError> {
        window_rules::validate(&self.window_rules).map_err(ConfigError::InvalidWindowRule)
    }

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

    pub fn theme_settings(&self) -> Result<ThemeSettings, ConfigError> {
        let border_width =
            nonnegative_theme_value(self.theme.geometry.border_width, "geometry.border_width")?;
        let focus_ring_width = nonnegative_theme_value(
            self.theme.geometry.focus_ring_width,
            "geometry.focus_ring_width",
        )?;
        let window_radius =
            nonnegative_theme_value(self.theme.geometry.window_radius, "geometry.window_radius")?;
        let shadow_offset_y =
            finite_theme_value(self.theme.shadow.soft.offset_y, "shadow.soft.offset_y")?;
        let shadow_blur = nonnegative_theme_value(self.theme.shadow.soft.blur, "shadow.soft.blur")?;
        let shadow_opacity =
            unit_theme_value(self.theme.shadow.soft.opacity, "shadow.soft.opacity")?;

        Ok(ThemeSettings {
            border_width,
            focus_ring_width,
            border_color: parse_color(&self.theme.colors.border, "colors.border")?,
            accent_color: parse_color(&self.theme.colors.accent, "colors.accent")?,
            shadow_color: parse_color(&self.theme.colors.shadow, "colors.shadow")?,
            window_radius,
            shadow_offset_y,
            shadow_blur,
            shadow_opacity,
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

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct BindingIdentity {
    modifiers: BindingModifiers,
    match_mode: BindingMatch,
    key: u32,
}

fn validate_commands(commands: &HashMap<String, Vec<String>>) -> Result<(), ConfigError> {
    for (name, argv) in commands {
        if name.trim().is_empty() {
            return Err(ConfigError::InvalidBinding(
                "command names cannot be empty".to_owned(),
            ));
        }
        if argv.is_empty() || argv[0].is_empty() {
            return Err(ConfigError::InvalidBinding(format!(
                "command {name:?} must contain a program"
            )));
        }
    }

    Ok(())
}

fn physical_keymap(input: &InputSettings) -> Result<xkb::Keymap, ConfigError> {
    let context = xkb::Context::new(xkb::CONTEXT_NO_FLAGS);
    let options = (!input.xkb_options.is_empty()).then(|| input.xkb_options.join(","));

    xkb::Keymap::new_from_names(
        &context,
        "",
        "",
        &input.xkb_layout,
        &input.xkb_variant,
        options,
        xkb::KEYMAP_COMPILE_NO_FLAGS,
    )
    .ok_or_else(|| ConfigError::InvalidBinding("failed to compile the XKB keymap".to_owned()))
}

fn parse_binding(
    configured: &BindingConfig,
    commands: &HashMap<String, Vec<String>>,
    keymap: &xkb::Keymap,
) -> Result<Binding, ConfigError> {
    if configured.disabled {
        return Err(ConfigError::InvalidBinding(format!(
            "disabled binding {:?} cannot be executed",
            configured.keys
        )));
    }

    let identity = binding_identity(configured, keymap)?;
    let action = configured.action.as_deref().ok_or_else(|| {
        ConfigError::InvalidBinding(format!("binding {:?} has no action", configured.keys))
    })?;
    let action = parse_action(action, configured.argument.as_deref(), commands)?;
    let trigger = match identity.match_mode {
        BindingMatch::Keysym => BindingTrigger::Keysym(identity.key),
        BindingMatch::Physical => BindingTrigger::Physical(Keycode::new(identity.key)),
    };

    Ok(Binding {
        modifiers: identity.modifiers,
        trigger,
        action,
    })
}

fn binding_identity(
    configured: &BindingConfig,
    keymap: &xkb::Keymap,
) -> Result<BindingIdentity, ConfigError> {
    let (modifiers, key) = parse_chord(&configured.keys)?;
    let key = match configured.match_mode {
        BindingMatch::Keysym => parse_keysym(&key)?,
        BindingMatch::Physical => keymap
            .key_by_name(&key.to_ascii_uppercase())
            .map(Keycode::raw)
            .ok_or_else(|| {
                ConfigError::InvalidBinding(format!(
                    "unknown XKB physical key name {key:?} in {:?}",
                    configured.keys
                ))
            })?,
    };

    Ok(BindingIdentity {
        modifiers,
        match_mode: configured.match_mode,
        key,
    })
}

fn binding_identity_for_runtime(binding: &Binding) -> BindingIdentity {
    let (match_mode, key) = match binding.trigger {
        BindingTrigger::Keysym(key) => (BindingMatch::Keysym, key),
        BindingTrigger::Physical(key) => (BindingMatch::Physical, key.raw()),
    };

    BindingIdentity {
        modifiers: binding.modifiers,
        match_mode,
        key,
    }
}

fn parse_chord(chord: &str) -> Result<(BindingModifiers, String), ConfigError> {
    let mut modifiers = BindingModifiers::default();
    let mut key = None;

    for component in chord.split('+').map(str::trim) {
        if component.is_empty() {
            return Err(ConfigError::InvalidBinding(format!(
                "invalid key chord {chord:?}"
            )));
        }

        let slot = match component.to_ascii_lowercase().as_str() {
            "super" | "logo" | "mod4" => Some(&mut modifiers.logo),
            "ctrl" | "control" => Some(&mut modifiers.ctrl),
            "alt" => Some(&mut modifiers.alt),
            "shift" => Some(&mut modifiers.shift),
            _ => None,
        };
        if let Some(slot) = slot {
            if *slot {
                return Err(ConfigError::InvalidBinding(format!(
                    "duplicate modifier in {chord:?}"
                )));
            }
            *slot = true;
        } else if key.replace(component.to_owned()).is_some() {
            return Err(ConfigError::InvalidBinding(format!(
                "key chord {chord:?} contains more than one key"
            )));
        }
    }

    let key = key.ok_or_else(|| {
        ConfigError::InvalidBinding(format!("key chord {chord:?} does not contain a key"))
    })?;
    Ok((modifiers, key))
}

fn parse_keysym(name: &str) -> Result<u32, ConfigError> {
    let normalized = match name {
        "Enter" | "enter" => "Return".to_owned(),
        "Space" | "space" => "space".to_owned(),
        "[" => "bracketleft".to_owned(),
        "]" => "bracketright".to_owned(),
        name if name.len() == 1 => name.to_ascii_lowercase(),
        name => name.to_owned(),
    };
    let mut symbol = xkb::keysym_from_name(&normalized, xkb::KEYSYM_NO_FLAGS);
    if symbol.raw() == keysyms::KEY_NoSymbol {
        symbol = xkb::keysym_from_name(&normalized, xkb::KEYSYM_CASE_INSENSITIVE);
    }
    if symbol.raw() == keysyms::KEY_NoSymbol {
        Err(ConfigError::InvalidBinding(format!(
            "unknown keysym {name:?}"
        )))
    } else {
        Ok(symbol.raw())
    }
}

fn parse_action(
    action: &str,
    argument: Option<&str>,
    commands: &HashMap<String, Vec<String>>,
) -> Result<BindingAction, ConfigError> {
    let no_argument = || {
        if argument.is_some() {
            Err(ConfigError::InvalidBinding(format!(
                "action {action:?} does not accept an argument"
            )))
        } else {
            Ok(())
        }
    };
    let required_argument = || {
        argument.ok_or_else(|| {
            ConfigError::InvalidBinding(format!("action {action:?} requires an argument"))
        })
    };

    match action {
        "spawn" => {
            let command = required_argument()?;
            let argv = commands.get(command).ok_or_else(|| {
                ConfigError::InvalidBinding(format!("unknown command {command:?}"))
            })?;
            Ok(BindingAction::Spawn(argv.clone()))
        }
        "close" => {
            no_argument()?;
            Ok(BindingAction::Close)
        }
        "focus" => Ok(BindingAction::Focus(parse_direction(required_argument()?)?)),
        "move" => Ok(BindingAction::Move(parse_direction(required_argument()?)?)),
        "resize" => Ok(BindingAction::Resize(
            parse_direction(required_argument()?)?,
        )),
        "workspace" => Ok(BindingAction::SwitchWorkspace(parse_workspace(
            required_argument()?,
        )?)),
        "move-to-workspace" => Ok(BindingAction::MoveToWorkspace(parse_workspace(
            required_argument()?,
        )?)),
        "toggle-fullscreen" => {
            no_argument()?;
            Ok(BindingAction::ToggleFullscreen)
        }
        "toggle-layout" => {
            no_argument()?;
            Ok(BindingAction::ToggleLayout)
        }
        "cycle-column-width" => {
            no_argument()?;
            Ok(BindingAction::CycleColumnWidth)
        }
        "center-column" => {
            no_argument()?;
            Ok(BindingAction::CenterColumn)
        }
        "consume" => {
            no_argument()?;
            Ok(BindingAction::Consume)
        }
        "expel" => {
            no_argument()?;
            Ok(BindingAction::Expel)
        }
        "toggle-floating" => {
            no_argument()?;
            Ok(BindingAction::ToggleFloating)
        }
        _ => Err(ConfigError::InvalidBinding(format!(
            "unknown action {action:?}"
        ))),
    }
}

fn parse_direction(argument: &str) -> Result<Direction, ConfigError> {
    match argument {
        "left" => Ok(Direction::Left),
        "right" => Ok(Direction::Right),
        "up" => Ok(Direction::Up),
        "down" => Ok(Direction::Down),
        _ => Err(ConfigError::InvalidBinding(format!(
            "invalid direction {argument:?}"
        ))),
    }
}

fn parse_workspace(argument: &str) -> Result<u8, ConfigError> {
    match argument.parse() {
        Ok(workspace) if workspace > 0 => Ok(workspace),
        _ => Err(ConfigError::InvalidBinding(format!(
            "invalid workspace {argument:?}"
        ))),
    }
}

fn default_bindings() -> Vec<BindingConfig> {
    let mut bindings = vec![
        binding("Super+Enter", "spawn", Some("terminal")),
        binding("Super+Q", "close", None),
        binding("Super+F", "toggle-fullscreen", None),
        binding("Super+M", "toggle-layout", None),
        binding("Super+R", "cycle-column-width", None),
        binding("Super+C", "center-column", None),
        binding("Super+[", "consume", None),
        binding("Super+]", "expel", None),
        binding("Super+Shift+Space", "toggle-floating", None),
    ];

    for (key, direction) in [("H", "left"), ("J", "down"), ("K", "up"), ("L", "right")] {
        bindings.push(binding("Super+".to_owned() + key, "focus", Some(direction)));
        bindings.push(binding(
            "Super+Shift+".to_owned() + key,
            "move",
            Some(direction),
        ));
        bindings.push(binding(
            "Super+Ctrl+".to_owned() + key,
            "resize",
            Some(direction),
        ));
    }

    for workspace in 1..=9 {
        let workspace = workspace.to_string();
        bindings.push(binding(
            format!("Super+{workspace}"),
            "workspace",
            Some(&workspace),
        ));
        bindings.push(binding(
            format!("Super+Shift+{workspace}"),
            "move-to-workspace",
            Some(&workspace),
        ));
    }

    bindings
}

fn binding(
    keys: impl Into<String>,
    action: impl Into<String>,
    argument: Option<&str>,
) -> BindingConfig {
    BindingConfig {
        keys: keys.into(),
        match_mode: BindingMatch::Keysym,
        action: Some(action.into()),
        argument: argument.map(str::to_owned),
        disabled: false,
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

fn nonnegative_theme_value(value: f64, field: &'static str) -> Result<f64, ConfigError> {
    if value.is_finite() && value >= 0.0 {
        Ok(value)
    } else {
        Err(ConfigError::InvalidThemeValue {
            field,
            value: value.to_string(),
        })
    }
}

fn finite_theme_value(value: f64, field: &'static str) -> Result<f64, ConfigError> {
    if value.is_finite() {
        Ok(value)
    } else {
        Err(ConfigError::InvalidThemeValue {
            field,
            value: value.to_string(),
        })
    }
}

fn unit_theme_value(value: f64, field: &'static str) -> Result<f64, ConfigError> {
    if value.is_finite() && (0.0..=1.0).contains(&value) {
        Ok(value)
    } else {
        Err(ConfigError::InvalidThemeValue {
            field,
            value: value.to_string(),
        })
    }
}

fn parse_color(value: &str, field: &'static str) -> Result<RgbaColor, ConfigError> {
    let digits = value.strip_prefix('#').unwrap_or(value);
    if !digits.is_ascii() || !matches!(digits.len(), 6 | 8) {
        return Err(ConfigError::InvalidThemeValue {
            field,
            value: value.to_owned(),
        });
    }

    let parse_channel = |offset| u8::from_str_radix(&digits[offset..offset + 2], 16).ok();
    let Some((red, green, blue)) = parse_channel(0)
        .zip(parse_channel(2))
        .zip(parse_channel(4))
        .map(|((red, green), blue)| (red, green, blue))
    else {
        return Err(ConfigError::InvalidThemeValue {
            field,
            value: value.to_owned(),
        });
    };
    let alpha = if digits.len() == 8 {
        parse_channel(6).ok_or_else(|| ConfigError::InvalidThemeValue {
            field,
            value: value.to_owned(),
        })?
    } else {
        u8::MAX
    };

    Ok(RgbaColor([
        f32::from(red) / 255.0,
        f32::from(green) / 255.0,
        f32::from(blue) / 255.0,
        f32::from(alpha) / 255.0,
    ]))
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

fn default_border_color() -> String {
    "#FFFFFF18".to_owned()
}

fn default_accent_color() -> String {
    "#5B8CFF".to_owned()
}

fn default_shadow_color() -> String {
    "#00000055".to_owned()
}

const fn default_border_width() -> f64 {
    1.0
}

const fn default_focus_ring_width() -> f64 {
    2.0
}

const fn default_window_radius() -> f64 {
    14.0
}

const fn default_shadow_offset_y() -> f64 {
    4.0
}

const fn default_shadow_blur() -> f64 {
    18.0
}

const fn default_shadow_opacity() -> f64 {
    0.20
}

fn default_xkb_layout() -> String {
    "us".to_owned()
}

const fn default_repeat_rate() -> i32 {
    25
}

const fn default_repeat_delay() -> i32 {
    600
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

const fn default_true() -> bool {
    true
}

const fn default_output_scale() -> f64 {
    1.0
}

fn parse_output_mode(value: &str) -> Result<OutputModeRequest, String> {
    let (size, refresh) = value
        .trim()
        .split_once('@')
        .map_or((value.trim(), None), |(size, refresh)| {
            (size, Some(refresh))
        });
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
    fn parses_and_validates_theme_window_tokens() {
        let configured = parse(
            "[theme.colors]\nborder = \"#11223344\"\naccent = \"#AABBCC\"\nshadow = \"#01020380\"\n\n[theme.geometry]\nborder_width = 1.5\nfocus_ring_width = 3.0\nwindow_radius = 12.0\n\n[theme.shadow.soft]\noffset_y = -2.0\nblur = 24.0\nopacity = 0.4",
        );
        let invalid_color = parse("[theme.colors]\naccent = \"blue\"");
        let invalid_radius = parse("[theme.geometry]\nwindow_radius = -1.0");
        let invalid_opacity = parse("[theme.shadow.soft]\nopacity = 1.1");

        assert_eq!(
            configured.theme_settings().unwrap(),
            ThemeSettings {
                border_width: 1.5,
                focus_ring_width: 3.0,
                border_color: RgbaColor([17.0 / 255.0, 34.0 / 255.0, 51.0 / 255.0, 68.0 / 255.0,]),
                accent_color: RgbaColor([170.0 / 255.0, 187.0 / 255.0, 0.8, 1.0]),
                shadow_color: RgbaColor([1.0 / 255.0, 2.0 / 255.0, 3.0 / 255.0, 128.0 / 255.0]),
                window_radius: 12.0,
                shadow_offset_y: -2.0,
                shadow_blur: 24.0,
                shadow_opacity: 0.4,
            }
        );
        assert!(invalid_color.theme_settings().is_err());
        assert!(invalid_radius.theme_settings().is_err());
        assert!(invalid_opacity.theme_settings().is_err());
    }

    #[test]
    fn parses_input_and_touchpad_settings() {
        let config = parse(
            "[input]\nfocus_follows_mouse = true\nxkb_layout = \"us,de\"\nxkb_variant = \",nodeadkeys\"\nxkb_options = [\"grp:alt_shift_toggle\"]\nrepeat_rate = 30\nrepeat_delay_ms = 450\n\n[input.touchpad]\ntap = false\nnatural_scroll = false\ndisable_while_typing = true",
        );

        assert_eq!(
            config.input_settings().unwrap(),
            InputSettings {
                focus_follows_mouse: true,
                xkb_layout: "us,de".to_owned(),
                xkb_variant: ",nodeadkeys".to_owned(),
                xkb_options: vec!["grp:alt_shift_toggle".to_owned()],
                repeat_rate: 30,
                repeat_delay_ms: 450,
                touchpad: TouchpadSettings {
                    tap: false,
                    natural_scroll: false,
                    disable_while_typing: true,
                },
            }
        );
    }

    #[test]
    fn rejects_invalid_input_settings() {
        let layout = parse("[input]\nxkb_layout = \"\"");
        let rate = parse("[input]\nrepeat_rate = 0");
        let delay = parse("[input]\nrepeat_delay_ms = -1");

        assert!(layout.input_settings().is_err());
        assert!(rate.input_settings().is_err());
        assert!(delay.input_settings().is_err());
    }

    #[test]
    fn supplies_complete_v0_bindings_and_terminal_command() {
        let config = parse("");
        let input = config.input_settings().unwrap();
        let bindings = config.bindings(&input).unwrap();

        assert_eq!(bindings.len(), 39);
        assert!(bindings.iter().any(|binding| {
            binding.modifiers.logo
                && binding.trigger == BindingTrigger::Keysym(keysyms::KEY_Return)
                && binding.action == BindingAction::Spawn(vec!["foot".to_owned()])
        }));
    }

    #[test]
    fn replaces_and_unbinds_default_bindings() {
        let replaced = parse(
            "[commands]\nterm = [\"foot\", \"--app-id\", \"work\"]\n\n[[bindings]]\nkeys = \"Super+Enter\"\naction = \"spawn\"\nargument = \"term\"",
        );
        let unbound = parse("[[bindings]]\nkeys = \"Super+Q\"\ndisabled = true");

        let input = replaced.input_settings().unwrap();
        let bindings = replaced.bindings(&input).unwrap();
        assert_eq!(bindings.len(), 39);
        assert!(bindings.iter().any(|binding| {
            binding.action
                == BindingAction::Spawn(vec![
                    "foot".to_owned(),
                    "--app-id".to_owned(),
                    "work".to_owned(),
                ])
        }));

        let input = unbound.input_settings().unwrap();
        let bindings = unbound.bindings(&input).unwrap();
        assert_eq!(bindings.len(), 38);
        assert!(
            !bindings
                .iter()
                .any(|binding| binding.action == BindingAction::Close)
        );
    }

    #[test]
    fn accepts_physical_xkb_key_names() {
        let config = parse(
            "[[bindings]]\nkeys = \"Super+AD06\"\nmatch = \"physical\"\naction = \"focus\"\nargument = \"left\"",
        );
        let input = config.input_settings().unwrap();
        let bindings = config.bindings(&input).unwrap();

        assert!(bindings.iter().any(|binding| {
            matches!(binding.trigger, BindingTrigger::Physical(_))
                && binding.action == BindingAction::Focus(Direction::Left)
        }));
    }

    #[test]
    fn rejects_duplicate_or_invalid_user_bindings() {
        let duplicate = parse(
            "[[bindings]]\nkeys = \"Super+Q\"\naction = \"close\"\n\n[[bindings]]\nkeys = \"logo+q\"\naction = \"close\"",
        );
        let missing_command = parse(
            "[[bindings]]\nkeys = \"Super+Enter\"\naction = \"spawn\"\nargument = \"missing\"",
        );
        let invalid_argument =
            parse("[[bindings]]\nkeys = \"Super+Q\"\naction = \"close\"\nargument = \"left\"");

        for config in [duplicate, missing_command, invalid_argument] {
            let input = config.input_settings().unwrap();
            assert!(config.bindings(&input).is_err());
        }
    }

    #[test]
    fn parses_and_validates_window_rules() {
        let config = parse(
            "[[window_rules]]\napp_id = \"org.example.Editor\"\nworkspace = 3\nfloating = true\nwidth = 900.0\nheight = 600.0\nfullscreen = false",
        );
        let rules = config.window_rules().unwrap();
        let result = window_rules::resolve(
            &rules,
            Some("org.example.editor.desktop"),
            Some("Document"),
            false,
        );

        assert_eq!(result.workspace, Some(3));
        assert_eq!(result.floating, Some(true));
        assert_eq!(result.width, Some(900.0));
        assert_eq!(result.height, Some(600.0));
        assert_eq!(result.fullscreen, Some(false));
    }

    #[test]
    fn rejects_invalid_window_rule_configuration() {
        let catch_all = parse("[[window_rules]]\nfloating = true");
        let zero_workspace = parse("[[window_rules]]\napp_id = \"editor\"\nworkspace = 0");

        assert!(catch_all.window_rules().is_err());
        assert!(zero_workspace.window_rules().is_err());
    }

    #[test]
    fn derives_critical_viewport_damping() {
        let config = parse("");
        let spring = config.viewport_spring_config().unwrap();

        assert_eq!(spring.mass, 1.0);
        assert_eq!(spring.stiffness, 320.0);
        assert!((spring.damping - 35.777_087_64).abs() < 0.000_001);
    }

    #[test]
    fn parses_output_profiles() {
        let config = parse(
            r#"
[[output_profiles]]
name = "docked"

[[output_profiles.outputs]]
match = "HDMI-A-1"
mode = "3840x2160@119.998"
scale = 1.6
position = [0, 0]

[[output_profiles.outputs]]
match = "eDP-1"
enabled = false
transform = "rotate_90"
"#,
        );

        assert_eq!(
            config.output_profiles().unwrap(),
            vec![OutputProfile {
                name: "docked".to_owned(),
                outputs: vec![
                    OutputSettings {
                        matcher: "HDMI-A-1".to_owned(),
                        enabled: true,
                        mode: Some(OutputModeRequest {
                            width: 3840,
                            height: 2160,
                            refresh_millihertz: Some(119_998),
                        }),
                        scale: 1.6,
                        transform: OutputTransform::Normal,
                        position: Some([0, 0]),
                    },
                    OutputSettings {
                        matcher: "eDP-1".to_owned(),
                        enabled: false,
                        mode: None,
                        scale: 1.0,
                        transform: OutputTransform::Rotate90,
                        position: None,
                    },
                ],
            }]
        );
    }

    #[test]
    fn rejects_invalid_output_profiles() {
        let invalid_scale = parse(
            "[[output_profiles]]\nname = \"bad\"\n[[output_profiles.outputs]]\nmatch = \"eDP-1\"\nscale = 0.0",
        );
        let invalid_mode = parse(
            "[[output_profiles]]\nname = \"bad\"\n[[output_profiles.outputs]]\nmatch = \"eDP-1\"\nmode = \"native\"",
        );
        let duplicate = parse(
            "[[output_profiles]]\nname = \"bad\"\n[[output_profiles.outputs]]\nmatch = \"eDP-1\"\n[[output_profiles.outputs]]\nmatch = \"eDP-1\"",
        );

        assert!(invalid_scale.output_profiles().is_err());
        assert!(invalid_mode.output_profiles().is_err());
        assert!(duplicate.output_profiles().is_err());
    }
}
