use super::*;
use cosmic::iced::platform_specific::runtime::wayland::popup::{SctkPopupSettings, SctkPositioner};
use cosmic::iced::{Alignment, Rectangle};
use cosmic::widget::{column, slider};
use status::{Action, Snapshot};

const WIDTH: f32 = 320.0;
const OPEN_MS: f32 = 160.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Menu {
    Network,
    Bluetooth,
    Audio,
    Battery,
    Notifications,
    System,
}
impl Menu {
    fn title(self) -> &'static str {
        match self {
            Self::Network => "Wi-Fi",
            Self::Bluetooth => "Bluetooth",
            Self::Audio => "Volume",
            Self::Battery => "Battery",
            Self::Notifications => "Notifications",
            Self::System => "Control Center",
        }
    }
    fn available(self, status: &Snapshot) -> bool {
        match self {
            Self::Network => status.network.is_some(),
            Self::Bluetooth => status.bluetooth.is_some(),
            Self::Audio => status.audio.is_some(),
            Self::Battery => status.battery.is_some(),
            Self::Notifications => status.notifications.is_some(),
            Self::System => true,
        }
    }
}

pub struct OpenMenu {
    pub id: window::Id,
    pub kind: Menu,
    opened: Instant,
    pub confirm: Option<Action>,
    pub effects: Option<EffectsBinding>,
}
impl OpenMenu {
    pub fn progress(&self) -> f32 {
        let ease = |t: f32| {
            let t = t.clamp(0.0, 1.0);
            1.0 - (1.0 - t).powi(3)
        };
        ease(self.opened.elapsed().as_secs_f32() * 1000.0 / OPEN_MS)
    }
    pub fn animating(&self) -> bool {
        self.progress() < 1.0
    }
}

