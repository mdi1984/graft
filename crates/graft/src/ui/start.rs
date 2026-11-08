//! The start page ("New tab"): open a local repository, clone one, create one,
//! reopen a recent one, or open all repositories of a tab group.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use egui::{
    Align, Align2, Color32, Layout, Rect, ScrollArea, Sense, Stroke, StrokeKind, Ui, pos2, vec2,
};
use graft_core::paths::{self, native};
use graft_core::repo::clone_folder_name;
use graft_core::wsl::Wsl;

use super::confirm;
use super::icons::{self, Icon};
use super::widgets::{
    ButtonKind, button, elided, elided_path, error_label, field_label, hint, paint_galley_left,
    paint_icon_button, paint_placeholder_left, progress_bar, section_heading, small_text,
    text_field,
};
use crate::groups::{GroupId, TabGroup};
use crate::keys::{Command, Keymap};
use crate::running::Running;
use crate::text::plural;
use crate::theme::{self, RADIUS, color};

const WIDTH: f32 = 600.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Home,
    Clone,
    Init,
}

/// Where Open starts looking and where Clone and Create put repositories.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Place {
    Windows,
    /// A WSL distro; its folders are typed as Linux paths (`~/projects`).
    Wsl(String),
}

/// A distro's home folder, read in the background (that starts the distro).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Home {
    Loading,
    Known(String),
    Failed,
}

fn place_of(path: &Path) -> Place {
    match Wsl::of(path) {
        Some(wsl) => Place::Wsl(wsl.distro().to_owned()),
        None => Place::Windows,
    }
}

pub struct StartPage {
    mode: Mode,
    url: String,
    clone_parent: String,
    folder_name: String,
    /// The user typed a folder name, so stop deriving it from the URL.
    folder_name_edited: bool,
    init_path: String,
    /// A clone or init in progress.
    busy: Option<Running>,
    error: Option<String>,
    place: Place,
    distros: Distros,
    homes: HashMap<String, Home>,
    /// The clone folder and new repository folder typed for other places.
    other_fields: HashMap<Place, (String, String)>,
    /// The clone folder Graft started with, kept while the field does not
    /// resolve (e.g. `~` before the home folder is known).
    saved_clone_parent: PathBuf,
    /// The group whose Delete waits for the user's OK, with its name.
    delete_group: Option<(GroupId, String)>,
}

/// The installed WSL distros, listed in the background once.
enum Distros {
    NotAsked,
    Asked,
    Known(Vec<String>),
}

/// A recently opened repository. `exists` is checked in the background
/// (see [`Recent::is_there`]) when the list is shown, not every frame.
pub struct Recent {
    pub path: PathBuf,
    pub exists: bool,
}

impl Recent {
    /// Assumed to exist until it was checked.
    pub fn new(path: PathBuf) -> Self {
        Self { path, exists: true }
    }

    /// Whether the folder at `path` is still there. Reads the disk.
    pub fn is_there(path: &Path) -> bool {
        // Looking into WSL would start a stopped distro, and this runs when
        // Graft starts: assume it is there; opening says if it is not.
        graft_core::wsl::Wsl::of(path).is_some() || path.is_dir()
    }
}

/// What the user asked for; the app carries it out.
pub enum StartAction {
    PickOpen,
    /// List the installed WSL distros; answer with [`StartPage::set_distros`].
    LoadDistros,
    /// Read a distro's home folder; answer with [`StartPage::set_home`].
    LoadHome(String),
    PickCloneParent,
    PickInitFolder,
    Clone {
        url: String,
        dest: PathBuf,
    },
    Init(PathBuf),
    Cancel,
    OpenRecent(PathBuf),
    ForgetRecent(PathBuf),
    OpenGroup(GroupId),
    RenameGroup(GroupId),
    DeleteGroup(GroupId),
}

impl StartPage {
    pub fn new(clone_parent: PathBuf) -> Self {
        let mut page = Self {
            mode: Mode::Home,
            url: String::new(),
            clone_parent: String::new(),
            folder_name: String::new(),
            folder_name_edited: false,
            init_path: String::new(),
            busy: None,
            error: None,
            place: place_of(&clone_parent),
            distros: Distros::NotAsked,
            homes: HashMap::new(),
            other_fields: HashMap::new(),
            saved_clone_parent: clone_parent.clone(),
            delete_group: None,
        };
        page.clone_parent = page.show_path(&clone_parent);
        page
    }

