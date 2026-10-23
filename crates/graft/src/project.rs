//! One open repository (a project tab): its snapshot, history, selection, and
//! the background jobs that load them. The UI reads this state and sends
//! [`Action`]s; nothing here blocks on git.

use std::cell::Cell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crossbeam_channel::{Receiver, Sender};
use graft_core::details::{self, CommitDetails};
use graft_core::history::{self, Commits, History};
use graft_core::jobs::JobPool;
use graft_core::{
    CancelToken, Console, Git, Oid, RepoSnapshot, Worktree, null_oid, same_path, snapshot,
};

/// Commits loaded into the graph. Enough for years of history in big repos.
const HISTORY_LIMIT: usize = 100_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tab {
    Workspace,
    Review,
    Stashes,
}

/// What the Workspace tab shows in its main area.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WorkspaceView {
    FileStatus,
    History,
}

/// Where the user is inside a project.
#[derive(Clone, Debug)]
pub struct Nav {
    pub tab: Tab,
    pub view: WorkspaceView,
    /// Path of the selected worktree.
    pub worktree: PathBuf,
}

/// Something the user did in the UI.
#[derive(Clone, Debug)]
pub enum Action {
    /// A dirty worktree opens File status, a clean one opens History.
    OpenWorktree(PathBuf),
    ShowFileStatus,
    ShowHistory,
    SetTab(Tab),
    /// Select a commit (the null id selects "Uncommitted changes").
    Select(Oid),
    /// Show a commit in History, scrolled into view.
    Reveal(Oid),
    Refresh,
    /// Close this project's tab. Handled by the app.
    Close,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum RefKind {
    /// The branch (or detached HEAD) of the selected worktree.
    Head,
    Local,
    Remote,
    Tag,
}

/// A ref chip in the commit graph.
#[derive(Clone, Debug)]
pub struct RefLabel {
    pub name: String,
    pub kind: RefKind,
    /// Checked out in another worktree.
    pub in_worktree: bool,
}

enum Event {
    Snapshot(Result<RepoSnapshot, String>),
    History {
        generation: u64,
        result: Result<(Arc<Commits>, History), String>,
    },
    Details(Oid, Result<CommitDetails, String>),
}

pub struct Project {
    pub root: PathBuf,
    pub name: String,
    git: Git,
    pool: JobPool,
    tx: Sender<Event>,
    rx: Receiver<Event>,

    pub nav: Nav,
    pub snapshot: Option<RepoSnapshot>,
    /// Why the snapshot could not be loaded (only while there is none).
    pub error: Option<String>,
    pub loading: bool,
    refresh_queued: bool,
    first_load: bool,

    commits: Option<Arc<Commits>>,
    tips: Vec<Oid>,
    pub history: Option<History>,
    pub history_loading: bool,
    history_generation: u64,
    history_cancel: CancelToken,
    /// The commit the "Uncommitted changes" row sits on in the current history.
    history_parent: Option<Oid>,
    pub labels: HashMap<Oid, Vec<RefLabel>>,

    pub selected: Option<Oid>,
    pub details: Option<CommitDetails>,
    pub details_error: Option<String>,
    /// Set when the history list should scroll the selection into view.
    pub scroll_to_selected: Cell<bool>,

    /// Errors for the app to show as toasts.
    pub notices: Vec<String>,
}

impl Project {
    /// `opened` is the worktree to select first (the folder the user picked).
    pub fn new(root: PathBuf, opened: PathBuf, console: Console, pool: JobPool) -> Self {
        let (tx, rx) = crossbeam_channel::unbounded();
        let name = root
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| root.display().to_string());
        let mut project = Self {
            git: Git::new(&root, console),
            root,
            name,
            pool,
            tx,
            rx,
            nav: Nav {
                tab: Tab::Workspace,
                view: WorkspaceView::History,
                worktree: opened,
            },
            snapshot: None,
            error: None,
            loading: false,
            refresh_queued: false,
            first_load: true,
            commits: None,
            tips: Vec::new(),
            history: None,
            history_loading: false,
            history_generation: 0,
            history_cancel: CancelToken::default(),
            history_parent: None,
            labels: HashMap::new(),
            selected: None,
            details: None,
            details_error: None,
            scroll_to_selected: Cell::new(false),
            notices: Vec::new(),
        };
        project.refresh();
        project
    }

