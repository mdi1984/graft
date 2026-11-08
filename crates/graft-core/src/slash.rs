//! Slash: clean up branches and worktrees that are done. Fetch with prune,
//! then propose branches whose upstream is gone, branches whose own commits
//! are merged into the default branch, branches never pushed (not
//! pre-selected), and worktree entries whose folder is missing. Worktrees go before their branches.
//! Never touched: the main worktree's branch, the default branch, protected
//! names, locked or dirty worktrees. Every deleted branch is reported with its
//! commit so it can be restored.

use std::collections::HashMap;
use std::path::PathBuf;

use anyhow::{Result, anyhow, bail};

use crate::cli::{Git, Progress};
use crate::jobs::CancelToken;
use crate::model::{DEFAULT_BRANCHES, Oid, RepoSnapshot, Upstream, Worktree};
use crate::paths::{display, folder_name, glob_match, pattern_list};
use crate::{refs, remote, repo, snapshot, worktree};

/// The branch name patterns Slash never proposes unless the user changes
/// them, besides the default branch: the usual default branch names and
/// release branches.
pub fn default_protected() -> String {
    let mut patterns = DEFAULT_BRANCHES.to_vec();
    patterns.push("release/*");
    patterns.join(", ")
}

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
    /// Stopped early; what was done before is in `slashed`.
    pub cancelled: bool,
}

fn protected(name: &str, patterns: &str) -> bool {
    pattern_list(patterns).any(|p| glob_match(p, name))
}

