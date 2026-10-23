//! Merges and conflicts: start a merge, detect an operation in progress
//! (merge, cherry-pick, revert, rebase), read a conflicted file's three sides
//! from the index, resolve it, and continue or abort.
//!
//! A resolution is written as a blob straight into the index and then checked
//! out, so git applies the worktree's line endings (core.autocrlf) itself.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

use crate::Git;
use crate::cli::failure_message;

/// An operation that stopped part-way, usually because of conflicts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Merge,
    CherryPick,
    Revert,
    Rebase,
}

impl Kind {
    fn command(self) -> &'static str {
        match self {
            Kind::Merge => "merge",
            Kind::CherryPick => "cherry-pick",
            Kind::Revert => "revert",
            Kind::Rebase => "rebase",
        }
    }

    pub fn noun(self) -> &'static str {
        match self {
            Kind::Merge => "Merge",
            Kind::CherryPick => "Cherry-pick",
            Kind::Revert => "Revert",
            Kind::Rebase => "Rebase",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InProgress {
    pub kind: Kind,
    /// What is being done, e.g. "Merge branch 'feature/x'".
    pub title: String,
}

/// The git dir of a worktree: `.git` itself, or what a linked worktree's
/// `.git` file points at.
fn git_dir(worktree: &Path) -> Option<PathBuf> {
    let dot = worktree.join(".git");
    if dot.is_dir() {
        return Some(dot);
    }
    let text = std::fs::read_to_string(&dot).ok()?;
    let target = text.trim().strip_prefix("gitdir:")?.trim();
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
    if dir.join("rebase-merge").is_dir() || dir.join("rebase-apply").is_dir() {
        let sub = if dir.join("rebase-merge").is_dir() {
            "rebase-merge"
        } else {
            "rebase-apply"
        };
        let head = first_line(&format!("{sub}/head-name"));
        let head = head.trim_start_matches("refs/heads/");
        return Some(InProgress {
            kind: Kind::Rebase,
            title: if head.is_empty() {
                "Rebasing".to_owned()
            } else {
                format!("Rebasing {head}")
            },
        });
    }
    if dir.join("MERGE_HEAD").is_file() {
        let msg = first_line("MERGE_MSG");
        return Some(InProgress {
            kind: Kind::Merge,
            title: if msg.is_empty() {
                "Merging".to_owned()
            } else {
                msg
            },
        });
    }
    if dir.join("CHERRY_PICK_HEAD").is_file() {
        let msg = first_line("MERGE_MSG");
        return Some(InProgress {
            kind: Kind::CherryPick,
            title: format!("Cherry-picking: {msg}"),
        });
    }
    if dir.join("REVERT_HEAD").is_file() {
        let msg = first_line("MERGE_MSG");
        return Some(InProgress {
            kind: Kind::Revert,
            title: if msg.is_empty() {
                "Reverting".to_owned()
            } else {
                msg
            },
        });
    }
    None
}

/// Run a command that may stop with conflicts. Ok(true) means it stopped and
/// the conflicts are waiting to be resolved.
pub fn run_may_conflict(git: &Git, args: &[&str]) -> Result<bool> {
    let out = git.run_expecting(args, None, |o| o.success() || stopped(o))?;
    if out.success() {
        return Ok(false);
    }
    if stopped(&out) {
        return Ok(true);
    }
    bail!(failure_message(&out))
}

/// Git stopped because of conflicts (rather than failing).
fn stopped(out: &crate::cli::Output) -> bool {
    let text = format!("{}\n{}", out.stdout_str(), out.stderr_str());
    text.contains("CONFLICT") || text.contains("could not apply")
}

/// Merge `rev` into the current branch. Ok(true): stopped with conflicts.
pub fn merge(git: &Git, rev: &str, no_ff: bool) -> Result<bool> {
    let mut args = vec!["merge", "--no-edit"];
    if no_ff {
        args.push("--no-ff");
    }
    args.push(rev);
    run_may_conflict(git, &args)
}

/// Rebase the current branch onto `onto`. Ok(true): stopped with conflicts.
pub fn rebase(git: &Git, onto: &str) -> Result<bool> {
    run_may_conflict(git, &["rebase", onto])
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
    let is_binary = |s: &Option<Side>| {
        s.as_ref()
            .is_some_and(|s| s.content.iter().take(8000).any(|b| *b == 0))
    };
    let binary = is_binary(&base) || is_binary(&ours) || is_binary(&theirs);
    let segments = match (&ours, &theirs) {
        (Some(o), Some(t)) if !binary => {
            let base_id = match &base {
                Some(b) => b.id.clone(),
                None => empty_blob(git)?,
            };
            // Exit code: the number of conflicts (negative on error).
            let out = git.run_expecting(
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
            if out.status < 0 || out.status > 127 {
                bail!("git merge-file failed: {}", out.stderr_str());
            }
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

fn empty_blob(git: &Git) -> Result<String> {
    let out = git.run_with_input(["hash-object", "-w", "--stdin"], Some(b""))?;
    Ok(out.stdout_str().trim().to_owned())
}

/// Split `git merge-file --diff3` output at its conflict markers.
pub fn parse_merged(text: &[u8]) -> Vec<Segment> {
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
                match resolution {
                    Resolution::Ours => out.extend_from_slice(ours),
                    Resolution::Theirs => out.extend_from_slice(theirs),
                    Resolution::OursThenTheirs => {
                        out.extend_from_slice(ours);
                        out.extend_from_slice(theirs);
                    }
                    Resolution::TheirsThenOurs => {
                        out.extend_from_slice(theirs);
                        out.extend_from_slice(ours);
                    }
                    Resolution::Custom(text) => out.extend_from_slice(text),
                }
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
    let out = git.run_with_input(
        ["hash-object", "-w", "--no-filters", "--stdin"],
        Some(content),
    )?;
    if !out.success() {
        bail!(failure_message(&out));
    }
    let id = out.stdout_str().trim().to_owned();
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
}
