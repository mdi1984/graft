//! Branches, tags and commit-level operations: create, switch, rename, delete,
//! reset.

use anyhow::{Result, bail};

use crate::cli::Git;

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
    fn flag(self) -> &'static str {
        match self {
            ResetMode::Soft => "--soft",
            ResetMode::Mixed => "--mixed",
            ResetMode::Hard => "--hard",
        }
    }
}

/// What a name is for, in [`check_ref_name`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RefKind {
    Branch,
    Tag,
    Remote,
}

impl RefKind {
    fn noun(self) -> &'static str {
        match self {
            RefKind::Branch => "branch",
            RefKind::Tag => "tag",
            RefKind::Remote => "remote",
        }
    }
}

/// The rules a name must pass before git is asked: not empty, no spaces.
/// The error says what is wrong, for showing in a dialog.
pub fn check_ref_name(kind: RefKind, name: &str) -> Result<(), String> {
    let noun = kind.noun();
    if name.trim().is_empty() {
        return Err(format!("Give the {noun} a name."));
    }
    if name.contains(char::is_whitespace) {
        let mut names = noun.to_owned();
        names[..1].make_ascii_uppercase();
        return Err(format!("{names} names cannot contain spaces."));
    }
    Ok(())
}

/// Whether git accepts `name` as a branch name; the error says why not.
pub fn check_branch_name(git: &Git, name: &str) -> Result<()> {
    check_name(git, RefKind::Branch, name)
}

/// [`check_ref_name`], then git's own rules for a branch or tag name.
fn check_name(git: &Git, kind: RefKind, name: &str) -> Result<()> {
    if let Err(why) = check_ref_name(kind, name) {
        bail!(why);
    }
    let full = match kind {
        RefKind::Branch => None,
        RefKind::Tag => Some(format!("refs/tags/{name}")),
        RefKind::Remote => Some(format!("refs/remotes/{name}")),
    };
    let args = match &full {
        None => vec!["check-ref-format", "--branch", name],
        Some(full) => vec!["check-ref-format", full],
    };
    if !git.run_raw(args)?.success() {
        bail!("{name} is not a valid {} name.", kind.noun());
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
    git.run(["switch", "--end-of-options", branch])?;
    Ok(())
}

/// Create a local branch tracking `remote_branch` (e.g. `origin/x`) and switch to it.
pub fn switch_tracking(git: &Git, local: &str, remote_branch: &str) -> Result<()> {
    git.run(["switch", "-c", local, "--track", remote_branch])?;
    Ok(())
}

/// Check out a commit without a branch.
pub fn switch_detached(git: &Git, rev: &str) -> Result<()> {
    git.run(["switch", "--detach", "--end-of-options", rev])?;
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
    git.run([
        "branch",
        if force { "-D" } else { "-d" },
        "--end-of-options",
        name,
    ])?;
    Ok(())
}

/// Create a tag at `target`; with a message it is an annotated tag.
pub fn create_tag(git: &Git, name: &str, target: &str, message: Option<&str>) -> Result<()> {
    check_name(git, RefKind::Tag, name)?;
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

pub fn reset(git: &Git, rev: &str, mode: ResetMode) -> Result<()> {
    // `rev -- ` rather than `--end-of-options rev`: git before 2.44 (e.g.
    // Ubuntu 24.04's, in WSL) rejects the latter for reset.
    git.run(["reset", mode.flag(), rev, "--"])?;
    Ok(())
}

/// Undo the last commit but keep its changes staged.
pub fn undo_last_commit(git: &Git) -> Result<()> {
    if git.rev("HEAD~1")?.is_none() {
        bail!("The first commit cannot be undone this way.");
    }
    reset(git, "HEAD~1", ResetMode::Soft)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn name_rules() {
        assert!(check_ref_name(RefKind::Branch, "feature/x").is_ok());
        assert_eq!(
            check_ref_name(RefKind::Branch, " "),
            Err("Give the branch a name.".to_owned())
        );
        assert_eq!(
            check_ref_name(RefKind::Tag, "v 1"),
            Err("Tag names cannot contain spaces.".to_owned())
        );
    }
}
