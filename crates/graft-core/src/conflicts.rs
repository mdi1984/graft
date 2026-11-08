//! Merges and conflicts: start a merge, rebase, cherry-pick or revert, detect
//! one in progress, read a conflicted file's three sides from the index,
//! resolve it, and continue or abort.
//!
//! A resolution is written as a blob straight into the index and then checked
//! out, so git applies the worktree's line endings (core.autocrlf) itself.

use std::path::Path;

use anyhow::{Context, Result, bail};

use crate::Git;
use crate::changes::is_binary;
pub use crate::model::{InProgress, Kind};
use crate::repo::git_dir;

/// The files and folders in a git dir that mark an operation in progress,
/// checked in this order.
pub(crate) const MARKERS: [(&str, Kind); 5] = [
    ("rebase-merge", Kind::Rebase),
    ("rebase-apply", Kind::Rebase),
    ("MERGE_HEAD", Kind::Merge),
    ("CHERRY_PICK_HEAD", Kind::CherryPick),
    ("REVERT_HEAD", Kind::Revert),
];

/// What is in progress in this worktree, judged by git's marker files.
pub fn in_progress(worktree: &Path) -> Option<InProgress> {
    let dir = git_dir(worktree)?;
    let first_line = |name: &str| {
        std::fs::read_to_string(dir.join(name))
            .ok()
            .and_then(|t| t.lines().next().map(str::to_owned))
            .unwrap_or_default()
    };
    let (marker, kind) = MARKERS
        .into_iter()
        .find(|(marker, _)| dir.join(marker).exists())?;
    let or = |text: String, empty: &str| {
        if text.is_empty() {
            empty.to_owned()
        } else {
            text
        }
    };
    let title = match kind {
        Kind::Rebase => {
            let head = first_line(&format!("{marker}/head-name"));
            match head.trim_start_matches("refs/heads/") {
                "" => "Rebasing".to_owned(),
                head => format!("Rebasing {head}"),
            }
        }
        Kind::Merge => or(first_line("MERGE_MSG"), "Merging"),
        Kind::CherryPick => format!("Cherry-picking: {}", first_line("MERGE_MSG")),
        Kind::Revert => or(first_line("MERGE_MSG"), "Reverting"),
    };
    Some(InProgress { kind, title })
}

/// Names for the two sides of a conflict, ours first. In a rebase, "ours"
/// is the branch being rebased onto and "theirs" is your commit being
/// replayed.
pub fn side_names(kind: Option<Kind>, branch: Option<&str>) -> (String, String) {
    let current = format!("Current ({})", branch.unwrap_or("HEAD"));
    match kind {
        Some(Kind::Rebase) => ("Upstream".to_owned(), "Your commit".to_owned()),
        Some(Kind::CherryPick) => (current, "Picked commit".to_owned()),
        Some(Kind::Revert) => (current, "Reverted commit".to_owned()),
        Some(Kind::Merge) | None => (current, "Incoming".to_owned()),
    }
}

/// Run a command that may stop with conflicts. Ok(true) means it stopped and
/// the conflicts are waiting to be resolved.
fn run_may_conflict(git: &Git, args: &[&str]) -> Result<bool> {
    let out = git.run_accepting(args, None, |o| o.success() || stopped(o))?;
    Ok(!out.success())
}

/// Git stopped because of conflicts (rather than failing).
fn stopped(out: &crate::cli::Output) -> bool {
    let text = format!("{}\n{}", out.stdout_str(), out.stderr_str());
    text.contains("CONFLICT") || text.contains("could not apply")
}

/// Merge `rev` into the current branch. Ok(true): stopped with conflicts.
pub fn merge(git: &Git, rev: &str, no_ff: bool) -> Result<bool> {
    let mut args = vec![Kind::Merge.command(), "--no-edit"];
    if no_ff {
        args.push("--no-ff");
    }
    args.extend(["--end-of-options", rev]);
    run_may_conflict(git, &args)
}

/// Rebase the current branch onto `onto`. Ok(true): stopped with conflicts.
pub fn rebase(git: &Git, onto: &str) -> Result<bool> {
    run_may_conflict(git, &[Kind::Rebase.command(), "--end-of-options", onto])
}

/// Apply the commit `rev` on top of HEAD. Ok(true): stopped with conflicts.
pub fn cherry_pick(git: &Git, rev: &str) -> Result<bool> {
    run_may_conflict(git, &[Kind::CherryPick.command(), "--end-of-options", rev])
}

/// Commit the opposite of `rev`. Ok(true): stopped with conflicts.
pub fn revert(git: &Git, rev: &str) -> Result<bool> {
    run_may_conflict(
        git,
        &[Kind::Revert.command(), "--no-edit", "--end-of-options", rev],
    )
}

/// Continue after all conflicts are resolved, without opening an editor.
/// Ok(true): the next step (a rebase's next commit) stopped with conflicts.
pub fn continue_op(git: &Git, kind: Kind) -> Result<bool> {
    run_may_conflict(
        git,
        &["-c", "core.editor=true", kind.command(), "--continue"],
    )
}

