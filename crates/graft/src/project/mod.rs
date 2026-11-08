//! One open repository (a project tab): its snapshot, history, selection, and
//! the background jobs that load them. The UI reads this state and sends
//! [`Action`]s; nothing here blocks on git.
//!
//! Each feature starts its jobs and handles their results in one place: a
//! job's result comes back as a closure that updates the project, applied on
//! the UI thread by [`Project::poll`].

mod changes;
pub mod comments;
mod conflicts;
mod file_history;
mod ops;
mod preview;
mod refs;
mod remote;
mod review;
mod settings;
mod slash;
mod stashes;
mod worktrees;

use std::cell::Cell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use crossbeam_channel::{Receiver, Sender};
use graft_core::console::Console;
use graft_core::details::{self, CommitDetails};
use graft_core::history::{self, Commit, History};
use graft_core::ops::LineOp;
use graft_core::stash::{Stash, StashFile};
use graft_core::watch::Watcher;
use graft_core::{
    CancelToken, Git, JobPool, Oid, Operation, Panicked, RepoSnapshot, Waker, Worktree,
    is_cancelled, null_oid, snapshot,
};

pub use changes::{CommitForm, DiffView, FileKey};
pub use conflicts::{ConflictView, merge_dialog};
pub use file_history::{FileHistoryMode, FileHistoryView};
pub use refs::{Dialog, DialogKind};
pub use remote::RemoteOp;
pub use review::{ReviewState, ReviewView};
pub use slash::{SlashView, SlashedBranch};
pub use worktrees::{BranchMode, NewWorktreeForm, checkout_choices};

use crate::running::Running;
use crate::settings::Settings;

/// Commits loaded into the graph. Enough for years of history in big repos.
const HISTORY_LIMIT: usize = 100_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tab {
    Workspace,
    Review,
    Stashes,
}

impl Tab {
    pub fn label(self) -> &'static str {
        match self {
            Tab::Workspace => "Workspace",
            Tab::Review => "Review",
            Tab::Stashes => "Stashes",
        }
    }
}

/// What the Workspace tab shows in its main area.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WorkspaceView {
    FileStatus,
    History,
    /// The commits that changed one file (see `Project::file_history`).
    FileHistory,
}

impl WorkspaceView {
    pub fn label(self) -> &'static str {
        match self {
            WorkspaceView::FileStatus => "File status",
            WorkspaceView::History => "History",
            WorkspaceView::FileHistory => "File history",
        }
    }
}

/// Where the user is inside a project.
#[derive(Clone, Debug)]
pub struct Nav {
    pub tab: Tab,
    pub view: WorkspaceView,
    /// Path of the selected worktree.
    pub worktree: PathBuf,
}

