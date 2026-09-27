use super::*;
use cosmic::widget::{column, scrollable};

const POPUP_WIDTH: u32 = 368;

fn popup_height(notice: &notifications::Notice, hovered: bool, count: usize) -> u32 {
    let body = if notice.body.is_empty() && count == 1 {
        0
    } else {
        22
    };
    let actions =
        if hovered && notice.live && notice.actions.iter().any(|(key, _)| key != "default") {
            39
        } else {
            0
        };
    76 + body + actions + if count > 1 { 4 } else { 0 }
}

fn blend(base: Color, foreground: Color, amount: f32) -> Color {
    Color {
        r: base.r + (foreground.r - base.r) * amount,
        g: base.g + (foreground.g - base.g) * amount,
        b: base.b + (foreground.b - base.b) * amount,
        a: base.a,
    }
}

fn contrast_text(background: Color, opacity: f32) -> Color {
    let level = background.r * 0.2126 + background.g * 0.7152 + background.b * 0.0722;
    Color {
        a: opacity,
        ..if level > 0.6 {
            Color::BLACK
        } else {
            Color::WHITE
        }
    }
}

fn age_label(age: Duration) -> String {
    match age.as_secs() {
        0..60 => "now".into(),
        60..3600 => format!("{}m ago", age.as_secs() / 60),
        3600..86400 => format!("{}h ago", age.as_secs() / 3600),
        seconds => format!("{}d ago", seconds / 86400),
    }
}

fn card_button<'a>(
    content: impl Into<Element<'a, cosmic::Action<Message>>>,
    message: Message,
    foreground: Color,
    hover: Color,
    round: bool,
) -> Element<'a, cosmic::Action<Message>> {
    let style = move |active: bool| button::Style {
        text_color: Some(foreground),
        icon_color: Some(foreground),
        background: (active || round).then_some(Background::Color(hover)),
        border_radius: (if round { 9.0 } else { 0.0 }).into(),
        ..Default::default()
    };
    button::custom(content)
        .width(if round { Length::Shrink } else { Length::Fill })
        .padding(0)
        .on_press(cosmic::Action::App(message))
        .class(theme::Button::Custom {
            active: Box::new(move |_, _| style(false)),
            hovered: Box::new(move |_, _| style(true)),
            pressed: Box::new(move |_, _| style(true)),
            disabled: Box::new(move |_| style(false)),
        })
        .into()
}

pub(super) struct NotificationSurface {
    pub id: window::Id,
    output: wl_output::WlOutput,
    height: u32,
}

