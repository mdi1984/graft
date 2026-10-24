//! Talking to remotes: fetch, pull and push, with progress and cancel.
//! Credentials go through git's own helpers (Git Credential Manager).

use anyhow::{Result, bail};

use crate::cli::{CancelToken, Git, failure_message};
use crate::repo::{Progress, parse_progress};

/// Fetch every remote.
pub fn fetch(git: &Git, cancel: &CancelToken, on_progress: impl FnMut(Progress)) -> Result<()> {
    run(git, &["fetch", "--all", "--progress"], cancel, on_progress)
}

/// Fetch every remote and drop remote branches that were deleted there, so
/// local branches tracking them show as gone.
pub fn fetch_prune(
    git: &Git,
    cancel: &CancelToken,
    on_progress: impl FnMut(Progress),
) -> Result<()> {
    run(
        git,
        &["fetch", "--all", "--prune", "--progress"],
        cancel,
        on_progress,
    )
}

/// Pull the current branch from its upstream (merge or rebase, as configured).
pub fn pull(git: &Git, cancel: &CancelToken, on_progress: impl FnMut(Progress)) -> Result<()> {
    current_branch(git)?;
    if upstream(git)?.is_none() {
        bail!("This branch has no upstream yet, so there is nothing to pull. Push it first.");
    }
    run(git, &["pull", "--progress"], cancel, on_progress)
}

/// Push the current branch. A branch without upstream is pushed to `origin`
/// (or the only remote) and starts tracking it.
pub fn push(git: &Git, cancel: &CancelToken, on_progress: impl FnMut(Progress)) -> Result<()> {
    let branch = current_branch(git)?;
    if upstream(git)?.is_some() {
        return run(git, &["push", "--progress"], cancel, on_progress);
    }
    let remotes = git.read(["remote"])?;
    let remotes: Vec<&str> = remotes.lines().filter(|l| !l.is_empty()).collect();
    let remote = if remotes.contains(&"origin") {
        "origin"
    } else if let [only] = remotes[..] {
        only
    } else if remotes.is_empty() {
        bail!("There is no remote to push to. Add one first (e.g. origin).");
    } else {
        bail!(
            "This branch has no upstream and there is no 'origin' remote. Pick a remote to push to."
        );
    };
    run(
        git,
        &["push", "--progress", "--set-upstream", remote, &branch],
        cancel,
        on_progress,
    )
}

/// Push the current branch over a diverged upstream, but only if the upstream
/// is still where we last saw it (someone else's new commits are never lost).
pub fn force_push(
    git: &Git,
    cancel: &CancelToken,
    on_progress: impl FnMut(Progress),
) -> Result<()> {
    current_branch(git)?;
    if upstream(git)?.is_none() {
        return push(git, cancel, on_progress);
    }
    run(
        git,
        &["push", "--progress", "--force-with-lease"],
        cancel,
        on_progress,
    )
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

fn upstream(git: &Git) -> Result<Option<String>> {
    let out = git.run_raw(["rev-parse", "--abbrev-ref", "--symbolic-full-name", "@{u}"])?;
    Ok(out
        .success()
        .then(|| out.stdout_str().trim().to_owned())
        .filter(|s| !s.is_empty()))
}

pub(crate) fn run(
    git: &Git,
    args: &[&str],
    cancel: &CancelToken,
    mut on_progress: impl FnMut(Progress),
) -> Result<()> {
    let out = git.run_with_progress(args, cancel.flag(), |line| {
        on_progress(parse_progress(line))
    })?;
    if !out.success() {
        bail!(failure_message(&out));
    }
    Ok(())
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
