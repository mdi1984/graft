//! The plain data a repository snapshot is made of. Only data and the rules
//! that read it; loading and changing it live in the feature modules.

use std::path::PathBuf;

pub use gix::ObjectId as Oid;

use crate::status::WorktreeStatus;

/// Names taken as the default branch, in this order, when origin/HEAD does
/// not name one that exists locally.
pub const DEFAULT_BRANCHES: [&str; 4] = ["main", "master", "trunk", "develop"];

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

    pub fn branch(&self, name: &str) -> Option<&Branch> {
        self.branches.iter().find(|b| b.name == name)
    }

    /// Worktree that has `branch` checked out, if any.
    pub fn worktree_for_branch(&self, branch: &str) -> Option<&Worktree> {
        self.worktrees
            .iter()
            .find(|wt| wt.branch.as_deref() == Some(branch))
    }

    /// Local branches that could be the default branch, best first:
    /// origin/HEAD's target, then [`DEFAULT_BRANCHES`].
    pub fn default_branches(&self) -> impl Iterator<Item = &str> {
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
pub enum Kind {
    Merge,
    CherryPick,
    Revert,
    Rebase,
}

impl Kind {
    /// The git command that runs, continues and aborts it.
    pub(crate) fn command(self) -> &'static str {
        match self {
            Kind::Merge => "merge",
            Kind::CherryPick => "cherry-pick",
            Kind::Revert => "revert",
            Kind::Rebase => "rebase",
        }
    }

    pub fn noun(self) -> &'static str {
        match self {
            Kind::Merge => "Merge",
            Kind::CherryPick => "Cherry-pick",
            Kind::Revert => "Revert",
            Kind::Rebase => "Rebase",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InProgress {
    pub kind: Kind,
    /// What is being done, e.g. "Merge branch 'feature/x'".
    pub title: String,
}

/// What the main worktree goes back to after a preview.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Restore {
    Branch(String),
    Detached(Oid),
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
    pub annotated: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Remote {
    pub name: String,
    pub fetch_url: String,
    pub push_url: String,
}

/// First 7 hex digits of an object id.
pub fn short(oid: &Oid) -> String {
    let mut s = oid.to_string();
    s.truncate(7);
    s
}

/// The all-zero id. In the history it stands for "uncommitted changes".
pub fn null_oid() -> Oid {
    Oid::null(gix::hash::Kind::Sha1)
}
