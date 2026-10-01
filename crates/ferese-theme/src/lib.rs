//! Shared colors, typography, controls and compositor materials for Ferese applications.
mod button;
pub mod calendar;
mod contrast;
pub mod controls;
pub mod gallery;
mod geometry;
pub mod icons;
pub mod material;
pub mod menus;
mod palette;
mod presets;
pub mod service;
mod typography;

pub use button::accent_button;
pub use contrast::{accent_color, accent_pair, apply, composite, contrast, foreground, luminance};
pub use geometry::inner_radius;
pub use palette::{Palette, mix, parse_color, surface_shade};
pub use presets::{PRESETS, Preset};
pub use typography::{font, text};
