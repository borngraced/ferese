use cosmic::iced::platform_specific::runtime::wayland::popup::{SctkPopupSettings, SctkPositioner};
use cosmic::iced::{Alignment, Rectangle};
use cosmic::widget::{column, slider};
use status::{Action, Snapshot};

use super::*;

const DEVICE_LIST_HEIGHT: f32 = 180.0;

fn device_list_height(count: usize) -> Option<f32> {
    (count > 4).then_some(DEVICE_LIST_HEIGHT)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Menu {
    Network,
    Bluetooth,
    Audio,
    Battery,
    Calendar,
    Recording,
    Notifications,
    System,
}

impl Menu {
    fn width(self) -> f32 {
        match self {
            Self::System => 360.0,
            Self::Battery => 328.0,
            Self::Calendar => 268.0,
            Self::Notifications => 368.0,
            _ => 300.0,
        }
    }

    fn height_limit(self) -> f32 {
        if self == Self::Calendar { 270.0 } else { 720.0 }
    }

    pub(super) fn material_role(self) -> Option<ferese_surface_effects_v1::Role> {
        Some(ferese_surface_effects_v1::Role::Popover)
    }

    fn title(self) -> &'static str {
        match self {
            Self::System => "Control Center",
            Self::Network => "Wi-Fi",
            Self::Bluetooth => "Bluetooth",
            Self::Audio => "Sound",
            Self::Battery => "Battery",
            Self::Calendar => "Calendar",
            Self::Recording => "Screen recording",
            Self::Notifications => "Notifications",
        }
    }

    fn available(self, status: &Snapshot) -> bool {
        match self {
            Self::Network => status.network.is_some(),
            Self::Bluetooth => status.bluetooth.is_some(),
            Self::Audio => status.audio.is_some(),
            Self::Battery => status.battery.is_some(),
            Self::Calendar | Self::Recording => true,
            Self::Notifications => status.notifications.is_some(),
            Self::System => true,
        }
    }
}

pub struct OpenMenu {
    pub id: window::Id,
    pub kind: Menu,
    pub motion: super::motion::PopupMotion,
    pub effects: Option<EffectsBinding>,
    pub regions: super::motion::Regions,
}

impl OpenMenu {
    pub fn progress(&self) -> f32 {
        self.motion.progress()
    }

    pub fn animating(&self) -> bool {
        self.motion.animating()
    }
}

