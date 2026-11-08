//! The plain data a repository snapshot is made of. Only data and the rules
//! that read it; loading and changing it live in the feature modules.

use std::path::{Path, PathBuf};

pub use gix::ObjectId as Oid;

use crate::paths::same_path;

/// Names taken as the default branch, in this order, when origin/HEAD does
/// not name one that exists locally.
pub(crate) const DEFAULT_BRANCHES: [&str; 4] = ["main", "master", "trunk", "develop"];

/// The remote `git clone` sets up: the one to use when there is a choice.
pub const ORIGIN: &str = "origin";

/// Everything the sidebar, toolbar and tabs need to know about one repository.
#[derive(Clone, Debug, Default)]
pub struct RepoSnapshot {
    pub name: String,
    /// The main worktree (where `.git` lives).
    pub root: PathBuf,
    pub worktrees: Vec<Worktree>,
    pub branches: Vec<Branch>,
    pub remote_branches: Vec<RemoteBranch>,
    pub tags: Vec<Tag>,
    pub remotes: Vec<Remote>,
    /// The branch origin/HEAD points at: the remote's default branch.
    pub remote_default: Option<String>,
    pub stash_count: usize,
    /// Another worktree shown in the main worktree ("Preview in main").
    pub preview: Option<Preview>,
}

impl RepoSnapshot {
    pub fn main_worktree(&self) -> Option<&Worktree> {
        self.worktrees.iter().find(|wt| wt.is_main)
    }

    /// The worktree at `path`.
    pub fn worktree_at(&self, path: &Path) -> Option<&Worktree> {
        self.worktrees.iter().find(|wt| same_path(&wt.path, path))
    }

    pub fn branch(&self, name: &str) -> Option<&Branch> {
        self.branches.iter().find(|b| b.name == name)
    }

    /// Why git must not move HEAD (commit, check out, reset, cherry-pick,
    /// merge, …) in the worktree at `path` now: main shows a preview, and
    /// Restore main would leave what that made behind.
    pub fn head_blocked(&self, path: &Path) -> Option<&'static str> {
        let main = self.main_worktree()?;
        (self.preview.is_some() && same_path(&main.path, path))
            .then_some("The main folder shows a preview. Restore main first.")
    }

    /// The branch the main worktree is on. During a preview git sees main
    /// detached, but it is still on the branch Restore main goes back to.
    pub fn main_branch(&self) -> Option<&str> {
        match &self.preview {
            Some(preview) => match &preview.restore {
                Restore::Branch(branch) => Some(branch),
                Restore::Detached(_) => None,
            },
            None => self.main_worktree()?.branch.as_deref(),
        }
    }

    /// Worktree that has `branch` checked out, if any (main's branch as
    /// [`Self::main_branch`] sees it).
    pub fn worktree_for_branch(&self, branch: &str) -> Option<&Worktree> {
        self.worktrees.iter().find(|wt| {
            let on = if wt.is_main {
                self.main_branch()
            } else {
                wt.branch.as_deref()
            };
            on == Some(branch)
        })
    }

    /// What can be merged into the branch `into`: the other local branches,
    /// then the remote branches.
    pub fn merge_choices<'a>(&'a self, into: &'a str) -> impl Iterator<Item = &'a str> {
        self.branches
            .iter()
            .map(|b| b.name.as_str())
            .filter(move |name| *name != into)
            .chain(self.remote_branches.iter().map(|b| b.name.as_str()))
    }

    /// Local branches that could be the default branch, best first:
    /// origin/HEAD's target, then [`DEFAULT_BRANCHES`].
    pub(crate) fn default_branches(&self) -> impl Iterator<Item = &str> {
        self.remote_default
            .as_deref()
            .into_iter()
            .chain(DEFAULT_BRANCHES)
            .filter(|name| self.branch(name).is_some())
    }

    /// The default branch: the first of [`Self::default_branches`].
    pub fn default_branch(&self) -> Option<&str> {
        self.default_branches().next()
    }

    /// True if any worktree has uncommitted changes.
    pub fn is_dirty(&self) -> bool {
        self.worktrees.iter().any(Worktree::is_dirty)
    }

    /// Every commit a ref or worktree points at, sorted, without
    /// duplicates: where the history walk starts.
    pub fn tips(&self) -> Vec<Oid> {
        let mut tips: Vec<Oid> = self
            .branches
            .iter()
            .map(|b| b.target)
            .chain(self.remote_branches.iter().map(|b| b.target))
            .chain(self.tags.iter().map(|t| t.target))
            .chain(self.worktrees.iter().filter_map(|wt| wt.head))
            .collect();
        tips.sort();
        tips.dedup();
        tips
    }
}

