//! One open repository (a project tab): its snapshot, history, selection, and
//! the background jobs that load them. The UI reads this state and sends
//! [`Action`]s; nothing here blocks on git.

mod changes;
pub mod comments;
mod conflicts;
mod file_history;
mod preview;
mod refs;
mod review;
mod settings;
mod slash;
mod stashes;
mod worktrees;

use std::cell::Cell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crossbeam_channel::{Receiver, Sender};
use graft_core::conflicts::{ConflictFile, Kind};
use graft_core::details::{self, CommitDetails};
use graft_core::diff::FileDiff;
use graft_core::history::{self, Commits, History};
use graft_core::jobs::JobPool;
use graft_core::ops::LineOp;
use graft_core::stash::{Stash, StashFile};
use graft_core::watch::Watcher;
use graft_core::{
    CancelToken, Console, Git, Oid, RepoSnapshot, Worktree, null_oid, same_path, snapshot,
};

pub use changes::{CommitForm, Confirm, DiffView, FileKey, Highlights, Operation, RemoteOp};
pub use conflicts::{ConflictView, merge_dialog};
pub use file_history::{FileHistoryMode, FileHistoryView};
pub use refs::{Dialog, DialogKind};
pub use review::{ReviewState, ReviewView};
pub use slash::{SlashView, SlashedBranch};
pub use worktrees::{BranchMode, NewWorktreeForm, checkout_choices};

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
    /// The commits that changed one file (see `Project::file_history`).
    FileHistory,
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

    // Merges and conflicts
    /// Merge `rev` into the selected worktree's branch (`into`, for messages).
    Merge {
        rev: String,
        no_ff: bool,
        into: String,
    },
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
    ContinueOp(Kind),
    AbortOp {
        kind: Kind,
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
    fn of(wt: Option<&graft_core::Worktree>) -> Self {
        match wt {
            None => Self::Nowhere,
            Some(wt) if wt.is_main => Self::Main,
            Some(_) => Self::Worktree,
        }
    }
}

enum Event {
    Snapshot(Result<RepoSnapshot, String>),
    History {
        generation: u64,
        result: Result<(Arc<Commits>, History), String>,
    },
    Details(Oid, Result<CommitDetails, String>),
    Diff {
        generation: u64,
        result: Result<(FileDiff, Highlights), String>,
    },
    LastMessage(Option<String>),
    OpDone {
        then: changes::Then,
        result: Result<Option<String>, String>,
    },
    CommitDiff {
        generation: u64,
        result: Result<(FileDiff, Highlights), String>,
    },
    /// Syntax colors for a big diff, after the diff itself.
    Syntax {
        generation: u64,
        syntax: graft_core::syntax::Syntax,
    },
    Stashes(Result<Vec<Stash>, String>),
    StashFiles(Oid, Result<Vec<StashFile>, String>),
    Conflict {
        generation: u64,
        result: Result<ConflictFile, String>,
    },
    Review {
        generation: u64,
        result: Result<graft_core::review::Review, String>,
    },
    SlashPlan(Result<graft_core::slash::Plan, String>),
    SlashDone(graft_core::slash::Outcome),
    Identity(Result<graft_core::identity::Identity, String>),
    AutoFetched(Result<(), String>),
    FileLog(
        String,
        Result<Vec<graft_core::file_log::FileCommit>, String>,
    ),
    Blame {
        generation: u64,
        result: Result<graft_core::file_log::Blame, String>,
    },
    /// The file watcher saw a change that matters.
    Changed,
    PickedFolder(Option<PathBuf>),
}

pub struct Project {
    pub root: PathBuf,
    pub name: String,
    git: Git,
    pool: JobPool,
    tx: Sender<Event>,
    rx: Receiver<Event>,

