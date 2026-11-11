//! Talking to remotes: fetch, pull and push, with progress and cancel.
//! Credentials go through git's own helpers (Git Credential Manager).

use std::path::{Path, PathBuf};

use anyhow::{Result, bail};

use crate::cli::{Git, Progress};
use crate::conflicts;
use crate::jobs::CancelToken;
use crate::model::{ORIGIN, RepoSnapshot, Upstream, branch_name, local_ref};
use crate::paths::{folder_name, same_path};
use crate::refs::{self, RefKind};
use crate::refusal::{forced_update, not_fast_forward};

/// Fetch every remote. With `prune`, remote branches deleted there go too,
/// so local branches tracking them show as gone.
pub fn fetch(
    git: &Git,
    prune: bool,
    cancel: &CancelToken,
    on_progress: impl FnMut(Progress),
) -> Result<()> {
    let mut args = vec!["fetch", "--all"];
    if prune {
        args.push("--prune");
    }
    args.push("--progress");
    git.run_progress(&args, cancel, on_progress)
}

/// Where a push of the current branch goes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PushPlan {
    /// To the upstream it tracks.
    Upstream,
    /// Publish it on `remote` and track it there (no upstream, or the
    /// upstream was deleted on the remote).
    Publish { remote: String },
}

/// The remote to push to when nothing else says which: [`ORIGIN`], or the
/// only remote. `None` when there is none or several others.
pub fn default_remote<'a>(remotes: impl IntoIterator<Item = &'a str>) -> Option<&'a str> {
    let remotes: Vec<&str> = remotes.into_iter().filter(|r| !r.is_empty()).collect();
    match remotes[..] {
        _ if remotes.contains(&ORIGIN) => Some(ORIGIN),
        [only] => Some(only),
        _ => None,
    }
}

/// Where a tag is pushed (its menu in the sidebar, the New tag dialog): the
/// [`default_remote`] of `snap`.
pub fn tag_remote(snap: &RepoSnapshot) -> Option<&str> {
    default_remote(snap.remotes.iter().map(|r| r.name.as_str()))
}

/// Where pushing a branch with this upstream goes, given the repository's
/// remotes: a branch without upstream goes to the [`default_remote`].
pub fn push_plan<'a>(
    upstream: &Upstream,
    remotes: impl IntoIterator<Item = &'a str>,
) -> Result<PushPlan, &'static str> {
    if matches!(upstream, Upstream::Tracking { .. }) {
        return Ok(PushPlan::Upstream);
    }
    let mut remotes = remotes.into_iter().filter(|r| !r.is_empty()).peekable();
    if remotes.peek().is_none() {
        return Err("There is no remote to push to. Add one first (e.g. origin).");
    }
    match default_remote(remotes) {
        Some(remote) => Ok(PushPlan::Publish {
            remote: remote.to_owned(),
        }),
        None => Err(
            "This branch has no upstream and there is no 'origin' remote. Pick a remote to push to.",
        ),
    }
}

/// The upstream a pull of `branch` takes from, or why there is none.
pub fn pull_check<'a>(branch: &str, upstream: &'a Upstream) -> Result<&'a str, String> {
    match upstream {
        Upstream::Tracking { name, .. } => Ok(name),
        Upstream::None => Err(format!(
            "{branch} has no upstream yet, so there is nothing to pull. Push publishes it."
        )),
        Upstream::Gone { name } => Err(format!("{name} was deleted on the remote")),
    }
}

/// Where a pull of a branch runs, and what it does in words.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PullPlan {
    /// The worktree that has the branch checked out; `None`: nowhere, so
    /// it only fast-forwards (see [`pull_branch`]).
    pub worktree: Option<PathBuf>,
    pub hint: String,
}