#[derive(Clone, Debug)]
pub struct Worktree {
    pub path: PathBuf,
    /// Checked-out branch, or `None` for a detached HEAD.
    pub branch: Option<String>,
    pub head: Option<Oid>,
    pub is_main: bool,
    pub locked: bool,
    /// The folder is gone; `git worktree prune` would remove the entry.
    pub prunable: bool,
    /// `None` if status could not be read (e.g. prunable worktree).
    pub status: Option<WorktreeStatus>,
    /// A merge, cherry-pick, revert or rebase that stopped part-way.
    pub in_progress: Option<InProgress>,
}

impl Worktree {
    pub fn label(&self) -> String {
        match (&self.branch, self.head) {
            (Some(b), _) => b.clone(),
            (None, Some(oid)) => format!("detached at {}", short(&oid)),
            (None, None) => "(no commits yet)".to_owned(),
        }
    }

    /// Number of changed files (staged, unstaged, untracked, conflicted).
    pub fn dirty(&self) -> usize {
        self.status.as_ref().map_or(0, |s| s.entries.len())
    }

    pub fn is_dirty(&self) -> bool {
        self.dirty() > 0
    }

    pub fn ahead(&self) -> usize {
        self.status.as_ref().map_or(0, |s| s.ahead)
    }

    pub fn behind(&self) -> usize {
        self.status.as_ref().map_or(0, |s| s.behind)
    }
}

/// An operation that stopped part-way, usually because of conflicts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Operation {
    Merge,
    CherryPick,
    Revert,
    Rebase,
}

impl Operation {
    /// The git command that runs, continues and aborts it.
    pub(crate) fn command(self) -> &'static str {
        match self {
            Operation::Merge => "merge",
            Operation::CherryPick => "cherry-pick",
            Operation::Revert => "revert",
            Operation::Rebase => "rebase",
        }
    }

    pub fn noun(self) -> &'static str {
        match self {
            Operation::Merge => "Merge",
            Operation::CherryPick => "Cherry-pick",
            Operation::Revert => "Revert",
            Operation::Rebase => "Rebase",
        }
    }
}

/// The files and folders in a git dir that mark an operation in progress,
/// checked in this order.
pub(crate) const OPERATION_MARKERS: [(&str, Operation); 5] = [
    ("rebase-merge", Operation::Rebase),
    ("rebase-apply", Operation::Rebase),
    ("MERGE_HEAD", Operation::Merge),
    ("CHERRY_PICK_HEAD", Operation::CherryPick),
    ("REVERT_HEAD", Operation::Revert),
];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InProgress {
    pub kind: Operation,
    /// What is being done, e.g. "Merge branch 'feature/x'".
    pub title: String,
}

/// What the main worktree goes back to after a preview.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Restore {
    Branch(String),
    Detached(Oid),
}

impl Restore {
    /// The branch name, or the short commit id.
    pub fn label(&self) -> String {
        match self {
            Restore::Branch(branch) => branch.clone(),
            Restore::Detached(id) => short(id),
        }
    }
}