    fn home(&self, distro: &str) -> Option<&str> {
        match self.homes.get(distro) {
            Some(Home::Known(home)) => Some(home),
            _ => None,
        }
    }

    /// The distro of the current place, if it is one, and its home folder.
    fn wsl(&self) -> Option<(Wsl, Option<&str>)> {
        match &self.place {
            Place::Windows => None,
            Place::Wsl(name) => Some((Wsl::new(name), self.home(name))),
        }
    }

    /// A folder the user typed, as a Windows path. `None` while a `~` path
    /// waits for the home folder, or for a relative Linux path.
    fn resolve(&self, text: &str) -> Option<PathBuf> {
        let text = text.trim();
        if text.is_empty() {
            return None;
        }
        match self.wsl() {
            None => Some(native(text)),
            Some((wsl, home)) => wsl.resolve(text, home),
        }
    }

    /// `path` the way the user types it: a Linux path for one inside WSL.
    pub fn show_path(&self, path: &Path) -> String {
        Wsl::of(path)
            .and_then(|wsl| wsl.show(path, self.home(wsl.distro())))
            .unwrap_or_else(|| path.display().to_string())
    }

    /// Like [`Self::show_path`], plus the distro for a path inside WSL.
    fn describe(&self, path: &Path) -> String {
        match Wsl::of(path) {
            Some(wsl) => format!("{} · {}", self.show_path(path), wsl.distro()),
            None => path.display().to_string(),
        }
    }

    /// Switch to `place`, keeping what was typed for the one left.
    fn set_place(&mut self, place: Place) {
        if place == self.place {
            return;
        }
        let fields = (
            std::mem::take(&mut self.clone_parent),
            std::mem::take(&mut self.init_path),
        );
        let old = std::mem::replace(&mut self.place, place.clone());
        self.other_fields.insert(old, fields);
        let (clone_parent, init_path) =
            self.other_fields
                .remove(&place)
                .unwrap_or_else(|| match place {
                    Place::Windows => (
                        self.show_path(&crate::settings::default_clone_parent()),
                        String::new(),
                    ),
                    Place::Wsl(_) => ("~".to_owned(), String::new()),
                });
        self.clone_parent = clone_parent;
        self.init_path = init_path;
        self.error = None;
    }

    /// A folder picked in the dialog; switches to the place it is in.
    pub fn picked_clone_parent(&mut self, path: &Path) {
        self.set_place(place_of(path));
        self.clone_parent = self.show_path(path);
    }

    pub fn picked_init_folder(&mut self, path: &Path) {
        self.set_place(place_of(path));
        self.init_path = self.show_path(path);
    }

    /// The installed distros arrived; a remembered one that is gone falls
    /// back to Windows.
    pub fn set_distros(&mut self, distros: Vec<String>) {
        if let Place::Wsl(name) = &self.place {
            // Distro names are case-insensitive; use the listed spelling.
            match distros.iter().find(|d| d.eq_ignore_ascii_case(name)) {
                Some(listed) if listed != name => self.place = Place::Wsl(listed.clone()),
                Some(_) => {}
                None => self.set_place(Place::Windows),
            }
        }
        self.distros = Distros::Known(distros);
    }

    /// A home folder arrived: show folders under it with `~`.
    pub fn set_home(&mut self, distro: String, home: Option<String>) {
        let parent = self.resolve(&self.clone_parent);
        let home = home.map_or(Home::Failed, Home::Known);
        self.homes.insert(distro, home);
        if let Some(parent) = parent {
            self.clone_parent = self.show_path(&parent);
        }
    }

    /// The clone folder to remember for next time.
    pub fn clone_parent_path(&self) -> PathBuf {
        self.resolve(&self.clone_parent)
            .unwrap_or_else(|| self.saved_clone_parent.clone())
    }

