use std::collections::{HashMap, HashSet};
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
    desktop_widgets: ferese_core::desktop::DesktopWidgets,
    #[serde(default)]
    pub(crate) autostart: Vec<DaemonConfig>,
    #[serde(default)]
    animations: AnimationsConfig,
    #[serde(default)]
    layout: LayoutConfig,
    #[serde(default)]
    input: InputConfig,
    #[serde(default)]
    theme: ThemeConfig,
    #[serde(default)]
    appearance: AppearanceConfig,
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
    #[serde(default, rename = "status")]
    _status: ShellStatusConfig,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub(crate) struct DaemonConfig {
    pub command: Vec<String>,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_true")]
    pub restart: bool,
    #[serde(default)]
    pub nested: bool,
}

// Validate shell-only fields too, before publishing an accepted source.
#[derive(Debug, Default, Deserialize)]
#[allow(dead_code)]
struct ShellStatusConfig {
    battery_percentage: Option<bool>,
    low_battery_threshold: Option<u8>,
    settings_command: Option<Vec<String>>,
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
pub struct BorderGradient {
    pub from: RgbaColor,
    pub to: RgbaColor,
    pub angle: f64,
}

#[derive(Debug, Default, Deserialize)]
struct BorderPaintConfig {
    gradient: Option<BorderGradientConfig>,
}

#[derive(Debug, Deserialize)]
struct BorderGradientConfig {
    from: String,
    to: String,
    #[serde(default)]
    angle: f64,
}

impl BorderPaintConfig {
    fn settings(&self, name: &str) -> Result<Option<BorderGradient>, ConfigError> {
        let (from_name, to_name, angle_name) = if name == "focus_ring" {
            (
                "focus_ring.gradient.from",
                "focus_ring.gradient.to",
                "focus_ring.gradient.angle",
            )
        } else {
            (
                "border.gradient.from",
                "border.gradient.to",
                "border.gradient.angle",
            )
        };
        self.gradient
            .as_ref()
            .map(|gradient| {
                Ok(BorderGradient {
                    from: parse_color(&gradient.from, from_name)?,
                    to: parse_color(&gradient.to, to_name)?,
                    angle: finite_theme_value(gradient.angle, angle_name)?.rem_euclid(360.0),
                })
            })
            .transpose()
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ThemeSettings {
    pub border_width: f64,
    pub focus_ring_width: f64,
    pub border_color: RgbaColor,
    pub accent_color: RgbaColor,
    pub border_gradient: Option<BorderGradient>,
    pub focus_ring_gradient: Option<BorderGradient>,
    pub shadow_color: RgbaColor,
    pub surface_base_color: RgbaColor,
    pub panel_opacity: f64,
    pub inactive_dim: InactiveDimSettings,
    pub window_radius: f64,
    pub shadow_offset_y: f64,
    pub shadow_blur: f64,
    pub shadow_opacity: f64,
    pub material_style: MaterialStyle,
    pub backdrop_blur: f64,
    pub material_radius: f64,
    pub panel_radius: f64,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum MaterialStyle {
    Translucent,
    #[default]
    Solid,
}

#[derive(Debug, Default, Deserialize)]
struct AppearanceConfig {
    corner_radius: Option<f64>,
    #[serde(default)]
    inactive_dim: InactiveDimConfig,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct InactiveDimSettings {
    pub enabled: bool,
    pub amount: f64,
    pub duration_ms: f64,
}

#[derive(Debug, Deserialize)]
#[serde(default)]
struct InactiveDimConfig {
    enabled: bool,
    amount: f64,
    duration_ms: f64,
}

impl Default for InactiveDimConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            amount: 0.15,
            duration_ms: 150.0,
        }
    }
}

#[derive(Debug, Default, Deserialize)]
struct ThemeConfig {
    #[serde(default)]
    border: BorderPaintConfig,
    #[serde(default)]
    focus_ring: BorderPaintConfig,
    #[serde(default)]
    typography: OverviewTypographyConfig,
    #[serde(default)]
    background: crate::wallpaper::WallpaperConfig,
    #[serde(default)]
    surface: ThemeSurfaceConfig,
    #[serde(default)]
    colors: ThemeColorsConfig,
    #[serde(default)]
    geometry: ThemeGeometryConfig,
    #[serde(default)]
    shadow: ThemeShadowConfig,
    #[serde(default)]
    material: ThemeMaterialConfig,
}

impl Config {
    pub(crate) fn parse_source(source: &str) -> Result<Self, ConfigError> {
        ferese_config::from_str(source).map_err(|source| ConfigError::Parse {
            path: config_path().unwrap_or_default(),
            source,
        })
    }

