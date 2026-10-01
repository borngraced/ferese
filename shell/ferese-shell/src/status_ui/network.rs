use cosmic::iced::Length;
use cosmic::widget::{column, row};

use super::bar::status_icon;
use super::controls::{level_meter, shell_switch, status_summary};
use super::{Menu, MenuRows, MenuStyle};
use crate::status::Action;
use crate::{FereseShell, text};

pub(super) fn view<'a>(shell: &'a FereseShell, mut rows: MenuRows<'a>, style: MenuStyle) -> MenuRows<'a> {
    let MenuStyle {
        primary, opacity: p, ..
    } = style;
    if let Some(n) = &shell.status.network {
        let connected = n.enabled && n.connection.is_some();
        rows = rows.push(status_summary(
            status_icon(Menu::Network, &shell.status).0,
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
            style,
            n.enabled,
            None,
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
    rows
}

pub(super) fn heading<'a>(
    shell: &'a FereseShell,
    mut heading: super::MenuHeading<'a>,
    style: MenuStyle,
) -> super::MenuHeading<'a> {
    let MenuStyle { theme, opacity: p, .. } = style;
    if let Some(network) = &shell.status.network {
        heading = heading.push(shell_switch(
            "Wi-Fi",
            network.enabled,
            Action::Wifi(!network.enabled),
            theme,
            p,
        ));
    }
    heading
}
