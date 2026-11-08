//! Branches, tags and commit-level operations: create, switch, rename, delete,
//! reset; and telling git's refusals apart.

use anyhow::{Result, bail};

use crate::Git;
pub use crate::conflicts::{cherry_pick, revert};

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
    if let Err(why) = check_ref_name(RefKind::Branch, name) {
        bail!(why);
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
    if let Err(why) = check_ref_name(RefKind::Tag, name) {
        bail!(why);
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

// Telling git's refusals apart by their message. The wording is stable
// because every command runs with LC_ALL=C (see `cli`).

/// Part of git's message when `branch -d` refuses an unmerged branch.
pub const NOT_MERGED: &str = "not fully merged";

/// Git refused to switch branches because local changes would be lost.
pub fn blocked_by_changes(error: &str) -> bool {
    error.contains("would be overwritten by checkout")
        || error.contains("Please commit your changes or stash them")
}

/// Among the changes in the way ([`blocked_by_changes`]) are untracked files.
pub fn blocked_by_untracked(error: &str) -> bool {
    error.contains("untracked working tree files")
}

/// The files git lists between "…would be overwritten by checkout:" and
/// "Please …", joined for a sentence.
pub fn files_in_the_way(error: &str) -> String {
    let files: Vec<&str> = error
        .lines()
        .skip_while(|l| !l.contains("would be overwritten"))
        .skip(1)
        .map(str::trim)
        .take_while(|l| !l.starts_with("Please") && !l.starts_with("Aborting"))
        .filter(|l| !l.is_empty())
        .collect();
    match files.len() {
        0 => "some files".to_owned(),
        1..=5 => files.join(", "),
        n => format!("{} and {} more", files[..4].join(", "), n - 4),
    }
}

/// A push or pull failed because the repository has no remote to use.
pub fn no_remote(error: &str) -> bool {
    error.contains("No configured push destination")
        || error.contains("No remote repository specified")
}

/// A commit failed because git does not know the user's name and email.
pub fn no_identity(error: &str) -> bool {
    error.contains("Please tell me who you are") || error.contains("Author identity unknown")
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

    #[test]
    fn files_in_the_way_of_a_checkout() {
        let error = "Your local changes to the following files would be overwritten by checkout:\n\
                     \ta.txt\n\tb.txt\nPlease commit your changes or stash them before you switch branches.\nAborting";
        assert!(blocked_by_changes(error));
        assert!(!blocked_by_untracked(error));
        assert_eq!(files_in_the_way(error), "a.txt, b.txt");
    }
}