/// Whether the local branch `branch` can be pulled now, and where, or why
/// not. `selected` is the worktree on screen.
pub fn pull_plan(snap: &RepoSnapshot, branch: &str, selected: &Path) -> Result<PullPlan, String> {
    if snap.remotes.is_empty() {
        return Err("No remote to pull from. Add one under Remotes in the sidebar.".to_owned());
    }
    let Some(local) = snap.branch(branch) else {
        return Err("Check out a branch to pull from".to_owned());
    };
    let upstream = pull_check(branch, &local.upstream)?;
    // The selected worktree first: during a preview git sees main detached
    // and lets main's branch be checked out in another worktree too.
    let here = snap
        .worktrees
        .iter()
        .find(|wt| same_path(&wt.path, selected) && wt.branch.as_deref() == Some(branch));
    let Some(wt) = here.or_else(|| snap.worktree_for_branch(branch)) else {
        return match local.upstream {
            Upstream::Tracking {
                ahead, behind: 0, ..
            } if ahead > 0 => Err(format!(
                "Nothing to pull: {branch} only has commits that {upstream} lacks. \
                 Check it out to push them."
            )),
            Upstream::Tracking { ahead, .. } if ahead > 0 => Err(diverged(branch, upstream)),
            _ => Ok(PullPlan {
                worktree: None,
                hint: format!(
                    "Move {branch} ahead to {upstream}'s newest commit (it is not checked \
                     out, so only a fast-forward)"
                ),
            }),
        };
    };
    if wt.prunable {
        return Err(format!(
            "{branch} is checked out in a worktree whose folder is missing. \
             Prune missing worktrees first."
        ));
    }
    if let Some(reason) = snap.head_blocked(&wt.path) {
        return Err(reason.to_owned());
    }
    if wt.in_progress.is_some() {
        return Err(format!(
            "Finish or abort what is in progress in {} first",
            folder_name(&wt.path)
        ));
    }
    let hint = if same_path(&wt.path, selected) {
        format!("Pull {upstream} into {branch}")
    } else {
        format!(
            "Pull {upstream} into {branch}, in its worktree {}",
            folder_name(&wt.path)
        )
    };
    Ok(PullPlan {
        worktree: Some(wt.path.clone()),
        hint,
    })
}

/// Why a branch checked out nowhere can't simply move ahead to `upstream`.
/// Its own commits look the same as a rewritten upstream, so this doesn't
/// suggest merging them back.
fn diverged(branch: &str, upstream: &str) -> String {
    format!(
        "{branch} and {upstream} have diverged: {branch} has commits {upstream} lacks, \
         or {upstream} was force-pushed. Check {branch} out to sort it out."
    )
}

/// Pull the current branch from its upstream (merge or rebase, as
/// configured). Ok(true): stopped with conflicts.
pub fn pull(git: &Git, cancel: &CancelToken, on_progress: impl FnMut(Progress)) -> Result<bool> {
    let branch = current_branch(git)?;
    if let Err(why) = pull_check(&branch, &upstream(git)?) {
        bail!(why);
    }
    let mut args = vec!["pull", "--progress"];
    if !says_how_to_pull(git, &branch)? {
        // Git's old default, which Git for Windows' installer sets too
        // (pull.rebase=false); since git 2.33 a pull of a diverged branch
        // fails without any setting, as in WSL.
        args.push("--no-rebase");
    }
    let accept = conflicts::done_or_stopped(git)?;
    let out = git.run_progress_accepting(&args, cancel, on_progress, accept)?;
    Ok(!out.success())
}

