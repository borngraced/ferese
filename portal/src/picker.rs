use crate::capture::Source;
use cosmic::{
    ApplicationExt, Element,
    app::{Core, Settings, Task},
    iced::Length,
    widget::{button, column, container, row, text},
};
use serde::{Deserialize, Serialize};
use std::io::{Read, Write};

#[derive(Clone, Serialize, Deserialize)]
pub struct Prompt {
    pub app: String,
    pub sources: Vec<Source>,
    pub multiple: bool,
    pub indicator: bool,
}

#[derive(Clone, Debug)]
enum Message {
    Select(usize),
    Share,
    Cancel,
}

struct Picker {
    core: Core,
    prompt: Prompt,
    selected: Vec<usize>,
}

pub fn run() -> Result<(), Box<dyn std::error::Error>> {
    let mut input = String::new();
    std::io::stdin()
        .take(64 * 1024)
        .read_to_string(&mut input)?;
    let prompt: Prompt = serde_json::from_str(&input)?;

    if prompt.sources.is_empty() || prompt.sources.len() > 16 {
        return Err("No shareable displays".into());
    }

    let (theme, font) = appearance();
    cosmic::app::run::<Picker>(
        Settings::default()
            .size(cosmic::iced::Size::new(480., 440.))
            .theme(theme)
            .default_font(font)
            .default_text_size(14.)
            .is_daemon(false),
        prompt,
    )?;

    Ok(())
}

impl cosmic::Application for Picker {
    type Executor = cosmic::executor::Default;
    type Flags = Prompt;
    type Message = Message;
    const APP_ID: &'static str = "dev.ferese.ScreenShare";

    fn core(&self) -> &Core {
        &self.core
    }

    fn core_mut(&mut self) -> &mut Core {
        &mut self.core
    }

    fn init(core: Core, prompt: Prompt) -> (Self, Task<Message>) {
        // No preselected source: clicking Share must be a deliberate choice.
        let mut app = Self {
            core,
            prompt,
            selected: Vec::new(),
        };
        let title = if app.prompt.indicator {
            "Ferese — screen sharing"
        } else {
            "Ferese — share your screen"
        };
        let task = app
            .core
            .main_window_id()
            .map(|id| app.set_window_title(title.into(), id))
            .unwrap_or_else(Task::none);

        (app, task)
    }

    fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::Select(index) if index < self.prompt.sources.len() => {
                if self.selected.contains(&index) {
                    self.selected.retain(|i| *i != index);
                } else {
                    if !self.prompt.multiple {
                        self.selected.clear();
                    }
                    if self.selected.len() < 4 {
                        self.selected.push(index);
                    }
                }
            }
            Message::Share if !self.selected.is_empty() => {
                let names: Vec<_> = self
                    .selected
                    .iter()
                    .map(|i| &self.prompt.sources[*i].name)
                    .collect();
                println!("{}", serde_json::to_string(&names).unwrap());
                let _ = std::io::stdout().flush();
                return cosmic::iced::exit();
            }
            Message::Cancel => return cosmic::iced::exit(),
            _ => (),
        }
        Task::none()
    }

    fn view(&self) -> Element<'_, Message> {
        let app = if self.prompt.app.is_empty() {
            "An application"
        } else {
            &self.prompt.app
        };
        let mut content = column([])
            .spacing(16)
            .push(
                text(if self.prompt.indicator {
                    "Your screen is being shared"
                } else {
                    "Share your screen"
                })
                .size(24),
            )
            .push(text(if self.prompt.indicator {
                format!("{app} can see the displays below.")
            } else {
                format!("{app} wants to see your screen. Choose what to share.")
            }));

        for (index, source) in self.prompt.sources.iter().enumerate() {
            let label = format!(
                "{}{} — {} × {}",
                if self.selected.contains(&index) {
                    "✓  "
                } else {
                    ""
                },
                source.name,
                source.width,
                source.height
            );

            if self.prompt.indicator {
                content = content.push(text(label));
            } else {
                content = content.push(
                    button::text(label)
                        .width(Length::Fill)
                        .on_press(Message::Select(index)),
                );
            }
        }

        content = content.push(text(
            "Everything visible on a shared display, including notifications, may be seen.",
        ));

        if self.prompt.indicator {
            content = content.push(button::destructive("Stop sharing").on_press(Message::Cancel));
        } else {
            let mut share = button::suggested("Share");
            if !self.selected.is_empty() {
                share = share.on_press(Message::Share);
            }
            content = content.push(
                row([])
                    .spacing(12)
                    .push(button::text("Cancel").on_press(Message::Cancel))
                    .push(share),
            );
        }

        container(content).padding(24).width(Length::Fill).into()
    }
}

fn appearance() -> (cosmic::Theme, cosmic::font::Font) {
    let document = ferese_config::config_path()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|s| ferese_config::Document::parse(&s).ok());
    let string = |key: &str, fallback: &str| {
        document
            .as_ref()
            .and_then(|d| d.get(key))
            .and_then(|v| v.as_str())
            .unwrap_or(fallback)
            .to_owned()
    };
    let rgba = |key, fallback| {
        let s = string(key, fallback);
        let parse = |s: &str| {
            let hex = s.strip_prefix('#')?;
            let n = u32::from_str_radix(hex, 16).ok()?;
            Some(match hex.len() {
                6 => (n << 8) | 255,
                8 => n,
                _ => return None,
            })
        };
        let n = parse(&s).unwrap_or_else(|| parse(fallback).unwrap());
        cosmic::cosmic_theme::palette::Srgba::new(
            ((n >> 24) & 255) as f32 / 255.,
            ((n >> 16) & 255) as f32 / 255.,
            ((n >> 8) & 255) as f32 / 255.,
            (n & 255) as f32 / 255.,
        )
    };
    let background = rgba("theme.colors.surface_base", "#111821");
    let foreground = rgba("theme.colors.text_primary", "#F4F7FB");
    let accent = rgba("theme.colors.accent", "#3D7BE6");
    let radius = document
        .as_ref()
        .and_then(|d| d.get("theme.geometry.shell_radius"))
        .and_then(|v| v.as_f64())
        .unwrap_or(14.)
        .clamp(0., 40.) as f32;
    let corners = cosmic::cosmic_theme::CornerRadii {
        radius_xs: [radius.min(4.); 4],
        radius_s: [radius.min(8.); 4],
        radius_m: [radius; 4],
        radius_l: [radius; 4],
        radius_xl: [radius; 4],
        ..Default::default()
    };

    let builder = if background.red + background.green + background.blue > 1.8 {
        cosmic::cosmic_theme::ThemeBuilder::light()
    } else {
        cosmic::cosmic_theme::ThemeBuilder::dark()
    };
    let theme = cosmic::Theme::custom(std::sync::Arc::new(
        builder
            .bg_color(background)
            .primary_container_bg(background)
            .text_tint(foreground.color)
            .accent(accent.color)
            .corner_radii(corners)
            .build(),
    ));
    let family: &'static str =
        Box::leak(string("theme.typography.font_family", "Inter").into_boxed_str());

    (
        theme,
        cosmic::font::Font {
            family: cosmic::iced::font::Family::Name(family),
            ..cosmic::font::default()
        },
    )
}