    /// Reload refs, worktrees and status; the history follows if refs moved.
    pub fn refresh(&mut self) {
        if self.loading {
            self.refresh_queued = true;
            return;
        }
        self.loading = true;
        let git = self.git.clone();
        self.pool.spawn(&self.tx, move || {
            Event::Snapshot(snapshot::load(&git).map_err(|e| format!("{e:#}")))
        });
    }

    pub fn is_dirty(&self) -> bool {
        self.snapshot.as_ref().is_some_and(RepoSnapshot::is_dirty)
    }

    /// The selected worktree.
    pub fn worktree(&self) -> Option<&Worktree> {
        let snap = self.snapshot.as_ref()?;
        snap.worktrees
            .iter()
            .find(|wt| same_path(&wt.path, &self.nav.worktree))
    }

    /// Apply results of finished jobs. Call once per frame.
    pub fn poll(&mut self) {
        while let Ok(event) = self.rx.try_recv() {
            match event {
                Event::Snapshot(result) => self.on_snapshot(result),
                Event::History { generation, result } => self.on_history(generation, result),
                Event::Details(id, result) => {
                    if self.selected == Some(id) {
                        match result {
                            Ok(d) => self.details = Some(d),
                            Err(e) => self.details_error = Some(e),
                        }
                    }
                }
            }
        }
    }

    fn on_snapshot(&mut self, result: Result<RepoSnapshot, String>) {
        self.loading = false;
        match result {
            Ok(snap) => {
                self.error = None;
                if !snap
                    .worktrees
                    .iter()
                    .any(|wt| same_path(&wt.path, &self.nav.worktree))
                    && let Some(main) = snap.main_worktree()
                {
                    self.nav.worktree = main.path.clone();
                }
                let tips = collect_tips(&snap);
                self.snapshot = Some(snap);
                if self.first_load {
                    self.first_load = false;
                    if self.worktree().is_some_and(|wt| wt.dirty() > 0) {
                        self.nav.view = WorkspaceView::FileStatus;
                    }
                }
                self.labels = build_labels(
                    self.snapshot.as_ref().expect("just set"),
                    &self.nav.worktree,
                );
                if tips != self.tips || self.commits.is_none() {
                    self.tips = tips;
                    self.load_history(true);
                } else if self.worktree_parent() != self.history_parent {
                    self.load_history(false);
                }
            }
            Err(e) => {
                if self.snapshot.is_none() {
                    self.error = Some(e);
                } else {
                    self.notices.push(e);
                }
            }
        }
        if self.refresh_queued {
            self.refresh_queued = false;
            self.refresh();
        }
    }

    /// HEAD of the selected worktree if it has uncommitted changes.
    fn worktree_parent(&self) -> Option<Oid> {
        self.worktree()
            .filter(|wt| wt.dirty() > 0)
            .and_then(|wt| wt.head)
    }

    /// Rebuild the graph; with `walk`, also re-read commits from disk.
    fn load_history(&mut self, walk: bool) {
        self.history_generation += 1;
        self.history_cancel.cancel();
        self.history_cancel = CancelToken::default();
        self.history_loading = true;
        let generation = self.history_generation;
        let parent = self.worktree_parent();
        self.history_parent = parent;
        let cached = if walk { None } else { self.commits.clone() };
        let root = self.root.clone();
        let tips = self.tips.clone();
        let cancel = self.history_cancel.clone();
        self.pool.spawn(&self.tx, move || {
            let result = (|| {
                let commits = match cached {
                    Some(c) => c,
                    None => Arc::new(
                        history::walk(&root, &tips, HISTORY_LIMIT, &cancel)
                            .map_err(|e| format!("{e:#}"))?,
                    ),
                };
                let history = History::build(&commits, parent);
                Ok((commits, history))
            })();
            Event::History { generation, result }
        });
    }

    fn on_history(&mut self, generation: u64, result: Result<(Arc<Commits>, History), String>) {
        if generation != self.history_generation {
            return;
        }
        self.history_loading = false;
        match result {
            Ok((commits, history)) => {
                self.commits = Some(commits);
                let keep = self
                    .selected
                    .is_some_and(|id| history.index.contains_key(&id));
                self.history = Some(history);
                if !keep {
                    self.select_head();
                }
            }
            Err(e) => {
                if e != "Cancelled" {
                    self.notices.push(format!("Could not load history: {e}"));
                }
            }
        }
    }