impl FereseShell {
    pub fn open_menu(&mut self, kind: Menu, anchor: Rectangle<i32>) -> Task<Message> {
        if self.menu.as_ref().is_some_and(|menu| menu.kind == kind) {
            if let Some(menu) = &mut self.menu
                && menu.motion.closing()
            {
                menu.motion.retarget(1.0, Instant::now());
                return Task::none();
            }

            return self.close_menu();
        }

        if kind == Menu::Calendar {
            self.calendar_offset = 0;
        }

        let destroy = self.destroy_menu();
        if !kind.available(&self.status) {
            return destroy;
        }

        if kind == Menu::Notifications && self.notifications.ready {
            self.notifications.toggle_history();
        }

        let notifications = self.sync_notification_surface();
        let id = window::Id::unique();
        self.menu = Some(OpenMenu {
            id,
            kind,
            motion: super::motion::PopupMotion::new(self.config.animations),
            effects: None,
            regions: Default::default(),
        });
        super::EFFECT_FRAME_PENDING.store(true, std::sync::atomic::Ordering::Relaxed);
        self.status_error = None;
        let parent = self.bar_surface_id;
        let anchor = if kind == Menu::System {
            self.outputs
                .iter()
                .find(|output| output.bar == parent)
                .and_then(|output| output.size)
                .map(|(width, _)| {
                    let theme = self.config.theme;
                    Rectangle {
                        x: (width - theme.bar_margin_horizontal * 2 - theme.panel_padding.round() as i32 - 1).max(0),
                        y: 0,
                        width: 1,
                        height: theme.bar_height.round() as i32,
                    }
                })
                .unwrap_or(anchor)
        } else {
            anchor
        };
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
                    offset: (anchor.width / 2, 8),
                    size_limits: Limits::NONE
                        .min_width(kind.width())
                        .max_width(kind.width())
                        .max_height(kind.height_limit()),
                    constraint_adjustment: 3, // slide X/Y, never flip above the bar
                    ..Default::default()
                },
            },
            Some(Box::new(Self::view_status_menu)),
        );
        Task::batch([destroy, notifications]).chain(cosmic::task::message(cosmic::Action::Surface(action)))
    }

    pub fn destroy_menu(&mut self) -> Task<Message> {
        super::EFFECT_FRAME_PENDING.store(false, std::sync::atomic::Ordering::Relaxed);
        let Some(menu) = self.menu.take() else {
            return Task::none();
        };
        if menu.kind == Menu::Notifications {
            self.notifications.history_open = false;
            self.notifications.hovered = None;
        }
        Task::batch([
            cosmic::task::message(cosmic::Action::Surface(cosmic::surface::action::destroy_popup(menu.id))),
            self.sync_notification_surface(),
        ])
    }

    pub fn close_menu(&mut self) -> Task<Message> {
        let Some(menu) = &mut self.menu else {
            return Task::none();
        };
        if menu.motion.closing() {
            return Task::none();
        }
        if menu
            .effects
            .as_ref()
            .is_some_and(|effects| effects.surface.version() >= 3)
        {
            menu.motion.retarget(0.0, Instant::now());
            if menu.animating() {
                return Task::none();
            }
        }
        self.destroy_menu()
    }

    pub fn animate_menu(&mut self) -> Task<Message> {
        if let Some(menu) = &self.menu {
            if let Some(effects) = &menu.effects {
                let _ = effects.set_opacity(menu.progress());
            }
            if menu.motion.closing() && !menu.animating() {
                return self.destroy_menu();
            }
        }
        Task::none()
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
        let mut controls = row::with_capacity(7).spacing(1).align_y(Alignment::Center);
        for kind in [
            Menu::System,
            Menu::Network,
            Menu::Bluetooth,
            Menu::Audio,
            Menu::Recording,
            Menu::Notifications,
            Menu::Battery,
        ] {
            if !kind.available(&self.status) {
                continue;
            }
            let (source, enabled) = if kind == Menu::Recording {
                let source: &'static [u8] = match self.recorder.state {
                    recording::State::Selecting => ferese_theme::icons::RECORD_CANCEL,
                    recording::State::Recording(_) => ferese_theme::icons::RECORD_STOP,
                    recording::State::Saving => ferese_theme::icons::RECORD_SAVING,
                    _ => ferese_theme::icons::RECORD,
                };
                (source, true)
            } else {
                status_icon(kind, &self.status)
            };
            let selected = self.menu.as_ref().is_some_and(|m| m.kind == kind)
                || (kind == Menu::Recording && matches!(self.recorder.state, recording::State::Recording(_)));
            let foreground = color(if selected {
                theme.accent
            } else if enabled {
                theme.text_primary
            } else {
                theme.text_muted
            });
            let mut content = row![accented_icon(
                source,
                metrics.icon_size,
                foreground,
                color(theme.accent)
            )]
            .spacing(4)
            .align_y(Alignment::Center);

            if kind == Menu::Recording
                && let Some(elapsed) = self.recorder.elapsed()
            {
                content = content.push(
                    text(elapsed)
                        .size(metrics.text_size)
                        .class(theme::Text::Color(foreground)),
                );
            }

            if kind == Menu::Battery
                && self.config.status.battery_percentage
                && let Some(battery) = &self.status.battery
            {
                content = content.push(
                    text(format!("{}%", battery.percent))
                        .size(metrics.text_size)
                        .class(theme::Text::Color(foreground)),
                );
            }
            if kind == Menu::Notifications
                && self
                    .status
                    .notifications
                    .as_ref()
                    .is_some_and(|n| n.count > 0 && !n.dnd)
            {
                content = content.push(container(text("")).width(4).height(4).class(theme::Container::custom(
                    move |_| container::Style {
                        background: Some(Background::Color(foreground)),
                        border: Border {
                            radius: motion::radius(2.0).into(),
                            ..Default::default()
                        },
                        ..Default::default()
                    },
                )));
            }
            // A fixed button height does not center its child in libcosmic.
            // center_y/center_x wrap the content in a centering layout, so use
            // those rather than align_y/align_x on a fixed-size container.
            let content = container(content)
                .align_x(alignment::Horizontal::Center)
                .height(metrics.control_height)
                .center_y(metrics.control_height);
            let recording_busy = kind == Menu::Recording && self.recorder.busy();
            let control = button::custom(content)
                .name(if recording_busy {
                    self.recorder.label().into()
                } else if kind == Menu::Recording {
                    "Record a display".into()
                } else {
                    status_label(kind, &self.status)
                })
                .padding([0.0, ((metrics.height - f32::from(metrics.icon_size)) * 0.5).max(4.0)])
                .height(metrics.control_height)
                .on_press_with_rectangle(move |offset, bounds| {
                    if kind == Menu::Recording {
                        return cosmic::Action::App(if recording_busy {
                            Message::StopRecording
                        } else {
                            Message::StartRecording
                        });
                    }
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
            controls = controls.push(motion::button(control, foreground, selected, 1.0));
        }
        container(controls)
            .padding([0, 3])
            .class(theme::Container::custom(move |_| bar_group_style(theme)))
            .into()
    }

    fn view_status_menu(&self) -> Element<'_, cosmic::Action<Message>> {
        let Some(menu) = &self.menu else {
            return text("").into();
        };
        if menu.kind == Menu::Notifications && self.notifications.ready {
            return cosmic::widget::autosize::autosize(
                self.view_notifications(),
                cosmic::iced::advanced::widget::Id::new("ferese-notification-center"),
            )
            .limits(
                Limits::NONE
                    .min_width(menu.kind.width())
                    .max_width(menu.kind.width())
                    .max_height(720.0),
            )
            .into();
        }
        let theme = self.config.theme;
        let p = if menu.effects.is_some() { 1.0 } else { menu.progress() };
        let kind = menu.kind;
        let primary = color_with_opacity(theme.text_primary, p);
        let muted = color_with_opacity(theme.text_muted, p);
        let badge = ferese_theme::menus::badge(
            accented_icon(
                status_icon(kind, &self.status).0,
                22,
                primary,
                color_with_opacity(theme.accent, p),
            )
            .width(Length::Fixed(22.))
            .into(),
            color(theme.accent),
            p,
            motion::radius(20.),
        );
        let mut heading = ferese_theme::menus::heading(badge, kind.title(), shell_font());
        if kind == Menu::System
            && self
                .config
                .status
                .settings_command
                .as_ref()
                .and_then(|command| command.first())
                .is_some_and(|program| status::available(program))
        {
            heading = heading.push(motion::button(
                button::custom(accented_icon(
                    ferese_theme::icons::SETTINGS,
                    20,
                    primary,
                    color_with_opacity(theme.accent, p),
                ))
                .padding(6)
                .name("Open Settings")
                .on_press(cosmic::Action::App(Message::Control(Action::Settings))),
                primary,
                false,
                p,
            ));
        }
        let mut rows = column::with_capacity(12).spacing(12).width(Length::Fill);
        if kind != Menu::Calendar {
            rows = rows.push(heading);
        }
        if kind == Menu::Calendar {
            rows = rows.push(calendar_grid(self.calendar_offset, theme, p));
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

                if self.status.network.is_some() || self.status.bluetooth.is_some() || self.status.battery.is_some() {
                    rows = rows.push(control_card(connections.into(), primary, p));
                }
            }

            if menu.kind == Menu::Network
                && let Some(n) = &self.status.network
            {
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
                    Some((n.enabled, Action::Wifi(!n.enabled))),
                ));
                if connected {
                    let strength = match n.signal {
                        75.. => "Excellent",
                        50..=74 => "Good",
                        25..=49 => "Fair",
                        _ => "Weak",
                    };
                    rows = rows.push(
                        column![
                            row![
                                text("Signal strength").size(12).width(Length::Fill),
                                text(strength).size(12)
                            ],
                            level_meter(n.signal, primary, p),
                        ]
                        .spacing(8),
                    );
                }
            }

            if menu.kind == Menu::Bluetooth
                && let Some(b) = &self.status.bluetooth
            {
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
                    Some((b.enabled, Action::Bluetooth(!b.enabled))),
                ));

                if b.enabled && !b.devices.is_empty() {
                    let mut devices = column::with_capacity(b.devices.len()).spacing(8);
                    for device in &b.devices {
                        devices = devices.push(
                            column![
                                text(device).size(14),
                                text("Connected").size(12).class(theme::Text::Color(muted)),
                            ]
                            .spacing(3),
                        );
                    }

                    let list: Element<'_, cosmic::Action<Message>> =
                        if let Some(height) = device_list_height(b.devices.len()) {
                            cosmic::iced::widget::scrollable(devices)
                                .height(height)
                                .width(Length::Fill)
                                .into()
                        } else {
                            devices.into()
                        };
                    rows = rows.push(list);
                }

                if (combined || menu.kind == Menu::Audio)
                    && let Some(a) = &self.status.audio
                {
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
                    let audio: Element<'_, cosmic::Action<Message>> = if combined {
                        control_card(audio.into(), primary, p)
                    } else {
                        audio.into()
                    };
                    rows = rows.push(audio);
                }
            }

            if combined && let Some(value) = self.status.brightness {
                let brightness = column![
                    text("Brightness").size(13),
                    slider_row(
                        ferese_theme::icons::BRIGHTNESS,
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

            if (combined || menu.kind == Menu::Notifications)
                && let Some(n) = &self.status.notifications
            {
                let toggle = toggle_row("Do Not Disturb", n.dnd, Action::Dnd(!n.dnd), theme, p);
                rows = rows.push(if combined {
                    control_card(toggle, primary, p)
                } else {
                    toggle
                });

                if menu.kind == Menu::Notifications {
                    rows = rows
                        .push(
                            text(if n.count == 0 {
                                "No notifications".to_owned()
                            } else if n.count == 1 {
                                "1 notification".to_owned()
                            } else {
                                format!("{} notifications", n.count)
                            })
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

            if menu.kind == Menu::Battery
                && let Some(b) = &self.status.battery
            {
                let battery_color = if b.percent < self.config.status.low_battery_threshold && b.status != "Charging" {
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
                            text(format!("{}%", b.percent)).size(24).width(Length::Fill),
                            accented_icon(
                                status_icon(Menu::Battery, &self.status).0,
                                24,
                                battery_color,
                                color_with_opacity(theme.accent, p),
                            ),
                        ]
                        .align_y(Alignment::Center),
                    )
                    .padding([2, 0])
                    .class(theme::Container::custom(move |_| container::Style {
                        text_color: Some(battery_color),
                        icon_color: Some(battery_color),
                        ..Default::default()
                    })),
                );
                rows = rows.push(level_meter(b.percent, battery_color, p));
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

            if menu.kind == Menu::Battery {
                rows = rows.push(ferese_theme::menus::section_label("Power mode", shell_font(), muted));
                if let Some(profiles) = &self.status.power_profiles {
                    let mut modes = row::with_capacity(3).spacing(6).width(Length::Fill);
                    for (index, (label, icon, profile)) in [
                        ("Power saver", ferese_theme::icons::POWER_SAVER, "power-saver"),
                        ("Balanced", ferese_theme::icons::POWER_BALANCED, "balanced"),
                        ("Performance", ferese_theme::icons::POWER_PERFORMANCE, "performance"),
                    ]
                    .into_iter()
                    .enumerate()
                    {
                        if profiles.available[index] {
                            modes = modes.push(power_tile(label, icon, profile, profiles.active == profile, theme, p));
                        }
                    }
                    rows = rows.push(modes);
                } else {
                    rows = rows.push(
                        text("Power Profiles service unavailable")
                            .size(12)
                            .class(theme::Text::Color(muted)),
                    );
                }

                let mut power = row::with_capacity(3).spacing(6).width(Length::Fill);
                for (enabled, label, icon, action) in [
                    (
                        self.status.reboot,
                        "Restart",
                        ferese_theme::icons::RESTART,
                        Action::Reboot,
                    ),
                    (
                        self.status.poweroff,
                        "Power off",
                        ferese_theme::icons::POWER_OFF,
                        Action::Poweroff,
                    ),
                    (
                        self.status.suspend,
                        "Suspend",
                        ferese_theme::icons::SUSPEND,
                        Action::Suspend,
                    ),
                ] {
                    if enabled {
                        power = power.push(power_tile_action(label, icon, action, theme, p));
                    }
                }

                if self.status.reboot || self.status.poweroff || self.status.suspend {
                    rows = rows.push(ferese_theme::menus::section_label("Power", shell_font(), muted));
                    rows = rows.push(power);
                }
            }

            if !menu.kind.available(&self.status) {
                rows = rows.push(text("Service unavailable").size(13).class(theme::Text::Color(muted)));
            }
        }

        if let Some(error) = &self.status_error {
            rows = rows.push(text(error).size(12).class(theme::Text::Color(Color {
                a: p,
                ..Color::from_rgb8(230, 172, 90)
            })));
        }

        let compositor_material = menu.effects.is_some();
        let panel = container(rows)
            .id("ferese-blur-card")
            .width(kind.width())
            .padding(16)
            .class(theme::Container::custom(move |_| container::Style {
                background: (!compositor_material)
                    .then_some(Background::Color(color_with_opacity(theme.surface_popover, p))),
                text_color: Some(primary),
                icon_color: Some(primary),
                border: Border {
                    color: color_with_opacity(theme.text_muted, 0.18 * p),
                    width: 1.0,
                    radius: theme.material_radius.into(),
                },
                ..Default::default()
            }));

        cosmic::widget::autosize::autosize(
            super::motion::animated(
                panel.into(),
                menu.progress(),
                menu.regions.clone(),
                theme.material_radius,
            ),
            cosmic::iced::advanced::widget::Id::new("ferese-status-menu"),
        )
        .limits(
            Limits::NONE
                .min_width(kind.width())
                .max_width(kind.width())
                .max_height(kind.height_limit()),
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
    toggle: Option<(bool, Action)>,
) -> Element<'a, cosmic::Action<Message>> {
    let enabled = toggle.as_ref().is_some_and(|(enabled, _)| *enabled);
    let badge_color = if enabled { accent } else { primary };
    let badge = container(accented_icon(source, 22, primary, accent))
        .width(32)
        .height(32)
        .center_x(32)
        .center_y(32)
        .class(theme::Container::custom(move |_| container::Style {
            background: Some(Background::Color(Color {
                a: if enabled { 0.22 * opacity } else { 0.06 * opacity },
                ..badge_color
            })),
            border: Border {
                radius: motion::radius(16.0).into(),
                ..Default::default()
            },
            ..Default::default()
        }));
    let badge: Element<'_, cosmic::Action<Message>> = if let Some((_, action)) = toggle {
        motion::button(
            button::custom(badge)
                .padding(0)
                .name(if enabled { "Turn off" } else { "Turn on" })
                .on_press(cosmic::Action::App(Message::Control(action))),
            badge_color,
            false,
            opacity,
        )
    } else {
        badge.into()
    };
    let summary = row![
        badge,
        column![
            text(title).size(14),
            text(subtitle).size(12).class(theme::Text::Color(muted)),
        ]
        .spacing(4)
        .width(Length::Fill)
    ]
    .spacing(8)
    .align_y(Alignment::Center);
    summary.into()
}

fn level_meter(value: u8, foreground: Color, opacity: f32) -> Element<'static, cosmic::Action<Message>> {
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
                    radius: motion::radius(2.5).into(),
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
    ferese_theme::menus::section(content, foreground, opacity, motion::radius(22.))
}

fn connection_caption(detail: &str) -> String {
    const MAX_CHARS: usize = 12;
    let normalized = detail.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut chars = normalized.chars();
    let mut caption: String = chars.by_ref().take(MAX_CHARS).collect();
    if chars.next().is_some() {
        caption.pop();
        caption.push('…');
    }
    caption
}

fn connection_control<'a>(
    label: &'static str,
    detail: &str,
    source: &'static [u8],
    enabled: bool,
    action: Option<Action>,
    palette: ShellTheme,
    opacity: f32,
) -> Element<'a, cosmic::Action<Message>> {
    let selected = enabled && action.is_some();
    let (fill, on_accent) = ferese_theme::accent_pair(
        ferese_theme::composite(color(palette.accent), color(palette.surface_base)),
        color(palette.text_primary),
    );
    let foreground = if selected {
        Color {
            a: opacity,
            ..on_accent
        }
    } else {
        color_with_opacity(palette.text_primary, opacity)
    };
    let icon_color = foreground;
    let muted = if selected {
        foreground
    } else {
        color_with_opacity(palette.text_muted, opacity)
    };
    let icon = container(accented_icon(source, 28, icon_color, icon_color))
        .width(Length::Fill)
        .height(28)
        .align_x(alignment::Horizontal::Center)
        .align_y(alignment::Vertical::Center);
    let content = column![icon].spacing(2).width(Length::Fill).align_x(Alignment::Center);
    let content = content.push(
        text(connection_caption(detail))
            .size(11)
            .width(Length::Fill)
            .height(16)
            .align_x(alignment::Horizontal::Center)
            .wrapping(cosmic::iced::widget::text::Wrapping::None)
            .ellipsize(cosmic::iced::widget::text::Ellipsize::End(
                cosmic::iced::advanced::text::EllipsizeHeightLimit::Lines(1),
            ))
            .class(theme::Text::Color(muted)),
    );
    let Some(action) = action else {
        return container(content).width(Length::FillPortion(1)).padding(4).into();
    };
    let button = button::custom(content)
        .width(Length::Fill)
        .padding(4)
        .name(format!("{label}: {}, {detail}", if enabled { "on" } else { "off" }))
        .on_press(cosmic::Action::App(Message::Control(action)));
    let tile: Element<'_, cosmic::Action<Message>> = if selected {
        button
            .class(ferese_theme::controls::filled_button(
                fill,
                foreground,
                motion::radius(14.),
                opacity,
            ))
            .into()
    } else {
        motion::button(button, foreground, false, opacity)
    };
    container(tile).width(Length::FillPortion(1)).into()
}

