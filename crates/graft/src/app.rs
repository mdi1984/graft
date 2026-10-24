//! The application: project tabs, the start page, and everything projects
//! share (job pool, git console, toasts, remembered state).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use crossbeam_channel::{Receiver, Sender};
use egui::{CentralPanel, Frame, Key, Margin, Modifiers, Panel};
use graft_core::jobs::{JobPool, Waker};
use graft_core::repo::{self, Location, Progress};
use graft_core::{CancelToken, Console, same_path};
use serde::{Deserialize, Serialize};

use crate::project::{Action, Project, ReviewState};
use crate::theme::color;
use crate::ui::console::{self as console_ui, ConsoleView, STATUS_HEIGHT};
use crate::ui::project_tabs::{self, TabAction, TabInfo};
use crate::ui::start::{Busy, Recent, StartAction, StartPage};
use crate::ui::toasts::Toasts;
use crate::{fonts, theme, ui};

const STORAGE_KEY: &str = "graft";
const MAX_RECENT: usize = 12;

/// What Graft remembers between runs.
#[derive(Serialize, Deserialize, Default)]
#[serde(default)]
struct Persisted {
    open: Vec<PathBuf>,
    active: Option<usize>,
    recent: Vec<PathBuf>,
    clone_parent: Option<PathBuf>,
    console_open: bool,
    /// Review marks and notes per repository root, then per worktree.
    reviews: HashMap<PathBuf, HashMap<PathBuf, ReviewState>>,
}

#[derive(Clone, Copy)]
enum Pick {
    Open,
    CloneParent,
    InitFolder,
}

enum AppEvent {
    Picked(Pick, Option<PathBuf>),
    Opened(Result<Location, String>),
}

pub struct GraftApp {
    projects: Vec<Project>,
    /// `None` shows the start page.
    active: Option<usize>,
    /// Where to go back to when the start page is closed.
    last_active: Option<usize>,
    start: StartPage,
    recent: Vec<Recent>,
    console: Console,
    console_view: ConsoleView,
    console_open: bool,
    pool: JobPool,
    tx: Sender<AppEvent>,
    rx: Receiver<AppEvent>,
    toasts: Toasts,
    search: String,
    window_title: String,
    was_focused: bool,
    /// Review state of repositories that are not open.
    reviews: HashMap<PathBuf, HashMap<PathBuf, ReviewState>>,
}

impl GraftApp {
    pub fn new(cc: &eframe::CreationContext<'_>, folders: Vec<PathBuf>) -> Self {
        fonts::install(&cc.egui_ctx);
        theme::apply(&cc.egui_ctx);

        let ctx = cc.egui_ctx.clone();
        let waker: Waker = Arc::new(move || ctx.request_repaint());
        let threads = std::thread::available_parallelism()
            .map_or(4, |n| n.get())
            .clamp(2, 8);
        let pool = JobPool::new(threads, waker);
        let console = Console::default();
        let (tx, rx) = crossbeam_channel::unbounded();

        let persisted: Persisted = cc
            .storage
            .and_then(|s| eframe::get_value(s, STORAGE_KEY))
            .unwrap_or_default();
        let clone_parent = persisted
            .clone_parent
            .clone()
            .or_else(|| {
                persisted
                    .recent
                    .first()
                    .and_then(|p| p.parent())
                    .map(Path::to_path_buf)
            })
            .unwrap_or_else(default_clone_parent);
        let mut reviews = persisted.reviews;
        let projects: Vec<Project> = persisted
            .open
            .iter()
            .map(|root| {
                let mut project =
                    Project::new(root.clone(), root.clone(), console.clone(), pool.clone());
                project.reviews = take_reviews(&mut reviews, root);
                project
            })
            .collect();
        let active = persisted
            .active
            .filter(|&i| i < projects.len())
            .or_else(|| (!projects.is_empty()).then_some(0));

        let mut app = Self {
            projects,
            active,
            last_active: None,
            start: StartPage::new(clone_parent),
            recent: persisted.recent.into_iter().map(Recent::new).collect(),
            console,
            console_view: ConsoleView::default(),
            console_open: persisted.console_open,
            pool,
            tx,
            rx,
            toasts: Toasts::default(),
            search: String::new(),
            window_title: String::new(),
            was_focused: true,
            reviews,
        };
        for folder in folders {
            let folder = std::path::absolute(&folder).unwrap_or(folder);
            app.open_folder(folder);
        }
        app
    }