pub fn abort(git: &Git, kind: Kind) -> Result<()> {
    git.run([kind.command(), "--abort"])?;
    Ok(())
}

/// One stage of a conflicted file in the index.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Side {
    pub mode: String,
    pub id: String,
    pub content: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Segment {
    /// Merged cleanly (from both sides).
    Common(Vec<u8>),
    Conflict {
        ours: Vec<u8>,
        base: Vec<u8>,
        theirs: Vec<u8>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConflictFile {
    pub path: String,
    pub base: Option<Side>,
    /// `None`: deleted on our side.
    pub ours: Option<Side>,
    /// `None`: deleted on their side.
    pub theirs: Option<Side>,
    pub binary: bool,
    /// The file split into clean parts and conflicts (text files where both
    /// sides exist).
    pub segments: Vec<Segment>,
}

impl ConflictFile {
    pub fn conflicts(&self) -> usize {
        self.segments
            .iter()
            .filter(|s| matches!(s, Segment::Conflict { .. }))
            .count()
    }
}

/// How one conflict is resolved.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Resolution {
    Ours,
    Theirs,
    OursThenTheirs,
    TheirsThenOurs,
    Custom(Vec<u8>),
}

impl Resolution {
    /// The text that replaces a conflict with these two sides.
    pub fn apply(&self, ours: &[u8], theirs: &[u8]) -> Vec<u8> {
        match self {
            Resolution::Ours => ours.to_vec(),
            Resolution::Theirs => theirs.to_vec(),
            Resolution::OursThenTheirs => [ours, theirs].concat(),
            Resolution::TheirsThenOurs => [theirs, ours].concat(),
            Resolution::Custom(text) => text.clone(),
        }
    }
}

pub fn load(git: &Git, path: &str) -> Result<ConflictFile> {
    let out = git.run(["ls-files", "-u", "-z", "--", path])?;
    let mut sides: [Option<Side>; 3] = [None, None, None];
    for record in out.stdout.split(|b| *b == 0).filter(|r| !r.is_empty()) {
        let record = String::from_utf8_lossy(record);
        let Some((meta, _)) = record.split_once('\t') else {
            continue;
        };
        let mut parts = meta.split(' ');
        let (Some(mode), Some(id), Some(stage)) = (parts.next(), parts.next(), parts.next()) else {
            continue;
        };
        let stage: usize = stage.parse().unwrap_or(0);
        if !(1..=3).contains(&stage) {
            continue;
        }
        let content = git.run(["cat-file", "blob", id])?.stdout;
        sides[stage - 1] = Some(Side {
            mode: mode.to_owned(),
            id: id.to_owned(),
            content,
        });
    }
    let [base, ours, theirs] = sides;
    if ours.is_none() && theirs.is_none() {
        bail!("{path} has no conflict in the index (already resolved?).");
    }
    let binary = [&base, &ours, &theirs]
        .into_iter()
        .flatten()
        .any(|s| is_binary(&s.content));
    let segments = match (&ours, &theirs) {
        (Some(o), Some(t)) if !binary => {
            let base_id = match &base {
                Some(b) => b.id.clone(),
                None => write_blob(git, b"", &[])?,
            };
            // Exit code: the number of conflicts (negative on error).
            let out = git.run_accepting(
                [
                    "merge-file",
                    "-p",
                    "--diff3",
                    "--object-id",
                    "-L",
                    "ours",
                    "-L",
                    "base",
                    "-L",
                    "theirs",
                    &o.id,
                    &base_id,
                    &t.id,
                ],
                None,
                |o| (0..=127).contains(&o.status),
            )?;
            parse_merged(&out.stdout)
        }
        _ => Vec::new(),
    };
    Ok(ConflictFile {
        path: path.to_owned(),
        base,
        ours,
        theirs,
        binary,
        segments,
    })
}

/// Store `content` as a blob; returns its id.
fn write_blob(git: &Git, content: &[u8], flags: &[&str]) -> Result<String> {
    let mut args = vec!["hash-object", "-w"];
    args.extend(flags);
    args.push("--stdin");
    let out = git.run_input(args, content)?;
    Ok(out.stdout_str().trim().to_owned())
}

/// Split `git merge-file --diff3` output at its conflict markers.
fn parse_merged(text: &[u8]) -> Vec<Segment> {
    enum State {
        Common,
        Ours,
        Base,
        Theirs,
    }
    let mut segments = Vec::new();
    let mut common = Vec::new();
    let (mut ours, mut base, mut theirs) = (Vec::new(), Vec::new(), Vec::new());
    let mut state = State::Common;
    for line in text.split_inclusive(|b| *b == b'\n') {
        let marker = |m: &[u8]| line.starts_with(m);
        match state {
            State::Common if marker(b"<<<<<<< ") => {
                if !common.is_empty() {
                    segments.push(Segment::Common(std::mem::take(&mut common)));
                }
                state = State::Ours;
            }
            State::Common => common.extend_from_slice(line),
            State::Ours if marker(b"||||||| ") => state = State::Base,
            State::Ours | State::Base if marker(b"=======") => state = State::Theirs,
            State::Ours => ours.extend_from_slice(line),
            State::Base => base.extend_from_slice(line),
            State::Theirs if marker(b">>>>>>> ") => {
                segments.push(Segment::Conflict {
                    ours: std::mem::take(&mut ours),
                    base: std::mem::take(&mut base),
                    theirs: std::mem::take(&mut theirs),
                });
                state = State::Common;
            }
            State::Theirs => theirs.extend_from_slice(line),
        }
    }
    if !common.is_empty() {
        segments.push(Segment::Common(common));
    }
    segments
}

/// The file's content with every conflict resolved. `None` while any
/// conflict has no resolution.
pub fn assemble(segments: &[Segment], resolutions: &[Option<Resolution>]) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    let mut i = 0;
    for segment in segments {
        match segment {
            Segment::Common(text) => out.extend_from_slice(text),
            Segment::Conflict { ours, theirs, .. } => {
                let resolution = resolutions.get(i)?.as_ref()?;
                i += 1;
                out.extend(resolution.apply(ours, theirs));
            }
        }
    }
    Some(out)
}

