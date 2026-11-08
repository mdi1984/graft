//! Finding, creating and cloning repositories, and finding a worktree's git
//! dir and what is in progress there.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

use crate::cli::{Git, Progress, failure_message};
use crate::console::Console;
use crate::jobs::CancelToken;
use crate::model::{InProgress, OPERATION_MARKERS, Operation};
use crate::paths::display;
use crate::refusal;

/// Where a repository lives on disk.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Location {
    /// The main worktree: the project's identity, shown on its tab.
    pub root: PathBuf,
    /// The worktree the user pointed at (may be a linked worktree of `root`).
    pub opened: PathBuf,
}

/// Find the repository containing `path`. Opening a linked worktree resolves
/// to its main repository, so each repository gets one project tab.
pub fn locate(path: &Path, console: &Console) -> Result<Location> {
    if !path.is_dir() {
        bail!("{} is not a folder", display(path));
    }
    let git = Git::new(path, console.clone());
    let out = git.run_raw([
        "rev-parse",
        "--path-format=absolute",
        "--show-toplevel",
        "--git-common-dir",
        "--is-bare-repository",
    ])?;
    if !out.success() {
        let msg = failure_message(&out);
        if refusal::not_a_repository(&msg) {
            bail!("{} is not a git repository", display(path));
        }
        bail!(msg);
    }
    let text = out.stdout_str();
    let mut lines = text.lines();
    let toplevel = lines.next().unwrap_or_default();
    let common_dir = lines.next().unwrap_or_default();
    if lines.next() == Some("true") || toplevel.is_empty() {
        bail!("Bare repositories are not supported yet");
    }
    let opened = git.native_path(toplevel);
    let common_dir = git.native_path(common_dir);
    // The main worktree is the folder that contains the common `.git` dir.
    let root = match common_dir.file_name() {
        Some(name) if name == ".git" => common_dir
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| opened.clone()),
        _ => opened.clone(),
    };
    Ok(Location { root, opened })
}

/// `git init` a new repository in `path` (created if missing).
pub fn init(path: &Path, console: &Console) -> Result<Location> {
    std::fs::create_dir_all(path).with_context(|| format!("Could not create {}", display(path)))?;
    Git::new(path, console.clone()).run(["init"])?;
    locate(path, console)
}

/// Folder name git would pick for a clone of `url`.
pub fn clone_folder_name(url: &str) -> String {
    let trimmed = url.trim().trim_end_matches(['/', '\\']);
    let last = trimmed.rsplit(['/', '\\', ':']).next().unwrap_or(trimmed);
    let name = last.strip_suffix(".git").unwrap_or(last);
    if name.is_empty() {
        "repository".to_owned()
    } else {
        name.to_owned()
    }
}

/// `git clone` `url` into `dest`, reporting progress. When the clone fails
/// or is cancelled, what it created goes again; an empty `dest` that was
/// already there stays.
pub fn clone(
    url: &str,
    dest: &Path,
    console: &Console,
    cancel: &CancelToken,
    on_progress: impl FnMut(Progress),
) -> Result<Location> {
    if dest.exists() && std::fs::read_dir(dest)?.next().is_some() {
        bail!(
            "The folder {} already exists and is not empty. Pick another folder name.",
            display(dest)
        );
    }
    let parent = dest
        .parent()
        .context("destination has no parent folder")?
        .to_path_buf();
    let created = first_missing(dest);
    std::fs::create_dir_all(&parent)
        .with_context(|| format!("Could not create {}", display(&parent)))?;
    let git = Git::new(&parent, console.clone());
    let result = git.path_arg(dest).and_then(|dest_arg| {
        git.run_progress(
            &["clone", "--progress", "--", url, &dest_arg],
            cancel,
            on_progress,
        )
    });
    if let Err(error) = result {
        match &created {
            Some(top) => {
                let _ = std::fs::remove_dir_all(top);
            }
            None => empty_folder(dest),
        }
        return Err(error);
    }
    locate(dest, console)
}

