//! Unified diffs: loading them from git, parsing them into hunks and lines
//! (as bytes, so CRLF and odd encodings survive), and building patches from a
//! selection of lines for staging, unstaging and discarding.

use std::ops::Range;

use anyhow::Result;

use crate::changes::EMPTY_TREE;
use crate::cli::{Git, Output};
use crate::model::Change;
use crate::model::Oid;

/// Which pair of trees a diff compares.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Source {
    /// Index → working tree (unstaged changes).
    Unstaged,
    /// HEAD → index (staged changes).
    Staged,
    /// An untracked file, shown as all added.
    Untracked,
    /// A commit against its first parent (or nothing, for a root commit).
    Commit(Oid),
    /// A commit (a review's merge-base) → working tree.
    Base(Oid),
}

impl Source {
    /// What happened to a file, as far as the source alone tells (before
    /// its diff is loaded).
    pub fn change(&self) -> Change {
        match self {
            Source::Untracked => Change::Added,
            _ => Change::Modified,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FileDiff {
    pub path: String,
    /// Everything before the first hunk (`diff --git`, `---`, `+++`, modes).
    pub header: Vec<u8>,
    pub hunks: Vec<Hunk>,
    pub binary: bool,
    pub new_file: bool,
    pub deleted_file: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hunk {
    pub old_start: u32,
    pub old_count: u32,
    pub new_start: u32,
    pub new_count: u32,
    /// Text after the second `@@` (often the enclosing function).
    pub section: String,
    pub lines: Vec<Line>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Context,
    Added,
    Removed,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Line {
    pub kind: Kind,
    pub old_no: Option<u32>,
    pub new_no: Option<u32>,
    /// Content without the marker and without the trailing `\n` (a `\r`
    /// stays, so patches apply byte for byte).
    pub content: Vec<u8>,
    /// Followed by "\ No newline at end of file".
    pub no_newline: bool,
}

/// How a tab shows on screen.
pub const TAB: &str = "    ";

/// A line of a file for display: lossy UTF-8, no trailing `\r`, tabs as
/// spaces.
pub fn display_text(line: &[u8]) -> String {
    let bytes = line.strip_suffix(b"\r").unwrap_or(line);
    String::from_utf8_lossy(bytes).replace('\t', TAB)
}

impl Line {
    /// Content for display, see [`display_text`].
    pub fn text(&self) -> String {
        display_text(&self.content)
    }

    pub fn is_change(&self) -> bool {
        self.kind != Kind::Context
    }
}

impl FileDiff {
    pub fn added(&self) -> usize {
        self.count(Kind::Added)
    }

    pub fn removed(&self) -> usize {
        self.count(Kind::Removed)
    }

    fn count(&self, kind: Kind) -> usize {
        self.hunks
            .iter()
            .flat_map(|h| &h.lines)
            .filter(|l| l.kind == kind)
            .count()
    }

    /// Number of added or removed lines (the selectable ones).
    pub fn changed_lines(&self) -> usize {
        self.added() + self.removed()
    }

    /// What happened to the file, as far as this diff of `source` tells.
    pub fn change(&self, source: &Source) -> Change {
        if self.new_file {
            Change::Added
        } else if self.deleted_file {
            Change::Deleted
        } else {
            source.change()
        }
    }

    /// A combined diff (`diff --cc`), the kind shown for a file with
    /// conflicts.
    pub fn is_combined(&self) -> bool {
        self.header.starts_with(b"diff --cc")
    }
}

/// Load the diff of one file.
pub fn load(git: &Git, source: &Source, path: &str) -> Result<FileDiff> {
    let mut args: Vec<String> = [
        "-c",
        "core.quotepath=false",
        "diff",
        "--no-color",
        "--no-ext-diff",
        "--histogram",
        "--src-prefix=a/",
        "--dst-prefix=b/",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    match source {
        Source::Unstaged => {}
        Source::Staged => args.push("--cached".into()),
        Source::Untracked => args.push("--no-index".into()),
        Source::Commit(id) => {
            args.push(parent(git, id)?.map_or_else(|| EMPTY_TREE.to_owned(), |p| p.to_string()));
            args.push(id.to_string());
        }
        Source::Base(id) => args.push(id.to_string()),
    }
    args.push("--".into());
    if *source == Source::Untracked {
        args.push("/dev/null".into());
    }
    args.push(path.to_owned());

    // `--no-index` exits with 1 when the files differ, which is the point.
    let out = git.run_accepting(&args, None, |o: &Output| {
        o.success() || (*source == Source::Untracked && o.status == 1)
    })?;
    let mut diff = parse(&out.stdout);
    // `--no-index` names the file by its path as given; keep it tidy.
    diff.path = path.to_owned();
    if *source == Source::Untracked {
        diff.new_file = true;
    }
    Ok(diff)
}

/// A commit's first parent, or `None` for a root commit.
fn parent(git: &Git, id: &Oid) -> Result<Option<Oid>> {
    git.rev(&format!("{id}^1"))
}

/// The old version of the file as git stores it (line endings as in the
/// diff), to fill in the unchanged lines between hunks. `None` when there
/// is no old version.
fn old_version(git: &Git, source: &Source, path: &str) -> Result<Option<Vec<u8>>> {
    let rev = match source {
        Source::Unstaged => format!(":0:{path}"),
        Source::Staged => format!("HEAD:{path}"),
        Source::Untracked => return Ok(None),
        Source::Commit(id) => match parent(git, id)? {
            Some(parent) => format!("{parent}:{path}"),
            None => return Ok(None),
        },
        Source::Base(id) => format!("{id}:{path}"),
    };
    let out = git.run_expecting(["cat-file", "blob", &rev], None, |_| true)?;
    Ok(out.success().then_some(out.stdout))
}

/// Unchanged lines the diff does not show, between two hunks or before the
/// first or after the last one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Gap {
    /// Line numbers in the old file, end exclusive.
    pub lines: Range<u32>,
    /// The new file's number of the first line.
    pub new_start: u32,
    /// The hunk right below the gap; `None` for the gap at the end.
    pub before: Option<usize>,
}

impl Gap {
    pub fn len(&self) -> u32 {
        self.lines.end - self.lines.start
    }

    pub fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }

    /// Above the first hunk: there is no hunk above to continue.
    pub fn at_start(&self) -> bool {
        self.before == Some(0)
    }

    pub fn at_end(&self) -> bool {
        self.before.is_none()
    }

    /// Old line numbers `lines` (part of this gap) in the new file.
    pub fn in_new(&self, lines: &Range<u32>) -> Range<u32> {
        let shift = |n: u32| n - self.lines.start + self.new_start;
        shift(lines.start)..shift(lines.end)
    }
}

/// Load the diff of one file with more unchanged lines shown as context,
/// and the gaps that stay hidden. `reveal` and `keep` pick lines by their
/// (old, new) numbers, see [`expand`]. Gaps are `None` when unchanged lines
/// cannot be shown: new, deleted or binary files, no old version to take
/// them from, or one that does not match the diff (a textconv driver, or
/// the index changed in between).
pub fn load_expanded(
    git: &Git,
    source: &Source,
    path: &str,
    reveal: impl Fn(u32, u32) -> bool,
    keep: impl Fn(u32, u32) -> bool,
) -> Result<(FileDiff, Option<Vec<Gap>>)> {
    let mut diff = load(git, source, path)?;
    if diff.binary || diff.new_file || diff.deleted_file || diff.hunks.is_empty() {
        return Ok((diff, None));
    }
    // Without the old version the diff still shows, just not expandable.
    let Ok(Some(old)) = old_version(git, source, path) else {
        return Ok((diff, None));
    };
    if !matches_old(&diff, &old) {
        return Ok((diff, None));
    }
    let gaps = expand(&mut diff, &old, reveal, keep);
    Ok((diff, Some(gaps)))
}

/// Whether every old line the diff shows is in `old` at its number.
fn matches_old(diff: &FileDiff, old: &[u8]) -> bool {
    let old_lines: Vec<&[u8]> = split_lines(old).collect();
    diff.hunks.iter().flat_map(|h| &h.lines).all(|line| {
        line.old_no.is_none_or(|n| {
            (n as usize)
                .checked_sub(1)
                .and_then(|i| old_lines.get(i))
                .is_some_and(|l| *l == line.content.as_slice())
        })
    })
}

/// Show more unchanged lines as context: the ones `reveal` picks, as far as
/// they reach on from a hunk (lines right below a hunk join that hunk, lines
/// right above one join the hunk below), and the ones `keep` picks wherever
/// they are, with every line between them and the nearest hunk. Both get
/// a line's (old, new) numbers. Hunks never merge, so Stage hunk still
/// stages what it did. `old` is the old version of the file. Returns the
/// gaps still hidden.
pub fn expand(
    diff: &mut FileDiff,
    old: &[u8],
    reveal: impl Fn(u32, u32) -> bool,
    keep: impl Fn(u32, u32) -> bool,
) -> Vec<Gap> {
    let no_newline = !old.is_empty() && !old.ends_with(b"\n");
    let old_lines: Vec<&[u8]> = split_lines(old).collect();
    let total = old_lines.len() as u32;
    let new_no = |n: u32, delta: i64| (i64::from(n) + delta) as u32;
    let context = |n: u32, delta: i64| Line {
        kind: Kind::Context,
        old_no: Some(n),
        new_no: Some(new_no(n, delta)),
        content: old_lines[n as usize - 1].to_vec(),
        no_newline: no_newline && n == total,
    };

    // The hidden stretch before each hunk and after the last one, with the
    // shift from old to new line numbers there.
    let mut stretches = Vec::new();
    let (mut next, mut delta) = (1u32, 0i64);
    for (h, hunk) in diff.hunks.iter().enumerate() {
        // An empty side's start is the line before it.
        let first = if hunk.old_count == 0 {
            hunk.old_start + 1
        } else {
            hunk.old_start
        };
        stretches.push((next..first.max(next), delta, Some(h)));
        next = first + hunk.old_count;
        delta += i64::from(hunk.new_count) - i64::from(hunk.old_count);
    }
    stretches.push((next..(total + 1).max(next), delta, None));

    let mut gaps = Vec::new();
    for (range, delta, before) in stretches {
        let (mut start, mut end) = (range.start, range.end.min(total + 1));
        let above = match before {
            Some(h) => h.checked_sub(1),
            None => diff.hunks.len().checked_sub(1),
        };
        // Kept lines open up the stretch from the nearer hunk to them.
        let kept: Vec<u32> = (start..end)
            .filter(|&n| keep(n, new_no(n, delta)))
            .collect();
        let forced = match (kept.first(), kept.last()) {
            (Some(&lo), Some(&hi)) => {
                let down = start..hi + 1;
                let up = lo..end;
                match (above, before) {
                    (None, _) => up,
                    (_, None) => down,
                    _ if down.len() <= up.len() => down,
                    _ => up,
                }
            }
            _ => 0..0,
        };
        let shown = |n: u32| forced.contains(&n) || reveal(n, new_no(n, delta));
        if let Some(a) = above {
            while start < end && shown(start) {
                diff.hunks[a].lines.push(context(start, delta));
                start += 1;
            }
        }
        if let Some(b) = before {
            let mut lines = Vec::new();
            while start < end && shown(end - 1) {
                end -= 1;
                lines.push(context(end, delta));
            }
            lines.reverse();
            diff.hunks[b].lines.splice(0..0, lines);
        }
        if start < end {
            gaps.push(Gap {
                lines: start..end,
                new_start: new_no(start, delta),
                before,
            });
        }
    }
    for hunk in &mut diff.hunks {
        renumber(hunk);
    }
    gaps
}

/// Header numbers that match a hunk's lines again.
fn renumber(hunk: &mut Hunk) {
    let old: Vec<u32> = hunk.lines.iter().filter_map(|l| l.old_no).collect();
    let new: Vec<u32> = hunk.lines.iter().filter_map(|l| l.new_no).collect();
    if let Some(&first) = old.first() {
        hunk.old_start = first;
    }
    if let Some(&first) = new.first() {
        hunk.new_start = first;
    }
    hunk.old_count = old.len() as u32;
    hunk.new_count = new.len() as u32;
}

/// Whether two diffs of a file have the same changed lines in the same
/// hunks, so they differ only in how many unchanged lines show.
pub fn same_changes(a: &FileDiff, b: &FileDiff) -> bool {
    let changes = |d: &FileDiff| -> Vec<(usize, Line)> {
        d.hunks
            .iter()
            .enumerate()
            .flat_map(|(h, hunk)| hunk.lines.iter().map(move |l| (h, l)))
            .filter(|(_, l)| l.is_change())
            .map(|(h, l)| (h, l.clone()))
            .collect()
    };
    a.hunks.len() == b.hunks.len() && changes(a) == changes(b)
}

/// Where the line at (hunk, line) of `from` is in `to`, a diff with the
/// same changes and more or fewer unchanged lines.
pub fn relocate(from: &FileDiff, to: &FileDiff, at: (usize, usize)) -> Option<(usize, usize)> {
    let line = from.hunks.get(at.0)?.lines.get(at.1)?;
    let l = to.hunks.get(at.0)?.lines.iter().position(|other| {
        other.kind == line.kind && other.old_no == line.old_no && other.new_no == line.new_no
    })?;
    Some((at.0, l))
}

/// Parse `git diff` output for a single file.
pub fn parse(bytes: &[u8]) -> FileDiff {
    let mut diff = FileDiff::default();
    let mut lines = split_lines(bytes).peekable();
    while let Some(line) = lines.peek() {
        if line.starts_with(b"@@") {
            break;
        }
        let line = lines.next().expect("peeked");
        if line.starts_with(b"Binary files ") || line.starts_with(b"GIT binary patch") {
            diff.binary = true;
        }
        if line.starts_with(b"new file mode") || line == b"--- /dev/null" {
            diff.new_file = true;
        }
        if line.starts_with(b"deleted file mode") || line == b"+++ /dev/null" {
            diff.deleted_file = true;
        }
        if let Some(path) = line.strip_prefix(b"+++ b/") {
            diff.path = String::from_utf8_lossy(path).into_owned();
        } else if let Some(path) = line.strip_prefix(b"--- a/")
            && diff.path.is_empty()
        {
            diff.path = String::from_utf8_lossy(path).into_owned();
        }
        diff.header.extend_from_slice(line);
        diff.header.push(b'\n');
    }
    let (mut old_no, mut new_no) = (0, 0);
    for line in lines {
        if line.starts_with(b"@@") {
            let Some(hunk) = parse_hunk_header(line) else {
                continue;
            };
            old_no = hunk.old_start;
            new_no = hunk.new_start;
            diff.hunks.push(hunk);
            continue;
        }
        let Some(hunk) = diff.hunks.last_mut() else {
            continue;
        };
        let (marker, content) = match line.split_first() {
            Some((m, rest)) => (*m, rest),
            None => (b' ', &[][..]),
        };
        let (kind, old, new) = match marker {
            b' ' => (Kind::Context, Some(old_no), Some(new_no)),
            b'-' => (Kind::Removed, Some(old_no), None),
            b'+' => (Kind::Added, None, Some(new_no)),
            b'\\' => {
                if let Some(last) = hunk.lines.last_mut() {
                    last.no_newline = true;
                }
                continue;
            }
            _ => continue,
        };
        if old.is_some() {
            old_no += 1;
        }
        if new.is_some() {
            new_no += 1;
        }
        hunk.lines.push(Line {
            kind,
            old_no: old,
            new_no: new,
            content: content.to_vec(),
            no_newline: false,
        });
    }
    diff
}

fn split_lines(bytes: &[u8]) -> impl Iterator<Item = &[u8]> {
    let bytes = bytes.strip_suffix(b"\n").unwrap_or(bytes);
    bytes
        .split(|&b| b == b'\n')
        .filter(move |_| !bytes.is_empty())
}

fn parse_hunk_header(line: &[u8]) -> Option<Hunk> {
    let text = String::from_utf8_lossy(line);
    let rest = text.strip_prefix("@@ ")?;
    let (ranges, section) = rest.split_once(" @@")?;
    let (old, new) = ranges.split_once(' ')?;
    let range = |s: &str| -> Option<(u32, u32)> {
        let s = &s[1..];
        match s.split_once(',') {
            Some((a, b)) => Some((a.parse().ok()?, b.parse().ok()?)),
            None => Some((s.parse().ok()?, 1)),
        }
    };
    let (old_start, old_count) = range(old)?;
    let (new_start, new_count) = range(new)?;
    Some(Hunk {
        old_start,
        old_count,
        new_start,
        new_count,
        section: section.trim().to_owned(),
        lines: Vec::new(),
    })
}

/// How a patch will be applied.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Direction {
    /// `git apply` (staging): unselected `+` lines are dropped, unselected
    /// `-` lines become context.
    Forward,
    /// `git apply --reverse` (unstaging, discarding): unselected `+` lines
    /// become context, unselected `-` lines are dropped.
    Reverse,
}

/// How much of a diff's changed lines a selection takes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Chosen {
    Nothing,
    Part,
    All,
}

/// How much `selected(hunk, line)` takes of the changed lines of `diff`.
pub(crate) fn chosen(diff: &FileDiff, selected: impl Fn(usize, usize) -> bool) -> Chosen {
    let (mut any, mut all) = (false, true);
    for (h, hunk) in diff.hunks.iter().enumerate() {
        for (l, line) in hunk.lines.iter().enumerate() {
            if line.is_change() {
                if selected(h, l) {
                    any = true;
                } else {
                    all = false;
                }
            }
        }
    }
    match (any, all) {
        (false, _) => Chosen::Nothing,
        (true, true) => Chosen::All,
        (true, false) => Chosen::Part,
    }
}

/// A patch containing only the selected lines, or `None` if nothing is
/// selected. `selected(hunk, line)` says whether a changed line is included.
pub(crate) fn build_patch(
    diff: &FileDiff,
    direction: Direction,
    selected: impl Fn(usize, usize) -> bool,
) -> Option<Vec<u8>> {
    let chosen = chosen(diff, &selected);
    if chosen == Chosen::Nothing {
        return None;
    }
    let mut body = Vec::new();
    for (h, hunk) in diff.hunks.iter().enumerate() {
        let mut lines: Vec<(u8, &Line)> = Vec::new();
        let mut hunk_any = false;
        for (l, line) in hunk.lines.iter().enumerate() {
            let take = line.is_change() && selected(h, l);
            hunk_any |= take;
            let marker = match (line.kind, take, direction) {
                (Kind::Context, ..) => Some(b' '),
                (Kind::Added, true, _) => Some(b'+'),
                (Kind::Removed, true, _) => Some(b'-'),
                (Kind::Added, false, Direction::Forward) => None,
                (Kind::Added, false, Direction::Reverse) => Some(b' '),
                (Kind::Removed, false, Direction::Forward) => Some(b' '),
                (Kind::Removed, false, Direction::Reverse) => None,
            };
            if let Some(m) = marker {
                lines.push((m, line));
            }
        }
        if !hunk_any {
            continue;
        }
        let old_count = lines.iter().filter(|(m, _)| *m != b'+').count();
        let new_count = lines.iter().filter(|(m, _)| *m != b'-').count();
        body.extend_from_slice(
            format!(
                "@@ -{},{} +{},{} @@\n",
                hunk.old_start, old_count, hunk.new_start, new_count
            )
            .as_bytes(),
        );
        for (m, line) in lines {
            body.push(m);
            body.extend_from_slice(&line.content);
            body.push(b'\n');
            if line.no_newline {
                body.extend_from_slice(b"\\ No newline at end of file\n");
            }
        }
    }
    let mut patch = if chosen == Chosen::All {
        diff.header.clone()
    } else {
        partial_header(diff, direction)
    };
    patch.extend_from_slice(&body);
    Some(patch)
}

/// A header for a patch that only takes part of a file's change. A partial
/// change of a new or deleted file is a modification of the file in between.
fn partial_header(diff: &FileDiff, direction: Direction) -> Vec<u8> {
    let p = &diff.path;
    let keeps_new_file = diff.new_file && direction == Direction::Forward;
    let keeps_deleted = diff.deleted_file && direction == Direction::Reverse;
    if keeps_new_file || keeps_deleted {
        return diff.header.clone();
    }
    format!("diff --git a/{p} b/{p}\n--- a/{p}\n+++ b/{p}\n").into_bytes()
}

/// The header of a diff that adds `path` as a new file.
pub(crate) fn new_file_header(path: &str) -> Vec<u8> {
    format!("diff --git a/{path} b/{path}\nnew file mode 100644\n--- /dev/null\n+++ b/{path}\n")
        .into_bytes()
}

/// Character ranges that differ between a removed and an added line, for
/// word-level highlights: the part between the common prefix and suffix,
/// widened to word boundaries.
fn changed_ranges(old: &str, new: &str) -> (Range<usize>, Range<usize>) {
    if old == new {
        return (0..0, 0..0);
    }
    let a: Vec<char> = old.chars().collect();
    let b: Vec<char> = new.chars().collect();
    let prefix = a.iter().zip(&b).take_while(|(x, y)| x == y).count();
    let max_suffix = a.len().min(b.len()) - prefix;
    let suffix = a
        .iter()
        .rev()
        .zip(b.iter().rev())
        .take(max_suffix)
        .take_while(|(x, y)| x == y)
        .count();
    let word = |c: &char| c.is_alphanumeric() || *c == '_';
    let mut start = prefix;
    while start > 0 && word(&a[start - 1]) && (start >= a.len() || word(&a[start])) {
        start -= 1;
    }
    let widen_end = |chars: &[char], mut end: usize| {
        while end < chars.len() && end > 0 && word(&chars[end]) && word(&chars[end - 1]) {
            end += 1;
        }
        end
    };
    let end_a = widen_end(&a, a.len() - suffix);
    let end_b = widen_end(&b, b.len() - suffix);
    let to_bytes = |chars: &[char], r: Range<usize>| {
        let s: usize = chars[..r.start].iter().map(|c| c.len_utf8()).sum();
        let e: usize = chars[..r.end.max(r.start)]
            .iter()
            .map(|c| c.len_utf8())
            .sum();
        s..e
    };
    (
        to_bytes(&a, start.min(end_a)..end_a),
        to_bytes(&b, start.min(end_b)..end_b),
    )
}

/// Word-level highlights: for each line of each hunk, the changed byte range
/// of its display text when it pairs with a line on the other side.
pub fn word_highlights(diff: &FileDiff) -> Vec<Vec<Option<Range<usize>>>> {
    diff.hunks
        .iter()
        .map(|hunk| {
            let lines = &hunk.lines;
            let mut out = vec![None; lines.len()];
            let mut i = 0;
            while i < lines.len() {
                if lines[i].kind != Kind::Removed {
                    i += 1;
                    continue;
                }
                let removed_start = i;
                while i < lines.len() && lines[i].kind == Kind::Removed {
                    i += 1;
                }
                let added_start = i;
                while i < lines.len() && lines[i].kind == Kind::Added {
                    i += 1;
                }
                let pairs = (added_start - removed_start).min(i - added_start);
                for k in 0..pairs {
                    let (old, new) = (
                        lines[removed_start + k].text(),
                        lines[added_start + k].text(),
                    );
                    let (a, b) = changed_ranges(&old, &new);
                    // Lines with nothing in common read better without highlights.
                    if a.len() < old.len() || b.len() < new.len() {
                        out[removed_start + k] = Some(a);
                        out[added_start + k] = Some(b);
                    }
                }
            }
            out
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &[u8] = b"diff --git a/src/a.rs b/src/a.rs
index 111..222 100644
--- a/src/a.rs
+++ b/src/a.rs
@@ -1,4 +1,4 @@ fn main()
 one
-two
+TWO
+two and a half
 three
-four
\\ No newline at end of file
";

    #[test]
    fn parses_hunks_and_numbers() {
        let d = parse(SAMPLE);
        assert_eq!(d.path, "src/a.rs");
        assert_eq!(d.hunks.len(), 1);
        let h = &d.hunks[0];
        assert_eq!(h.section, "fn main()");
        assert_eq!(h.lines.len(), 6);
        assert_eq!(h.lines[1].kind, Kind::Removed);
        assert_eq!(h.lines[1].old_no, Some(2));
        assert_eq!(h.lines[3].new_no, Some(3));
        assert!(h.lines[5].no_newline);
        assert_eq!((d.added(), d.removed()), (2, 2));
    }

    #[test]
    fn forward_patch_keeps_selected_lines_only() {
        let d = parse(SAMPLE);
        // Select only "+TWO" (line 2).
        let patch = build_patch(&d, Direction::Forward, |_, l| l == 2).unwrap();
        let text = String::from_utf8(patch).unwrap();
        assert!(text.contains("@@ -1,4 +1,5 @@\n one\n two\n+TWO\n three\n four\n\\ No newline"));
        assert!(
            text.starts_with("diff --git a/src/a.rs b/src/a.rs\n--- a/src/a.rs\n+++ b/src/a.rs\n")
        );
    }

    #[test]
    fn reverse_patch_turns_unselected_additions_into_context() {
        let d = parse(SAMPLE);
        // Select only "-two" (line 1).
        let patch = build_patch(&d, Direction::Reverse, |_, l| l == 1).unwrap();
        let text = String::from_utf8(patch).unwrap();
        assert!(text.contains("@@ -1,5 +1,4 @@\n one\n-two\n TWO\n two and a half\n three\n"));
        assert!(!text.contains("four"));
    }

    #[test]
    fn nothing_selected_is_no_patch() {
        let d = parse(SAMPLE);
        assert!(build_patch(&d, Direction::Forward, |_, _| false).is_none());
    }

    #[test]
    fn full_selection_keeps_header() {
        let d = parse(SAMPLE);
        let patch = build_patch(&d, Direction::Forward, |_, _| true).unwrap();
        assert!(patch.starts_with(b"diff --git a/src/a.rs b/src/a.rs\nindex 111..222"));
    }

    #[test]
    fn binary_and_new_files() {
        let d = parse(b"diff --git a/x.png b/x.png\nnew file mode 100644\nindex 0..1\nBinary files /dev/null and b/x.png differ\n");
        assert!(d.binary && d.new_file && d.hunks.is_empty());
    }

    /// Twelve lines; line 5 changed, line 9 replaced by two.
    const OLD: &[u8] = b"1\n2\n3\n4\n5\n6\n7\n8\n9\n10\n11\n12";
    const TWO_HUNKS: &[u8] = b"diff --git a/n.txt b/n.txt
--- a/n.txt
+++ b/n.txt
@@ -4,3 +4,3 @@
 4
-5
+five
 6
@@ -8,3 +8,4 @@
 8
-9
+nine
+nine and a bit
 10
";

    fn numbers(d: &FileDiff) -> Vec<(Option<u32>, Option<u32>, String)> {
        d.hunks
            .iter()
            .flat_map(|h| &h.lines)
            .map(|l| (l.old_no, l.new_no, l.text()))
            .collect()
    }

    #[test]
    fn gaps_without_expanding() {
        let mut d = parse(TWO_HUNKS);
        let before = d.clone();
        let gaps = expand(&mut d, OLD, |_, _| false, |_, _| false);
        assert_eq!(d, before);
        let shape: Vec<_> = gaps.iter().map(|g| (g.lines.clone(), g.before)).collect();
        assert_eq!(shape, [(1..4, Some(0)), (7..8, Some(1)), (11..13, None)]);
        assert!(gaps[0].at_start() && gaps[2].at_end());
    }

    #[test]
    fn expanding_joins_lines_to_the_nearest_hunk() {
        let mut d = parse(TWO_HUNKS);
        // Line 3 (above hunk 0), 7 (below hunk 0) and 11 (after hunk 1).
        let gaps = expand(&mut d, OLD, |n, _| [3, 7, 11].contains(&n), |_, _| false);
        let h0 = &d.hunks[0];
        assert_eq!(
            (h0.old_start, h0.old_count, h0.new_start, h0.new_count),
            (3, 5, 3, 5)
        );
        assert_eq!(h0.lines[0].text(), "3");
        assert_eq!(h0.lines.last().unwrap().text(), "7");
        let h1 = &d.hunks[1];
        assert_eq!(
            (h1.old_start, h1.old_count, h1.new_start, h1.new_count),
            (8, 4, 8, 5)
        );
        // After hunk 1 the new side is one line ahead.
        assert_eq!(
            h1.lines.last().map(|l| (l.old_no, l.new_no)),
            Some((Some(11), Some(12)))
        );
        let shape: Vec<_> = gaps.iter().map(|g| (g.lines.clone(), g.before)).collect();
        assert_eq!(shape, [(1..3, Some(0)), (12..13, None)]);
    }

    #[test]
    fn whole_file_keeps_hunks_and_the_missing_newline() {
        let mut d = parse(TWO_HUNKS);
        let gaps = expand(&mut d, OLD, |_, _| true, |_, _| false);
        assert!(gaps.is_empty());
        assert_eq!(d.hunks.len(), 2);
        let all = numbers(&d);
        assert_eq!(all.len(), 15);
        assert_eq!(all[0], (Some(1), Some(1), "1".to_owned()));
        assert_eq!(all[14], (Some(12), Some(13), "12".to_owned()));
        assert!(d.hunks[1].lines.last().unwrap().no_newline);
        // A patch from the expanded diff still stages just the change.
        let patch = build_patch(&d, Direction::Forward, |h, _| h == 0).unwrap();
        let text = String::from_utf8(patch).unwrap();
        assert!(text.contains("@@ -1,7 +1,7 @@\n 1\n 2\n 3\n 4\n-5\n+five\n 6\n 7\n"));
    }

    #[test]
    fn insertion_at_the_top() {
        // Two lines added before line 1.
        let mut d = parse(
            b"diff --git a/n.txt b/n.txt\n--- a/n.txt\n+++ b/n.txt\n@@ -0,0 +1,2 @@\n+a\n+b\n",
        );
        let gaps = expand(&mut d, b"1\n2\n3\n", |n, _| n == 1, |_, _| false);
        let h = &d.hunks[0];
        assert_eq!(
            (h.old_start, h.old_count, h.new_start, h.new_count),
            (1, 1, 1, 3)
        );
        assert_eq!(h.lines[2].new_no, Some(3));
        assert_eq!(
            gaps,
            [Gap {
                lines: 2..4,
                new_start: 4,
                before: None
            }]
        );
    }

    #[test]
    fn kept_lines_show_with_the_way_from_the_nearer_hunk() {
        // Old 10 is new 11 (after hunk 1); old 2 is above the first hunk.
        let mut d = parse(TWO_HUNKS);
        let gaps = expand(&mut d, OLD, |_, _| false, |old, new| old == 2 || new == 12);
        let shape: Vec<_> = gaps.iter().map(|g| (g.lines.clone(), g.before)).collect();
        // 2..4 joins hunk 0 from above, 11 joins hunk 1 from below.
        assert_eq!(shape, [(1..2, Some(0)), (7..8, Some(1)), (12..13, None)]);
        assert_eq!(gaps[2].new_start, 13);
        assert_eq!(gaps[2].in_new(&(12..13)), 13..14);
    }

    #[test]
    fn a_diff_that_does_not_match_the_old_file_is_not_expanded() {
        let d = parse(TWO_HUNKS);
        assert!(matches_old(&d, OLD));
        assert!(!matches_old(
            &d,
            b"1\n2\n3\n4\nFIVE\n6\n7\n8\n9\n10\n11\n12"
        ));
        assert!(!matches_old(&d, b"1\n2\n3"));
    }

    #[test]
    fn selections_follow_an_expansion() {
        let small = parse(TWO_HUNKS);
        let mut big = small.clone();
        expand(&mut big, OLD, |_, _| true, |_, _| false);
        assert!(same_changes(&small, &big));
        // Line 7 joins hunk 0, so "+nine" stays put and "-5" moves down.
        assert_eq!(relocate(&small, &big, (1, 2)), Some((1, 2)));
        assert_eq!(relocate(&small, &big, (0, 1)), Some((0, 4)));
        let mut other = small.clone();
        other.hunks[0].lines[2].content = b"FIVE".to_vec();
        assert!(!same_changes(&small, &other));
    }

    #[test]
    fn word_ranges() {
        let (a, b) = changed_ranges("let count = 1;", "let total = 1;");
        assert_eq!(&"let count = 1;"[a], "count");
        assert_eq!(&"let total = 1;"[b], "total");
        let (a, b) = changed_ranges("abc", "abc");
        assert!(a.is_empty() && b.is_empty());
    }
}