impl FereseShell {
    pub(super) fn sync_notification_surface(&mut self) -> Task<Message> {
        if self.notifications.ready {
            self.status.notifications = Some(status::Notifications {
                count: self.notifications.unread(),
                dnd: self.notifications.dnd,
            });
        }
        let count = self.notifications.popup_groups().len();
        let wanted = self.notifications.ready && (self.notifications.history_open || count > 0);
        let selected = self
            .outputs
            .iter()
            .find(|entry| {
                self.snapshot.outputs.iter().any(|output| {
                    output.focused && Some(output.name.as_str()) == entry.name.as_deref()
                })
            })
            .or_else(|| self.outputs.first())
            .map(|entry| entry.output.clone());
        let mut tasks = Vec::new();
        if self
            .notification_surface
            .as_ref()
            .is_some_and(|surface| !wanted || selected.as_ref() != Some(&surface.output))
        {
            let surface = self.notification_surface.take().unwrap();
            tasks.push(destroy_layer_surface(surface.id));
        }
        if !wanted {
            return Task::batch(tasks);
        }
        let Some(output) = selected else {
            return Task::batch(tasks);
        };
        let desired_height = if self.notifications.history_open {
            520
        } else {
            self.notifications
                .popup_groups()
                .iter()
                .map(|(notice, _, count)| {
                    popup_height(
                        notice,
                        self.notifications.hovered == Some(notice.id),
                        *count,
                    ) + if *count > 1 { 12 } else { 0 }
                })
                .sum::<u32>()
                + count.saturating_sub(1) as u32 * 8
                + 8
        };
        let output_height = self
            .outputs
            .iter()
            .find(|entry| entry.output == output)
            .and_then(|entry| entry.size)
            .map_or(1080, |size| size.1);
        let top = self.config.theme.bar_height + self.config.theme.bar_margin_top as f32 + 12.0;
        let height = desired_height.min((output_height as f32 - top - 12.0).max(100.0) as u32);
        if let Some(surface) = &mut self.notification_surface {
            if surface.height != height {
                surface.height = height;
                tasks.push(set_size(surface.id, Some(POPUP_WIDTH), Some(height)));
            }
        } else {
            let id = window::Id::unique();
            self.notification_surface = Some(NotificationSurface {
                id,
                output: output.clone(),
                height,
            });
            let top = self.config.theme.bar_height + self.config.theme.bar_margin_top as f32 + 12.0;
            let action = cosmic::surface::action::app_layer_shell::<Self>(
                |_| Default::default(),
                move |_| SctkLayerSurfaceSettings {
                    id,
                    layer: Layer::Overlay,
                    keyboard_interactivity: KeyboardInteractivity::None,
                    anchor: Anchor::TOP | Anchor::RIGHT,
                    output: IcedOutput::Output(output.clone()),
                    margin: IcedMargin {
                        top: top as i32,
                        right: 12,
                        ..Default::default()
                    },
                    exclusive_zone: -1,
                    size: Some((Some(POPUP_WIDTH), Some(height))),
                    size_limits: Limits::NONE,
                    namespace: "ferese-shell-notifications".into(),
                    ..Default::default()
                },
                Some(Box::new(Self::view_notifications)),
            );
            tasks.push(cosmic::task::message(cosmic::Action::Surface(action)));
        }
        Task::batch(tasks)
    }

