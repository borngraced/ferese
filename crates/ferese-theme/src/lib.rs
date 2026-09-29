//! Shared colors, typography, controls and compositor materials for Ferese applications.
mod button;
pub mod calendar;
mod contrast;
pub mod controls;
pub mod icons;
pub mod material;
pub mod menus;
mod palette;
mod presets;
mod typography;

pub use button::accent_button;
pub use contrast::{accent_color, accent_pair, apply, composite, contrast, foreground, luminance};
pub use palette::{Palette, material_opacity, mix, parse_color, surface_shade};
pub use presets::{PRESETS, Preset};
pub use typography::{font, text};
