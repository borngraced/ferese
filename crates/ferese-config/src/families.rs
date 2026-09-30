use serde::{Deserialize, Serialize};

use crate::theme::{Appearance, Tokens};

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct Palette {
    pub background: String,
    pub bar: String,
    pub surface: String,
    pub text: String,
    pub muted: String,
    pub accent: String,
}

impl From<&Tokens> for Palette {
    fn from(tokens: &Tokens) -> Self {
        Self {
            background: tokens.colors.surface_base.clone(),
            bar: tokens.surface.bar.background.clone(),
            surface: tokens.colors.surface_raised.clone(),
            text: tokens.colors.text_primary.clone(),
            muted: tokens.colors.text_muted.clone(),
            accent: tokens.colors.accent.clone(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct Family {
    pub id: String,
    pub name: String,
    pub light: Option<Palette>,
    pub dark: Option<Palette>,
}

impl Family {
    pub fn palette(&self, appearance: Appearance) -> Option<&Palette> {
        match appearance {
            Appearance::Light => self.light.as_ref(),
            Appearance::Dark => self.dark.as_ref(),
        }
    }

    pub fn availability(&self) -> &'static str {
        match (self.light.is_some(), self.dark.is_some()) {
            (true, true) => "light and dark",
            (true, false) => "light only",
            _ => "dark only",
        }
    }
}

pub const BUILTINS: [(&str, &str, &str, &str); 6] = [
    ("ferese-blue", "Ferese Blue", "ferese-blue-light", "ferese-blue"),
    ("catppuccin", "Catppuccin", "catppuccin-latte", "catppuccin-mocha"),
    ("gruvbox", "Gruvbox", "gruvbox-light", "gruvbox"),
    ("rose-pine", "Rosé Pine", "rose-pine-dawn", "rose-pine-moon"),
    ("tokyo-night", "Tokyo Night", "tokyo-night-day", "tokyo-night"),
    ("everforest", "Everforest", "everforest-light", "everforest-dark"),
];

pub fn family_id(preset: &str) -> &str {
    BUILTINS
        .iter()
        .find(|row| row.2 == preset || row.3 == preset)
        .map_or(preset, |row| row.0)
}

pub fn variant(id: &str, appearance: Appearance) -> Option<&'static str> {
    if let Some(row) = BUILTINS.iter().find(|row| row.0 == id) {
        return Some(if appearance == Appearance::Light { row.2 } else { row.3 });
    }
    match (id, appearance) {
        ("dracula", Appearance::Dark) => Some("dracula"),
        ("monochrome", Appearance::Dark) => Some("monochrome"),
        ("monokai", Appearance::Dark) => Some("monokai"),
        ("ayu-light", Appearance::Light) => Some("ayu-light"),
        _ => None,
    }
}

pub fn builtins() -> Vec<Family> {
    BUILTINS
        .iter()
        .map(|(id, name, light, dark)| Family {
            id: (*id).into(),
            name: (*name).into(),
            light: Some(Palette::from(&crate::theme::preset(light, Appearance::Light).unwrap())),
            dark: Some(Palette::from(&crate::theme::preset(dark, Appearance::Dark).unwrap())),
        })
        .collect()
}
