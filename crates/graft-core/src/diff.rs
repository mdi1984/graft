//! Unified diffs: loading them from git, parsing them into hunks and lines
//! (as bytes, so CRLF and odd encodings survive), and building patches from a
//! selection of lines for staging, unstaging and discarding.

use std::ops::Range;

use anyhow::{Result, bail};

use crate::cli::{Git, failure_message};
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

impl Line {
    /// Content for display: lossy UTF-8, no trailing `\r`, tabs as spaces.
    pub fn text(&self) -> String {
        let bytes = self.content.strip_suffix(b"\r").unwrap_or(&self.content);
        String::from_utf8_lossy(bytes).replace('\t', "    ")
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
            // Against the first parent; a root commit against the empty tree.
            let parent = git.run_raw(["rev-parse", "-q", "--verify", &format!("{id}^1")])?;
            let base = if parent.success() {
                parent.stdout_str().trim().to_owned()
            } else {
                EMPTY_TREE.to_owned()
            };
            args.push(base);
            args.push(id.to_string());
        }
    }
    args.push("--".into());
    if *source == Source::Untracked {
        args.push("/dev/null".into());
    }
    args.push(path.to_owned());

    let out = git.run_raw(&args)?;
    // `--no-index` exits with 1 when the files differ, which is the point.
    let ok = out.success() || (*source == Source::Untracked && out.status == 1);
    if !ok {
        bail!(failure_message(&out));
    }
    let mut diff = parse(&out.stdout);
    diff.path = path.to_owned();
    if *source == Source::Untracked {
        // `--no-index` names the file by its path as given; keep it tidy.
        diff.new_file = true;
    }
    Ok(diff)
}

const EMPTY_TREE: &str = "4b825dc642cb6eb9a060e54bf8d69288fbee4904";

/// Parse `git diff` output for a single file.
pub fn parse(bytes: &[u8]) -> FileDiff {
    let mut diff = FileDiff::default();
    let mut lines = split_lines(bytes).peekable();
    // Header.
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
    // Hunks.
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
pub enum Direction {
    /// `git apply` (staging): unselected `+` lines are dropped, unselected
    /// `-` lines become context.
    Forward,
    /// `git apply --reverse` (unstaging, discarding): unselected `+` lines
    /// become context, unselected `-` lines are dropped.
    Reverse,
}

/// A patch containing only the selected lines, or `None` if nothing is
/// selected. `selected(hunk, line)` says whether a changed line is included.
pub fn build_patch(
    diff: &FileDiff,
    direction: Direction,
    selected: impl Fn(usize, usize) -> bool,
) -> Option<Vec<u8>> {
    let mut body = Vec::new();
    let mut all = true;
    let mut any = false;
    for (h, hunk) in diff.hunks.iter().enumerate() {
        let mut lines: Vec<(u8, &Line)> = Vec::new();
        let mut hunk_any = false;
        for (l, line) in hunk.lines.iter().enumerate() {
            let take = line.is_change() && selected(h, l);
            if line.is_change() && !take {
                all = false;
            }
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
        any = true;
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
    if !any {
        return None;
    }
    let mut patch = if all {
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

/// Character ranges that differ between a removed and an added line, for
/// word-level highlights: the part between the common prefix and suffix,
/// widened to word boundaries.
pub fn changed_ranges(old: &str, new: &str) -> (Range<usize>, Range<usize>) {
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

    #[test]
    fn word_ranges() {
        let (a, b) = changed_ranges("let count = 1;", "let total = 1;");
        assert_eq!(&"let count = 1;"[a], "count");
        assert_eq!(&"let total = 1;"[b], "total");
        let (a, b) = changed_ranges("abc", "abc");
        assert!(a.is_empty() && b.is_empty());
    }
}