/// Pull `branch`, checked out here or not. Checked out here: [`pull`]
/// (merge or rebase, as configured). Elsewhere: fetch its upstream and
/// move it ahead only when that loses nothing (a fast-forward); git
/// refuses a branch that diverged, and one checked out in any worktree.
/// Ok(true): stopped with conflicts.
pub fn pull_branch(
    git: &Git,
    branch: &str,
    cancel: &CancelToken,
    on_progress: impl FnMut(Progress),
) -> Result<bool> {
    if git.current_branch()?.as_deref() == Some(branch) {
        return pull(git, cancel, on_progress);
    }
    let key = |what: &str| git.config(&["--get", &format!("branch.{branch}.{what}")]);
    let (Some(remote), Some(merge)) = (key("remote")?, key("merge")?) else {
        bail!("{branch} has no upstream yet, so there is nothing to pull. Push publishes it.");
    };
    let refspec = format!("{merge}:{}", local_ref(branch));
    let args = [
        "fetch",
        "--progress",
        "--no-write-fetch-head",
        &remote,
        &refspec,
    ];
    let tip = || -> Result<String> {
        let out = git.run_raw(["rev-parse", "--verify", "-q", &local_ref(branch)])?;
        Ok(out.stdout_str().trim().to_owned())
    };
    let before = tip()?;
    let Err(error) = git.run_progress(&args, cancel, on_progress) else {
        return Ok(false);
    };
    let text = format!("{error:#}");
    // One fetch updates the remote-tracking branch too; git can refuse
    // that one (a fetch refspec without `+`) after moving the branch.
    if tip()? != before {
        bail!("{branch} moved ahead, but git also reported a problem:\n{text}");
    }
    if !not_fast_forward(&text) {
        return Err(error);
    }
    if forced_update(&text) {
        bail!(
            "{branch}'s upstream was rewritten on the remote (force-pushed), so {branch} \
             can't simply move ahead to it. Check {branch} out to decide what to keep."
        );
    }
    bail!(diverged(branch, "its upstream"))
}