    /// Where the Open dialog starts: the home folder in WSL.
    pub fn open_start(&self) -> Option<PathBuf> {
        let (wsl, home) = self.wsl()?;
        Some(wsl.to_windows(home.unwrap_or("/")))
    }

    /// Where the Browse dialog of the clone form starts.
    pub fn clone_parent_start(&self) -> Option<PathBuf> {
        self.resolve(&self.clone_parent)
            .or_else(|| self.open_start())
    }

    /// Where the Browse dialog of the create form starts.
    pub fn init_start(&self) -> Option<PathBuf> {
        self.resolve(&self.init_path).or_else(|| self.open_start())
    }

    /// Why a typed folder does not resolve, if it doesn't.
    fn folder_problem(&self, text: &str) -> Option<String> {
        let (wsl, _) = self.wsl()?;
        let text = text.trim();
        if text.is_empty() || self.resolve(text).is_some() {
            return None;
        }
        let in_home = text == "~" || text.starts_with("~/");
        Some(match self.homes.get(wsl.distro()) {
            Some(Home::Failed) if in_home => format!(
                "Could not read your home folder in {}. Use a full path like /home/you/projects.",
                wsl.distro()
            ),
            Some(Home::Loading) | None if in_home => format!("Starting {}…", wsl.distro()),
            _ => "Use a Linux path, like ~/projects or /home/you/projects.".to_owned(),
        })
    }

    /// Background work the page needs before it can be used: the distro
    /// list, then the home folder of the chosen distro. Asked for apart from
    /// the page's own actions, so a click in the same frame cannot drop it.
    pub fn take_load(&mut self) -> Option<StartAction> {
        if let Distros::NotAsked = self.distros {
            self.distros = Distros::Asked;
            return Some(StartAction::LoadDistros);
        }
        if let Place::Wsl(name) = &self.place
            && !self.homes.contains_key(name)
        {
            let name = name.clone();
            self.homes.insert(name.clone(), Home::Loading);
            return Some(StartAction::LoadHome(name));
        }
        None
    }

    pub fn is_busy(&self) -> bool {
        self.busy.is_some()
    }

    /// A clone or init started.
    pub fn begin(&mut self, running: Running) {
        self.error = None;
        self.busy = Some(running);
    }

    /// The clone or init ended without a repository: cancelled, or with
    /// `error` to show.
    pub fn stop(&mut self, error: Option<String>) {
        self.busy = None;
        self.error = error;
    }

    pub fn cancel(&self) {
        if let Some(busy) = &self.busy {
            busy.cancel();
        }
    }

    pub fn busy_status(&self) -> Option<String> {
        self.busy.as_ref().map(Running::status)
    }

    /// Forget the forms after a repository was opened.
    pub fn reset(&mut self) {
        self.mode = Mode::Home;
        self.url.clear();
        self.folder_name.clear();
        self.folder_name_edited = false;
        self.init_path.clear();
        self.busy = None;
        self.error = None;
    }

    pub fn clone_dest(&self) -> Option<PathBuf> {
        Some(
            self.resolve(&self.clone_parent)?
                .join(self.folder_name.trim()),
        )
    }