impl FereseShell {
    pub fn open_menu(&mut self, kind: Menu, anchor: Rectangle<i32>) -> Task<Message> {
        if self.menu.as_ref().is_some_and(|menu| menu.kind == kind) {
            // Close atomically. Per-widget alpha cannot fade SVG/text caches
            // as one surface and leaves bright fragments during the fade.
            return self.destroy_menu();
        }
        let destroy = self.destroy_menu();
        if !kind.available(&self.status) {
            return destroy;
        }
        let id = window::Id::unique();
        self.menu = Some(OpenMenu {
            id,
            kind,
            opened: Instant::now(),
            confirm: None,
            effects: None,
        });
        super::EFFECT_FRAME_PENDING.store(true, std::sync::atomic::Ordering::Relaxed);
        self.status_error = None;
        let parent = self.bar_surface_id;
        let action = cosmic::surface::action::app_popup::<Self>(
            |_| Default::default(),
            move |_| SctkPopupSettings {
                id,
                parent,
                parent_size: None,
                grab: true,
                close_with_children: false,
                input_zone: None,
                positioner: SctkPositioner {
                    anchor_rect: anchor,
                    // Numeric protocol values avoid depending on libcosmic's private SCTK reexport.
                    anchor: 2u32.try_into().unwrap(),  // bottom
                    gravity: 6u32.try_into().unwrap(), // bottom-left
                    offset: (anchor.width / 2, 4),
                    size_limits: Limits::NONE
                        .min_width(WIDTH)
                        .max_width(WIDTH)
                        .max_height(720.0),
                    constraint_adjustment: 3, // slide X/Y, never flip above the bar
                    ..Default::default()
                },
            },
            Some(Box::new(Self::view_status_menu)),
        );
        destroy.chain(cosmic::task::message(cosmic::Action::Surface(action)))
    }
    pub fn destroy_menu(&mut self) -> Task<Message> {
        super::EFFECT_FRAME_PENDING.store(false, std::sync::atomic::Ordering::Relaxed);
        self.menu.take().map_or_else(Task::none, |menu| {
            cosmic::task::message(cosmic::Action::Surface(
                cosmic::surface::action::destroy_popup(menu.id),
            ))
        })
    }
    pub fn optimistic_status(&mut self, action: &Action) {
        match action {
            Action::Volume(v) => {
                if let Some(a) = &mut self.status.audio {
                    a.volume = *v;
                }
            }
            Action::Mute(v) => {
                if let Some(a) = &mut self.status.audio {
                    a.muted = *v;
                }
            }
            Action::Brightness(v) => self.status.brightness = Some(*v),
            Action::Wifi(v) => {
                if let Some(n) = &mut self.status.network {
                    n.enabled = *v;
                }
            }
            Action::Bluetooth(v) => {
                if let Some(b) = &mut self.status.bluetooth {
                    b.enabled = *v;
                }
            }
            Action::Dnd(v) => {
                if let Some(n) = &mut self.status.notifications {
                    n.dnd = *v;
                }
            }
            _ => {}
        }
    }
    pub fn view_status_bar(&self) -> Element<'_, cosmic::Action<Message>> {
        let theme = self.config.theme.for_bar();
        let metrics = BarMetrics::from(theme);
        let mut controls = row::with_capacity(6).spacing(3).align_y(Alignment::Center);
        for kind in [
            Menu::System,
            Menu::Network,
            Menu::Bluetooth,
            Menu::Audio,
            Menu::Notifications,
            Menu::Battery,
        ] {
            if !kind.available(&self.status) {
                continue;
            }
            let (source, _) = status_icon(kind, &self.status);
            let foreground = color(theme.text_primary);
            let mut content = row![accented_icon(
                source,
                metrics.icon_size,
                foreground,
                color(theme.accent)
            )]
            .spacing(4)
            .align_y(Alignment::Center);
            if kind == Menu::Battery && self.config.status.battery_percentage {
                if let Some(battery) = &self.status.battery {
                    content = content.push(
                        text(format!("{}%", battery.percent))
                            .size(metrics.text_size)
                            .class(theme::Text::Color(foreground)),
                    );
                }
            }
            if kind == Menu::Notifications
                && self
                    .status
                    .notifications
                    .as_ref()
                    .is_some_and(|n| n.count > 0 && !n.dnd)
            {
                content = content.push(container(text("")).width(4).height(4).class(
                    theme::Container::custom(move |_| container::Style {
                        background: Some(Background::Color(foreground)),
                        border: Border {
                            radius: 2.0.into(),
                            ..Default::default()
                        },
                        ..Default::default()
                    }),
                ));
            }
            let selected = self.menu.as_ref().is_some_and(|m| m.kind == kind);
            // A fixed button height does not center its child in libcosmic.
            // Keep the visual content centered inside the entire click target.
            let content = container(content)
                .align_x(alignment::Horizontal::Center)
                .height(metrics.height)
                .align_y(alignment::Vertical::Center);
            let control = button::custom(content)
                .name(status_label(kind, &self.status))
                .padding([
                    0.0,
                    ((metrics.height - f32::from(metrics.icon_size)) * 0.5).max(7.0),
                ])
                .height(metrics.height)
                .class(button_style(foreground, selected, 1.0))
                .on_press_with_rectangle(move |offset, bounds| {
                    cosmic::Action::App(Message::OpenMenu(
                        kind,
                        Rectangle {
                            x: (bounds.x - offset.x).round() as i32,
                            y: (bounds.y - offset.y).round() as i32,
                            width: bounds.width.round() as i32,
                            height: bounds.height.round() as i32,
                        },
                    ))
                });
            controls = controls.push(control);
        }
        controls.into()
    }

    fn view_status_menu(&self) -> Element<'_, cosmic::Action<Message>> {
        let Some(menu) = &self.menu else {
            return text("").into();
        };
        let theme = self.config.theme;
        let p = menu.progress();
        let primary = color_with_opacity(theme.text_primary, p);
        let muted = color_with_opacity(theme.text_muted, p);
        // The bar has already established whether this connection supports
        // Ferese materials. Keep a glass popup's client buffer transparent
        // from its FIRST frame, not only after its own focus/effects event.
        // Otherwise the initial opaque fallback can be captured beneath glass
        // while popup registration and client repaint happen asynchronously.
        let compositor_material = self.effects.is_some();
        let mut rows = column::with_capacity(12).spacing(8).width(Length::Fill);
        let mut heading =
            row![text(menu.kind.title()).size(16).width(Length::Fill)].align_y(Alignment::Center);
        let radio = match menu.kind {
            Menu::Network => self
                .status
                .network
                .as_ref()
                .map(|n| (n.enabled, Action::Wifi(!n.enabled))),
            Menu::Bluetooth => self
                .status
                .bluetooth
                .as_ref()
                .map(|b| (b.enabled, Action::Bluetooth(!b.enabled))),
            _ => None,
        };
        if let Some((enabled, action)) = radio {
            heading = heading.push(menu_button(
                if enabled { "Turn off" } else { "Turn on" },
                Message::Control(action),
                primary,
                p,
            ));
        }
        rows = rows.push(heading);
        if let Some(action) = &menu.confirm {
            let title = match action {
                Action::Poweroff => "Power off this computer?",
                Action::Suspend => "Suspend this computer?",
                _ => "Restart this computer?",
            };
            rows = rows
                .push(text(title))
                .push(
                    text(if matches!(action, Action::Suspend) {
                        "The session will not be locked. Anyone can access it after waking."
                    } else {
                        "Save your work before continuing."
                    })
                    .size(13),
                )
                .push(
                    row![
                        menu_button("Cancel", Message::CancelPower, primary, p),
                        menu_button("Confirm", Message::Control(action.clone()), primary, p)
                    ]
                    .spacing(8),
                );
        } else {
            let combined = menu.kind == Menu::System;
            if combined {
                let mut connections = row::with_capacity(3).spacing(8).width(Length::Fill);
                if let Some(n) = &self.status.network {
                    connections = connections.push(connection_control(
                        "Wi-Fi",
                        if n.enabled {
                            n.connection.as_deref().unwrap_or("Not connected")
                        } else {
                            "Off"
                        },
                        status_icon(Menu::Network, &self.status).0,
                        n.enabled,
                        Some(Action::Wifi(!n.enabled)),
                        theme,
                        p,
                    ));
                }
                if let Some(b) = &self.status.bluetooth {
                    connections = connections.push(connection_control(
                        "Bluetooth",
                        if b.enabled {
                            b.devices.first().map(String::as_str).unwrap_or("On")
                        } else {
                            "Off"
                        },
                        status_icon(Menu::Bluetooth, &self.status).0,
                        b.enabled,
                        Some(Action::Bluetooth(!b.enabled)),
                        theme,
                        p,
                    ));
                }
                if let Some(b) = &self.status.battery {
                    connections = connections.push(connection_control(
                        "Battery",
                        &format!("{}%", b.percent),
                        status_icon(Menu::Battery, &self.status).0,
                        b.status == "Charging",
                        None,
                        theme,
                        p,
                    ));
                }
                if self.status.network.is_some()
                    || self.status.bluetooth.is_some()
                    || self.status.battery.is_some()
                {
                    rows = rows.push(control_card(connections.into(), primary, p));
                }
            }
            if menu.kind == Menu::Network {
                if let Some(n) = &self.status.network {
                    let connected = n.enabled && n.connection.is_some();
                    rows = rows.push(status_summary(
                        status_icon(Menu::Network, &self.status).0,
                        if !n.enabled {
                            "Wi-Fi is off"
                        } else {
                            n.connection.as_deref().unwrap_or("Not connected")
                        },
                        if connected {
                            "Connected"
                        } else if n.enabled {
                            "No active wireless connection"
                        } else {
                            "Wireless connections are paused"
                        },
                        primary,
                        muted,
                        color_with_opacity(theme.accent, p),
                        p,
                    ));
                    if connected {
                        let strength = match n.signal {
                            75.. => "Excellent",
                            50..=74 => "Good",
                            25..=49 => "Fair",
                            _ => "Weak",
                        };
                        rows = rows.push(control_card(
                            column![
                                row![
                                    text("Signal strength").size(12).width(Length::Fill),
                                    text(strength).size(12)
                                ],
                                level_meter(n.signal, primary, p),
                            ]
                            .spacing(8)
                            .into(),
                            primary,
                            p,
                        ));
                    }
                }
            }
            if menu.kind == Menu::Bluetooth {
                if let Some(b) = &self.status.bluetooth {
                    rows = rows.push(status_summary(
                        status_icon(Menu::Bluetooth, &self.status).0,
                        if b.enabled {
                            "Bluetooth is on"
                        } else {
                            "Bluetooth is off"
                        },
                        if !b.enabled {
                            "Device connections are paused"
                        } else if b.devices.is_empty() {
                            "No devices connected"
                        } else {
                            "Your connected devices"
                        },
                        primary,
                        muted,
                        color_with_opacity(theme.accent, p),
                        p,
                    ));
                    if b.enabled && !b.devices.is_empty() {
                        let mut devices = column::with_capacity(b.devices.len()).spacing(12);
                        for device in &b.devices {
                            devices = devices.push(
                                column![
                                    text(device).size(14),
                                    text("Connected").size(12).class(theme::Text::Color(muted)),
                                ]
                                .spacing(3),
                            );
                        }
                        rows = rows.push(control_card(devices.into(), primary, p));
                    }
                }
            }
            if combined || menu.kind == Menu::Audio {
                if let Some(a) = &self.status.audio {
                    let audio = column![
                        row![
                            text("Volume").size(13).width(Length::Fill),
                            menu_button(
                                if a.muted { "Unmute" } else { "Mute" },
                                Message::Control(Action::Mute(!a.muted)),
                                primary,
                                p
                            )
                        ]
                        .align_y(Alignment::Center),
                        text(&a.output).size(12).class(theme::Text::Color(muted)),
                        slider_row(
                            audio_icon(a.volume, a.muted),
                            a.volume,
                            false,
                            primary,
                            color_with_opacity(theme.accent, p),
                            p,
                        )
                    ]
                    .spacing(4);
                    rows = rows.push(control_card(audio.into(), primary, p));
                }
            }
            if combined {
                if let Some(value) = self.status.brightness {
                    let brightness = column![
                        text("Brightness").size(13),
                        slider_row(
                            include_bytes!("../assets/icons/status/brightness.svg"),
                            value,
                            true,
                            primary,
                            color_with_opacity(theme.accent, p),
                            p,
                        )
                    ]
                    .spacing(6);
                    rows = rows.push(control_card(brightness.into(), primary, p));
                }
            }
            if combined || menu.kind == Menu::Notifications {
                if let Some(n) = &self.status.notifications {
                    rows = rows.push(toggle_row(
                        "Do Not Disturb",
                        n.dnd,
                        Action::Dnd(!n.dnd),
                        primary,
                        p,
                    ));
                    if menu.kind == Menu::Notifications {
                        rows = rows
                            .push(
                                text(format!("{} notifications", n.count))
                                    .size(13)
                                    .class(theme::Text::Color(muted)),
                            )
                            .push(menu_button(
                                "Open notification center",
                                Message::Control(Action::Notifications),
                                primary,
                                p,
                            ));
                    }
                }
            }
            if menu.kind == Menu::Battery {
                if let Some(b) = &self.status.battery {
                    let battery_color = if b.percent < self.config.status.low_battery_threshold
                        && b.status != "Charging"
                    {
                        Color {
                            a: p,
                            ..Color::from_rgb8(230, 172, 90)
                        }
                    } else {
                        primary
                    };
                    rows = rows.push(
                        container(
                            row![
                                text(format!("{}%", b.percent)).size(40).width(Length::Fill),
                                accented_icon(
                                    status_icon(Menu::Battery, &self.status).0,
                                    40,
                                    battery_color,
                                    color_with_opacity(theme.accent, p),
                                ),
                            ]
                            .align_y(Alignment::Center),
                        )
                        .padding([10, 0])
                        .class(theme::Container::custom(move |_| container::Style {
                            text_color: Some(battery_color),
                            icon_color: Some(battery_color),
                            ..Default::default()
                        })),
                    );
                    rows = rows.push(level_meter(b.percent, battery_color, p));
                    rows = rows.push(text(&b.status).size(14));
                    rows = rows.push(
                        text(match b.status.as_str() {
                            "Charging" => "Connected to power",
                            "Discharging" => "Running on battery power",
                            "Full" => "Battery fully charged",
                            _ => "Battery status reported by the system",
                        })
                        .size(12)
                        .class(theme::Text::Color(muted)),
                    );
                }
            }
            if combined {
                let mut actions = row::with_capacity(4).spacing(4);
                if self
                    .config
                    .status
                    .settings_command
                    .as_ref()
                    .and_then(|a| a.first())
                    .is_some_and(|p| status::available(p))
                {
                    actions = actions.push(menu_button(
                        "Settings",
                        Message::Control(Action::Settings),
                        primary,
                        p,
                    ));
                }
                if self.status.poweroff {
                    actions = actions.push(menu_button(
                        "Power off…",
                        Message::ConfirmPower(Action::Poweroff),
                        primary,
                        p,
                    ));
                }
                if self.status.reboot {
                    actions = actions.push(menu_button(
                        "Restart…",
                        Message::ConfirmPower(Action::Reboot),
                        primary,
                        p,
                    ));
                }
                if self.status.suspend {
                    actions = actions.push(menu_button(
                        "Suspend…",
                        Message::ConfirmPower(Action::Suspend),
                        primary,
                        p,
                    ));
                }
                rows = rows.push(actions.wrap());
            }
            if !menu.kind.available(&self.status) {
                rows = rows.push(
                    text("Service unavailable")
                        .size(13)
                        .class(theme::Text::Color(muted)),
                );
            }
        }
        if let Some(error) = &self.status_error {
            rows = rows.push(text(error).size(12).class(theme::Text::Color(Color {
                a: p,
                ..Color::from_rgb8(230, 172, 90)
            })));
        }
        let panel = container(rows)
            .width(WIDTH)
            .padding(12)
            .class(theme::Container::custom(move |_| container::Style {
                background: if compositor_material {
                    None
                } else {
                    Some(Background::Color(color_with_opacity(
                        theme.surface_popover,
                        p,
                    )))
                },
                text_color: Some(primary),
                icon_color: Some(primary),
                border: Border {
                    radius: theme.material_radius.into(),
                    ..Default::default()
                },
                // Iced's shadow fills the silhouette behind the quad. With
                // transparent client content that fill remains visible over
                // the compositor's glass, creating a second dark rectangle.
                shadow: if compositor_material {
                    Shadow::default()
                } else {
                    Shadow {
                        color: color_with_opacity(theme.shadow, p * theme.shadow_opacity.min(0.18)),
                        offset: Vector::new(0.0, 2.0),
                        blur_radius: 6.0,
                    }
                },
                ..Default::default()
            }));
        // Keep the client panel and compositor material on the same surface
        // bounds. A second outer padding box exposes a glass ring around the
        // fallback fill and makes the first committed frame visibly jump.
        cosmic::widget::autosize::autosize(
            super::motion::animated(panel.into(), p),
            cosmic::iced::advanced::widget::Id::new("ferese-status-menu"),
        )
        .limits(
            Limits::NONE
                .min_width(WIDTH)
                .max_width(WIDTH)
                .max_height(720.0),
        )
        .into()
    }
}