    fn notification_card(
        &self,
        notice: &notifications::Notice,
        opacity: f32,
        history: bool,
        count: usize,
    ) -> Element<'_, cosmic::Action<Message>> {
        let palette = self.config.theme;
        let tint = move |rgba| {
            let mut value = color(rgba);
            value.a *= opacity;
            value
        };
        let foreground = tint(palette.text_primary);
        let muted = tint(palette.text_muted);
        let accent = tint(palette.accent);
        let base = tint(palette.surface_base);
        let divider = tint(palette.border);
        let well = blend(base, foreground, 0.07);
        let id = notice.id;
        let hovered = self.notifications.hovered == Some(id);
        let mut bold = *SHELL_FONT.get().unwrap().read().unwrap();
        bold.weight = cosmic::iced::font::Weight::Bold;
        let app_icon = if notice.app.to_lowercase().starts_with("ferese") || notice.icon.is_empty()
        {
            accented_icon(
                include_bytes!("../assets/icons/ferese.svg"),
                18,
                accent,
                accent,
            )
        } else if notice.icon.starts_with('/') {
            icon::icon(icon::from_path(notice.icon.clone().into())).size(18)
        } else {
            icon::from_name(notice.icon.as_str()).size(18).icon()
        }
        .opacity(opacity);
        let icon_well =
            container(app_icon)
                .center_x(30)
                .center_y(30)
                .class(theme::Container::custom(move |_| container::Style {
                    background: Some(Background::Color(well)),
                    border: Border {
                        radius: 8.0.into(),
                        ..Default::default()
                    },
                    ..Default::default()
                }));
        let icon: Element<'_, cosmic::Action<Message>> = if count > 1 {
            let badge = container(
                text(count.to_string())
                    .size(10)
                    .font(bold)
                    .class(theme::Text::Color(contrast_text(accent, opacity))),
            )
            .center_x(16)
            .center_y(16)
            .class(theme::Container::custom(move |_| container::Style {
                background: Some(Background::Color(accent)),
                border: Border {
                    radius: 8.0.into(),
                    ..Default::default()
                },
                ..Default::default()
            }));
            cosmic::iced::widget::stack([
                container(icon_well)
                    .padding(cosmic::iced::Padding {
                        top: 4.0,
                        ..Default::default()
                    })
                    .into(),
                container(badge).align_right(34).into(),
            ])
            .into()
        } else {
            icon_well.into()
        };
        let app = if notice.app.is_empty() {
            "NOTIFICATION".to_owned()
        } else {
            notice.app.to_uppercase()
        };
        let app = if count > 1 {
            format!("{app} · {count} NOTIFICATIONS")
        } else {
            app
        };
        let header = row([])
            .spacing(10)
            .align_y(alignment::Vertical::Center)
            .push(icon)
            .push(
                container(
                    text(app)
                        .size(11)
                        .font(bold)
                        .class(theme::Text::Color(muted)),
                )
                .width(Length::Fill)
                .height(16)
                .clip(true),
            )
            .push(
                text(age_label(notice.received_at.elapsed()))
                    .size(11)
                    .class(theme::Text::Color(muted)),
            );
        let mut content = column([]).spacing(4).push(header).push(
            container(
                text(notice.title.clone())
                    .size(14)
                    .font(bold)
                    .class(theme::Text::Color(foreground)),
            )
            .height(18)
            .clip(true),
        );
        let body = if count > 1 {
            if notice.body.is_empty() {
                format!("{} more", count - 1)
            } else {
                format!("{} · {} more", notice.body, count - 1)
            }
        } else {
            notice.body.clone()
        };
        if !body.is_empty() {
            content = content.push(
                container(text(body).size(13).class(theme::Text::Color(muted)))
                    .height(if history {
                        Length::Shrink
                    } else {
                        Length::Fixed(18.0)
                    })
                    .clip(true),
            );
        }
        let mut face = column([]).push(container(content).padding([12, 14]).width(Length::Fill));
        if hovered && notice.live {
            let available: Vec<_> = notice
                .actions
                .iter()
                .filter(|(key, _)| key != "default")
                .take(2)
                .collect();
            if !available.is_empty() {
                let mut actions = row([]);
                for (index, (key, label)) in available.into_iter().enumerate() {
                    if index > 0 {
                        actions = actions.push(
                            container(cosmic::iced::widget::Space::new().width(1).height(38))
                                .class(theme::Container::custom(move |_| container::Style {
                                    background: Some(Background::Color(divider)),
                                    ..Default::default()
                                })),
                        );
                    }
                    actions = actions.push(card_button(
                        container(text(label.clone()).font(bold).size(13))
                            .center_x(Length::Fill)
                            .center_y(38),
                        Message::InvokeNotification(id, key.clone()),
                        accent,
                        well,
                        false,
                    ));
                }
                face = face
                    .push(
                        container(
                            cosmic::iced::widget::Space::new()
                                .width(Length::Fill)
                                .height(1),
                        )
                        .class(theme::Container::custom(move |_| {
                            container::Style {
                                background: Some(Background::Color(divider)),
                                ..Default::default()
                            }
                        })),
                    )
                    .push(actions);
            }
        }
        let card = container(face)
            .width(Length::Fill)
            .clip(true)
            .class(theme::Container::custom(move |_| container::Style {
                background: Some(Background::Color(base)),
                border: Border {
                    radius: palette.material_radius.into(),
                    ..Default::default()
                },
                ..Default::default()
            }));
        let card: Element<'_, cosmic::Action<Message>> = if hovered {
            let close = card_button(
                container(text("×").size(14)).center_x(18).center_y(18),
                if history {
                    Message::RemoveNotification(id)
                } else {
                    Message::DismissNotification(id)
                },
                muted,
                well,
                true,
            );
            cosmic::iced::widget::stack([
                card.into(),
                container(close)
                    .align_right(Length::Fill)
                    .padding([5, 6])
                    .into(),
            ])
            .into()
        } else {
            card.into()
        };
        let card: Element<'_, cosmic::Action<Message>> = if count > 1 {
            let height = popup_height(notice, hovered, count) as f32;
            let back = move |inset: f32, offset: f32| {
                container(
                    container(
                        cosmic::iced::widget::Space::new()
                            .width(Length::Fill)
                            .height(height),
                    )
                    .class(theme::Container::custom(move |_| {
                        container::Style {
                            background: Some(Background::Color(blend(base, foreground, 0.025))),
                            border: Border {
                                radius: palette.material_radius.into(),
                                ..Default::default()
                            },
                            ..Default::default()
                        }
                    })),
                )
                .padding(cosmic::iced::Padding {
                    top: offset,
                    left: inset,
                    right: inset,
                    bottom: 0.0,
                })
            };
            cosmic::iced::widget::stack([back(12.0, 12.0).into(), back(6.0, 6.0).into(), card])
                .into()
        } else {
            card
        };
        let mut area = cosmic::widget::mouse_area(card)
            .on_enter(cosmic::Action::App(Message::HoverNotification(id, true)))
            .on_exit(cosmic::Action::App(Message::HoverNotification(id, false)));
        if count > 1 {
            area = area.on_press(cosmic::Action::App(Message::ToggleNotificationHistory));
        } else if notice.live && notice.actions.iter().any(|(key, _)| key == "default") {
            area = area.on_press(cosmic::Action::App(Message::InvokeNotification(
                id,
                "default".into(),
            )));
        }
        area.into()
    }

    fn view_notifications(&self) -> Element<'_, cosmic::Action<Message>> {
        if self.notifications.history_open {
            let palette = self.config.theme;
            let primary = color(palette.text_primary);
            let muted = color(palette.text_muted);
            let accent = color(palette.accent);
            let base = color(palette.surface_base);
            let header = row([])
                .spacing(8)
                .align_y(alignment::Vertical::Center)
                .push(
                    text("Notifications")
                        .size(16)
                        .class(theme::Text::Color(primary))
                        .width(Length::Fill),
                )
                .push(motion::button(
                    button::custom(text("Clear all").size(11))
                        .on_press(cosmic::Action::App(Message::ClearNotifications))
                        .padding([4, 6]),
                    muted,
                    false,
                    1.0,
                ))
                .push(card_button(
                    container(text("×").size(14)).center_x(18).center_y(18),
                    Message::ToggleNotificationHistory,
                    muted,
                    blend(base, primary, 0.07),
                    true,
                ));
            let dnd = row([])
                .align_y(alignment::Vertical::Center)
                .push(
                    text("Do Not Disturb")
                        .size(12)
                        .class(theme::Text::Color(muted))
                        .width(Length::Fill),
                )
                .push(motion::button(
                    button::custom(
                        text(if self.notifications.dnd { "On" } else { "Off" }).size(12),
                    )
                    .on_press(cosmic::Action::App(Message::Control(status::Action::Dnd(
                        !self.notifications.dnd,
                    ))))
                    .padding([4, 8]),
                    if self.notifications.dnd {
                        accent
                    } else {
                        muted
                    },
                    self.notifications.dnd,
                    1.0,
                ));
            let controls = container(column([]).spacing(8).push(header).push(dnd))
                .padding(14)
                .width(Length::Fill)
                .class(theme::Container::custom(move |_| container::Style {
                    background: Some(Background::Color(base)),
                    border: Border {
                        radius: palette.material_radius.into(),
                        ..Default::default()
                    },
                    ..Default::default()
                }));
            let mut entries = column([]).spacing(8);
            if self.notifications.entries.is_empty() {
                entries = entries.push(
                    container(
                        text("You're all caught up")
                            .size(14)
                            .class(theme::Text::Color(muted)),
                    )
                    .padding(32)
                    .center_x(Length::Fill),
                );
            }
            for notice in self.notifications.entries.iter().rev() {
                entries = entries.push(self.notification_card(notice, 1.0, true, 1));
            }
            let content = column([])
                .spacing(8)
                .push(controls)
                .push(scrollable(entries).height(Length::Fill));
            container(content)
                .padding(4)
                .height(Length::Fill)
                .width(Length::Fill)
                .into()
        } else {
            let mut cards = column([]).spacing(8);
            for (notice, opacity, count) in self.notifications.popup_groups() {
                cards = cards.push(self.notification_card(notice, opacity, false, count));
            }
            container(scrollable(cards).height(Length::Fill))
                .padding(4)
                .into()
        }
    }
}