/// Another worktree shown in the main worktree ("Preview in main").
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Preview {
    pub restore: Restore,
    /// The worktree being previewed.
    pub source: PathBuf,
    /// Its commit, now checked out (detached) in main.
    pub head: Oid,
    /// Files the preview created in main, removed again on restore.
    pub added: Vec<String>,
    /// The tree of main's files as the preview left them (untracked files
    /// included): whether main changed since. `None` if unknown.
    pub shown: Option<Oid>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Branch {
    pub name: String,
    pub target: Oid,
    pub upstream: Upstream,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Upstream {
    /// No upstream configured: the branch only exists locally.
    None,
    Tracking {
        name: String,
        ahead: usize,
        behind: usize,
    },
    /// An upstream is configured but the remote branch was deleted.
    Gone { name: String },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RemoteBranch {
    /// Short name, e.g. `origin/main`.
    pub name: String,
    pub remote: String,
    pub target: Oid,
}

impl RemoteBranch {
    /// The branch name without the remote prefix, e.g. `main`.
    pub fn branch_name(&self) -> &str {
        self.name
            .strip_prefix(&self.remote)
            .and_then(|s| s.strip_prefix('/'))
            .unwrap_or(&self.name)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tag {
    pub name: String,
    /// The commit the tag points at (annotated tags are peeled).
    pub target: Oid,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Remote {
    pub name: String,
    /// Its (first) URL, or its push URL when it has only that.
    pub url: String,
    /// `url` is a push URL: the remote has no fetch URL.
    pub push_only: bool,
}

/// A worktree's uncommitted changes and how far it is ahead of and behind
/// its upstream.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WorktreeStatus {
    pub ahead: usize,
    pub behind: usize,
    pub entries: Vec<StatusEntry>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StatusEntry {
    pub path: String,
    /// Source path of a rename or copy.
    pub orig_path: Option<String>,
    /// Change between HEAD and the index (the "staged" side).
    pub index: Change,
    /// Change between the index and the working tree (the "unstaged" side).
    pub worktree: Change,
    pub kind: EntryKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EntryKind {
    Tracked,
    Untracked,
    /// Merge conflict.
    Conflicted,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Change {
    None,
    Modified,
    TypeChanged,
    Added,
    Deleted,
    Renamed,
    Copied,
    Unmerged,
}

impl Change {
    /// A status letter as git prints it (porcelain `XY`, `--name-status`).
    pub(crate) fn from_code(c: u8) -> Self {
        match c {
            b'M' => Self::Modified,
            b'T' => Self::TypeChanged,
            b'A' => Self::Added,
            b'D' => Self::Deleted,
            b'R' => Self::Renamed,
            b'C' => Self::Copied,
            b'U' => Self::Unmerged,
            _ => Self::None,
        }
    }

    /// One-letter badge as shown in the file lists.
    pub fn letter(self) -> &'static str {
        match self {
            Self::None => "",
            Self::Modified => "M",
            Self::TypeChanged => "T",
            Self::Added => "A",
            Self::Deleted => "D",
            Self::Renamed => "R",
            Self::Copied => "C",
            Self::Unmerged => "U",
        }
    }
}

impl StatusEntry {
    pub fn is_staged(&self) -> bool {
        self.kind == EntryKind::Tracked && self.index != Change::None
    }

    pub fn is_unstaged(&self) -> bool {
        match self.kind {
            EntryKind::Untracked => true,
            EntryKind::Tracked => self.worktree != Change::None,
            EntryKind::Conflicted => false,
        }
    }

    pub fn is_conflicted(&self) -> bool {
        self.kind == EntryKind::Conflicted
    }

    /// Badge for the unstaged list ("?" counts as added).
    pub(crate) fn unstaged_change(&self) -> Change {
        match self.kind {
            EntryKind::Untracked => Change::Added,
            _ => self.worktree,
        }
    }

    /// Badge where the file is listed once: its badge in the first
    /// [`FileList`] it is in.
    pub fn shown_change(&self) -> Change {
        if self.is_conflicted() {
            Change::Unmerged
        } else if self.is_staged() {
            self.index
        } else {
            self.unstaged_change()
        }
    }
}

/// The lists of changed files in File status, top to bottom. A file with
/// staged and unstaged changes is in both of those.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileList {
    Conflicts,
    Staged,
    Unstaged,
}

impl FileList {
    /// In display order.
    pub const ALL: [FileList; 3] = [FileList::Conflicts, FileList::Staged, FileList::Unstaged];

    pub fn contains(self, entry: &StatusEntry) -> bool {
        match self {
            FileList::Conflicts => entry.is_conflicted(),
            FileList::Staged => entry.is_staged(),
            FileList::Unstaged => entry.is_unstaged(),
        }
    }

    /// The badge `entry` shows in this list.
    pub fn change(self, entry: &StatusEntry) -> Change {
        match self {
            FileList::Conflicts => Change::Unmerged,
            FileList::Staged => entry.index,
            FileList::Unstaged => entry.unstaged_change(),
        }
    }
}

impl WorktreeStatus {
    pub fn in_list(&self, list: FileList) -> impl Iterator<Item = &StatusEntry> {
        self.entries.iter().filter(move |e| list.contains(e))
    }

    pub fn staged(&self) -> impl Iterator<Item = &StatusEntry> {
        self.in_list(FileList::Staged)
    }

    pub fn unstaged(&self) -> impl Iterator<Item = &StatusEntry> {
        self.in_list(FileList::Unstaged)
    }

    pub fn conflicted(&self) -> impl Iterator<Item = &StatusEntry> {
        self.in_list(FileList::Conflicts)
    }

    /// Every row of File status in display order: each list in
    /// [`FileList::ALL`] order, entries in status order within a list.
    pub fn listed(&self) -> impl Iterator<Item = (FileList, &StatusEntry)> {
        FileList::ALL
            .into_iter()
            .flat_map(move |list| self.in_list(list).map(move |e| (list, e)))
    }
}

/// First 7 hex digits of an object id.
pub fn short(oid: &Oid) -> String {
    let mut s = oid.to_string();
    s.truncate(7);
    s
}

/// The all-zero id: no object has it.
pub fn null_oid() -> Oid {
    Oid::null(gix::hash::Kind::Sha1)
}