/// Something the user did in this project.
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

    // File status
    SelectFile(FileKey),
    /// Toggle a changed line; with shift, select the range from the anchor.
    ClickLine {
        hunk: usize,
        line: usize,
        shift: bool,
    },
    /// The pointer was dragged onto a line while selecting.
    DragLine {
        hunk: usize,
        line: usize,
    },
    ClearLines,
    /// Show these unchanged lines (their numbers in the old and the new
    /// file) in the diff that shows `key`.
    ShowLines {
        key: FileKey,
        old: std::ops::Range<u32>,
        new: std::ops::Range<u32>,
    },
    HunkOp(usize, LineOp),
    LinesOp(LineOp),
    StageFiles(Vec<String>),
    UnstageFiles(Vec<String>),
    Discard {
        tracked: Vec<String>,
        untracked: Vec<String>,
    },
    Commit,
    ToggleAmend,
    Remote(RemoteOp),
    CancelOperation,
    /// Ask before doing something destructive.
    Ask(Box<Confirm>),
    ConfirmYes,
    ConfirmNo,
    /// Open a file of the selected worktree with its default program.
    OpenPath(String),
    /// Show a file of the selected worktree in Explorer.
    RevealPath(String),

    // Worktrees
    /// Open the New worktree dialog, optionally to check out this branch.
    NewWorktree(Option<String>),
    CloseNewWorktree,
    BrowseWorktreeFolder,
    CreateWorktree,
    RemoveWorktree {
        path: PathBuf,
        force: bool,
    },
    LockWorktree {
        path: PathBuf,
        locked: bool,
    },
    PruneWorktrees,
    OpenTerminal(PathBuf),
    OpenFolder(PathBuf),

    // Diffs of commits and stashes
    /// Show one file of a commit or stash.
    SelectCommitFile(FileKey),
    CloseCommitFile,

    // Stashes
    SelectStash(Oid),
    /// Apply (or pop) a stash to a worktree.
    ApplyStash {
        id: Oid,
        worktree: PathBuf,
        pop: bool,
    },
    DropStash(Oid),

    // Branches, tags, commits
    OpenDialog(DialogKind),
    CloseDialog,
    SubmitDialog,
    /// Switch the selected worktree to a local branch.
    Checkout(String),
    /// Stash the selected worktree's changes, then switch to the branch.
    StashAndCheckout {
        branch: String,
        /// Also stash untracked files (they were in the way).
        untracked: bool,
    },
    /// Create `local` tracking `remote` (e.g. origin/x) and switch to it.
    CheckoutRemote {
        local: String,
        remote: String,
    },
    CheckoutDetached(Oid),
    DeleteBranch {
        name: String,
        force: bool,
    },
    DeleteTag(String),
    RemoveRemote(String),
    CherryPick(Oid),
    Revert(Oid),
    /// Undo the selected worktree's last commit, keeping its changes staged.
    UndoCommit,

    // Merges (the Merge dialog) and conflicts
    /// Rebase the selected worktree's branch onto this.
    Rebase(String),
    /// Pick (or clear) the resolution of one conflict of the open file.
    ResolveConflict {
        index: usize,
        resolution: Option<graft_core::conflicts::Resolution>,
    },
    /// Edit one conflict's result by hand.
    EditConflict(usize),
    /// Write the open file's resolved content and stage it.
    SaveResolution,
    /// Resolve a whole file with one side.
    TakeSide {
        path: String,
        ours: bool,
    },
    /// The user fixed the file in an editor: stage it.
    MarkResolved(String),
    ContinueOp(Operation),
    AbortOp {
        kind: Operation,
        confirmed: bool,
    },

    // Review
    SelectReviewFile(String),
    ToggleViewed(String),
    SetReviewBase(String),
    ToggleReviewSplit,
    /// Pick a line of the review diff; `extend` grows the pick to it.
    PickLine {
        at: (usize, usize),
        side: Option<comments::Side>,
        extend: bool,
    },
    /// Save the draft as a comment on the picked lines (or the edited one).
    SaveComment,
    /// Drop the pick and the draft.
    CancelComment,
    EditComment(u64),
    DeleteComment(u64),
    /// Remove every line comment and file note of this review.
    ClearReviewComments,
    /// Merge the reviewed branch into its base, in the worktree that has the
    /// base checked out.
    MergeReviewed {
        worktree: PathBuf,
        branch: String,
        into: String,
    },
    OpenInIde(PathBuf),

    // Slash
    /// Fetch, scan and show the Slash dialog.
    OpenSlash,
    CloseSlash,
    ToggleSlashItem(usize),
    RunSlash,
    /// Bring back an entry of "Recently slashed".
    RestoreSlashed(usize),

    // Preview in main
    /// Show this worktree's state (commit + uncommitted changes) in main.
    PreviewInMain(PathBuf),
    /// Show the previewed worktree's current state again.
    UpdatePreview,
    RestoreMain,

    // File history
    /// Show the history of this file (path in the selected worktree).
    OpenFileHistory(String),
    CloseFileHistory,
    SelectFileCommit(usize),
    SetFileHistoryMode(FileHistoryMode),

    // Settings
    LoadIdentity,
    SaveIdentity(graft_core::identity::Identity),
}