    pub fn show(
        &mut self,
        ui: &mut Ui,
        recent: &[Recent],
        groups: &[TabGroup],
        keys: &Keymap,
    ) -> Option<StartAction> {
        let mut action = None;
        ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                let avail = ui.available_width();
                let width = WIDTH.min(avail - 32.0);
                let margin = ((avail - width) / 2.0).max(16.0);
                ui.add_space(48.0);
                ui.horizontal(|ui| {
                    ui.add_space(margin);
                    ui.vertical(|ui| {
                        ui.set_width(width);
                        ui.spacing_mut().item_spacing.y = 8.0;
                        self.header(ui);
                        ui.add_space(20.0);
                        self.places(ui);
                        self.cards(ui, width, &mut action);
                        ui.add_space(12.0);
                        match self.mode {
                            Mode::Home => {}
                            Mode::Clone => self.clone_form(ui, width, &mut action),
                            Mode::Init => self.init_form(ui, &mut action),
                        }
                        if let Some(busy) = &self.busy {
                            busy_panel(ui, busy, &mut action);
                        }
                        if let Some(error) = &self.error {
                            ui.add_space(4.0);
                            error_label(ui, error);
                        }
                        ui.add_space(24.0);
                        if !groups.is_empty() {
                            let ask = &mut self.delete_group;
                            group_list(ui, groups, width, ask, &mut action);
                            ui.add_space(16.0);
                        }
                        self.recent_list(ui, recent, width, &mut action);
                        ui.add_space(16.0);
                        hint(ui, &tip(keys));
                    });
                });
            });
        if let Some((id, name)) = &self.delete_group {
            let body = "Only the group goes away: its repositories are not touched, and \
                        open tabs stay open.";
            let title = format!("Delete group {name}?");
            match confirm::ask(ui.ctx(), &title, body, "Delete group") {
                Some(true) => {
                    action = Some(StartAction::DeleteGroup(*id));
                    self.delete_group = None;
                }
                Some(false) => self.delete_group = None,
                None => {}
            }
        }
        action
    }

    fn header(&mut self, ui: &mut Ui) {
        let logo = icons::app_logo(ui.ctx());
        ui.horizontal(|ui| {
            ui.add(egui::Image::new(&logo).fit_to_exact_size(vec2(52.0, 52.0)));
            ui.add_space(8.0);
            ui.vertical(|ui| {
                ui.add_space(4.0);
                ui.label(
                    egui::RichText::new("Graft")
                        .font(theme::semibold(24.0))
                        .color(color::text()),
                );
                ui.label(
                    egui::RichText::new("Open a repository to get started.")
                        .color(color::text_weak()),
                );
            });
        });
    }

    /// "Location: This PC | WSL distros", only when WSL has distros.
    fn places(&mut self, ui: &mut Ui) {
        let distros = match &self.distros {
            Distros::Known(distros) if !distros.is_empty() => distros.clone(),
            _ => return,
        };
        let idle = self.busy.is_none();
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            ui.label(
                egui::RichText::new("Location")
                    .color(color::text_weak())
                    .size(theme::SMALL_SIZE),
            );
            ui.add_space(4.0);
            let mut choice = None;
            let windows = place_button(ui, "This PC", self.place == Place::Windows)
                .on_hover_text("Folders on Windows");
            if windows.clicked() {
                choice = Some(Place::Windows);
            }
            for name in distros {
                let place = Place::Wsl(name.clone());
                let response = place_button(ui, &format!("WSL · {name}"), self.place == place)
                    .on_hover_text(format!(
                        "Folders inside {name}; git runs there too. Type Linux paths like ~/projects."
                    ));
                if response.clicked() {
                    choice = Some(place);
                }
            }
            if let Some(place) = choice
                && idle
            {
                self.set_place(place);
            }
        });
        ui.add_space(8.0);
    }

    fn cards(&mut self, ui: &mut Ui, width: f32, action: &mut Option<StartAction>) {
        let gap = 12.0;
        let card_width = (width - 2.0 * gap) / 3.0;
        let idle = self.busy.is_none();
        let open_detail = match &self.place {
            Place::Windows => "A folder with a git repository".to_owned(),
            Place::Wsl(_) => "A git repository in WSL".to_owned(),
        };
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = gap;
            if card(ui, card_width, Icon::Folder, "Open", &open_detail, false).clicked() && idle {
                self.error = None;
                *action = Some(StartAction::PickOpen);
            }
            if card(
                ui,
                card_width,
                Icon::Download,
                "Clone",
                "Copy a repository from a URL",
                self.mode == Mode::Clone,
            )
            .clicked()
                && idle
            {
                self.error = None;
                self.mode = Mode::Clone;
            }
            if card(
                ui,
                card_width,
                Icon::FolderPlus,
                "Create new",
                "Start an empty repository",
                self.mode == Mode::Init,
            )
            .clicked()
                && idle
            {
                self.error = None;
                self.mode = Mode::Init;
            }
        });
    }

    fn clone_form(&mut self, ui: &mut Ui, width: f32, action: &mut Option<StartAction>) {
        form_frame(ui, |ui| {
            let inner = width - 32.0;
            field_label(ui, "Repository URL");
            let url = text_field(
                ui,
                &mut self.url,
                "https://github.com/owner/repo.git",
                inner,
            );
            if url.changed() {
                self.error = None;
            }
            if url.changed() && !self.folder_name_edited {
                self.folder_name = if self.url.trim().is_empty() {
                    String::new()
                } else {
                    clone_folder_name(&self.url)
                };
            }
            ui.add_space(4.0);
            field_label(ui, "Clone into folder");
            let hint = match self.place {
                Place::Windows => r"C:\dev",
                Place::Wsl(_) => "~/projects",
            };
            let (browse, edited) =
                browse_row(ui, &mut self.clone_parent, hint, self.busy.is_none());
            if edited {
                self.error = None;
            }
            if browse {
                *action = Some(StartAction::PickCloneParent);
            }
            ui.add_space(4.0);
            field_label(ui, "Folder name");
            if text_field(ui, &mut self.folder_name, "repo", inner).changed() {
                self.folder_name_edited = true;
                self.error = None;
            }
            let dest = self.clone_dest();
            let ready = !self.url.trim().is_empty()
                && dest.is_some()
                && !self.folder_name.trim().is_empty()
                && self.busy.is_none();
            if let Some(problem) = self.folder_problem(&self.clone_parent) {
                small_text(ui, &problem, color::text_weak());
            } else if let Some(dest) = dest.as_deref().filter(|_| ready) {
                let (rect, _) = ui.allocate_exact_size(vec2(inner, 16.0), Sense::hover());
                let prefix = ui.painter().layout_no_wrap(
                    "Will clone to ".to_owned(),
                    theme::small(),
                    color::text_faint(),
                );
                let prefix_width = prefix.size().x;
                let y = rect.center().y;
                paint_galley_left(ui.painter(), rect.left(), y, prefix);
                let dest = self.describe(dest);
                let dest = elided_path(
                    ui,
                    &dest,
                    theme::small(),
                    color::text_weak(),
                    inner - prefix_width,
                );
                paint_galley_left(ui.painter(), rect.left() + prefix_width, y, dest);
            }
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                let submit = url.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                if (button(ui, "Clone", ButtonKind::Primary, ready).clicked() || submit)
                    && ready
                    && let Some(dest) = dest
                {
                    self.error = None;
                    *action = Some(StartAction::Clone {
                        url: self.url.trim().to_owned(),
                        dest,
                    });
                }
                if button(ui, "Cancel", ButtonKind::Secondary, self.busy.is_none()).clicked() {
                    self.mode = Mode::Home;
                    self.error = None;
                }
            });
        });
    }

    fn init_form(&mut self, ui: &mut Ui, action: &mut Option<StartAction>) {
        form_frame(ui, |ui| {
            field_label(ui, "New repository folder (created if missing)");
            let hint = match self.place {
                Place::Windows => r"C:\dev\my-project",
                Place::Wsl(_) => "~/projects/my-project",
            };
            let (browse, _) = browse_row(ui, &mut self.init_path, hint, self.busy.is_none());
            if browse {
                *action = Some(StartAction::PickInitFolder);
            }
            if let Some(problem) = self.folder_problem(&self.init_path) {
                small_text(ui, &problem, color::text_weak());
            }
            ui.add_space(6.0);
            let target = self.resolve(&self.init_path);
            let ready = target.is_some() && self.busy.is_none();
            ui.horizontal(|ui| {
                if button(ui, "Create repository", ButtonKind::Primary, ready).clicked()
                    && let Some(target) = target
                {
                    self.error = None;
                    *action = Some(StartAction::Init(target));
                }
                if button(ui, "Cancel", ButtonKind::Secondary, self.busy.is_none()).clicked() {
                    self.mode = Mode::Home;
                    self.error = None;
                }
            });
        });
    }
}