    fn poll(&mut self) {
        while let Ok(event) = self.rx.try_recv() {
            match event {
                AppEvent::Picked(_, None) => {}
                AppEvent::Picked(Pick::Open, Some(path)) => self.open_folder(path),
                AppEvent::Picked(Pick::CloneParent, Some(path)) => {
                    self.start.clone_parent = path.display().to_string();
                }
                AppEvent::Picked(Pick::InitFolder, Some(path)) => {
                    self.start.init_path = path.display().to_string();
                }
                AppEvent::Opened(Ok(location)) => self.add_project(location),
                AppEvent::Opened(Err(error)) => {
                    self.start.busy = None;
                    if error == "Cancelled" {
                        continue;
                    }
                    if self.active.is_none() {
                        self.start.error = Some(error);
                    } else {
                        self.toasts.error(error);
                    }
                }
            }
        }
        for project in &mut self.projects {
            project.poll();
            for notice in project.notices.drain(..) {
                self.toasts.error(notice);
            }
            for info in project.infos.drain(..) {
                self.toasts.info(info);
            }
        }
        self.console_view.sync(&self.console);
    }

    /// Find the repository at `path` in the background, then open it.
    fn open_folder(&mut self, path: PathBuf) {
        let console = self.console.clone();
        self.pool.spawn(&self.tx, move || {
            AppEvent::Opened(repo::locate(&path, &console).map_err(|e| format!("{e:#}")))
        });
    }

    fn add_project(&mut self, location: Location) {
        if self.start.busy.is_some() {
            self.toasts
                .info(format!("Ready: {}", location.root.display()));
        }
        self.start.reset();
        self.remember(&location.root);
        if let Some(i) = self
            .projects
            .iter()
            .position(|p| same_path(&p.root, &location.root))
        {
            self.activate(Some(i));
            if !same_path(&location.opened, &location.root) {
                self.projects[i].apply(Action::OpenWorktree(location.opened));
            }
            return;
        }
        let mut project = Project::new(
            location.root,
            location.opened,
            self.console.clone(),
            self.pool.clone(),
        );
        project.reviews = take_reviews(&mut self.reviews, &project.root);
        self.projects.push(project);
        self.activate(Some(self.projects.len() - 1));
    }

    fn remember(&mut self, root: &Path) {
        self.recent.retain(|r| !same_path(&r.path, root));
        self.recent.insert(0, Recent::new(root.to_path_buf()));
        self.recent.truncate(MAX_RECENT);
    }

    fn activate(&mut self, index: Option<usize>) {
        if index == self.active {
            return;
        }
        if self.active.is_some() {
            self.last_active = self.active;
        }
        self.active = index;
        if let Some(i) = index {
            self.projects[i].refresh();
        } else {
            // Recent repositories may have moved since the list was built.
            for r in &mut self.recent {
                *r = Recent::new(std::mem::take(&mut r.path));
            }
        }
    }

    fn close_project(&mut self, index: usize) {
        if index >= self.projects.len() {
            return;
        }
        let mut project = self.projects.remove(index);
        self.reviews
            .insert(project.root.clone(), std::mem::take(&mut project.reviews));
        let fix = |slot: Option<usize>, len: usize| match slot {
            Some(i) if i == index => None,
            Some(i) if i > index => Some(i - 1),
            Some(i) if i < len => Some(i),
            _ => None,
        };
        let len = self.projects.len();
        let was_active = self.active == Some(index);
        self.active = fix(self.active, len);
        self.last_active = fix(self.last_active, len);
        if was_active && len > 0 {
            self.active = self.last_active.or(Some(index.min(len - 1)));
            if let Some(i) = self.active {
                self.projects[i].refresh();
            }
        }
    }

    fn close_start(&mut self) {
        if self.projects.is_empty() {
            return;
        }
        let target = self
            .last_active
            .filter(|&i| i < self.projects.len())
            .unwrap_or(self.projects.len() - 1);
        self.activate(Some(target));
    }

    fn on_tab_action(&mut self, action: TabAction) {
        match action {
            TabAction::Activate(i) => self.activate(Some(i)),
            TabAction::Close(i) => self.close_project(i),
            TabAction::Move { from, to } => {
                self.projects.swap(from, to);
                let swap = |slot: Option<usize>| match slot {
                    Some(i) if i == from => Some(to),
                    Some(i) if i == to => Some(from),
                    other => other,
                };
                self.active = swap(self.active);
                self.last_active = swap(self.last_active);
            }
            TabAction::NewTab => self.activate(None),
            TabAction::CloseStart => self.close_start(),
        }
    }

