mod schema;
mod store;
mod visuals;
mod watch;

use cosmic::{
    ApplicationExt, Element,
    app::{Core, Settings, Task},
    iced::{Alignment, Length},
    widget::{self, button, column, container, row, scrollable, slider, text_input, toggler},
};
use schema::{Field, Kind, Page};
use std::{collections::HashMap, path::PathBuf};
use store::{Edit, Snapshot, set};

fn main() -> cosmic::iced::Result {
    let mut args = std::env::args_os().skip(1);
    let path = match args.next() {
        Some(arg) if arg == "--config" => {
            PathBuf::from(args.next().expect("--config requires a path"))
        }
        Some(_) => {
            eprintln!("Usage: ferese-settings [--config PATH]");
            return Ok(());
        }
        None => store::config_path(),
    };
    let path = path.canonicalize().unwrap_or(path);
    let initial = Snapshot::read(&path);
    let theme = visuals::native_theme(initial.as_ref().ok());

    cosmic::app::run::<App>(
        Settings::default()
            .theme(theme)
            .is_daemon(false)
            .antialiasing(true)
            .default_text_size(14.),
        (path, initial),
    )
}

#[derive(Clone, Debug)]
enum Message {
    DragWindow,
    Page(Page),
    Search(String),
    Change(Edit),
    Draft(String, String),
    Commit(Field),
    Range(Field, f64),
    Release(Field),
    Saved(Result<(Snapshot, bool), String>),
    Reload,
    ExternalConfig(Result<Snapshot, String>),
    Undo,
    Preset(usize),
    PickWallpaper,
    AddNote,
    NoteAction(String, widget::text_editor::Action),
    SaveNote(String, u64),
    WallpaperPicked(Result<Option<String>, String>),
    NewCommand(String),
    AddCommand,
    AddSwipe(&'static str),
    Remove(String, usize),
    Thumbnail(String, Result<widget::image::Handle, String>),
}

struct App {
    note_editors: HashMap<String, NoteEditor>,
    core: Core,
    path: PathBuf,
    current: Snapshot,
    draft: Snapshot,
    pending: Vec<Edit>,
    saving: bool,
    undo: Option<String>,
    saving_previous: String,
    error: Option<String>,
    status: String,
    page: Page,
    search: String,
    inputs: HashMap<String, String>,
    ranges: HashMap<String, f64>,
    new_command: String,
    font: cosmic::font::Font,
    native_palette: visuals::Palette,
    thumbnail: Option<widget::image::Handle>,
    thumbnail_path: String,
    thumbnail_loading: bool,
    thumbnail_error: Option<String>,
}

struct NoteEditor {
    content: widget::text_editor::Content<cosmic::Renderer>,
    revision: u64,
    dirty: bool,
}

impl cosmic::Application for App {
    type Executor = cosmic::executor::Default;
    type Flags = (PathBuf, Result<Snapshot, String>);
    type Message = Message;
    const APP_ID: &'static str = "dev.ferese.Settings";

    fn core(&self) -> &Core {
        &self.core
    }

    fn core_mut(&mut self) -> &mut Core {
        &mut self.core
    }

    fn init(mut core: Core, (path, initial): Self::Flags) -> (Self, Task<Message>) {
        core.window.show_headerbar = false;

        let status = if path == store::config_path() {
            "Changes save automatically"
        } else {
            "Preview config · changes do not update your desktop"
        }
        .into();
        let error = initial.as_ref().err().cloned();
        let current = initial.unwrap_or_else(|_| Snapshot::parse(String::new()).unwrap());
        let font = visuals::configured_font(&current);
        let native_palette = visuals::Palette::from(&current);
        let mut app = Self {
            note_editors: HashMap::new(),
            core,
            path,
            draft: current.clone(),
            current,
            pending: vec![],
            saving: false,
            undo: None,
            saving_previous: String::new(),
            error,
            status,
            page: Page::Appearance,
            search: String::new(),
            inputs: HashMap::new(),
            ranges: HashMap::new(),
            new_command: String::new(),
            font,
            native_palette,
            thumbnail: None,
            thumbnail_path: String::new(),
            thumbnail_loading: false,
            thumbnail_error: None,
        };
        let task = app
            .core
            .main_window_id()
            .map(|id| app.set_window_title("Ferese Settings".into(), id))
            .unwrap_or_else(Task::none);

        app.sync_notes();
        (app, task)
    }

    fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::NoteAction(id, action) => {
                if let Some(editor) = self.note_editors.get_mut(&id) {
                    let edited = action.is_edit();
                    editor.content.perform(action);

                    if edited {
                        editor.dirty = true;
                        editor.revision = editor.revision.wrapping_add(1);
                        let revision = editor.revision;
                        return cosmic::task::future(async move {
                            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                            Message::SaveNote(id, revision)
                        });
                    }
                }
            }
            Message::SaveNote(id, revision) => {
                let Some(index) = self.note_index(&id) else {
                    return Task::none();
                };

                if let Some(editor) = self.note_editors.get_mut(&id)
                    && editor.dirty
                    && editor.revision == revision
                {
                    let value = editor.content.text();
                    editor.dirty = false;
                    return self.change(set(&format!("desktop_widgets.notes.{index}.text"), value));
                }
            }
            Message::AddNote
                if !self.saving && self.draft.records("desktop_widgets.notes") < 32 =>
            {
                let id = format!(
                    "note-{}",
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_nanos()
                );
                let task = self.change(Edit::Add(
                    "desktop_widgets.notes".into(),
                    vec![
                        ("id".into(), id.into()),
                        ("title".into(), "Note".into()),
                        ("text".into(), "".into()),
                    ],
                ));

                self.sync_notes();
                return task;
            }
            Message::DragWindow => return self.core.drag(None),
            Message::ExternalConfig(result) => match result {
                Ok(snapshot)
                    if snapshot.source == self.current.source
                        || snapshot.source == self.draft.source => {}
                Ok(snapshot) => {
                    if self.saving
                        || self.note_editors.values().any(|editor| editor.dirty)
                        || !self.inputs.is_empty()
                        || !self.ranges.is_empty()
                        || !self.pending.is_empty()
                    {
                        self.status =
                            "Config changed externally · Reload when your edits are finished"
                                .into();
                    } else {
                        self.current = snapshot.clone();
                        self.draft = snapshot;
                        self.sync_notes();
                        self.font = visuals::configured_font(&self.current);
                        self.undo = None;
                        self.error = None;
                        self.status = "Updated from your config".into();

                        return Task::batch([self.update_theme(), self.load_thumbnail()]);
                    }
                }
                Err(error) => self.error = Some(format!("Config reload failed: {error}")),
            },
            Message::Page(page) => {
                self.page = page;
                self.search.clear();
                if page == Page::Wallpaper {
                    return self.load_thumbnail();
                }
            }
            Message::Search(query) => self.search = query,
            Message::Draft(path, value) => {
                self.inputs.insert(path, value);
            }
            Message::Range(field, value) => {
                self.ranges.insert(field.path, value);
            }
            Message::Release(field) => {
                if let Some(value) = self.ranges.remove(&field.path) {
                    let integer = matches!(field.kind, Kind::Range { integer: true, .. });
                    return self.change(if integer {
                        set(&field.path, value.round() as i64)
                    } else {
                        set(&field.path, value)
                    });
                }
            }
            Message::Commit(field) => {
                if let Some(value) = self.inputs.remove(&field.path) {
                    let edit = if matches!(field.kind, Kind::Text { argv: true, .. }) {
                        match shlex::split(&value).filter(|v| !v.is_empty()) {
                            Some(args) => set(
                                &field.path,
                                args.into_iter()
                                    .map(serde_json::Value::from)
                                    .collect::<Vec<_>>(),
                            ),
                            None => {
                                self.error = Some("Enter a program and arguments with balanced quotes. Commands are not run through a shell.".into());
                                return Task::none();
                            }
                        }
                    } else {
                        set(&field.path, value)
                    };
                    return self.change(edit);
                }
            }
            Message::Change(edit) => return self.change(edit),
            Message::Saved(result) => {
                self.saving = false;
                match result {
                    Ok((snapshot, live)) => {
                        self.undo = Some(std::mem::take(&mut self.saving_previous));
                        self.current = snapshot;
                        self.font = visuals::configured_font(&self.current);
                        self.draft = self.current.clone();

                        for edit in &self.pending {
                            if let Err(error) = self.draft.edit(edit) {
                                self.error = Some(error);
                            }
                        }

                        self.sync_notes();
                        self.status = if live {
                            "Saved · desktop updated"
                        } else if self.path != store::config_path() {
                            "Saved to preview config · desktop unchanged"
                        } else {
                            "Saved · could not confirm desktop reload"
                        }
                        .into();

                        return Task::batch([
                            self.update_theme(),
                            self.flush(),
                            self.load_thumbnail(),
                        ]);
                    }
                    Err(error) => {
                        self.error = Some(error);
                        self.pending.clear();
                        self.draft = self.current.clone();
                        self.status = "Not saved".into();
                    }
                }
            }
            Message::Reload if !self.saving => match Snapshot::read(&self.path) {
                Ok(snapshot) => {
                    self.current = snapshot.clone();
                    self.draft = snapshot;
                    self.note_editors.clear();
                    self.sync_notes();
                    self.font = visuals::configured_font(&self.current);
                    self.pending.clear();
                    self.inputs.clear();
                    self.ranges.clear();
                    self.error = None;
                    self.undo = None;
                    self.status = "Reloaded from your config".into();

                    return Task::batch([self.update_theme(), self.load_thumbnail()]);
                }
                Err(error) => self.error = Some(error),
            },
            Message::Undo if !self.saving => {
                if let Some(source) = self.undo.take() {
                    return self.start_save(source);
                }
            }
            Message::Preset(index) => {
                for edit in visuals::preset(index) {
                    if let Err(error) = self.draft.edit(&edit) {
                        self.error = Some(error);
                        return Task::none();
                    }

                    self.pending.push(edit);
                }
                return self.flush();
            }
            Message::PickWallpaper => {
                return cosmic::task::future(async {
                    let (send, receive) = cosmic::iced::futures::channel::oneshot::channel();
                    // Native GTK file chooser, independent of the desktop portal.
                    // Waiting for user input must never block the UI executor.
                    std::thread::spawn(move || {
                        let result = std::process::Command::new("zenity")
                            .args([
                                "--file-selection",
                                "--title=Choose a wallpaper",
                                "--file-filter=Images | *.png *.jpg *.jpeg *.webp *.PNG *.JPG *.JPEG *.WEBP",
                                "--file-filter=All files | *",
                            ])
                            .output()
                            .map_err(|error| format!("Could not open the file chooser: {error}. Install zenity or enter an image path."))
                            .and_then(|output| {
                                if output.status.code() == Some(1) {
                                    return Ok(None); // User cancelled.
                                }
                                if !output.status.success() {
                                    return Err(format!("File chooser failed: {}", String::from_utf8_lossy(&output.stderr).trim()));
                                }
                                let path = String::from_utf8(output.stdout)
                                    .map_err(|_| "The image path is not valid UTF-8.".to_owned())?;
                                let path = path.trim_end_matches(['\r', '\n']);
                                Ok((!path.is_empty()).then(|| path.to_owned()))
                            });
                        let _ = send.send(result);
                    });
                    Message::WallpaperPicked(
                        receive
                            .await
                            .unwrap_or_else(|_| Err("File chooser worker stopped.".into())),
                    )
                });
            }
            Message::WallpaperPicked(Ok(Some(path))) => {
                return self.change(set("theme.background.path", path));
            }
            Message::WallpaperPicked(Err(error)) => self.error = Some(error),
            Message::Thumbnail(path, result) => {
                self.thumbnail_loading = false;
                if path
                    == self
                        .current
                        .string("theme.background.path", ferese_config::default_wallpaper())
                {
                    self.thumbnail_path = path;
                    match result {
                        Ok(handle) => {
                            self.thumbnail = Some(handle);
                            self.thumbnail_error = None;
                        }
                        Err(error) => {
                            self.thumbnail = None;
                            self.thumbnail_error = Some(error);
                        }
                    }
                }
                return self.load_thumbnail();
            }
            Message::NewCommand(value) => self.new_command = value,
            Message::AddCommand if !self.saving => {
                match shlex::split(&self.new_command).filter(|a| !a.is_empty()) {
                    Some(args) => {
                        self.new_command.clear();
                        return self.change(Edit::Add(
                            "autostart".into(),
                            vec![
                                (
                                    "command".into(),
                                    args.into_iter()
                                        .map(serde_json::Value::from)
                                        .collect::<Vec<_>>()
                                        .into(),
                                ),
                                ("enabled".into(), true.into()),
                                ("restart".into(), true.into()),
                            ],
                        ));
                    }
                    None => {
                        self.error =
                            Some("Enter a program and arguments with balanced quotes.".into())
                    }
                }
            }
            Message::AddSwipe(keys) if !self.saving => {
                let (action, argument) = match keys {
                    "Swipe3Up" => ("workspace-next", None),
                    "Swipe3Down" => ("workspace-previous", None),
                    "Swipe3Left" => ("focus", Some("right")),
                    _ => ("focus", Some("left")),
                };
                let mut fields = vec![
                    ("keys".into(), keys.into()),
                    ("action".into(), action.into()),
                ];

                if let Some(argument) = argument {
                    fields.push(("argument".into(), argument.into()));
                }

                return self.change(Edit::Add("bindings".into(), fields));
            }
            Message::Remove(table, index) if !self.saving => {
                self.inputs.clear();
                let task = self.change(Edit::Remove(table, index));
                self.sync_notes();
                return task;
            }
            _ => {}
        }
        Task::none()
    }

    fn subscription(&self) -> cosmic::iced::Subscription<Message> {
        cosmic::iced::Subscription::run_with(self.path.clone(), watch::changes)
    }

    fn view(&self) -> Element<'_, Message> {
        let palette = visuals::Palette::from(&self.draft);
        let mut sidebar = column([])
            .spacing(3)
            .push(
                widget::mouse_area(
                    row([])
                        .spacing(10)
                        .align_y(Alignment::Center)
                        .push(visuals::brand_icon())
                        .push(self.label("Ferese", 16.))
                        .width(Length::Fill),
                )
                .on_drag(Message::DragWindow),
            )
            .push(widget::Space::new().height(14))
            .push(
                text_input("Search settings", self.search.clone())
                    .on_input(Message::Search)
                    .font(self.font)
                    .style(visuals::input_style(palette))
                    .size(12),
            )
            .push(widget::Space::new().height(8));
        for page in Page::ALL {
            sidebar = sidebar.push(
                button::custom(
                    row([])
                        .spacing(10)
                        .align_y(Alignment::Center)
                        .push(visuals::icon(
                            page,
                            if self.page == page {
                                palette.accent
                            } else {
                                palette.muted
                            },
                        ))
                        .push(self.label(page.title(), 13.)),
                )
                .width(Length::Fill)
                .padding([8, 8])
                .class(visuals::navigation_style(palette, self.page == page))
                .on_press(Message::Page(page)),
            );
        }
        let sidebar = container(sidebar.push(widget::Space::new().height(Length::Fill)))
            .width(204)
            .height(Length::Fill)
            .padding([20, 10])
            .class(visuals::surface(palette.sidebar, 0.));
        let heading = row([]).align_y(Alignment::Center).spacing(20).push(
            column([])
                .spacing(5)
                .push(self.label(
                    if self.search.is_empty() {
                        self.page.title()
                    } else {
                        "Search"
                    },
                    24.,
                ))
                .push(
                    self.label(
                        if self.search.is_empty() {
                            self.page.subtitle()
                        } else {
                            "Find a setting for your desktop."
                        },
                        12.,
                    )
                    .class(cosmic::theme::Text::Color(palette.muted)),
                )
                .width(Length::Fill),
        );
        let mut body = column([]).spacing(12);

        if !self.search.is_empty() {
            if !Page::ALL.into_iter().any(|p| p.matches(&self.search)) {
                body = body
                    .push(self.note("No matching settings. Try wallpaper, keyboard, or motion."));
            }
            for page in Page::ALL.into_iter().filter(|p| p.matches(&self.search)) {
                body = body.push(
                    button::custom(
                        column([])
                            .spacing(5)
                            .push(self.label(page.title(), 16.))
                            .push(self.label(page.subtitle(), 12.)),
                    )
                    .width(Length::Fill)
                    .padding(20)
                    .class(visuals::button_style(palette, false))
                    .on_press(Message::Page(page)),
                );
            }
        } else {
            if matches!(self.page, Page::Bar | Page::Windows) {
                body = body.push(visuals::preview(&self.draft));
            }
            if self.page == Page::Appearance {
                body = body.push(
                    column([])
                        .spacing(4)
                        .push(self.label("Theme", 15.))
                        .push(self.note("Choose a look for your desktop.")),
                );
                let mut presets = column([]).spacing(10);
                for (row_index, choices) in visuals::PRESETS.chunks(3).enumerate() {
                    let mut tiles = row([]).spacing(10);
                    for (column_index, preset) in choices.iter().enumerate() {
                        let index = row_index * 3 + column_index;
                        let selected = visuals::preset_selected(&self.draft, index);
                        let indicator: Element<'_, Message> = if selected {
                            visuals::action_icon("M5 12l4 4L19 6", palette.accent).into()
                        } else {
                            widget::Space::new().width(16).height(16).into()
                        };
                        let tile = button::custom(
                            column([])
                                .spacing(8)
                                .push(visuals::preset_preview(index))
                                .push(
                                    row([])
                                        .spacing(6)
                                        .align_y(Alignment::Center)
                                        .push(self.label(preset.name, 12.).width(Length::Fill))
                                        .push(indicator),
                                )
                                .push(
                                    self.label(preset.description, 10.)
                                        .class(cosmic::theme::Text::Color(palette.muted)),
                                ),
                        )
                        .width(Length::Fill)
                        .padding(10)
                        .class(visuals::button_style(palette, selected));
                        tiles = tiles.push(if self.saving {
                            tile
                        } else {
                            tile.on_press(Message::Preset(index))
                        });
                    }
                    presets = presets.push(tiles);
                }
                body = body.push(presets);
            }

            if self.page == Page::Wallpaper {
                if let Some(handle) = &self.thumbnail {
                    body = body.push(
                        widget::image(handle.clone())
                            .width(Length::Fill)
                            .height(190)
                            .content_fit(cosmic::iced::ContentFit::Contain),
                    );
                } else {
                    body = body.push(self.note(if self.thumbnail_loading {
                        "Loading wallpaper preview…"
                    } else {
                        self.thumbnail_error
                            .as_deref()
                            .unwrap_or("Choose an image to preview your wallpaper.")
                    }));
                }
                body =
                    body.push(button::standard("Choose image…").on_press(Message::PickWallpaper));
            }
            let fields = schema::fields(self.page);

            if !fields.is_empty() {
                let mut group = column([]).spacing(1);
                for field in fields {
                    group = group.push(self.field(field));
                }
                body = body.push(
                    container(group)
                        .padding(8)
                        .class(visuals::surface(palette.card, 14.)),
                );
            }

            match self.page {
                Page::Desktop => {
                    let can_change_list =
                        !self.saving && !self.note_editors.values().any(|e| e.dirty);
                    body = body.push(self.label("Sticky notes", 16.));
                    body = body.push(self.note("Edit here; notes save after you pause typing. Desktop cards stay behind windows and are click-through."));
                    for index in 0..self.draft.records("desktop_widgets.notes") {
                        let id = self
                            .draft
                            .string(&format!("desktop_widgets.notes.{index}.id"), "note");
                        let mut group = column([]).spacing(8);

                        for field in schema::note_fields(index) {
                            group = group.push(self.field(field));
                        }

                        if let Some(editor) = self.note_editors.get(&id) {
                            let id = id.clone();
                            group = group.push(
                                widget::TextEditor::new(&editor.content)
                                    .height(160)
                                    .font(self.font)
                                    .size(14.)
                                    .on_action(move |action| {
                                        Message::NoteAction(id.clone(), action)
                                    }),
                            );
                        }
                        group =
                            group.push(button::destructive("Remove note").on_press_maybe(
                                can_change_list.then_some(Message::Remove(
                                    "desktop_widgets.notes".into(),
                                    index,
                                )),
                            ));
                        body = body.push(
                            container(group)
                                .padding(12)
                                .class(visuals::surface(palette.card, 14.)),
                        );
                    }
                    body = body.push(
                        button::standard("Add note").on_press_maybe(
                            (can_change_list && self.draft.records("desktop_widgets.notes") < 32)
                                .then_some(Message::AddNote),
                        ),
                    );
                }
                Page::Startup => {
                    body = body.push(self.note("Login items update live. Disabling or removing an item stops the session-owned process; enabling one starts it."));

                    for index in 0..self.draft.records("autostart") {
                        let prefix = format!("autostart.{index}");
                        let group = column([])
                            .spacing(1)
                            .push(self.field(Field::new(
                                format!("{prefix}.command"),
                                "Program",
                                "A program and its arguments, not a shell script.",
                                Kind::Text {
                                    default: "",
                                    argv: true,
                                },
                            )))
                            .push(self.field(Field::new(
                                format!("{prefix}.enabled"),
                                "Open at login",
                                "",
                                Kind::Toggle(true),
                            )))
                            .push(self.field(Field::new(
                                format!("{prefix}.restart"),
                                "Restart if it exits",
                                "Managed by the Ferese session.",
                                Kind::Toggle(true),
                            )))
                            .push(
                                container(
                                    button::destructive("Remove item").on_press_maybe(
                                        (!self.saving)
                                            .then_some(Message::Remove("autostart".into(), index)),
                                    ),
                                )
                                .padding(12),
                            );
                        body = body.push(
                            container(group)
                                .padding(8)
                                .class(visuals::surface(palette.card, 14.)),
                        );
                    }
                    body = body.push(
                        row([])
                            .spacing(10)
                            .push(
                                text_input("Program and arguments", self.new_command.clone())
                                    .on_input(Message::NewCommand),
                            )
                            .push(
                                button::standard("Add login item").on_press_maybe(
                                    (!self.saving && !self.new_command.trim().is_empty())
                                        .then_some(Message::AddCommand),
                                ),
                            ),
                    );
                }
                Page::Shortcuts => {
                    body = body.push(self.note("Keyboard and swipe bindings share the same actions. Use Swipe3Up, Swipe3Down, Swipe3Left, or Swipe3Right as a shortcut (also supports 4 or 5 fingers). Actions include toggle-overview, spawn, focus, move, workspace-next, workspace-previous, and none. Leave Argument empty for actions such as toggle-overview or none."));
                    let mut gestures = row([]).spacing(6);
                    for (keys, label) in [
                        ("Swipe3Up", "Customize swipe up"),
                        ("Swipe3Down", "Customize swipe down"),
                        ("Swipe3Left", "Customize swipe left"),
                        ("Swipe3Right", "Customize swipe right"),
                    ] {
                        let exists = (0..self.draft.records("bindings")).any(|index| {
                            self.draft
                                .string(&format!("bindings.{index}.keys"), "")
                                .eq_ignore_ascii_case(keys)
                        });

                        if !exists {
                            gestures =
                                gestures.push(button::standard(label).on_press_maybe(
                                    (!self.saving).then_some(Message::AddSwipe(keys)),
                                ));
                        }
                    }

                    body = body.push(gestures);

                    for index in 0..self.draft.records("bindings") {
                        let prefix = format!("bindings.{index}");
                        let group = column([])
                            .spacing(1)
                            .push(self.field(schema::text(
                                format!("{prefix}.keys"),
                                "Shortcut",
                                "For example: Super+Return or Swipe3Up",
                                "",
                            )))
                            .push(self.field(schema::text(
                                format!("{prefix}.action"),
                                "Action",
                                "Action such as toggle-overview, spawn, focus, move, or none.",
                                "",
                            )))
                            .push(self.field(schema::text(
                                format!("{prefix}.argument"),
                                "Argument",
                                "Command name, direction, or action argument.",
                                "",
                            )));
                        body = body.push(
                            container(group)
                                .padding(8)
                                .class(visuals::surface(palette.card, 14.)),
                        );
                    }
                    if self.draft.records("bindings") == 0 {
                        body = body.push(self.note("You are using the built-in shortcuts. Add custom bindings in config.kdl; they will appear here after Reload."));
                    }
                }
                Page::Displays => {
                    body = body.push(self.note("Display profiles update connected outputs live. Keep your config open for adding profiles or changing display positions."));
                    for profile in 0..self.draft.records("output_profiles") {
                        let prefix = format!("output_profiles.{profile}");
                        body = body.push(
                            self.label(
                                self.draft
                                    .string(&format!("{prefix}.name"), "Display profile"),
                                17.,
                            ),
                        );

                        for output in 0..self.draft.records(&format!("{prefix}.outputs")) {
                            let prefix = format!("{prefix}.outputs.{output}");
                            let group = column([]).spacing(1)
                                .push(self.field(schema::text(format!("{prefix}.match"), "Display", "Output name or matching pattern.", "")))
                                .push(self.field(schema::text(format!("{prefix}.mode"), "Resolution", "For example: 2560x1440@60. Leave unchanged to retain automatic selection.", "")))
                                .push(self.field(schema::range(format!("{prefix}.scale"), "Scale", "Logical size of text and controls.", 1., 0.75, 3., 0.25, "×", false)));
                            body = body.push(
                                container(group)
                                    .padding(8)
                                    .class(visuals::surface(palette.card, 14.)),
                            );
                        }
                    }
                    if self.draft.records("output_profiles") == 0 {
                        body = body.push(self.note("No saved profiles. Ferese currently configures connected displays automatically."));
                    }
                }
                _ => {}
            }
        }

        let mut content = column([]).spacing(16).push(heading);

        if let Some(error) = &self.error {
            content = content.push(
                container(self.label(error.clone(), 12.))
                    .padding(12)
                    .width(Length::Fill)
                    .class(visuals::surface(palette.error, 10.)),
            );
        }

        content = content.push(
            scrollable(
                container(body)
                    .padding(cosmic::iced::Padding {
                        top: 0.,
                        right: 12.,
                        bottom: 0.,
                        left: 2.,
                    })
                    .width(Length::Fill),
            )
            .direction(cosmic::iced::widget::scrollable::Direction::Vertical(
                cosmic::iced::widget::scrollable::Scrollbar::new()
                    .width(10)
                    .scroller_width(3),
            ))
            .height(Length::Fill),
        );
        let footer = row([])
            .spacing(4)
            .align_y(Alignment::Center)
            .push(widget::tooltip(
                visuals::action_icon(
                    if self.saving {
                        "M12 3v9l5 3 M21 12a9 9 0 1 1-9-9"
                    } else {
                        "M9 12l2 2 4-4 M21 12a9 9 0 1 1-18 0 9 9 0 0 1 18 0"
                    },
                    palette.muted,
                ),
                self.label(
                    if self.saving {
                        "Saving…"
                    } else {
                        &self.status
                    },
                    11.,
                ),
                widget::tooltip::Position::Top,
            ))
            .push(widget::Space::new().width(Length::Fill))
            .push(widget::tooltip(
                button::custom(visuals::action_icon(
                    "M3 10h8 M3 10V3 M3 10c3-7 17-6 17 3a7 7 0 0 1-7 7",
                    if self.undo.is_some() && !self.saving {
                        palette.text
                    } else {
                        palette.muted
                    },
                ))
                .padding(4)
                .on_press_maybe((self.undo.is_some() && !self.saving).then_some(Message::Undo)),
                self.label("Undo last change", 11.),
                widget::tooltip::Position::Top,
            ))
            .push(widget::tooltip(
                button::custom(visuals::action_icon(
                    "M21 3v6h-6 M3 21v-6h6 M3 9a9 9 0 0 1 15-6l3 6 M21 15a9 9 0 0 1-15 6l-3-6",
                    palette.text,
                ))
                .padding(4)
                .on_press_maybe((!self.saving).then_some(Message::Reload)),
                self.label("Reload configuration", 11.),
                widget::tooltip::Position::Top,
            ));
        row([])
            .push(sidebar)
            .push(
                container(
                    container(
                        column([])
                            .push(content.height(Length::Fill))
                            .push(footer)
                            .spacing(6),
                    )
                    .padding(cosmic::iced::Padding {
                        top: 20.,
                        right: 20.,
                        bottom: 8.,
                        left: 20.,
                    })
                    .max_width(840)
                    .width(Length::Fill)
                    .height(Length::Fill),
                )
                .width(Length::Fill)
                .center_x(Length::Fill)
                .height(Length::Fill)
                .class(visuals::surface(palette.background, 0.)),
            )
            .into()
    }
}