/// A folder field with "Browse…" at its right. Returns whether Browse was
/// clicked and whether the field changed.
fn browse_row(ui: &mut Ui, text: &mut String, hint: &str, enabled: bool) -> (bool, bool) {
    ui.horizontal(|ui| {
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            let browse = button(ui, "Browse…", ButtonKind::Secondary, enabled).clicked();
            let width = ui.available_width();
            (browse, text_field(ui, text, hint, width).changed())
        })
        .inner
    })
    .inner
}

fn form_frame(ui: &mut Ui, add: impl FnOnce(&mut Ui)) {
    egui::Frame::new()
        .fill(color::sidebar_bg())
        .stroke(Stroke::new(1.0, color::border()))
        .corner_radius(8)
        .inner_margin(egui::Margin::same(16))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = 6.0;
            add(ui);
        });
}

fn card(
    ui: &mut Ui,
    width: f32,
    icon: Icon,
    title: &str,
    detail: &str,
    active: bool,
) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(vec2(width, 92.0), Sense::click());
    let painter = ui.painter();
    let (fill, border) = if active {
        (color::selected_row_bg(), color::selected_outline())
    } else if response.hovered() {
        (color::control_hover_bg(), color::control_border())
    } else {
        (color::control_bg(), color::control_border())
    };
    painter.rect(rect, 8, fill, Stroke::new(1.0, border), StrokeKind::Inside);
    let icon_rect = Rect::from_min_size(rect.min + vec2(14.0, 14.0), vec2(20.0, 20.0));
    icons::paint(painter, icon_rect, icon, color::accent());
    painter.text(
        pos2(rect.left() + 14.0, rect.top() + 52.0),
        Align2::LEFT_CENTER,
        title,
        theme::semibold(theme::BODY_SIZE),
        color::text(),
    );
    let detail = elided(
        ui,
        detail,
        theme::small(),
        color::text_weak(),
        rect.width() - 28.0,
    );
    paint_galley_left(painter, rect.left() + 14.0, rect.top() + 72.0, detail);
    response
}