/// Where each of `branches` started: the commit of the oldest entry in its
/// reflog. Branches without a reflog (expired, or never written) are left out.
fn created_at(git: &Git, branches: &[String]) -> Result<HashMap<String, Oid>> {
    let mut oldest: HashMap<String, (u32, Oid)> = HashMap::new();
    if branches.is_empty() {
        return Ok(HashMap::new());
    }
    let input: String = branches
        .iter()
        .map(|b| {
            format!(
                "refs/heads/{b}
"
            )
        })
        .collect();
    // On stdin: hundreds of merged branches do not fit on the command line.
    let args = [
        "log",
        "-g",
        "--ignore-missing",
        "--stdin",
        "--format=%gD %H",
    ];
    let out = git.run_input(args, input.as_bytes())?.stdout_str();
    for line in out.lines() {
        let parsed = line.rsplit_once(' ').and_then(|(selector, oid)| {
            let (refname, index) = selector.rsplit_once("@{")?;
            let branch = refname.strip_prefix("refs/heads/")?;
            let index = index.strip_suffix('}')?.parse().ok()?;
            Some((branch, index, Oid::from_hex(oid.as_bytes()).ok()?))
        });
        let Some((branch, index, oid)) = parsed else {
            continue;
        };
        let entry = oldest.entry(branch.to_owned()).or_insert((index, oid));
        if index >= entry.0 {
            *entry = (index, oid);
        }
    }
    Ok(oldest.into_iter().map(|(b, (_, oid))| (b, oid)).collect())
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
        remote::fetch(git, true, cancel, on_progress)
            .err()
            .map(|e| format!("{e:#}"))
    } else {
        None
    };
    cancel.check()?;
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
    let created_at = created_at(git, &merged)?;
    let main_branch = snap.main_branch();

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
        if main_branch == Some(name) {
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
            // Unknown is not clean: removing it could lose work.
            Some(wt) if wt.status.is_none() => {
                out.kept
                    .push(keep("the status of its worktree could not be read"));
                continue;
            }
            Some(wt) if wt.in_progress.is_some() => {
                out.kept
                    .push(keep("its worktree has a merge or rebase in progress"));
                continue;
            }
            Some(wt) if wt.is_dirty() => {
                out.kept.push(keep("its worktree has uncommitted changes"));
                continue;
            }
            Some(wt) => Some(wt.path.clone()),
            None => None,
        };
        // A branch created from the default branch is "merged" before it has
        // commits of its own: a new worktree an agent has not committed in
        // yet, with its ignored .env and build output. Without a reflog to
        // tell, it is offered but not pre-selected.
        let suggested = match (reason, created_at.get(name)) {
            (Reason::Merged, Some(start)) if *start == branch.target => {
                out.kept.push(keep("it has no commits of its own yet"));
                continue;
            }
            (Reason::Merged, created) => created.is_some(),
            (reason, _) => reason != Reason::NeverPushed,
        };
        out.candidates.push(Candidate {
            reason,
            branch: Some(name.to_owned()),
            target: Some(branch.target),
            worktree,
            suggested,
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
/// after an error and reports everything. Reports each step to
/// `on_progress`; `cancel` stops before the next one. A branch that moved
/// since the scan (new commits) is kept, and so is its worktree.
pub fn execute(
    git: &Git,
    chosen: &[Candidate],
    cancel: &CancelToken,
    mut on_progress: impl FnMut(Progress),
) -> Outcome {
    let mut outcome = Outcome::default();
    let removals: Vec<&Candidate> = chosen
        .iter()
        .filter(|c| c.reason != Reason::MissingWorktree && c.worktree.is_some())
        .collect();
    let prune = chosen.iter().any(|c| c.reason == Reason::MissingWorktree);
    let deletions = chosen
        .iter()
        .filter(|c| c.branch.is_some() && c.target.is_some())
        .count();
    let total = removals.len() + usize::from(prune) + deletions;
    let mut done = 0;
    let mut step = |what: String| {
        if cancel.is_cancelled() {
            return false;
        }
        done += 1;
        on_progress(Progress::step(
            format!("{what} ({done} of {total})"),
            (done - 1) as f32 / total.max(1) as f32,
        ));
        true
    };

    let mut failed_worktrees = Vec::new();
    for c in removals {
        let path = c.worktree.as_ref().expect("filtered");
        if !step(format!("Removing worktree {}", folder_name(path))) {
            outcome.cancelled = true;
            return outcome;
        }
        let removed = match (&c.branch, c.target) {
            (Some(branch), Some(target)) if moved(git, branch, target) => Err(anyhow!(
                "its branch {branch} moved since the scan (new commits?), so both were kept."
            )),
            _ => worktree::remove(git, path, false),
        };
        match removed {
            Ok(()) => outcome.worktrees_removed += 1,
            Err(e) => {
                outcome
                    .errors
                    .push(format!("Worktree {}: {e:#}", display(path)));
                failed_worktrees.push(path.clone());
            }
        }
    }
    if prune {
        if !step("Forgetting missing worktrees".to_owned()) {
            outcome.cancelled = true;
            return outcome;
        }
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
        // A branch whose worktree stayed stays too.
        let kept = c
            .worktree
            .as_ref()
            .is_some_and(|p| failed_worktrees.contains(p));
        let what = if kept {
            format!("Keeping branch {branch}: its worktree is still there")
        } else {
            format!("Deleting branch {branch}")
        };
        if !step(what) {
            outcome.cancelled = true;
            return outcome;
        }
        if kept {
            continue;
        }
        match delete_branch(git, branch, target) {
            Ok(()) => outcome.slashed.push(Slashed {
                branch: branch.clone(),
                target,
            }),
            Err(e) => outcome.errors.push(format!("Branch {branch}: {e:#}")),
        }
    }
    outcome
}

/// Whether `branch` no longer points at `target`, the commit the scan saw
/// (or cannot be read).
fn moved(git: &Git, branch: &str, target: Oid) -> bool {
    !matches!(git.rev(&format!("refs/heads/{branch}")), Ok(Some(at)) if at == target)
}

const MOVED: &str = "it moved since the scan (new commits?), so it was kept.";

/// Delete `branch` only while it is still at `target` (git checks and
/// deletes in one step) and no worktree has it checked out or is rebasing
/// it, then drop its settings (upstream and the like) as `git branch -D`
/// would.
fn delete_branch(git: &Git, branch: &str, target: Oid) -> Result<()> {
    if moved(git, branch, target) {
        bail!(MOVED);
    }
    let full = format!("refs/heads/{branch}");
    let checked_out = git.read(["for-each-ref", "--format=%(worktreepath)", &full])?;
    if !checked_out.trim().is_empty() {
        bail!("it is checked out in a worktree, so it was kept.");
    }
    // Git sees a worktree detached while it rebases the branch.
    let worktrees = snapshot::load_worktrees(git, git.cwd())?;
    let rebased = |wt: &Worktree| repo::in_progress(&wt.path)?.branch;
    if worktrees
        .iter()
        .any(|wt| rebased(wt).as_deref() == Some(branch))
    {
        bail!("a worktree is rebasing it, so it was kept.");
    }
    if let Err(e) = git.run(["update-ref", "-d", &full, &target.to_string()]) {
        if moved(git, branch, target) {
            bail!(MOVED);
        }
        return Err(e);
    }
    let section = format!("branch.{branch}");
    git.run_accepting(["config", "--remove-section", &section], None, |_| true)?;
    Ok(())
}

/// Bring back a slashed branch at its old commit.
pub fn restore(git: &Git, slashed: &Slashed) -> Result<()> {
    refs::create_branch(git, &slashed.branch, &slashed.target.to_string(), false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn protected_patterns_in_every_list_form() {
        for list in ["main, release/*", "main; release/*", "main\nrelease/*"] {
            assert!(protected("release/1", list), "{list:?}");
            assert!(protected("main", list), "{list:?}");
        }
        assert!(!protected("feature/x", "main; release/*"));
    }
}