    fn on_start_action(&mut self, action: StartAction) {
        match action {
            StartAction::PickOpen => self.pick(Pick::Open, "Open a git repository", None),
            StartAction::PickCloneParent => {
                let dir = PathBuf::from(self.start.clone_parent.trim());
                self.pick(Pick::CloneParent, "Clone into folder", Some(dir));
            }
            StartAction::PickInitFolder => {
                self.pick(Pick::InitFolder, "Folder for the new repository", None);
            }
            StartAction::Clone { url, dest } => {
                let progress = Arc::new(Mutex::new(Progress::default()));
                let cancel = CancelToken::default();
                self.start.busy = Some(Busy {
                    label: format!("Cloning {url}"),
                    progress: progress.clone(),
                    cancel: Some(cancel.clone()),
                });
                let console = self.console.clone();
                self.pool.spawn_long(&self.tx, move |waker| {
                    let result = repo::clone(&url, &dest, &console, &cancel, |p| {
                        if let Ok(mut slot) = progress.lock() {
                            *slot = p;
                        }
                        waker();
                    });
                    AppEvent::Opened(result.map_err(|e| format!("{e:#}")))
                });
            }
            StartAction::Init(path) => {
                self.start.busy = Some(Busy {
                    label: format!("Creating {}", path.display()),
                    progress: Arc::default(),
                    cancel: None,
                });
                let console = self.console.clone();
                self.pool.spawn(&self.tx, move || {
                    AppEvent::Opened(repo::init(&path, &console).map_err(|e| format!("{e:#}")))
                });
            }
            StartAction::Cancel => {
                if let Some(cancel) = self.start.busy.as_ref().and_then(|b| b.cancel.as_ref()) {
                    cancel.cancel();
                }
            }
            StartAction::OpenRecent(path) => self.open_folder(path),
            StartAction::ForgetRecent(path) => self.recent.retain(|r| !same_path(&r.path, &path)),
        }
    }

    /// Show a folder picker on its own thread; the answer arrives as an event.
    fn pick(&self, pick: Pick, title: &'static str, start: Option<PathBuf>) {
        self.pool.spawn_long(&self.tx, move |_| {
            let mut dialog = rfd::FileDialog::new().set_title(title);
            if let Some(dir) = start.filter(|d| d.is_dir()) {
                dialog = dialog.set_directory(dir);
            }
            AppEvent::Picked(pick, dialog.pick_folder())
        });
    }

    fn shortcuts(&mut self, ctx: &egui::Context) {
        let ctrl = Modifiers::COMMAND;
        let ctrl_shift = Modifiers::COMMAND | Modifiers::SHIFT;
        let pressed = |mods, key| ctx.input_mut(|i| i.consume_key(mods, key));

        if pressed(ctrl, Key::T) {
            self.activate(None);
        }
        if pressed(ctrl, Key::O) {
            self.pick(Pick::Open, "Open a git repository", None);
        }
        if pressed(ctrl, Key::W) {
            match self.active {
                Some(i) => self.close_project(i),
                None => self.close_start(),
            }
        }
        let count = self.projects.len();
        if count > 0 {
            let current = self.active.unwrap_or(count - 1);
            if pressed(ctrl_shift, Key::Tab) || pressed(ctrl, Key::PageUp) {
                self.activate(Some((current + count - 1) % count));
            } else if pressed(ctrl, Key::Tab) || pressed(ctrl, Key::PageDown) {
                let next = if self.active.is_some() {
                    (current + 1) % count
                } else {
                    0
                };
                self.activate(Some(next));
            }
            let digits = [
                Key::Num1,
                Key::Num2,
                Key::Num3,
                Key::Num4,
                Key::Num5,
                Key::Num6,
                Key::Num7,
                Key::Num8,
                Key::Num9,
            ];
            for (n, key) in digits.into_iter().enumerate() {
                if pressed(ctrl, key) && n < count {
                    self.activate(Some(n));
                }
            }
        }
        if (pressed(Modifiers::NONE, Key::F5) || pressed(ctrl, Key::R))
            && let Some(i) = self.active
        {
            self.projects[i].refresh();
        }
        if pressed(ctrl, Key::Backtick) || pressed(ctrl, Key::J) {
            self.console_open = !self.console_open;
        }
    }