/// The running clone or init: what it is doing, a progress bar and Cancel.
/// Each progress report from git wakes the UI, so this needs no polling.
fn busy_panel(ui: &mut Ui, busy: &Running, action: &mut Option<StartAction>) {
    ui.add_space(6.0);
    ui.label(egui::RichText::new(busy.status()).color(color::text()));
    ui.horizontal(|ui| {
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if busy.can_cancel() && button(ui, "Cancel", ButtonKind::Secondary, true).clicked() {
                *action = Some(StartAction::Cancel);
            }
            let width = ui.available_width();
            progress_bar(ui, busy.progress().fraction, width);
        });
    });
}

/// A toggle in the Location row.
fn place_button(ui: &mut Ui, label: &str, selected: bool) -> egui::Response {
    let galley =
        ui.painter()
            .layout_no_wrap(label.to_owned(), theme::small(), Color32::PLACEHOLDER);
    let size = vec2(galley.size().x + 20.0, 24.0);
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
    let (fill, border, fg) = if selected {
        (
            color::selected_row_bg(),
            color::selected_outline(),
            color::text(),
        )
    } else if response.hovered() {
        (
            color::control_hover_bg(),
            color::control_border(),
            color::text(),
        )
    } else {
        (
            color::control_bg(),
            color::control_border(),
            color::text_weak(),
        )
    };
    ui.painter()
        .rect(rect, 6, fill, Stroke::new(1.0, border), StrokeKind::Inside);
    paint_placeholder_left(
        ui.painter(),
        rect.left() + 10.0,
        rect.center().y,
        galley,
        fg,
    );
    response
}