fn status_summary<'a>(
    source: &'static [u8],
    title: &'a str,
    subtitle: &'a str,
    primary: Color,
    muted: Color,
    accent: Color,
    opacity: f32,
) -> Element<'a, cosmic::Action<Message>> {
    let badge = container(accented_icon(source, 24, primary, accent))
        .width(48)
        .height(48)
        .align_x(alignment::Horizontal::Center)
        .align_y(alignment::Vertical::Center)
        .class(theme::Container::custom(move |_| container::Style {
            background: Some(Background::Color(Color {
                a: 0.06 * opacity,
                ..primary
            })),
            border: Border {
                radius: 14.0.into(),
                ..Default::default()
            },
            ..Default::default()
        }));
    container(
        row![
            badge,
            column![
                text(title).size(18),
                text(subtitle).size(12).class(theme::Text::Color(muted)),
            ]
            .spacing(4)
            .width(Length::Fill)
        ]
        .spacing(12)
        .align_y(Alignment::Center),
    )
    .padding([12, 0])
    .into()
}

fn level_meter(
    value: u8,
    foreground: Color,
    opacity: f32,
) -> Element<'static, cosmic::Action<Message>> {
    let segment = move |amount: u16, filled: bool| {
        container(text(""))
            .width(Length::FillPortion(amount))
            .height(5)
            .class(theme::Container::custom(move |_| container::Style {
                background: Some(Background::Color(if filled {
                    foreground
                } else {
                    Color {
                        a: 0.1 * opacity,
                        ..foreground
                    }
                })),
                border: Border {
                    radius: 2.5.into(),
                    ..Default::default()
                },
                ..Default::default()
            }))
    };
    let value = u16::from(value.min(100));
    let mut meter = row::with_capacity(2).width(Length::Fill);
    if value > 0 {
        meter = meter.push(segment(value, true));
    }
    if value < 100 {
        meter = meter.push(segment(100 - value, false));
    }
    meter.into()
}