    /// Select the selected worktree's HEAD (or its uncommitted changes).
    fn select_head(&mut self) {
        let Some(history) = &self.history else { return };
        let target = match self.worktree() {
            Some(wt) if wt.dirty() > 0 => Some(null_oid()),
            Some(wt) => wt.head,
            None => None,
        }
        .filter(|id| history.index.contains_key(id))
        .or_else(|| history.commits.first().map(|c| c.id));
        if let Some(id) = target {
            self.select(id);
            self.scroll_to_selected.set(true);
        }
    }

    pub fn select(&mut self, id: Oid) {
        if self.selected == Some(id) && (self.details.is_some() || id.is_null()) {
            return;
        }
        self.selected = Some(id);
        self.details = None;
        self.details_error = None;
        if id.is_null() {
            return;
        }
        let git = self.git.clone();
        self.pool.spawn(&self.tx, move || {
            Event::Details(id, details::load(&git, id).map_err(|e| format!("{e:#}")))
        });
    }

    pub fn apply(&mut self, action: Action) {
        match action {
            Action::OpenWorktree(path) => {
                self.nav.worktree = path;
                self.nav.tab = Tab::Workspace;
                let dirty = self.worktree().is_some_and(|wt| wt.dirty() > 0);
                self.nav.view = if dirty {
                    WorkspaceView::FileStatus
                } else {
                    WorkspaceView::History
                };
                if let Some(snap) = &self.snapshot {
                    self.labels = build_labels(snap, &self.nav.worktree);
                }
                if self.history.is_some() && self.worktree_parent() != self.history_parent {
                    self.load_history(false);
                } else {
                    self.select_head();
                }
            }
            Action::ShowFileStatus => {
                self.nav.tab = Tab::Workspace;
                self.nav.view = WorkspaceView::FileStatus;
            }
            Action::ShowHistory => {
                self.nav.tab = Tab::Workspace;
                self.nav.view = WorkspaceView::History;
            }
            Action::SetTab(tab) => self.nav.tab = tab,
            Action::Select(id) => self.select(id),
            Action::Reveal(id) => {
                self.nav.tab = Tab::Workspace;
                self.nav.view = WorkspaceView::History;
                self.select(id);
                self.scroll_to_selected.set(true);
            }
            Action::Refresh => self.refresh(),
            Action::Close => {}
        }
    }
}

impl Drop for Project {
    fn drop(&mut self) {
        self.history_cancel.cancel();
    }
}

/// Every commit a ref or worktree points at: the starting points of the walk.
fn collect_tips(snap: &RepoSnapshot) -> Vec<Oid> {
    let mut tips: Vec<Oid> = snap
        .branches
        .iter()
        .map(|b| b.target)
        .chain(snap.remote_branches.iter().map(|b| b.target))
        .chain(snap.tags.iter().map(|t| t.target))
        .chain(snap.worktrees.iter().filter_map(|wt| wt.head))
        .collect();
    tips.sort();
    tips.dedup();
    tips
}

fn build_labels(snap: &RepoSnapshot, selected: &Path) -> HashMap<Oid, Vec<RefLabel>> {
    let mut labels: HashMap<Oid, Vec<RefLabel>> = HashMap::new();
    let current = snap
        .worktrees
        .iter()
        .find(|wt| same_path(&wt.path, selected));
    let current_branch = current.and_then(|wt| wt.branch.as_deref());
    for branch in &snap.branches {
        let head = current_branch == Some(branch.name.as_str());
        labels.entry(branch.target).or_default().push(RefLabel {
            name: branch.name.clone(),
            kind: if head { RefKind::Head } else { RefKind::Local },
            in_worktree: !head && snap.worktree_for_branch(&branch.name).is_some(),
        });
    }
    if let Some(wt) = current.filter(|wt| wt.branch.is_none())
        && let Some(head) = wt.head
    {
        labels.entry(head).or_default().push(RefLabel {
            name: "HEAD".to_owned(),
            kind: RefKind::Head,
            in_worktree: false,
        });
    }
    for remote in &snap.remote_branches {
        labels.entry(remote.target).or_default().push(RefLabel {
            name: remote.name.clone(),
            kind: RefKind::Remote,
            in_worktree: false,
        });
    }
    for tag in &snap.tags {
        labels.entry(tag.target).or_default().push(RefLabel {
            name: tag.name.clone(),
            kind: RefKind::Tag,
            in_worktree: false,
        });
    }
    for list in labels.values_mut() {
        list.sort_by(|a, b| a.kind.cmp(&b.kind).then_with(|| a.name.cmp(&b.name)));
    }
    labels
}
