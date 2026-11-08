//! The application: project tabs, the start page, and everything projects
//! share (job pool, git console, toasts, remembered state).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Context as _;
use crossbeam_channel::Receiver;
use egui::{CentralPanel, Frame, Margin, Panel};
use graft_core::console::Console;
use graft_core::paths::same_path;
use graft_core::repo::{self, Location};
use graft_core::{JobPool, Waker};
use serde::{Deserialize, Serialize};

use crate::groups::{self, GroupId, TabGroup};
use crate::jobs::{Jobs, shown_error};
use crate::keys::{self, Command};
use crate::project::{Action, Project, RepoMemory, ReviewState, SlashedBranch};
use crate::running::Running;
use crate::settings::Settings;
use crate::theme::color;
use crate::ui::about::AboutAction;
use crate::ui::console::{self as console_ui, ConsoleView, STATUS_HEIGHT};
use crate::ui::group_dialog::{self, GroupForm, GroupTarget};
use crate::ui::project_tabs::{self, GroupInfo, TabAction, TabInfo};
use crate::ui::settings::{self as settings_ui, SettingsForm};
use crate::ui::start::{Recent, StartAction, StartPage};
use crate::ui::toasts::Toasts;
use crate::ui::{Actions, AppAction};
use crate::update;
use crate::{fonts, launch, theme, ui};

const STORAGE_KEY: &str = "graft";
const MAX_RECENT: usize = 12;
/// How soon a second close of the window quits although an operation runs.
const CLOSE_AGAIN: Duration = Duration::from_secs(5);

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
    /// Review state (viewed marks, notes, comments, base branch) per
    /// repository root, then per worktree.
    reviews: HashMap<PathBuf, HashMap<PathBuf, ReviewState>>,
    /// "Recently slashed" per repository root.
    slashed: HashMap<PathBuf, Vec<SlashedBranch>>,
    settings: Settings,
}

impl Persisted {
    /// What was saved about each repository, by its root.
    fn memories(&mut self) -> HashMap<PathBuf, RepoMemory> {
        let mut memories: HashMap<PathBuf, RepoMemory> = HashMap::new();
        for (root, reviews) in std::mem::take(&mut self.reviews) {
            memories.entry(root).or_default().reviews = reviews;
        }
        for (root, slashed) in std::mem::take(&mut self.slashed) {
            let key = memories.keys().find(|k| same_path(k, &root)).cloned();
            memories.entry(key.unwrap_or(root)).or_default().slashed = slashed;
        }
        memories
    }

    fn set_memories(&mut self, memories: impl IntoIterator<Item = (PathBuf, RepoMemory)>) {
        for (root, memory) in memories {
            let memory = memory.pruned();
            if !memory.reviews.is_empty() {
                self.reviews.insert(root.clone(), memory.reviews);
            }
            if !memory.slashed.is_empty() {
                self.slashed.insert(root, memory.slashed);
            }
        }
    }
}

impl crate::jobs::Owner for GraftApp {
    fn panicked(&mut self, message: String) {
        self.toasts.error(message);
    }
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
    jobs: Jobs<GraftApp>,
    toasts: Toasts,
    search: String,
    window_title: String,
    was_focused: bool,
    /// What Graft remembers about repositories that are not open.
    memories: HashMap<PathBuf, RepoMemory>,
    settings: Settings,
    settings_form: Option<SettingsForm>,
    update: update::Status,
    about_open: bool,
    /// An update is installed and the new Graft started: close this one.
    closing: bool,
    /// When closing the window was last refused, as an operation ran.
    close_refused: Option<Instant>,
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