fn control_card<'a>(
    content: Element<'a, cosmic::Action<Message>>,
    foreground: Color,
    opacity: f32,
) -> Element<'a, cosmic::Action<Message>> {
    container(content)
        .padding(10)
        .width(Length::Fill)
        .class(theme::Container::custom(move |_| container::Style {
            background: Some(Background::Color(Color {
                a: 0.045 * opacity,
                ..foreground
            })),
            border: Border {
                radius: 11.0.into(),
                ..Default::default()
            },
            ..Default::default()
        }))
        .into()
}

fn connection_control<'a>(
    label: &'static str,
    detail: &str,
    source: &'static [u8],
    enabled: bool,
    action: Option<Action>,
    theme: ShellTheme,
    opacity: f32,
) -> Element<'a, cosmic::Action<Message>> {
    let foreground = color_with_opacity(theme.text_primary, opacity);
    let icon_accent = color_with_opacity(theme.accent, opacity);
    let accent = color_with_opacity(theme.accent, opacity * 0.14);
    let icon = container(accented_icon(source, 18, foreground, icon_accent))
        .width(32)
        .height(32)
        .align_x(alignment::Horizontal::Center)
        .align_y(alignment::Vertical::Center)
        .class(theme::Container::custom(move |_| container::Style {
            background: enabled.then_some(Background::Color(accent)),
            icon_color: Some(color_with_opacity(
                if enabled {
                    theme.text_primary
                } else {
                    theme.text_muted
                },
                opacity,
            )),
            border: Border {
                radius: 10.0.into(),
                ..Default::default()
            },
            ..Default::default()
        }));
    let content = column![
        icon,
        text(label).size(13),
        text(detail.to_owned())
            .size(12)
            .width(Length::Fill)
            .height(18)
            .wrapping(cosmic::iced::widget::text::Wrapping::None)
            .class(theme::Text::Color(color_with_opacity(
                theme.text_muted,
                opacity
            )))
    ]
    .spacing(6)
    .width(Length::Fill);
    // Battery is informational, not a toggle. Match its tile's layout without
    // giving it a misleading clickable/disabled-button appearance.
    let Some(action) = action else {
        return container(content)
            .width(Length::FillPortion(1))
            .padding(4)
            .into();
    };
    button::custom(content)
        .width(Length::FillPortion(1))
        .padding(4)
        .class(button_style(foreground, false, opacity))
        .on_press(cosmic::Action::App(Message::Control(action)))
        .into()
}

