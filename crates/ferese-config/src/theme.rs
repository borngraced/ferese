//! Pure theme resolution, appearance scheduling, and readable transition samples.
use std::path::{Path, PathBuf};

use jiff::civil::{DateTime, Time};
use jiff::tz::{Disambiguation, TimeZone};
use jiff::{Timestamp, ToSpan};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::Document;

pub const SCHEMA_VERSION: u32 = 2;
pub const TRANSITION_MS: u64 = 250;

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Appearance {
    Light,
    #[default]
    Dark,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    Light,
    #[default]
    Dark,
    Auto,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default)]
pub struct Accessibility {
    pub increase_contrast: bool,
    pub reduce_transparency: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(default)]
pub struct Selection {
    pub family: Option<String>,
    pub preset: String,
    pub file: Option<PathBuf>,
    #[serde(flatten)]
    pub overrides: serde_json::Map<String, Value>,
}

impl Default for Selection {
    fn default() -> Self {
        Self {
            preset: "ferese-blue".into(),
            family: None,
            file: None,
            overrides: Default::default(),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum AutoSource {
    System,
    SunriseSunset,
    #[default]
    Schedule,
}

#[derive(Clone, Debug, Default)]
pub struct AutoContext {
    pub system: Option<Appearance>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(default)]
pub struct Schedule {
    pub source: AutoSource,
    pub timezone: String,
    pub light_at: String,
    pub dark_at: String,
}

impl Default for Schedule {
    fn default() -> Self {
        Self {
            source: AutoSource::Schedule,
            timezone: "system".into(),
            light_at: "07:00".into(),
            dark_at: "19:00".into(),
        }
    }
}

impl Schedule {
    pub fn resolve(
        &self,
        now: Timestamp,
        zone: &TimeZone,
        context: &AutoContext,
    ) -> Result<(Appearance, Option<Timestamp>, Option<String>), String> {
        match self.source {
            AutoSource::System => Ok((
                context.system.unwrap_or(Appearance::Dark),
                None,
                context
                    .system
                    .is_none()
                    .then(|| "System appearance is unavailable. Using Dark until it becomes available.".into()),
            )),
            AutoSource::Schedule => self
                .boundaries(now, zone)
                .map(|(appearance, next)| (appearance, Some(next), None)),
            AutoSource::SunriseSunset => {
                Err("Sunrise/sunset is not available yet; choose Follow system or Custom schedule.".into())
            }
        }
    }

    pub fn zone(&self) -> Result<TimeZone, String> {
        if self.timezone == "system" {
            if std::env::var_os("TZ").is_some() {
                TimeZone::try_system().map_err(|e| e.to_string())
            } else {
                let bytes = std::fs::read("/etc/localtime").map_err(|e| e.to_string())?;
                TimeZone::tzif("system", &bytes).map_err(|e| e.to_string())
            }
        } else {
            TimeZone::get(&self.timezone).map_err(|e| e.to_string())
        }
    }

    pub fn boundaries(&self, now: Timestamp, zone: &TimeZone) -> Result<(Appearance, Timestamp), String> {
        let parse = |s: &str| -> Result<Time, String> {
            if s.len() != 5 || s.as_bytes()[2] != b':' {
                return Err("theme schedule times must use HH:MM".into());
            }
            format!("{s}:00").parse::<Time>().map_err(|e| e.to_string())
        };
        let light = parse(&self.light_at)?;
        let dark = parse(&self.dark_at)?;
        if light == dark {
            return Err("light-at and dark-at must differ".into());
        }
        let date = now.to_zoned(zone.clone()).date();
        let mut boundaries = Vec::new();
        for days in -2i64..=2 {
            let date = date.checked_add(days.days()).map_err(|e| e.to_string())?;
            for (time, appearance) in [(light, Appearance::Light), (dark, Appearance::Dark)] {
                let civil = DateTime::from_parts(date, time);
                let ambiguous = zone.to_ambiguous_zoned(civil);
                let zoned = match ambiguous.earlier() {
                    Ok(earlier) if earlier.datetime() == civil => earlier,
                    _ => zone
                        .to_ambiguous_zoned(civil)
                        .disambiguate(Disambiguation::Compatible)
                        .map_err(|e| e.to_string())?,
                };
                boundaries.push((zoned.timestamp(), appearance));
            }
        }
        boundaries.sort_by_key(|entry| entry.0);
        let appearance = boundaries
            .iter()
            .rev()
            .find(|entry| entry.0 <= now)
            .map(|entry| entry.1)
            .ok_or("no preceding theme boundary")?;
        let next = boundaries
            .iter()
            .find(|entry| entry.0 > now)
            .map(|entry| entry.0)
            .ok_or("no upcoming theme boundary")?;
        Ok((appearance, next))
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct CustomTheme {
    pub file: PathBuf,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(default)]
pub struct Policy {
    pub family: Option<String>,
    pub split: Option<bool>,
    pub custom_themes: std::collections::BTreeMap<String, CustomTheme>,
    pub mode: Mode,
    pub file: Option<PathBuf>,
    #[serde(deserialize_with = "deserialize_light_selection")]
    pub light: Selection,
    pub dark: Selection,
    pub schedule: Schedule,
    pub accessibility: Accessibility,
    pub accent: Option<String>,
}

fn deserialize_light_selection<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<Selection, D::Error> {
    let mut value = Value::deserialize(deserializer)?;
    if let Some(object) = value.as_object_mut() {
        object.entry("preset").or_insert(json!("ferese-blue-light"));
    }
    serde_json::from_value(value).map_err(serde::de::Error::custom)
}

impl Default for Policy {
    fn default() -> Self {
        Self {
            mode: Mode::Dark,
            family: None,
            split: None,
            custom_themes: Default::default(),
            file: None,
            light: Selection {
                preset: "ferese-blue-light".into(),
                family: None,
                file: None,
                overrides: Default::default(),
            },
            dark: Selection::default(),
            schedule: Schedule::default(),
            accessibility: Accessibility::default(),
            accent: None,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct Colors {
    pub surface_base: String,
    pub surface_raised: String,
    pub application_background: String,
    pub text_primary: String,
    pub text_muted: String,
    pub accent: String,
    pub on_accent: String,
    pub border: String,
    pub shadow: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct Material {
    pub style: String,
    pub opacity: f64,
    pub blur_radius: f64,
    pub tint_strength: f64,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct Geometry {
    pub border_width: f64,
    pub focus_ring_width: f64,
    pub window_radius: f64,
    pub shell_radius: f64,
    pub top_bar_height: f64,
    pub top_bar_margin_top: i32,
    pub top_bar_margin_horizontal: i32,
    pub top_bar_window_gap: i32,
    pub panel_padding: f64,
    pub control_gap: f64,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct Typography {
    pub font_family: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct Background {
    pub path: Option<PathBuf>,
    pub lock_path: Option<PathBuf>,
    pub mode: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct BarSurface {
    pub background: String,
    pub text_primary: String,
    pub text_muted: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct Surfaces {
    pub bar: BarSurface,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct SoftShadow {
    pub offset_y: f64,
    pub blur: f64,
    pub opacity: f64,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct Shadows {
    pub soft: SoftShadow,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct Gradient {
    pub from: String,
    pub to: String,
    pub angle: f64,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
pub struct Paint {
    pub gradient: Option<Gradient>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct Tokens {
    pub colors: Colors,
    pub material: Material,
    pub geometry: Geometry,
    pub typography: Typography,
    pub background: Background,
    pub surface: Surfaces,
    pub shadow: Shadows,
    pub border: Paint,
    pub focus_ring: Paint,
}

impl Default for Tokens {
    fn default() -> Self {
        serde_json::from_value(json!({
            "colors": {"surface_base":"#111821", "surface_raised":"#1E2530", "application_background":"#171E27", "text_primary":"#F4F7FB", "text_muted":"#8793A2",
                "accent":"#3D7BE6", "on_accent":"#FFFFFF", "border":"#FFFFFF18", "shadow":"#00000055"},
            "material":{"style":"solid","opacity":0.78,"blur_radius":12.0,"tint_strength":0.5},
            "geometry":{"border_width":1.0,"focus_ring_width":2.0,"window_radius":14.0,"shell_radius":14.0,
                "top_bar_height":28.0,"top_bar_margin_top":0,"top_bar_margin_horizontal":0,
                "top_bar_window_gap":0,"panel_padding":12.0,"control_gap":12.0},
            "typography":{"font_family":"Inter"},
            "background":{"path":crate::default_wallpaper(),"lock_path":null,"mode":"fill"},
            "surface":{"bar":{"background":"#111821","text_primary":"#F4F7FB","text_muted":"#8793A2"}},
            "shadow":{"soft":{"offset_y":4.0,"blur":18.0,"opacity":0.2}},
            "border":{"gradient":null},"focus_ring":{"gradient":null}
        }))
        .expect("valid built-in theme")
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct ResolvedTheme {
    pub appearance: Appearance,
    pub tokens: Tokens,
    pub requested_accent: String,
    pub accessibility: Accessibility,
    pub reduced_motion: bool,
}

impl Default for ResolvedTheme {
    fn default() -> Self {
        Self {
            appearance: Appearance::Dark,
            requested_accent: "#3D7BE6".into(),
            tokens: Tokens::default(),
            accessibility: Accessibility::default(),
            reduced_motion: false,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct Snapshot {
    pub version: u32,
    pub revision: u64,
    pub mode: Mode,
    pub theme: ResolvedTheme,
    pub presented: ResolvedTheme,
    pub warnings: Vec<String>,
    pub error: Option<String>,
    #[serde(default = "crate::families::builtins")]
    pub families: Vec<crate::families::Family>,
    #[serde(default)]
    pub fallback_note: Option<String>,
}

impl Default for Snapshot {
    fn default() -> Self {
        Self {
            version: SCHEMA_VERSION,
            revision: 0,
            mode: Mode::Dark,
            theme: ResolvedTheme::default(),
            presented: ResolvedTheme::default(),
            warnings: vec![],
            error: None,
            families: crate::families::builtins(),
            fallback_note: None,
        }
    }
}

#[derive(Clone)]
pub struct Candidate {
    pub policy: Policy,
    pub theme: ResolvedTheme,
    pub warnings: Vec<String>,
    pub files: Vec<PathBuf>,
    pub next_transition: Option<Timestamp>,
    pub families: Vec<crate::families::Family>,
    pub fallback_note: Option<String>,
}

pub fn resolve(
    document: &Document,
    directory: &Path,
    now: Timestamp,
    read: impl FnMut(&Path) -> Result<String, String>,
) -> Result<Candidate, String> {
    resolve_with_context(document, directory, now, &AutoContext::default(), read)
}

pub fn resolve_with_context(
    document: &Document,
    directory: &Path,
    now: Timestamp,
    context: &AutoContext,
    mut read: impl FnMut(&Path) -> Result<String, String>,
) -> Result<Candidate, String> {
    let root = document.get("theme").cloned().unwrap_or(json!({}));
    let policy: Policy = serde_json::from_value(root.clone()).map_err(|e| e.to_string())?;
    let zone = if policy.schedule.source == AutoSource::Schedule {
        policy.schedule.zone()?
    } else {
        TimeZone::UTC
    };
    let (scheduled, next, auto_note) = if policy.mode == Mode::Auto || policy.schedule.source == AutoSource::Schedule {
        policy.schedule.resolve(now, &zone, context)?
    } else {
        (Appearance::Dark, None, None)
    };
    let appearance = match policy.mode {
        Mode::Light => Appearance::Light,
        Mode::Dark => Appearance::Dark,
        Mode::Auto => scheduled,
    };
    // Both selections are validated so the next automatic transition cannot reveal a broken chain.
    let mut all_files = vec![];
    let mut sources: std::collections::HashMap<PathBuf, String> = std::collections::HashMap::new();
    let mut warnings = vec![];
    let mut selected = None;
    let mut families = crate::families::builtins();
    let mut imported = std::collections::HashMap::new();
    for (id, custom) in &policy.custom_themes {
        if id.is_empty()
            || id.chars().any(|c| !c.is_ascii_alphanumeric() && c != '-')
            || families.iter().any(|family| family.id == *id)
        {
            return Err(format!("Invalid or duplicate custom theme id: {id}"));
        }
        let path = theme_path(directory, &custom.file);
        all_files.push(path.clone());
        let source = read(&path)?;
        if source.len() > 60 * 1024 {
            return Err(format!("{} exceeds 60 KiB", path.display()));
        }
        let (family, variants, notes) = import_family(id, &source)?;
        warnings.extend(notes);
        families.push(family);
        imported.insert(id.clone(), variants);
    }
    let split = policy.split.unwrap_or_else(|| {
        crate::families::family_id(&policy.light.preset) != crate::families::family_id(&policy.dark.preset)
    });
    let single = policy
        .family
        .as_deref()
        .unwrap_or_else(|| crate::families::family_id(&policy.dark.preset));
    let mut fallback_note = if policy.mode == Mode::Auto { auto_note } else { None };
    for (kind, selection) in [(Appearance::Light, &policy.light), (Appearance::Dark, &policy.dark)] {
        let family_id = if split {
            selection
                .family
                .as_deref()
                .unwrap_or_else(|| crate::families::family_id(&selection.preset))
        } else {
            single
        };
        let variant = imported
            .get(family_id)
            .and_then(|pair: &(Option<Tokens>, Option<Tokens>)| {
                if kind == Appearance::Light {
                    pair.0.clone()
                } else {
                    pair.1.clone()
                }
            });
        let mut tokens = if let Some(tokens) = variant {
            tokens
        } else if let Some(id) = crate::families::variant(family_id, kind) {
            preset(id, kind)?
        } else {
            if !imported.contains_key(family_id)
                && !matches!(family_id, "dracula" | "monochrome" | "monokai" | "ayu-light")
            {
                return Err(format!("Unknown theme family: {family_id}"));
            }
            if kind == appearance {
                let name = families
                    .iter()
                    .find(|family| family.id == family_id)
                    .map_or(family_id, |family| family.name.as_str());
                fallback_note = Some(format!(
                    "{name} has no {} variant. Using Ferese Blue for this appearance.",
                    if kind == Appearance::Light { "light" } else { "dark" }
                ));
            }
            preset(
                if kind == Appearance::Light {
                    "ferese-blue-light"
                } else {
                    "ferese-blue"
                },
                kind,
            )?
        };
        if !families.iter().any(|family| family.id == family_id) {
            let light = crate::families::variant(family_id, Appearance::Light)
                .and_then(|id| preset(id, Appearance::Light).ok());
            let dark =
                crate::families::variant(family_id, Appearance::Dark).and_then(|id| preset(id, Appearance::Dark).ok());
            families.push(crate::families::Family {
                id: family_id.into(),
                name: family_id.into(),
                light: light.as_ref().map(crate::families::Palette::from),
                dark: dark.as_ref().map(crate::families::Palette::from),
            });
        }

        let authored_base = tokens.colors.surface_base.clone();
        let mut explicit_surfaces = std::collections::HashSet::new();
        if root
            .get("geometry")
            .and_then(|value| value.get("shell_radius"))
            .is_none()
            && let Some(radius) = document
                .get("appearance.corner_radius")
                .and_then(Value::as_f64)
                .or_else(|| document.get("theme.geometry.top_bar_radius").and_then(Value::as_f64))
        {
            tokens.geometry.shell_radius = radius;
        }
        let mut value = serde_json::to_value(&tokens).map_err(|e| e.to_string())?;
        for file in [&policy.file, &selection.file].into_iter().flatten() {
            let path = theme_path(directory, file);
            if !all_files.contains(&path) {
                all_files.push(path.clone());
            }
            let source = if let Some(source) = sources.get(&path) {
                source.clone()
            } else {
                let source = read(&path)?;
                sources.insert(path.clone(), source.clone());
                source
            };
            if source.len() > 60 * 1024 {
                return Err(format!("{} exceeds 60 KiB", path.display()));
            }
            let layer = Document::parse(&source).map_err(|e| format!("{}: {e}", path.display()))?;
            let layer = layer.get("theme").unwrap_or(layer.value());
            for role in ["surface_raised", "application_background"] {
                if layer.get("colors").and_then(|colors| colors.get(role)).is_some() {
                    explicit_surfaces.insert(role);
                }
            }
            merge(&mut value, layer, "", &mut warnings, false)?;
        }
        merge(&mut value, &root, "", &mut warnings, true)?;
        let mut overrides = selection.overrides.clone();
        if !split && policy.family.is_some() {
            for key in ["colors", "surface", "border", "focus_ring"] {
                overrides.remove(key);
            }
        }
        merge(&mut value, &Value::Object(overrides.clone()), "", &mut warnings, false)?;
        tokens = serde_json::from_value(value).map_err(|e| e.to_string())?;
        for layer in [&root, &Value::Object(overrides.clone())] {
            for role in ["surface_raised", "application_background"] {
                if layer.get("colors").and_then(|colors| colors.get(role)).is_some() {
                    explicit_surfaces.insert(role);
                }
            }
        }
        let base = rgba(&tokens.colors.surface_base)?;
        let shade = if kind == Appearance::Light {
            [0., 0., 0., 1.]
        } else {
            [1.; 4]
        };
        if tokens.colors.surface_base != authored_base && !explicit_surfaces.contains("surface_raised") {
            tokens.colors.surface_raised = hex(blend(base, shade, 0.055));
        }
        if tokens.colors.surface_base != authored_base && !explicit_surfaces.contains("application_background") {
            tokens.colors.application_background = hex(blend(base, shade, 0.025));
        }
        if let Some(accent) = &policy.accent {
            tokens.colors.accent = accent.clone();
        }
        for path in [&mut tokens.background.path, &mut tokens.background.lock_path]
            .into_iter()
            .flatten()
        {
            *path = theme_path(directory, path);
        }
        validate(&tokens)?;
        let requested_accent = tokens.colors.accent.clone();
        transform(&mut tokens, &policy.accessibility, &mut warnings);
        let theme = ResolvedTheme {
            appearance: kind,
            tokens,
            requested_accent,
            accessibility: policy.accessibility.clone(),
            reduced_motion: document
                .get("animations.reduced_motion")
                .and_then(Value::as_bool)
                .unwrap_or(false)
                || document.get("animations.enabled").and_then(Value::as_bool) == Some(false),
        };
        if kind == appearance {
            selected = Some(theme);
        }
    }
    warnings.sort();
    warnings.dedup();
    Ok(Candidate {
        policy: policy.clone(),
        theme: selected.expect("selected appearance"),
        warnings,
        files: all_files,
        next_transition: if policy.mode == Mode::Auto { next } else { None },
        families,
        fallback_note,
    })
}

pub fn theme_path(directory: &Path, path: &Path) -> PathBuf {
    let path = if let Some(tail) = path.to_str().and_then(|path| path.strip_prefix("~/")) {
        std::env::var_os("HOME")
            .map(|home| PathBuf::from(home).join(tail))
            .unwrap_or_else(|| directory.join(path))
    } else if path.is_absolute() {
        path.to_owned()
    } else {
        directory.join(path)
    };
    if let (Some(parent), Some(name)) = (path.parent(), path.file_name())
        && let Ok(parent) = parent.canonicalize()
    {
        return parent.join(name);
    }
    path
}

fn merge(
    target: &mut Value,
    layer: &Value,
    prefix: &str,
    warnings: &mut Vec<String>,
    inline: bool,
) -> Result<(), String> {
    let map = layer
        .as_object()
        .ok_or_else(|| format!("theme {prefix} must be a section"))?;
    for (key, value) in map {
        if prefix.is_empty()
            && inline
            && [
                "mode",
                "file",
                "light",
                "dark",
                "schedule",
                "accessibility",
                "accent",
                "family",
                "split",
                "custom_themes",
            ]
            .contains(&key.as_str())
        {
            continue;
        }
        if prefix == "geometry" && key == "top_bar_radius" {
            continue;
        }
        let path = if prefix.is_empty() {
            key.clone()
        } else {
            format!("{prefix}.{key}")
        };
        let Some(current) = target.get_mut(key) else {
            warnings.push(format!("Unknown theme token: {path}"));
            continue;
        };
        if current.is_null() && path.ends_with(".gradient") && value.is_object() {
            *current = json!({"from": "#3D7BE6", "to": "#3D7BE6", "angle": 0.0});
        }
        if current.is_object() {
            merge(current, value, &path, warnings, inline)?;
        } else {
            *current = value.clone();
        }
    }
    Ok(())
}

pub fn preset(name: &str, appearance: Appearance) -> Result<Tokens, String> {
    if let Some(preset) = crate::presets::PRESETS.iter().find(|preset| preset.id == name) {
        if preset.appearance != appearance {
            return Err(format!("Preset {name} does not support {appearance:?}"));
        }
        let mut tokens = Tokens::default();
        tokens.colors.surface_base = preset.base.into();
        tokens.colors.surface_raised = preset.raised.into();
        tokens.colors.application_background = preset.application_background.into();
        tokens.colors.text_primary = preset.text.into();
        tokens.colors.text_muted = preset.muted.into();
        tokens.colors.accent = preset.accent.into();
        tokens.colors.on_accent = hex(readable(rgba(preset.text)?, rgba(preset.accent)?, 4.5));
        tokens.colors.border = preset.border.into();
        tokens.colors.shadow = if appearance == Appearance::Light {
            "#00000022"
        } else {
            "#00000055"
        }
        .into();
        tokens.surface.bar = BarSurface {
            background: preset.base.into(),
            text_primary: preset.text.into(),
            text_muted: preset.muted.into(),
        };
        tokens.material.opacity = preset.opacity;
        tokens.material.blur_radius = preset.blur_radius;
        tokens.material.tint_strength = preset.tint_strength;
        tokens.shadow.soft.opacity = preset.shadow_opacity;
        return Ok(tokens);
    }
    let mut t = Tokens::default();
    let (base, text, muted, accent, border) = match name {
        "monochrome" => ("#101012", "#EDEDF0", "#97979F", "#E5E5E5", "#FFFFFF18"),
        "dracula" => ("#282A36", "#F8F8F2", "#A4ADCD", "#BD93F9", "#44475A"),
        "ayu-light" => ("#F8F9FA", "#5C6166", "#596574", "#8F5300", "#C8CDD3"),
        "monokai" => ("#272822", "#F8F8F2", "#B2B29F", "#A6E22E", "#414339"),
        _ => return Err(format!("Unknown theme preset: {name}")),
    };
    let light_preset = name == "ayu-light";
    if light_preset != (appearance == Appearance::Light) {
        return Err(format!("Preset {name} does not support {appearance:?}"));
    }
    t.colors.surface_base = base.into();
    let shade = if appearance == Appearance::Light {
        [0., 0., 0., 1.]
    } else {
        [1.; 4]
    };
    t.colors.surface_raised = hex(blend(rgba(base).unwrap(), shade, 0.055));
    t.colors.application_background = hex(blend(rgba(base).unwrap(), shade, 0.025));
    t.colors.text_primary = text.into();
    t.colors.text_muted = muted.into();
    t.colors.accent = accent.into();
    t.colors.border = border.into();
    t.surface.bar = BarSurface {
        background: base.into(),
        text_primary: text.into(),
        text_muted: muted.into(),
    };
    if light_preset {
        t.material.opacity = 0.94;
        t.material.tint_strength = 1.0;
        t.material.blur_radius = 10.0;
        t.shadow.soft.opacity = 0.12;
    }
    Ok(t)
}

pub fn authored_warnings(tokens: &Tokens) -> Vec<String> {
    if let Err(error) = validate(tokens) {
        return vec![error];
    }
    let mut warnings = Vec::new();
    let surfaces = [
        ("surface-base", &tokens.colors.surface_base),
        ("surface-raised", &tokens.colors.surface_raised),
        ("application-background", &tokens.colors.application_background),
    ];
    let check = |foreground: &str, background: &str, minimum: f64| {
        let foreground = rgba(foreground).unwrap();
        let background = rgba(background).unwrap();
        contrast(composite(foreground, background), background) >= minimum
    };
    for (name, background) in surfaces {
        for (role, foreground) in [
            ("text-primary", &tokens.colors.text_primary),
            ("text-muted", &tokens.colors.text_muted),
        ] {
            if !check(foreground, background, 4.5) {
                warnings.push(format!("Low contrast: {role} on {name}"));
            }
        }
        if !check(&tokens.colors.accent, background, 3.) {
            warnings.push(format!("Low contrast: accent on {name}"));
        }
    }
    for (role, foreground) in [
        ("bar.text-primary", &tokens.surface.bar.text_primary),
        ("bar.text-muted", &tokens.surface.bar.text_muted),
    ] {
        if !check(foreground, &tokens.surface.bar.background, 4.5) {
            warnings.push(format!("Low contrast: {role}"));
        }
    }
    if !check(&tokens.colors.on_accent, &tokens.colors.accent, 4.5) {
        warnings.push("Low contrast: on-accent".into());
    }
    warnings
}

fn validate(tokens: &Tokens) -> Result<(), String> {
    let value = serde_json::to_value(tokens).map_err(|e| e.to_string())?;
    fn visit(value: &Value, path: &str) -> Result<(), String> {
        match value {
            Value::Object(map) => {
                for (k, v) in map {
                    visit(v, &format!("{path}.{k}"))?;
                }
            }
            Value::Number(n) => {
                let n = n.as_f64().ok_or("invalid theme number")?;
                if !n.is_finite() || (path != ".shadow.soft.offset_y" && n < 0.) {
                    return Err(format!("Invalid theme value {path}"));
                }
                if (path.ends_with("opacity") || path.ends_with("tint_strength")) && n > 1. {
                    return Err(format!("Invalid opacity {path}"));
                }
            }
            Value::String(s)
                if path.starts_with(".colors.")
                    || path.starts_with(".surface.")
                    || path.ends_with(".from")
                    || path.ends_with(".to") =>
            {
                rgba(s)?;
            }
            _ => {}
        }
        Ok(())
    }
    visit(&value, "")?;
    if !["solid", "translucent"].contains(&tokens.material.style.as_str()) {
        return Err("Unknown material style".into());
    }
    if !["fill", "fit"].contains(&tokens.background.mode.as_str()) {
        return Err("Unknown wallpaper mode".into());
    }
    if tokens.geometry.top_bar_height <= 0. {
        return Err("top-bar-height must be positive".into());
    }
    if tokens.typography.font_family.len() > 128 {
        return Err("font-family exceeds 128 bytes".into());
    }
    Ok(())
}

pub fn rgba(s: &str) -> Result<[f64; 4], String> {
    let hex = s.strip_prefix('#').ok_or_else(|| format!("Invalid color: {s}"))?;
    if !hex.is_ascii() || !matches!(hex.len(), 6 | 8) {
        return Err(format!("Invalid color: {s}"));
    }
    let mut out = [1.; 4];
    for (i, c) in out.iter_mut().enumerate().take(hex.len() / 2) {
        *c = f64::from(u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).map_err(|_| format!("Invalid color: {s}"))?)
            / 255.;
    }
    Ok(out)
}

pub fn hex(c: [f64; 4]) -> String {
    format!(
        "#{:02X}{:02X}{:02X}{:02X}",
        (c[0].clamp(0., 1.) * 255.).round() as u8,
        (c[1].clamp(0., 1.) * 255.).round() as u8,
        (c[2].clamp(0., 1.) * 255.).round() as u8,
        (c[3].clamp(0., 1.) * 255.).round() as u8
    )
}

pub fn luminance(c: [f64; 4]) -> f64 {
    let linear = |v: f64| {
        if v <= 0.04045 {
            v / 12.92
        } else {
            ((v + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * linear(c[0]) + 0.7152 * linear(c[1]) + 0.0722 * linear(c[2])
}

pub fn contrast(a: [f64; 4], b: [f64; 4]) -> f64 {
    let (a, b) = (luminance(a), luminance(b));
    (a.max(b) + 0.05) / (a.min(b) + 0.05)
}

pub fn composite(fg: [f64; 4], bg: [f64; 4]) -> [f64; 4] {
    let mut c = [0.; 4];
    for i in 0..3 {
        c[i] = fg[i] * fg[3] + bg[i] * (1. - fg[3]);
    }
    c[3] = 1.;
    c
}

fn blend(a: [f64; 4], b: [f64; 4], p: f64) -> [f64; 4] {
    std::array::from_fn(|i| a[i] + (b[i] - a[i]) * p)
}

pub fn readable(preferred: [f64; 4], background: [f64; 4], minimum: f64) -> [f64; 4] {
    let mut preferred = preferred;
    preferred[3] = 1.;
    if contrast(preferred, background) >= minimum {
        return preferred;
    }
    let black = [0., 0., 0., 1.];
    let white = [1., 1., 1., 1.];
    let target = if contrast(black, background) > contrast(white, background) {
        black
    } else {
        white
    };
    for step in 1..=100 {
        let c = blend(preferred, target, f64::from(step) / 100.);
        if contrast(rgba(&hex(c)).expect("generated color"), background) >= minimum {
            return c;
        }
    }
    target
}

fn readable_across(mut preferred: [f64; 4], backgrounds: &[[f64; 4]], minimum: f64) -> [f64; 4] {
    preferred[3] = 1.;
    let score = |color| {
        backgrounds
            .iter()
            .map(|background| contrast(color, *background))
            .fold(f64::INFINITY, f64::min)
    };
    if score(preferred) >= minimum {
        return preferred;
    }
    let black = [0., 0., 0., 1.];
    let white = [1.; 4];
    let target = if score(black) > score(white) { black } else { white };
    for step in 1..=100 {
        let color = rgba(&hex(blend(preferred, target, f64::from(step) / 100.))).unwrap();
        if score(color) >= minimum {
            return color;
        }
    }
    target
}

fn transform(t: &mut Tokens, accessibility: &Accessibility, warnings: &mut Vec<String>) {
    let surface = rgba(&t.colors.surface_base).expect("validated surface");
    let mut requested = rgba(&t.colors.accent).expect("validated accent");
    requested[3] = 1.;
    let backgrounds = [
        surface,
        rgba(&t.surface.bar.background).expect("validated bar"),
        rgba(&t.colors.surface_raised).expect("validated raised surface"),
        rgba(&t.colors.application_background).expect("validated application background"),
    ];
    let preferred = rgba(&t.colors.text_primary).expect("validated foreground");
    let score = |color| {
        backgrounds
            .iter()
            .map(|background| contrast(color, *background))
            .fold(f64::INFINITY, f64::min)
    };
    let choose = |preserve_foreground: bool| {
        let acceptable = |color| score(color) >= 3. && (!preserve_foreground || contrast(preferred, color) >= 4.5);
        if acceptable(requested) {
            return Some(requested);
        }
        let mut best = None;
        let mut best_distance = f64::INFINITY;
        for target in [[0., 0., 0., 1.], [1.; 4]] {
            for step in 0..=1000 {
                let distance = f64::from(step) / 1000.;
                let color = rgba(&hex(blend(requested, target, distance))).unwrap();
                if acceptable(color) && distance < best_distance {
                    best = Some(color);
                    best_distance = distance;
                }
            }
        }
        best
    };
    let adjusted = choose(true).or_else(|| choose(false)).unwrap_or_else(|| {
        warnings.push("Accent cannot reach 3:1 contrast on all configured surfaces".into());
        readable(requested, surface, 3.)
    });
    t.colors.accent = hex(adjusted);
    t.colors.on_accent = hex(readable(preferred, adjusted, 4.5));
    let minimum = if accessibility.increase_contrast { 7. } else { 4.5 };
    let body = [surface, backgrounds[2], backgrounds[3]];
    let bar = [backgrounds[1]; 3];
    for (label, color, backgrounds) in [
        ("text-primary", &mut t.colors.text_primary, body),
        ("text-muted", &mut t.colors.text_muted, body),
        ("bar.text-primary", &mut t.surface.bar.text_primary, bar),
        ("bar.text-muted", &mut t.surface.bar.text_muted, bar),
    ] {
        if accessibility.increase_contrast {
            *color = hex(readable_across(
                rgba(color).expect("validated text"),
                &backgrounds,
                minimum,
            ));
        }
        let c = rgba(color).expect("validated text");
        if backgrounds
            .iter()
            .any(|background| contrast(composite(c, *background), *background) < minimum)
        {
            warnings.push(format!("Low contrast: {label}"));
        }
    }
    if accessibility.increase_contrast {
        t.colors.border = hex(readable(rgba(&t.colors.border).unwrap(), surface, 3.));
        resolve_material_contrast(t, minimum);
    }
    if accessibility.reduce_transparency {
        t.material.style = "solid".into();
        t.material.opacity = 1.;
        t.material.blur_radius = 0.;
    }
}

fn resolve_material_contrast(tokens: &mut Tokens, minimum: f64) {
    if tokens.material.style == "solid" {
        return;
    }
    let surface = rgba(&tokens.colors.surface_base).unwrap();
    let bar = rgba(&tokens.surface.bar.background).unwrap();
    let raised = rgba(&tokens.colors.surface_raised).unwrap();
    let app = rgba(&tokens.colors.application_background).unwrap();
    let initial = tokens.material.opacity * tokens.material.tint_strength;
    let readable = |backgrounds: &[[f64; 4]]| {
        [[0., 0., 0., 1.], [1.; 4]].into_iter().any(|text| {
            backgrounds
                .iter()
                .all(|background| contrast(text, *background) >= minimum)
        })
    };
    let opacity = (0..=100)
        .map(|step| initial + (1. - initial) * f64::from(step) / 100.)
        .find(|opacity| {
            let bounds = material_bounds(surface, *opacity);
            readable(&[bounds[0], bounds[1], raised, app]) && readable(&material_bounds(bar, *opacity))
        })
        .unwrap_or(1.0);
    tokens.material.opacity = tokens.material.opacity.max(opacity);
    tokens.material.tint_strength = opacity / tokens.material.opacity.max(f64::EPSILON);
    let bounds = material_bounds(surface, opacity);
    let body = [bounds[0], bounds[1], raised, app];
    let bar = material_bounds(bar, opacity);
    for (color, backgrounds) in [
        (&mut tokens.colors.text_primary, body.as_slice()),
        (&mut tokens.colors.text_muted, body.as_slice()),
        (&mut tokens.surface.bar.text_primary, bar.as_slice()),
        (&mut tokens.surface.bar.text_muted, bar.as_slice()),
    ] {
        *color = hex(readable_across(rgba(color).unwrap(), backgrounds, minimum));
    }
}

fn material_bounds(mut surface: [f64; 4], opacity: f64) -> [[f64; 4]; 2] {
    surface[3] = opacity;
    [composite(surface, [0., 0., 0., 1.]), composite(surface, [1.; 4])]
}

fn material_foreground(preferred: [f64; 4], backgrounds: [[f64; 4]; 2]) -> [f64; 4] {
    let black = [0., 0., 0., 1.];
    let white = [1.; 4];
    let (target, worst) = if contrast(black, backgrounds[0]) >= contrast(white, backgrounds[1]) {
        (black, backgrounds[0])
    } else {
        (white, backgrounds[1])
    };
    let result = readable(preferred, worst, 4.5);
    let outside = luminance(result) < luminance(backgrounds[0]) || luminance(result) > luminance(backgrounds[1]);
    if outside
        && backgrounds
            .iter()
            .all(|background| contrast(rgba(&hex(result)).unwrap(), *background) >= 4.5)
    {
        result
    } else {
        target
    }
}

impl ResolvedTheme {
    /// Bound contrast across all backdrops instead of interpolating foregrounds.
    pub fn transition(&self, to: &Self, progress: f64) -> Self {
        if to.reduced_motion || self.accessibility != to.accessibility || progress >= 1.0 {
            return to.clone();
        }
        if progress <= 0.0 || self == to {
            return self.clone();
        }
        let p = progress.clamp(0., 1.);
        let p = p * p * (3. - 2. * p);
        let mut frame = to.clone();
        let mix = |a: &str, b: &str| hex(blend(rgba(a).unwrap(), rgba(b).unwrap(), p));
        frame.tokens.colors.surface_base = mix(&self.tokens.colors.surface_base, &to.tokens.colors.surface_base);
        frame.tokens.colors.surface_raised = mix(&self.tokens.colors.surface_raised, &to.tokens.colors.surface_raised);
        frame.tokens.colors.application_background = mix(
            &self.tokens.colors.application_background,
            &to.tokens.colors.application_background,
        );
        frame.tokens.colors.accent = mix(&self.tokens.colors.accent, &to.tokens.colors.accent);
        frame.tokens.colors.border = mix(&self.tokens.colors.border, &to.tokens.colors.border);
        frame.tokens.colors.shadow = mix(&self.tokens.colors.shadow, &to.tokens.colors.shadow);
        frame.tokens.surface.bar.background =
            mix(&self.tokens.surface.bar.background, &to.tokens.surface.bar.background);
        frame.tokens.material.opacity =
            self.tokens.material.opacity + (to.tokens.material.opacity - self.tokens.material.opacity) * p;
        frame.tokens.material.tint_strength = self.tokens.material.tint_strength
            + (to.tokens.material.tint_strength - self.tokens.material.tint_strength) * p;
        frame.tokens.shadow.soft.opacity =
            self.tokens.shadow.soft.opacity + (to.tokens.shadow.soft.opacity - self.tokens.shadow.soft.opacity) * p;
        let preferred = if p < 0.5 { self } else { to };
        frame.appearance = preferred.appearance;
        if self.appearance == to.appearance {
            let body = [
                rgba(&frame.tokens.colors.surface_base).unwrap(),
                rgba(&frame.tokens.colors.surface_raised).unwrap(),
                rgba(&frame.tokens.colors.application_background).unwrap(),
            ];
            let minimum = if to.accessibility.increase_contrast { 7.0 } else { 4.5 };
            frame.tokens.colors.text_primary = hex(readable_across(
                rgba(&preferred.tokens.colors.text_primary).unwrap(),
                &body,
                minimum,
            ));
            frame.tokens.colors.text_muted = hex(readable_across(
                rgba(&preferred.tokens.colors.text_muted).unwrap(),
                &body,
                minimum,
            ));
            let bar = rgba(&frame.tokens.surface.bar.background).unwrap();
            frame.tokens.surface.bar.text_primary = hex(readable(
                rgba(&preferred.tokens.surface.bar.text_primary).unwrap(),
                bar,
                minimum,
            ));
            frame.tokens.surface.bar.text_muted = hex(readable(
                rgba(&preferred.tokens.surface.bar.text_muted).unwrap(),
                bar,
                minimum,
            ));
            frame.tokens.colors.on_accent = hex(readable(
                rgba(&preferred.tokens.colors.on_accent).unwrap(),
                rgba(&frame.tokens.colors.accent).unwrap(),
                4.5,
            ));
            return frame;
        }
        let base = rgba(&frame.tokens.colors.surface_base).unwrap();
        let raised = rgba(&frame.tokens.colors.surface_raised).unwrap();
        let app = rgba(&frame.tokens.colors.application_background).unwrap();
        let minimum = [base, raised, app]
            .into_iter()
            .map(luminance)
            .fold(f64::INFINITY, f64::min);
        let maximum = [base, raised, app].into_iter().map(luminance).fold(0., f64::max);
        if ((minimum + 0.05) / 0.05).max(1.05 / (maximum + 0.05)) < 4.5 {
            frame.tokens.colors.surface_raised = frame.tokens.colors.surface_base.clone();
            frame.tokens.colors.application_background = frame.tokens.colors.surface_base.clone();
        }
        let surface = rgba(&frame.tokens.colors.surface_base).unwrap();
        let bar = rgba(&frame.tokens.surface.bar.background).unwrap();
        let mut opacity = if frame.tokens.material.style == "solid" {
            1.
        } else {
            frame.tokens.material.opacity * frame.tokens.material.tint_strength
        };
        for step in 0..=100 {
            let candidate = opacity + (1. - opacity) * f64::from(step) / 100.;
            let backgrounds = material_bounds(surface, candidate);
            let raised = rgba(&frame.tokens.colors.surface_raised).unwrap();
            let app = rgba(&frame.tokens.colors.application_background).unwrap();
            let all = [backgrounds[0], backgrounds[1], raised, app];
            let minimum = all.into_iter().map(luminance).fold(f64::INFINITY, f64::min);
            let maximum = all.into_iter().map(luminance).fold(0., f64::max);
            let bar = material_bounds(bar, candidate);
            let readable = ((minimum + 0.05) / 0.05).max(1.05 / (maximum + 0.05)) >= 4.5
                && contrast([0., 0., 0., 1.], bar[0]).max(contrast([1.; 4], bar[1])) >= 4.5;
            if readable {
                opacity = candidate;
                break;
            }
        }
        if opacity > frame.tokens.material.opacity {
            frame.tokens.material.opacity = opacity;
            frame.tokens.material.tint_strength = 1.;
        } else if frame.tokens.material.opacity > 0. {
            frame.tokens.material.tint_strength = opacity / frame.tokens.material.opacity;
        }
        let surface = material_bounds(surface, opacity);
        let bar = material_bounds(bar, opacity);
        let raised = rgba(&frame.tokens.colors.surface_raised).unwrap();
        let app = rgba(&frame.tokens.colors.application_background).unwrap();
        let body_backgrounds = [surface[0], surface[1], raised, app];
        let body_bounds = [
            *body_backgrounds
                .iter()
                .min_by(|a, b| luminance(**a).total_cmp(&luminance(**b)))
                .unwrap(),
            *body_backgrounds
                .iter()
                .max_by(|a, b| luminance(**a).total_cmp(&luminance(**b)))
                .unwrap(),
        ];
        frame.tokens.colors.text_primary = hex(material_foreground(
            rgba(&preferred.tokens.colors.text_primary).unwrap(),
            body_bounds,
        ));
        frame.tokens.colors.text_muted = hex(material_foreground(
            rgba(&preferred.tokens.colors.text_muted).unwrap(),
            body_bounds,
        ));
        frame.tokens.surface.bar.text_primary = hex(material_foreground(
            rgba(&preferred.tokens.surface.bar.text_primary).unwrap(),
            bar,
        ));
        frame.tokens.surface.bar.text_muted = hex(material_foreground(
            rgba(&preferred.tokens.surface.bar.text_muted).unwrap(),
            bar,
        ));
        frame.tokens.colors.on_accent = hex(readable(
            rgba(&preferred.tokens.colors.on_accent).unwrap(),
            rgba(&frame.tokens.colors.accent).unwrap(),
            4.5,
        ));
        frame
    }
}

pub type ImportedFamily = (crate::families::Family, (Option<Tokens>, Option<Tokens>), Vec<String>);

pub fn import_family(id: &str, source: &str) -> Result<ImportedFamily, String> {
    let document = Document::parse(source).map_err(|e| e.to_string())?;
    let root = document.get("theme").unwrap_or(document.value());
    let name = root.get("name").and_then(Value::as_str).unwrap_or(id).to_owned();
    let mut variants = [None, None];
    let mut warnings = vec![];
    for (index, (key, appearance)) in [("light", Appearance::Light), ("dark", Appearance::Dark)]
        .into_iter()
        .enumerate()
    {
        if let Some(layer) = root.get(key) {
            let base = preset(
                if appearance == Appearance::Light {
                    "ferese-blue-light"
                } else {
                    "ferese-blue"
                },
                appearance,
            )?;
            let mut value = serde_json::to_value(base).map_err(|e| e.to_string())?;
            merge(&mut value, layer, "", &mut warnings, false)?;
            let tokens: Tokens = serde_json::from_value(value).map_err(|e| e.to_string())?;
            validate(&tokens)?;
            warnings.extend(authored_warnings(&tokens));
            variants[index] = Some(tokens);
        }
    }
    if variants.iter().all(Option::is_none) {
        return Err("A theme file must declare a light and/or dark section; partial palettes are allowed.".into());
    }
    for key in root.as_object().ok_or("Theme file must be a section")?.keys() {
        if !matches!(key.as_str(), "name" | "light" | "dark") {
            warnings.push(format!("Unknown theme token: {key}"));
        }
    }
    let [light, dark] = variants;
    let family = crate::families::Family {
        id: id.into(),
        name,
        light: light.as_ref().map(crate::families::Palette::from),
        dark: dark.as_ref().map(crate::families::Palette::from),
    };
    Ok((family, (light, dark), warnings))
}

#[cfg(test)]
mod tests {
    #[test]
    fn resolved_shell_radius_preserves_fractional_values() {
        for (requested, expected) in [(0.0, 0.0), (0.4, 0.4), (13.4, 13.4), (13.5, 13.5)] {
            let document = crate::Document::parse(&format!(
                "theme {{ geometry {{ shell-radius {requested}; window-radius 7.25; }} }}"
            ))
            .unwrap();
            let result = super::resolve(&document, std::path::Path::new("/tmp"), jiff::Timestamp::now(), |_| {
                Err("unexpected theme file".into())
            })
            .unwrap();
            assert_eq!(result.theme.tokens.geometry.shell_radius, expected);
            assert_eq!(result.theme.tokens.geometry.window_radius, 7.25);
        }
    }

    use super::*;

    fn candidate(source: &str) -> Result<Candidate, String> {
        resolve(
            &Document::parse(source).unwrap(),
            Path::new("/config/ferese"),
            "2026-09-30T12:00:00Z".parse().unwrap(),
            |_| Err("unexpected file read".into()),
        )
    }

    #[test]
    fn every_bundled_pair_has_authored_materials_and_readable_tokens() {
        use crate::presets::PRESETS;
        for entry in &PRESETS {
            let tokens = preset(entry.id, entry.appearance).unwrap();
            assert!(
                authored_warnings(&tokens).is_empty(),
                "{}: {:?}",
                entry.id,
                authored_warnings(&tokens)
            );
            let source = match entry.appearance {
                Appearance::Light => format!("theme {{ mode \"light\"; light {{ preset \"{}\"; }}; }}", entry.id),
                Appearance::Dark => format!("theme {{ mode \"dark\"; dark {{ preset \"{}\"; }}; }}", entry.id),
            };
            let resolved = candidate(&source).unwrap().theme;
            assert_eq!(resolved.tokens.colors.surface_raised, tokens.colors.surface_raised);
            assert_eq!(
                resolved.tokens.colors.application_background,
                tokens.colors.application_background
            );
            assert!(authored_warnings(&resolved.tokens).is_empty(), "{}", entry.id);
        }
        let dark: Vec<_> = PRESETS.iter().filter(|p| p.appearance == Appearance::Dark).collect();
        let light: Vec<_> = PRESETS.iter().filter(|p| p.appearance == Appearance::Light).collect();
        assert_eq!(dark.len(), 6);
        assert_eq!(light.len(), 6);
        for (dark, light) in dark.iter().zip(light) {
            assert_eq!(dark.name, light.name);
            assert_ne!(dark.base, light.base);
            assert_ne!(dark.opacity, light.opacity);
            assert_ne!(dark.tint_strength, light.tint_strength);
            assert_ne!(dark.shadow_opacity, light.shadow_opacity);
        }
    }

    #[test]
    fn bundled_pairs_support_accessibility_and_custom_accents_on_every_surface() {
        for entry in &crate::presets::PRESETS {
            for increase in [false, true] {
                for reduce in [false, true] {
                    for accent in ["#3D7BE6", "#E5E5E5", "#111821"] {
                        let kind = if entry.appearance == Appearance::Light {
                            "light"
                        } else {
                            "dark"
                        };
                        let source = format!(
                            r##"theme {{ mode "{kind}"; {kind} {{ preset "{}"; }}; accent "{accent}"; accessibility {{ increase-contrast #{increase}; reduce-transparency #{reduce}; }}; material {{ style "translucent"; }}; }}"##,
                            entry.id
                        );
                        let result = candidate(&source).unwrap();
                        assert!(result.warnings.is_empty(), "{}: {:?}", entry.id, result.warnings);
                        let theme = result.theme;
                        assert!(authored_warnings(&theme.tokens).is_empty(), "{}", entry.id);
                        if increase {
                            for background in [
                                &theme.tokens.colors.surface_base,
                                &theme.tokens.colors.surface_raised,
                                &theme.tokens.colors.application_background,
                            ] {
                                for foreground in [&theme.tokens.colors.text_primary, &theme.tokens.colors.text_muted] {
                                    assert!(
                                        contrast(rgba(foreground).unwrap(), rgba(background).unwrap()) >= 7.,
                                        "{}: {foreground} on {background}",
                                        entry.id
                                    );
                                }
                            }
                        }
                        if reduce {
                            assert_eq!(theme.tokens.material.style, "solid");
                            assert_eq!(theme.tokens.material.blur_radius, 0.);
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn selecting_a_family_resolves_each_variant_and_preserves_a_stored_pair() {
        for (id, _, light, dark) in crate::families::BUILTINS {
            for (mode, expected) in [("light", light), ("dark", dark)] {
                let source = format!(
                    r#"theme {{ mode "{mode}"; family "{id}"; split #false; light {{ family "gruvbox"; }}; dark {{ family "catppuccin"; }}; }}"#
                );
                let resolved = candidate(&source).unwrap();
                let appearance = if mode == "light" {
                    Appearance::Light
                } else {
                    Appearance::Dark
                };
                assert_eq!(
                    resolved.theme.tokens.colors.surface_base,
                    preset(expected, appearance).unwrap().colors.surface_base
                );
                assert_eq!(resolved.policy.light.family.as_deref(), Some("gruvbox"));
                assert_eq!(resolved.policy.dark.family.as_deref(), Some("catppuccin"));
                assert_eq!(resolved.families.len(), 6);
            }
        }
        let resolved = candidate(r#"theme { mode "light"; family "catppuccin"; split #true; light { family "gruvbox"; }; dark { family "everforest"; }; }"#).unwrap();
        assert_eq!(
            resolved.theme.tokens.colors.surface_base,
            preset("gruvbox-light", Appearance::Light).unwrap().colors.surface_base
        );
    }

    #[test]
    fn single_variant_families_use_an_explicit_default_fallback() {
        let resolved = candidate(r#"theme { mode "light"; family "dracula"; split #false; }"#).unwrap();
        assert_eq!(
            resolved.theme.tokens.colors.surface_base,
            preset("ferese-blue-light", Appearance::Light)
                .unwrap()
                .colors
                .surface_base
        );
        assert!(resolved.fallback_note.unwrap().contains("no light variant"));
        assert!(
            resolved
                .families
                .iter()
                .any(|family| family.id == "dracula" && family.light.is_none())
        );
        let resolved = candidate(r#"theme { mode "dark"; family "dracula"; split #false; }"#).unwrap();
        assert!(resolved.fallback_note.is_none());
        assert_eq!(resolved.theme.tokens.colors.surface_base, "#282A36");
    }

    #[test]
    fn imported_partial_families_are_validated_by_the_owner_and_watched() {
        let document = Document::parse(
            r#"theme { mode "light"; family "custom-test"; custom-themes { custom-test { file "test.kdl"; }; }; }"#,
        )
        .unwrap();
        let resolved = resolve(
            &document,
            Path::new("/tmp"),
            "2026-09-30T12:00:00Z".parse().unwrap(),
            |_| Ok(r##"theme { name "Test"; light { colors { accent "#8F5300"; }; future-token 2; }; }"##.into()),
        )
        .unwrap();
        assert_eq!(resolved.theme.requested_accent, "#8F5300");
        let imported = resolved
            .families
            .iter()
            .find(|family| family.id == "custom-test")
            .unwrap();
        assert_eq!(imported.name, "Test");
        assert!(imported.dark.is_none());
        assert!(resolved.files.contains(&PathBuf::from("/tmp/test.kdl")));
        assert!(resolved.warnings.iter().any(|warning| warning.contains("future_token")));
        assert!(
            resolve(&document, Path::new("/tmp"), Timestamp::now(), |_| Ok(
                r#"theme { light { colors { accent "invalid"; }; }; }"#.into()
            ))
            .is_err()
        );
        assert!(import_family("custom-test", "colors { accent \"#FFFFFF\"; }").is_err());
    }

    #[test]
    fn system_auto_is_context_driven_and_has_no_polling_deadline() {
        let document = Document::parse(r#"theme { mode "auto"; schedule { source "system"; }; }"#).unwrap();
        for appearance in [Appearance::Light, Appearance::Dark] {
            let resolved = resolve_with_context(
                &document,
                Path::new("/tmp"),
                Timestamp::now(),
                &AutoContext {
                    system: Some(appearance),
                },
                |_| unreachable!(),
            )
            .unwrap();
            assert_eq!(resolved.theme.appearance, appearance);
            assert!(resolved.next_transition.is_none());
            assert!(resolved.fallback_note.is_none());
        }
        let unavailable = resolve(&document, Path::new("/tmp"), Timestamp::now(), |_| unreachable!()).unwrap();
        assert!(unavailable.fallback_note.unwrap().contains("unavailable"));
    }

    #[test]
    fn schedule_handles_circular_intervals_and_equal_boundaries() {
        let zone = TimeZone::get("Africa/Lagos").unwrap();
        for (light, dark, now, expected) in [
            ("07:00", "19:00", "2026-09-30T05:59:00Z", Appearance::Dark),
            ("07:00", "19:00", "2026-09-30T06:00:00Z", Appearance::Light),
            ("19:00", "07:00", "2026-09-30T20:00:00Z", Appearance::Light),
            ("19:00", "07:00", "2026-09-30T12:00:00Z", Appearance::Dark),
        ] {
            let schedule = Schedule {
                source: AutoSource::Schedule,
                light_at: light.into(),
                dark_at: dark.into(),
                ..Schedule::default()
            };
            let now = now.parse().unwrap();
            let (actual, next) = schedule.boundaries(now, &zone).unwrap();
            assert_eq!(actual, expected);
            assert!(next > now);
        }
        assert!(
            Schedule {
                light_at: "07:00".into(),
                dark_at: "07:00".into(),
                ..Schedule::default()
            }
            .boundaries(Timestamp::now(), &zone)
            .is_err()
        );
    }

    #[test]
    fn dst_uses_first_repeated_boundary_and_shifts_skipped_time_forward() {
        let zone = TimeZone::get("America/New_York").unwrap();
        let schedule = Schedule {
            source: AutoSource::Schedule,
            light_at: "02:30".into(),
            dark_at: "19:00".into(),
            ..Schedule::default()
        };
        let (appearance, next) = schedule
            .boundaries("2026-03-08T06:00:00Z".parse().unwrap(), &zone)
            .unwrap();
        assert_eq!(appearance, Appearance::Dark);
        assert_eq!(next, "2026-03-08T07:30:00Z".parse::<Timestamp>().unwrap());
        let schedule = Schedule {
            source: AutoSource::Schedule,
            light_at: "01:30".into(),
            ..schedule
        };
        for now in ["2026-11-01T05:30:00Z", "2026-11-01T06:00:00Z", "2026-11-01T06:30:00Z"] {
            let (appearance, next) = schedule.boundaries(now.parse().unwrap(), &zone).unwrap();
            assert_eq!(appearance, Appearance::Light);
            assert_eq!(next, "2026-11-02T00:00:00Z".parse::<Timestamp>().unwrap());
        }
    }

    #[test]
    fn partial_files_merge_per_token_and_unknown_tokens_warn() {
        let doc = Document::parse(
            "theme { file \"themes/shared.kdl\"; light { file \"themes/light.kdl\"; }; mode \"light\"; }",
        )
        .unwrap();
        let result = resolve(&doc, Path::new("/config/ferese"), Timestamp::now(), |path| {
            match path.to_str().unwrap() {
                "/config/ferese/themes/shared.kdl" => {
                    Ok("colors { accent \"#12AABB\"; text-primary \"#182234\"; }; future-token 1".into())
                }
                "/config/ferese/themes/light.kdl" => Ok("theme { colors { accent \"#315090\"; }; }".into()),
                _ => panic!("wrong relative path"),
            }
        })
        .unwrap();
        assert_eq!(result.theme.requested_accent, "#315090");
        assert_eq!(result.theme.tokens.colors.text_primary, "#182234");
        assert_eq!(result.files.len(), 2);
        assert!(result.warnings.iter().any(|s| s.contains("future_token")));
        assert!(candidate("theme { colors { accent \"invalid\"; }; }").is_err());
    }

    #[test]
    fn accessibility_matrix_resolves_contrast_materials_and_accent() {
        for mode in ["light", "dark"] {
            for increase in [false, true] {
                for reduce in [false, true] {
                    for accent in ["#3D7BE6", "#F5F7FB", "#111821"] {
                        let source = format!(
                            "theme {{ mode \"{mode}\"; accent \"{accent}\"; accessibility {{ increase-contrast #{increase}; reduce-transparency #{reduce}; }}; material {{ style \"translucent\"; }}; }}"
                        );
                        let result = candidate(&source).unwrap();
                        let tokens = &result.theme.tokens;
                        let surface = rgba(&tokens.colors.surface_base).unwrap();
                        assert!(contrast(rgba(&tokens.colors.accent).unwrap(), surface) >= 3.);
                        assert!(
                            contrast(
                                rgba(&tokens.colors.on_accent).unwrap(),
                                rgba(&tokens.colors.accent).unwrap()
                            ) >= 4.5
                        );
                        if increase {
                            assert!(contrast(rgba(&tokens.colors.text_primary).unwrap(), surface) >= 7.);
                        }
                        if reduce {
                            assert_eq!(tokens.material.style, "solid");
                            assert_eq!(tokens.material.opacity, 1.);
                            assert_eq!(tokens.material.blur_radius, 0.);
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn transition_text_remains_readable_at_fixed_steps_on_composited_surfaces() {
        use crate::presets::PRESETS;
        let darks = PRESETS.iter().filter(|entry| entry.appearance == Appearance::Dark);
        let lights = PRESETS.iter().filter(|entry| entry.appearance == Appearance::Light);
        for (dark_entry, light_entry) in darks.zip(lights) {
            let dark = candidate(&format!(
                r#"theme {{ mode "dark"; dark {{ preset "{}"; }}; material {{ style "translucent"; }}; }}"#,
                dark_entry.id
            ))
            .unwrap()
            .theme;
            let light = candidate(&format!(
                r#"theme {{ mode "light"; light {{ preset "{}"; }}; material {{ style "translucent"; }}; }}"#,
                light_entry.id
            ))
            .unwrap()
            .theme;
            for (from, to) in [(&dark, &light), (&light, &dark)] {
                for backdrop in [[0., 0., 0., 1.], [1., 1., 1., 1.], [0.2, 0.4, 0.7, 1.]] {
                    for step in 1..20 {
                        let frame = from.transition(to, f64::from(step) / 20.);
                        let mut surface = rgba(&frame.tokens.colors.surface_base).unwrap();
                        surface[3] = frame.tokens.material.opacity * frame.tokens.material.tint_strength;
                        let surface = composite(surface, backdrop);
                        for text in [&frame.tokens.colors.text_primary, &frame.tokens.colors.text_muted] {
                            assert!(
                                contrast(rgba(text).unwrap(), surface) >= 4.5,
                                "step {step}: {text}, contrast {}",
                                contrast(rgba(text).unwrap(), surface)
                            );
                        }
                        for background in [
                            &frame.tokens.colors.surface_raised,
                            &frame.tokens.colors.application_background,
                        ] {
                            for text in [&frame.tokens.colors.text_primary, &frame.tokens.colors.text_muted] {
                                assert!(
                                    contrast(rgba(text).unwrap(), rgba(background).unwrap()) >= 4.5,
                                    "native app step {step}: {text} on {background}"
                                );
                            }
                        }
                        let mut bar = rgba(&frame.tokens.surface.bar.background).unwrap();
                        bar[3] = frame.tokens.material.opacity * frame.tokens.material.tint_strength;
                        let bar = composite(bar, backdrop);
                        for text in [
                            &frame.tokens.surface.bar.text_primary,
                            &frame.tokens.surface.bar.text_muted,
                        ] {
                            assert!(contrast(rgba(text).unwrap(), bar) >= 4.5, "bar step {step}: {text}");
                        }
                        assert!(
                            contrast(
                                rgba(&frame.tokens.colors.on_accent).unwrap(),
                                rgba(&frame.tokens.colors.accent).unwrap()
                            ) >= 4.5
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn high_contrast_covers_translucent_surfaces() {
        for mode in ["light", "dark"] {
            let resolved = candidate(&format!(r#"theme {{ family "tokyo-night"; mode "{mode}"; material {{ style "translucent"; opacity 0.52; }}; accessibility {{ increase-contrast #true; }}; }}"#)).unwrap().theme;
            let opacity = resolved.tokens.material.opacity * resolved.tokens.material.tint_strength;
            for (background, primary, muted) in [
                (
                    &resolved.tokens.colors.surface_base,
                    &resolved.tokens.colors.text_primary,
                    &resolved.tokens.colors.text_muted,
                ),
                (
                    &resolved.tokens.surface.bar.background,
                    &resolved.tokens.surface.bar.text_primary,
                    &resolved.tokens.surface.bar.text_muted,
                ),
            ] {
                for backdrop in material_bounds(rgba(background).unwrap(), opacity) {
                    for text in [primary, muted] {
                        assert!(contrast(rgba(text).unwrap(), backdrop) >= 7.0);
                    }
                }
            }
        }
    }

    #[test]
    fn transitions_preserve_endpoints_and_never_restyle_an_unchanged_theme() {
        let from = candidate(
            r#"theme { family "tokyo-night"; mode "dark"; material { style "translucent"; opacity 0.52; }; }"#,
        )
        .unwrap()
        .theme;
        let to =
            candidate(r#"theme { family "gruvbox"; mode "dark"; material { style "translucent"; opacity 0.52; }; }"#)
                .unwrap()
                .theme;
        assert_eq!(from.transition(&to, 0.0), from);
        assert_eq!(from.transition(&to, 1.0), to);
        for step in 0..=20 {
            let p = f64::from(step) / 20.0;
            assert_eq!(from.transition(&from, p), from);
            let frame = from.transition(&to, p);
            assert_eq!(frame.tokens.material.opacity, from.tokens.material.opacity);
            let low = from.tokens.material.tint_strength.min(to.tokens.material.tint_strength);
            let high = from.tokens.material.tint_strength.max(to.tokens.material.tint_strength);
            assert!((low..=high).contains(&frame.tokens.material.tint_strength));
        }
    }

    #[test]
    fn accessibility_changes_publish_the_resolved_theme_without_a_transient_palette() {
        let from = candidate(r#"theme { family "tokyo-night"; mode "dark"; }"#)
            .unwrap()
            .theme;
        let to =
            candidate(r#"theme { family "tokyo-night"; mode "dark"; accessibility { increase-contrast #true; }; }"#)
                .unwrap()
                .theme;
        for step in 0..=20 {
            assert_eq!(from.transition(&to, f64::from(step) / 20.0), to);
        }
    }

    #[test]
    fn reduced_motion_cuts_and_policy_does_not_follow_brightness() {
        let mut light = candidate("theme { mode \"light\"; colors { surface-base \"#111111\"; }; }")
            .unwrap()
            .theme;
        assert_eq!(light.appearance, Appearance::Light);
        light.reduced_motion = true;
        assert_eq!(ResolvedTheme::default().transition(&light, 0.), light);
    }
}
