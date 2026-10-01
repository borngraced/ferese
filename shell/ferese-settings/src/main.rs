mod displays;
mod fonts;
mod schema;
mod store;
mod visuals;
mod watch;

use std::collections::HashMap;
use std::path::PathBuf;

use cosmic::app::{Core, Settings, Task};
use cosmic::iced::{Alignment, Length};
use cosmic::widget::{button, column, container, row, scrollable, slider, text_input};
use cosmic::{ApplicationExt, Element, widget};
use ferese_theme::gallery;
use schema::{Field, Kind, Page};
use store::{Edit, Snapshot, set};

fn main() -> cosmic::iced::Result {
    let mut args = std::env::args_os().skip(1);
    let path = match args.next() {
        Some(arg) if arg == "--config" => PathBuf::from(args.next().expect("--config requires a path")),
        Some(_) => {
            eprintln!("Usage: ferese-settings [--config PATH]");
            return Ok(());
        }
        None => store::config_path(),
    };
    let path = path.canonicalize().unwrap_or(path);
    fonts::families();
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
    RefreshDisplays,
    DisplaysLoaded(Result<Vec<displays::Display>, String>),
    RefreshRate(String, displays::Mode, bool),
    ThemeChanged(ferese_config::theme::Snapshot),
    DragWindow,
    Page(Page),
    Search(String),
    Change(Edit),
    SelectFont(String, String),
    Draft(String, String),
    Commit(Field),
    Range(Field, f64),
    Release(Field),
    Saved(Result<(Snapshot, bool), String>),
    Reload,
    ExternalConfig(Result<Snapshot, String>),
    Undo,
    Family(String, Option<ferese_config::theme::Appearance>),
    SplitThemes(bool),
    AutoAppearance(bool),
    AutoDetails(bool),
    AdvancedTheme(bool),
    ThemeFileTarget(usize),
    PickThemeFile(&'static str),
    ThemeFilePicked(&'static str, Result<Option<String>, String>),
    ImportTheme,
    ThemeImported(Result<Option<(String, String)>, String>),
    ExpireUndo(u64),
    PickWallpaper,
    PreviewLock,
    LockPreviewStarted(Result<(), String>),
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
    displays: Vec<displays::Display>,
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
    resolved: ferese_config::theme::Snapshot,
    undo_revision: u64,
    auto_details: bool,
    advanced_theme: bool,
    theme_file_target: usize,
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
        let resolved = ferese_theme::service::current();
        let font = ferese_theme::font(Some(&resolved.presented.tokens.typography.font_family));
        let native_palette = visuals::Palette::from_resolved(&resolved.presented);
        let mut app = Self {
            displays: vec![],
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
            resolved,
            undo_revision: 0,
            auto_details: false,
            advanced_theme: false,
            theme_file_target: 0,
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
            Message::RefreshDisplays => return self.load_displays(),
            Message::DisplaysLoaded(result) => {
                self.displays = result.unwrap_or_default();
            }
            Message::RefreshRate(prefix, mode, automatic) => {
                self.inputs.remove(&format!("{prefix}.mode"));
                return self.edit_many(displays::edits(&prefix, mode, automatic));
            }
            Message::ThemeChanged(snapshot) => {
                self.resolved = snapshot;
                self.font = ferese_theme::font(Some(&self.resolved.presented.tokens.typography.font_family));
                return self.update_theme();
            }
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
            Message::AddNote if !self.saving && self.draft.records("desktop_widgets.notes") < 32 => {
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
            Message::SelectFont(path, family) => {
                self.inputs.remove(&path);
                return self.change(set(&path, family));
            }
            Message::DragWindow => return self.core.drag(None),
            Message::ExternalConfig(result) => match result {
                Ok(snapshot) if snapshot.source == self.current.source || snapshot.source == self.draft.source => {}
                Ok(snapshot) => {
                    if self.saving
                        || self.note_editors.values().any(|editor| editor.dirty)
                        || !self.inputs.is_empty()
                        || !self.ranges.is_empty()
                        || !self.pending.is_empty()
                    {
                        self.status = "Config changed externally · Reload when your edits are finished".into();
                    } else {
                        self.current = snapshot.clone();
                        self.draft = snapshot;
                        self.sync_notes();
                        self.font = ferese_theme::font(Some(&self.resolved.presented.tokens.typography.font_family));
                        self.undo = None;
                        self.error = None;
                        self.status = "Updated from your config".into();

                        return Task::batch([self.update_theme(), self.load_thumbnail()]);
                    }
                }
                Err(error) => self.error = Some(format!("Config reload failed: {error}")),
            },
            Message::PreviewLock => {
                let path = self.path.clone();
                return Task::perform(
                    async move {
                        let binary = std::env::current_exe()
                            .ok()
                            .and_then(|p| p.parent().map(|p| p.join("ferese-lock")))
                            .filter(|p| p.is_file())
                            .unwrap_or_else(|| "ferese-lock".into());
                        std::process::Command::new(binary)
                            .arg("--preview")
                            .arg("--config")
                            .arg(path)
                            .spawn()
                            .map(|mut child| {
                                std::thread::spawn(move || {
                                    let _ = child.wait();
                                });
                            })
                            .map_err(|error| format!("Could not open the lock preview: {error}"))
                    },
                    |result| cosmic::Action::App(Message::LockPreviewStarted(result)),
                );
            }
            Message::LockPreviewStarted(Err(error)) => {
                self.error = Some(error);
            }
            Message::Page(page) => {
                self.page = page;
                self.search.clear();
                if page == Page::Displays {
                    return self.load_displays();
                }
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
                                args.into_iter().map(serde_json::Value::from).collect::<Vec<_>>(),
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
                        self.font = ferese_theme::font(Some(&self.resolved.presented.tokens.typography.font_family));
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

                        self.undo_revision = self.undo_revision.wrapping_add(1);
                        let revision = self.undo_revision;
                        let expire = cosmic::task::future(async move {
                            tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                            Message::ExpireUndo(revision)
                        });
                        return Task::batch([self.update_theme(), self.flush(), self.load_thumbnail(), expire]);
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
                    self.font = ferese_theme::font(Some(&self.resolved.presented.tokens.typography.font_family));
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
            Message::Family(id, appearance) => {
                let edits = visuals::choose_family(&id, appearance);
                let focus = cosmic::iced::advanced::widget::operate(
                    cosmic::iced::advanced::widget::operation::focusable::focus(gallery_id(&id, appearance)),
                );
                return Task::batch([self.edit_many(edits), focus.map(cosmic::Action::App)]);
            }
            Message::AutoDetails(enabled) => self.auto_details = enabled,
            Message::AdvancedTheme(enabled) => self.advanced_theme = enabled,
            Message::AutoAppearance(enabled) => {
                let mode = if enabled {
                    "auto"
                } else if self.resolved.theme.appearance == ferese_config::theme::Appearance::Light {
                    "light"
                } else {
                    "dark"
                };
                return self.change(set("theme.mode", mode));
            }
            Message::ThemeFileTarget(index) => self.theme_file_target = index.min(2),
            Message::PickThemeFile(path) => {
                return cosmic::task::future(async move {
                    let result = tokio::task::spawn_blocking(|| {
                        store::pick_file("Choose theme overrides", "KDL themes | *.kdl")
                    })
                    .await
                    .unwrap_or_else(|e| Err(e.to_string()));
                    Message::ThemeFilePicked(path, result)
                });
            }
            Message::ThemeFilePicked(path, Ok(Some(file))) => return self.change(set(path, file)),
            Message::ThemeFilePicked(_, Err(error)) => self.error = Some(error),
            Message::SplitThemes(enabled) => {
                return self.edit_many(visuals::toggle_split(
                    &self.draft,
                    enabled,
                    self.resolved.theme.appearance,
                ));
            }
            Message::ImportTheme => {
                let source = self.draft.source.clone();
                let directory = self.path.parent().map(ToOwned::to_owned);
                return cosmic::task::future(async move {
                    let result = tokio::task::spawn_blocking(move || -> Result<_, String> {
                        let Some(path) = store::pick_file("Import theme", "KDL themes | *.kdl")? else {
                            return Ok(None);
                        };
                        let mut snapshot = Snapshot::parse(source)?;
                        let stem = PathBuf::from(&path)
                            .file_stem()
                            .unwrap_or_default()
                            .to_string_lossy()
                            .to_string();
                        let stem: String = stem
                            .to_lowercase()
                            .chars()
                            .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
                            .collect();
                        let mut id = format!("custom-{stem}");
                        let mut suffix = 2;
                        while snapshot.item(&format!("theme.custom_themes.{id}")).is_some() {
                            id = format!("custom-{stem}-{suffix}");
                            suffix += 1;
                        }
                        snapshot.edit(&set(&format!("theme.custom_themes.{id}.file"), path.as_str()))?;
                        let mut connection = ferese_ipc::theme::Connection::connect().map_err(|e| e.to_string())?;
                        let value = connection.call(
                            "theme-preview",
                            serde_json::json!({"source": snapshot.source, "directory": directory}),
                        )?;
                        let _: Vec<ferese_config::families::Family> =
                            serde_json::from_value(value["families"].clone()).map_err(|e| e.to_string())?;
                        Ok(Some((id, path)))
                    })
                    .await
                    .unwrap_or_else(|e| Err(e.to_string()));
                    Message::ThemeImported(result)
                });
            }
            Message::ThemeImported(Ok(Some((id, path)))) => {
                return self.change(set(&format!("theme.custom_themes.{id}.file"), path));
            }
            Message::ThemeImported(Err(error)) => self.error = Some(error),
            Message::ExpireUndo(revision) if revision == self.undo_revision => self.undo = None,
            Message::PickWallpaper => {
                return cosmic::task::future(async {
                    let result = tokio::task::spawn_blocking(|| {
                        store::pick_file(
                            "Choose a wallpaper",
                            "Images | *.png *.jpg *.jpeg *.webp *.PNG *.JPG *.JPEG *.WEBP",
                        )
                    })
                    .await
                    .unwrap_or_else(|e| Err(e.to_string()));
                    Message::WallpaperPicked(result)
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
            Message::AddCommand if !self.saving => match shlex::split(&self.new_command).filter(|a| !a.is_empty()) {
                Some(args) => {
                    self.new_command.clear();
                    return self.change(Edit::Add(
                        "autostart".into(),
                        vec![
                            (
                                "command".into(),
                                args.into_iter().map(serde_json::Value::from).collect::<Vec<_>>().into(),
                            ),
                            ("enabled".into(), true.into()),
                            ("restart".into(), true.into()),
                        ],
                    ));
                }
                None => self.error = Some("Enter a program and arguments with balanced quotes.".into()),
            },
            Message::AddSwipe(keys) if !self.saving => {
                let (action, argument) = match keys {
                    "Swipe3Up" => ("workspace-next", None),
                    "Swipe3Down" => ("workspace-previous", None),
                    "Swipe3Left" => ("focus", Some("right")),
                    _ => ("focus", Some("left")),
                };
                let mut fields = vec![("keys".into(), keys.into()), ("action".into(), action.into())];

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
        cosmic::iced::Subscription::batch([
            cosmic::iced::Subscription::run_with(self.path.clone(), watch::changes),
            ferese_theme::service::subscription().map(Message::ThemeChanged),
            if self.page == Page::Displays {
                cosmic::iced::time::every(std::time::Duration::from_secs(2)).map(|_| Message::RefreshDisplays)
            } else {
                cosmic::iced::Subscription::none()
            },
        ])
    }

    fn view(&self) -> Element<'_, Message> {
        let palette = visuals::Palette::from_resolved(&self.resolved.presented);
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
                .on_press(Message::DragWindow)
                .interaction(cosmic::iced::mouse::Interaction::Grab),
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
        let mut heading = row([]).align_y(Alignment::Center).spacing(20).push(
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
        if self.page == Page::Appearance && self.search.is_empty() {
            let automatic = self.draft.string("theme.mode", "dark") == "auto";
            heading = heading.push(
                row([])
                    .spacing(8)
                    .align_y(Alignment::Center)
                    .push(self.label("Auto", 12.))
                    .push(
                        ferese_theme::controls::switch(automatic, palette)
                            .name(if automatic {
                                "Auto appearance: on"
                            } else {
                                "Auto appearance: off"
                            })
                            .on_press(Message::AutoAppearance(!automatic)),
                    ),
            );
            let scheduled = self.draft.string("theme.schedule.source", "schedule") == "schedule";
            let source = if scheduled { "Schedule" } else { "System" };
            let label = if automatic {
                format!(
                    "{source} · {}",
                    if self.resolved.theme.appearance == ferese_config::theme::Appearance::Light {
                        "Light"
                    } else {
                        "Dark"
                    },
                )
            } else {
                source.to_owned()
            };
            let explanation = if scheduled {
                format!(
                    "Auto uses Light from {} and Dark from {} ({})",
                    self.draft.string("theme.schedule.light_at", "07:00"),
                    self.draft.string("theme.schedule.dark_at", "19:00"),
                    self.draft.string("theme.schedule.timezone", "system"),
                )
            } else {
                "Auto follows the GTK/GNOME appearance preference".to_owned()
            };
            heading = heading.push(widget::tooltip(
                ferese_theme::controls::text_button(label, self.font, palette, self.auto_details)
                    .name("Automatic appearance settings")
                    .on_press(Message::AutoDetails(!self.auto_details)),
                self.label(explanation, 12.),
                widget::tooltip::Position::Bottom,
            ));
        }

        let mut body = column([]).spacing(12);

        if !self.search.is_empty() {
            if !Page::ALL.into_iter().any(|p| p.matches(&self.search)) {
                body = body.push(self.note("No matching settings. Try wallpaper, keyboard, or motion."));
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
                body = body.push(visuals::preview_resolved(&self.draft, &self.resolved.presented));
            }
            if self.page == Page::Appearance {
                let split = visuals::split(&self.draft);
                if split {
                    body = body.push(self.theme_gallery(Some(ferese_config::theme::Appearance::Light)));
                    body = body.push(self.theme_gallery(Some(ferese_config::theme::Appearance::Dark)));
                } else {
                    body = body.push(self.theme_gallery(None));
                }
                body = body.push(
                    widget::checkbox(split)
                        .label("Use different themes for light and dark")
                        .text_size(12)
                        .font(self.font)
                        .on_toggle(Message::SplitThemes),
                );
                if let Some(note) = &self.resolved.fallback_note {
                    body = body.push(self.note(note));
                }
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
                body = body.push(button::standard("Choose image…").on_press(Message::PickWallpaper));
            }
            let fields = schema::fields(self.page);

            if !fields.is_empty() {
                let mut group = column([]).spacing(1);
                if self.page == Page::Appearance && self.auto_details {
                    group = group.push(self.auto_controls());
                }
                for field in fields {
                    if self.page == Page::Appearance
                        && (field.path == "theme.mode"
                            || field.path.starts_with("theme.schedule.")
                            || field.path.ends_with(".file"))
                    {
                        continue;
                    }
                    group = group.push(self.field(field));
                }
                if self.page == Page::Appearance {
                    group = group.push(
                        button::custom(
                            row([])
                                .spacing(8)
                                .align_y(Alignment::Center)
                                .push(self.label("Advanced customization", 13.))
                                .push(visuals::action_icon(
                                    if self.advanced_theme {
                                        "M6 15l6-6 6 6"
                                    } else {
                                        "M6 9l6 6 6-6"
                                    },
                                    palette.muted,
                                )),
                        )
                        .name(if self.advanced_theme {
                            "Collapse advanced customization"
                        } else {
                            "Expand advanced customization"
                        })
                        .padding(12)
                        .width(Length::Fill)
                        .class(visuals::button_style(palette, false))
                        .on_press(Message::AdvancedTheme(!self.advanced_theme)),
                    );
                    if self.advanced_theme {
                        group = group.push(self.theme_file_picker());
                    }
                }
                body = body.push(container(group).padding(8).class(visuals::surface(palette.card, 14.)));
            }

            match self.page {
                Page::LockScreen => {
                    body = body.push(self.note("Uses your wallpaper, shell colors, font and corner radius from Appearance. Changes apply when the locker or preview opens."));
                    body = body.push(button::standard("Preview lock screen").on_press(Message::PreviewLock));
                    body = body.push(self.note("The preview is an ordinary window and does not lock your session. Automatic locking is configured separately in Login items."));
                }
                Page::Desktop => {
                    let can_change_list = !self.saving && !self.note_editors.values().any(|e| e.dirty);
                    body = body.push(self.label("Sticky notes", 16.));
                    body = body.push(self.note("Edit here; notes save after you pause typing. Desktop cards stay behind windows and are click-through."));
                    for index in 0..self.draft.records("desktop_widgets.notes") {
                        let id = self.draft.string(&format!("desktop_widgets.notes.{index}.id"), "note");
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
                                    .on_action(move |action| Message::NoteAction(id.clone(), action)),
                            );
                        }
                        group = group.push(button::destructive("Remove note").on_press_maybe(
                            can_change_list.then_some(Message::Remove("desktop_widgets.notes".into(), index)),
                        ));
                        body = body.push(container(group).padding(12).class(visuals::surface(palette.card, 14.)));
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
                                container(button::destructive("Remove item").on_press_maybe(
                                    (!self.saving).then_some(Message::Remove("autostart".into(), index)),
                                ))
                                .padding(12),
                            );
                        body = body.push(container(group).padding(8).class(visuals::surface(palette.card, 14.)));
                    }
                    body = body.push(
                        row([])
                            .spacing(10)
                            .push(
                                text_input("Program and arguments", self.new_command.clone())
                                    .on_input(Message::NewCommand),
                            )
                            .push(button::standard("Add login item").on_press_maybe(
                                (!self.saving && !self.new_command.trim().is_empty()).then_some(Message::AddCommand),
                            )),
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
                            gestures = gestures.push(
                                button::standard(label)
                                    .on_press_maybe((!self.saving).then_some(Message::AddSwipe(keys))),
                            );
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
                        body = body.push(container(group).padding(8).class(visuals::surface(palette.card, 14.)));
                    }
                    if self.draft.records("bindings") == 0 {
                        body = body.push(self.note("You are using the built-in shortcuts. Add custom bindings in config.kdl; they will appear here after Reload."));
                    }
                }
                Page::Displays => {
                    body = body.push(self.note("Display profiles update connected outputs live. Keep your config open for adding profiles or changing display positions."));
                    for profile in 0..self.draft.records("output_profiles") {
                        let prefix = format!("output_profiles.{profile}");
                        body =
                            body.push(self.label(self.draft.string(&format!("{prefix}.name"), "Display profile"), 17.));

                        for output in 0..self.draft.records(&format!("{prefix}.outputs")) {
                            let prefix = format!("{prefix}.outputs.{output}");
                            let group = column([])
                                .spacing(1)
                                .push(self.refresh_controls(&prefix))
                                .push(self.field(schema::text(
                                    format!("{prefix}.match"),
                                    "Display",
                                    "Output name or matching pattern.",
                                    "",
                                )))
                                .push(self.field(schema::text(
                                    format!("{prefix}.mode"),
                                    "Resolution",
                                    "For example: 2560x1440@60. Leave unchanged to retain automatic selection.",
                                    "",
                                )))
                                .push(self.field(schema::range(
                                    format!("{prefix}.scale"),
                                    "Scale",
                                    "Logical size of text and controls.",
                                    1.,
                                    0.75,
                                    3.,
                                    0.25,
                                    "×",
                                    false,
                                )));
                            body = body.push(container(group).padding(8).class(visuals::surface(palette.card, 14.)));
                        }
                    }
                    if self.draft.records("output_profiles") == 0 {
                        body =
                            body.push(self.note(
                                "No saved profiles. Ferese currently configures connected displays automatically.",
                            ));
                    }
                }
                _ => {}
            }
        }

        let mut content = column([]).spacing(16).push(
            widget::mouse_area(heading.width(Length::Fill))
                .on_press(Message::DragWindow)
                .interaction(cosmic::iced::mouse::Interaction::Grab),
        );

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
                self.label(if self.saving { "Saving…" } else { &self.status }, 11.),
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
                    container(column([]).push(content.height(Length::Fill)).push(footer).spacing(6))
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
        let palette = visuals::Palette::from_resolved(&self.resolved.presented);
        if palette == self.native_palette {
            return Task::none();
        }
        self.native_palette = palette;
        cosmic::command::set_theme(palette.native_theme())
    }

    fn note_index(&self, id: &str) -> Option<usize> {
        (0..self.draft.records("desktop_widgets.notes"))
            .find(|index| self.draft.string(&format!("desktop_widgets.notes.{index}.id"), "note") == id)
    }

    fn sync_notes(&mut self) {
        let mut ids = Vec::new();

        for index in 0..self.draft.records("desktop_widgets.notes") {
            let prefix = format!("desktop_widgets.notes.{index}");
            let id = self.draft.string(&format!("{prefix}.id"), "note");
            let text = self.draft.string(&format!("{prefix}.text"), "");
            let editor = self.note_editors.entry(id.clone()).or_insert_with(|| NoteEditor {
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
        ferese_theme::text(text, self.font).size(size)
    }

    fn note(&self, text: &str) -> Element<'static, Message> {
        let palette = visuals::Palette::from_resolved(&self.resolved.presented);
        self.label(text.to_owned(), 12.)
            .class(cosmic::theme::Text::Color(palette.muted))
            .into()
    }

    fn theme_file_picker(&self) -> Element<'static, Message> {
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

    fn auto_controls(&self) -> Element<'static, Message> {
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

    fn theme_gallery(&self, appearance: Option<ferese_config::theme::Appearance>) -> Element<'static, Message> {
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
                    appearance,
                    active,
                    selected,
                    palette,
                    self.font,
                    tile_id,
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

    fn edit_many(&mut self, edits: Vec<Edit>) -> Task<Message> {
        for edit in edits {
            if let Err(error) = self.draft.edit(&edit) {
                self.error = Some(error);
                return Task::none();
            }
            self.pending.push(edit);
        }
        self.flush()
    }

    fn field(&self, mut field: Field) -> Element<'static, Message> {
        if field.path == "theme.material.opacity" {
            let appearance = self.resolved.theme.appearance;
            let name = match appearance {
                ferese_config::theme::Appearance::Light => "light",
                ferese_config::theme::Appearance::Dark => "dark",
            };
            field.path = format!("theme.{name}.material.opacity");
            field.description = format!("Opacity for the current {name} appearance. Text and icons stay opaque.");
            if let Kind::Range { default, .. } = &mut field.kind {
                *default = self.resolved.theme.tokens.material.opacity;
            }
        }
        let palette = visuals::Palette::from_resolved(&self.resolved.presented);
        let mut labels = column([]).spacing(3).push(self.label(field.label.clone(), 13.));
        if !field.description.is_empty() {
            labels = labels.push(
                self.label(field.description.clone(), 11.)
                    .class(cosmic::theme::Text::Color(palette.muted)),
            );
        }
        let labels = labels.width(Length::Fill);
        let path = field.path.clone();
        let control: Element<'static, Message> = match field.kind.clone() {
            Kind::Font => {
                let value = self
                    .inputs
                    .get(&path)
                    .cloned()
                    .unwrap_or_else(|| self.draft.string(&path, ""));
                let families = fonts::families();
                let selected = if value.is_empty() {
                    Some(0)
                } else {
                    families.iter().position(|family| family == &value)
                };
                let selection_path = path.clone();
                let commit = field.clone();
                column([])
                    .spacing(6)
                    .push(
                        cosmic::iced::widget::pick_list(
                            families,
                            selected.map(|index| families[index].clone()),
                            move |family: String| {
                                let value = if family == "Default font" {
                                    String::new()
                                } else {
                                    family
                                };
                                Message::SelectFont(selection_path.clone(), value)
                            },
                        )
                        .font(self.font)
                        .text_size(12)
                        .width(225),
                    )
                    .push(
                        text_input("Default font", value)
                            .font(self.font)
                            .style(visuals::input_style(palette))
                            .on_input(move |value| Message::Draft(path.clone(), value))
                            .on_submit(move |_| Message::Commit(field.clone()))
                            .on_unfocus(Message::Commit(commit))
                            .width(225)
                            .size(12),
                    )
                    .into()
            }
            Kind::Toggle(default) => {
                let enabled = self.draft.boolean(&path, default);
                ferese_theme::controls::switch(enabled, palette)
                    .name(format!("{}: {}", field.label, if enabled { "on" } else { "off" }))
                    .on_press(Message::Change(set(&path, !enabled)))
                    .into()
            }
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
                        slider(min..=max, value, move |value| Message::Range(field.clone(), value))
                            .step(step)
                            .class(ferese_theme::menus::slider(palette.accent, 1.))
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
                        .push(container(widget::Space::new().width(24).height(24)).class(visuals::surface(swatch, 6.)))
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

    fn load_displays(&self) -> Task<Message> {
        cosmic::task::future(async {
            Message::DisplaysLoaded(
                tokio::task::spawn_blocking(displays::load)
                    .await
                    .unwrap_or_else(|error| Err(error.to_string())),
            )
        })
    }

    fn refresh_controls(&self, prefix: &str) -> Element<'_, Message> {
        let matcher = self.draft.string(&format!("{prefix}.match"), "");
        let configured = self.draft.string(&format!("{prefix}.mode"), "");
        let automatic = self.draft.boolean(&format!("{prefix}.auto_refresh"), false);
        let Some(display) = self
            .displays
            .iter()
            .find(|display| display.connector == matcher || display.identity == matcher)
        else {
            return self.note("Connect this display to choose a supported refresh rate.");
        };
        let modes = displays::choices(display, &configured);
        let mut controls = row([]).spacing(8).align_y(Alignment::Center);
        for mode in &modes {
            let selected = !automatic
                && configured
                    .split('@')
                    .nth(1)
                    .and_then(|rate| rate.parse::<f64>().ok())
                    .is_some_and(|rate| (rate * 1000. - f64::from(mode.refresh)).abs() < 1.);
            controls = controls.push(
                ferese_theme::controls::text_button(
                    mode.label(),
                    self.font,
                    visuals::Palette::from_resolved(&self.resolved.presented),
                    selected,
                )
                .on_press(Message::RefreshRate(prefix.to_owned(), *mode, false)),
            );
        }
        let supports_auto = modes.iter().any(|mode| (59_000..=61_000).contains(&mode.refresh));
        if supports_auto && let Some(mode) = modes.last() {
            controls = controls.push(
                ferese_theme::controls::text_button(
                    "Auto",
                    self.font,
                    visuals::Palette::from_resolved(&self.resolved.presented),
                    automatic,
                )
                .on_press(Message::RefreshRate(prefix.to_owned(), *mode, true)),
            );
        }
        let current = display.current.map(|mode| mode.label()).unwrap_or_else(|| "Off".into());
        let profile = display.profile.as_deref().unwrap_or("automatic configuration");
        column![
            self.label("Refresh rate", 14.),
            self.note(&format!("Current: {current} · active profile: {profile}")),
            controls,
            self.note(if supports_auto {
                "Auto uses 60 Hz below 30% on battery. Normal refresh returns on AC or at 35%. Switching may briefly blank the display. Choosing a rate turns Auto off."
            } else {
                "This resolution has no supported 60 Hz mode for automatic switching."
            }),
        ].spacing(8).into()
    }

    fn change(&mut self, edit: Edit) -> Task<Message> {
        match self.draft.edit(&edit) {
            Ok(()) => {
                // When a gradient exists, changing Accent also changes its
                // leading stop; otherwise the visible focus border would stay
                // on the old palette despite the control saying it changed.
                if let Edit::Set(path, value) = &edit
                    && path == "theme.colors.accent"
                    && self.draft.item("theme.focus_ring.gradient").is_some()
                {
                    let gradient = set("theme.focus_ring.gradient.from", value.clone());
                    if self.draft.edit(&gradient).is_ok() {
                        self.pending.push(gradient);
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

fn gallery_id(id: &str, appearance: Option<ferese_config::theme::Appearance>) -> cosmic::iced::advanced::widget::Id {
    cosmic::iced::advanced::widget::Id::new(format!("theme-{appearance:?}-{id}"))
}

#[cfg(test)]
mod tests {
    use cosmic::Application;

    use super::*;
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
    fn disabling_auto_keeps_the_effective_appearance() {
        for appearance in [
            ferese_config::theme::Appearance::Light,
            ferese_config::theme::Appearance::Dark,
        ] {
            let mut app = app();
            app.resolved.theme.appearance = appearance;
            let _ = app.update(Message::AutoAppearance(false));
            assert_eq!(
                app.draft.string("theme.mode", ""),
                if appearance == ferese_config::theme::Appearance::Light {
                    "light"
                } else {
                    "dark"
                }
            );
            let _ = app.update(Message::AutoAppearance(true));
            assert_eq!(app.draft.string("theme.mode", ""), "auto");
        }
    }

    #[test]
    fn file_picker_applies_to_the_scope_it_was_opened_for_and_cancel_keeps_it() {
        let mut app = app();
        let _ = app.update(Message::ThemeFileTarget(2));
        let _ = app.update(Message::ThemeFilePicked(
            "theme.light.file",
            Ok(Some("/tmp/light.kdl".into())),
        ));
        assert_eq!(app.draft.string("theme.light.file", ""), "/tmp/light.kdl");
        assert!(app.draft.item("theme.dark.file").is_none());
        let _ = app.update(Message::ThemeFilePicked("theme.light.file", Ok(None)));
        assert_eq!(app.draft.string("theme.light.file", ""), "/tmp/light.kdl");
    }

    #[test]
    fn notes_save_only_the_latest_revision_and_preserve_multiline_text() {
        let mut app = app();
        app.draft
            .edit(&Edit::Add(
                "desktop_widgets.notes".into(),
                vec![("id".into(), "test".into()), ("text".into(), "first\nsecond".into())],
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
        assert_eq!(app.draft.string("desktop_widgets.notes.0.text", ""), "first\nsecond");
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
        app.inputs.insert("theme.colors.accent".into(), "#ffffff".into());
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