    /// Folders dropped onto the window open as projects.
    fn handle_drops(&mut self, ctx: &egui::Context) {
        let dropped: Vec<PathBuf> = ctx.input(|i| {
            i.raw
                .dropped_files
                .iter()
                .map(|f| f.path().to_path_buf())
                .filter(|p| !p.as_os_str().is_empty())
                .collect()
        });
        for path in dropped {
            let folder = if path.is_dir() {
                path
            } else {
                match path.parent() {
                    Some(p) => p.to_path_buf(),
                    None => continue,
                }
            };
            self.open_folder(folder);
        }
        let hovering = ctx.input(|i| !i.raw.hovered_files.is_empty());
        if hovering {
            let painter = ctx.layer_painter(egui::LayerId::new(
                egui::Order::Foreground,
                egui::Id::unique("drop-overlay"),
            ));
            let rect = ctx.content_rect();
            painter.rect_filled(rect, 0, egui::Color32::from_black_alpha(170));
            painter.text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                "Drop a folder to open it",
                theme::semibold(20.0),
                color::TEXT,
            );
        }
    }

    /// Coming back to Graft (e.g. from an editor or terminal) re-reads status.
    fn refresh_on_focus(&mut self, ctx: &egui::Context) {
        let focused = ctx.input(|i| i.focused);
        if focused
            && !self.was_focused
            && let Some(i) = self.active
        {
            self.projects[i].refresh();
        }
        self.was_focused = focused;
    }

    fn sync_window_title(&mut self, ctx: &egui::Context) {
        let title = match self.active.map(|i| &self.projects[i]) {
            Some(project) => match project.worktree() {
                Some(wt) if !wt.is_main => format!("{} · {} — Graft", project.name, wt.label()),
                _ => format!("{} — Graft", project.name),
            },
            None => "Graft".to_owned(),
        };
        if title != self.window_title {
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(title.clone()));
            self.window_title = title;
        }
    }

    /// What the status bar shows as running, and whether it can be cancelled.
    fn activity(&self) -> Option<(String, bool)> {
        if let Some(busy) = &self.start.busy {
            return Some((busy.label.clone(), false));
        }
        let project = &self.projects[self.active?];
        if let Some(op) = &project.op {
            let progress = op.progress.lock().map(|p| p.clone()).unwrap_or_default();
            let mut label = op.label.clone();
            if !progress.phase.is_empty() {
                label.push_str(" · ");
                label.push_str(&progress.phase);
            }
            if let Some(fraction) = progress.fraction {
                label.push_str(&format!(" {:.0}%", fraction * 100.0));
            }
            return Some((label, op.cancel.is_some()));
        }
        if project.loading {
            Some((format!("Refreshing {}…", project.name), false))
        } else if project.history_loading {
            Some(("Loading history…".to_owned(), false))
        } else {
            None
        }
    }

    fn project_ui(&mut self, ui: &mut egui::Ui, index: usize) {
        let mut actions = Vec::new();
        let project = &self.projects[index];
        if let Some(confirm) = &project.confirm {
            ui::confirm::show(ui.ctx(), confirm, &mut actions);
        }
        let project = &mut self.projects[index];
        if let (Some(form), Some(snap)) = (&mut project.new_worktree, &project.snapshot) {
            ui::new_worktree::show(ui.ctx(), form, snap, &mut actions);
        }
        if let Some(dialog) = &mut project.dialog {
            ui::dialog::show(ui.ctx(), dialog, &mut actions);
        }
        let project = &self.projects[index];
        let bar = |margin| Frame::new().fill(color::BAR_BG).inner_margin(margin);

        Panel::top("toolbar")
            .frame(bar(Margin::symmetric(12, 8)))
            .show(ui, |ui| {
                ui::toolbar::show(ui, project, &mut actions, &mut self.search)
            });
        Panel::top("tabs")
            .frame(bar(Margin::symmetric(12, 0)))
            .show(ui, |ui| ui::tabs::show(ui, project, &mut actions));
        if project.snapshot.is_some() {
            Panel::left("sidebar")
                .resizable(true)
                .default_size(260.0)
                .size_range(200.0..=440.0)
                .frame(
                    Frame::new()
                        .fill(color::SIDEBAR_BG)
                        .inner_margin(Margin::symmetric(8, 12)),
                )
                .show(ui, |ui| ui::sidebar::show(ui, project, &mut actions));
        }
        let project = &mut self.projects[index];
        CentralPanel::no_frame()
            .frame(Frame::new().fill(color::APP_BG))
            .show(ui, |ui| ui::main_area::show(ui, project, &mut actions));

        for action in actions {
            if matches!(action, Action::Close) {
                self.close_project(index);
                break;
            }
            self.projects[index].apply(action);
        }
    }
}

