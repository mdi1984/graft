//! Finding, creating and cloning repositories.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

use crate::cli::{CancelToken, Git, failure_message};
use crate::console::Console;

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
        bail!("{} is not a folder", path.display());
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
        if msg.contains("not a git repository") {
            bail!("{} is not a git repository", path.display());
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
    let opened = native(toplevel);
    let common_dir = native(common_dir);
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
    std::fs::create_dir_all(path).with_context(|| format!("create {}", path.display()))?;
    Git::new(path, console.clone()).run(["init"])?;
    locate(path, console)
}

/// Progress of a clone (or fetch), parsed from git's stderr.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Progress {
    /// E.g. "Receiving objects".
    pub phase: String,
    /// 0.0..=1.0 if git reported a percentage.
    pub fraction: Option<f32>,
    /// The raw line, for display.
    pub line: String,
}

pub fn parse_progress(line: &str) -> Progress {
    let line = line.trim().trim_start_matches("remote:").trim();
    let (phase, rest) = line.split_once(':').unwrap_or((line, ""));
    let fraction = rest
        .split_whitespace()
        .find_map(|w| w.strip_suffix('%'))
        .and_then(|p| p.parse::<f32>().ok())
        .map(|p| (p / 100.0).clamp(0.0, 1.0));
    Progress {
        phase: phase.trim().to_owned(),
        fraction,
        line: line.to_owned(),
    }
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

/// `git clone` `url` into `dest`, reporting progress. Cancelling deletes the
/// partial clone.
pub fn clone(
    url: &str,
    dest: &Path,
    console: &Console,
    cancel: &CancelToken,
    mut on_progress: impl FnMut(Progress),
) -> Result<Location> {
    if dest.exists() && std::fs::read_dir(dest)?.next().is_some() {
        bail!(
            "The folder {} already exists and is not empty. Pick another folder name.",
            dest.display()
        );
    }
    let parent = dest
        .parent()
        .context("destination has no parent folder")?
        .to_path_buf();
    std::fs::create_dir_all(&parent).with_context(|| format!("create {}", parent.display()))?;
    let git = Git::new(&parent, console.clone());
    let result = git.run_with_progress(
        [
            "clone".as_ref(),
            "--progress".as_ref(),
            "--".as_ref(),
            url.as_ref(),
            dest.as_os_str(),
        ],
        cancel.flag(),
        |line| on_progress(parse_progress(line)),
    );
    let out = match result {
        Ok(out) => out,
        Err(err) => {
            let _ = std::fs::remove_dir_all(dest);
            return Err(err);
        }
    };
    if !out.success() {
        let _ = std::fs::remove_dir_all(dest);
        bail!(failure_message(&out));
    }
    locate(dest, console)
}

/// Paths from git use forward slashes; show them the Windows way.
pub fn native(path: &str) -> PathBuf {
    if cfg!(windows) {
        PathBuf::from(path.replace('/', "\\"))
    } else {
        PathBuf::from(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn progress_lines() {
        let p = parse_progress("Receiving objects:  45% (450/1000), 1.20 MiB | 2.00 MiB/s");
        assert_eq!(p.phase, "Receiving objects");
        assert_eq!(p.fraction, Some(0.45));
        let p = parse_progress("remote: Counting objects: 100% (12/12), done.");
        assert_eq!(p.phase, "Counting objects");
        assert_eq!(p.fraction, Some(1.0));
        let p = parse_progress("Cloning into 'graft'...");
        assert_eq!(p.fraction, None);
    }

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
}
