use std::path::{Path, PathBuf};

pub use gix::ObjectId as Oid;

use crate::status::WorktreeStatus;

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
    pub preview: Option<crate::preview::Preview>,
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

    /// The default branch: origin/HEAD's target if it exists locally, else
    /// the first of main, master, trunk, develop.
    pub fn default_branch(&self) -> Option<&str> {
        let remote = self.remote_default.as_deref();
        remote
            .into_iter()
            .chain(["main", "master", "trunk", "develop"])
            .find(|name| self.branch(name).is_some())
    }

    /// The remote branch that `branch` tracks, if it still exists.
    pub fn upstream_of(&self, branch: &str) -> Option<&str> {
        match &self.branch(branch)?.upstream {
            Upstream::Tracking { name, .. } => Some(name),
            Upstream::None | Upstream::Gone { .. } => None,
        }
    }

    /// True if any worktree has uncommitted changes.
    pub fn is_dirty(&self) -> bool {
        self.worktrees.iter().any(|wt| wt.dirty() > 0)
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
    pub in_progress: Option<crate::conflicts::InProgress>,
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

    pub fn ahead(&self) -> usize {
        self.status.as_ref().map_or(0, |s| s.ahead)
    }

    pub fn behind(&self) -> usize {
        self.status.as_ref().map_or(0, |s| s.behind)
    }
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

/// Compare paths the way Windows does: case-insensitive, either slash, no
/// trailing separator.
pub fn same_path(a: &Path, b: &Path) -> bool {
    let norm = |p: &Path| {
        p.to_string_lossy()
            .replace('/', "\\")
            .trim_end_matches('\\')
            .to_lowercase()
    };
    norm(a) == norm(b)
}