/// The outermost folder of `path` (or `path` itself) that does not exist yet.
fn first_missing(path: &Path) -> Option<PathBuf> {
    path.ancestors()
        .take_while(|p| !p.as_os_str().is_empty() && !p.exists())
        .last()
        .map(Path::to_path_buf)
}

/// Remove everything inside `dir`, keeping `dir`.
fn empty_folder(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let _ = if entry.file_type().is_ok_and(|t| t.is_dir()) {
            std::fs::remove_dir_all(&path)
        } else {
            std::fs::remove_file(&path)
        };
    }
}

/// The git dir of a worktree: `.git` itself, or what a linked worktree's
/// `.git` file points at.
pub(crate) fn git_dir(worktree: &Path) -> Option<PathBuf> {
    let dot = worktree.join(".git");
    if dot.is_dir() {
        return Some(dot);
    }
    let text = std::fs::read_to_string(&dot).ok()?;
    let target = text.trim().strip_prefix("gitdir:")?.trim();
    // In WSL git writes a Linux path.
    if let Some(wsl) = crate::wsl::Wsl::of(worktree)
        && target.starts_with('/')
    {
        return Some(wsl.to_windows(target));
    }
    let path = PathBuf::from(target);
    Some(if path.is_absolute() {
        path
    } else {
        worktree.join(path)
    })
}

/// What is in progress in this worktree, judged by git's marker files.
pub fn in_progress(worktree: &Path) -> Option<InProgress> {
    let dir = git_dir(worktree)?;
    let first_line = |name: &str| {
        std::fs::read_to_string(dir.join(name))
            .ok()
            .and_then(|t| t.lines().next().map(str::to_owned))
            .unwrap_or_default()
    };
    let (marker, kind) = OPERATION_MARKERS
        .into_iter()
        .find(|(marker, _)| dir.join(marker).exists())?;
    let or = |text: String, empty: &str| {
        if text.is_empty() {
            empty.to_owned()
        } else {
            text
        }
    };
    // "detached HEAD" when no branch is rebased.
    let branch = (kind == Operation::Rebase)
        .then(|| first_line(&format!("{marker}/head-name")))
        .and_then(|head| Some(head.strip_prefix("refs/heads/")?.to_owned()));
    let title = match kind {
        Operation::Rebase => match &branch {
            Some(branch) => format!("Rebasing {branch}"),
            None => "Rebasing".to_owned(),
        },
        Operation::Merge => or(first_line("MERGE_MSG"), "Merging"),
        Operation::CherryPick => format!("Cherry-picking: {}", first_line("MERGE_MSG")),
        Operation::Revert => or(first_line("MERGE_MSG"), "Reverting"),
    };
    Some(InProgress {
        kind,
        title,
        branch,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folder_names() {
        assert_eq!(
            clone_folder_name("https://github.com/mdi1984/graft.git"),
            "graft"
        );
        assert_eq!(
            clone_folder_name("git@github.com:rust-lang/rust.git"),
            "rust"
        );
        assert_eq!(clone_folder_name("https://example.com/a/b/"), "b");
        assert_eq!(clone_folder_name(r"C:\repos\thing"), "thing");
    }

    #[test]
    fn a_failed_clone_keeps_a_folder_that_was_there() {
        let tmp = tempfile::tempdir().unwrap();
        let console = Console::default();
        let missing = tmp.path().join("no-such-repo");
        let url = missing.to_str().unwrap();

        let empty = tmp.path().join("empty");
        std::fs::create_dir(&empty).unwrap();
        assert!(clone(url, &empty, &console, &CancelToken::default(), |_| {}).is_err());
        assert!(empty.is_dir(), "the user's folder stays");

        let new = tmp.path().join("new").join("deeper");
        assert!(clone(url, &new, &console, &CancelToken::default(), |_| {}).is_err());
        assert!(!tmp.path().join("new").exists(), "what the clone made goes");
    }
}