fn power_tile<'a>(
    label: &'static str,
    icon: &'static [u8],
    profile: &'static str,
    selected: bool,
    theme: ShellTheme,
    opacity: f32,
) -> Element<'a, cosmic::Action<Message>> {
    let foreground = color_with_opacity(if selected { theme.accent } else { theme.text_primary }, opacity);
    let tile = container(
        column![bar_icon(icon, 19, foreground), text(label).size(11)]
            .spacing(3)
            .align_x(Alignment::Center),
    )
    .width(Length::Fill)
    .center_x(Length::Fill);
    let btn = button::custom(tile)
        .width(Length::FillPortion(1))
        .padding([8, 2])
        .on_press(cosmic::Action::App(Message::Control(Action::PowerProfile(profile))));

    motion::button(btn, foreground, selected, opacity)
}

fn power_tile_action<'a>(
    label: &'static str,
    icon: &'static [u8],
    action: Action,
    theme: ShellTheme,
    opacity: f32,
) -> Element<'a, cosmic::Action<Message>> {
    let foreground = color_with_opacity(theme.text_primary, opacity);
    let tile = container(
        column![bar_icon(icon, 19, foreground), text(label).size(11)]
            .spacing(3)
            .align_x(Alignment::Center),
    )
    .width(Length::Fill)
    .center_x(Length::Fill);
    let btn = button::custom(tile)
        .width(Length::FillPortion(1))
        .padding([8, 2])
        .on_press(cosmic::Action::App(Message::ConfirmPower(action)));

    motion::button(btn, foreground, false, opacity)
}

