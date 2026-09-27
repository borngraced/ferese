use super::*;
use cosmic::iced::widget::scrollable::{Direction, Scrollbar};
use cosmic::widget::{column, scrollable};

const POPUP_WIDTH: u32 = 368;

fn popup_height(notice: &notifications::Notice, count: usize) -> u32 {
    let body = if notice.body.is_empty() && count == 1 {
        0
    } else {
        22
    };
    let actions = if notice.live && notice.actions.iter().any(|(key, _)| key != "default") {
        39
    } else {
        0
    };
    76 + body + actions + if count > 1 { 4 } else { 0 }
}

fn history_height(center: &notifications::Center) -> u32 {
    if center.entries.is_empty() {
        return 280;
    }
    let groups = center.history_groups();
    let card_height = |notice: &notifications::Notice, count| {
        let wrapped_lines = if count == 1 {
            notice
                .body
                .lines()
                .map(|line| line.chars().count().div_ceil(40).max(1))
                .sum::<usize>()
        } else {
            1
        };
        popup_height(notice, count)
            + wrapped_lines.saturating_sub(1) as u32 * 17
            + if count > 1 { 12 } else { 0 }
    };
    let cards: u32 = groups
        .iter()
        .map(|group| {
            if group.len() > 1 && center.expanded_apps.contains(&group[0].app) {
                32 + group
                    .iter()
                    .map(|notice| card_height(notice, 1))
                    .sum::<u32>()
                    + group.len().saturating_sub(1) as u32 * 8
            } else {
                card_height(group[0], group.len())
            }
        })
        .sum();
    // Header, DND row and section tools share the same compact rhythm as the
    // cards. Long histories scroll; short histories do not reserve blank space.
    (170 + cards + groups.len().saturating_sub(1) as u32 * 8).min(620)
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
    accessible_name: String,
) -> Element<'a, cosmic::Action<Message>> {
    let style = move |active: bool| button::Style {
        text_color: Some(foreground),
        icon_color: Some(foreground),
        background: (active || round).then_some(Background::Color(hover)),
        border_radius: (if round { 9.0 } else { 0.0 }).into(),
        ..Default::default()
    };
    button::custom(content)
        .name(accessible_name)
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

fn icon_control<'a>(
    source: &'static [u8],
    label: &'static str,
    message: Message,
    foreground: Color,
    background: Color,
    extent: u16,
) -> Element<'a, cosmic::Action<Message>> {
    let control = card_button(
        container(bar_icon(source, 14, foreground))
            .center_x(extent)
            .center_y(extent),
        message,
        foreground,
        background,
        true,
        label.into(),
    );
    cosmic::widget::tooltip(
        control,
        text(label).size(11).class(theme::Text::Color(foreground)),
        cosmic::widget::tooltip::Position::Left,
    )
    .class(theme::Container::custom(move |_| container::Style {
        background: Some(Background::Color(background)),
        border: Border {
            radius: 6.0.into(),
            ..Default::default()
        },
        text_color: Some(foreground),
        ..Default::default()
    }))
    .into()
}

pub(super) struct NotificationSurface {
    pub id: window::Id,
    output: wl_output::WlOutput,
    height: u32,
    pub motion: Option<motion::PopupMotion>,
    pub effects: Option<EffectsBinding>,
    pub ready: bool,
    regions: motion::Regions,
}

impl NotificationSurface {
    pub fn animating(&self) -> bool {
        self.motion
            .as_ref()
            .is_some_and(|motion| motion.animating() || motion.closing())
    }

    pub fn progress(&self) -> f32 {
        self.motion
            .as_ref()
            .map_or(1.0, motion::PopupMotion::progress)
    }
}

impl FereseShell {
    pub(super) fn toggle_notification_history(&mut self) -> Task<Message> {
        if self.notifications.history_open {
            if let Some(motion) = self
                .notification_surface
                .as_mut()
                .and_then(|surface| surface.motion.as_mut())
            {
                if motion.closing() {
                    motion.retarget(1.0, Instant::now());
                    return self.sync_notification_surface();
                }
            }
            return self.close_notification_history();
        }
        self.notifications.toggle_history();
        self.sync_notification_surface()
    }

    pub(super) fn close_notification_history(&mut self) -> Task<Message> {
        if let Some(motion) = self
            .notification_surface
            .as_mut()
            .and_then(|surface| surface.motion.as_mut())
        {
            if !motion.closing() {
                motion.retarget(0.0, Instant::now());
            }
            return self.animate_notification_history();
        }
        self.notifications.history_open = false;
        self.sync_notification_surface()
    }