impl eframe::App for GraftApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.poll();
        self.shortcuts(&ctx);
        self.handle_drops(&ctx);
        self.refresh_on_focus(&ctx);
        self.sync_window_title(&ctx);

        let infos: Vec<TabInfo> = self
            .projects
            .iter()
            .map(|p| TabInfo {
                name: p.name.clone(),
                tooltip: p.root.display().to_string(),
                dirty: p.is_dirty(),
                busy: p.snapshot.is_none() && p.error.is_none(),
                error: p.error.is_some(),
            })
            .collect();
        let tab_action = Panel::top("project-tabs")
            .frame(Frame::new().fill(color::TABSTRIP_BG))
            .exact_size(project_tabs::HEIGHT)
            .show_separator_line(false)
            .show(ui, |ui| project_tabs::show(ui, &infos, self.active))
            .inner;
        if let Some(action) = tab_action {
            self.on_tab_action(action);
        }

        let activity = self.activity();
        let cancellable = activity.as_ref().is_some_and(|(_, c)| *c);
        let cancel = Panel::bottom("status-bar")
            .frame(
                Frame::new()
                    .fill(color::TABSTRIP_BG)
                    .inner_margin(Margin::symmetric(8, 0)),
            )
            .exact_size(STATUS_HEIGHT)
            .show(ui, |ui| {
                console_ui::status_bar(
                    ui,
                    activity.as_ref().map(|(label, _)| label.as_str()),
                    cancellable,
                    &self.console_view,
                    &mut self.console_open,
                )
            })
            .inner;
        if cancel && let Some(index) = self.active {
            self.projects[index].apply(Action::CancelOperation);
        }
        if self.console_open {
            Panel::bottom("console")
                .resizable(true)
                .default_size(220.0)
                .size_range(100.0..=640.0)
                .frame(
                    Frame::new()
                        .fill(color::SIDEBAR_BG)
                        .inner_margin(Margin::symmetric(12, 8)),
                )
                .show(ui, |ui| {
                    console_ui::console(ui, &mut self.console_view, &mut self.console_open)
                });
        }

        match self.active {
            Some(index) => self.project_ui(ui, index),
            None => {
                let action = CentralPanel::no_frame()
                    .frame(Frame::new().fill(color::APP_BG))
                    .show(ui, |ui| self.start.show(ui, &self.recent))
                    .inner;
                if let Some(action) = action {
                    self.on_start_action(action);
                }
            }
        }

        self.toasts.show(&ctx);
    }

    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        let clone_parent = self.start.clone_parent.trim();
        let mut reviews = self.reviews.clone();
        for p in &self.projects {
            reviews.insert(p.root.clone(), p.reviews.clone());
        }
        for worktrees in reviews.values_mut() {
            worktrees
                .retain(|_, s| s.base.is_some() || !s.viewed.is_empty() || !s.notes.is_empty());
        }
        reviews.retain(|_, worktrees| !worktrees.is_empty());
        let persisted = Persisted {
            open: self.projects.iter().map(|p| p.root.clone()).collect(),
            active: self.active,
            recent: self.recent.iter().map(|r| r.path.clone()).collect(),
            clone_parent: (!clone_parent.is_empty()).then(|| PathBuf::from(clone_parent)),
            console_open: self.console_open,
            reviews,
        };
        eframe::set_value(storage, STORAGE_KEY, &persisted);
    }

    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        color::APP_BG.to_normalized_gamma_f32()
    }
}

/// The saved review state of the repository at `root`.
fn take_reviews(
    reviews: &mut HashMap<PathBuf, HashMap<PathBuf, ReviewState>>,
    root: &Path,
) -> HashMap<PathBuf, ReviewState> {
    let key = reviews.keys().find(|k| same_path(k, root)).cloned();
    key.and_then(|k| reviews.remove(&k)).unwrap_or_default()
}

/// Where clones go when the user has not picked a folder yet.
fn default_clone_parent() -> PathBuf {
    std::env::var_os("USERPROFILE")
        .map(|home| PathBuf::from(home).join("source").join("repos"))
        .unwrap_or_else(|| PathBuf::from(r"C:\dev"))
}