fn calendar_grid<'a>(offset: i32, theme: ShellTheme, opacity: f32) -> Element<'a, cosmic::Action<Message>> {
    let primary = color_with_opacity(theme.text_primary, opacity);
    ferese_theme::calendar::grid(
        Zoned::now().date(),
        offset,
        theme.palette(),
        shell_font(),
        opacity,
        [
            menu_button("‹", Message::CalendarMonth(-1), primary, opacity),
            menu_button("Today", Message::CalendarToday, primary, opacity),
            menu_button("›", Message::CalendarMonth(1), primary, opacity),
        ],
    )
}

fn menu_button<'a>(
    label: &'a str,
    message: Message,
    foreground: Color,
    opacity: f32,
) -> Element<'a, cosmic::Action<Message>> {
    motion::button(
        ferese_theme::menus::button(label, cosmic::Action::App(message), shell_font()),
        foreground,
        false,
        opacity,
    )
}

fn shell_switch<'a>(
    label: &str,
    enabled: bool,
    action: Action,
    palette: ShellTheme,
    opacity: f32,
) -> Element<'a, cosmic::Action<Message>> {
    let toggle = ferese_theme::menus::switch(enabled, palette.palette(), opacity);
    motion::button(
        button::custom(toggle)
            .padding(4)
            .name(format!("{label}: {}", if enabled { "on" } else { "off" }))
            .on_press(cosmic::Action::App(Message::Control(action))),
        color(palette.text_primary),
        false,
        opacity,
    )
}