fn menu_button<'a>(
    label: &'a str,
    message: Message,
    foreground: Color,
    opacity: f32,
) -> Element<'a, cosmic::Action<Message>> {
    button::custom(text(label).size(13))
        .padding([6, 8])
        .class(button_style(foreground, false, opacity))
        .on_press(cosmic::Action::App(message))
        .into()
}
fn toggle_row<'a>(
    label: &'a str,
    on: bool,
    action: Action,
    foreground: Color,
    opacity: f32,
) -> Element<'a, cosmic::Action<Message>> {
    row![
        text(label).width(Length::Fill).size(14),
        menu_button(
            if on { "On" } else { "Off" },
            Message::Control(action),
            foreground,
            opacity
        )
    ]
    .align_y(Alignment::Center)
    .into()
}
pub(super) fn button_style(foreground: Color, selected: bool, opacity: f32) -> theme::Button {
    let style = move |strength: f32| button::Style {
        text_color: Some(foreground),
        icon_color: Some(foreground),
        border_radius: 6.0.into(),
        background: Some(Background::Color(Color {
            a: strength * opacity,
            ..foreground
        })),
        ..Default::default()
    };
    theme::Button::Custom {
        active: Box::new(move |_, _| style(if selected { 0.14 } else { 0.0 })),
        hovered: Box::new(move |_, _| style(if selected { 0.16 } else { 0.08 })),
        pressed: Box::new(move |_, _| style(0.20)),
        disabled: Box::new(move |_| style(0.0)),
    }
}

