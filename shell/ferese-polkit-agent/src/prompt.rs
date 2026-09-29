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
        let setting = |key: &str, fallback: &str| {
            document
                .as_ref()
                .and_then(|doc| doc.get(key))
                .and_then(|value| value.as_str())
                .unwrap_or(fallback)
                .to_owned()
        };
        let color = |key, fallback| parse_color(&setting(key, fallback), fallback);
        let family: &'static str =
            Box::leak(setting("theme.typography.font_family", "Inter").into_boxed_str());
        let translucent = setting("theme.material.style", "solid") == "translucent";
        let shell_opacity = document
            .as_ref()
            .and_then(|doc| doc.get("theme.material.opacity"))
            .and_then(|value| value.as_f64())
            .unwrap_or(ferese_config::DEFAULT_MATERIAL_OPACITY)
            .clamp(0., 1.) as f32;

        let mut surface = color("theme.colors.surface_base", "#111821");
        surface.a = if translucent { shell_opacity } else { 1. };

        Self {
            surface,
            text: color("theme.colors.text_primary", "#F4F7FB"),
            muted: color("theme.colors.text_muted", "#8793A2"),
            accent: color("theme.colors.accent", "#3D7BE6"),
            radius: document
                .as_ref()
                .and_then(|doc| doc.get("theme.geometry.shell_radius"))
                .and_then(|value| value.as_f64())
                .unwrap_or(14.)
                .clamp(8., 36.) as f32,
            font: cosmic::font::Font {
                family: cosmic::iced::font::Family::Name(family),
                ..cosmic::font::default()
            },
        }
    }

    fn theme(&self) -> cosmic::Theme {
        use cosmic::cosmic_theme::{ThemeBuilder, palette::Srgba};

        let rgba = |color: Color| Srgba::new(color.r, color.g, color.b, color.a);
        let opaque_surface = Color {
            a: 1.,
            ..self.surface
        };
        let builder = if self.surface.r + self.surface.g + self.surface.b > 1.5 {
            ThemeBuilder::light()
        } else {
            ThemeBuilder::dark()
        };
        let corners = cosmic::cosmic_theme::CornerRadii {
            radius_xs: [self.radius.min(4.); 4],
            radius_s: [self.radius.min(8.); 4],
            radius_m: [self.radius; 4],
            radius_l: [self.radius; 4],
            radius_xl: [self.radius; 4],
            radius_0: Default::default(),
        };

        let mut native = builder
            .corner_radii(corners)
            .bg_color(rgba(opaque_surface))
            .primary_container_bg(rgba(opaque_surface))
            .text_tint(rgba(self.text).color)
            .accent(ferese_theme::accent_color(self.accent, opaque_surface))
            .build();
        ferese_theme::apply(&mut native, self.text);
        cosmic::Theme::custom(Arc::new(native))
    }
}

fn parse_color(input: &str, fallback: &str) -> Color {
    let parse = |source: &str| {
        let value = u32::from_str_radix(source.strip_prefix('#')?, 16).ok()?;
        (source.len() == 7)
            .then(|| Color::from_rgb8((value >> 16) as u8, (value >> 8) as u8, value as u8))
    };
    parse(input)
        .or_else(|| parse(fallback))
        .unwrap_or(Color::BLACK)
}

#[derive(Clone)]
enum Message {
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

fn input_style(palette: &Appearance) -> theme::TextInput {
    let text = palette.text;
    let muted = palette.muted;
    let accent = palette.accent;
    let radius = palette.radius.min(10.);
    let appearance = move |focused: bool| widget::text_input::Appearance {
        background: Color::from_rgba(text.r, text.g, text.b, 0.045).into(),
        border_radius: radius.into(),
        border_width: 1.,
        border_offset: None,
        border_color: if focused {
            accent.scale_alpha(0.82)
        } else {
            muted.scale_alpha(0.26)
        },
        icon_color: Some(muted),
        text_color: Some(text),
        placeholder_color: muted,
        selected_text_color: text,
        selected_fill: accent.scale_alpha(0.35),
        label_color: text,
    };
    theme::TextInput::Custom {
        active: Box::new(move |_| appearance(false)),
        hovered: Box::new(move |_| appearance(true)),
        focused: Box::new(move |_| appearance(true)),
        error: Box::new(move |_| appearance(false)),
        disabled: Box::new(move |_| appearance(false)),
    }
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
            background_color: self.appearance.surface,
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
        cosmic::iced::time::every(Duration::from_millis(30)).map(|_| Message::Tick)
    }

    fn update(&mut self, message: Message) -> Task<Message> {
        match message {
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
            .style(input_style(palette));
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
        let lock_svg = std::str::from_utf8(include_bytes!(
            "../../ferese-shell/assets/icons/status/lock.svg"
        ))
        .expect("embedded lock icon is valid UTF-8")
        .replace(
            "currentColor",
            &format!(
                "#{:02x}{:02x}{:02x}",
                (palette.accent.r * 255.) as u8,
                (palette.accent.g * 255.) as u8,
                (palette.accent.b * 255.) as u8,
            ),
        );
        let lock_icon = widget::icon::from_svg_bytes(lock_svg.into_bytes())
            .symbolic(false)
            .icon()
            .size(20);
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