impl App {
    fn update_theme(&mut self) -> Task<Message> {
        let palette = visuals::Palette::from(&self.draft);
        if palette == self.native_palette {
            return Task::none();
        }
        self.native_palette = palette;
        cosmic::command::set_theme(visuals::native_theme(Some(&self.draft)))
    }

    fn note_index(&self, id: &str) -> Option<usize> {
        (0..self.draft.records("desktop_widgets.notes")).find(|index| {
            self.draft
                .string(&format!("desktop_widgets.notes.{index}.id"), "note")
                == id
        })
    }

    fn sync_notes(&mut self) {
        let mut ids = Vec::new();

        for index in 0..self.draft.records("desktop_widgets.notes") {
            let prefix = format!("desktop_widgets.notes.{index}");
            let id = self.draft.string(&format!("{prefix}.id"), "note");
            let text = self.draft.string(&format!("{prefix}.text"), "");
            let editor = self
                .note_editors
                .entry(id.clone())
                .or_insert_with(|| NoteEditor {
                    content: widget::text_editor::Content::with_text(&text),
                    revision: 0,
                    dirty: false,
                });
            if !editor.dirty && editor.content.text() != text {
                editor.content = widget::text_editor::Content::with_text(&text);
            }
            ids.push(id);
        }

        self.note_editors.retain(|id, _| ids.contains(id));
    }

