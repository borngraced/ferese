use crate::PromptEvent;
use cosmic::{
    Element,
    app::{Core, Settings, Task},
    iced::{Alignment, Background, Border, Color, Length, Size, Subscription, window},
    theme,
    widget::{self, button, column, container, row, text},
};
use std::{
    collections::VecDeque,
    io::{BufRead, BufReader, Write},
    sync::{Arc, Mutex},
    time::Duration,
};
use zeroize::{Zeroize, Zeroizing};

#[derive(Clone)]
struct Appearance {
    surface: Color,
    text: Color,
    muted: Color,
    accent: Color,
    radius: f32,
    font: cosmic::font::Font,
}

impl Appearance {
    fn load() -> Self {
        let document = ferese_config::config_path()
            .and_then(|path| std::fs::read_to_string(path).ok())
            .and_then(|source| ferese_config::Document::parse(&source).ok());
        let palette = ferese_theme::Palette::from_document(document.as_ref()).flat();
        let family = document
            .as_ref()
            .and_then(|document| document.get("theme.typography.font_family"))
            .and_then(|value| value.as_str())
            .unwrap_or("Inter");
        Self {
            surface: Color {
                a: ferese_theme::material_opacity(document.as_ref()),
                ..palette.sidebar
            },
            text: palette.text,
            muted: palette.muted,
            accent: palette.accent,
            radius: palette.radius,
            font: ferese_theme::font(Some(family)),
        }
    }

    fn palette(&self) -> ferese_theme::Palette {
        ferese_theme::Palette {
            background: self.surface,
            sidebar: self.surface,
            card: self.surface,
            text: self.text,
            muted: self.muted,
            accent: self.accent,
            radius: self.radius,
            error: Color::from_rgb8(235, 98, 98),
        }
    }

    fn theme(&self) -> cosmic::Theme {
        self.palette().native_theme()
    }
}

#[derive(Clone)]
enum Message {
    WindowOpened(cosmic::iced::window::Id),
    MaterialAttached(Result<ferese_theme::material::ModalMaterial, String>),
    Tick,
    Input(String),
    Submit,
    Cancel,
}

impl std::fmt::Debug for Message {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("AuthenticationPromptMessage")
    }
}

struct Prompt {
    core: Core,
    material: Option<ferese_theme::material::ModalMaterial>,
    appearance: Appearance,
    incoming: Arc<Mutex<VecDeque<PromptEvent>>>,
    description: String,
    user: String,
    question: String,
    echo: bool,
    answer: Zeroizing<String>,
    waiting: bool,
    error: Option<String>,
    info: Option<String>,
    height: f32,
}

fn content_height(description: &str, status: Option<&str>) -> f32 {
    let lines: usize = description
        .lines()
        .map(|line| line.chars().count().div_ceil(48).max(1))
        .sum();
    let status_height = status.map_or(0, |message| {
        14 + 18 * message.chars().count().div_ceil(48).clamp(1, 3)
    });
    (214 + 18 * lines.saturating_sub(1) + status_height).min(360) as f32
}

pub fn run() -> Result<(), Box<dyn std::error::Error>> {
    let stdin = std::io::stdin();
    let mut lines = BufReader::new(stdin).lines();
    let Some(Ok(first)) = lines.next() else {
        return Err("Missing authentication request".into());
    };
    let PromptEvent::Start { message, user } = serde_json::from_str(&first)? else {
        return Err("Invalid authentication request".into());
    };
    let incoming = Arc::new(Mutex::new(VecDeque::new()));
    let queue = incoming.clone();
    std::thread::spawn(move || {
        for line in lines {
            let Ok(line) = line else { break };
            if let Ok(event) = serde_json::from_str(&line) {
                queue.lock().unwrap().push_back(event);
            }
        }
        queue.lock().unwrap().push_back(PromptEvent::Cancel);
    });
    let appearance = Appearance::load();
    let height = content_height(&message, None);
    cosmic::app::run::<Prompt>(
        Settings::default()
            .size(Size::new(434., height))
            .client_decorations(false)
            .transparent(true)
            .theme(appearance.theme())
            .default_font(appearance.font)
            .default_text_size(14.)
            .antialiasing(true),
        (incoming, message, user, appearance),
    )?;
    Ok(())
}

