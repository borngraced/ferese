use cosmic::Element;
use cosmic::widget::{column, row, text_input};
use zeroize::{Zeroize, Zeroizing};

use super::ui::{Input, message};
use super::{Command, agent};
use crate::{App, Message, visuals};

impl App {
    pub(super) fn pairing_view(&self, prompt: &agent::Prompt) -> Element<'_, Message> {
        let s = &self.connections;
        let p = visuals::Palette::from_resolved(&self.resolved.presented);
        let device = s
            .snapshot
            .as_ref()
            .and_then(|s| s.bluetooth.as_ref().ok())
            .and_then(|b| b.devices.iter().find(|d| d.path == prompt.device))
            .map(|d| d.name.as_str())
            .unwrap_or("device");
        let explanation = match &prompt.kind {
            agent::Kind::Pin => "Enter the PIN shown on your device.".into(),
            agent::Kind::Passkey => "Enter the six-digit passkey shown on your device.".into(),
            agent::Kind::Confirm(key) => format!("Does {key:06} match the number on your device?"),
            agent::Kind::Authorize => "Allow this device to pair?".into(),
            agent::Kind::Service(uuid) => format!("Allow the device to use service {uuid}?"),
            agent::Kind::Display(text) => text.clone(),
        };
        let mut body = column([])
            .spacing(8)
            .push(self.label(format!("Pair with {device}"), 15.))
            .push(self.connection_note(&explanation));
        let id = prompt.id;
        let input = matches!(prompt.kind, agent::Kind::Pin | agent::Kind::Passkey);
        if input {
            body = body.push(
                text_input("PIN or passkey", s.pairing_input.as_str())
                    .id(cosmic::widget::Id::new("bluetooth-passkey"))
                    .password()
                    .font(self.font)
                    .size(13)
                    .padding([5, 8])
                    .style(visuals::input_style(p))
                    .on_input(move |value| message(Input::PairingValue(id, Zeroizing::new(value))))
                    .on_submit(move |mut value| {
                        value.zeroize();
                        message(Input::Answer(id, true))
                    }),
            );
        }
        let valid = agent::valid_answer(&prompt.kind, &s.pairing_input);
        let mut controls = row([]).spacing(8).push(self.connection_button(
            "Cancel pairing",
            Input::Run(Command::CancelPairing),
            true,
            false,
        ));
        if !matches!(prompt.kind, agent::Kind::Display(_)) {
            controls = controls.push(self.connection_button(
                if input { "Submit" } else { "Confirm" },
                Input::Answer(id, true),
                valid,
                true,
            ));
        }
        self.connection_card(body.push(controls))
    }
}