fn toggle_row<'a>(
    label: &'a str,
    on: bool,
    action: Action,
    palette: ShellTheme,
    opacity: f32,
) -> Element<'a, cosmic::Action<Message>> {
    ferese_theme::menus::row(label, shell_switch(label, on, action, palette, opacity), shell_font())
}

fn slider_row(
    source: &'static [u8],
    value: u8,
    brightness: bool,
    foreground: Color,
    icon_accent: Color,
    opacity: f32,
) -> Element<'static, cosmic::Action<Message>> {
    let control = slider(if brightness { 1..=100 } else { 0..=100 }, value, move |value| {
        cosmic::Action::App(Message::Control(if brightness {
            Action::Brightness(value)
        } else {
            Action::Volume(value)
        }))
    })
    .width(Length::Fill)
    .height(24)
    .class(ferese_theme::menus::slider(foreground, opacity, motion::radius(5.)));
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
            Some(n) if n.enabled => format!("Wi-Fi: {}", n.connection.as_deref().unwrap_or("not connected")),
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
        Menu::Calendar => "Calendar".into(),
        Menu::Recording => "Screen recording".into(),
        Menu::Battery => s.battery.as_ref().map_or_else(
            || "Battery: unavailable".into(),
            |b| format!("Battery: {}%, {}", b.percent, b.status),
        ),
        Menu::System => "Control Center".into(),
    }
}