fn slider_row(
    source: &'static [u8],
    value: u8,
    brightness: bool,
    foreground: Color,
    icon_accent: Color,
    opacity: f32,
) -> Element<'static, cosmic::Action<Message>> {
    let style = std::rc::Rc::new(move |_: &cosmic::Theme| {
        use cosmic::iced::widget::slider::{Breakpoint, Handle, HandleShape, Rail, Style};
        Style {
            rail: Rail {
                backgrounds: (
                    foreground.into(),
                    Color {
                        a: 0.16 * opacity,
                        ..foreground
                    }
                    .into(),
                ),
                width: 4.0,
                border: Border {
                    radius: 2.0.into(),
                    ..Default::default()
                },
            },
            handle: Handle {
                shape: HandleShape::Circle { radius: 5.0 },
                background: foreground.into(),
                border_width: 0.0,
                border_color: Color::TRANSPARENT,
            },
            breakpoint: Breakpoint {
                color: Color::TRANSPARENT,
            },
        }
    });
    let control = slider(
        if brightness { 1..=100 } else { 0..=100 },
        value,
        move |value| {
            cosmic::Action::App(Message::Control(if brightness {
                Action::Brightness(value)
            } else {
                Action::Volume(value)
            }))
        },
    )
    .width(Length::Fill)
    .height(24)
    .class(theme::iced::Slider::Custom {
        active: style.clone(),
        hovered: style.clone(),
        dragging: style,
    });
    row![
        accented_icon(source, 18, foreground, icon_accent),
        control,
        text(format!("{value}%")).size(12).width(36)
    ]
    .spacing(8)
    .align_y(Alignment::Center)
    .into()
}

