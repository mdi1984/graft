//! Talking to remotes: fetch, pull and push, with progress and cancel.
//! Credentials go through git's own helpers (Git Credential Manager).

use anyhow::{Result, bail};

use crate::cli::{Git, Progress};
use crate::jobs::CancelToken;
use crate::model::{ORIGIN, Upstream, branch_name, local_ref};
use crate::refs::{self, RefKind};

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

/// Pull the current branch from its upstream (merge or rebase, as configured).
pub fn pull(git: &Git, cancel: &CancelToken, on_progress: impl FnMut(Progress)) -> Result<()> {
    let branch = current_branch(git)?;
    if let Err(why) = pull_check(&branch, &upstream(git)?) {
        bail!(why);
    }
    git.run_progress(&["pull", "--progress"], cancel, on_progress)
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
