//! Changing the index and working tree: stage, unstage and discard whole
//! files or selected lines, and commit. Every change goes through git.exe so
//! hooks, filters and config apply.

use anyhow::{Context, Result, bail};

use crate::cli::Git;
use crate::diff::{Chosen, Direction, FileDiff, Kind, Source, build_patch, chosen};
use crate::paths::display;

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

fn has_head(git: &Git) -> Result<bool> {
    Ok(git.rev("HEAD")?.is_some())
}

/// Throw away unstaged changes. Tracked files go back to their staged (or
/// committed) state; untracked files go to the Recycle Bin, not oblivion,
/// and leave the index if they were added with intent (`add -N`).
pub fn discard_paths(git: &Git, tracked: &[String], untracked: &[String]) -> Result<()> {
    if !tracked.is_empty() {
        let mut args = vec!["restore", "--worktree", "--"];
        args.extend(tracked.iter().map(String::as_str));
        git.run(args)?;
    }
    if !untracked.is_empty() {
        let files: Vec<_> = untracked.iter().map(|p| git.cwd().join(p)).collect();
        trash::delete_all(&files).context("move untracked files to the Recycle Bin")?;
        let mut args = vec!["rm", "--cached", "--ignore-unmatch", "-q", "--"];
        args.extend(untracked.iter().map(String::as_str));
        git.run(args)?;
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
    let chosen = chosen(diff, &selected);
    if chosen == Chosen::Nothing {
        return Ok(());
    }
    let everything = chosen == Chosen::All;
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
        (Source::Untracked, LineOp::Stage) => stage_new_file_lines(
            git,
            &diff.path,
            &kept_lines(diff, Kept::Selected, &selected),
        ),
        (Source::Untracked, LineOp::Discard) if everything => {
            discard_paths(git, &[], std::slice::from_ref(&diff.path))
        }
        (Source::Untracked, LineOp::Discard) => remove_lines(git, diff, &selected),
        _ => bail!("That change cannot be applied here"),
    }
}

/// Which lines of a new file's diff to keep.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Kept {
    Selected,
    Unselected,
}

/// The content of a new file's diff with only the selected (or only the
/// unselected) added lines. Like the diff, it is in git's form (line endings,
/// encoding and filters as stored), not as on disk.
fn kept_lines(diff: &FileDiff, keep: Kept, selected: impl Fn(usize, usize) -> bool) -> Vec<u8> {
    let mut out = Vec::new();
    for (h, hunk) in diff.hunks.iter().enumerate() {
        for (l, line) in hunk.lines.iter().enumerate() {
            let picked = line.kind == Kind::Added && selected(h, l);
            if picked != (keep == Kept::Selected) {
                continue;
            }
            out.extend_from_slice(&line.content);
            if !line.no_newline {
                out.push(b'\n');
            }
        }
    }
    out
}

/// Store `content` (git's form) as a blob as it is, and return its id.
fn write_blob(git: &Git, content: &[u8]) -> Result<String> {
    let out = git.run_input(["hash-object", "-w", "--stdin", "--no-filters"], content)?;
    Ok(out.stdout_str().trim().to_owned())
}

/// Stage an untracked file with only `content` (git's form), with the mode
/// git sees on the file (core.fileMode). A failure leaves the index as it
/// was.
fn stage_new_file_lines(git: &Git, path: &str, content: &[u8]) -> Result<()> {
    let id = write_blob(git, content)?;
    // An intent-to-add entry carries the mode git sees on the file.
    git.run(["add", "--intent-to-add", "--", path])?;
    let staged = replace_entry_content(git, path, &id);
    if staged.is_err() {
        let _ = git.run(["update-index", "--force-remove", "--", path]);
    }
    staged.context("git could not stage the lines")
}

/// Point the index entry of `path` at blob `id`, keeping its mode.
fn replace_entry_content(git: &Git, path: &str, id: &str) -> Result<()> {
    let entry = git.read(["ls-files", "--stage", "--", path])?;
    let mode = entry
        .split_whitespace()
        .next()
        .with_context(|| format!("git did not list {path} in the index"))?
        .to_owned();
    git.run(["update-index", "--add", "--cacheinfo", &mode, id, path])?;
    Ok(())
}

/// Rewrite an untracked file without the selected lines, converted back to
/// its form on disk as checkout would (line endings, encoding, filters).
fn remove_lines(git: &Git, diff: &FileDiff, selected: impl Fn(usize, usize) -> bool) -> Result<()> {
    let path = git.cwd().join(&diff.path);
    if std::fs::symlink_metadata(&path).is_ok_and(|m| m.file_type().is_symlink()) {
        bail!(
            "{} is a link; Graft only edits plain files.",
            display(&path)
        );
    }
    let id = write_blob(git, &kept_lines(diff, Kept::Unselected, selected))?;
    let on_disk = git.run([
        "cat-file",
        "--filters",
        &format!("--path={}", diff.path),
        &id,
    ])?;
    std::fs::write(&path, on_disk.stdout)
        .with_context(|| format!("Could not write {}", display(&path)))
}

/// `git apply` a patch to the index (`cached`) or the working tree.
fn apply_patch(git: &Git, patch: &[u8], cached: bool, reverse: bool) -> Result<()> {
    let mut args = vec!["apply", "--recount", "--whitespace=nowarn"];
    if cached {
        args.push("--cached");
    }
    if reverse {
        args.push("--reverse");
    }
    args.push("-");
    git.run_input(args, patch)
        .context("git could not apply the change")?;
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
    let out = git.run_input(args, message.as_bytes())?;
    let summary = out
        .stdout_str()
        .lines()
        .next()
        .unwrap_or_default()
        .trim()
        .to_owned();
    Ok(summary)
}
