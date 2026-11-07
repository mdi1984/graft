//! Talking to remotes: fetch, pull and push, with progress and cancel.
//! Credentials go through git's own helpers (Git Credential Manager).

use anyhow::{Result, bail};

use crate::cli::{Git, Progress};
use crate::jobs::CancelToken;
use crate::model::Upstream;

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

/// Where pushing a branch with this upstream goes, given the repository's
/// remotes: a branch without upstream goes to `origin`, or the only remote.
pub fn push_plan<'a>(
    upstream: &Upstream,
    remotes: impl IntoIterator<Item = &'a str>,
) -> Result<PushPlan, &'static str> {
    if matches!(upstream, Upstream::Tracking { .. }) {
        return Ok(PushPlan::Upstream);
    }
    let remotes: Vec<&str> = remotes.into_iter().filter(|r| !r.is_empty()).collect();
    let remote = if remotes.contains(&"origin") {
        "origin"
    } else if let [only] = remotes[..] {
        only
    } else if remotes.is_empty() {
        return Err("There is no remote to push to. Add one first (e.g. origin).");
    } else {
        return Err(
            "This branch has no upstream and there is no 'origin' remote. Pick a remote to push to.",
        );
    };
    Ok(PushPlan::Publish {
        remote: remote.to_owned(),
    })
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
            &["push", "--progress", "--set-upstream", &remote, &branch],
            cancel,
            on_progress,
        ),
        Err(why) => bail!(why),
    }
}

/// Push the current branch over a diverged upstream, but only if the upstream
/// is still where we last saw it (someone else's new commits are never lost).
pub fn force_push(
    git: &Git,
    cancel: &CancelToken,
    on_progress: impl FnMut(Progress),
) -> Result<()> {
    current_branch(git)?;
    if upstream(git)? == Upstream::None {
        return push(git, cancel, on_progress);
    }
    git.run_progress(
        &["push", "--progress", "--force-with-lease"],
        cancel,
        on_progress,
    )
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
        &["push", "--progress", remote, "--delete", branch],
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

/// Whether a push failed because the remote has commits we don't.
pub fn is_rejected_push(error: &str) -> bool {
    error.contains("[rejected]")
        || error.contains("non-fast-forward")
        || error.contains("fetch first")
}

fn current_branch(git: &Git) -> Result<String> {
    let out = git.run_raw(["symbolic-ref", "--short", "-q", "HEAD"])?;
    let branch = out.stdout_str().trim().to_owned();
    if !out.success() || branch.is_empty() {
        bail!("HEAD is detached, so there is no branch to push or pull. Create a branch first.");
    }
    Ok(branch)
}

/// The current branch's upstream as far as git can resolve it: one deleted
/// on the remote counts as none.
fn upstream(git: &Git) -> Result<Upstream> {
    let out = git.run_raw(["rev-parse", "--abbrev-ref", "--symbolic-full-name", "@{u}"])?;
    let name = out.stdout_str().trim().to_owned();
    Ok(if out.success() && !name.is_empty() {
        Upstream::Tracking {
            name,
            ahead: 0,
            behind: 0,
        }
    } else {
        Upstream::None
    })
}

/// Add a remote, or change one (`original` is its current name): rename it
/// and set its URL. With `fetch`, fetch from it afterwards.
pub fn save_remote(
    git: &Git,
    original: Option<&str>,
    name: &str,
    url: &str,
    fetch: bool,
) -> Result<()> {
    match original {
        None => {
            git.run(["remote", "add", name, url])?;
        }
        Some(old) => {
            if old != name {
                git.run(["remote", "rename", old, name])?;
            }
            git.run(["remote", "set-url", name, url])?;
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
        assert_eq!(pull_check("x", &tracking), Ok("origin/x"));
        assert!(pull_check("x", &gone).is_err());
    }
}