/// A question to confirm before doing something destructive.
#[derive(Clone, Debug)]
pub struct Confirm {
    pub title: String,
    pub body: String,
    pub confirm_label: String,
    pub action: Action,
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
    pub checked_out: CheckedOut,
}

/// Where a branch is checked out; the sidebar and the graph color it alike.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CheckedOut {
    Nowhere,
    /// In the main folder (the repository's own working tree).
    Main,
    /// In a linked worktree.
    Worktree,
}

impl CheckedOut {
    pub fn of(wt: Option<&graft_core::Worktree>) -> Self {
        match wt {
            None => Self::Nowhere,
            Some(wt) if wt.is_main => Self::Main,
            Some(_) => Self::Worktree,
        }
    }
}

/// What Graft remembers about a repository between runs.
#[derive(Clone, Debug, Default)]
pub struct RepoMemory {
    /// Viewed marks, notes, comments and base branch per worktree.
    pub reviews: HashMap<PathBuf, ReviewState>,
    /// Deleted branches, newest first.
    pub slashed: Vec<SlashedBranch>,
}

impl RepoMemory {
    /// Without the reviews that remember nothing.
    pub fn pruned(mut self) -> Self {
        self.reviews.retain(|_, state| !state.is_empty());
        self
    }
}

/// What to select once the snapshot or history being loaded arrives.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Pending {
    /// The project just opened: show the view its worktree calls for.
    Open,
    /// HEAD of the selected worktree moved: select it in the new history.
    Head,
    /// A search hit revealed a commit while a refresh ran: keep it selected.
    Revealed,
}

/// A finished job's result, applied to the project on the UI thread.
struct Done(Box<dyn FnOnce(&mut Project) + Send>);

impl From<Panicked> for Done {
    fn from(panic: Panicked) -> Self {
        Done(Box::new(move |project| {
            project.op = None;
            project.notices.push(panic.0);
        }))
    }
}

/// A job's error as text for the screen.
fn readable<T>(result: anyhow::Result<T>) -> Result<T, String> {
    result.map_err(|e| format!("{e:#}"))
}

pub struct Project {
    pub root: PathBuf,
    pub name: String,
    git: Git,
    pool: JobPool,
    tx: Sender<Done>,
    rx: Receiver<Done>,

    pub nav: Nav,
    /// Shared, so screens can hold it without copying it every frame.
    pub snapshot: Option<Arc<RepoSnapshot>>,
    /// Why the snapshot could not be loaded (only while there is none).
    pub error: Option<String>,
    pub loading: bool,
    refresh_queued: bool,
    pending: Option<Pending>,
    /// Numbers loads, so a result that arrives after a newer load of the
    /// same view started is dropped.
    tickets: u64,

    commits: Option<Arc<Vec<Commit>>>,
    tips: Vec<Oid>,
    pub history: Option<History>,
    pub history_loading: bool,
    history_generation: u64,
    history_cancel: CancelToken,
    /// The commit the "Uncommitted changes" row sits on in the current history.
    history_parent: Option<Oid>,
    /// Bumped whenever a new snapshot or history arrives.
    data_generation: u64,
    pub labels: HashMap<Oid, Vec<RefLabel>>,

    pub selected: Option<Oid>,
    pub details: Option<CommitDetails>,
    pub details_error: Option<String>,
    /// Set when the history list should scroll the selection into view.
    pub scroll_to_selected: Cell<bool>,

    pub diff_view: Option<DiffView>,
    /// A file of the selected commit or stash, read-only.
    pub commit_diff: Option<DiffView>,
    /// `None` until the Stashes tab was opened.
    pub stashes: Option<Result<Vec<Stash>, String>>,
    pub selected_stash: Option<Oid>,
    pub stash_files: Option<Result<Vec<StashFile>, String>>,
    /// The resolver, while the selected file has conflicts.
    pub conflict: Option<ConflictView>,
    /// The Review tab, once opened.
    pub review: Option<ReviewView>,
    memory: RepoMemory,
    /// The Slash dialog while open.
    pub slash: Option<SlashView>,
    /// File history, while WorkspaceView::FileHistory shows it.
    pub file_history: Option<FileHistoryView>,
    settings: Settings,
    /// The commit identity, loaded for the Settings dialog.
    pub identity: Option<Result<graft_core::identity::Identity, String>>,
    last_fetch: Instant,
    auto_fetching: bool,
    pub commit_form: CommitForm,
    /// A git command that changes things, while it runs.
    op: Option<Running>,
    /// A destructive action waiting for the user's OK.
    pub confirm: Option<Confirm>,