fn reply(mut event: PromptEvent) {
    if let Ok(mut line) = serde_json::to_string(&event) {
        let mut stdout = std::io::stdout().lock();
        let _ = writeln!(stdout, "{line}");
        let _ = stdout.flush();
        line.zeroize();
    }
    if let PromptEvent::Response { value } = &mut event {
        value.zeroize();
    }
}

fn focus() -> Task<Message> {
    widget::text_input::focus(widget::Id::new("auth-response"))
}

impl Prompt {
    fn resize_to_content(&mut self) -> Task<Message> {
        let height = content_height(
            &self.description,
            self.error.as_deref().or(self.info.as_deref()),
        );
        if (height - self.height).abs() < 1.0 {
            return Task::none();
        }
        self.height = height;
        self.core
            .main_window_id()
            .map(|id| window::resize(id, Size::new(434., height)))
            .unwrap_or_else(Task::none)
    }
}

impl cosmic::Application for Prompt {
    type Executor = cosmic::executor::Default;
    type Flags = (
        Arc<Mutex<VecDeque<PromptEvent>>>,
        String,
        String,
        Appearance,
    );
    type Message = Message;
    const APP_ID: &'static str = "dev.ferese.Authentication";

    fn core(&self) -> &Core {
        &self.core
    }

    fn core_mut(&mut self) -> &mut Core {
        &mut self.core
    }

    fn style(&self) -> Option<cosmic::iced::theme::Style> {
        Some(cosmic::iced::theme::Style {
            background_color: if self.material.is_some() {
                cosmic::iced::Color::TRANSPARENT
            } else {
                self.appearance.surface
            },
            text_color: self.appearance.text,
            icon_color: self.appearance.text,
        })
    }

    fn init(
        mut core: Core,
        (incoming, description, user, appearance): Self::Flags,
    ) -> (Self, Task<Message>) {
        core.window.show_headerbar = false;
        core.window.border_padding = Some(0);
        core.window.content_container = false;
        let height = content_height(&description, None);
        let bounds = core
            .main_window_id()
            .map(|id| window::set_max_size(id, Some(Size::new(434., 360.))))
            .unwrap_or_else(Task::none);
        (
            Self {
                core,
                material: None,
                appearance,
                incoming,
                description,
                user,
                question: "Waiting for authentication…".into(),
                echo: false,
                answer: Zeroizing::new(String::new()),
                waiting: true,
                error: None,
                info: None,
                height,
            },
            Task::batch([focus(), bounds]),
        )
    }

