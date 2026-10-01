use super::{
    Alignment, App, Edit, Element, Kind, Length, Message, Page, PathBuf, button, column, container, gallery, row,
    schema, set, text_input, visuals, widget,
};

impl App {
    pub(super) fn theme_file_picker(&self) -> Element<'static, Message> {
        let palette = visuals::Palette::from_resolved(&self.resolved.presented);
        let path = ["theme.file", "theme.light.file", "theme.dark.file"][self.theme_file_target];
        let file = self.draft.string(path, "");
        let filename = if file.is_empty() {
            "No override file".to_owned()
        } else {
            PathBuf::from(&file)
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned()
        };
        let labels = column([])
            .spacing(3)
            .width(Length::Fill)
            .push(self.label("Theme overrides", 13.))
            .push(widget::tooltip(
                self.label(filename, 11.)
                    .class(cosmic::theme::Text::Color(palette.muted)),
                self.label(
                    if file.is_empty() {
                        "Choose a partial KDL theme file".to_owned()
                    } else {
                        file.clone()
                    },
                    11.,
                ),
                widget::tooltip::Position::Bottom,
            ));
        let mut scopes = row([]).spacing(2);
        for (index, label) in ["Shared", "Light", "Dark"].into_iter().enumerate() {
            scopes = scopes.push(
                button::custom(self.label(label, 12.))
                    .name(format!("Theme file: {label}"))
                    .padding([6, 8])
                    .class(visuals::button_style(palette, index == self.theme_file_target))
                    .on_press(Message::ThemeFileTarget(index)),
            );
        }
        let mut controls = row([])
            .spacing(8)
            .align_y(Alignment::Center)
            .push(
                container(scopes)
                    .padding(3)
                    .class(visuals::surface(palette.sidebar, 10.)),
            )
            .push(
                ferese_theme::controls::text_button(
                    if file.is_empty() { "Choose…" } else { "Replace…" },
                    self.font,
                    palette,
                    false,
                )
                .on_press(Message::PickThemeFile(path)),
            );
        if !file.is_empty() {
            controls = controls.push(widget::tooltip(
                button::custom(visuals::action_icon("M6 6l12 12M6 18L18 6", palette.muted))
                    .padding(5)
                    .on_press(Message::Change(Edit::Unset(path.into()))),
                self.label("Remove override", 11.),
                widget::tooltip::Position::Bottom,
            ));
        }
        container(
            row([])
                .spacing(16)
                .align_y(Alignment::Center)
                .push(labels)
                .push(controls),
        )
        .padding([12, 12])
        .width(Length::Fill)
        .class(visuals::surface(palette.card, 0.))
        .into()
    }

    pub(super) fn auto_controls(&self) -> Element<'static, Message> {
        let palette = visuals::Palette::from_resolved(&self.resolved.presented);
        let scheduled = self.draft.string("theme.schedule.source", "schedule") == "schedule";
        let mut sources = row([]).spacing(4);
        for (source, label) in [("system", "System"), ("schedule", "Schedule")] {
            sources = sources.push(
                ferese_theme::controls::text_button(label, self.font, palette, scheduled == (source == "schedule"))
                    .name(format!("Auto source: {label}"))
                    .on_press(Message::Change(set("theme.schedule.source", source))),
            );
        }
        let description = if scheduled {
            "Switch between light and dark at set times."
        } else {
            "Follow the GTK/GNOME appearance preference."
        };
        let mut options = column([]).spacing(8).push(
            row([])
                .spacing(12)
                .align_y(Alignment::Center)
                .push(
                    column([])
                        .spacing(3)
                        .width(Length::Fill)
                        .push(self.label("Automatic appearance", 13.))
                        .push(self.note(description)),
                )
                .push(sources),
        );
        if scheduled {
            let mut times = row([]).spacing(16).align_y(Alignment::Center);
            for field in schema::fields(Page::Appearance)
                .into_iter()
                .filter(|field| field.path.starts_with("theme.schedule."))
            {
                let path = field.path.clone();
                let Kind::Text { default, .. } = &field.kind else {
                    continue;
                };
                let value = self
                    .inputs
                    .get(&path)
                    .cloned()
                    .unwrap_or_else(|| self.draft.string(&path, default));
                let submit = field.clone();
                let input = text_input(*default, value)
                    .font(self.font)
                    .size(12)
                    .style(visuals::input_style(palette))
                    .on_input(move |value| Message::Draft(path.clone(), value))
                    .on_submit(move |_| Message::Commit(submit.clone()))
                    .on_unfocus(Message::Commit(field.clone()))
                    .width(if field.path.ends_with("timezone") { 150 } else { 75 });
                times = times.push(
                    row([])
                        .spacing(8)
                        .align_y(Alignment::Center)
                        .push(self.label(field.label, 12.))
                        .push(input),
                );
            }
            options = options.push(times);
        }
        container(options)
            .padding(12)
            .width(Length::Fill)
            .class(visuals::surface(palette.card, 0.))
            .into()
    }

    pub(super) fn theme_gallery(
        &self,
        appearance: Option<ferese_config::theme::Appearance>,
    ) -> Element<'static, Message> {
        use ferese_config::theme::Appearance::{Dark, Light};
        let palette = visuals::Palette::from_resolved(&self.resolved.presented);
        let selected_id = visuals::family_selection(&self.draft, appearance);
        let choices = self.resolved.families.clone();
        let ids: Vec<_> = choices.iter().map(|family| family.id.clone()).collect();
        let mut heading = row([]).align_y(Alignment::Center).spacing(12).push(
            self.label(
                match appearance {
                    Some(Light) => "Light theme",
                    Some(Dark) => "Dark theme",
                    None => "Theme",
                },
                15.,
            )
            .width(Length::Fill),
        );
        if appearance != Some(Dark) {
            if self.undo.is_some() {
                heading = heading.push(
                    ferese_theme::controls::text_button("Undo", self.font, palette, false)
                        .on_press_maybe((!self.saving).then_some(Message::Undo)),
                );
            }
            heading = heading.push(
                ferese_theme::controls::text_button("Import…", self.font, palette, false)
                    .on_press(Message::ImportTheme),
            );
        }
        let mut gallery = column([]).spacing(10).push(heading);
        for (chunk, families) in choices.chunks(3).enumerate() {
            let mut tiles = row([]).spacing(12);
            for (offset, family) in families.iter().enumerate() {
                let selected = family.id == selected_id;
                let index = chunk * 3 + offset;
                let id = family.id.clone();
                let active = selected
                    .then_some(self.resolved.theme.appearance)
                    .filter(|a| appearance.is_none_or(|variant| variant == *a));
                let ids = ids.clone();
                let tile_id = gallery_id(&id, appearance);
                tiles = tiles.push(gallery::tile(
                    family,
                    gallery::TileOptions {
                        variant: appearance,
                        active,
                        selected,
                        palette,
                        font: self.font,
                        id: tile_id,
                    },
                    Message::Family(id, appearance),
                    move |key| {
                        gallery::neighbor(index, ids.len(), key)
                            .map(|index| Message::Family(ids[index].clone(), appearance))
                    },
                ));
            }
            for _ in families.len()..3 {
                tiles = tiles.push(widget::Space::new().width(Length::Fill));
            }
            gallery = gallery.push(tiles);
        }
        gallery::group(
            container(gallery).width(Length::Fill).max_width(640),
            cosmic::iced::advanced::widget::Id::new(format!("theme-gallery-{appearance:?}")),
            match appearance {
                Some(Light) => "Light theme",
                Some(Dark) => "Dark theme",
                None => "Theme",
            },
        )
    }
}

pub(super) fn gallery_id(
    id: &str,
    appearance: Option<ferese_config::theme::Appearance>,
) -> cosmic::iced::advanced::widget::Id {
    cosmic::iced::advanced::widget::Id::new(format!("theme-{appearance:?}-{id}"))
}