/// Resolve `path` with this content (in repository form, LF line endings):
/// stage it and write it to the worktree.
pub fn resolve_with(git: &Git, file: &ConflictFile, content: &[u8]) -> Result<()> {
    let mode = file
        .ours
        .as_ref()
        .or(file.theirs.as_ref())
        .map(|s| s.mode.clone())
        .unwrap_or_else(|| "100644".to_owned());
    let id = write_blob(git, content, &["--no-filters"])?;
    git.run([
        "update-index",
        "--cacheinfo",
        &format!("{mode},{id},{}", file.path),
    ])?;
    git.run(["checkout", "--", &file.path])
        .context("Could not write the resolved file")?;
    Ok(())
}

/// Take one side for the whole file (lock files, binaries). A side that
/// deleted the file deletes it.
pub fn take_side(git: &Git, file: &ConflictFile, ours: bool) -> Result<()> {
    let side = if ours { &file.ours } else { &file.theirs };
    if side.is_none() {
        git.run(["rm", "--quiet", "--", &file.path])?;
        return Ok(());
    }
    git.run([
        "checkout",
        if ours { "--ours" } else { "--theirs" },
        "--",
        &file.path,
    ])?;
    git.run(["add", "--", &file.path])?;
    Ok(())
}

/// The user fixed the file by hand: stage it as it is.
pub fn mark_resolved(git: &Git, path: &str) -> Result<()> {
    let full = git.cwd().join(path);
    if let Ok(bytes) = std::fs::read(&full)
        && bytes
            .split(|b| *b == b'\n')
            .any(|l| l.starts_with(b"<<<<<<< ") || l.starts_with(b">>>>>>> "))
    {
        bail!("{path} still contains conflict markers (<<<<<<< / >>>>>>>).");
    }
    git.run(["add", "--", path])?;
    Ok(())
}

/// Number of lines in a piece of a file (a last line without `\n` counts).
pub fn count_lines(text: &[u8]) -> usize {
    let n = text.iter().filter(|b| **b == b'\n').count();
    if text.last().is_some_and(|b| *b != b'\n') {
        n + 1
    } else {
        n
    }
}

/// The last `n` lines of `text`, for context above a conflict.
pub fn last_lines(text: &[u8], n: usize) -> String {
    let text = String::from_utf8_lossy(text);
    let lines: Vec<&str> = text.lines().collect();
    lines[lines.len().saturating_sub(n)..].join("\n")
}

/// The first `n` lines of `text`, for context below a conflict.
pub fn first_lines(text: &[u8], n: usize) -> String {
    let text = String::from_utf8_lossy(text);
    text.lines().take(n).collect::<Vec<_>>().join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_diff3_output() {
        let text = b"a\n<<<<<<< ours\nB1\n||||||| base\nb\n=======\nB2\n>>>>>>> theirs\nc\n";
        let segments = parse_merged(text);
        assert_eq!(
            segments,
            vec![
                Segment::Common(b"a\n".to_vec()),
                Segment::Conflict {
                    ours: b"B1\n".to_vec(),
                    base: b"b\n".to_vec(),
                    theirs: b"B2\n".to_vec()
                },
                Segment::Common(b"c\n".to_vec()),
            ]
        );
        assert_eq!(assemble(&segments, &[None]), None);
        assert_eq!(
            assemble(&segments, &[Some(Resolution::TheirsThenOurs)]).unwrap(),
            b"a\nB2\nB1\nc\n"
        );
        assert_eq!(
            assemble(&segments, &[Some(Resolution::Custom(b"X\n".to_vec()))]).unwrap(),
            b"a\nX\nc\n"
        );
    }

    #[test]
    fn context_lines() {
        assert_eq!(count_lines(b"a\nb"), 2);
        assert_eq!(count_lines(b"a\nb\n"), 2);
        assert_eq!(last_lines(b"1\n2\n3\n", 2), "2\n3");
        assert_eq!(first_lines(b"1\n2\n3\n", 2), "1\n2");
    }
}
