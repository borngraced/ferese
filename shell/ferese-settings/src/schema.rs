#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Page {
    Appearance,
    Wallpaper,
    Bar,
    Windows,
    Motion,
    Keyboard,
    Shortcuts,
    Startup,
    Displays,
}
impl Page {
    pub const ALL: [Self; 9] = [
        Self::Appearance,
        Self::Wallpaper,
        Self::Bar,
        Self::Windows,
        Self::Motion,
        Self::Keyboard,
        Self::Shortcuts,
        Self::Startup,
        Self::Displays,
    ];
    pub fn title(self) -> &'static str {
        match self {
            Self::Appearance => "Appearance",
            Self::Wallpaper => "Wallpaper",
            Self::Bar => "Menu bar",
            Self::Windows => "Windows",
            Self::Motion => "Motion",
            Self::Keyboard => "Keyboard & mouse",
            Self::Shortcuts => "Shortcuts",
            Self::Startup => "Login items",
            Self::Displays => "Displays",
        }
    }
    pub fn subtitle(self) -> &'static str {
        match self {
            Self::Appearance => "A desktop that feels like yours.",
            Self::Wallpaper => "Set the scene for your workspace.",
            Self::Bar => "Everything you need, within reach.",
            Self::Windows => "Make room for the way you work.",
            Self::Motion => "Find your rhythm.",
            Self::Keyboard => "Fine-tune the everyday details.",
            Self::Shortcuts => "Your most-used actions, a keystroke away.",
            Self::Startup => "Ready when you sign in.",
            Self::Displays => "A place for every screen.",
        }
    }
    pub fn icon(self) -> &'static str {
        match self {
            Self::Appearance => {
                "M12 3a9 9 0 1 0 0 18h1a2 2 0 0 0 0-4h-1a1 1 0 0 1 0-2h3a6 6 0 0 0 0-12z M7 9h.01 M10 6h.01 M15 6h.01 M18 10h.01"
            }
            Self::Wallpaper => "M4 4h16v16H4z M4 16l5-5 4 4 3-3 4 4 M15 8h.01",
            Self::Bar => "M3 5h18v14H3z M3 9h18 M6 7h.01 M18 7h.01",
            Self::Windows => "M3 4h12v12H3z M8 9h13v12H8z",
            Self::Motion => "M3 8h9a3 3 0 1 0-3-3 M3 12h15a3 3 0 1 1-3 3 M3 16h5",
            Self::Keyboard => {
                "M3 6h18v12H3z M6 9h.01 M10 9h.01 M14 9h.01 M18 9h.01 M6 12h.01 M10 12h.01 M14 12h.01 M18 12h.01 M8 15h8"
            }
            Self::Shortcuts => {
                "M8 8h8v8H8z M8 8H5a3 3 0 1 1 3-3v3z M16 8V5a3 3 0 1 1 3 3h-3z M16 16h3a3 3 0 1 1-3 3v-3z M8 16v3a3 3 0 1 1-3-3h3z"
            }
            Self::Startup => "M12 3v9 M7 5a9 9 0 1 0 10 0",
            Self::Displays => "M3 4h18v13H3z M12 17v4 M8 21h8",
        }
    }
    pub fn matches(self, query: &str) -> bool {
        let query = query.to_lowercase();
        self.title().to_lowercase().contains(&query)
            || fields(self).iter().any(|f| {
                format!("{} {} {}", f.label, f.description, f.path)
                    .to_lowercase()
                    .contains(&query)
            })
    }
}

