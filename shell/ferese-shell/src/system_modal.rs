use super::*;
use cosmic::iced::widget::Space;
use cosmic::widget::{column, text};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum PowerAction {
    Poweroff,
    Reboot,
    Suspend,
    Logout(u32),
}

impl PowerAction {
    pub(super) fn from_status(action: status::Action) -> Option<Self> {
        match action {
            status::Action::Poweroff => Some(Self::Poweroff),
            status::Action::Reboot => Some(Self::Reboot),
            status::Action::Suspend => Some(Self::Suspend),
            _ => None,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Poweroff => "Power off",
            Self::Reboot => "Restart",
            Self::Suspend => "Suspend",
            Self::Logout(_) => "Log out",
        }
    }

    fn title(self) -> &'static str {
        match self {
            Self::Poweroff => "Power off this computer?",
            Self::Reboot => "Restart this computer?",
            Self::Suspend => "Suspend this computer?",
            Self::Logout(_) => "Log out of Ferese?",
        }
    }

    fn description(self) -> &'static str {
        match self {
            Self::Suspend => "Your apps will stay open. The session will not be locked.",
            Self::Logout(_) => "Save your work. Your apps will close when you log out.",
            _ => "Save your work before continuing.",
        }
    }
}

struct ModalSurface {
    id: window::Id,
    primary: bool,
    effects: Option<EffectsBinding>,
    regions: motion::Regions,
}

#[derive(Clone, Debug, PartialEq)]
enum Content {
    Power(PowerAction),
    Guide(Vec<keybinding_guide::Entry>),
}

pub(super) struct SystemModal {
    content: Content,
    surfaces: Vec<ModalSurface>,
    pub(super) motion: motion::PopupMotion,
    error: Option<String>,
}

impl SystemModal {
    pub(super) fn is_guide(&self) -> bool {
        matches!(self.content, Content::Guide(_))
    }

    pub(super) fn contains(&self, id: window::Id) -> bool {
        self.surfaces.iter().any(|surface| surface.id == id)
    }
}

impl FereseShell {
    pub(super) fn open_system_modal(
        &mut self,
        action: PowerAction,
        output_name: Option<&str>,
    ) -> Task<Message> {
        self.open_modal(Content::Power(action), output_name)
    }

    pub(super) fn rebuild_system_modal(&mut self) -> Task<Message> {
        let guide = self
            .system_modal
            .as_ref()
            .and_then(|modal| match &modal.content {
                Content::Guide(entries)
                    if !modal.motion.closing() && self.config.status.keybinding_guide =>
                {
                    Some(entries.clone())
                }
                _ => None,
            });
        let destroy = self.destroy_system_modal(true);
        if let Some(entries) = guide {
            if self.outputs.is_empty() {
                self.guide_shown = false;
                self.guide_attempts = 0;
                return destroy;
            }
            let open = self.open_guide(entries);
            Task::batch([destroy, open])
        } else {
            destroy
        }
    }

    pub(super) fn open_guide(&mut self, entries: Vec<keybinding_guide::Entry>) -> Task<Message> {
        self.open_modal(Content::Guide(entries), None)
    }

    fn open_modal(&mut self, content: Content, output_name: Option<&str>) -> Task<Message> {
        if self.pending_power.is_some() || self.outputs.is_empty() {
            if let Content::Power(PowerAction::Logout(serial)) = content
                && let Some(control) = &self.control
            {
                control.cancel_logout(serial);
            }
            return Task::none();
        }
        if self
            .system_modal
            .as_ref()
            .is_some_and(|modal| modal.content == content && !modal.motion.closing())
        {
            return Task::none();
        }
        let primary = output_name
            .and_then(|name| {
                self.outputs
                    .iter()
                    .position(|output| output.name.as_deref() == Some(name))
            })
            .or_else(|| {
                self.outputs
                    .iter()
                    .position(|output| output.bar == self.bar_surface_id)
            })
            .unwrap_or(0);
        let mut tasks = vec![
            self.destroy_system_modal(true),
            self.destroy_menu(),
            self.close_notification_history(),
        ];
        let mut surfaces = Vec::new();
        let mut outputs = (0..self.outputs.len()).collect::<Vec<_>>();
        outputs.sort_by_key(|index| *index == primary);
        for index in outputs {
            let id = window::Id::unique();
            let output = self.outputs[index].output.clone();
            let primary = index == primary;
            surfaces.push(ModalSurface {
                id,
                primary,
                effects: None,
                regions: Default::default(),
            });
            let action = cosmic::surface::action::app_layer_shell::<Self>(
                |_| Default::default(),
                move |_| SctkLayerSurfaceSettings {
                    id,
                    layer: Layer::Overlay,
                    keyboard_interactivity: if primary {
                        KeyboardInteractivity::Exclusive
                    } else {
                        KeyboardInteractivity::None
                    },
                    anchor: Anchor::TOP | Anchor::RIGHT | Anchor::BOTTOM | Anchor::LEFT,
                    output: IcedOutput::Output(output.clone()),
                    namespace: "ferese-system-modal".to_owned(),
                    exclusive_zone: -1,
                    size: Some((None, None)),
                    ..Default::default()
                },
                Some(Box::new(move |app| app.view_system_modal(id))),
            );
            tasks.push(cosmic::task::message(cosmic::Action::Surface(action)));
        }
        self.system_modal = Some(SystemModal {
            content,
            surfaces,
            motion: motion::PopupMotion::new(self.config.animations),
            error: None,
        });
        Task::batch(tasks)
    }