impl StartPage {
    fn recent_list(
        &self,
        ui: &mut Ui,
        recent: &[Recent],
        width: f32,
        action: &mut Option<StartAction>,
    ) {
        let (rect, _) = ui.allocate_exact_size(vec2(width, 20.0), Sense::hover());
        section_heading(ui, ui.painter(), rect.left(), rect.center().y, "RECENT");
        if recent.is_empty() {
            ui.label(
                egui::RichText::new("Repositories you open show up here.")
                    .color(color::text_faint()),
            );
            return;
        }
        for Recent { path, exists } in recent {
            let exists = *exists;
            let (rect, response) = ui.allocate_exact_size(vec2(width, 44.0), Sense::click());
            let close = Rect::from_center_size(
                pos2(rect.right() - 18.0, rect.center().y),
                vec2(22.0, 22.0),
            );
            let close_response =
                ui.interact(close, ui.scope_id().with(("forget", path)), Sense::click());
            let hovered = response.hovered() || close_response.hovered();
            let painter = ui.painter();
            if hovered {
                painter.rect_filled(rect, RADIUS, color::hover_row_bg());
            }
            let name = paths::folder_name(path);
            let fg = if exists {
                color::text()
            } else {
                color::text_faint()
            };
            let name = elided(
                ui,
                &name,
                theme::semibold(theme::BODY_SIZE),
                fg,
                width - 60.0,
            );
            paint_galley_left(painter, rect.left() + 10.0, rect.top() + 14.0, name);
            let detail = if exists {
                self.describe(path)
            } else {
                format!("{} (missing)", self.describe(path))
            };
            let detail = elided_path(
                ui,
                &detail,
                theme::small(),
                color::text_weak(),
                width - 60.0,
            );
            paint_galley_left(painter, rect.left() + 10.0, rect.top() + 31.0, detail);
            if hovered {
                paint_icon_button(ui, close, &close_response, Icon::Close);
            }
            if close_response.on_hover_text("Remove from recent").clicked() {
                *action = Some(StartAction::ForgetRecent(path.clone()));
            } else if response.clicked() && exists {
                *action = Some(StartAction::OpenRecent(path.clone()));
            }
        }
    }
}

/// Tab groups: click one to open all of its repositories.
/// "Delete group…" puts the group into `ask`, to confirm first.
fn group_list(
    ui: &mut Ui,
    groups: &[TabGroup],
    width: f32,
    ask: &mut Option<(GroupId, String)>,
    action: &mut Option<StartAction>,
) {
    let (rect, _) = ui.allocate_exact_size(vec2(width, 20.0), Sense::hover());
    section_heading(ui, ui.painter(), rect.left(), rect.center().y, "GROUPS");
    for group in groups {
        let (rect, response) = ui.allocate_exact_size(vec2(width, 44.0), Sense::click());
        let response = response.on_hover_text("Open all of its repositories");
        let painter = ui.painter();
        if response.hovered() {
            painter.rect_filled(rect, RADIUS, color::hover_row_bg());
        }
        let fg = color::group(group.color);
        painter.circle_filled(pos2(rect.left() + 14.0, rect.top() + 14.0), 4.0, fg);
        let count = match group.repos.len() {
            0 => "no repositories yet".to_owned(),
            n => plural(n, "repository", "repositories"),
        };
        let name = elided(
            ui,
            &group.name,
            theme::semibold(theme::BODY_SIZE),
            color::text(),
            width - 200.0,
        );
        let name_right = rect.left() + 26.0 + name.size().x;
        paint_galley_left(painter, rect.left() + 26.0, rect.top() + 14.0, name);
        let count = elided(ui, &count, theme::small(), color::text_weak(), 160.0);
        paint_galley_left(painter, name_right + 8.0, rect.top() + 14.0, count);
        let names: Vec<String> = group.repos.iter().map(|p| paths::folder_name(p)).collect();
        let names = elided(
            ui,
            &names.join(" · "),
            theme::small(),
            color::text_weak(),
            width - 120.0,
        );
        paint_galley_left(painter, rect.left() + 26.0, rect.top() + 31.0, names);
        if response.hovered() {
            painter.text(
                pos2(rect.right() - 12.0, rect.center().y),
                Align2::RIGHT_CENTER,
                "Open all",
                theme::semibold(theme::BODY_SIZE),
                color::accent(),
            );
        }
        if response.clicked() {
            *action = Some(StartAction::OpenGroup(group.id));
        }
        response.context_menu(|ui| {
            if ui.button("Open all").clicked() {
                *action = Some(StartAction::OpenGroup(group.id));
                ui.close();
            }
            if ui.button("Rename…").clicked() {
                *action = Some(StartAction::RenameGroup(group.id));
                ui.close();
            }
            ui.separator();
            if ui.button("Delete group…").clicked() {
                *ask = Some((group.id, group.name.clone()));
                ui.close();
            }
        });
    }
}