        let mut persisted: Persisted = cc
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
            .unwrap_or_else(crate::settings::default_clone_parent);
        let mut memories = persisted.memories();
        let settings = std::mem::take(&mut persisted.settings);
        theme::apply(&cc.egui_ctx, settings.theme.preference());
        let projects: Vec<Project> = persisted
            .open
            .iter()
            .map(|root| {
                Project::new(
                    root.clone(),
                    root.clone(),
                    settings.clone(),
                    take_memory(&mut memories, root),
                    console.clone(),
                    pool.clone(),
                )
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
            jobs: Jobs::new(pool),
            toasts: Toasts::default(),
            search: String::new(),
            window_title: String::new(),
            was_focused: true,
            memories,
            settings,
            settings_form: None,
            update: update::Status::Idle,
            about_open: false,
            closing: false,
            close_refused: None,
            forwarded,
        };
        app.arrange();
        app.check_recent();
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
        while let Some(apply) = self.jobs.next_done() {
            apply(self);
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
        self.jobs
            .spawn(move || repo::locate(&path, &console), Self::opened);
    }

    /// A dropped folder opens; a dropped file opens its folder.
    fn open_dropped(&mut self, path: PathBuf) {
        let console = self.console.clone();
        self.jobs.spawn(
            move || {
                let folder = if path.is_dir() {
                    path
                } else {
                    path.parent()?.to_path_buf()
                };
                Some(repo::locate(&folder, &console))
            },
            |app, located| {
                if let Some(result) = located {
                    app.opened(result);
                }
            },
        );
    }

    /// A repository was found, cloned or created, or that failed.
    fn opened(&mut self, result: anyhow::Result<Location>) {
        match result {
            Ok(location) => self.add_project(location),
            Err(e) => {
                if let Some(error) = shown_error(&e) {
                    self.open_failed(error);
                }
            }
        }
    }

    /// The start page's clone or init ended.
    fn created(&mut self, result: anyhow::Result<Location>) {
        self.start.stop(None);
        if let Ok(location) = &result {
            self.toasts
                .info(format!("Ready: {}", location.root.display()));
        }
        self.opened(result);
    }

    /// Say why something failed (see [`shown_error`]), as a toast.
    fn report(&mut self, error: anyhow::Error) {
        if let Some(error) = shown_error(&error) {
            self.toasts.error(error);
        }
    }

    /// Say why opening failed: on the start page while it shows and no
    /// clone or init runs there, else as a toast.
    fn open_failed(&mut self, error: String) {
        if self.active.is_none() && !self.start.is_busy() {
            self.start.stop(Some(error));
        } else {
            self.toasts.error(error);
        }
    }

    fn add_project(&mut self, location: Location) {
        self.start.opened();
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
        let memory = take_memory(&mut self.memories, &location.root);
        let project = Project::new(
            location.root,
            location.opened,
            self.settings.clone(),
            memory,
            self.console.clone(),
            self.jobs.pool().clone(),
        );
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
                let id = groups::next_id(&self.groups);
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
            self.check_recent();
        }
    }

    /// See in the background which recent repositories are still there.
    fn check_recent(&self) {
        let paths: Vec<PathBuf> = self.recent.iter().map(|r| r.path.clone()).collect();
        self.jobs.spawn(
            move || {
                Some(
                    paths
                        .into_iter()
                        .map(|path| {
                            let there = Recent::is_there(&path);
                            (path, there)
                        })
                        .collect::<Vec<_>>(),
                )
            },
            |app, checked| {
                for (path, there) in checked.into_iter().flatten() {
                    for r in app.recent.iter_mut().filter(|r| r.path == path) {
                        r.exists = there;
                    }
                }
            },
        );
    }

    /// The window is closing (its close button, Alt+F4, an update's
    /// restart): not while an operation runs in a tab, as for closing the
    /// tab, unless the user closes again right away.
    fn close_requested(&mut self, ctx: &egui::Context) {
        let waits: Vec<String> = self
            .projects
            .iter()
            .filter_map(|p| {
                let wait = p.wait_before_closing()?;
                Some(format!(
                    "{wait} in the {} tab before quitting. Close again to quit anyway.",
                    p.name
                ))
            })
            .collect();
        let now = Instant::now();
        if self.closing || may_quit(!waits.is_empty(), self.close_refused, now) {
            return;
        }
        ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
        self.close_refused = Some(now);
        for wait in waits {
            self.toasts.error(wait);
        }
    }