    pub(super) fn attach_power_material(
        &mut self,
        id: window::Id,
        surface: &wl_surface::WlSurface,
    ) {
        let Some(modal) = &mut self.system_modal else {
            return;
        };
        let Some(entry) = modal.surfaces.iter_mut().find(|entry| entry.id == id) else {
            return;
        };
        modal.motion.begin(Instant::now());
        if entry.primary && entry.effects.is_none() {
            match EffectsBinding::attach_role(surface, None, 0.0) {
                Ok(effects) => entry.effects = Some(effects),
                Err(error) => eprintln!("ferese-shell: system modal material unavailable: {error}"),
            }
        }
    }

    pub(super) fn update_power_materials(&self) {
        if let Some(modal) = &self.system_modal {
            for entry in &modal.surfaces {
                if let Some(effects) = &entry.effects {
                    let regions = entry.regions.lock().unwrap().clone();
                    let _ = effects
                        .set_material_regions(&regions, ferese_surface_effects_v1::Role::Modal);
                    let _ = effects.set_opacity(modal.motion.progress());
                }
            }
        }
    }

    pub(super) fn close_system_modal(&mut self) -> Task<Message> {
        if let Some(modal) = &mut self.system_modal {
            if let Content::Power(PowerAction::Logout(serial)) = modal.content
                && let Some(control) = &self.control
            {
                control.cancel_logout(serial);
            }
            modal.motion.retarget(0.0, Instant::now());
        }
        self.animate_system_modal()
    }

    pub(super) fn animate_system_modal(&mut self) -> Task<Message> {
        if self
            .system_modal
            .as_ref()
            .is_some_and(|modal| modal.motion.closing() && !modal.motion.animating())
        {
            return self.destroy_system_modal(false);
        }
        self.update_power_materials();
        Task::none()
    }

    pub(super) fn destroy_system_modal(&mut self, cancel: bool) -> Task<Message> {
        let Some(modal) = self.system_modal.take() else {
            return Task::none();
        };
        if cancel
            && let Content::Power(PowerAction::Logout(serial)) = modal.content
            && let Some(control) = &self.control
        {
            control.cancel_logout(serial);
        }
        Task::batch(
            modal
                .surfaces
                .into_iter()
                .map(|surface| destroy_layer_surface(surface.id)),
        )
    }

    pub(super) fn cancel_logout_modal(&mut self, serial: u32) -> Task<Message> {
        if self
            .system_modal
            .as_ref()
            .is_some_and(|modal| modal.content == Content::Power(PowerAction::Logout(serial)))
        {
            self.destroy_system_modal(false)
        } else {
            Task::none()
        }
    }

    pub(super) fn execute_system_modal(&mut self) -> Task<Message> {
        let Some(modal) = &mut self.system_modal else {
            return Task::none();
        };
        if modal.motion.closing() || self.pending_power.is_some() {
            return Task::none();
        }
        let Content::Power(power) = modal.content else {
            return self.close_system_modal();
        };
        let action = match power {
            PowerAction::Logout(serial) => {
                if let Some(control) = &self.control {
                    control.confirm_logout(serial);
                    return self.destroy_system_modal(false);
                }
                modal.error = Some("Ferese shell control is unavailable".to_owned());
                return Task::none();
            }
            PowerAction::Poweroff => status::Action::Poweroff,
            PowerAction::Reboot => status::Action::Reboot,
            PowerAction::Suspend => status::Action::Suspend,
        };
        let id = modal.surfaces[0].id;
        self.pending_power = Some((id, power));
        let destroy = self.destroy_system_modal(false);
        let execute = Task::perform(
            async move {
                tokio::task::spawn_blocking(move || status::execute_power(action))
                    .await
                    .map_err(|error| error.to_string())
                    .and_then(|result| result)
            },
            move |result| cosmic::Action::App(Message::PowerCompleted(id, result)),
        );
        Task::batch([destroy, execute])
    }

    pub(super) fn finish_power_action(
        &mut self,
        id: window::Id,
        result: Result<(), String>,
    ) -> Task<Message> {
        let Some((pending_id, action)) = self.pending_power else {
            return Task::none();
        };
        if id != pending_id {
            return Task::none();
        }
        self.pending_power = None;
        match result {
            Ok(()) => Task::none(),
            Err(error) => {
                let task = self.open_system_modal(action, None);
                if let Some(modal) = &mut self.system_modal {
                    modal.error = Some(error);
                }
                task
            }
        }
    }