    fn load_thumbnail(&mut self) -> Task<Message> {
        let path = self
            .current
            .string("theme.background.path", ferese_config::default_wallpaper());
        if self.page != Page::Wallpaper || self.thumbnail_loading || path == self.thumbnail_path {
            return Task::none();
        }
        if path.is_empty() {
            self.thumbnail = None;
            self.thumbnail_path.clear();
            self.thumbnail_error = None;
            return Task::none();
        }

        self.thumbnail_loading = true;
        cosmic::task::future(async move {
            let (send, receive) = cosmic::iced::futures::channel::oneshot::channel();
            let file = path.clone();
            std::thread::spawn(move || {
                let result = (|| -> Result<_, String> {
                    let mut reader = image::ImageReader::open(&file)
                        .map_err(|e| e.to_string())?
                        .with_guessed_format()
                        .map_err(|e| e.to_string())?;
                    let mut limits = image::Limits::default();
                    limits.max_alloc = Some(256 * 1024 * 1024);
                    limits.max_image_width = Some(16384);
                    limits.max_image_height = Some(16384);
                    reader.limits(limits);
                    let pixels = reader
                        .decode()
                        .map_err(|e| e.to_string())?
                        .thumbnail(1000, 500)
                        .into_rgba8();
                    Ok(widget::image::Handle::from_rgba(
                        pixels.width(),
                        pixels.height(),
                        pixels.into_raw(),
                    ))
                })();
                let _ = send.send(result);
            });
            Message::Thumbnail(
                path,
                receive
                    .await
                    .unwrap_or_else(|_| Err("Could not load wallpaper preview.".into())),
            )
        })
    }

