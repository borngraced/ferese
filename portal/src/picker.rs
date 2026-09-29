use crate::capture::Source;
use cosmic::{
    ApplicationExt, Element,
    app::{Core, Settings, Task},
    iced::{Alignment, Length},
    widget::{button, column, container, row, scrollable},
};
use ferese_theme::icons::symbolic as glyph;
use serde::{Deserialize, Serialize};
use std::io::{Read, Write};

#[derive(Clone, Serialize, Deserialize)]
pub struct Prompt {
    pub app: String,
    pub sources: Vec<Source>,
    pub multiple: bool,
}

#[derive(Clone, Debug)]
enum Message {
    WindowOpened(cosmic::iced::window::Id),
    MaterialAttached(Result<ferese_theme::material::ModalMaterial, String>),
    Select(usize),
    Share,
    Cancel,
}

struct Picker {
    core: Core,
    material: Option<ferese_theme::material::ModalMaterial>,
    prompt: Prompt,
    selected: Vec<usize>,
    background: cosmic::iced::Color,
    foreground: cosmic::iced::Color,
    font: cosmic::font::Font,
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

    let (theme, font, background, foreground) = appearance();
    let height = 180. + (prompt.sources.len().min(4) as f32 * 56.);
    cosmic::app::run::<Picker>(
        Settings::default()
            .size(cosmic::iced::Size::new(400., height))
            .client_decorations(false)
            .transparent(true)
            .theme(theme)
            .default_font(font)
            .default_text_size(14.)
            .is_daemon(false),
        (prompt, background, foreground, font),
    )?;

    Ok(())
}

impl cosmic::Application for Picker {
    type Executor = cosmic::executor::Default;
    type Flags = (
        Prompt,
        cosmic::iced::Color,
        cosmic::iced::Color,
        cosmic::font::Font,
    );
    type Message = Message;
    const APP_ID: &'static str = "dev.ferese.ScreenShare";

    fn core(&self) -> &Core {
        &self.core
    }

    fn core_mut(&mut self) -> &mut Core {
        &mut self.core
    }

    fn init(
        mut core: Core,
        (prompt, background, foreground, font): Self::Flags,
    ) -> (Self, Task<Message>) {
        core.window.show_headerbar = false;
        core.window.border_padding = Some(0);
        core.window.content_container = false;
        // No preselected source: clicking Share must be a deliberate choice.
        let mut app = Self {
            core,
            material: None,
            prompt,
            selected: Vec::new(),
            background,
            foreground,
            font,
        };
        let task = app
            .core
            .main_window_id()
            .map(|id| app.set_window_title("Ferese — share your screen".into(), id))
            .unwrap_or_else(Task::none);

        (app, task)
    }

    fn subscription(&self) -> cosmic::iced::Subscription<Message> {
        cosmic::iced::event::listen_with(|event, _, id| match event {
            cosmic::iced::Event::Window(cosmic::iced::window::Event::Opened { .. }) => {
                Some(Message::WindowOpened(id))
            }
            _ => None,
        })
    }

    fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::WindowOpened(id) => {
                return cosmic::iced::window::run(
                    id,
                    ferese_theme::material::ModalMaterial::attach,
                )
                .map(|result| cosmic::Action::App(Message::MaterialAttached(result)));
            }
            Message::MaterialAttached(result) => {
                self.material = result.ok();
            }
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

    fn style(&self) -> Option<cosmic::iced::theme::Style> {
        Some(cosmic::iced::theme::Style {
            background_color: if self.material.is_some() {
                cosmic::iced::Color::TRANSPARENT
            } else {
                self.background
            },
            text_color: self.foreground,
            icon_color: self.foreground,
        })
    }

    fn view(&self) -> Element<'_, Message> {
        let app = if self.prompt.app.is_empty() {
            "this application"
        } else {
            &self.prompt.app
        };
        let mut content = column([]).width(Length::Fill).spacing(10).push(
            row![
                glyph(ferese_theme::icons::DISPLAY, 24),
                column![
                    self.text("Choose a display").size(18),
                    self.text(format!("Share with {app}")).size(12),
                ]
                .spacing(2)
                .width(Length::Fill),
            ]
            .spacing(12)
            .align_y(Alignment::Center),
        );
        let mut sources = column([]).spacing(6);
        for (index, source) in self.prompt.sources.iter().enumerate() {
            let selected = self.selected.contains(&index);
            let mut entry = row![
                glyph(ferese_theme::icons::DISPLAY, 24),
                column![
                    self.text(&source.name).size(14),
                    self.text(format!("{} × {}", source.width, source.height))
                        .size(12)
                ]
                .spacing(3)
                .width(Length::Fill),
            ]
            .spacing(12)
            .align_y(Alignment::Center);
            if selected {
                entry = entry.push(glyph(ferese_theme::icons::CHECK, 20));
            }
            sources = sources.push(
                button::custom(entry)
                    .class(if selected {
                        ferese_theme::accent_button()
                    } else {
                        cosmic::theme::Button::Standard
                    })
                    .padding(8)
                    .width(Length::Fill)
                    .on_press(Message::Select(index)),
            );
        }
        content = content.push(scrollable(sources).height(Length::Fixed(
            (self.prompt.sources.len().min(4) * 56) as f32,
        )));
        content = content.push(
            self.text("Everything on this display, including notifications, is visible.")
                .size(12)
                .width(Length::Fill),
        );
        let mut share = button::custom(
            row![self.text("Share"), glyph(ferese_theme::icons::ARROW, 16)]
                .spacing(8)
                .align_y(Alignment::Center),
        )
        .class(ferese_theme::accent_button())
        .padding([6, 16]);
        if !self.selected.is_empty() {
            share = share.on_press(Message::Share);
        }
        content = content.push(
            row![
                button::custom(self.text("Cancel"))
                    .class(cosmic::theme::Button::Text)
                    .on_press(Message::Cancel),
                share
            ]
            .spacing(8),
        );
        container(content).padding(14).width(Length::Fill).into()
    }
}

impl Picker {
    fn text<'a>(
        &self,
        content: impl Into<std::borrow::Cow<'a, str>> + 'a,
    ) -> cosmic::widget::Text<'a, cosmic::Theme, cosmic::Renderer> {
        ferese_theme::text(content, self.font)
    }
}

fn appearance() -> (
    cosmic::Theme,
    cosmic::font::Font,
    cosmic::iced::Color,
    cosmic::iced::Color,
) {
    let document = ferese_config::config_path()
        .and_then(|path| std::fs::read_to_string(path).ok())
        .and_then(|source| ferese_config::Document::parse(&source).ok());
    let palette = ferese_theme::Palette::from_document(document.as_ref()).flat();
    let family = document
        .as_ref()
        .and_then(|document| document.get("theme.typography.font_family"))
        .and_then(|value| value.as_str())
        .unwrap_or("Inter");
    (
        palette.native_theme(),
        ferese_theme::font(Some(family)),
        cosmic::iced::Color {
            a: ferese_theme::material_opacity(document.as_ref()),
            ..palette.sidebar
        },
        palette.text,
    )
}