fn status_label(kind: Menu, s: &Snapshot) -> String {
    match kind {
        Menu::Network => match &s.network {
            Some(n) if n.enabled => format!(
                "Wi-Fi: {}",
                n.connection.as_deref().unwrap_or("not connected")
            ),
            _ => "Wi-Fi: off".into(),
        },
        Menu::Bluetooth => match &s.bluetooth {
            Some(b) if b.enabled => format!("Bluetooth: on, {} devices connected", b.devices.len()),
            _ => "Bluetooth: off".into(),
        },
        Menu::Audio => match &s.audio {
            Some(a) if !a.muted && a.volume > 0 => format!("Audio: {}%", a.volume),
            _ => "Audio: muted".into(),
        },
        Menu::Notifications => match &s.notifications {
            Some(n) if n.dnd => "Notifications: do not disturb".into(),
            Some(n) => format!("Notifications: {} unread", n.count),
            _ => "Notifications: unavailable".into(),
        },
        Menu::Battery => s.battery.as_ref().map_or_else(
            || "Battery: unavailable".into(),
            |b| format!("Battery: {}%, {}", b.percent, b.status),
        ),
        Menu::System => "Control Center".into(),
    }
}

fn audio_icon(volume: u8, muted: bool) -> &'static [u8] {
    if muted || volume == 0 {
        include_bytes!("../assets/icons/status/volume-mute.svg")
    } else if volume <= 33 {
        include_bytes!("../assets/icons/status/volume-low.svg")
    } else if volume <= 66 {
        include_bytes!("../assets/icons/status/volume-medium.svg")
    } else {
        include_bytes!("../assets/icons/status/volume-high.svg")
    }
}
fn status_icon(kind: Menu, s: &Snapshot) -> (&'static [u8], bool) {
    match kind {
        Menu::Network => {
            let Some(n) = &s.network else {
                return (include_bytes!("../assets/icons/status/wifi-off.svg"), false);
            };
            if !n.enabled || n.connection.is_none() {
                (include_bytes!("../assets/icons/status/wifi-off.svg"), false)
            } else if n.signal <= 33 {
                (include_bytes!("../assets/icons/status/wifi-low.svg"), true)
            } else if n.signal <= 66 {
                (
                    include_bytes!("../assets/icons/status/wifi-medium.svg"),
                    true,
                )
            } else {
                (include_bytes!("../assets/icons/status/wifi-full.svg"), true)
            }
        }
        Menu::Bluetooth => match &s.bluetooth {
            Some(b) if b.enabled && !b.devices.is_empty() => (
                include_bytes!("../assets/icons/status/bluetooth-connected.svg"),
                true,
            ),
            Some(b) if b.enabled => (
                include_bytes!("../assets/icons/status/bluetooth-on.svg"),
                true,
            ),
            _ => (
                include_bytes!("../assets/icons/status/bluetooth-off.svg"),
                false,
            ),
        },
        Menu::Audio => s.audio.as_ref().map_or((audio_icon(0, true), false), |a| {
            (audio_icon(a.volume, a.muted), !a.muted && a.volume > 0)
        }),
        Menu::Battery => {
            let Some(b) = &s.battery else {
                return (
                    include_bytes!("../assets/icons/status/battery-empty.svg"),
                    false,
                );
            };
            let icon: &'static [u8] = if b.status == "Charging" {
                include_bytes!("../assets/icons/status/battery-charging.svg")
            } else if b.percent >= 80 {
                include_bytes!("../assets/icons/status/battery-full.svg")
            } else if b.percent >= 50 {
                include_bytes!("../assets/icons/status/battery-75.svg")
            } else if b.percent >= 20 {
                include_bytes!("../assets/icons/status/battery-50.svg")
            } else if b.percent > 0 {
                include_bytes!("../assets/icons/status/battery-25.svg")
            } else {
                include_bytes!("../assets/icons/status/battery-empty.svg")
            };
            (icon, true)
        }
        Menu::Notifications => {
            if s.notifications.as_ref().is_some_and(|n| n.dnd) {
                (
                    include_bytes!("../assets/icons/status/notifications-off.svg"),
                    false,
                )
            } else {
                (
                    include_bytes!("../assets/icons/status/notifications.svg"),
                    true,
                )
            }
        }
        Menu::System => (
            include_bytes!("../assets/icons/status/control-center.svg"),
            true,
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn completed_open_stops_requesting_animation_ticks() {
        let past = Instant::now() - Duration::from_secs(1);
        let menu = OpenMenu {
            id: window::Id::unique(),
            kind: Menu::Network,
            opened: past,
            confirm: None,
            effects: None,
        };
        assert_eq!(menu.progress(), 1.0);
        assert!(!menu.animating());
    }

    #[test]
    fn services_are_hidden_until_available() {
        let s = Snapshot::default();
        assert!(!Menu::Network.available(&s));
        assert!(!Menu::Notifications.available(&s));
        assert!(Menu::System.available(&s));
    }
    #[test]
    fn icons_follow_real_states() {
        let mut s = Snapshot::default();
        s.audio = Some(status::Audio {
            volume: 80,
            muted: true,
            output: String::new(),
        });
        assert_eq!(
            status_icon(Menu::Audio, &s),
            (
                include_bytes!("../assets/icons/status/volume-mute.svg").as_slice(),
                false
            )
        );
        s.battery = Some(status::Battery {
            percent: 2,
            status: "Charging".into(),
        });
        assert_eq!(
            status_icon(Menu::Battery, &s).0,
            include_bytes!("../assets/icons/status/battery-charging.svg")
        );
        s.notifications = Some(status::Notifications {
            count: 3,
            dnd: true,
        });
        assert_eq!(
            status_icon(Menu::Notifications, &s).0,
            include_bytes!("../assets/icons/status/notifications-off.svg")
        );
        assert_eq!(
            status_label(Menu::Notifications, &s),
            "Notifications: do not disturb"
        );
        s.bluetooth = Some(status::Bluetooth {
            enabled: false,
            devices: vec!["Headphones".into()],
        });
        assert_eq!(
            status_icon(Menu::Bluetooth, &s).0,
            include_bytes!("../assets/icons/status/bluetooth-off.svg")
        );
    }

    #[test]
    fn battery_bands_and_live_label_follow_charge_state() {
        let mut s = Snapshot::default();
        for (percent, expected) in [
            (
                0,
                include_bytes!("../assets/icons/status/battery-empty.svg").as_slice(),
            ),
            (
                19,
                include_bytes!("../assets/icons/status/battery-25.svg").as_slice(),
            ),
            (
                20,
                include_bytes!("../assets/icons/status/battery-50.svg").as_slice(),
            ),
            (
                50,
                include_bytes!("../assets/icons/status/battery-75.svg").as_slice(),
            ),
            (
                80,
                include_bytes!("../assets/icons/status/battery-full.svg").as_slice(),
            ),
        ] {
            s.battery = Some(status::Battery {
                percent,
                status: "Discharging".into(),
            });
            assert_eq!(status_icon(Menu::Battery, &s).0, expected);
            assert_eq!(
                status_label(Menu::Battery, &s),
                format!("Battery: {percent}%, Discharging")
            );
        }
    }
}