    fn subscription(&self) -> Subscription<Message> {
        Subscription::batch([
            cosmic::iced::time::every(Duration::from_millis(30)).map(|_| Message::Tick),
            cosmic::iced::event::listen_with(|event, _, id| match event {
                cosmic::iced::Event::Window(cosmic::iced::window::Event::Opened { .. }) => {
                    Some(Message::WindowOpened(id))
                }
                _ => None,
            }),
        ])
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
            Message::Tick => {
                let events: Vec<_> = self.incoming.lock().unwrap().drain(..).collect();
                let mut refocus = false;
                for event in events {
                    match event {
                        PromptEvent::Request { prompt, echo } => {
                            self.answer.zeroize();
                            self.question = prompt;
                            self.echo = echo;
                            self.waiting = false;
                            refocus = true;
                        }
                        PromptEvent::Info { text } => self.info = Some(text),
                        PromptEvent::Error { text } => self.error = Some(text),
                        PromptEvent::Cancel => return cosmic::iced::exit(),
                        _ => {}
                    }
                }
                let resize = self.resize_to_content();
                if refocus {
                    return Task::batch([resize, focus()]);
                }
                return resize;
            }
            Message::Input(mut value) => {
                if !self.waiting && value.len() <= 4096 {
                    self.answer.zeroize();
                    *self.answer = std::mem::take(&mut value);
                    self.error = None;
                }
                value.zeroize();
            }
            Message::Submit if !self.waiting && !self.answer.is_empty() => {
                let value = std::mem::take(&mut *self.answer);
                reply(PromptEvent::Response { value });
                self.waiting = true;
            }
            Message::Cancel => {
                self.answer.zeroize();
                reply(PromptEvent::Cancel);
                return cosmic::iced::exit();
            }
            _ => {}
        }
        Task::none()
    }

    fn on_escape(&mut self) -> Task<Message> {
        self.update(Message::Cancel)
    }

    fn view(&self) -> Element<'_, Message> {
        let palette = &self.appearance;
        let mut input = widget::text_input(&self.question, self.answer.as_str())
            .id(widget::Id::new("auth-response"))
            .font(palette.font)
            .padding([7, 12])
            .style(ferese_theme::controls::authentication_input(
                palette.palette(),
            ));
        if !self.echo {
            input = input.password();
        }
        if !self.waiting {
            input = input.on_input(Message::Input).on_submit(|mut value| {
                value.zeroize();
                Message::Submit
            });
        }
        let action = button::custom(text("Authenticate").font(palette.font).size(13))
            .class(ferese_theme::accent_button())
            .height(Length::Fixed(36.))
            .padding([8, 16])
            .on_press_maybe((!self.waiting && !self.answer.is_empty()).then_some(Message::Submit));
        let lock_icon = ferese_theme::icons::tinted(ferese_theme::icons::LOCK, 20, palette.accent);
        let heading = row![
            container(lock_icon)
                .width(40)
                .height(40)
                .center_x(40)
                .center_y(40)
                .class(theme::Container::custom(move |_| {
                    widget::container::Style {
                        background: Some(Background::Color(palette.accent.scale_alpha(0.13))),
                        border: Border {
                            radius: 12.0.into(),
                            ..Default::default()
                        },
                        ..Default::default()
                    }
                })),
            column![
                text("Authentication required").font(palette.font).size(18),
                text(format!("Confirm as {}", self.user))
                    .font(palette.font)
                    .size(12)
                    .class(theme::Text::Color(palette.muted)),
            ]
            .spacing(2),
        ]
        .spacing(12)
        .align_y(Alignment::Center);
        let mut body = column![
            heading,
            text(&self.description).font(palette.font).size(13),
            input
        ]
        .spacing(14)
        .width(Length::Fill);
        if let Some(error) = &self.error {
            body = body.push(
                text(error)
                    .font(palette.font)
                    .size(12)
                    .class(theme::Text::Color(Color::from_rgb8(235, 98, 98))),
            );
        } else if let Some(info) = &self.info {
            body = body.push(
                text(info)
                    .font(palette.font)
                    .size(12)
                    .class(theme::Text::Color(palette.muted)),
            );
        }
        body = body.push(
            row![
                button::custom(text("Cancel").font(palette.font).size(13))
                    .class(theme::Button::Text)
                    .padding([8, 14])
                    .on_press(Message::Cancel),
                action
            ]
            .spacing(8)
            .align_y(Alignment::Center),
        );
        container(body)
            .padding(20)
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    }
}

#[cfg(test)]
mod tests {
    use super::content_height;

    #[test]
    fn dialog_grows_for_long_messages_and_caps_its_height() {
        assert_eq!(content_height("Short request", None), 214.);
        assert!(content_height(&"Long message ".repeat(20), None) > 214.);
        assert_eq!(content_height(&"Long message ".repeat(100), None), 360.);
        assert!(content_height("Short request", Some("Password not accepted")) > 214.);
    }
}
