//! Changing the index and working tree: stage, unstage and discard whole
//! files or selected lines, and commit. Every change goes through git.exe so
//! hooks, filters and config apply.

use std::path::Path;

use anyhow::{Context, Result, bail};

use crate::cli::Git;
use crate::diff::{Direction, FileDiff, Kind, Source, build_patch};

/// Stage whole files (including deletions).
pub fn stage_paths(git: &Git, paths: &[String]) -> Result<()> {
    if paths.is_empty() {
        return Ok(());
    }
    let mut args = vec!["add", "-A", "--"];
    args.extend(paths.iter().map(String::as_str));
    git.run(args)?;
    Ok(())
}

/// Move staged changes of whole files back to the working tree.
pub fn unstage_paths(git: &Git, paths: &[String]) -> Result<()> {
    if paths.is_empty() {
        return Ok(());
    }
    let mut args = if has_head(git)? {
        vec!["restore", "--staged", "--"]
    } else {
        // Before the first commit there is nothing to restore from.
        vec!["rm", "--cached", "-r", "-q", "--"]
    };
    args.extend(paths.iter().map(String::as_str));
    git.run(args)?;
    Ok(())
}

pub fn has_head(git: &Git) -> Result<bool> {
    Ok(git
        .run_raw(["rev-parse", "-q", "--verify", "HEAD"])?
        .success())
}

/// Throw away unstaged changes. Tracked files go back to their staged (or
/// committed) state; untracked files go to the Recycle Bin, not oblivion.
pub fn discard_paths(git: &Git, tracked: &[String], untracked: &[String]) -> Result<()> {
    if !tracked.is_empty() {
        let mut args = vec!["restore", "--worktree", "--"];
        args.extend(tracked.iter().map(String::as_str));
        git.run(args)?;
    }
    if !untracked.is_empty() {
        let files: Vec<_> = untracked.iter().map(|p| git.cwd().join(p)).collect();
        trash::delete_all(&files).context("move untracked files to the Recycle Bin")?;
    }
    Ok(())
}

/// What to do with the selected lines of a diff.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LineOp {
    Stage,
    Unstage,
    Discard,
}

/// Stage, unstage or discard the selected changed lines of `diff`.
/// `selected(hunk, line)` picks lines; a whole hunk is all its lines.
pub fn apply_lines(
    git: &Git,
    diff: &FileDiff,
    source: &Source,
    op: LineOp,
    selected: impl Fn(usize, usize) -> bool,
) -> Result<()> {
    let total = diff.changed_lines();
    let chosen = diff
        .hunks
        .iter()
        .enumerate()
        .flat_map(|(h, hunk)| {
            hunk.lines
                .iter()
                .enumerate()
                .filter(move |(_, l)| l.is_change())
                .map(move |(l, _)| (h, l))
        })
        .filter(|&(h, l)| selected(h, l))
        .count();
    if chosen == 0 {
        return Ok(());
    }
    let everything = chosen == total;
    match (source, op) {
        (Source::Unstaged, LineOp::Stage) => {
            let patch = build_patch(diff, Direction::Forward, &selected).expect("lines chosen");
            apply_patch(git, &patch, true, false)
        }
        (Source::Staged, LineOp::Unstage) => {
            let patch = build_patch(diff, Direction::Reverse, &selected).expect("lines chosen");
            apply_patch(git, &patch, true, true)
        }
        (Source::Unstaged, LineOp::Discard) => {
            let patch = build_patch(diff, Direction::Reverse, &selected).expect("lines chosen");
            apply_patch(git, &patch, false, true)
        }
        (Source::Untracked, LineOp::Stage) if everything => {
            stage_paths(git, std::slice::from_ref(&diff.path))
        }
        (Source::Untracked, LineOp::Stage) => {
            let mut diff = diff.clone();
            if strips_cr(git)? {
                for line in diff.hunks.iter_mut().flat_map(|h| &mut h.lines) {
                    if line.content.ends_with(b"\r") {
                        line.content.pop();
                    }
                }
            }
            diff.header = new_file_header(&diff.path);
            let patch = build_patch(&diff, Direction::Forward, &selected).expect("lines chosen");
            apply_patch(git, &patch, true, false)
        }
        (Source::Untracked, LineOp::Discard) if everything => {
            discard_paths(git, &[], std::slice::from_ref(&diff.path))
        }
        (Source::Untracked, LineOp::Discard) => {
            remove_lines(&git.cwd().join(&diff.path), diff, &selected)
        }
        _ => bail!("That change cannot be applied here"),
    }
}

fn new_file_header(path: &str) -> Vec<u8> {
    format!("diff --git a/{path} b/{path}\nnew file mode 100644\n--- /dev/null\n+++ b/{path}\n")
        .into_bytes()
}

/// Whether git turns CRLF into LF when adding files (core.autocrlf).
fn strips_cr(git: &Git) -> Result<bool> {
    let out = git.run_raw(["config", "--get", "core.autocrlf"])?;
    let value = out.stdout_str().trim().to_lowercase();
    Ok(value == "true" || value == "input")
}

/// Rewrite an untracked file without the selected lines.
fn remove_lines(
    path: &Path,
    diff: &FileDiff,
    selected: impl Fn(usize, usize) -> bool,
) -> Result<()> {
    if std::fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_symlink()) {
        bail!(
            "{} is a link; Graft only edits plain files.",
            path.display()
        );
    }
    let mut out = Vec::new();
    for (h, hunk) in diff.hunks.iter().enumerate() {
        for (l, line) in hunk.lines.iter().enumerate() {
            if line.kind == Kind::Added && selected(h, l) {
                continue;
            }
            out.extend_from_slice(&line.content);
            if !line.no_newline {
                out.push(b'\n');
            }
        }
    }
    std::fs::write(path, out).with_context(|| format!("write {}", path.display()))
}

/// `git apply` a patch to the index (`cached`) or the working tree.
pub fn apply_patch(git: &Git, patch: &[u8], cached: bool, reverse: bool) -> Result<()> {
    let mut args = vec!["apply", "--recount", "--whitespace=nowarn"];
    if cached {
        args.push("--cached");
    }
    if reverse {
        args.push("--reverse");
    }
    args.push("-");
    let out = git.run_with_input(args, Some(patch))?;
    if !out.success() {
        bail!(
            "git could not apply the change: {}",
            crate::cli::failure_message(&out)
        );
    }
    Ok(())
}

/// The message of HEAD, for amending. `None` before the first commit.
pub fn last_commit_message(git: &Git) -> Result<Option<String>> {
    if !has_head(git)? {
        return Ok(None);
    }
    let text = git.read(["log", "-1", "--format=%B"])?;
    Ok(Some(text.trim_end().to_owned()))
}

/// Commit what is staged. Hooks run as usual. Returns git's one-line summary.
pub fn commit(git: &Git, message: &str, amend: bool) -> Result<String> {
    let mut args = vec!["commit", "--file=-", "--cleanup=strip"];
    if amend {
        args.push("--amend");
    }
    let out = git.run_with_input(args, Some(message.as_bytes()))?;
    if !out.success() {
        bail!(crate::cli::failure_message(&out));
    }
    let summary = out
        .stdout_str()
        .lines()
        .next()
        .unwrap_or_default()
        .trim()
        .to_owned();
    Ok(summary)
}
