//! Stashes: list (with the branch each came from), create, apply, pop, drop,
//! branch from a stash, and the files a stash contains.

use anyhow::{Result, bail};

use crate::details::{self, FileChange};
use crate::status::Change;
use crate::{Git, Oid};

#[derive(Clone, Debug, PartialEq)]
pub struct Stash {
    /// Position in `git stash list` (stash@{index}).
    pub index: usize,
    pub id: Oid,
    pub message: String,
    /// Branch it was made on; `None` for a detached HEAD.
    pub branch: Option<String>,
    pub time: i64,
}

#[derive(Clone, Debug, Default)]
pub struct Options {
    pub message: String,
    /// Also stash untracked files (`-u`).
    pub include_untracked: bool,
    /// Leave staged changes in place (`--keep-index`).
    pub keep_index: bool,
    /// Stash only what is staged (`--staged`).
    pub staged_only: bool,
}

/// A file in a stash, and the commit whose diff shows it: the stash itself
/// for tracked changes, its third parent for untracked files.
#[derive(Clone, Debug, PartialEq)]
pub struct StashFile {
    pub change: FileChange,
    pub commit: Oid,
}

pub fn list(git: &Git) -> Result<Vec<Stash>> {
    let out = git.read(["stash", "list", "--format=%H%x1f%gs%x1f%ct"])?;
    Ok(out
        .lines()
        .enumerate()
        .filter_map(|(index, line)| {
            let mut parts = line.split('\x1f');
            let id = Oid::from_hex(parts.next()?.as_bytes()).ok()?;
            let (branch, message) = parse_subject(parts.next().unwrap_or(""));
            let time = parts
                .next()
                .and_then(|t| t.trim().parse().ok())
                .unwrap_or(0);
            Some(Stash {
                index,
                id,
                message,
                branch,
                time,
            })
        })
        .collect())
}

/// "On main: my message" or "WIP on main: 1a2b3c4 last commit" → branch, message.
fn parse_subject(subject: &str) -> (Option<String>, String) {
    let rest = subject
        .strip_prefix("WIP on ")
        .or_else(|| subject.strip_prefix("On "));
    let Some((branch, message)) = rest.and_then(|r| r.split_once(": ")) else {
        return (None, subject.to_owned());
    };
    let branch = (branch != "(no branch)").then(|| branch.to_owned());
    let message = if subject.starts_with("WIP") {
        // "1a2b3c4 last commit": no message was given.
        let summary = message.split_once(' ').map_or(message, |(_, s)| s);
        format!("WIP: {summary}")
    } else {
        message.to_owned()
    };
    (branch, message)
}

pub fn push(git: &Git, options: &Options) -> Result<()> {
    let mut args = vec!["stash", "push"];
    if options.include_untracked {
        args.push("--include-untracked");
    }
    if options.keep_index {
        args.push("--keep-index");
    }
    if options.staged_only {
        args.push("--staged");
    }
    let message = options.message.trim();
    if !message.is_empty() {
        args.extend(["--message", message]);
    }
    let out = git.run(args)?;
    if out.stdout_str().contains("No local changes to save") {
        bail!("There are no changes to stash.");
    }
    Ok(())
}

/// The `stash@{n}` name of the stash with this id. Positions shift whenever a
/// stash is added or dropped, so they are looked up right before use.
fn reference(git: &Git, id: Oid) -> Result<String> {
    match list(git)?.into_iter().find(|s| s.id == id) {
        Some(s) => Ok(format!("stash@{{{}}}", s.index)),
        None => bail!("That stash no longer exists."),
    }
}

/// Apply a stash to the worktree `git` points at; `pop` also drops it.
pub fn apply(git: &Git, id: Oid, pop: bool) -> Result<()> {
    let name = reference(git, id)?;
    git.run(["stash", if pop { "pop" } else { "apply" }, &name])?;
    Ok(())
}

pub fn drop(git: &Git, id: Oid) -> Result<()> {
    let name = reference(git, id)?;
    git.run(["stash", "drop", &name])?;
    Ok(())
}

/// Create `branch` at the commit the stash was made on, check it out, apply
/// the stash there and drop it.
pub fn branch(git: &Git, branch: &str, id: Oid) -> Result<()> {
    crate::refs::check_branch_name(git, branch)?;
    let name = reference(git, id)?;
    git.run(["stash", "branch", branch, &name])?;
    Ok(())
}

pub fn files(git: &Git, id: Oid) -> Result<Vec<StashFile>> {
    let mut files: Vec<StashFile> = details::load(git, id)?
        .files
        .into_iter()
        .map(|change| StashFile { change, commit: id })
        .collect();
    // Untracked files stashed with -u live in a third parent.
    let third = git.run_raw(["rev-parse", "--verify", "--quiet", &format!("{id}^3")])?;
    if third.success()
        && let Ok(untracked) = Oid::from_hex(third.stdout_str().trim().as_bytes())
    {
        let names = git.read(["ls-tree", "-r", "--name-only", "-z", &untracked.to_string()])?;
        for path in names.split('\0').filter(|p| !p.is_empty()) {
            files.push(StashFile {
                change: FileChange {
                    path: path.to_owned(),
                    orig_path: None,
                    change: Change::Added,
                    added: None,
                    removed: None,
                },
                commit: untracked,
            });
        }
    }
    Ok(files)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subjects() {
        assert_eq!(
            parse_subject("On main: fix later"),
            (Some("main".to_owned()), "fix later".to_owned())
        );
        assert_eq!(
            parse_subject("WIP on feature/x: 1a2b3c4 Add login"),
            (Some("feature/x".to_owned()), "WIP: Add login".to_owned())
        );
        assert_eq!(
            parse_subject("WIP on (no branch): 1a2b3c4 Detached work"),
            (None, "WIP: Detached work".to_owned())
        );
        assert_eq!(parse_subject("odd"), (None, "odd".to_owned()));
    }
}