    fn label<'a>(
        &self,
        text: impl Into<std::borrow::Cow<'a, str>> + 'a,
        size: f32,
    ) -> widget::Text<'a, cosmic::Theme, cosmic::Renderer> {
        widget::text(text).size(size).font(self.font)
    }

    fn note(&self, text: &str) -> Element<'static, Message> {
        let palette = visuals::Palette::from(&self.draft);
        self.label(text.to_owned(), 12.)
            .class(cosmic::theme::Text::Color(palette.muted))
            .into()
    }

    fn field(&self, field: Field) -> Element<'static, Message> {
        let palette = visuals::Palette::from(&self.draft);
        let mut labels = column([])
            .spacing(3)
            .push(self.label(field.label.clone(), 13.));
        if !field.description.is_empty() {
            labels = labels.push(
                self.label(field.description.clone(), 11.)
                    .class(cosmic::theme::Text::Color(palette.muted)),
            );
        }
        let labels = labels.width(Length::Fill);
        let path = field.path.clone();
        let control: Element<'static, Message> = match field.kind.clone() {
            Kind::Toggle(default) => toggler(self.draft.boolean(&path, default))
                .size(22)
                .on_toggle(move |value| Message::Change(set(&path, value)))
                .into(),
            Kind::Range {
                default,
                min,
                max,
                step,
                suffix,
                integer,
            } => {
                let value = self
                    .ranges
                    .get(&path)
                    .copied()
                    .unwrap_or_else(|| self.draft.number(&path, default));
                let display = if integer || step >= 1. {
                    format!("{value:.0}{suffix}")
                } else {
                    format!("{value:.2}{suffix}")
                };
                let release = field.clone();
                row([])
                    .align_y(Alignment::Center)
                    .spacing(14)
                    .push(
                        slider(min..=max, value, move |value| {
                            Message::Range(field.clone(), value)
                        })
                        .step(step)
                        .on_release(Message::Release(release))
                        .width(145),
                    )
                    .push(self.label(display, 12.).width(65))
                    .into()
            }
            Kind::Choice { default, choices } => {
                let value = self.draft.string(&path, default);
                let mut options = row([]).spacing(2);
                for (key, label) in choices {
                    options = options.push(
                        button::custom(self.label(*label, 12.))
                            .padding([6, 9])
                            .class(visuals::button_style(palette, value == *key))
                            .on_press(Message::Change(set(&path, *key))),
                    );
                }
                container(options)
                    .padding(3)
                    .class(visuals::surface(palette.sidebar, 10.))
                    .into()
            }
            Kind::Text { default, argv } => {
                let value = self.inputs.get(&path).cloned().unwrap_or_else(|| {
                    if argv {
                        self.draft.argv(&path)
                    } else {
                        self.draft.string(&path, default)
                    }
                });
                let commit = field.clone();
                let is_color = path.starts_with("theme.colors.");
                let swatch = visuals::color(&value, palette.accent);
                let input = text_input(default, value)
                    .font(self.font)
                    .style(visuals::input_style(palette))
                    .on_input(move |value| Message::Draft(path.clone(), value))
                    .on_submit(move |_| Message::Commit(field.clone()))
                    .on_unfocus(Message::Commit(commit))
                    .width(if is_color { 155 } else { 225 })
                    .size(12);
                if is_color {
                    row([])
                        .spacing(8)
                        .align_y(Alignment::Center)
                        .push(
                            container(widget::Space::new().width(24).height(24))
                                .class(visuals::surface(swatch, 6.)),
                        )
                        .push(input)
                        .into()
                } else {
                    input.into()
                }
            }
        };
        container(
            row([])
                .spacing(16)
                .align_y(Alignment::Center)
                .push(labels)
                .push(control),
        )
        .padding([12, 12])
        .width(Length::Fill)
        .class(visuals::surface(palette.card, 0.))
        .into()
    }

    fn change(&mut self, edit: Edit) -> Task<Message> {
        match self.draft.edit(&edit) {
            Ok(()) => {
                // When a gradient exists, changing Accent also changes its
                // leading stop; otherwise the visible focus border would stay
                // on the old palette despite the control saying it changed.
                if let Edit::Set(path, value) = &edit {
                    if path == "theme.colors.accent"
                        && self.draft.item("theme.focus_ring.gradient").is_some()
                    {
                        let gradient = set("theme.focus_ring.gradient.from", value.clone());
                        if self.draft.edit(&gradient).is_ok() {
                            self.pending.push(gradient);
                        }
                    }
                }
                self.pending.push(edit);
                self.error = None;
                self.flush()
            }
            Err(error) => {
                self.error = Some(error);
                Task::none()
            }
        }
    }

    fn flush(&mut self) -> Task<Message> {
        if self.saving || self.pending.is_empty() {
            return Task::none();
        }
        self.pending.clear();
        if self.draft.source == self.current.source {
            return Task::none();
        }
        self.start_save(self.draft.source.clone())
    }

    fn start_save(&mut self, desired: String) -> Task<Message> {
        self.saving = true;
        self.saving_previous = self.current.source.clone();
        let expected = self.current.source.clone();
        let path = self.path.clone();
        cosmic::task::future(async move {
            // Parsing XKB, disk fsync, and IPC must never block native UI events.
            let (send, receive) = cosmic::iced::futures::channel::oneshot::channel();
            std::thread::spawn(move || {
                let result = store::save(&path, &expected, &desired).map(|snapshot| {
                    let live = path == store::config_path() && store::reload_running();
                    (snapshot, live)
                });
                let _ = send.send(result);
            });
            Message::Saved(
                receive
                    .await
                    .unwrap_or_else(|_| Err("Settings worker stopped unexpectedly.".into())),
            )
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cosmic::Application;
    fn app() -> App {
        App::init(
            Core::default(),
            (
                PathBuf::from("/unused/settings-test.kdl"),
                Snapshot::parse(String::new()),
            ),
        )
        .0
    }

    #[test]
    fn notes_save_only_the_latest_revision_and_preserve_multiline_text() {
        let mut app = app();
        app.draft
            .edit(&Edit::Add(
                "desktop_widgets.notes".into(),
                vec![
                    ("id".into(), "test".into()),
                    ("text".into(), "first\nsecond".into()),
                ],
            ))
            .unwrap();
        app.sync_notes();
        let editor = app.note_editors.get_mut("test").unwrap();
        editor.dirty = true;
        editor.revision = 2;
        let _ = app.update(Message::SaveNote("test".into(), 1));
        assert!(!app.saving);
        assert!(app.note_editors["test"].dirty);
        let _ = app.update(Message::SaveNote("test".into(), 2));
        assert!(app.saving);
        assert!(!app.note_editors["test"].dirty);
        assert_eq!(
            app.draft.string("desktop_widgets.notes.0.text", ""),
            "first\nsecond"
        );
    }

    #[test]
    fn external_config_does_not_replace_an_unsaved_note() {
        let mut app = app();
        app.draft
            .edit(&Edit::Add(
                "desktop_widgets.notes".into(),
                vec![("id".into(), "test".into())],
            ))
            .unwrap();
        app.sync_notes();
        app.note_editors.get_mut("test").unwrap().dirty = true;
        let _ = app.update(Message::ExternalConfig(Snapshot::parse(
            "animations {\n    speed 0.5\n}\n".into(),
        )));
        assert!(app.note_editors.contains_key("test"));
        assert!(app.status.contains("externally"));
    }

    #[test]
    fn rapid_edits_are_serialized_and_not_lost_when_a_save_finishes() {
        let mut app = app();
        let _ = app.change(set("animations.speed", 0.75));
        assert!(app.saving);
        let completed = app.draft.clone();
        let _ = app.change(set("layout.inner_gap", 6.));
        assert_eq!(app.pending.len(), 1);
        let _ = app.update(Message::Saved(Ok((completed.clone(), true))));
        assert!(app.saving);
        assert!(app.pending.is_empty());
        assert_eq!(app.saving_previous, completed.source);
        assert_eq!(app.draft.number("layout.inner_gap", 0.), 6.);
        assert_eq!(app.draft.number("animations.speed", 0.), 0.75);
    }

    #[test]
    fn external_config_refreshes_idle_settings_but_preserves_active_edits() {
        let mut app = app();
        let external = Snapshot::parse("animations {\n    speed 0.5\n}\n".into()).unwrap();
        let _ = app.update(Message::ExternalConfig(Ok(external.clone())));
        assert_eq!(app.current.source, external.source);
        assert_eq!(app.draft.number("animations.speed", 0.), 0.5);
        app.inputs
            .insert("theme.colors.accent".into(), "#ffffff".into());
        let newer = Snapshot::parse("animations {\n    speed 0.8\n}\n".into()).unwrap();
        let _ = app.update(Message::ExternalConfig(Ok(newer)));
        assert_eq!(app.current.source, external.source);
        assert_eq!(app.inputs["theme.colors.accent"], "#ffffff");
        assert!(app.status.contains("externally"));
        let _ = app.update(Message::ExternalConfig(Err("invalid KDL".into())));
        assert_eq!(app.current.source, external.source);
        assert!(app.error.unwrap().contains("invalid KDL"));
    }

    #[test]
    fn settings_has_no_native_headerbar() {
        assert!(!app().core.window.show_headerbar);
    }

    #[test]
    fn rejected_save_restores_committed_values_without_claiming_success() {
        let mut app = app();
        let _ = app.change(set("animations.speed", -1.));
        let _ = app.change(set("layout.inner_gap", 6.));
        let _ = app.update(Message::Saved(Err("invalid speed".into())));
        assert!(!app.saving);
        assert!(app.pending.is_empty());
        assert_eq!(app.current.source, app.draft.source);
        assert_eq!(app.status, "Not saved");
        assert_eq!(app.error.as_deref(), Some("invalid speed"));
    }
}