    /// Errors for the app to show as toasts.
    pub notices: Vec<String>,
    /// Confirmations for the app to show as toasts.
    pub infos: Vec<String>,

    pub new_worktree: Option<NewWorktreeForm>,
    pub dialog: Option<Dialog>,
    /// Select this worktree once a snapshot has it (just created).
    pending_open: Option<PathBuf>,
    watcher: Option<Watcher>,
    /// The folders `watcher` was asked to watch.
    watched: Vec<PathBuf>,
}

impl Project {
    /// `opened` is the worktree to select first (the folder the user picked).
    pub fn new(
        root: PathBuf,
        opened: PathBuf,
        settings: Settings,
        memory: RepoMemory,
        console: Console,
        pool: JobPool,
    ) -> Self {
        let (tx, rx) = crossbeam_channel::unbounded();
        let mut project = Self {
            git: Git::new(&root, console),
            name: graft_core::paths::folder_name(&root),
            root,
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
            pending: Some(Pending::Open),
            tickets: 0,
            commits: None,
            tips: Vec::new(),
            history: None,
            history_loading: false,
            history_generation: 0,
            history_cancel: CancelToken::default(),
            history_parent: None,
            data_generation: 0,
            labels: HashMap::new(),
            selected: None,
            details: None,
            details_error: None,
            scroll_to_selected: Cell::new(false),
            diff_view: None,
            commit_diff: None,
            stashes: None,
            selected_stash: None,
            stash_files: None,
            conflict: None,
            review: None,
            memory,
            slash: None,
            file_history: None,
            settings,
            identity: None,
            last_fetch: Instant::now(),
            auto_fetching: false,
            commit_form: CommitForm::default(),
            op: None,
            confirm: None,
            notices: Vec::new(),
            infos: Vec::new(),
            new_worktree: None,
            dialog: None,
            pending_open: None,
            watcher: None,
            watched: Vec::new(),
        };
        project.refresh();
        project
    }