    /// Close a project's tab, unless an operation still runs there (see
    /// [`Project::may_close`]). Every way of closing tabs comes here.
    fn close_project(&mut self, index: usize) {
        if !self.projects.get_mut(index).is_some_and(Project::may_close) {
            return;
        }
        let project = self.projects.remove(index);
        self.memories
            .insert(project.root.clone(), project.memory().clone());
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
                    self.group_form = Some(GroupForm::new(
                        GroupTarget::New(p.root.clone()),
                        String::new(),
                        groups::free_color(&self.groups, color::GROUP_COUNT),
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
            StartAction::PickOpen => {
                let start = self.start.open_start();
                self.pick("Open a git repository", start, Self::open_folder);
            }
            StartAction::LoadDistros => {
                self.jobs.spawn(
                    || Some(graft_core::wsl::distros()),
                    |app, distros| app.start.set_distros(distros.unwrap_or_default()),
                );
            }
            StartAction::LoadHome(distro) => {
                let asked = distro.clone();
                // Its own thread: the first call may boot the distro.
                self.jobs.spawn_long(
                    move |_| graft_core::wsl::Wsl::new(&asked).home(),
                    |app, home| app.start.set_home(distro, home),
                );
            }
            StartAction::PickCloneParent => {
                let start = self.start.clone_parent_start();
                self.pick("Clone into folder", start, |app, folder| {
                    app.start.picked_clone_parent(&folder)
                });
            }
            StartAction::PickInitFolder => {
                let start = self.start.init_start();
                self.pick("Folder for the new repository", start, |app, folder| {
                    app.start.picked_init_folder(&folder)
                });
            }
            StartAction::Clone { url, dest } => {
                let label = format!("Cloning {}", graft_core::console::redact(&url));
                let (running, cancel) = Running::cancellable(label);
                let report = running.reporter(self.jobs.waker());
                self.start.begin(running);
                let console = self.console.clone();
                self.jobs.spawn_long(
                    move |_| repo::clone(&url, &dest, &console, &cancel, report),
                    Self::created,
                );
            }
            StartAction::Init(path) => {
                let label = format!("Creating {}", self.start.show_path(&path));
                self.start.begin(Running::new(label));
                let console = self.console.clone();
                self.jobs
                    .spawn(move || repo::init(&path, &console), Self::created);
            }
            StartAction::Cancel => self.start.cancel(),
            StartAction::OpenRecent(path) => self.open_folder(path),
            StartAction::ForgetRecent(path) => self.recent.retain(|r| !same_path(&r.path, &path)),
            StartAction::OpenGroup(id) => self.open_group(id),
            StartAction::RenameGroup(id) => self.rename_group(id),
            StartAction::DeleteGroup(id) => self.groups.retain(|g| g.id != id),
        }
    }

    /// Show a folder picker on its own thread; `picked` gets the folder.
    fn pick(
        &self,
        title: &'static str,
        start: Option<PathBuf>,
        picked: impl FnOnce(&mut GraftApp, PathBuf) + Send + 'static,
    ) {
        self.jobs.spawn_long(
            move |_| launch::pick_folder(title, start),
            |app, folder| {
                if let Some(folder) = folder {
                    picked(app, folder);
                }
            },
        );
    }

    /// `manual` when the user asked (About), so failures are shown.
    fn check_update(&mut self, manual: bool) {
        self.update = update::Status::Checking;
        // Its own thread: it waits on the network.
        self.jobs.spawn_long(
            |_| update::check(),
            move |app, result| {
                app.update = match result {
                    Ok(release) if release.is_newer() => update::Status::Available(release),
                    Ok(_) => update::Status::UpToDate,
                    Err(e) if manual => update::Status::Failed(format!("{e:#}")),
                    Err(e) => {
                        tracing::info!("update check: {e:#}");
                        update::Status::Idle
                    }
                };
            },
        );
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
                self.jobs.spawn(
                    move || {
                        open::that_detached(&url).with_context(|| format!("Could not open {url}"))
                    },
                    |app, opened| {
                        if let Err(e) = opened {
                            app.report(e);
                        }
                    },
                );
            }
            Some(AboutAction::Install) => {
                let update::Status::Available(release) = &self.update else {
                    return;
                };
                let release = release.clone();
                self.update = update::Status::Installing(release.clone());
                self.jobs.spawn_long(
                    // Starting the new Graft (or its setup) happens here too,
                    // off the UI thread.
                    move |_| update::install(&release).and_then(|i| update::restart(&i)),
                    |app, restarted| match restarted {
                        // The new Graft runs: close this one.
                        Ok(()) => app.closing = true,
                        Err(e) => app.update = update::Status::Failed(format!("{e:#}")),
                    },
                );
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
                self.change_settings(|s| *s = settings.clone());
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
        let keys = self.settings.keys.clone();
        let pressed = |command| keys.pressed(ctx, command);

        if pressed(Command::Settings) {
            self.open_settings();
        }

        if pressed(Command::NewTab) {
            self.activate(None);
        }
        if pressed(Command::OpenFolder) {
            self.pick("Open a git repository", None, Self::open_folder);
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
            if pressed(Command::PreviousTab) {
                self.activate(Some((current + count - 1) % count));
            } else if pressed(Command::NextTab) {
                let next = if self.active.is_some() {
                    (current + 1) % count
                } else {
                    0
                };
                self.activate(Some(next));
            }
            if let Some(n) = keys::go_to_tab(ctx, count) {
                self.activate(Some(n));
            }
        }
        if pressed(Command::Refresh)
            && let Some(i) = self.active
        {
            self.projects[i].refresh();
        }
        if pressed(Command::Console) {
            self.console_open = !self.console_open;
        }
        if pressed(Command::WrapLines) {
            let wrap = !self.settings.wrap_lines;
            self.change_settings(|s| s.wrap_lines = wrap);
        }
        if pressed(Command::WholeFile) {
            self.toggle_whole_file();
        }
    }

    /// Change the settings everywhere they live: the app's, every
    /// project's, and an open Settings window's (it saves what it shows).
    fn change_settings(&mut self, change: impl Fn(&mut Settings)) {
        change(&mut self.settings);
        for p in &mut self.projects {
            p.set_settings(&self.settings);
        }
        if let Some(form) = &mut self.settings_form {
            change(&mut form.draft);
        }
    }

    fn toggle_whole_file(&mut self) {
        let whole = !self.settings.whole_file;
        self.change_settings(|s| s.whole_file = whole);
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
            self.open_dropped(path);
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
        let title = match self.active {
            Some(i) => self.projects[i].title(),
            None => "Graft".to_owned(),
        };
        if title != self.window_title {
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(title.clone()));
            self.window_title = title;
        }
    }

