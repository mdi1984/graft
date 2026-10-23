//! Branches, tags and commit-level operations: create, switch, rename, delete,
//! cherry-pick, revert, reset.

use anyhow::{Result, bail};

use crate::Git;
use crate::cli::CancelToken;
use crate::remote;
use crate::repo::Progress;

/// How far `git reset` goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResetMode {
    /// Move the branch; keep changes staged.
    Soft,
    /// Move the branch; keep changes in the working tree, unstaged.
    Mixed,
    /// Move the branch and throw the changes away.
    Hard,
}

impl ResetMode {
    pub fn flag(self) -> &'static str {
        match self {
            ResetMode::Soft => "--soft",
            ResetMode::Mixed => "--mixed",
            ResetMode::Hard => "--hard",
        }
    }
}

/// Whether git accepts `name` as a branch name; the error says why not.
pub fn check_branch_name(git: &Git, name: &str) -> Result<()> {
    if name.trim().is_empty() {
        bail!("Give the branch a name.");
    }
    if name.contains(char::is_whitespace) {
        bail!("Branch names cannot contain spaces.");
    }
    let out = git.run_raw(["check-ref-format", "--branch", name])?;
    if !out.success() {
        bail!("{name} is not a valid branch name.");
    }
    Ok(())
}

/// Create `name` at `start`; with `checkout`, switch the worktree to it.
pub fn create_branch(git: &Git, name: &str, start: &str, checkout: bool) -> Result<()> {
    check_branch_name(git, name)?;
    if checkout {
        git.run(["switch", "-c", name, start])?;
    } else {
        git.run(["branch", name, start])?;
    }
    Ok(())
}

/// Switch the worktree to a local branch.
pub fn switch(git: &Git, branch: &str) -> Result<()> {
    git.run(["switch", branch])?;
    Ok(())
}

/// Create a local branch tracking `remote_branch` (e.g. `origin/x`) and switch to it.
pub fn switch_tracking(git: &Git, local: &str, remote_branch: &str) -> Result<()> {
    git.run(["switch", "-c", local, "--track", remote_branch])?;
    Ok(())
}

/// Check out a commit without a branch.
pub fn switch_detached(git: &Git, rev: &str) -> Result<()> {
    git.run(["switch", "--detach", rev])?;
    Ok(())
}

pub fn rename_branch(git: &Git, old: &str, new: &str) -> Result<()> {
    check_branch_name(git, new)?;
    git.run(["branch", "-m", old, new])?;
    Ok(())
}

/// Delete a local branch. Without `force`, git refuses if it is not merged;
/// the error then contains [`NOT_MERGED`].
pub fn delete_branch(git: &Git, name: &str, force: bool) -> Result<()> {
    git.run(["branch", if force { "-D" } else { "-d" }, name])?;
    Ok(())
}

/// Part of git's message when `branch -d` refuses an unmerged branch.
pub const NOT_MERGED: &str = "not fully merged";

/// Delete a branch on its remote.
pub fn delete_remote_branch(
    git: &Git,
    remote: &str,
    branch: &str,
    cancel: &CancelToken,
    progress: impl FnMut(Progress),
) -> Result<()> {
    remote::run(
        git,
        &["push", "--progress", remote, "--delete", branch],
        cancel,
        progress,
    )
}

/// Create a tag at `target`; with a message it is an annotated tag.
pub fn create_tag(git: &Git, name: &str, target: &str, message: Option<&str>) -> Result<()> {
    if name.trim().is_empty() || name.contains(char::is_whitespace) {
        bail!("Give the tag a name without spaces.");
    }
    let out = git.run_raw(["check-ref-format", &format!("refs/tags/{name}")])?;
    if !out.success() {
        bail!("{name} is not a valid tag name.");
    }
    match message.map(str::trim).filter(|m| !m.is_empty()) {
        Some(message) => git.run(["tag", "-a", name, "-m", message, target])?,
        None => git.run(["tag", name, target])?,
    };
    Ok(())
}

pub fn delete_tag(git: &Git, name: &str) -> Result<()> {
    git.run(["tag", "-d", name])?;
    Ok(())
}

pub fn push_tag(
    git: &Git,
    remote: &str,
    tag: &str,
    cancel: &CancelToken,
    progress: impl FnMut(Progress),
) -> Result<()> {
    let refspec = format!("refs/tags/{tag}");
    remote::run(
        git,
        &["push", "--progress", remote, &refspec],
        cancel,
        progress,
    )
}

pub fn cherry_pick(git: &Git, rev: &str) -> Result<()> {
    git.run(["cherry-pick", rev])?;
    Ok(())
}

pub fn revert(git: &Git, rev: &str) -> Result<()> {
    git.run(["revert", "--no-edit", rev])?;
    Ok(())
}

pub fn reset(git: &Git, rev: &str, mode: ResetMode) -> Result<()> {
    git.run(["reset", mode.flag(), rev])?;
    Ok(())
}

/// Undo the last commit but keep its changes staged.
pub fn undo_last_commit(git: &Git) -> Result<()> {
    let out = git.run_raw(["rev-parse", "--verify", "--quiet", "HEAD~1"])?;
    if !out.success() {
        bail!("The first commit cannot be undone this way.");
    }
    reset(git, "HEAD~1", ResetMode::Soft)
}