    pub(super) fn animate_notification_history(&mut self) -> Task<Message> {
        if let Some(surface) = &mut self.notification_surface {
            if surface
                .motion
                .as_ref()
                .is_some_and(|motion| motion.closing() && !motion.animating())
            {
                self.notifications.history_open = false;
                self.notifications.hovered = None;
                surface.motion = None;
            }
            if let Some(effects) = &surface.effects {
                let _ = effects.set_opacity(surface.progress());
            }
        }
        self.sync_notification_surface()
    }

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
            history_height(&self.notifications)
        } else {
            self.notifications
                .popup_groups()
                .iter()
                .map(|(notice, _, count)| {
                    popup_height(notice, *count) + if *count > 1 { 12 } else { 0 }
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
                motion: None,
                effects: None,
                ready: false,
                regions: Default::default(),
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
        if self.notifications.history_open {
            let surface = self.notification_surface.as_mut().unwrap();
            if surface.motion.is_none() {
                let mut motion = motion::PopupMotion::new(self.config.animations);
                if surface.ready {
                    motion.begin(Instant::now());
                }
                surface.motion = Some(motion);
            }
            if let Some(effects) = &surface.effects {
                let _ = effects.set_opacity(surface.progress());
            }
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
            )
            .push(icon_control(
                include_bytes!("../assets/icons/status/close.svg"),
                "Dismiss notification",
                if history && count > 1 {
                    Message::RemoveNotificationGroup(notice.app.clone())
                } else if history {
                    Message::RemoveNotification(id)
                } else {
                    Message::DismissNotification(id)
                },
                muted,
                well,
                22,
            ));
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
                    .height(if history && count == 1 {
                        Length::Shrink
                    } else {
                        Length::Fixed(18.0)
                    })
                    .clip(true),
            );
        }
        let mut face = column([]).push(container(content).padding([12, 14]).width(Length::Fill));
        if notice.live {
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
                        container(
                            text(label.clone())
                                .font(bold)
                                .size(13)
                                .class(theme::Text::Color(accent)),
                        )
                        .center_x(Length::Fill)
                        .center_y(38),
                        Message::InvokeNotification(id, key.clone()),
                        accent,
                        well,
                        false,
                        label.clone(),
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
        let card: Element<'_, cosmic::Action<Message>> = card.into();
        let card: Element<'_, cosmic::Action<Message>> = if count > 1 {
            let height = popup_height(notice, count) as f32;
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
            area = area.on_press(cosmic::Action::App(if history {
                Message::ToggleNotificationGroup(notice.app.clone())
            } else {
                Message::ToggleNotificationHistory
            }));
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
            let surface = self.notification_surface.as_ref();
            let progress = surface.map_or(1.0, NotificationSurface::progress);
            let opacity = if surface
                .and_then(|surface| surface.effects.as_ref())
                .is_some_and(|effects| effects.surface.version() >= 3)
            {
                1.0
            } else {
                progress
            };
            let color = move |rgba| color_with_opacity(rgba, opacity);
            let primary = color(palette.text_primary);
            let muted = color(palette.text_muted);
            let accent = color(palette.accent);
            let base = color(palette.surface_base);
            let panel = blend(base, primary, 0.035);
            let well = blend(base, primary, 0.07);
            let mut bold = *SHELL_FONT.get().unwrap().read().unwrap();
            bold.weight = cosmic::iced::font::Weight::Bold;
            let groups = self.notifications.history_groups();
            let count = self.notifications.entries.len();
            let subtitle = if count == 0 {
                "A quiet moment".to_owned()
            } else {
                format!(
                    "{count} {} · {} {}",
                    if count == 1 {
                        "notification"
                    } else {
                        "notifications"
                    },
                    groups.len(),
                    if groups.len() == 1 { "app" } else { "apps" }
                )
            };
            let heading = column([])
                .spacing(3)
                .push(
                    text("Notifications")
                        .size(16)
                        .font(bold)
                        .class(theme::Text::Color(primary)),
                )
                .push(text(subtitle).size(11).class(theme::Text::Color(muted)));
            let header = row([])
                .spacing(10)
                .align_y(alignment::Vertical::Center)
                .push(container(heading).width(Length::Fill))
                .push(icon_control(
                    include_bytes!("../assets/icons/status/close.svg"),
                    "Close notification center",
                    Message::ToggleNotificationHistory,
                    muted,
                    well,
                    28,
                ));
            let dnd_color = if self.notifications.dnd {
                accent
            } else {
                muted
            };
            let dnd_icon = if self.notifications.dnd {
                include_bytes!("../assets/icons/status/notifications-off.svg").as_slice()
            } else {
                include_bytes!("../assets/icons/status/notifications.svg").as_slice()
            };
            let dnd = row([])
                .spacing(10)
                .align_y(alignment::Vertical::Center)
                .push(accented_icon(dnd_icon, 20, dnd_color, dnd_color))
                .push(
                    container(
                        column([])
                            .spacing(2)
                            .push(
                                text("Do Not Disturb")
                                    .size(12)
                                    .font(bold)
                                    .class(theme::Text::Color(primary)),
                            )
                            .push(
                                text(if self.notifications.dnd {
                                    "Popups paused"
                                } else {
                                    "Popups enabled"
                                })
                                .size(11)
                                .class(theme::Text::Color(muted)),
                            ),
                    )
                    .width(Length::Fill),
                )
                .push(motion::button(
                    button::custom(
                        container(
                            text(if self.notifications.dnd { "On" } else { "Off" })
                                .size(12)
                                .font(bold)
                                .class(theme::Text::Color(dnd_color)),
                        )
                        .center_x(40)
                        .center_y(26),
                    )
                    .on_press(cosmic::Action::App(Message::Control(status::Action::Dnd(
                        !self.notifications.dnd,
                    ))))
                    .padding(0),
                    dnd_color,
                    self.notifications.dnd,
                    1.0,
                ));
            let controls =
                container(dnd)
                    .padding(10)
                    .width(Length::Fill)
                    .class(theme::Container::custom(move |_| container::Style {
                        background: Some(Background::Color(base)),
                        border: Border {
                            radius: palette.material_radius.into(),
                            ..Default::default()
                        },
                        ..Default::default()
                    }));
            let mut content = column([]).spacing(10).push(header).push(controls);
            if count == 0 {
                let empty = column([])
                    .spacing(10)
                    .align_x(alignment::Horizontal::Center)
                    .push(
                        container(accented_icon(
                            include_bytes!("../assets/icons/status/notifications.svg"),
                            26,
                            muted,
                            accent,
                        ))
                        .center_x(56)
                        .center_y(56)
                        .class(theme::Container::custom(move |_| container::Style {
                            background: Some(Background::Color(well)),
                            border: Border {
                                radius: 28.0.into(),
                                ..Default::default()
                            },
                            ..Default::default()
                        })),
                    )
                    .push(
                        text("You're all caught up")
                            .size(15)
                            .font(bold)
                            .class(theme::Text::Color(primary)),
                    )
                    .push(
                        text("New notifications will appear here.")
                            .size(11)
                            .class(theme::Text::Color(muted)),
                    );
                content = content.push(
                    container(empty)
                        .center_x(Length::Fill)
                        .center_y(Length::Fill),
                );
            } else {
                content = content.push(
                    row([])
                        .align_y(alignment::Vertical::Center)
                        .push(
                            text("Recent")
                                .size(11)
                                .font(bold)
                                .class(theme::Text::Color(muted))
                                .width(Length::Fill),
                        )
                        .push(icon_control(
                            include_bytes!("../assets/icons/status/trash.svg"),
                            "Clear all notifications",
                            Message::ClearNotifications,
                            muted,
                            well,
                            26,
                        )),
                );
                let mut entries = column([]).spacing(8);
                for group in groups {
                    let latest = group[0];
                    if group.len() > 1 && self.notifications.expanded_apps.contains(&latest.app) {
                        let app = if latest.app.is_empty() {
                            "Notifications"
                        } else {
                            &latest.app
                        };
                        let group_header = row([])
                            .spacing(6)
                            .align_y(alignment::Vertical::Center)
                            .push(
                                text(format!("{} · {}", app, group.len()))
                                    .size(11)
                                    .font(bold)
                                    .class(theme::Text::Color(muted))
                                    .width(Length::Fill),
                            )
                            .push(icon_control(
                                include_bytes!("../assets/icons/status/chevron-up.svg"),
                                "Collapse group",
                                Message::ToggleNotificationGroup(latest.app.clone()),
                                muted,
                                well,
                                24,
                            ))
                            .push(icon_control(
                                include_bytes!("../assets/icons/status/trash.svg"),
                                "Clear group",
                                Message::RemoveNotificationGroup(latest.app.clone()),
                                muted,
                                well,
                                24,
                            ));
                        let mut cards = column([]).spacing(8).push(group_header);
                        for notice in group {
                            cards = cards.push(self.notification_card(notice, opacity, true, 1));
                        }
                        entries = entries.push(cards);
                    } else {
                        entries = entries.push(self.notification_card(
                            latest,
                            opacity,
                            true,
                            group.len(),
                        ));
                    }
                }
                content = content.push(
                    scrollable(entries)
                        .direction(Direction::Vertical(Scrollbar::hidden()))
                        .height(Length::Fill),
                );
            }
            let panel = container(content)
                .padding(14)
                .height(Length::Fill)
                .width(Length::Fill)
                .class(theme::Container::custom(move |_| container::Style {
                    background: Some(Background::Color(panel)),
                    border: Border {
                        color: color(palette.border),
                        width: 1.0,
                        radius: palette.material_radius.into(),
                    },
                    ..Default::default()
                }))
                .into();
            motion::animated(
                panel,
                progress,
                surface
                    .map(|surface| surface.regions.clone())
                    .unwrap_or_default(),
                palette.material_radius,
            )
        } else {
            let mut cards = column([]).spacing(8);
            for (notice, opacity, count) in self.notifications.popup_groups() {
                cards = cards.push(self.notification_card(notice, opacity, false, count));
            }
            container(
                scrollable(cards)
                    .direction(Direction::Vertical(Scrollbar::hidden()))
                    .height(Length::Fill),
            )
            .padding(4)
            .into()
        }
    }
}
