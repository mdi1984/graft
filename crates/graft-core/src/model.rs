use std::path::PathBuf;

/// Everything the sidebar and toolbar need to know about one open repository.
#[derive(Clone, Debug, Default)]
pub struct RepoSnapshot {
    pub name: String,
    pub path: PathBuf,
    pub worktrees: Vec<Worktree>,
    pub branches: Vec<Branch>,
    pub remotes: Vec<String>,
    pub stash_count: usize,
    pub tag_count: usize,
}

impl RepoSnapshot {
    /// The worktree at the repository root (the one that is not a linked worktree).
    pub fn main_worktree(&self) -> Option<&Worktree> {
        self.worktrees.iter().find(|wt| wt.is_main)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Worktree {
    /// Checked-out branch, or `None` for a detached HEAD.
    pub branch: Option<String>,
    pub path: PathBuf,
    pub is_main: bool,
    /// Number of changed files (staged + unstaged + untracked).
    pub dirty: usize,
    /// Commits not yet pushed to the upstream.
    pub ahead: usize,
    pub behind: usize,
}

impl Worktree {
    pub fn label(&self) -> &str {
        self.branch.as_deref().unwrap_or("(detached)")
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Branch {
    pub name: String,
    pub upstream: Upstream,
    /// True for the branch checked out in the main worktree.
    pub is_head: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Upstream {
    /// No upstream configured: the branch only exists locally.
    None,
    Tracking {
        ahead: usize,
        behind: usize,
    },
    /// An upstream is configured but the remote branch was deleted.
    Gone,
}
