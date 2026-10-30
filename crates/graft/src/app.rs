//! The application: project tabs, the start page, and everything projects
//! share (job pool, git console, toasts, remembered state).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use crossbeam_channel::{Receiver, Sender};
use egui::{CentralPanel, Frame, Key, KeyboardShortcut, Margin, Modifiers, Panel};
use graft_core::jobs::{JobPool, Waker};
use graft_core::repo::{self, Location, Progress};
use graft_core::{CancelToken, Console, same_path};
use serde::{Deserialize, Serialize};

use crate::groups::{self, GroupId, TabGroup};
use crate::keys::{self, Command};
use crate::project::{Action, Project, ReviewState, SlashedBranch};
use crate::settings::Settings;
use crate::theme::color;
use crate::ui::about::AboutAction;
use crate::ui::console::{self as console_ui, ConsoleView, STATUS_HEIGHT};
use crate::ui::group_dialog::{self, GroupForm, GroupTarget};
use crate::ui::project_tabs::{self, GroupInfo, TabAction, TabInfo};
use crate::ui::settings::{self as settings_ui, SettingsForm};
use crate::ui::start::{Busy, Recent, StartAction, StartPage};
use crate::ui::toasts::Toasts;
use crate::update::{self, Release};
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
    groups: Vec<TabGroup>,
    clone_parent: Option<PathBuf>,
    console_open: bool,
    /// Review marks and notes per repository root, then per worktree.
    reviews: HashMap<PathBuf, HashMap<PathBuf, ReviewState>>,
    /// "Recently slashed" per repository root.
    slashed: HashMap<PathBuf, Vec<SlashedBranch>>,
    settings: Settings,
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
    /// `manual` when the user asked (About), so failures are shown.
    UpdateChecked {
        result: Result<Release, String>,
        manual: bool,
    },
    /// The new program, ready to start.
    UpdateInstalled(Result<update::Installed, String>),
}

pub struct GraftApp {
    projects: Vec<Project>,
    /// `None` shows the start page.
    active: Option<usize>,
    /// Where to go back to when the start page is closed.
    last_active: Option<usize>,
    start: StartPage,
    recent: Vec<Recent>,
    /// Tab groups, open or not.
    groups: Vec<TabGroup>,
    /// Naming a new group or renaming one.
    group_form: Option<GroupForm>,
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
    /// "Recently slashed" of repositories that are not open.
    slashed: HashMap<PathBuf, Vec<SlashedBranch>>,
    settings: Settings,
    settings_form: Option<SettingsForm>,
    update: update::Status,
    about_open: bool,
    /// An update is installed and the new Graft started: close this one.
    closing: bool,
    /// Folders from a second start of Graft (empty: just come to the front).
    forwarded: Receiver<Vec<PathBuf>>,
}