/// Whether any config scope says how a pull of `branch` reconciles a
/// diverged upstream (merge, rebase or fast-forward only).
fn says_how_to_pull(git: &Git, branch: &str) -> Result<bool> {
    for key in ["pull.rebase", &format!("branch.{branch}.rebase"), "pull.ff"] {
        if git.config(&["--get", key])?.is_some() {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Push the current branch (see [`push_plan`]).
pub fn push(git: &Git, cancel: &CancelToken, on_progress: impl FnMut(Progress)) -> Result<()> {
    let branch = current_branch(git)?;
    let remotes = git.read(["remote"])?;
    match push_plan(&upstream(git)?, remotes.lines()) {
        Ok(PushPlan::Upstream) => git.run_progress(&["push", "--progress"], cancel, on_progress),
        Ok(PushPlan::Publish { remote }) => git.run_progress(
            &[
                "push",
                "--progress",
                "--set-upstream",
                &remote,
                &local_ref(&branch),
            ],
            cancel,
            on_progress,
        ),
        Err(why) => bail!(why),
    }
}

/// Push the current branch over a diverged upstream, but only if the upstream
/// is still where we last saw it and its commits were once in our branch:
/// someone else's commits are never lost, even after a background fetch.
/// (The second check needs git's log of the upstream, which a fresh clone
/// lacks; until a fetch or push writes one, only the first applies.)
pub fn force_push(
    git: &Git,
    cancel: &CancelToken,
    on_progress: impl FnMut(Progress),
) -> Result<()> {
    current_branch(git)?;
    if upstream(git)? == Upstream::None {
        return push(git, cancel, on_progress);
    }
    let mut args = vec!["push", "--progress", "--force-with-lease"];
    // It reads the remote-tracking branch's reflog and refuses every push
    // without one (a fresh clone, core.logAllRefUpdates=false).
    if push_target_has_reflog(git)? {
        args.push("--force-if-includes");
    }
    git.run_progress(&args, cancel, on_progress)
}

/// Whether the remote-tracking branch a push of the current branch updates
/// has a reflog.
fn push_target_has_reflog(git: &Git) -> Result<bool> {
    let out = git.run_raw(["rev-parse", "--symbolic-full-name", "@{push}"])?;
    let target = out.stdout_str().trim().to_owned();
    if !out.success() || target.is_empty() {
        return Ok(false);
    }
    let exists = git.run_expecting(["reflog", "exists", &target], None, |o| o.status <= 1)?;
    Ok(exists.success())
}

/// Delete a branch on its remote.
pub fn delete_remote_branch(
    git: &Git,
    remote: &str,
    branch: &str,
    cancel: &CancelToken,
    progress: impl FnMut(Progress),
) -> Result<()> {
    git.run_progress(
        &["push", "--progress", remote, "--delete", &local_ref(branch)],
        cancel,
        progress,
    )
}

pub fn push_tag(
    git: &Git,
    remote: &str,
    tag: &str,
    cancel: &CancelToken,
    progress: impl FnMut(Progress),
) -> Result<()> {
    let refspec = format!("refs/tags/{tag}");
    git.run_progress(&["push", "--progress", remote, &refspec], cancel, progress)
}

fn current_branch(git: &Git) -> Result<String> {
    match git.current_branch()? {
        Some(branch) => Ok(branch),
        None => {
            bail!("HEAD is detached, so there is no branch to push or pull. Create a branch first.")
        }
    }
}

/// The current branch's upstream as far as git can resolve it: one deleted
/// on the remote counts as none.
fn upstream(git: &Git) -> Result<Upstream> {
    let out = git.run_raw(["rev-parse", "--symbolic-full-name", "@{u}"])?;
    let full = out.stdout_str();
    let name = branch_name(full.trim()).unwrap_or_default();
    Ok(if out.success() && !name.is_empty() {
        Upstream::Tracking {
            name: name.to_owned(),
            ahead: 0,
            behind: 0,
        }
    } else {
        Upstream::None
    })
}

/// Add a remote, or change one (`original` is its current name): rename it
/// and set its URL, or its push URL if it has only that (`push_only`). With
/// `fetch`, fetch from it afterwards.
pub fn save_remote(
    git: &Git,
    original: Option<&str>,
    name: &str,
    url: &str,
    push_only: bool,
    fetch: bool,
) -> Result<()> {
    refs::check_name(git, RefKind::Remote, name)?;
    match original {
        None => {
            git.run(["remote", "add", name, url])?;
        }
        Some(old) => {
            if old != name {
                git.run(["remote", "rename", old, name])?;
            }
            let mut args = vec!["remote", "set-url"];
            if push_only {
                args.push("--push");
            }
            args.extend([name, url]);
            git.run(args)?;
        }
    }
    if fetch && let Err(error) = git.run(["fetch", name]) {
        // A new remote that can't be fetched is most likely a typo in the URL:
        // take it back out so the dialog can simply be tried again.
        if original.is_none() {
            let _ = git.run(["remote", "remove", name]);
        }
        return Err(error);
    }
    Ok(())
}

/// Remove a remote and its remote branches.
pub fn remove_remote(git: &Git, name: &str) -> Result<()> {
    git.run(["remote", "remove", name])?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn push_targets() {
        let tracking = Upstream::Tracking {
            name: "origin/x".into(),
            ahead: 1,
            behind: 0,
        };
        assert_eq!(push_plan(&tracking, []), Ok(PushPlan::Upstream));
        let publish = |remote: &str| {
            Ok(PushPlan::Publish {
                remote: remote.to_owned(),
            })
        };
        assert_eq!(
            push_plan(&Upstream::None, ["backup", "origin"]),
            publish("origin")
        );
        let gone = Upstream::Gone {
            name: "origin/x".into(),
        };
        assert_eq!(push_plan(&gone, ["fork"]), publish("fork"));
        assert!(push_plan(&Upstream::None, []).is_err());
        assert!(push_plan(&Upstream::None, ["a", "b"]).is_err());
        assert_eq!(default_remote(["fork", "origin"]), Some("origin"));
        assert_eq!(default_remote(["fork"]), Some("fork"));
        assert_eq!(default_remote(["a", "b"]), None);
        assert_eq!(pull_check("x", &tracking), Ok("origin/x"));
        assert!(pull_check("x", &gone).is_err());
    }
}