    pub(crate) fn runtime_config(&self) -> Result<crate::RuntimeConfig, ConfigError> {
        self.desktop_widgets
            .validate()
            .map_err(ConfigError::InvalidBinding)?;
        for daemon in &self.autostart {
            if daemon
                .command
                .first()
                .is_none_or(|program| program.trim().is_empty())
            {
                return Err(ConfigError::InvalidBinding(
                    "autostart command must contain a program".into(),
                ));
            }
        }
        let input_settings = self.input_settings()?;
        let bindings = self.bindings(&input_settings)?;
        Ok(crate::RuntimeConfig {
            autostart: self.autostart.clone(),
            layout_mode: self.layout_mode(),
            gap_config: self.gap_config()?,
            input_settings,
            bindings,
            window_rules: self.window_rules()?,
            theme_settings: self.theme_settings()?,
            default_column_width: self.default_column_width()?,
            scrolling_focus_strategy: self.scrolling_focus_strategy(),
            column_width_presets: self.width_presets()?,
            animations_enabled: self.animations_enabled(),
            animation_speed: self.animation_speed()?,
            spring_config: self.spring_config()?,
            viewport_spring_config: self.viewport_spring_config()?,
            output_profiles: self.output_profiles()?,
            wallpaper: self.wallpaper_settings(),
            overview_font_family: self.overview_font_family(),
        })
    }

    pub(crate) fn overview_font_family(&self) -> String {
        self.theme
            .typography
            .font_family
            .clone()
            .unwrap_or_else(|| "sans-serif".into())
    }

    pub(crate) fn wallpaper_settings(&self) -> crate::wallpaper::WallpaperConfig {
        self.theme.background.clone()
    }
}

#[derive(Debug, Default, Deserialize)]
struct OverviewTypographyConfig {
    font_family: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct ThemeSurfaceConfig {
    #[serde(default)]
    bar: ThemeBarConfig,
}

#[derive(Debug, Default, Deserialize)]
struct ThemeBarConfig {
    opacity: Option<f64>,
}

#[derive(Debug, Deserialize)]
struct ThemeMaterialConfig {
    #[serde(default)]
    style: MaterialStyle,
    #[serde(default = "default_backdrop_blur")]
    blur_radius: f64,
}

impl Default for ThemeMaterialConfig {
    fn default() -> Self {
        Self {
            style: MaterialStyle::Solid,
            blur_radius: default_backdrop_blur(),
        }
    }
}

fn default_backdrop_blur() -> f64 {
    12.0
}

#[derive(Debug, Deserialize)]
struct ThemeColorsConfig {
    #[serde(default = "default_surface_base_color")]
    surface_base: String,
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
            surface_base: default_surface_base_color(),
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
    #[serde(default)]
    top_bar_radius: f64,
}

impl Default for ThemeGeometryConfig {
    fn default() -> Self {
        Self {
            border_width: default_border_width(),
            focus_ring_width: default_focus_ring_width(),
            window_radius: default_window_radius(),
            top_bar_radius: 0.0,
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
    Swipe {
        fingers: u32,
        direction: crate::gestures::SwipeDirection,
    },
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
    None,
    Spawn(Vec<String>),
    Close,
    Exit,
    Focus(ferese_layout::Direction),
    Move(ferese_layout::Direction),
    Resize(ferese_layout::Direction),
    SwitchWorkspace(u8),
    SwitchRelativeWorkspace(bool),
    MoveToWorkspace(u8),
    ToggleFullscreen,
    ToggleMaximized,
    ToggleLayout,
    CycleColumnWidth,
    CenterColumn,
    Consume,
    Expel,
    ToggleFloating,
    ToggleOverview,
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
                BindingTrigger::Swipe { .. } => false,
            }
    }

    pub(crate) fn swipe_fingers(&self, count: u32) -> bool {
        matches!(self.trigger, BindingTrigger::Swipe { fingers, .. } if fingers == count)
    }