impl GraftApp {
    pub fn new(
        cc: &eframe::CreationContext<'_>,
        folders: Vec<PathBuf>,
        forwarded: Receiver<Vec<PathBuf>>,
        last_update: Option<Result<String, String>>,
    ) -> Self {
        fonts::install(&cc.egui_ctx);

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
        let mut slashed = persisted.slashed;
        let settings = persisted.settings;
        theme::apply(&cc.egui_ctx, settings.theme.preference());
        let projects: Vec<Project> = persisted
            .open
            .iter()
            .map(|root| {
                let mut project =
                    Project::new(root.clone(), root.clone(), console.clone(), pool.clone());
                project.reviews = take_reviews(&mut reviews, root);
                project.slashed = take_reviews(&mut slashed, root);
                project.settings = settings.clone();
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
            groups: persisted.groups,
            group_form: None,
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
            slashed,
            settings,
            settings_form: None,
            update: update::Status::Idle,
            about_open: false,
            closing: false,
            forwarded,
        };
        app.arrange();
        match last_update {
            Some(Ok(done)) => app.toasts.info(done),
            Some(Err(failed)) => app.toasts.error(failed),
            None => {}
        }
        if app.settings.check_updates && update::check_on_start() {
            app.check_update(false);
        }
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
                AppEvent::UpdateChecked { result, manual } => {
                    self.update = match result {
                        Ok(release) if release.is_newer() => update::Status::Available(release),
                        Ok(_) => update::Status::UpToDate,
                        Err(error) if manual => update::Status::Failed(error),
                        Err(error) => {
                            tracing::info!("update check: {error}");
                            update::Status::Idle
                        }
                    };
                }
                AppEvent::UpdateInstalled(result) => {
                    match result.and_then(|installed| {
                        update::restart(&installed).map_err(|e| format!("{e:#}"))
                    }) {
                        Ok(()) => self.closing = true,
                        Err(error) => self.update = update::Status::Failed(error),
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

    /// Another start of Graft: come to the front and open its folders.
    fn take_forwarded(&mut self, ctx: &egui::Context) {
        while let Ok(folders) = self.forwarded.try_recv() {
            ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
            ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
            for folder in folders {
                self.open_folder(folder);
            }
        }
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
        project.slashed = take_reviews(&mut self.slashed, &project.root);
        project.settings = self.settings.clone();
        self.projects.push(project);
        self.activate(Some(self.projects.len() - 1));
        // Next to the other open tabs of its group.
        self.arrange();
    }

    /// The group of each open tab.
    fn memberships(&self) -> Vec<Option<GroupId>> {
        self.projects
            .iter()
            .map(|p| groups::group_of(&self.groups, &p.root))
            .collect()
    }

    /// Put the tabs in `order` (old indexes); the active tab stays active.
    fn reorder(&mut self, order: &[usize]) {
        let mut old: Vec<Option<Project>> = std::mem::take(&mut self.projects)
            .into_iter()
            .map(Some)
            .collect();
        self.projects = order.iter().filter_map(|&i| old[i].take()).collect();
        let map = |slot: Option<usize>| slot.and_then(|i| order.iter().position(|&o| o == i));
        self.active = map(self.active);
        self.last_active = map(self.last_active);
    }

    /// Keep each group's tabs together, in the group's order.
    fn arrange(&mut self) {
        let rank = |i: usize| {
            let root = &self.projects[i].root;
            self.groups
                .iter()
                .find_map(|g| g.repos.iter().position(|r| same_path(r, root)))
                .unwrap_or(0)
        };
        let order = groups::contiguous_order(&self.memberships(), rank);
        if order.iter().enumerate().any(|(k, &i)| k != i) {
            self.reorder(&order);
        }
    }

    fn group_mut(&mut self, id: GroupId) -> Option<&mut TabGroup> {
        self.groups.iter_mut().find(|g| g.id == id)
    }

    /// Put tab `index` into `group` (or none), keeping groups together.
    fn set_group(&mut self, index: usize, group: Option<GroupId>) {
        let root = self.projects[index].root.clone();
        groups::set_group(&mut self.groups, &root, group);
        // A tab added to a collapsed group would vanish into it.
        if let Some(g) = group.and_then(|id| self.group_mut(id)) {
            g.collapsed = false;
        }
        self.arrange();
    }

    fn toggle_group(&mut self, id: GroupId) {
        let Some(group) = self.group_mut(id) else {
            return;
        };
        group.collapsed = !group.collapsed;
        if !group.collapsed {
            return;
        }
        // Collapsing hides its tabs: move off them, to the tab after the
        // group, else the one before it. Without another tab, the active
        // one stays shown.
        let memberships = self.memberships();
        if self.active.is_none_or(|a| memberships[a] != Some(id)) {
            return;
        }
        let first = memberships.iter().position(|g| *g == Some(id));
        let last = memberships.iter().rposition(|g| *g == Some(id));
        let next = last
            .map(|l| l + 1)
            .filter(|&n| n < self.projects.len())
            .or_else(|| first.and_then(|f| f.checked_sub(1)));
        if next.is_some() {
            self.activate(next);
        }
    }

    /// Open every repository of a group that is not open yet.
    fn open_group(&mut self, id: GroupId) {
        let Some(group) = self.group_mut(id) else {
            return;
        };
        group.collapsed = false;
        let repos = group.repos.clone();
        let mut first_open = None;
        for root in repos {
            match self.projects.iter().position(|p| same_path(&p.root, &root)) {
                Some(i) => first_open = first_open.or(Some(i)),
                None => self.open_folder(root),
            }
        }
        if let Some(i) = first_open {
            self.activate(Some(i));
        }
    }

    fn save_group_form(&mut self) {
        let Some(form) = self.group_form.take() else {
            return;
        };
        let name = form.name.trim().to_owned();
        match form.target {
            GroupTarget::New(root) => {
                let (id, _) = groups::next_id_and_color(&self.groups, color::lanes().len());
                self.groups.push(TabGroup {
                    id,
                    name,
                    color: form.color,
                    collapsed: false,
                    repos: Vec::new(),
                });
                groups::set_group(&mut self.groups, &root, Some(id));
                self.arrange();
            }
            GroupTarget::Rename(id) => {
                if let Some(g) = self.group_mut(id) {
                    g.name = name;
                    g.color = form.color;
                }
            }
        }
    }

    fn group_form_ui(&mut self, ctx: &egui::Context) {
        let Some(form) = &mut self.group_form else {
            return;
        };
        match group_dialog::show(ctx, form) {
            group_dialog::Outcome::Open => {}
            group_dialog::Outcome::Cancel => self.group_form = None,
            group_dialog::Outcome::Save => self.save_group_form(),
        }
    }

    fn rename_group(&mut self, id: GroupId) {
        if let Some(g) = self.groups.iter().find(|g| g.id == id) {
            self.group_form = Some(GroupForm::new(
                GroupTarget::Rename(id),
                g.name.clone(),
                g.color,
            ));
        }
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
        self.slashed
            .insert(project.root.clone(), std::mem::take(&mut project.slashed));
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
            TabAction::Move { from, to, group } => {
                if from >= self.projects.len() || to >= self.projects.len() {
                    return;
                }
                let mut order: Vec<usize> = (0..self.projects.len()).collect();
                order.remove(from);
                order.insert(to, from);
                self.reorder(&order);
                let root = self.projects[to].root.clone();
                groups::set_group(&mut self.groups, &root, group);
                // Groups remember the order their tabs were dragged into.
                let roots: Vec<PathBuf> = self.projects.iter().map(|p| p.root.clone()).collect();
                groups::follow_tabs(&mut self.groups, &roots);
                self.arrange();
            }
            TabAction::ToggleGroup(id) => self.toggle_group(id),
            TabAction::NewGroup(i) => {
                if let Some(p) = self.projects.get(i) {
                    let (_, color) = groups::next_id_and_color(&self.groups, color::lanes().len());
                    self.group_form = Some(GroupForm::new(
                        GroupTarget::New(p.root.clone()),
                        String::new(),
                        color,
                    ));
                }
            }
            TabAction::SetGroup { tab, group } => {
                if tab < self.projects.len() {
                    self.set_group(tab, group);
                }
            }
            TabAction::RenameGroup(id) => self.rename_group(id),
            TabAction::RecolorGroup(id, c) => {
                if let Some(g) = self.group_mut(id) {
                    g.color = c;
                }
            }
            TabAction::CloseGroup(id) => {
                let memberships = self.memberships();
                for i in (0..memberships.len()).rev() {
                    if memberships[i] == Some(id) {
                        self.close_project(i);
                    }
                }
            }
            TabAction::Ungroup(id) => self.groups.retain(|g| g.id != id),
            TabAction::NewTab => self.activate(None),
            TabAction::CloseStart => self.close_start(),
            TabAction::OpenSettings => self.open_settings(),
            TabAction::OpenAbout => self.about_open = true,
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
                    label: format!("Cloning {}", graft_core::console::redact(&url)),
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
            StartAction::OpenGroup(id) => self.open_group(id),
            StartAction::RenameGroup(id) => self.rename_group(id),
            StartAction::DeleteGroup(id) => self.groups.retain(|g| g.id != id),
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

    fn check_update(&mut self, manual: bool) {
        self.update = update::Status::Checking;
        self.pool.spawn(&self.tx, move || AppEvent::UpdateChecked {
            result: update::check().map_err(|e| format!("{e:#}")),
            manual,
        });
    }

    fn about_ui(&mut self, ctx: &egui::Context) {
        if !self.about_open {
            return;
        }
        match ui::about::show(ctx, &self.update) {
            None => {}
            Some(AboutAction::Close) => self.about_open = false,
            Some(AboutAction::Check) => self.check_update(true),
            Some(AboutAction::Open(url)) => {
                if let Err(e) = open::that_detached(&url) {
                    self.toasts.error(format!("Could not open {url}: {e}"));
                }
            }
            Some(AboutAction::Install) => {
                let update::Status::Available(release) = &self.update else {
                    return;
                };
                let release = release.clone();
                self.update = update::Status::Installing(release.clone());
                self.pool.spawn_long(&self.tx, move |_| {
                    AppEvent::UpdateInstalled(
                        update::install(&release).map_err(|e| format!("{e:#}")),
                    )
                });
            }
        }
    }

    fn open_settings(&mut self) {
        if self.settings_form.is_some() {
            return;
        }
        self.settings_form = Some(SettingsForm::new(&self.settings));
        if let Some(i) = self.active {
            self.projects[i].apply(Action::LoadIdentity);
        }
    }

    fn settings_ui(&mut self, ctx: &egui::Context) {
        let Some(form) = &mut self.settings_form else {
            return;
        };
        let repo = self.active.map(|i| {
            let p = &self.projects[i];
            (p.name.as_str(), p.identity.as_ref())
        });
        let outcome = ui::settings::show(ctx, form, repo);
        // The theme choice previews live; Cancel puts the saved one back.
        let mut theme = form.draft.theme;
        match outcome {
            settings_ui::Outcome::Open => {}
            settings_ui::Outcome::Cancel => {
                self.settings_form = None;
                theme = self.settings.theme;
            }
            settings_ui::Outcome::About => {
                self.settings_form = None;
                theme = self.settings.theme;
                self.about_open = true;
            }
            settings_ui::Outcome::Save(settings, identity) => {
                self.settings_form = None;
                self.settings = settings;
                for p in &mut self.projects {
                    p.settings = self.settings.clone();
                }
                if let (Some(identity), Some(i)) = (identity, self.active) {
                    self.projects[i].apply(Action::SaveIdentity(identity));
                }
            }
        }
        if ctx.options(|o| o.theme_preference) != theme.preference() {
            ctx.set_theme(theme.preference());
            ctx.request_repaint();
        }
    }

    fn shortcuts(&mut self, ctx: &egui::Context) {
        let ctrl = Modifiers::COMMAND;
        let keys = self.settings.keys.clone();
        let pressed = |command| keys.pressed(ctx, command);
        // Fixed extras next to the configurable shortcuts (keys::FIXED).
        let fixed = |mods, key| keys::fixed_pressed(ctx, KeyboardShortcut::new(mods, key));

        if pressed(Command::Settings) {
            self.open_settings();
        }

        if pressed(Command::NewTab) {
            self.activate(None);
        }
        if pressed(Command::OpenFolder) {
            self.pick(Pick::Open, "Open a git repository", None);
        }
        if pressed(Command::CloseTab) {
            match self.active {
                Some(i) => self.close_project(i),
                None => self.close_start(),
            }
        }
        let count = self.projects.len();
        if count > 0 {
            let current = self.active.unwrap_or(count - 1);
            if pressed(Command::PreviousTab) || fixed(ctrl, Key::PageUp) {
                self.activate(Some((current + count - 1) % count));
            } else if pressed(Command::NextTab) || fixed(ctrl, Key::PageDown) {
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
                if n < count && fixed(ctrl, key) {
                    self.activate(Some(n));
                }
            }
        }
        if (pressed(Command::Refresh) || fixed(ctrl, Key::R))
            && let Some(i) = self.active
        {
            self.projects[i].refresh();
        }
        if pressed(Command::Console) || fixed(ctrl, Key::Backtick) {
            self.console_open = !self.console_open;
        }
        if pressed(Command::WrapLines) {
            self.settings.wrap_lines = !self.settings.wrap_lines;
            for p in &mut self.projects {
                p.settings.wrap_lines = self.settings.wrap_lines;
            }
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
            painter.rect_filled(rect, 0, color::app_bg().gamma_multiply(0.8));
            painter.text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                "Drop a folder to open it",
                theme::semibold(20.0),
                color::text(),
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
        if let Some(view) = &project.slash {
            let progress = project.op.as_ref().and_then(|op| {
                let p = op.progress.lock().ok()?;
                (!p.line.is_empty()).then(|| p.clone())
            });
            ui::slash::show(ui.ctx(), view, &project.slashed, progress, &mut actions);
        }
        let project = &self.projects[index];
        let bar = |margin| Frame::new().fill(color::bar_bg()).inner_margin(margin);

        Panel::top("toolbar")
            .frame(bar(Margin::symmetric(12, 8)))
            .show(ui, |ui| {
                ui::toolbar::show(ui, &self.projects, index, &mut actions, &mut self.search)
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
                        .fill(color::sidebar_bg())
                        .inner_margin(Margin::symmetric(8, 12)),
                )
                .show(ui, |ui| ui::sidebar::show(ui, project, &mut actions));
        }
        let project = &mut self.projects[index];
        CentralPanel::no_frame()
            .frame(Frame::new().fill(color::app_bg()))
            .show(ui, |ui| ui::main_area::show(ui, project, &mut actions));

        for action in actions {
            match action {
                Action::Close => {
                    self.close_project(index);
                    break;
                }
                Action::InProject { project, actions } if project < self.projects.len() => {
                    self.activate(Some(project));
                    for action in actions {
                        self.projects[project].apply(action);
                    }
                }
                action => self.projects[index].apply(action),
            }
        }
    }
}

impl eframe::App for GraftApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        if theme::sync(&ctx) {
            crate::native::set_border(color::border());
        }
        self.poll();
        self.take_forwarded(&ctx);
        self.shortcuts(&ctx);
        self.handle_drops(&ctx);
        self.refresh_on_focus(&ctx);
        self.sync_window_title(&ctx);
        self.settings_ui(&ctx);
        self.about_ui(&ctx);
        self.group_form_ui(&ctx);
        if self.closing {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        if self.settings.auto_fetch_minutes > 0 && !self.projects.is_empty() {
            ctx.request_repaint_after(std::time::Duration::from_secs(20));
        }

        let infos: Vec<TabInfo> = self
            .projects
            .iter()
            .map(|p| TabInfo {
                name: p.name.clone(),
                tooltip: p.root.display().to_string(),
                dirty: p.is_dirty(),
                busy: p.snapshot.is_none() && p.error.is_none(),
                error: p.error.is_some(),
                group: groups::group_of(&self.groups, &p.root),
            })
            .collect();
        let group_infos: Vec<GroupInfo> = self
            .groups
            .iter()
            .map(|g| GroupInfo {
                id: g.id,
                name: g.name.clone(),
                color: g.color,
                collapsed: g.collapsed,
            })
            .collect();
        let tab_action = Panel::top("project-tabs")
            .frame(Frame::new().fill(color::tabstrip_bg()))
            .exact_size(project_tabs::HEIGHT)
            .show_separator_line(false)
            .show(ui, |ui| {
                let update = match &self.update {
                    update::Status::Available(release) => Some(release.version.as_str()),
                    _ => None,
                };
                project_tabs::show(
                    ui,
                    &infos,
                    &group_infos,
                    self.active,
                    update,
                    &self.settings.keys,
                )
            })
            .inner;
        if let Some(action) = tab_action {
            self.on_tab_action(action);
        }

        let activity = self.activity();
        let cancellable = activity.as_ref().is_some_and(|(_, c)| *c);
        let cancel = Panel::bottom("status-bar")
            .frame(
                Frame::new()
                    .fill(color::tabstrip_bg())
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
                    &self.settings.keys,
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
                        .fill(color::sidebar_bg())
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
                    .frame(Frame::new().fill(color::app_bg()))
                    .show(ui, |ui| self.start.show(ui, &self.recent, &self.groups))
                    .inner;
                if let Some(action) = action {
                    self.on_start_action(action);
                }
            }
        }

        self.toasts.show(&ctx);
        ui::window_frame::resize_edges(&ctx);
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
        let mut slashed = self.slashed.clone();
        for p in &self.projects {
            slashed.insert(p.root.clone(), p.slashed.clone());
        }
        slashed.retain(|_, list| !list.is_empty());
        let persisted = Persisted {
            open: self.projects.iter().map(|p| p.root.clone()).collect(),
            active: self.active,
            recent: self.recent.iter().map(|r| r.path.clone()).collect(),
            groups: self.groups.clone(),
            clone_parent: (!clone_parent.is_empty()).then(|| PathBuf::from(clone_parent)),
            console_open: self.console_open,
            reviews,
            slashed,
            settings: self.settings.clone(),
        };
        eframe::set_value(storage, STORAGE_KEY, &persisted);
    }

    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        color::app_bg().to_normalized_gamma_f32()
    }
}

/// What was saved for the repository at `root`.
fn take_reviews<T: Default>(reviews: &mut HashMap<PathBuf, T>, root: &Path) -> T {
    let key = reviews.keys().find(|k| same_path(k, root)).cloned();
    key.and_then(|k| reviews.remove(&k)).unwrap_or_default()
}

/// Where clones go when the user has not picked a folder yet.
fn default_clone_parent() -> PathBuf {
    std::env::var_os("USERPROFILE")
        .map(|home| PathBuf::from(home).join("source").join("repos"))
        .unwrap_or_else(|| PathBuf::from(r"C:\dev"))
}
