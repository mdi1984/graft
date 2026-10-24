//! Slash: clean up branches and worktrees that are done. Fetch with prune,
//! then propose branches whose upstream is gone, branches merged into the
//! default branch, branches never pushed (not pre-selected), and worktree
//! entries whose folder is missing. Worktrees go before their branches.
//! Never touched: the main worktree's branch, the default branch, protected
//! names, locked or dirty worktrees. Every deleted branch is reported with its
//! commit so it can be restored.

use std::path::PathBuf;

use anyhow::Result;

use crate::cli::{CancelToken, Git};
use crate::model::{Oid, RepoSnapshot, Upstream};
use crate::repo::Progress;
use crate::worktree::glob_match;
use crate::{remote, snapshot, worktree};

/// Branch names Slash never proposes, besides the default branch.
pub const PROTECTED: &str = "main, master, develop, trunk, release/*";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reason {
    /// The upstream branch was deleted on the remote (usually: PR merged).
    Gone,
    /// Everything on it is in the default branch.
    Merged,
    /// No upstream and not merged: its commits exist only here.
    NeverPushed,
    /// A worktree whose folder no longer exists.
    MissingWorktree,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Candidate {
    pub reason: Reason,
    pub branch: Option<String>,
    /// The branch's commit, to restore it.
    pub target: Option<Oid>,
    /// A clean worktree that has the branch checked out (removed first), or
    /// the missing worktree to prune.
    pub worktree: Option<PathBuf>,
    /// Pre-selected in the dialog.
    pub suggested: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Plan {
    pub default_branch: Option<String>,
    pub candidates: Vec<Candidate>,
    /// Branches that would qualify but are kept, and why.
    pub kept: Vec<(String, String)>,
    /// The fetch failed (offline?); the plan uses what was fetched before.
    pub fetch_error: Option<String>,
}

/// A deleted branch, for "Recently slashed".
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Slashed {
    pub branch: String,
    pub target: Oid,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Outcome {
    pub slashed: Vec<Slashed>,
    pub worktrees_removed: usize,
    pub errors: Vec<String>,
}

fn protected(name: &str, patterns: &str) -> bool {
    patterns
        .split(',')
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .any(|p| glob_match(p, name))
}

/// Fetch (with prune) and work out what could go. `git` points at the main
/// worktree.
pub fn scan(
    git: &Git,
    patterns: &str,
    cancel: &CancelToken,
    on_progress: impl FnMut(Progress),
) -> Result<Plan> {
    let has_remotes = !git.read(["remote"])?.trim().is_empty();
    let fetch_error = if has_remotes {
        remote::fetch_prune(git, cancel, on_progress)
            .err()
            .map(|e| format!("{e:#}"))
    } else {
        None
    };
    if cancel.is_cancelled() {
        anyhow::bail!("Cancelled");
    }
    let snap = snapshot::load(git)?;
    let mut plan = plan(git, &snap, patterns)?;
    plan.fetch_error = fetch_error;
    Ok(plan)
}

/// The candidates for this snapshot (after fetching).
pub fn plan(git: &Git, snap: &RepoSnapshot, patterns: &str) -> Result<Plan> {
    let default = snap.default_branch().map(str::to_owned);
    let merged: Vec<String> = match &default {
        Some(d) => git
            .read([
                "for-each-ref",
                "--format=%(refname:short)",
                "--merged",
                d,
                "refs/heads",
            ])?
            .lines()
            .map(str::to_owned)
            .collect(),
        None => Vec::new(),
    };
    let main_branch = snap.main_worktree().and_then(|wt| wt.branch.clone());

    let mut out = Plan {
        default_branch: default.clone(),
        ..Plan::default()
    };
    for branch in &snap.branches {
        let name = branch.name.as_str();
        let is_merged = merged.iter().any(|m| m == name);
        let reason = match &branch.upstream {
            Upstream::Gone { .. } => Reason::Gone,
            _ if is_merged => Reason::Merged,
            Upstream::None => Reason::NeverPushed,
            Upstream::Tracking { .. } => continue,
        };
        let keep = |why: &str| (name.to_owned(), why.to_owned());
        if default.as_deref() == Some(name) || protected(name, patterns) {
            if reason != Reason::NeverPushed {
                out.kept.push(keep("protected name"));
            }
            continue;
        }
        if main_branch.as_deref() == Some(name) {
            if reason != Reason::NeverPushed {
                out.kept.push(keep("checked out in the main worktree"));
            }
            continue;
        }
        let worktree = match snap.worktree_for_branch(name) {
            Some(wt) if wt.prunable => None,
            Some(wt) if wt.locked => {
                out.kept.push(keep("its worktree is locked"));
                continue;
            }
            Some(wt) if wt.dirty() > 0 => {
                out.kept.push(keep("its worktree has uncommitted changes"));
                continue;
            }
            Some(wt) if wt.in_progress.is_some() => {
                out.kept
                    .push(keep("its worktree has a merge or rebase in progress"));
                continue;
            }
            Some(wt) => Some(wt.path.clone()),
            None => None,
        };
        out.candidates.push(Candidate {
            reason,
            branch: Some(name.to_owned()),
            target: Some(branch.target),
            worktree,
            suggested: reason != Reason::NeverPushed,
        });
    }
    for wt in snap.worktrees.iter().filter(|wt| wt.prunable && !wt.locked) {
        out.candidates.push(Candidate {
            reason: Reason::MissingWorktree,
            branch: None,
            target: None,
            worktree: Some(wt.path.clone()),
            suggested: true,
        });
    }
    // Gone first, then merged, missing worktrees, never pushed.
    let rank = |r: Reason| match r {
        Reason::Gone => 0,
        Reason::Merged => 1,
        Reason::MissingWorktree => 2,
        Reason::NeverPushed => 3,
    };
    out.candidates.sort_by_key(|c| rank(c.reason));
    Ok(out)
}

/// Delete the chosen candidates: worktrees first, then branches. Keeps going
/// after an error and reports everything.
pub fn execute(git: &Git, chosen: &[Candidate]) -> Outcome {
    let mut outcome = Outcome::default();
    let mut failed_worktrees = Vec::new();
    for c in chosen
        .iter()
        .filter(|c| c.reason != Reason::MissingWorktree)
    {
        if let Some(path) = &c.worktree {
            match worktree::remove(git, path, false) {
                Ok(()) => outcome.worktrees_removed += 1,
                Err(e) => {
                    outcome
                        .errors
                        .push(format!("Worktree {}: {e:#}", path.display()));
                    failed_worktrees.push(path.clone());
                }
            }
        }
    }
    if chosen.iter().any(|c| c.reason == Reason::MissingWorktree) {
        match worktree::prune(git) {
            Ok(()) => {
                outcome.worktrees_removed += chosen
                    .iter()
                    .filter(|c| c.reason == Reason::MissingWorktree)
                    .count();
            }
            Err(e) => outcome.errors.push(format!("Pruning worktrees: {e:#}")),
        }
    }
    for c in chosen {
        let (Some(branch), Some(target)) = (&c.branch, c.target) else {
            continue;
        };
        if c.worktree
            .as_ref()
            .is_some_and(|p| failed_worktrees.contains(p))
        {
            continue;
        }
        match git.run(["branch", "-D", "--end-of-options", branch]) {
            Ok(_) => outcome.slashed.push(Slashed {
                branch: branch.clone(),
                target,
            }),
            Err(e) => outcome.errors.push(format!("Branch {branch}: {e:#}")),
        }
    }
    outcome
}

/// Bring back a slashed branch at its old commit.
pub fn restore(git: &Git, slashed: &Slashed) -> Result<()> {
    git.run(["branch", &slashed.branch, &slashed.target.to_string()])?;
    Ok(())
}