    pub(crate) fn matches_swipe(
        &self,
        count: u32,
        target: crate::gestures::SwipeDirection,
    ) -> bool {
        matches!(self.trigger, BindingTrigger::Swipe { fingers, direction } if fingers == count && direction == target)
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, Hash, PartialEq)]
#[serde(rename_all = "lowercase")]
enum BindingMatch {
    #[default]
    Keysym,
    Physical,
    Swipe,
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
    pub swipe_threshold: u16,
}

fn default_swipe_threshold() -> u16 {
    80
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
    #[serde(default = "default_swipe_threshold")]
    swipe_threshold: u16,
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
    Paged,
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
        source: ferese_config::Error,
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

        ferese_config::from_str(&source).map_err(|source| ConfigError::Parse { path, source })
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
            FocusStrategyValue::Paged => ViewportFocusStrategy::Paged,
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

    pub fn bindings(&self, input: &InputSettings) -> Result<Vec<Binding>, ConfigError> {
        let mut commands = HashMap::from([
            ("terminal".to_owned(), vec!["foot".to_owned()]),
            (
                "screenshot".to_owned(),
                vec!["ferese-screenshot".to_owned()],
            ),
            (
                "screenshot-full".to_owned(),
                vec!["ferese-screenshot".to_owned(), "--full".to_owned()],
            ),
        ]);
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

        let material_radius = nonnegative_theme_value(
            self.appearance.corner_radius.unwrap_or(14.0),
            "appearance.corner_radius",
        )?;
        Ok(ThemeSettings {
            border_width,
            focus_ring_width,
            border_color: parse_color(&self.theme.colors.border, "colors.border")?,
            accent_color: parse_color(&self.theme.colors.accent, "colors.accent")?,
            border_gradient: self.theme.border.settings("border")?,
            focus_ring_gradient: self.theme.focus_ring.settings("focus_ring")?,
            shadow_color: parse_color(&self.theme.colors.shadow, "colors.shadow")?,
            surface_base_color: parse_color(
                &self.theme.colors.surface_base,
                "colors.surface_base",
            )?,
            panel_opacity: unit_theme_value(
                self.theme.surface.bar.opacity.unwrap_or(0.78),
                "surface.bar.opacity",
            )?,
            inactive_dim: InactiveDimSettings {
                enabled: self.appearance.inactive_dim.enabled,
                amount: unit_theme_value(
                    self.appearance.inactive_dim.amount,
                    "appearance.inactive_dim.amount",
                )?,
                duration_ms: nonnegative_theme_value(
                    self.appearance.inactive_dim.duration_ms,
                    "appearance.inactive_dim.duration_ms",
                )?,
            },
            window_radius,
            shadow_offset_y,
            shadow_blur,
            shadow_opacity,
            material_style: self.theme.material.style,
            backdrop_blur: nonnegative_theme_value(
                self.theme.material.blur_radius,
                "material.blur_radius",
            )?
            .min(32.0),
            material_radius,
            panel_radius: nonnegative_theme_value(
                self.theme.geometry.top_bar_radius,
                "geometry.top_bar_radius",
            )?,
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
        BindingMatch::Swipe => BindingTrigger::Swipe {
            fingers: identity.key / 4,
            direction: [
                crate::gestures::SwipeDirection::Up,
                crate::gestures::SwipeDirection::Down,
                crate::gestures::SwipeDirection::Left,
                crate::gestures::SwipeDirection::Right,
            ][(identity.key % 4) as usize],
        },
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
    if let Some((fingers, direction)) = parse_swipe(&key) {
        if configured.match_mode == BindingMatch::Physical
            || modifiers != BindingModifiers::default()
        {
            return Err(ConfigError::InvalidBinding(
                "swipes cannot use keyboard modifiers or physical key matching".into(),
            ));
        }
        return Ok(BindingIdentity {
            modifiers,
            match_mode: BindingMatch::Swipe,
            key: fingers * 4 + direction as u32,
        });
    }
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
        BindingMatch::Swipe => {
            return Err(ConfigError::InvalidBinding(
                "gesture keys must use Swipe3Left/Right/Up/Down (3–5 fingers)".into(),
            ));
        }
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
        BindingTrigger::Swipe { fingers, direction } => {
            (BindingMatch::Swipe, fingers * 4 + direction as u32)
        }
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

fn parse_swipe(key: &str) -> Option<(u32, crate::gestures::SwipeDirection)> {
    use crate::gestures::SwipeDirection;
    let lower = key.to_ascii_lowercase();
    let rest = lower.strip_prefix("swipe")?;
    let fingers = rest.get(..1)?.parse::<u32>().ok()?;
    if !(3..=5).contains(&fingers) {
        return None;
    }
    let direction = match rest.get(1..)? {
        "up" => SwipeDirection::Up,
        "down" => SwipeDirection::Down,
        "left" => SwipeDirection::Left,
        "right" => SwipeDirection::Right,
        _ => return None,
    };
    Some((fingers, direction))
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
        "none" => {
            no_argument()?;
            Ok(BindingAction::None)
        }
        "workspace-next" | "workspace-previous" => {
            no_argument()?;
            Ok(BindingAction::SwitchRelativeWorkspace(
                action == "workspace-next",
            ))
        }
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
        "exit" => {
            no_argument()?;
            Ok(BindingAction::Exit)
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
        "toggle-maximized" => {
            no_argument()?;
            Ok(BindingAction::ToggleMaximized)
        }
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
        "toggle-overview" => {
            no_argument()?;
            Ok(BindingAction::ToggleOverview)
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
        binding("Super+Shift+S", "spawn", Some("screenshot")),
        binding("Print", "spawn", Some("screenshot-full")),
        binding("Swipe3Up", "workspace-next", None),
        binding("Swipe3Down", "workspace-previous", None),
        binding("Swipe3Left", "focus", Some("right")),
        binding("Swipe3Right", "focus", Some("left")),
        binding("Super+Q", "close", None),
        binding("Super+F", "toggle-maximized", None),
        binding("Super+Shift+F", "toggle-fullscreen", None),
        binding("Super+M", "toggle-layout", None),
        binding("Super+R", "cycle-column-width", None),
        binding("Super+C", "center-column", None),
        binding("Super+[", "consume", None),
        binding("Super+]", "expel", None),
        binding("Super+Shift+Space", "toggle-floating", None),
        binding("Super+Tab", "toggle-overview", None),
        binding("Super+Shift+E", "exit", None),
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
    4.0
}

fn default_border_color() -> String {
    "#FFFFFF18".to_owned()
}

fn default_surface_base_color() -> String {
    "#111821".to_owned()
}

fn default_accent_color() -> String {
    "#3D7BE6".to_owned()
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

pub(crate) fn config_path() -> Option<PathBuf> {
    ferese_config::config_path()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packaged_and_custom_kdl_pass_runtime_validation() {
        Config::parse_source(include_str!("../../packaging/config.kdl"))
            .unwrap()
            .runtime_config()
            .unwrap();
        let source = "input {\n    touchpad {\n        swipe-threshold 96\n    }\n}\nbinding keys=\"Swipe3Up\" action=\"toggle-overview\"\noutput-profile name=\"desk\" {\n    output match=\"DP-1\" scale=1.5 {\n        position 0 0\n    }\n}\ndesktop-widgets {\n    clock {\n        enabled #true\n        outputs \"DP-1\"\n    }\n}\nautostart {\n    command \"program\" \"argument with space\"\n}\n";
        let config = Config::parse_source(source).unwrap();
        config.runtime_config().unwrap();
        assert_eq!(
            config.input_settings().unwrap().touchpad.swipe_threshold,
            96
        );
        assert_eq!(config.desktop_widgets.clock.outputs, ["DP-1"]);
        assert_eq!(
            config.output_profiles().unwrap()[0].outputs[0].position,
            Some([0, 0])
        );
    }

    #[test]
    fn desktop_clock_settings_are_validated_before_live_publication() {
        assert!(
            parse(
                "desktop-widgets {\n    clock {\n        enabled #true\n        anchor \"center\"\n        time-format \"%H:%M\"\n    }\n}\n"
            )
            .runtime_config()
            .is_ok()
        );
        for source in [
            "desktop-widgets {\n    clock {\n        width 8192\n    }\n}\n",
            "desktop-widgets {\n    clock {\n        opacity #nan\n    }\n}\n",
            "desktop-widgets {\n    clock {\n        time-format \"%\"\n    }\n}\n",
            "desktop-widgets {\n    clock {\n        time-zone \"invalid/zone\"\n    }\n}\n",
        ] {
            assert!(
                Config::parse_source(source)
                    .and_then(|config| config.runtime_config())
                    .is_err()
            );
        }
        assert!(Config::parse_source("desktop-widgets { clock { anchor \"wrong\"; }; }").is_err());
    }

    fn parse(source: &str) -> Config {
        ferese_config::from_str(source).unwrap()
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
        let full = parse("scrolling {\n    default-column-width \"full\"\n}\n");
        let numeric = parse("scrolling {\n    default-column-width 1.0\n}\n");

        assert_eq!(full.default_column_width().unwrap(), ColumnWidth::Full);
        assert_eq!(
            numeric.default_column_width().unwrap(),
            ColumnWidth::Proportion(1.0)
        );
    }

    #[test]
    fn parses_width_presets_and_supplies_defaults() {
        let defaults = parse("");
        let configured = parse("scrolling {\n    width-presets 0.5 1.0 \"full\"\n}\n");

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
        let centered = parse("scrolling {\n    focus-strategy \"center_on_focus\"\n}\n");
        let paged = parse("scrolling {\n    focus-strategy \"paged\"\n}\n");
        assert_eq!(
            paged.scrolling_focus_strategy(),
            ViewportFocusStrategy::Paged
        );

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
        let zero = parse("scrolling {\n    default-column-width 0.0\n}\n");
        let unknown = parse("scrolling {\n    default-column-width \"wide\"\n}\n");

        assert!(zero.default_column_width().is_err());
        assert!(unknown.default_column_width().is_err());
    }

    #[test]
    fn parses_animation_policy_and_reduced_motion() {
        let config = parse(
            "animations {\n    speed 1.5\n    reduced-motion #true\n    spring {\n        mass 2.0\n        stiffness 500.0\n        damping 40.0\n    }\n}\n",
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
        let speed = parse("animations {\n    speed 0.0\n}\n");
        let damping = parse("animations {\n    spring {\n        damping -1.0\n    }\n}\n");

        assert!(speed.animation_speed().is_err());
        assert!(damping.spring_config().is_err());
    }

    #[test]
    fn parses_and_validates_layout_gaps() {
        let configured =
            parse("layout {\n    inner-gap 6.0\n    outer-gap 14.0\n    smart-gaps #true\n}\n");
        let invalid = parse("layout {\n    outer-gap -1.0\n}\n");

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
            "theme {\n    colors {\n        border \"#11223344\"\n        accent \"#AABBCC\"\n        shadow \"#01020380\"\n    }\n    geometry {\n        border-width 1.5\n        focus-ring-width 3.0\n        window-radius 12.0\n    }\n    shadow {\n        soft {\n            offset-y -2.0\n            blur 24.0\n            opacity 0.4\n        }\n    }\n    material {\n        style \"translucent\"\n    }\n}\n",
        );
        let invalid_color = parse("theme {\n    colors {\n        accent \"blue\"\n    }\n}\n");
        let invalid_radius =
            parse("theme {\n    geometry {\n        window-radius -1.0\n    }\n}\n");
        let invalid_opacity = parse(
            "theme {\n    shadow {\n        soft {\n            opacity 1.1\n        }\n    }\n}\n",
        );

        assert_eq!(
            configured.theme_settings().unwrap(),
            ThemeSettings {
                border_width: 1.5,
                focus_ring_width: 3.0,
                border_color: RgbaColor([17.0 / 255.0, 34.0 / 255.0, 51.0 / 255.0, 68.0 / 255.0,]),
                accent_color: RgbaColor([170.0 / 255.0, 187.0 / 255.0, 0.8, 1.0]),
                border_gradient: None,
                focus_ring_gradient: None,
                shadow_color: RgbaColor([1.0 / 255.0, 2.0 / 255.0, 3.0 / 255.0, 128.0 / 255.0]),
                surface_base_color: RgbaColor([17.0 / 255.0, 24.0 / 255.0, 33.0 / 255.0, 1.0,]),
                panel_opacity: 0.78,
                inactive_dim: InactiveDimSettings {
                    enabled: false,
                    amount: 0.15,
                    duration_ms: 150.0
                },
                window_radius: 12.0,
                shadow_offset_y: -2.0,
                shadow_blur: 24.0,
                shadow_opacity: 0.4,
                material_style: MaterialStyle::Translucent,
                backdrop_blur: 12.0,
                material_radius: 14.0,
                panel_radius: 0.0,
            }
        );
        assert!(invalid_color.theme_settings().is_err());
        assert!(invalid_radius.theme_settings().is_err());
        assert!(invalid_opacity.theme_settings().is_err());
        assert!(
            ferese_config::from_str::<Config>(
                "theme {\n    material {\n        style \"mist\"\n    }\n}\n"
            )
            .is_err()
        );
    }

    #[test]
    fn materials_default_to_solid_and_reject_removed_style() {
        assert_eq!(
            parse("").theme_settings().unwrap().material_style,
            MaterialStyle::Solid
        );
        assert!(
            ferese_config::from_str::<Config>(
                "theme {\n    material {\n        style \"glass\"\n    }\n}\n"
            )
            .is_err()
        );
    }

    #[test]
    fn border_gradients_are_optional_and_independent() {
        let defaults = parse("").theme_settings().unwrap();
        assert_eq!(defaults.border_gradient, None);
        assert_eq!(defaults.focus_ring_gradient, None);
        let configured =
            parse("theme {\n    focus-ring {\n        gradient {\n            from \"#e5c890\"\n            to \"#b98d5880\"\n            angle -45\n        }\n    }\n}\n")
                .theme_settings()
                .unwrap();
        let gradient = configured.focus_ring_gradient.unwrap();
        assert_eq!(configured.border_gradient, None);
        assert_eq!(gradient.angle, 315.0);
        assert_eq!(gradient.to.0[3], 128.0 / 255.0);
        let border = parse("theme {\n    border {\n        gradient {\n            from \"#112233\"\n            to \"#445566\"\n        }\n    }\n}\n")
            .theme_settings()
            .unwrap();
        assert_eq!(border.border_gradient.unwrap().angle, 0.0);
        assert_eq!(border.focus_ring_gradient, None);
        assert_eq!(configured.border_color, defaults.border_color);
        assert_eq!(configured.accent_color, defaults.accent_color);
    }

    #[test]
    fn border_gradients_reject_invalid_colors_and_nonfinite_angles() {
        for settings in [
            "from \"invalid\"\nto \"#445566\"\n",
            "from \"#112233\"\nto \"invalid\"\n",
            "from \"#112233\"\nto \"#445566\"\nangle #nan\n",
            "from \"#112233\"\nto \"#445566\"\nangle #inf\n",
        ] {
            assert!(
                Config::parse_source(&format!(
                    "theme {{ focus-ring {{ gradient {{\n{settings}\n}} }} }}"
                ))
                .and_then(|config| config.theme_settings())
                .is_err()
            );
        }
        assert!(ferese_config::from_str::<Config>("theme {\n    border {\n        gradient {\n            from \"#112233\"\n        }\n    }\n}\n").is_err());
    }

    #[test]
    fn material_color_comes_from_surface_base() {
        let theme = parse("theme {\n    colors {\n        surface-base \"#000000\"\n    }\n    surface {\n        bar {\n            background \"#FFFFFF\"\n        }\n    }\n}\n")
            .theme_settings()
            .unwrap();
        assert_eq!(theme.surface_base_color, RgbaColor([0.0, 0.0, 0.0, 1.0]));
    }

    #[test]
    fn bar_opacity_uses_config_and_rejects_invalid_values() {
        assert_eq!(parse("").theme_settings().unwrap().panel_opacity, 0.78);
        for opacity in [0.0, 0.65, 1.0] {
            assert_eq!(
                parse(&format!(
                    "theme {{ surface {{ bar {{ opacity {opacity}; }} }} }}"
                ))
                .theme_settings()
                .unwrap()
                .panel_opacity,
                opacity
            );
        }
        for opacity in ["-0.1", "1.1", "#nan", "#inf"] {
            assert!(
                Config::parse_source(&format!(
                    "theme {{ surface {{ bar {{ opacity {opacity}; }} }} }}"
                ))
                .and_then(|config| config.theme_settings())
                .is_err()
            );
        }
    }

    #[test]
    fn backdrop_blur_is_configurable_without_glass_settings() {
        assert_eq!(parse("").theme_settings().unwrap().backdrop_blur, 12.0);
        assert_eq!(
            parse("theme {\n    material {\n        style \"translucent\"\n        blur-radius 100\n    }\n}\n")
                .theme_settings()
                .unwrap()
                .backdrop_blur,
            32.0
        );
        assert_eq!(
            parse("theme {\n    material {\n        blur-radius 0\n    }\n}\n")
                .theme_settings()
                .unwrap()
                .backdrop_blur,
            0.0
        );
        for value in ["-1", "#nan", "#inf"] {
            assert!(
                Config::parse_source(&format!("theme {{ material {{ blur-radius {value}; }} }}"))
                    .and_then(|config| config.theme_settings())
                    .is_err()
            );
        }
    }

    #[test]
    fn inactive_dimming_is_opt_in_and_configurable() {
        let defaults = parse("").theme_settings().unwrap().inactive_dim;
        assert!(!defaults.enabled);
        assert_eq!(defaults.amount, 0.15);
        assert_eq!(defaults.duration_ms, 150.0);
        let settings =
            parse("appearance {\n    inactive-dim {\n        enabled #true\n        amount 0.25\n        duration-ms 100\n    }\n}\n")
                .theme_settings()
                .unwrap()
                .inactive_dim;
        assert!(settings.enabled);
        assert_eq!(settings.amount, 0.25);
        assert_eq!(settings.duration_ms, 100.0);
        for (key, value) in [
            ("amount", "-0.1"),
            ("amount", "1.1"),
            ("amount", "#nan"),
            ("duration_ms", "-1"),
            ("duration_ms", "#inf"),
        ] {
            assert!(
                Config::parse_source(&format!(
                    "appearance {{ inactive-dim {{ {key} {value}; }} }}"
                ))
                .and_then(|config| config.theme_settings())
                .is_err()
            );
        }
    }

    #[test]
    fn parses_input_and_touchpad_settings() {
        let config = parse(
            "input {\n    focus-follows-mouse #true\n    xkb-layout \"us,de\"\n    xkb-variant \",nodeadkeys\"\n    xkb-options \"grp:alt_shift_toggle\"\n    repeat-rate 30\n    repeat-delay-ms 450\n    touchpad {\n        tap #false\n        natural-scroll #false\n        disable-while-typing #true\n    }\n}\n",
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
                    swipe_threshold: 80,
                },
            }
        );
    }

    #[test]
    fn rejects_invalid_input_settings() {
        let layout = parse("input {\n    xkb-layout \"\"\n}\n");
        let rate = parse("input {\n    repeat-rate 0\n}\n");
        let delay = parse("input {\n    repeat-delay-ms -1\n}\n");

        assert!(layout.input_settings().is_err());
        assert!(rate.input_settings().is_err());
        assert!(delay.input_settings().is_err());
    }

    #[test]
    fn swipe_bindings_can_override_actions_disable_defaults_and_set_distance() {
        use crate::gestures::SwipeDirection;
        let config = parse(
            "input {\n    touchpad {\n        swipe-threshold 120\n    }\n}\nbinding keys=\"Swipe3Up\" action=\"toggle-overview\"\nbinding keys=\"Swipe3Left\" action=\"move\" argument=\"left\"\nbinding keys=\"Swipe3Down\" disabled=#true\n",
        );
        let input = config.input_settings().unwrap();
        assert_eq!(input.touchpad.swipe_threshold, 120);
        let bindings = config.bindings(&input).unwrap();
        assert!(
            bindings
                .iter()
                .any(|binding| binding.matches_swipe(3, SwipeDirection::Up)
                    && binding.action == BindingAction::ToggleOverview)
        );
        assert!(
            bindings
                .iter()
                .any(|binding| binding.matches_swipe(3, SwipeDirection::Left)
                    && binding.action == BindingAction::Move(ferese_layout::Direction::Left))
        );
        assert!(
            !bindings
                .iter()
                .any(|binding| binding.matches_swipe(3, SwipeDirection::Down))
        );
        for threshold in [0, 15, 1001] {
            assert!(
                parse(&format!(
                    "input {{ touchpad {{ swipe-threshold {threshold}; }} }}"
                ))
                .input_settings()
                .is_err()
            );
        }
    }

    #[test]
    fn gesture_bindings_validate_fingers_directions_and_actions() {
        for keys in ["Swipe2Up", "Swipe3Diagonal", "Super+Swipe3Up"] {
            let config = parse(&format!("binding \"{keys}\" \"toggle-overview\""));
            assert!(config.bindings(&config.input_settings().unwrap()).is_err());
        }
        let config = parse("binding keys=\"Swipe4Up\" action=\"toggle-overview\"\n");
        let bindings = config.bindings(&config.input_settings().unwrap()).unwrap();
        assert!(
            bindings
                .iter()
                .any(|binding| binding.matches_swipe(4, crate::gestures::SwipeDirection::Up))
        );
    }

    #[test]
    fn supplies_complete_v0_bindings_and_terminal_command() {
        let config = parse("");
        let input = config.input_settings().unwrap();
        let bindings = config.bindings(&input).unwrap();

        assert_eq!(bindings.len(), 48);
        for (shift, action) in [
            (false, BindingAction::ToggleMaximized),
            (true, BindingAction::ToggleFullscreen),
        ] {
            assert!(bindings.iter().any(|binding| binding.modifiers.logo
                && binding.modifiers.shift == shift
                && binding.trigger == BindingTrigger::Keysym(keysyms::KEY_f)
                && binding.action == action));
        }
        assert!(
            bindings
                .iter()
                .any(|binding| binding.action == BindingAction::Exit)
        );
        assert!(bindings.iter().any(|binding| {
            binding.modifiers.logo
                && binding.trigger == BindingTrigger::Keysym(keysyms::KEY_Return)
                && binding.action == BindingAction::Spawn(vec!["foot".to_owned()])
        }));
        assert!(bindings.iter().any(|binding| {
            binding.modifiers.logo
                && binding.modifiers.shift
                && binding.trigger == BindingTrigger::Keysym(keysyms::KEY_s)
                && binding.action == BindingAction::Spawn(vec!["ferese-screenshot".to_owned()])
        }));
        assert!(bindings.iter().any(|binding| {
            binding.trigger == BindingTrigger::Keysym(keysyms::KEY_Print)
                && binding.modifiers == BindingModifiers::default()
                && binding.action
                    == BindingAction::Spawn(vec![
                        "ferese-screenshot".to_owned(),
                        "--full".to_owned(),
                    ])
        }));
        assert!(bindings.iter().any(|binding| {
            binding.modifiers.logo
                && binding.trigger == BindingTrigger::Keysym(keysyms::KEY_Tab)
                && binding.action == BindingAction::ToggleOverview
        }));
    }

    #[test]
    fn replaces_and_unbinds_default_bindings() {
        let replaced = parse(
            "commands {\n    term \"foot\" \"--app-id\" \"work\"\n}\nbinding keys=\"Super+Enter\" action=\"spawn\" argument=\"term\"\n",
        );
        let unbound = parse("binding keys=\"Super+Q\" disabled=#true\n");

        let input = replaced.input_settings().unwrap();
        let bindings = replaced.bindings(&input).unwrap();
        assert_eq!(bindings.len(), 48);
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
        assert_eq!(bindings.len(), 47);
        assert!(
            !bindings
                .iter()
                .any(|binding| binding.action == BindingAction::Close)
        );
    }

    #[test]
    fn accepts_physical_xkb_key_names() {
        let config = parse(
            "binding keys=\"Super+AD06\" match=\"physical\" action=\"focus\" argument=\"left\"\n",
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
            "binding keys=\"Super+Q\" action=\"close\"\nbinding keys=\"logo+q\" action=\"close\"\n",
        );
        let missing_command =
            parse("binding keys=\"Super+Enter\" action=\"spawn\" argument=\"missing\"\n");
        let invalid_argument =
            parse("binding keys=\"Super+Q\" action=\"close\" argument=\"left\"\n");

        for config in [duplicate, missing_command, invalid_argument] {
            let input = config.input_settings().unwrap();
            assert!(config.bindings(&input).is_err());
        }
    }

    #[test]
    fn parses_and_validates_window_rules() {
        let config = parse(
            "window-rule app-id=\"org.example.Editor\" workspace=3 floating=#true width=900.0 height=600.0 fullscreen=#false\n",
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
        let catch_all = parse("window-rule floating=#true\n");
        let zero_workspace = parse("window-rule app-id=\"editor\" workspace=0\n");

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
            "output-profile name=\"docked\" {\n    output match=\"HDMI-A-1\" mode=\"3840x2160@119.998\" scale=1.6 {\n        position 0 0\n    }\n    output match=\"eDP-1\" enabled=#false transform=\"rotate_90\"\n}\n",
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
        let invalid_scale =
            parse("output-profile name=\"bad\" {\n    output match=\"eDP-1\" scale=0.0\n}\n");
        let invalid_mode =
            parse("output-profile name=\"bad\" {\n    output match=\"eDP-1\" mode=\"native\"\n}\n");
        let duplicate = parse(
            "output-profile name=\"bad\" {\n    output match=\"eDP-1\"\n    output match=\"eDP-1\"\n}\n",
        );

        assert!(invalid_scale.output_profiles().is_err());
        assert!(invalid_mode.output_profiles().is_err());
        assert!(duplicate.output_profiles().is_err());
    }
}