    pub nav: Nav,
    /// Shared, so screens can hold it without copying it every frame.
    pub snapshot: Option<Arc<RepoSnapshot>>,
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
    /// HEAD of the selected worktree moved: select it once history is rebuilt.
    reselect_head: bool,
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
    /// Viewed marks, notes and base branch per worktree. Saved by the app.
    pub reviews: HashMap<PathBuf, ReviewState>,
    /// A merge runs in this other worktree: show its conflicts if it stops.
    pending_conflicts: Option<PathBuf>,
    /// The Slash dialog while open.
    pub slash: Option<SlashView>,
    /// Deleted branches, newest first. Saved by the app.
    pub slashed: Vec<SlashedBranch>,
    /// File history, while WorkspaceView::FileHistory shows it.
    pub file_history: Option<FileHistoryView>,
    /// The app's settings; the app keeps this copy current.
    pub settings: crate::settings::Settings,
    /// The commit identity, loaded for the Settings dialog.
    pub identity: Option<Result<graft_core::identity::Identity, String>>,
    last_fetch: std::time::Instant,
    auto_fetching: bool,
    diff_generation: u64,
    pub commit_form: CommitForm,
    /// A git command that changes things, while it runs.
    pub op: Option<Operation>,
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
            reselect_head: false,
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
            reviews: HashMap::new(),
            pending_conflicts: None,
            slash: None,
            slashed: Vec::new(),
            file_history: None,
            settings: crate::settings::Settings::default(),
            identity: None,
            last_fetch: std::time::Instant::now(),
            auto_fetching: false,
            diff_generation: 0,
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
        self.snapshot.as_ref().is_some_and(|s| s.is_dirty())
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
        self.maybe_auto_fetch();
        while let Ok(event) = self.rx.try_recv() {
            match event {
                Event::Snapshot(result) => self.on_snapshot(result),
                Event::History { generation, result } => self.on_history(generation, result),
                Event::Diff { generation, result } => self.on_diff(generation, result),
                Event::LastMessage(message) => self.on_last_message(message),
                Event::OpDone { then, result } => self.on_op_done(then, result),
                Event::CommitDiff { generation, result } => self.on_commit_diff(generation, result),
                Event::Syntax { generation, syntax } => self.on_syntax(generation, syntax),
                Event::Stashes(result) => self.on_stashes(result),
                Event::Conflict { generation, result } => self.on_conflict(generation, result),
                Event::Review { generation, result } => self.on_review(generation, result),
                Event::SlashPlan(result) => self.on_slash_plan(result),
                Event::SlashDone(outcome) => self.on_slash_done(outcome),
                Event::Identity(result) => self.on_identity(result),
                Event::AutoFetched(result) => self.on_auto_fetched(result),
                Event::FileLog(path, result) => self.on_file_log(path, result),
                Event::Blame { generation, result } => self.on_blame(generation, result),
                Event::StashFiles(id, result) => {
                    if self.selected_stash == Some(id) {
                        self.stash_files = Some(result);
                    }
                }
                Event::Changed => {
                    if !self.is_operating() {
                        self.refresh();
                    }
                }
                Event::PickedFolder(folder) => self.on_picked_folder(folder),
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
                let had_snapshot = self.snapshot.is_some();
                let old_head = self.worktree().and_then(|wt| wt.head);
                self.snapshot = Some(Arc::new(snap));
                let head_moved = had_snapshot && self.worktree().and_then(|wt| wt.head) != old_head;
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
                if head_moved {
                    if reload {
                        self.reselect_head = true;
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
                    if self
                        .snapshot
                        .as_ref()
                        .is_some_and(|s| s.worktrees.iter().any(|wt| same_path(&wt.path, &path)))
                    {
                        self.open_worktree(path);
                    } else {
                        self.pending_open = Some(path);
                    }
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
                if !keep || std::mem::take(&mut self.reselect_head) {
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
        self.commit_diff = None;
        self.details_error = None;
        if id.is_null() {
            return;
        }
        let git = self.git.clone();
        self.pool.spawn(&self.tx, move || {
            Event::Details(id, details::load(&git, id).map_err(|e| format!("{e:#}")))
        });
    }

    /// Select a worktree: a dirty one opens File status, a clean one History.
    fn open_worktree(&mut self, path: PathBuf) {
        self.nav.worktree = path;
        if self.nav.tab == Tab::Review {
            // Stay in Review: review the worktree that was clicked.
            if let Some(snap) = &self.snapshot {
                self.labels = build_labels(snap, &self.nav.worktree);
            }
            self.load_review();
            return;
        }
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
        self.diff_view = None;
        self.conflict = None;
        self.commit_form = CommitForm::default();
        self.sync_file_selection();
    }

    pub fn apply(&mut self, action: Action) {
        match action {
            Action::OpenWorktree(path) => self.open_worktree(path),
            Action::NewWorktree(branch) => self.open_worktree_form(branch),
            Action::CloseNewWorktree => {
                if !self.new_worktree.as_ref().is_some_and(|f| f.creating) {
                    self.new_worktree = None;
                }
            }
            Action::BrowseWorktreeFolder => self.browse_worktree_folder(),
            Action::CreateWorktree => self.create_worktree(),
            Action::RemoveWorktree { path, force } => self.remove_worktree(path, force),
            Action::LockWorktree { path, locked } => self.lock_worktree(path, locked),
            Action::PruneWorktrees => self.prune_worktrees(),
            Action::OpenTerminal(dir) => crate::launch::open_terminal(&dir),
            Action::OpenFolder(dir) => {
                if let Err(e) = crate::launch::open_folder(&dir) {
                    self.notices
                        .push(format!("Could not open {}: {e}", dir.display()));
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
            Action::SetTab(tab) => {
                self.nav.tab = tab;
                self.commit_diff = None;
                if tab == Tab::Stashes {
                    self.load_stashes();
                }
                if tab == Tab::Review {
                    if let Some(view) = &mut self.review {
                        view.selected = None;
                    }
                    self.load_review();
                }
            }
            Action::SelectCommitFile(key) => self.select_commit_file(key),
            Action::CloseCommitFile => self.commit_diff = None,
            Action::SelectStash(id) => self.select_stash(id),
            Action::ApplyStash { id, worktree, pop } => self.apply_stash(id, worktree, pop),
            Action::DropStash(id) => self.drop_stash(id),
            Action::Select(id) => self.select(id),
            Action::Reveal(id) => {
                self.nav.tab = Tab::Workspace;
                self.nav.view = WorkspaceView::History;
                self.select(id);
                self.scroll_to_selected.set(true);
            }
            Action::Refresh => self.refresh(),
            Action::Close => {}
            Action::OpenDialog(_)
            | Action::CloseDialog
            | Action::SubmitDialog
            | Action::Checkout(_)
            | Action::StashAndCheckout { .. }
            | Action::CheckoutRemote { .. }
            | Action::CheckoutDetached(_)
            | Action::DeleteBranch { .. }
            | Action::DeleteTag(_)
            | Action::RemoveRemote(_)
            | Action::CherryPick(_)
            | Action::Revert(_)
            | Action::UndoCommit => self.apply_ref_action(action),
            Action::Merge { .. }
            | Action::Rebase(_)
            | Action::ResolveConflict { .. }
            | Action::EditConflict(_)
            | Action::SaveResolution
            | Action::TakeSide { .. }
            | Action::MarkResolved(_)
            | Action::ContinueOp(_)
            | Action::AbortOp { .. } => self.apply_conflict_action(action),
            Action::OpenInIde(path) => {
                crate::launch::open_in_ide(&path, &self.settings.ide_command)
            }
            Action::SelectReviewFile(_)
            | Action::ToggleViewed(_)
            | Action::SetReviewBase(_)
            | Action::ToggleReviewSplit
            | Action::PickLine { .. }
            | Action::SaveComment
            | Action::CancelComment
            | Action::EditComment(_)
            | Action::DeleteComment(_)
            | Action::ClearReviewComments
            | Action::MergeReviewed { .. } => self.apply_review_action(action),
            Action::OpenSlash
            | Action::CloseSlash
            | Action::ToggleSlashItem(_)
            | Action::RunSlash
            | Action::RestoreSlashed(_) => self.apply_slash_action(action),
            Action::PreviewInMain(_) | Action::UpdatePreview | Action::RestoreMain => {
                self.apply_preview_action(action)
            }
            Action::LoadIdentity | Action::SaveIdentity(_) => self.apply_settings_action(action),
            Action::OpenFileHistory(_)
            | Action::CloseFileHistory
            | Action::SelectFileCommit(_)
            | Action::SetFileHistoryMode(_) => self.apply_file_history_action(action),
            other => self.apply_change(other),
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