#[derive(Clone, Debug)]
pub enum Kind {
    Toggle(bool),
    Range {
        default: f64,
        min: f64,
        max: f64,
        step: f64,
        suffix: &'static str,
        integer: bool,
    },
    Choice {
        default: &'static str,
        choices: &'static [(&'static str, &'static str)],
    },
    Text {
        default: &'static str,
        argv: bool,
    },
}
#[derive(Clone, Debug)]
pub struct Field {
    pub path: String,
    pub label: String,
    pub description: String,
    pub kind: Kind,
}
impl Field {
    pub fn new(
        path: impl Into<String>,
        label: impl Into<String>,
        description: impl Into<String>,
        kind: Kind,
    ) -> Self {
        Self {
            path: path.into(),
            label: label.into(),
            description: description.into(),
            kind,
        }
    }
}
fn toggle(path: &str, label: &str, description: &str, default: bool) -> Field {
    Field::new(path, label, description, Kind::Toggle(default))
}
pub fn range(
    path: impl Into<String>,
    label: impl Into<String>,
    description: impl Into<String>,
    default: f64,
    min: f64,
    max: f64,
    step: f64,
    suffix: &'static str,
    integer: bool,
) -> Field {
    Field::new(
        path,
        label,
        description,
        Kind::Range {
            default,
            min,
            max,
            step,
            suffix,
            integer,
        },
    )
}
pub fn text(
    path: impl Into<String>,
    label: impl Into<String>,
    description: impl Into<String>,
    default: &'static str,
) -> Field {
    Field::new(
        path,
        label,
        description,
        Kind::Text {
            default,
            argv: false,
        },
    )
}
fn choice(
    path: &str,
    label: &str,
    description: &str,
    default: &'static str,
    choices: &'static [(&'static str, &'static str)],
) -> Field {
    Field::new(path, label, description, Kind::Choice { default, choices })
}