    /// What the status bar shows as running, and whether it can be cancelled.
    fn activity(&self) -> Option<(String, bool)> {
        if let Some(status) = self.start.busy_status() {
            return Some((status, false));
        }
        self.projects[self.active?].activity()
    }

    fn project_ui(&mut self, ui: &mut egui::Ui, index: usize) {
        let Actions { project, app } =
            ui::project::show(ui, &mut self.projects, index, &mut self.search);
        for action in project {
            self.projects[index].apply(action);
        }
        if let Some(text) = self.projects[index].clipboard.take() {
            ui.ctx().copy_text(text);
        }
        for action in app {
            match action {
                AppAction::Close => {
                    self.close_project(index);
                    return;
                }
                AppAction::ToggleWholeFile => self.toggle_whole_file(),
                AppAction::InProject { project, actions } if project < self.projects.len() => {
                    self.activate(Some(project));
                    for action in actions {
                        self.projects[project].apply(action);
                    }
                }
                AppAction::InProject { .. } => {}
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
        if ctx.input(|i| i.viewport().close_requested()) {
            self.close_requested(&ctx);
        }
        if let Some(wake) = self.projects.iter().filter_map(Project::next_wake).min() {
            ctx.request_repaint_after(wake);
        }

        let infos: Vec<TabInfo> = self
            .projects
            .iter()
            .map(|p| TabInfo {
                name: p.name.clone(),
                path: graft_core::paths::display(&p.root),
                dirty: p.is_dirty(),
                busy: p.is_starting(),
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
                if let Some(load) = self.start.take_load() {
                    self.on_start_action(load);
                }
                let action = CentralPanel::no_frame()
                    .frame(Frame::new().fill(color::app_bg()))
                    .show(ui, |ui| {
                        self.start
                            .show(ui, &self.recent, &self.groups, &self.settings.keys)
                    })
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
        let mut persisted = Persisted {
            open: self.projects.iter().map(|p| p.root.clone()).collect(),
            active: self.active,
            recent: self.recent.iter().map(|r| r.path.clone()).collect(),
            groups: self.groups.clone(),
            clone_parent: Some(self.start.clone_parent_path()),
            console_open: self.console_open,
            settings: self.settings.clone(),
            ..Persisted::default()
        };
        let open = self
            .projects
            .iter()
            .map(|p| (p.root.clone(), p.memory().clone()));
        persisted.set_memories(self.memories.clone().into_iter().chain(open));
        eframe::set_value(storage, STORAGE_KEY, &persisted);
    }

    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        color::app_bg().to_normalized_gamma_f32()
    }
}

/// What was saved for the repository at `root`.
fn take_memory(memories: &mut HashMap<PathBuf, RepoMemory>, root: &Path) -> RepoMemory {
    let key = memories.keys().find(|k| same_path(k, root)).cloned();
    key.and_then(|k| memories.remove(&k)).unwrap_or_default()
}

/// Whether the window may close now: yes when no operation runs (`busy`),
/// or when closing was refused (`refused`) less than [`CLOSE_AGAIN`] ago.
fn may_quit(busy: bool, refused: Option<Instant>, now: Instant) -> bool {
    !busy || refused.is_some_and(|at| now.saturating_duration_since(at) < CLOSE_AGAIN)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_second_close_soon_after_quits_anyway() {
        let now = Instant::now();
        let ago = |secs| now.checked_sub(Duration::from_secs(secs)).unwrap();
        assert!(may_quit(false, None, now), "nothing runs");
        assert!(!may_quit(true, None, now), "the first close");
        assert!(may_quit(true, Some(ago(1)), now), "closed again soon");
        assert!(
            !may_quit(true, Some(ago(6)), now),
            "closed again much later"
        );
    }
}
