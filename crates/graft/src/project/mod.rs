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
pub mod filter;
mod launch;
mod ops;
mod preview;
mod refs;
mod remote;
mod review;
mod settings;
mod slash;
mod stashes;
mod worktrees;

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use graft_core::console::Console;
use graft_core::details::{self, CommitDetails};
use graft_core::history::{self, Commit, History};
use graft_core::stash::{Stash, StashFile};
use graft_core::watch::Watcher;
use graft_core::{CancelToken, Git, JobPool, Oid, Operation, RepoSnapshot, Worktree, snapshot};

pub use changes::{CommitForm, DiffOwner, DiffView, FileKey, LineChange, Staging};
pub use conflicts::{ConflictView, merge_dialog};
pub use file_history::{FileHistoryMode, FileHistoryView};
pub use preview::LeavePreview;
pub use refs::{Dialog, DialogKind};
pub use remote::RemoteOp;
pub use review::{ReviewState, ReviewView};
pub use slash::{SlashView, SlashedBranch};
pub use worktrees::{BranchMode, NewWorktreeForm, checkout_choices, start_choices};

use crate::jobs::Jobs;
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
    /// Select a commit ([`history::worktree_id`] selects "Uncommitted
    /// changes"); with
    /// `scroll`, the history list scrolls to it.
    Select {
        id: Oid,
        scroll: bool,
    },
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
    /// file) in `owner`'s diff, if it still shows `key`.
    ShowLines {
        owner: DiffOwner,
        key: FileKey,
        old: std::ops::Range<u32>,
        new: std::ops::Range<u32>,
    },
    /// Stage, unstage or discard a hunk or the selected lines.
    ChangeLines(Box<LineChange>),
    StageFiles(Vec<String>),
    UnstageFiles(Vec<String>),
    Discard {
        tracked: Vec<String>,
        untracked: Vec<String>,
    },
    /// Ask before discarding a whole file, see [`Action::discard_file`].
    AskDiscard {
        worktree: PathBuf,
        source: graft_core::diff::Source,
        path: String,
    },
    Commit,
    ToggleAmend,
    /// Fill the commit box with a message the AI command writes for what
    /// is staged.
    WriteCommitMessage,
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
    /// Show one file of the commit selected in History.
    SelectCommitFile(FileKey),
    /// Show one file of the selected stash.
    SelectStashFile(FileKey),
    /// Close the diff of a commit's or stash's file.
    CloseDiff(DiffOwner),

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
    // The checkouts below always run in the main folder, never in a linked
    // worktree (see `Project::checkout`).
    /// Switch main to a local branch.
    Checkout(String),
    /// Stash main's changes, then switch it to the branch.
    StashAndCheckout {
        branch: String,
        /// Also stash untracked files (they were in the way).
        untracked: bool,
    },
    /// Create `local` tracking `remote` (e.g. origin/x) and switch main to it.
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
    /// Save it although the file was edited on disk since the resolver
    /// loaded it: the user agreed to replace those edits.
    SaveResolutionAnyway,
    /// Resolve a whole file with one side.
    TakeSide {
        path: String,
        ours: bool,
    },
    /// The user fixed the file in an editor: stage it.
    MarkResolved(String),
    ContinueOp(Operation),
    AbortOp(Operation),

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
    /// Copy every comment of this review as Markdown for a coding agent.
    CopyReviewComments,
    /// Set the note on the whole file at `path`; empty text removes it.
    SetFileNote {
        path: String,
        text: String,
    },
    /// Merge `branch` into `into` (checked out in `worktree`) if it is
    /// still at the reviewed `head`.
    MergeReviewed {
        worktree: PathBuf,
        branch: String,
        into: String,
        head: Oid,
    },
    OpenInIde(PathBuf),

    /// Narrow the sidebar's lists and the stashes to this text.
    SetSidebarFilter(String),

    // Slash
    /// Fetch, scan and show the Slash dialog.
    OpenSlash,
    CloseSlash,
    ToggleSlashItem(usize),
    /// Show only the Slash rows that match this text.
    SetSlashFilter(String),
    /// Check (true) or uncheck every Slash row the filter shows.
    CheckShownSlash(bool),
    /// Slash the rows that are checked and shown: their indexes as the
    /// user saw them, so a change made meanwhile refuses instead.
    RunSlash(Vec<usize>),
    /// Bring back an entry of "Recently slashed".
    RestoreSlashed(usize),

    // Preview in main
    /// Show this worktree's state (commit + uncommitted changes) in main.
    PreviewInMain(PathBuf),
    /// Show the previewed worktree's current state again.
    UpdatePreview,
    RestoreMain,
    /// Update or restore although main changed since the preview: the user
    /// agreed to lose those changes.
    LeavePreviewAnyway(LeavePreview),

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