    /// Run `job` on the pool; `done` gets its result on the UI thread.
    fn spawn<T: Send + 'static>(
        &self,
        job: impl FnOnce() -> T + Send + 'static,
        done: impl FnOnce(&mut Project, T) + Send + 'static,
    ) {
        self.pool.spawn(&self.tx, move || {
            let result = job();
            Done(Box::new(move |project| done(project, result)))
        });
    }

    /// Like [`Self::spawn`], on a thread of its own: for jobs that wait on
    /// the network or the user, so they cannot hold up the pool.
    fn spawn_long<T: Send + 'static>(
        &self,
        job: impl FnOnce(Waker) -> T + Send + 'static,
        done: impl FnOnce(&mut Project, T) + Send + 'static,
    ) {
        self.pool.spawn_long(&self.tx, move |waker| {
            let result = job(waker);
            Done(Box::new(move |project| done(project, result)))
        });
    }

    fn ticket(&mut self) -> u64 {
        self.tickets += 1;
        self.tickets
    }

    /// Apply results of finished jobs. Call once per frame.
    pub fn poll(&mut self) {
        self.maybe_auto_fetch();
        while let Ok(Done(apply)) = self.rx.try_recv() {
            apply(self);
        }
    }

    /// What Graft remembers about this repository.
    pub fn memory(&self) -> &RepoMemory {
        &self.memory
    }

    pub fn settings(&self) -> &Settings {
        &self.settings
    }

    /// Take on changed app settings.
    pub fn set_settings(&mut self, settings: &Settings) {
        let reload = settings.whole_file != self.settings.whole_file;
        self.settings = settings.clone();
        if reload {
            self.load_view(changes::Slot::Status);
            self.load_view(changes::Slot::ReadOnly);
        }
    }

    /// Reload refs, worktrees and status; the history follows if refs moved.
    pub fn refresh(&mut self) {
        if self.loading {
            self.refresh_queued = true;
            return;
        }
        self.loading = true;
        let git = self.git.clone();
        self.spawn(
            move || snapshot::load(&git),
            |p, result| p.on_snapshot(readable(result)),
        );
    }

    pub fn is_dirty(&self) -> bool {
        self.snapshot.as_ref().is_some_and(|s| s.is_dirty())
    }

    /// Opened, but nothing loaded yet.
    pub fn is_starting(&self) -> bool {
        self.snapshot.is_none() && self.error.is_none()
    }

    /// The window title while this project is shown.
    pub fn title(&self) -> String {
        match self.worktree() {
            Some(wt) if !wt.is_main => format!("{} · {} — Graft", self.name, wt.label()),
            _ => format!("{} — Graft", self.name),
        }
    }

    /// What the status bar shows as running, and whether it can be cancelled.
    pub fn activity(&self) -> Option<(String, bool)> {
        if let Some(op) = &self.op {
            Some((op.status(), op.can_cancel()))
        } else if self.loading {
            Some((format!("Refreshing {}…", self.name), false))
        } else if self.history_loading {
            Some(("Loading history…".to_owned(), false))
        } else {
            None
        }
    }

    /// Changes whenever what a search finds in this project can change.
    pub fn search_stamp(&self) -> (u64, &Path) {
        (self.data_generation, &self.nav.worktree)
    }

    /// The selected worktree.
    pub fn worktree(&self) -> Option<&Worktree> {
        self.snapshot.as_ref()?.worktree_at(&self.nav.worktree)
    }

    fn on_snapshot(&mut self, result: Result<RepoSnapshot, String>) {
        self.loading = false;
        match result {
            Ok(snap) => self.take_snapshot(snap),
            Err(e) if self.snapshot.is_none() => self.error = Some(e),
            Err(e) => self.notices.push(e),
        }
        if self.refresh_queued {
            self.refresh_queued = false;
            self.refresh();
        }
    }

    fn take_snapshot(&mut self, snap: RepoSnapshot) {
        self.error = None;
        let tips = snap.tips();
        let had_snapshot = self.snapshot.is_some();
        let old_head = self.worktree().and_then(|wt| wt.head);
        self.snapshot = Some(Arc::new(snap));
        self.data_generation += 1;
        if self.worktree().is_none()
            && let Some(main) = self.main_worktree()
        {
            let main = main.path.clone();
            self.set_worktree(main);
        }
        let head_moved = had_snapshot && self.worktree().and_then(|wt| wt.head) != old_head;
        let pending = self.pending.take();
        if pending == Some(Pending::Open) {
            self.nav.view = self.default_view();
        }
        self.labels = build_labels(
            self.snapshot.as_ref().expect("just set"),
            &self.nav.worktree,
        );
        let reload = if tips != self.tips || self.commits.is_none() {
            self.tips = tips;
            self.load_history(true);
            true
        } else if self.worktree_parent() != self.history_parent {
            self.load_history(false);
            true
        } else {
            false
        };
        // A HEAD to select waits for the history that is loading.
        let head = pending == Some(Pending::Head);
        if head || (head_moved && pending != Some(Pending::Revealed)) {
            if reload || head {
                self.pending = Some(Pending::Head);
            } else {
                self.select_head();
            }
        }
        self.sync_file_selection();
        self.sync_watcher();
        if self.stashes.is_some() || self.nav.tab == Tab::Stashes {
            self.load_stashes();
        }
        if self.nav.tab == Tab::Review {
            self.load_review();
        }
        if let Some(path) = self.pending_open.take() {
            let there = self
                .snapshot
                .as_ref()
                .is_some_and(|s| s.worktree_at(&path).is_some());
            if there {
                self.open_worktree(path);
            } else {
                self.pending_open = Some(path);
            }
        }
    }

    /// HEAD of the selected worktree if it has uncommitted changes.
    fn worktree_parent(&self) -> Option<Oid> {
        self.worktree()
            .filter(|wt| wt.is_dirty())
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
        self.spawn(
            move || -> anyhow::Result<_> {
                let commits = match cached {
                    Some(c) => c,
                    None => Arc::new(history::walk(&root, &tips, HISTORY_LIMIT, &cancel)?),
                };
                let history = History::build(&commits, parent);
                Ok((commits, history))
            },
            move |p, result| p.on_history(generation, result),
        );
    }

    fn on_history(&mut self, generation: u64, result: anyhow::Result<(Arc<Vec<Commit>>, History)>) {
        if generation != self.history_generation {
            return;
        }
        self.history_loading = false;
        let head = self.pending == Some(Pending::Head);
        if head {
            self.pending = None;
        }
        match result {
            Ok((commits, history)) => {
                self.commits = Some(commits);
                let keep = self
                    .selected
                    .is_some_and(|id| history.index.contains_key(&id));
                self.history = Some(history);
                self.data_generation += 1;
                if !keep || head {
                    self.select_head();
                }
            }
            Err(e) if is_cancelled(&e) => {}
            Err(e) => self.notices.push(format!("Could not load history: {e:#}")),
        }
    }

    /// Select the selected worktree's HEAD (or its uncommitted changes).
    fn select_head(&mut self) {
        let Some(history) = &self.history else { return };
        let target = match self.worktree() {
            Some(wt) if wt.is_dirty() => Some(null_oid()),
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
        self.commit_diff = None;
        self.details_error = None;
        if id.is_null() {
            return;
        }
        let git = self.git.clone();
        self.spawn(
            move || details::load(&git, id),
            move |p, result| {
                if p.selected == Some(id) {
                    match readable(result) {
                        Ok(d) => p.details = Some(d),
                        Err(e) => p.details_error = Some(e),
                    }
                }
            },
        );
    }

    fn reveal(&mut self, id: Oid) {
        self.pending = self.loading.then_some(Pending::Revealed);
        self.show_view(WorkspaceView::History);
        self.select(id);
        self.scroll_to_selected.set(true);
    }

    /// File status for a worktree with changes, else History.
    fn default_view(&self) -> WorkspaceView {
        if self.worktree().is_some_and(Worktree::is_dirty) {
            WorkspaceView::FileStatus
        } else {
            WorkspaceView::History
        }
    }

    /// Make `path` the selected worktree: what was shown for the old one
    /// (its diff, conflict, commit message, ref chips) goes.
    fn set_worktree(&mut self, path: PathBuf) {
        self.nav.worktree = path;
        self.diff_view = None;
        self.conflict = None;
        self.commit_form = CommitForm::default();
        if let Some(snap) = &self.snapshot {
            self.labels = build_labels(snap, &self.nav.worktree);
        }
        self.sync_file_selection();
    }

    /// Select a worktree and its HEAD: a dirty one opens File status, a
    /// clean one History. On the Review tab, review it instead.
    fn open_worktree(&mut self, path: PathBuf) {
        self.set_worktree(path);
        if self.nav.tab == Tab::Review {
            self.load_review();
        } else {
            self.show_view(self.default_view());
        }
        if self.history.is_some() && self.worktree_parent() != self.history_parent {
            self.pending = Some(Pending::Head);
            self.load_history(false);
        } else {
            self.select_head();
        }
    }

    fn set_tab(&mut self, tab: Tab) {
        self.nav.tab = tab;
        self.commit_diff = None;
        match tab {
            Tab::Workspace => {}
            Tab::Stashes => self.load_stashes(),
            Tab::Review => {
                if let Some(view) = &mut self.review {
                    view.selected = None;
                }
                self.load_review();
            }
        }
    }

    fn show_view(&mut self, view: WorkspaceView) {
        self.nav.tab = Tab::Workspace;
        self.nav.view = view;
    }

    fn open_folder(&mut self, dir: &Path) {
        if let Err(e) = crate::launch::open_folder(dir) {
            self.notices
                .push(format!("Could not open {}: {e}", dir.display()));
        }
    }

    pub fn apply(&mut self, action: Action) {
        match action {
            Action::OpenWorktree(path) => self.open_worktree(path),
            Action::ShowFileStatus => self.show_view(WorkspaceView::FileStatus),
            Action::ShowHistory => self.show_view(WorkspaceView::History),
            Action::SetTab(tab) => self.set_tab(tab),
            Action::Select(id) => self.select(id),
            Action::Reveal(id) => self.reveal(id),
            Action::Refresh => self.refresh(),

            Action::SelectFile(key) => self.select_file(key),
            Action::ClickLine { hunk, line, shift } => self.click_line(hunk, line, shift),
            Action::DragLine { hunk, line } => self.drag_line(hunk, line),
            Action::ClearLines => self.clear_lines(),
            Action::ShowLines { key, old, new } => self.show_lines(&key, old, new),
            Action::HunkOp(hunk, op) => self.lines_op(op, Some(hunk)),
            Action::LinesOp(op) => self.lines_op(op, None),
            Action::StageFiles(paths) => self.stage_files(paths),
            Action::UnstageFiles(paths) => self.unstage_files(paths),
            Action::Discard { tracked, untracked } => self.discard(tracked, untracked),
            Action::Commit => self.commit(),
            Action::ToggleAmend => self.toggle_amend(),
            Action::Remote(op) => {
                self.remote(op);
            }
            Action::CancelOperation => self.cancel_op(),
            Action::Ask(confirm) => self.confirm = Some(*confirm),
            Action::ConfirmYes => {
                if let Some(confirm) = self.confirm.take() {
                    self.apply(confirm.action);
                }
            }
            Action::ConfirmNo => self.confirm = None,
            Action::OpenPath(path) => self.open_path(&path),
            Action::RevealPath(path) => self.reveal_path(&path),

            Action::NewWorktree(branch) => self.open_worktree_form(branch),
            Action::CloseNewWorktree => self.close_worktree_form(),
            Action::BrowseWorktreeFolder => self.browse_worktree_folder(),
            Action::CreateWorktree => self.create_worktree(),
            Action::RemoveWorktree { path, force } => self.remove_worktree(path, force),
            Action::LockWorktree { path, locked } => self.lock_worktree(path, locked),
            Action::PruneWorktrees => self.prune_worktrees(),
            Action::OpenTerminal(dir) => crate::launch::open_terminal(&dir),
            Action::OpenFolder(dir) => self.open_folder(&dir),

            Action::SelectCommitFile(key) => self.select_commit_file(key),
            Action::CloseCommitFile => self.commit_diff = None,

            Action::SelectStash(id) => self.select_stash(id),
            Action::ApplyStash { id, worktree, pop } => self.apply_stash(id, worktree, pop),
            Action::DropStash(id) => self.drop_stash(id),

            Action::OpenDialog(kind) => self.open_dialog(kind),
            Action::CloseDialog => self.close_dialog(),
            Action::SubmitDialog => self.submit_dialog(),
            Action::Checkout(branch) => self.switch(branch),
            Action::StashAndCheckout { branch, untracked } => {
                self.stash_and_switch(branch, untracked)
            }
            Action::CheckoutRemote { local, remote } => self.switch_tracking(local, remote),
            Action::CheckoutDetached(id) => self.switch_detached(id),
            Action::DeleteBranch { name, force } => self.delete_branch(name, force),
            Action::DeleteTag(name) => self.delete_tag(name),
            Action::RemoveRemote(name) => self.remove_remote(name),
            Action::CherryPick(id) => self.cherry_pick(id),
            Action::Revert(id) => self.revert(id),
            Action::UndoCommit => self.undo_commit(),

            Action::Rebase(onto) => self.rebase(onto),
            Action::ResolveConflict { index, resolution } => {
                self.resolve_conflict(index, resolution)
            }
            Action::EditConflict(index) => self.edit_conflict(index),
            Action::SaveResolution => self.save_resolution(),
            Action::TakeSide { path, ours } => self.take_side(path, ours),
            Action::MarkResolved(path) => self.mark_resolved(path),
            Action::ContinueOp(kind) => self.continue_op(kind),
            Action::AbortOp { kind, confirmed } => self.abort_op(kind, confirmed),

            Action::SelectReviewFile(path) => self.open_review_file(path, false),
            Action::ToggleViewed(path) => self.toggle_viewed(path),
            Action::SetReviewBase(base) => self.set_review_base(base),
            Action::ToggleReviewSplit => self.toggle_review_split(),
            Action::PickLine { at, side, extend } => self.pick_line(at, side, extend),
            Action::SaveComment => self.save_comment(),
            Action::CancelComment => self.cancel_comment(),
            Action::EditComment(id) => self.edit_comment(id),
            Action::DeleteComment(id) => self.delete_comment(id),
            Action::ClearReviewComments => self.clear_review_comments(),
            Action::MergeReviewed {
                worktree,
                branch,
                into,
            } => self.merge_reviewed(worktree, branch, into),
            Action::OpenInIde(path) => {
                crate::launch::open_in_ide(&path, &self.settings.ide_command)
            }

            Action::OpenSlash => self.open_slash(),
            Action::CloseSlash => self.close_slash(),
            Action::ToggleSlashItem(i) => self.toggle_slash_item(i),
            Action::RunSlash => self.run_slash(),
            Action::RestoreSlashed(i) => self.restore_slashed(i),

            Action::PreviewInMain(worktree) => self.preview_in_main(worktree),
            Action::UpdatePreview => self.update_preview(),
            Action::RestoreMain => self.restore_main(),

            Action::OpenFileHistory(path) => self.open_file_history(path),
            Action::CloseFileHistory => self.close_file_history(),
            Action::SelectFileCommit(i) => self.select_file_commit(i),
            Action::SetFileHistoryMode(mode) => self.set_file_history_mode(mode),

            Action::LoadIdentity => self.load_identity(),
            Action::SaveIdentity(identity) => self.save_identity(identity),
        }
    }
}