    fn view_system_modal(&self, id: window::Id) -> Element<'_, cosmic::Action<Message>> {
        let Some(modal) = &self.system_modal else {
            return text("").into();
        };
        let Some(surface) = modal.surfaces.iter().find(|surface| surface.id == id) else {
            return text("").into();
        };
        let progress = modal.motion.progress();
        let content: Element<'_, cosmic::Action<Message>> = if surface.primary {
            let theme = self.config.theme;
            let material = surface.effects.is_some();
            let alpha = if material { 1.0 } else { progress };
            let mut palette = theme.palette();
            palette.text = palette.text.scale_alpha(alpha);
            palette.muted = palette.muted.scale_alpha(alpha);
            palette.sidebar = palette.sidebar.scale_alpha(alpha);
            palette.card = palette.card.scale_alpha(alpha);
            palette.accent = palette.accent.scale_alpha(alpha);
            let rows = match &modal.content {
                Content::Power(action) => {
                    let mut rows = column![
                        text(action.title()).font(shell_font()).size(23),
                        text(action.description())
                            .font(shell_font())
                            .size(14)
                            .class(theme::Text::Color(palette.muted)),
                    ]
                    .spacing(12);
                    if let Some(error) = &modal.error {
                        rows = rows.push(text(error).size(13));
                    }
                    rows.push(
                        row![
                            Space::new().width(Length::Fill),
                            button::text("Cancel")
                                .class(ferese_theme::controls::button_style(palette, false))
                                .on_press(cosmic::Action::App(Message::CancelPower)),
                            button::text(action.label())
                                .class(ferese_theme::controls::button_style(palette, true))
                                .on_press(cosmic::Action::App(Message::ExecutePower)),
                        ]
                        .spacing(10),
                    )
                }
                Content::Guide(entries) => {
                    let mut bindings = column::with_capacity(entries.len()).spacing(12);
                    for entry in entries {
                        bindings = bindings.push(
                            row![
                                text(&entry.keys)
                                    .font(shell_font())
                                    .size(13)
                                    .width(Length::FillPortion(1)),
                                text(&entry.description)
                                    .font(shell_font())
                                    .size(13)
                                    .width(Length::FillPortion(1)),
                            ]
                            .spacing(16),
                        );
                    }
                    let height = self
                        .outputs
                        .iter()
                        .filter_map(|output| output.size)
                        .map(|(_, height)| height)
                        .min()
                        .unwrap_or(720)
                        .saturating_sub(360)
                        .clamp(40, 400) as f32;
                    column![
                        text("Welcome to Ferese").font(shell_font()).size(23),
                        text("Your active shortcuts").font(shell_font()).size(14).class(theme::Text::Color(palette.muted)),
                        container(cosmic::widget::scrollable(bindings).height(Length::Shrink)).max_height(height),
                        text("Disable this guide in Settings → Shortcuts. It appears at each login until disabled.").font(shell_font()).size(12).class(theme::Text::Color(palette.muted)),
                        row![Space::new().width(Length::Fill), button::text("Got it").class(ferese_theme::controls::button_style(palette, true)).on_press(cosmic::Action::App(Message::CancelPower))],
                    ].spacing(14)
                }
            };
            container(rows)
                .id("ferese-blur-card")
                .width(Length::Fill)
                .max_width(if modal.is_guide() { 600 } else { 440 })
                .padding(24)
                .class(theme::Container::custom(move |_| container::Style {
                    background: (!material).then_some(Background::Color(color_with_opacity(
                        theme.surface_popover,
                        alpha,
                    ))),
                    text_color: Some(palette.text),
                    icon_color: Some(palette.text),
                    border: Border {
                        color: palette.muted.scale_alpha(0.16),
                        width: 1.0,
                        radius: theme.material_radius.into(),
                    },
                    ..Default::default()
                }))
                .into()
        } else {
            text("").into()
        };
        let content = if surface.primary {
            motion::animated(
                content,
                progress,
                surface.regions.clone(),
                self.config.theme.material_radius,
            )
            .into()
        } else {
            content
        };
        let centered = container(content)
            .width(Length::Fill)
            .height(Length::Fill)
            .padding(24)
            .align_x(cosmic::iced::Alignment::Center)
            .align_y(cosmic::iced::Alignment::Center)
            .class(theme::Container::custom(move |_| container::Style {
                background: Some(Background::Color(Color::from_rgba(
                    0.0,
                    0.0,
                    0.0,
                    0.34 * progress,
                ))),
                ..Default::default()
            }));
        centered.into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_power_actions_require_confirmation() {
        assert_eq!(
            PowerAction::from_status(status::Action::Poweroff),
            Some(PowerAction::Poweroff)
        );
        assert_eq!(
            PowerAction::from_status(status::Action::Reboot),
            Some(PowerAction::Reboot)
        );
        assert_eq!(
            PowerAction::from_status(status::Action::Suspend),
            Some(PowerAction::Suspend)
        );
        assert_eq!(
            PowerAction::from_status(status::Action::Brightness(50)),
            None
        );
        assert_eq!(PowerAction::Logout(1).label(), "Log out");
    }
}