fn audio_icon(volume: u8, muted: bool) -> &'static [u8] {
    if muted || volume == 0 {
        ferese_theme::icons::VOLUME_MUTE
    } else if volume <= 33 {
        ferese_theme::icons::VOLUME_LOW
    } else if volume <= 66 {
        ferese_theme::icons::VOLUME_MEDIUM
    } else {
        ferese_theme::icons::VOLUME_HIGH
    }
}

fn status_icon(kind: Menu, s: &Snapshot) -> (&'static [u8], bool) {
    match kind {
        Menu::Network => {
            let Some(n) = &s.network else {
                return (ferese_theme::icons::WIFI_OFF, false);
            };
            if !n.enabled || n.connection.is_none() {
                (ferese_theme::icons::WIFI_OFF, false)
            } else if n.signal <= 33 {
                (ferese_theme::icons::WIFI_LOW, true)
            } else if n.signal <= 66 {
                (ferese_theme::icons::WIFI_MEDIUM, true)
            } else {
                (ferese_theme::icons::WIFI_FULL, true)
            }
        }
        Menu::Bluetooth => match &s.bluetooth {
            Some(b) if b.enabled && !b.devices.is_empty() => (ferese_theme::icons::BLUETOOTH_CONNECTED, true),
            Some(b) if b.enabled => (ferese_theme::icons::BLUETOOTH_ON, true),
            _ => (ferese_theme::icons::BLUETOOTH_OFF, false),
        },
        Menu::Audio => s.audio.as_ref().map_or((audio_icon(0, true), false), |a| {
            (audio_icon(a.volume, a.muted), !a.muted && a.volume > 0)
        }),
        Menu::Calendar => (ferese_theme::icons::CALENDAR, true),
        Menu::Recording => (ferese_theme::icons::RECORD, true),
        Menu::Battery => {
            let Some(b) = &s.battery else {
                return (ferese_theme::icons::BATTERY_EMPTY, false);
            };
            let icon: &'static [u8] = if b.status == "Charging" {
                ferese_theme::icons::BATTERY_CHARGING
            } else if b.percent >= 80 {
                ferese_theme::icons::BATTERY_FULL
            } else if b.percent >= 50 {
                ferese_theme::icons::BATTERY_75
            } else if b.percent >= 20 {
                ferese_theme::icons::BATTERY_50
            } else if b.percent > 0 {
                ferese_theme::icons::BATTERY_25
            } else {
                ferese_theme::icons::BATTERY_EMPTY
            };
            (icon, true)
        }
        Menu::Notifications => {
            if s.notifications.as_ref().is_some_and(|n| n.dnd) {
                (ferese_theme::icons::NOTIFICATIONS_OFF, false)
            } else {
                (ferese_theme::icons::NOTIFICATIONS, true)
            }
        }
        Menu::System => (ferese_theme::icons::CONTROL_CENTER, true),
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn connection_captions_fit_one_short_line() {
        assert_eq!(super::connection_caption("88%"), "88%");
        assert_eq!(super::connection_caption("Flow84@Lofree"), "Flow84@Lofr…");
        assert_eq!(super::connection_caption("123456789012"), "123456789012");
        assert_eq!(super::connection_caption("My very long headphones"), "My very lon…");
        assert_eq!(super::connection_caption("  Device\n name  "), "Device name");
        let caption = super::connection_caption("耳機耳機耳機耳機耳機耳機耳機");
        assert_eq!(caption.chars().count(), 12);
        assert!(caption.ends_with('…'));
    }

    #[test]
    fn smaller_popups_have_content_specific_widths() {
        assert_eq!(Menu::System.width(), 360.0);
        assert_eq!(Menu::Network.width(), 300.0);
        assert_eq!(Menu::Bluetooth.width(), 300.0);
        assert_eq!(Menu::Battery.width(), 328.0);
        assert_eq!(Menu::Calendar.width(), 268.0);
        assert_eq!(Menu::Calendar.height_limit(), 270.0);
        assert_eq!(Menu::Audio.width(), 300.0);
        assert_eq!(Menu::Notifications.width(), 368.0);
    }

    #[test]
    fn short_device_lists_shrink_and_long_lists_are_bounded() {
        for count in 0..=4 {
            assert_eq!(device_list_height(count), None);
        }
        for count in [5, 20, 1000] {
            assert_eq!(device_list_height(count), Some(180.0));
        }
    }
    use super::*;
    #[test]
    fn completed_open_stops_requesting_animation_ticks() {
        let past = Instant::now() - Duration::from_secs(1);
        let mut motion = super::motion::PopupMotion::new(Default::default());
        motion.begin(past);
        let menu = OpenMenu {
            id: window::Id::unique(),
            kind: Menu::Network,
            motion,
            effects: None,
            regions: Default::default(),
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
        let mut s = Snapshot {
            audio: Some(status::Audio {
                volume: 80,
                muted: true,
                output: String::new(),
            }),
            ..Default::default()
        };
        assert_eq!(status_icon(Menu::Audio, &s), (ferese_theme::icons::VOLUME_MUTE, false));

        s.battery = Some(status::Battery {
            percent: 2,
            status: "Charging".into(),
        });
        assert_eq!(status_icon(Menu::Battery, &s).0, ferese_theme::icons::BATTERY_CHARGING);

        s.notifications = Some(status::Notifications { count: 3, dnd: true });
        assert_eq!(
            status_icon(Menu::Notifications, &s).0,
            ferese_theme::icons::NOTIFICATIONS_OFF
        );
        assert_eq!(status_label(Menu::Notifications, &s), "Notifications: do not disturb");

        s.bluetooth = Some(status::Bluetooth {
            enabled: false,
            devices: vec!["Headphones".into()],
        });
        assert_eq!(status_icon(Menu::Bluetooth, &s).0, ferese_theme::icons::BLUETOOTH_OFF);
    }

    #[test]
    fn battery_bands_and_live_label_follow_charge_state() {
        let mut s = Snapshot::default();
        for (percent, expected) in [
            (0, ferese_theme::icons::BATTERY_EMPTY),
            (19, ferese_theme::icons::BATTERY_25),
            (20, ferese_theme::icons::BATTERY_50),
            (50, ferese_theme::icons::BATTERY_75),
            (80, ferese_theme::icons::BATTERY_FULL),
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