/// The tip under the lists: drop a folder to open it, plus the keys that
/// open a folder and this page, as they are set.
fn tip(keys: &Keymap) -> String {
    let mut tip = "Tip: drop a folder onto this window to open it.".to_owned();
    if let Some(key) = keys.text(Command::OpenFolder) {
        tip += &format!(" {key} opens");
    }
    if let Some(key) = keys.text(Command::NewTab) {
        let and = if tip.ends_with("opens") { "," } else { "" };
        tip += &format!("{and} {key} shows this page");
    }
    if !tip.ends_with('.') {
        tip.push('.');
    }
    tip
}

#[cfg(test)]
mod tests {
    use super::*;

    const PROJECTS: &str = r"\\wsl.localhost\Ubuntu\home\mdi\projects";

    #[test]
    fn starts_where_the_last_clone_went() {
        let mut page = StartPage::new(PathBuf::from(PROJECTS));
        assert_eq!(page.place, Place::Wsl("Ubuntu".into()));
        // Before the home folder is known the path is shown in full.
        assert_eq!(page.clone_parent, "/home/mdi/projects");
        assert!(matches!(page.take_load(), Some(StartAction::LoadDistros)));
        page.set_distros(vec!["ubuntu".into()]);
        assert_eq!(page.place, Place::Wsl("ubuntu".into()), "listed spelling");
        assert!(matches!(page.take_load(), Some(StartAction::LoadHome(d)) if d == "ubuntu"));
        assert!(page.take_load().is_none(), "asked once");
        page.set_home("ubuntu".into(), Some("/home/mdi".into()));
        assert_eq!(page.clone_parent, "~/projects");
        assert_eq!(
            page.clone_dest().map(|_| page.clone_parent_path()),
            Some(PathBuf::from(r"\\wsl.localhost\ubuntu\home\mdi\projects"))
        );
    }

    #[test]
    fn a_missing_distro_falls_back_to_windows() {
        let mut page = StartPage::new(PathBuf::from(PROJECTS));
        page.set_distros(vec!["Debian".into()]);
        assert_eq!(page.place, Place::Windows);
        assert!(!page.clone_parent.starts_with('/'));
    }

    #[test]
    fn each_place_keeps_its_folders() {
        let mut page = StartPage::new(PathBuf::from(r"C:\dev"));
        page.set_distros(vec!["Ubuntu".into()]);
        page.init_path = r"C:\dev\new".into();
        page.set_place(Place::Wsl("Ubuntu".into()));
        assert_eq!(
            (page.clone_parent.as_str(), page.init_path.as_str()),
            ("~", "")
        );
        page.clone_parent = "~/src".into();
        page.set_place(Place::Windows);
        assert_eq!(page.clone_parent, r"C:\dev");
        assert_eq!(page.init_path, r"C:\dev\new");
        page.set_place(Place::Wsl("Ubuntu".into()));
        assert_eq!(page.clone_parent, "~/src");
        // Picking a Windows folder switches back.
        page.picked_init_folder(Path::new(r"D:\x"));
        assert_eq!(page.place, Place::Windows);
        assert_eq!(page.init_path, r"D:\x");
    }

    #[test]
    fn explains_folders_that_do_not_work_yet() {
        let mut page = StartPage::new(PathBuf::from(PROJECTS));
        page.set_distros(vec!["Ubuntu".into()]);
        let _ = page.take_load();
        let problem = |page: &StartPage, text: &str| page.folder_problem(text).unwrap_or_default();
        assert_eq!(problem(&page, "~/x"), "Starting Ubuntu…");
        assert!(problem(&page, "~bob/x").starts_with("Use a Linux path"));
        assert!(problem(&page, "projects").starts_with("Use a Linux path"));
        page.set_home("Ubuntu".into(), None);
        assert!(problem(&page, "~/x").starts_with("Could not read your home folder"));
        assert_eq!(problem(&page, "/srv/x"), "");
        // While a stopped repo's place is unknown, the saved folder is kept.
        assert_eq!(page.clone_parent_path(), PathBuf::from(PROJECTS));
    }

    #[test]
    fn recent_wsl_entries_are_not_looked_up() {
        // A stopped distro must not start just to draw the start page.
        assert!(Recent::is_there(Path::new(r"\\wsl.localhost\Gone\home\x")));
        assert!(!Recent::is_there(Path::new(r"C:\graft-no-such-folder-x")));
    }
}
