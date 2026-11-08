//! Merges and conflicts: start a merge, rebase, cherry-pick or revert, read
//! a conflicted file's three sides from the index, resolve it, and continue
//! or abort. What is in progress is `repo::in_progress`.
//!
//! A resolution is written as a blob straight into the index and then checked
//! out, so git applies the worktree's line endings (core.autocrlf) itself.

use anyhow::{Context, Result, bail};

use crate::changes::is_binary;
use crate::cli::{Git, Output};
use crate::model::Operation;
use crate::refusal::stopped_with_conflicts;

/// What one side of a conflict is called.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SideName {
    /// As a heading, e.g. "Current (main)".
    pub long: String,
    /// Inside a sentence, e.g. "current".
    pub short: String,
}

impl SideName {
    fn new(name: &str, detail: Option<&str>) -> Self {
        Self {
            long: match detail {
                Some(detail) => format!("{name} ({detail})"),
                None => name.to_owned(),
            },
            short: name.to_lowercase(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SideNames {
    pub ours: SideName,
    pub theirs: SideName,
}

/// Names for the two sides of a conflict. In a rebase, "ours" is the branch
/// being rebased onto and "theirs" is your commit being replayed.
pub fn side_names(kind: Option<Operation>, branch: Option<&str>) -> SideNames {
    let current = SideName::new("Current", Some(branch.unwrap_or("HEAD")));
    let (ours, theirs) = match kind {
        Some(Operation::Rebase) => (SideName::new("Upstream", None), "Your commit"),
        Some(Operation::CherryPick) => (current, "Picked commit"),
        Some(Operation::Revert) => (current, "Reverted commit"),
        Some(Operation::Merge) | None => (current, "Incoming"),
    };
    SideNames {
        ours,
        theirs: SideName::new(theirs, None),
    }
}

/// Run a command that may stop with conflicts. Ok(true) means it stopped and
/// the conflicts are waiting to be resolved.
fn run_may_conflict(git: &Git, args: &[&str]) -> Result<bool> {
    let out = git.run_accepting(args, None, |o| o.success() || stopped(o))?;
    Ok(!out.success())
}

/// Git stopped because of conflicts (rather than failing).
pub(crate) fn stopped(out: &Output) -> bool {
    stopped_with_conflicts(&format!("{}\n{}", out.stdout_str(), out.stderr_str()))
}

/// Merge `rev` into the current branch. Ok(true): stopped with conflicts.
pub fn merge(git: &Git, rev: &str, no_ff: bool) -> Result<bool> {
    let mut args = vec![Operation::Merge.command(), "--no-edit"];
    if no_ff {
        args.push("--no-ff");
    }
    args.extend(["--end-of-options", rev]);
    run_may_conflict(git, &args)
}

/// Rebase the current branch onto `onto`. Ok(true): stopped with conflicts.
pub fn rebase(git: &Git, onto: &str) -> Result<bool> {
    run_may_conflict(
        git,
        &[Operation::Rebase.command(), "--end-of-options", onto],
    )
}

/// Apply the commit `rev` on top of HEAD. Ok(true): stopped with conflicts.
pub fn cherry_pick(git: &Git, rev: &str) -> Result<bool> {
    run_may_conflict(
        git,
        &[Operation::CherryPick.command(), "--end-of-options", rev],
    )
}

/// Commit the opposite of `rev`. Ok(true): stopped with conflicts.
pub fn revert(git: &Git, rev: &str) -> Result<bool> {
    run_may_conflict(
        git,
        &[
            Operation::Revert.command(),
            "--no-edit",
            "--end-of-options",
            rev,
        ],
    )
}

/// Continue after all conflicts are resolved, keeping the prepared message.
/// Ok(true): the next step (a rebase's next commit) stopped with conflicts.
pub fn continue_op(git: &Git, kind: Operation) -> Result<bool> {
    run_may_conflict(git, &[kind.command(), "--continue"])
}

pub fn abort(git: &Git, kind: Operation) -> Result<()> {
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
    /// A hash of the working file when this was loaded (`None`: missing),
    /// see [`edited_since_load`].
    pub on_disk: Option<u64>,
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
    let on_disk = disk_hash(git, path);
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
                    &format!("--marker-size={MARKER_SIZE}"),
                    "-L",
                    LABELS[0],
                    "-L",
                    LABELS[1],
                    "-L",
                    LABELS[2],
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
        on_disk,
    })
}

/// A hash of the working file's content; `None` when it is missing.
fn disk_hash(git: &Git, path: &str) -> Option<u64> {
    use std::hash::{DefaultHasher, Hash, Hasher};
    let bytes = std::fs::read(git.cwd().join(path)).ok()?;
    let mut hasher = DefaultHasher::new();
    bytes.hash(&mut hasher);
    Some(hasher.finish())
}

/// Whether the working file changed since `file` was loaded (edited by
/// hand, e.g. in another editor): saving a resolution would replace that.
pub fn edited_since_load(git: &Git, file: &ConflictFile) -> bool {
    disk_hash(git, &file.path) != file.on_disk
}

/// Store `content` as a blob; returns its id.
fn write_blob(git: &Git, content: &[u8], flags: &[&str]) -> Result<String> {
    let mut args = vec!["hash-object", "-w"];
    args.extend(flags);
    args.push("--stdin");
    let out = git.run_input(args, content)?;
    Ok(out.stdout_str().trim().to_owned())
}

/// The `-L` labels for ours, base and theirs.
const LABELS: [&str; 3] = ["ours", "base", "theirs"];

/// How long the conflict markers are: far longer than git's usual 7, so a
/// content line (a Markdown `=======` underline) is never taken for one.
const MARKER_SIZE: usize = 41;

/// The marker lines of `git merge-file --marker-size` [`MARKER_SIZE`] with
/// [`LABELS`]: ours starts, base starts, theirs starts, the conflict ends.
/// A line is a marker only if it is exactly one of these.
static MARKERS: std::sync::LazyLock<[Vec<u8>; 4]> = std::sync::LazyLock::new(|| {
    let marker = |c: &str, label: Option<&str>| {
        let line = c.repeat(MARKER_SIZE);
        match label {
            Some(label) => format!("{line} {label}").into_bytes(),
            None => line.into_bytes(),
        }
    };
    [
        marker("<", Some(LABELS[0])),
        marker("|", Some(LABELS[1])),
        marker("=", None),
        marker(">", Some(LABELS[2])),
    ]
});

/// The lines of `text` (each with its line ending), and each without it.
fn lines(text: &[u8]) -> impl Iterator<Item = (&[u8], &[u8])> {
    text.split_inclusive(|b| *b == b'\n').map(|line| {
        let bare = line.strip_suffix(b"\n").unwrap_or(line);
        (line, bare.strip_suffix(b"\r").unwrap_or(bare))
    })
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
    for (line, bare) in lines(text) {
        let marker = |m: usize| bare == MARKERS[m];
        match state {
            State::Common if marker(0) => {
                if !common.is_empty() {
                    segments.push(Segment::Common(std::mem::take(&mut common)));
                }
                state = State::Ours;
            }
            State::Common => common.extend_from_slice(line),
            State::Ours if marker(1) => state = State::Base,
            State::Ours | State::Base if marker(2) => state = State::Theirs,
            State::Ours => ours.extend_from_slice(line),
            State::Base => base.extend_from_slice(line),
            State::Theirs if marker(3) => {
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

/// A piece of the file being resolved, in order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Piece<'a> {
    /// Text outside the conflicts.
    Common(&'a [u8]),
    /// A conflict's text as its resolution says.
    Resolved(Vec<u8>),
    /// Conflict number `n` (from 0) has no resolution yet.
    Unresolved(usize),
}

/// The file as far as `resolutions` (one per conflict) resolve it.
pub fn pieces<'a>(
    segments: &'a [Segment],
    resolutions: &'a [Option<Resolution>],
) -> impl Iterator<Item = Piece<'a>> {
    let mut conflict = 0;
    segments.iter().map(move |segment| match segment {
        Segment::Common(text) => Piece::Common(text),
        Segment::Conflict { ours, theirs, .. } => {
            let n = conflict;
            conflict += 1;
            match resolutions.get(n).and_then(Option::as_ref) {
                Some(resolution) => Piece::Resolved(resolution.apply(ours, theirs)),
                None => Piece::Unresolved(n),
            }
        }
    })
}

/// The file's content with every conflict resolved. `None` while any
/// conflict has no resolution.
pub fn assemble(segments: &[Segment], resolutions: &[Option<Resolution>]) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    for piece in pieces(segments, resolutions) {
        match piece {
            Piece::Common(text) => out.extend_from_slice(text),
            Piece::Resolved(text) => out.extend(text),
            Piece::Unresolved(_) => return None,
        }
    }
    Some(out)
}

/// Resolve `path` with this content (in repository form, LF line endings):
/// stage it and write it to the worktree.
pub fn resolve_with(git: &Git, file: &ConflictFile, content: &[u8]) -> Result<()> {
    // Only the labelled markers: a bare line of `=` is a heading underline
    // or a banner just as well.
    let labelled = [&MARKERS[0], &MARKERS[1], &MARKERS[3]];
    if lines(content).any(|(_, bare)| labelled.iter().any(|m| bare == m.as_slice())) {
        bail!(
            "The result for {} still contains conflict markers. Edit them out first.",
            file.path
        );
    }
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

/// The user fixed the file by hand: stage it as it is (deleting it
/// resolves the conflict with a deletion).
pub fn mark_resolved(git: &Git, path: &str) -> Result<()> {
    let bytes = match std::fs::read(git.cwd().join(path)) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(e) => return Err(e).with_context(|| format!("Could not read {path}")),
    };
    if bytes
        .split(|b| *b == b'\n')
        .any(|l| l.starts_with(b"<<<<<<< ") || l.starts_with(b">>>>>>> "))
    {
        bail!("{path} still contains conflict markers (<<<<<<< / >>>>>>>).");
    }
    git.run(["add", "--", path])?;
    Ok(())
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
        let [start, base, sep, end] = MARKERS.clone().map(|m| String::from_utf8(m).unwrap());
        let text = format!("a\n{start}\nB1\n{base}\nb\n{sep}\nB2\n{end}\nc\n");
        let segments = parse_merged(text.as_bytes());
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
        let unresolved: Vec<Piece> = pieces(&segments, &[None]).collect();
        assert_eq!(unresolved[1], Piece::Unresolved(0));
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
    fn side_names_per_operation() {
        let merge = side_names(Some(Operation::Merge), Some("main"));
        assert_eq!(merge.ours.long, "Current (main)");
        assert_eq!(merge.ours.short, "current");
        assert_eq!(merge.theirs.short, "incoming");
        let rebase = side_names(Some(Operation::Rebase), Some("main"));
        assert_eq!(rebase.ours.long, "Upstream");
        assert_eq!(rebase.theirs.short, "your commit");
    }

    #[test]
    fn context_lines() {
        assert_eq!(last_lines(b"1\n2\n3\n", 2), "2\n3");
        assert_eq!(first_lines(b"1\n2\n3\n", 2), "1\n2");
    }
}