impl Drop for Project {
    fn drop(&mut self) {
        self.history_cancel.cancel();
    }
}

/// The ref chips of each commit; the branch of the `selected` worktree is
/// its HEAD.
fn build_labels(snap: &RepoSnapshot, selected: &Path) -> HashMap<Oid, Vec<RefLabel>> {
    let mut labels: HashMap<Oid, Vec<RefLabel>> = HashMap::new();
    let current = snap.worktree_at(selected);
    let current_branch = current.and_then(|wt| wt.branch.as_deref());
    for branch in &snap.branches {
        let head = current_branch == Some(branch.name.as_str());
        labels.entry(branch.target).or_default().push(RefLabel {
            name: branch.name.clone(),
            kind: if head { RefKind::Head } else { RefKind::Local },
            checked_out: CheckedOut::of(snap.worktree_for_branch(&branch.name)),
        });
    }
    if let Some(wt) = current.filter(|wt| wt.branch.is_none())
        && let Some(head) = wt.head
    {
        labels.entry(head).or_default().push(RefLabel {
            name: "HEAD".to_owned(),
            kind: RefKind::Head,
            checked_out: CheckedOut::of(Some(wt)),
        });
    }
    for remote in &snap.remote_branches {
        labels.entry(remote.target).or_default().push(RefLabel {
            name: remote.name.clone(),
            kind: RefKind::Remote,
            checked_out: CheckedOut::Nowhere,
        });
    }
    for tag in &snap.tags {
        labels.entry(tag.target).or_default().push(RefLabel {
            name: tag.name.clone(),
            kind: RefKind::Tag,
            checked_out: CheckedOut::Nowhere,
        });
    }
    for list in labels.values_mut() {
        list.sort_by(|a, b| a.kind.cmp(&b.kind).then_with(|| a.name.cmp(&b.name)));
    }
    labels
}