pub fn fields(page: Page) -> Vec<Field> {
    match page {
        Page::Appearance => vec![
            text(
                "theme.colors.accent",
                "Accent color",
                "Focus rings and selected controls. Use a hex color.",
                "#5B8CFF",
            ),
            text(
                "theme.colors.surface_base",
                "Surface color",
                "The base color for desktop surfaces.",
                "#111821",
            ),
            choice(
                "theme.material.style",
                "Surface style",
                "Choose the finish for the bar and menus.",
                "solid",
                &[("solid", "Solid"), ("translucent", "Translucent")],
            ),
            range(
                "theme.material.blur_radius",
                "Background blur",
                "Softness behind translucent surfaces.",
                12.0,
                0.0,
                32.0,
                1.0,
                " px",
                false,
            ),
            text(
                "theme.typography.font_family",
                "Interface font",
                "Use the name of an installed font family.",
                "Inter",
            ),
            range(
                "theme.geometry.focus_ring_width",
                "Focus border",
                "A subtle outline around the active window.",
                2.0,
                0.0,
                4.0,
                0.5,
                " px",
                false,
            ),
        ],
        Page::Wallpaper => vec![
            text(
                "theme.background.path",
                "Image location",
                "Choose an image above, or enter its full path.",
                "",
            ),
            choice(
                "theme.background.mode",
                "Image placement",
                "Fill crops to the screen. Fit keeps the whole image.",
                "fill",
                &[("fill", "Fill"), ("fit", "Fit")],
            ),
        ],
        Page::Bar => vec![
            range(
                "theme.geometry.top_bar_height",
                "Height",
                "Keeps icon and text sizes unchanged.",
                28.0,
                24.0,
                48.0,
                1.0,
                " px",
                false,
            ),
            range(
                "theme.geometry.top_bar_margin_top",
                "Top margin",
                "Set to zero for an edge-to-edge bar.",
                0.0,
                0.0,
                24.0,
                1.0,
                " px",
                true,
            ),
            range(
                "theme.geometry.top_bar_margin_horizontal",
                "Side margins",
                "Space between the bar and the display edges.",
                0.0,
                0.0,
                32.0,
                1.0,
                " px",
                true,
            ),
            range(
                "theme.geometry.top_bar_radius",
                "Corner radius",
                "The shape of the floating bar.",
                0.0,
                0.0,
                24.0,
                1.0,
                " px",
                false,
            ),
            range(
                "theme.surface.bar.opacity",
                "Opacity",
                "Lower values reveal more of the background.",
                0.78,
                0.2,
                1.0,
                0.01,
                "",
                false,
            ),
            range(
                "theme.geometry.top_bar_window_gap",
                "Window clearance",
                "Space below the menu bar.",
                0.0,
                0.0,
                24.0,
                1.0,
                " px",
                true,
            ),
            toggle(
                "status.battery_percentage",
                "Battery percentage",
                "Show the remaining charge beside the battery icon.",
                true,
            ),
        ],
        Page::Windows => vec![
            choice(
                "layout.mode",
                "Layout for new workspaces",
                "Existing workspaces keep their current layout.",
                "scrolling",
                &[("scrolling", "Scrolling"), ("tree", "Tiling")],
            ),
            choice(
                "scrolling.focus_strategy",
                "Scroll behavior",
                "How the viewport follows the focused column.",
                "minimal",
                &[
                    ("minimal", "Minimal"),
                    ("center_on_focus", "Centered"),
                    ("paged", "Paged"),
                ],
            ),
            range(
                "scrolling.default_column_width",
                "New window width",
                "Fraction of the available workspace.",
                0.5,
                0.25,
                1.0,
                0.05,
                "",
                false,
            ),
            range(
                "layout.inner_gap",
                "Between windows",
                "Space between neighboring windows.",
                10.0,
                0.0,
                32.0,
                1.0,
                " px",
                false,
            ),
            range(
                "layout.outer_gap",
                "Workspace edges",
                "Space around the outside of the layout.",
                4.0,
                0.0,
                32.0,
                1.0,
                " px",
                false,
            ),
            range(
                "theme.geometry.window_radius",
                "Corner radius",
                "Rounded corners for application windows.",
                14.0,
                0.0,
                28.0,
                1.0,
                " px",
                false,
            ),
            toggle(
                "appearance.inactive_dim.enabled",
                "Dim inactive windows",
                "Keep attention on the window you are using.",
                false,
            ),
            range(
                "appearance.inactive_dim.amount",
                "Dimming amount",
                "How much inactive windows recede.",
                0.15,
                0.0,
                0.5,
                0.01,
                "",
                false,
            ),
        ],
        Page::Motion => vec![
            toggle(
                "animations.enabled",
                "Animations",
                "Animate window changes and shell transitions.",
                true,
            ),
            range(
                "animations.speed",
                "Animation speed",
                "Lower values give transitions more time.",
                1.0,
                0.25,
                2.0,
                0.05,
                "×",
                false,
            ),
            toggle(
                "animations.reduced_motion",
                "Reduce motion",
                "Use immediate changes in place of animations.",
                false,
            ),
            range(
                "appearance.inactive_dim.duration_ms",
                "Focus transition",
                "Time taken to dim an inactive window.",
                150.0,
                0.0,
                600.0,
                10.0,
                " ms",
                false,
            ),
        ],
        Page::Keyboard => vec![
            text(
                "input.xkb_layout",
                "Keyboard layouts",
                "Layout codes, separated by commas. For example: us,ru.",
                "us",
            ),
            text(
                "input.xkb_variant",
                "Layout variant",
                "Leave empty to use the standard variant.",
                "",
            ),
            range(
                "input.repeat_rate",
                "Key repeat",
                "Characters repeated each second while a key is held.",
                25.0,
                1.0,
                70.0,
                1.0,
                " /s",
                true,
            ),
            range(
                "input.repeat_delay_ms",
                "Delay before repeat",
                "How long to hold a key before it starts repeating.",
                600.0,
                150.0,
                1000.0,
                25.0,
                " ms",
                true,
            ),
            toggle(
                "input.focus_follows_mouse",
                "Focus follows pointer",
                "Focus windows as the pointer moves over them.",
                false,
            ),
            toggle(
                "input.touchpad.tap",
                "Tap to click",
                "Tap the touchpad to click. Updates connected devices immediately.",
                true,
            ),
            toggle(
                "input.touchpad.natural_scroll",
                "Natural scrolling",
                "Move content in the direction of your fingers. Updates immediately.",
                true,
            ),
            toggle(
                "input.touchpad.disable_while_typing",
                "Ignore touchpad while typing",
                "Avoid accidental pointer movement.",
                true,
            ),
        ],
        Page::Shortcuts | Page::Startup | Page::Displays => vec![],
    }
}
