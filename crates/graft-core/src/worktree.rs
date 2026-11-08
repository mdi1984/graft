//! Adding, removing, locking and pruning worktrees, and copying the ignored
//! files a new worktree needs to run (`.env` and friends).

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

use crate::cli::Git;
use crate::paths::{display, glob_match, pattern_list, through_link};

/// Files worth copying into a new worktree by default: ignored, so git will
/// not bring them, but needed to build or run.
pub const DEFAULT_COPY_PATTERNS: &str = ".env, .env.*, *.local, *.local.json";

/// What the new worktree checks out.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Start {
    /// Create `name` at `base` (a branch, tag or commit).
    NewBranch { name: String, base: String },
    /// Check out an existing local branch.
    Existing(String),
    /// Create a local branch `name` tracking `remote` (e.g. `origin/feature`).
    Tracking { name: String, remote: String },
}

pub fn add(git: &Git, path: &Path, start: &Start) -> Result<()> {
    if path.exists()
        && path
            .read_dir()
            .map(|mut d| d.next().is_some())
            .unwrap_or(true)
    {
        bail!(
            "The folder {} already exists and is not empty. Pick another folder.",
            display(path)
        );
    }
    let path = git.path_arg(path)?;
    match start {
        Start::NewBranch { name, base } => {
            git.run(["worktree", "add", "-b", name, &path, base])?;
        }
        Start::Existing(name) => {
            git.run(["worktree", "add", &path, name])?;
        }
        Start::Tracking { name, remote } => {
            git.run(["worktree", "add", "--track", "-b", name, &path, remote])?;
        }
    }
    Ok(())
}

/// Remove a linked worktree; `force` also throws away its uncommitted changes.
pub fn remove(git: &Git, path: &Path, force: bool) -> Result<()> {
    let path = git.path_arg(path)?;
    let mut args = vec!["worktree", "remove"];
    if force {
        args.push("--force");
    }
    args.push(&path);
    git.run(args)?;
    Ok(())
}

pub fn set_locked(git: &Git, path: &Path, locked: bool) -> Result<()> {
    let path = git.path_arg(path)?;
    if locked {
        git.run(["worktree", "lock", "--reason", "Locked in Graft", &path])?;
    } else {
        git.run(["worktree", "unlock", &path])?;
    }
    Ok(())
}

/// Forget worktrees whose folders are gone.
pub fn prune(git: &Git) -> Result<()> {
    git.run(["worktree", "prune"])?;
    Ok(())
}

/// Where a new worktree for `branch` goes by default: a sibling folder
/// `<repo>.worktrees\<branch>`, so worktrees never nest inside the repo.
pub fn default_path(main: &Path, branch: &str) -> PathBuf {
    let repo = main
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "repo".to_owned());
    let parent = main.parent().unwrap_or(main);
    parent
        .join(format!("{repo}.worktrees"))
        .join(folder_for(branch))
}

/// The folder name for a worktree of `branch`: the name with characters a
/// folder name cannot have (and spaces) as `-`.
pub fn folder_for(branch: &str) -> String {
    let folder: String = branch
        .chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' | ' ' => '-',
            c => c,
        })
        .collect();
    folder.trim_matches('-').to_owned()
}

/// Copy ignored files of `from` whose names match one of `patterns` (see
/// [`pattern_list`]) into the same place in `to`. Existing files are kept.
/// Returns the copied paths.
pub fn copy_ignored(from: &Git, to: &Path, patterns: &str) -> Result<Vec<String>> {
    let patterns: Vec<&str> = pattern_list(patterns).collect();
    if patterns.is_empty() {
        return Ok(Vec::new());
    }
    // --directory lists an ignored folder (target/, node_modules/) as one
    // entry instead of every file in it.
    let out = from.run([
        "ls-files",
        "--others",
        "--ignored",
        "--exclude-standard",
        "--directory",
        "-z",
    ])?;
    let mut copied = Vec::new();
    for entry in out.stdout.split(|b| *b == 0) {
        let rel = String::from_utf8_lossy(entry);
        if rel.is_empty() || rel.ends_with('/') {
            continue;
        }
        let name = rel.rsplit('/').next().unwrap_or(&rel);
        if !patterns.iter().any(|p| glob_match(p, name)) {
            continue;
        }
        let src = from.cwd().join(rel.as_ref());
        let dst = to.join(rel.as_ref());
        // A link in the new worktree (a checked-in symlink folder) must not
        // send the copy somewhere outside it.
        if dst.exists() || !src.is_file() || through_link(to, Path::new(rel.as_ref())) {
            continue;
        }
        if let Some(parent) = dst.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("Could not create {}", display(parent)))?;
        }
        std::fs::copy(&src, &dst).with_context(|| format!("Could not copy {rel}"))?;
        copied.push(rel.into_owned());
    }
    Ok(copied)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_paths() {
        assert_eq!(folder_for("/feature/x:y/"), "feature-x-y");
        let p = default_path(Path::new(r"C:\dev\graft"), "feature/login page");
        assert_eq!(
            p,
            PathBuf::from(r"C:\dev\graft.worktrees\feature-login-page")
        );
    }
}