/// The selected worktree and its branch when a question or dialog opened:
/// its OK must not act on another worktree or branch that took their place
/// meanwhile (the worktree was removed, its branch switched).
#[derive(Clone, Debug)]
pub struct Selection {
    worktree: PathBuf,
    /// Its branch; `None` once any branch will do (see
    /// [`Self::branch_changed_by_us`]).
    branch: Option<Option<String>>,
}

impl Selection {
    /// Graft's own operation may have switched the branch (`git stash
    /// branch` switches before it applies, and can fail after): the
    /// worktree must still be the same, its branch need not.
    pub(super) fn branch_changed_by_us(&mut self) {
        self.branch = None;
    }
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
    /// The sidebar filter, as typed.
    pub sidebar_filter: String,
}

impl RepoMemory {
    /// Without blank file notes and the reviews that remember nothing.
    pub fn pruned(mut self) -> Self {
        for state in self.reviews.values_mut() {
            state.forget_blank_notes();
        }
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

impl crate::jobs::Owner for Project {
    fn panicked(&mut self, message: String) {
        self.notices.push(message);
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
    jobs: Jobs<Project>,

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
    /// The tips a history walk in flight reads.
    walking: Option<Vec<Oid>>,
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
    /// Bumped when the history list should scroll the selection into view.
    pub scroll_request: u64,

    /// File status: the selected file's diff.
    pub diff_view: Option<DiffView>,
    /// History: the open file of the selected commit.
    pub commit_diff: Option<DiffView>,
    /// `None` until the Stashes tab was opened.
    pub stashes: Option<Result<Vec<Stash>, String>>,
    pub selected_stash: Option<Oid>,
    pub stash_files: Option<Result<Vec<StashFile>, String>>,
    /// The open file of the selected stash.
    pub stash_diff: Option<DiffView>,
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
    /// The worktree whose commit message the AI command is writing.
    pub writing_message: Option<PathBuf>,
    /// Commits made from the commit box, so a message written meanwhile
    /// is not put in for changes that are already committed.
    commits_made: u64,
    /// A git command that changes things, while it runs.
    op: Option<Running>,
    /// A destructive action waiting for the user's OK, and what was
    /// selected when it was asked.
    confirm: Option<(Confirm, Selection)>,
    /// Questions that came up while another was open, in order.
    queued_confirms: VecDeque<Confirm>,

    /// Errors for the app to show as toasts.
    pub notices: Vec<String>,
    /// Confirmations for the app to show as toasts.
    pub infos: Vec<String>,
    /// Text for the app to put on the clipboard.
    pub clipboard: Option<String>,

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
        let mut project = Self {
            git: Git::new(&root, console),
            name: graft_core::paths::folder_name(&root),
            root,
            jobs: Jobs::new(pool),
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
            walking: None,
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
            scroll_request: 0,
            diff_view: None,
            commit_diff: None,
            stashes: None,
            selected_stash: None,
            stash_files: None,
            stash_diff: None,
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
            writing_message: None,
            commits_made: 0,
            op: None,
            confirm: None,
            queued_confirms: VecDeque::new(),
            notices: Vec::new(),
            infos: Vec::new(),
            clipboard: None,
            new_worktree: None,
            dialog: None,
            pending_open: None,
            watcher: None,
            watched: Vec::new(),
        };
        project.refresh();
        project
    }

    fn ticket(&mut self) -> u64 {
        self.tickets += 1;
        self.tickets
    }

    /// Apply results of finished jobs. Call once per frame.
    pub fn poll(&mut self) {
        self.maybe_auto_fetch();
        while let Some(apply) = self.jobs.next_done() {
            apply(self);
        }
    }

    /// What Graft remembers about this repository.
    pub fn memory(&self) -> &RepoMemory {
        &self.memory
    }

    /// The sidebar filter, as the lists apply it.
    pub fn filter(&self) -> filter::ListFilter {
        filter::ListFilter::new(&self.memory.sidebar_filter)
    }

    fn set_sidebar_filter(&mut self, text: String) {
        self.memory.sidebar_filter = text;
        // The STASHES count follows the filter, so it needs the list.
        if self.filter().is_on() && self.stashes.is_none() {
            self.load_stashes();
        }
        self.sync_stash_selection();
    }

    pub fn settings(&self) -> &Settings {
        &self.settings
    }

    /// Take on changed app settings.
    pub fn set_settings(&mut self, settings: &Settings) {
        let reload = settings.whole_file != self.settings.whole_file;
        self.settings = settings.clone();
        if reload {
            self.reload_diffs();
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
        self.jobs.spawn(
            move || snapshot::load(&git),
            |p, result| p.on_snapshot(result),
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
        self.worktree_in(self.snapshot.as_ref()?)
    }

    /// A branch, remote branch or tag picked by name, as git must be given
    /// it (see [`RepoSnapshot::rev`]).
    fn rev(&self, name: &str) -> String {
        match &self.snapshot {
            Some(snap) => snap.rev(name),
            None => name.to_owned(),
        }
    }

    /// The selected worktree in `snap`: a screen holding its own handle on
    /// the snapshot can borrow from it while it changes the project.
    pub fn worktree_in<'a>(&self, snap: &'a RepoSnapshot) -> Option<&'a Worktree> {
        snap.worktree_at(&self.nav.worktree)
    }

    fn on_snapshot(&mut self, result: anyhow::Result<RepoSnapshot>) {
        self.loading = false;
        match result {
            Ok(snap) => self.take_snapshot(snap),
            Err(e) if self.snapshot.is_none() => self.error = Some(format!("{e:#}")),
            Err(e) => self.report(e),
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
        if self.stashes.is_some() || self.nav.tab == Tab::Stashes || self.filter().is_on() {
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
    /// A walk in flight for the same tips is kept, never restarted or
    /// replaced by a rebuild from older commits: when it lands,
    /// [`Self::on_history`] builds again if the parent changed meanwhile.
    fn load_history(&mut self, walk: bool) {
        if self.walking.as_ref() == Some(&self.tips) {
            return;
        }
        self.walking = walk.then(|| self.tips.clone());
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
        self.jobs.spawn(
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
        self.walking = None;
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
                // Another worktree was picked while it loaded: build it again
                // for that one (from the commits just read) and select its HEAD.
                if self.worktree_parent() != self.history_parent {
                    self.pending = Some(Pending::Head);
                    self.load_history(false);
                    return;
                }
                if !keep || head {
                    self.select_head();
                }
            }
            Err(e) => self.report(e.context("Could not load history")),
        }
    }

    /// Select the selected worktree's HEAD (or its uncommitted changes).
    fn select_head(&mut self) {
        let Some(history) = &self.history else { return };
        let target = match self.worktree() {
            Some(wt) if wt.is_dirty() => Some(history::worktree_id()),
            Some(wt) => wt.head,
            None => None,
        }
        .filter(|id| history.index.contains_key(id))
        .or_else(|| history.commits.first().map(|c| c.id));
        if let Some(id) = target {
            self.select(id);
            self.scroll_to_selected();
        }
    }

    fn scroll_to_selected(&mut self) {
        self.scroll_request += 1;
    }

    pub(super) fn select(&mut self, id: Oid) {
        let worktree = history::is_worktree_id(&id);
        if self.selected == Some(id) && (self.details.is_some() || worktree) {
            return;
        }
        self.selected = Some(id);
        self.details = None;
        self.commit_diff = None;
        self.details_error = None;
        if worktree {
            return;
        }
        let git = self.git.clone();
        self.jobs.spawn(
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
        self.scroll_to_selected();
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
        self.commit_form = self.commit_form.cleared();
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
        match tab {
            Tab::Workspace => {}
            Tab::Stashes => self.load_stashes(),
            Tab::Review => self.load_review(),
        }
    }

    /// Show `view` in the Workspace tab; leaving File history closes it.
    fn show_view(&mut self, view: WorkspaceView) {
        self.nav.tab = Tab::Workspace;
        if view != WorkspaceView::FileHistory {
            self.file_history = None;
        }
        self.nav.view = view;
    }

    /// The question waiting for the user's answer.
    pub fn confirm(&self) -> Option<&Confirm> {
        self.confirm.as_ref().map(|(confirm, _)| confirm)
    }

    /// Ask before doing something. A question never replaces an open one
    /// (its button could change under the pointer): it waits its turn.
    pub(super) fn ask(&mut self, confirm: Confirm) {
        self.queued_confirms.push_back(confirm);
        self.next_question();
    }

    fn next_question(&mut self) {
        if self.confirm.is_none()
            && let Some(confirm) = self.queued_confirms.pop_front()
        {
            self.confirm = Some((confirm, self.selection()));
        }
    }

    fn answer(&mut self, yes: bool) {
        if let Some((confirm, asked)) = self.confirm.take()
            && yes
            && self.still_selected(&asked)
        {
            self.apply(confirm.action);
        }
        self.next_question();
    }

    pub(super) fn selection(&self) -> Selection {
        Selection {
            worktree: self.nav.worktree.clone(),
            branch: Some(self.worktree().and_then(|wt| wt.branch.clone())),
        }
    }

    /// Whether the same worktree and branch are selected as when `asked`;
    /// if not, says so and nothing should run.
    pub(super) fn still_selected(&mut self, asked: &Selection) -> bool {
        let now = self.selection();
        let same = graft_core::paths::same_path(&now.worktree, &asked.worktree)
            && (asked.branch.is_none() || now.branch == asked.branch);
        if !same {
            self.notices.push(
                "Nothing was done: the selected worktree or its branch changed while \
                 you were deciding. Check and try again."
                    .to_owned(),
            );
        }
        same
    }

    pub fn apply(&mut self, action: Action) {
        match action {
            Action::OpenWorktree(path) => self.open_worktree(path),
            Action::ShowFileStatus => self.show_view(WorkspaceView::FileStatus),
            Action::ShowHistory => self.show_view(WorkspaceView::History),
            Action::SetTab(tab) => self.set_tab(tab),
            Action::Select { id, scroll } => {
                self.select(id);
                if scroll {
                    self.scroll_to_selected();
                }
            }
            Action::Reveal(id) => self.reveal(id),
            Action::Refresh => self.refresh(),

            Action::SelectFile(key) => self.select_file(key),
            Action::ClickLine { hunk, line, shift } => self.click_line(hunk, line, shift),
            Action::DragLine { hunk, line } => self.drag_line(hunk, line),
            Action::ClearLines => self.clear_lines(),
            Action::ShowLines {
                owner,
                key,
                old,
                new,
            } => self.show_lines(owner, &key, old, new),
            Action::ChangeLines(change) => self.change_lines(*change),
            Action::StageFiles(paths) => self.stage_files(paths),
            Action::UnstageFiles(paths) => self.unstage_files(paths),
            Action::Discard { tracked, untracked } => self.discard(tracked, untracked),
            Action::AskDiscard {
                worktree,
                source,
                path,
            } => self.ask_discard(&worktree, &source, path),
            Action::Commit => self.commit(),
            Action::ToggleAmend => self.toggle_amend(),
            Action::WriteCommitMessage => self.write_commit_message(),
            Action::Remote(op) => self.remote(op),
            Action::CancelOperation => self.cancel_op(),
            Action::Ask(confirm) => self.ask(*confirm),
            Action::ConfirmYes => self.answer(true),
            Action::ConfirmNo => self.answer(false),
            Action::OpenPath(path) => self.open_path(&path),
            Action::RevealPath(path) => self.reveal_path(&path),

            Action::NewWorktree(branch) => self.open_worktree_form(branch),
            Action::CloseNewWorktree => self.close_worktree_form(),
            Action::BrowseWorktreeFolder => self.browse_worktree_folder(),
            Action::CreateWorktree => self.create_worktree(),
            Action::RemoveWorktree { path, force } => self.remove_worktree(path, force),
            Action::LockWorktree { path, locked } => self.lock_worktree(path, locked),
            Action::PruneWorktrees => self.prune_worktrees(),
            Action::OpenTerminal(dir) => self.open_terminal(dir),
            Action::OpenFolder(dir) => self.open_folder(dir),

            Action::SelectCommitFile(key) => self.open_diff(DiffOwner::Commit, key),
            Action::SelectStashFile(key) => self.open_diff(DiffOwner::Stash, key),
            Action::CloseDiff(owner) => self.close_diff(owner),

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
            Action::SaveResolution => self.save_resolution(false),
            Action::SaveResolutionAnyway => self.save_resolution(true),
            Action::TakeSide { path, ours } => self.take_side(path, ours),
            Action::MarkResolved(path) => self.mark_resolved(path),
            Action::ContinueOp(kind) => self.continue_op(kind),
            Action::AbortOp(kind) => self.abort_op(kind),

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
            Action::CopyReviewComments => self.copy_review_comments(),
            Action::SetFileNote { path, text } => self.set_file_note(path, text),
            Action::MergeReviewed {
                worktree,
                branch,
                into,
                head,
            } => self.merge_reviewed(worktree, branch, into, head),
            Action::OpenInIde(path) => self.open_in_ide(path),

            Action::OpenSlash => self.open_slash(),
            Action::CloseSlash => self.close_slash(),
            Action::ToggleSlashItem(i) => self.toggle_slash_item(i),
            Action::SetSlashFilter(filter) => self.set_slash_filter(filter),
            Action::SetSidebarFilter(filter) => self.set_sidebar_filter(filter),
            Action::CheckShownSlash(on) => self.check_shown_slash(on),
            Action::RunSlash(seen) => self.run_slash(seen),
            Action::RestoreSlashed(i) => self.restore_slashed(i),

            Action::PreviewInMain(worktree) => self.preview_in_main(worktree),
            Action::UpdatePreview => self.leave_preview(LeavePreview::Update, false),
            Action::RestoreMain => self.leave_preview(LeavePreview::Restore, false),
            Action::LeavePreviewAnyway(leave) => self.leave_preview(leave, true),

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{git, open, wait};

    /// The STASHES count in the sidebar follows the filter, so the stash
    /// list loads once a filter is typed, and right away when a filter was
    /// saved before a restart.
    #[test]
    fn a_sidebar_filter_loads_the_stashes_for_their_count() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_path_buf();
        git(&root, &["init", "-q", "-b", "main"]);
        std::fs::write(root.join("a.txt"), "a\n").unwrap();
        git(&root, &["add", "a.txt"]);
        git(&root, &["commit", "-q", "-m", "first"]);
        std::fs::write(root.join("a.txt"), "b\n").unwrap();
        git(&root, &["stash", "push", "-q", "-m", "login form"]);
        std::fs::write(root.join("a.txt"), "c\n").unwrap();
        git(&root, &["stash", "push", "-q", "-m", "tidy readme"]);

        let mut project = open(&root);
        assert!(project.stashes.is_none(), "no filter: loaded on demand");
        project.apply(Action::SetSidebarFilter("login".to_owned()));
        wait(
            &mut project,
            "the stashes never loaded",
            |p| matches!(&p.stashes, Some(Ok(list)) if list.len() == 2),
        );
        // The newest stash is hidden, so the one the filter shows is picked.
        let list = project.stashes.as_ref().unwrap().as_ref().unwrap();
        let (tidy, login) = (list[0].id, list[1].id);
        assert_eq!(project.selected_stash, Some(login));
        // Typing more so that it hides too leaves nothing selected.
        project.apply(Action::SetSidebarFilter("login formx".to_owned()));
        assert_eq!(project.selected_stash, None);
        project.apply(Action::SetSidebarFilter(String::new()));
        assert_eq!(project.selected_stash, Some(tidy), "the first again");
        // A selection the filter still shows stays.
        project.apply(Action::SelectStash(login));
        project.apply(Action::SetSidebarFilter("form".to_owned()));
        assert_eq!(project.selected_stash, Some(login));

        let memory = RepoMemory {
            sidebar_filter: "login".to_owned(),
            ..RepoMemory::default()
        };
        let pool = JobPool::new(2, std::sync::Arc::new(|| {}));
        let mut restarted = Project::new(
            root.clone(),
            root.clone(),
            Settings::default(),
            memory,
            Console::default(),
            pool,
        );
        wait(
            &mut restarted,
            "the saved filter loaded no stashes",
            |p| matches!(&p.stashes, Some(Ok(list)) if list.len() == 2),
        );
    }

    #[test]
    fn a_dirty_worktree_picked_during_the_first_load_gets_its_row() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("app");
        std::fs::create_dir(&root).unwrap();
        git(&root, &["init", "-q", "-b", "main"]);
        std::fs::write(root.join("a.txt"), "a\n").unwrap();
        git(&root, &["add", "a.txt"]);
        git(&root, &["commit", "-q", "-m", "first"]);
        let other = tmp.path().join("other");
        git(
            &root,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                "topic",
                other.to_str().unwrap(),
            ],
        );
        std::fs::write(other.join("a.txt"), "changed\n").unwrap();
        let mut project = open(&root);
        // Git's spelling of the folder (the temp folder may be a short name).
        let snap = project.snapshot.clone().unwrap();
        let other = snap.worktrees.iter().find(|wt| !wt.is_main).unwrap();
        let other = other.path.clone();
        wait(&mut project, "the worktree never showed dirty", |p| {
            p.snapshot
                .as_ref()
                .and_then(|s| s.worktree_at(&other))
                .is_some_and(|wt| wt.is_dirty())
        });

        // The first history load, built for the clean main worktree, is
        // still running when the dirty one is picked.
        project.history = None;
        project.load_history(true);
        project.apply(Action::OpenWorktree(other));
        wait(&mut project, "history never loaded", |p| {
            !p.history_loading && p.history.is_some()
        });
        let history = project.history.as_ref().unwrap();
        assert!(history.index.contains_key(&history::worktree_id()));
        assert_eq!(project.selected, Some(history::worktree_id()));
    }

    /// A repository with one commit, opened once its history is loaded.
    fn opened_repo(tmp: &Path) -> (PathBuf, Project) {
        let root = tmp.join("app");
        std::fs::create_dir(&root).unwrap();
        git(&root, &["init", "-q", "-b", "main"]);
        std::fs::write(root.join("a.txt"), "a\n").unwrap();
        git(&root, &["add", "a.txt"]);
        git(&root, &["commit", "-q", "-m", "first"]);
        let mut project = open(&root);
        wait(&mut project, "history never loaded", |p| {
            !p.history_loading && p.history.is_some()
        });
        (root, project)
    }

    /// The files changed: what the watcher's refresh brings.
    fn refreshed(project: &mut Project) {
        let snap = snapshot::load(&project.git).unwrap();
        project.take_snapshot(snap);
    }

    /// Refreshes while the first history loads (files keep changing, or a
    /// repository in WSL) keep that load instead of starting it again.
    #[test]
    fn refreshes_during_the_first_load_keep_it() {
        let tmp = tempfile::tempdir().unwrap();
        let (_root, mut project) = opened_repo(tmp.path());
        project.commits = None;
        project.history = None;
        project.load_history(true);
        let generation = project.history_generation;
        for _ in 0..3 {
            refreshed(&mut project);
        }
        assert_eq!(project.history_generation, generation, "load restarted");
        wait(&mut project, "history never loaded", |p| {
            !p.history_loading && p.history.is_some()
        });
    }

    /// The worktree gets changes while the history is read again after a
    /// commit: the new history has both the commit and the changes.
    #[test]
    fn changes_during_a_reload_keep_its_new_commits() {
        let tmp = tempfile::tempdir().unwrap();
        let (root, mut project) = opened_repo(tmp.path());
        std::fs::write(root.join("a.txt"), "b\n").unwrap();
        git(&root, &["commit", "-q", "-am", "second"]);
        refreshed(&mut project);
        assert!(project.history_loading);
        std::fs::write(root.join("a.txt"), "dirty\n").unwrap();
        refreshed(&mut project);
        wait(&mut project, "history never loaded", |p| {
            !p.history_loading && p.history.is_some()
        });
        let head = project.worktree().and_then(|wt| wt.head).unwrap();
        let history = project.history.as_ref().unwrap();
        assert!(history.index.contains_key(&head), "the new commit");
        assert!(history.index.contains_key(&history::worktree_id()));
    }

    fn question(title: &str, action: Action) -> Confirm {
        Confirm {
            title: title.to_owned(),
            body: String::new(),
            confirm_label: "OK".to_owned(),
            action,
        }
    }

    /// The worktree a question was asked for went away while it was open:
    /// OK does nothing in the main folder that took its place, and says so.
    /// The same for a dialog when the branch switched.
    #[test]
    fn ok_never_acts_on_what_took_the_asked_worktrees_place() {
        let tmp = tempfile::tempdir().unwrap();
        let (root, mut project) = opened_repo(tmp.path());
        let linked = tmp.path().join("linked");
        let linked_arg = linked.to_str().unwrap();
        git(&root, &["worktree", "add", "-q", "-b", "agent", linked_arg]);
        std::fs::write(root.join("a.txt"), "main's work\n").unwrap();
        refreshed(&mut project);
        let snap = project.snapshot.clone().unwrap();
        let linked = snap.worktrees.iter().find(|wt| !wt.is_main).unwrap();
        project.apply(Action::OpenWorktree(linked.path.clone()));

        let discard = Action::Discard {
            tracked: vec!["a.txt".to_owned()],
            untracked: Vec::new(),
        };
        project.apply(Action::Ask(Box::new(question("Discard?", discard))));
        git(&root, &["worktree", "remove", "--force", linked_arg]);
        refreshed(&mut project);
        assert!(project.worktree().is_some_and(|wt| wt.is_main));
        project.apply(Action::ConfirmYes);
        assert!(!project.is_operating());
        assert_eq!(project.notices.len(), 1, "{:?}", project.notices);
        assert!(project.confirm().is_none());
        let a = std::fs::read_to_string(root.join("a.txt")).unwrap();
        assert_eq!(a, "main's work\n");

        project.notices.clear();
        project.apply(Action::OpenDialog(DialogKind::stash()));
        git(&root, &["switch", "-q", "-c", "other"]);
        refreshed(&mut project);
        project.apply(Action::SubmitDialog);
        assert!(!project.is_operating());
        assert!(project.dialog.is_none());
        assert_eq!(project.notices.len(), 1, "{:?}", project.notices);
        let a = std::fs::read_to_string(root.join("a.txt")).unwrap();
        assert_eq!(a, "main's work\n", "not stashed");
    }

    /// A question that comes up while another is open (a follow-up of an
    /// operation) waits for it instead of taking its place.
    #[test]
    fn a_question_waits_for_the_open_one() {
        let tmp = tempfile::tempdir().unwrap();
        let (_root, mut project) = opened_repo(tmp.path());
        let clear = question("Clear all comments?", Action::ClearReviewComments);
        project.apply(Action::Ask(Box::new(clear)));
        project.ask(question("Force push?", Action::Refresh));
        assert_eq!(project.confirm().unwrap().title, "Clear all comments?");
        project.apply(Action::ConfirmNo);
        assert_eq!(project.confirm().unwrap().title, "Force push?");
        project.apply(Action::ConfirmNo);
        assert!(project.confirm().is_none());
    }
}
