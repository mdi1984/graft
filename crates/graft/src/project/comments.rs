//! Review comments on lines of the review diff: which lines are picked,
//! where a comment shows, and the Markdown copied for a coding agent.

use std::collections::{BTreeMap, BTreeSet};

use graft_core::diff::{FileDiff, Kind, Line};
use serde::{Deserialize, Serialize};

/// Which version of the file line numbers refer to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Side {
    /// The base version (removed lines).
    Old,
    /// The worktree's version.
    New,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LineComment {
    pub id: u64,
    pub path: String,
    pub side: Side,
    pub start: u32,
    pub end: u32,
    /// The lines as they read when commented, with diff markers. Copied as
    /// context, and tells when the code has changed since (outdated).
    pub excerpt: String,
    pub text: String,
}

impl LineComment {
    /// "Line 12", "Lines 12–18"; removed lines name the old numbering.
    pub fn lines_label(&self) -> String {
        lines_label(self.side, self.start, self.end)
    }
}

pub fn lines_label(side: Side, start: u32, end: u32) -> String {
    let lines = if start == end {
        format!("line {start}")
    } else {
        format!("lines {start}–{end}")
    };
    match side {
        Side::New => capitalize(&lines),
        Side::Old => format!("Removed {lines} (old numbering)"),
    }
}

fn capitalize(s: &str) -> String {
    let mut chars = s.chars();
    chars
        .next()
        .map(|c| c.to_uppercase().chain(chars).collect())
        .unwrap_or_default()
}

/// Lines picked in the diff, from `anchor` to `cursor` in diff order. In the
/// side-by-side view `side` keeps the pick to one column.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Pick {
    pub anchor: (usize, usize),
    pub cursor: (usize, usize),
    pub side: Option<Side>,
}

impl Pick {
    pub fn contains(&self, at: (usize, usize), kind: Kind) -> bool {
        let (lo, hi) = if self.anchor <= self.cursor {
            (self.anchor, self.cursor)
        } else {
            (self.cursor, self.anchor)
        };
        let on_side = match self.side {
            Some(Side::New) => kind != Kind::Removed,
            Some(Side::Old) => kind != Kind::Added,
            None => true,
        };
        lo <= at && at <= hi && on_side
    }
}

/// Every line of the diff with its (hunk, line) position.
fn rows(diff: &FileDiff) -> impl Iterator<Item = ((usize, usize), &Line)> {
    diff.hunks.iter().enumerate().flat_map(|(h, hunk)| {
        hunk.lines
            .iter()
            .enumerate()
            .map(move |(l, line)| ((h, l), line))
    })
}

fn number(line: &Line, side: Side) -> Option<u32> {
    match side {
        Side::New if line.kind != Kind::Removed => line.new_no,
        Side::Old if line.kind != Kind::Added => line.old_no,
        _ => None,
    }
}

/// The picked lines as a range: in the worktree's numbering when the pick
/// has any line that is still there, else in the base's.
pub fn range(diff: &FileDiff, pick: &Pick) -> Option<(Side, u32, u32)> {
    let picked: Vec<&Line> = rows(diff)
        .filter(|(at, line)| pick.contains(*at, line.kind))
        .map(|(_, line)| line)
        .collect();
    let side = if pick.side == Some(Side::Old) || picked.iter().all(|l| l.kind == Kind::Removed) {
        Side::Old
    } else {
        Side::New
    };
    let numbers = picked.iter().filter_map(|l| number(l, side));
    let start = numbers.clone().min()?;
    let end = numbers.max()?;
    Some((side, start, end))
}

fn locate(diff: &FileDiff, side: Side, n: u32) -> Option<(usize, usize)> {
    rows(diff)
        .find(|(_, line)| number(line, side) == Some(n))
        .map(|(at, _)| at)
}

/// Lines `start..=end` on `side` with diff markers. On the worktree side the
/// removed lines in between and right before come along: what was replaced.
pub fn excerpt(diff: &FileDiff, side: Side, start: u32, end: u32) -> Option<String> {
    let first = locate(diff, side, start)?;
    let last = locate(diff, side, end)?;
    let all: Vec<_> = rows(diff).collect();
    let mut i = all.iter().position(|(at, _)| *at == first)?;
    let j = all.iter().position(|(at, _)| *at == last)?;
    if side == Side::New {
        while i > 0 && all[i - 1].1.kind == Kind::Removed && all[i - 1].0.0 == first.0 {
            i -= 1;
        }
    }
    let mut out = String::new();
    for (_, line) in &all[i..=j] {
        let marker = match line.kind {
            Kind::Added if side == Side::Old => continue,
            Kind::Added => '+',
            Kind::Removed => '-',
            Kind::Context => ' ',
        };
        let content = line.content.strip_suffix(b"\r").unwrap_or(&line.content);
        out.push(marker);
        out.push_str(&String::from_utf8_lossy(content));
        out.push('\n');
    }
    Some(out)
}

/// A comment and the (hunk, line) it ends on.
pub type Placed<'a> = ((usize, usize), &'a LineComment);

/// The comments that still match the diff, each with the line it ends on.
/// The others are outdated: their lines changed since.
pub fn place<'a>(
    diff: &FileDiff,
    comments: impl IntoIterator<Item = &'a LineComment>,
) -> (Vec<Placed<'a>>, Vec<&'a LineComment>) {
    let mut placed = Vec::new();
    let mut outdated = Vec::new();
    for c in comments {
        let current = excerpt(diff, c.side, c.start, c.end);
        match locate(diff, c.side, c.end) {
            Some(at) if current.as_deref() == Some(c.excerpt.as_str()) => placed.push((at, c)),
            _ => outdated.push(c),
        }
    }
    (placed, outdated)
}

/// File notes and line comments as Markdown for a coding agent.
pub fn export(title: &str, notes: &BTreeMap<String, String>, comments: &[LineComment]) -> String {
    let notes: BTreeMap<&str, &str> = notes
        .iter()
        .map(|(path, note)| (path.as_str(), note.trim()))
        .filter(|(_, note)| !note.is_empty())
        .collect();
    let paths: BTreeSet<&str> = notes
        .keys()
        .copied()
        .chain(comments.iter().map(|c| c.path.as_str()))
        .collect();

    let mut out = format!(
        "# Review comments: {title}\n\n\
         Please address these review comments. Line numbers refer to the files as they \
         are in the working tree; `-` lines in the excerpts were removed, `+` lines added.\n"
    );
    for path in paths {
        out.push_str(&format!("\n## {path}\n"));
        if let Some(note) = notes.get(path) {
            out.push_str(&format!("\nAbout the whole file:\n{note}\n"));
        }
        let mut here: Vec<&LineComment> = comments.iter().filter(|c| c.path == path).collect();
        here.sort_by_key(|c| (c.side, c.start, c.end, c.id));
        for c in here {
            let fence = fence_for(&c.excerpt);
            out.push_str(&format!(
                "\n### {}\n{fence}diff\n{}{fence}\n{}\n",
                c.lines_label(),
                c.excerpt,
                c.text.trim()
            ));
        }
    }
    out
}

/// A code fence longer than any run of backticks in `text`.
fn fence_for(text: &str) -> String {
    let mut longest = 0;
    let mut run = 0;
    for ch in text.chars() {
        run = if ch == '`' { run + 1 } else { 0 };
        longest = longest.max(run);
    }
    "`".repeat(longest.max(2) + 1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use graft_core::diff::Hunk;

    fn line(kind: Kind, old: Option<u32>, new: Option<u32>, text: &str) -> Line {
        Line {
            kind,
            old_no: old,
            new_no: new,
            content: text.as_bytes().to_vec(),
            no_newline: false,
        }
    }

    /// 1 a | -2 b | -3 c | +2 B | +3 C | 4 d
    fn diff() -> FileDiff {
        FileDiff {
            hunks: vec![Hunk {
                old_start: 1,
                old_count: 4,
                new_start: 1,
                new_count: 4,
                section: String::new(),
                lines: vec![
                    line(Kind::Context, Some(1), Some(1), "a"),
                    line(Kind::Removed, Some(2), None, "b"),
                    line(Kind::Removed, Some(3), None, "c"),
                    line(Kind::Added, None, Some(2), "B"),
                    line(Kind::Added, None, Some(3), "C"),
                    line(Kind::Context, Some(4), Some(4), "d\r"),
                ],
            }],
            ..FileDiff::default()
        }
    }

    fn pick(a: usize, b: usize, side: Option<Side>) -> Pick {
        Pick {
            anchor: (0, a),
            cursor: (0, b),
            side,
        }
    }

    #[test]
    fn ranges() {
        let d = diff();
        // Unified, dragged upwards over a replacement: the new numbering.
        assert_eq!(range(&d, &pick(4, 1, None)), Some((Side::New, 2, 3)));
        // Only removed lines: the old numbering.
        assert_eq!(range(&d, &pick(1, 2, None)), Some((Side::Old, 2, 3)));
        // Left column of side by side, across context.
        assert_eq!(
            range(&d, &pick(0, 5, Some(Side::Old))),
            Some((Side::Old, 1, 4))
        );
        assert_eq!(
            range(&d, &pick(0, 5, Some(Side::New))),
            Some((Side::New, 1, 4))
        );
    }

    #[test]
    fn excerpts_carry_what_was_replaced() {
        let d = diff();
        assert_eq!(excerpt(&d, Side::New, 2, 3).unwrap(), "-b\n-c\n+B\n+C\n");
        assert_eq!(excerpt(&d, Side::New, 4, 4).unwrap(), " d\n");
        assert_eq!(excerpt(&d, Side::Old, 1, 3).unwrap(), " a\n-b\n-c\n");
        assert_eq!(excerpt(&d, Side::New, 9, 9), None);
    }

    #[test]
    fn outdated_when_the_lines_changed() {
        let d = diff();
        let mut c = LineComment {
            id: 1,
            path: "f.rs".into(),
            side: Side::New,
            start: 2,
            end: 3,
            excerpt: excerpt(&d, Side::New, 2, 3).unwrap(),
            text: "why?".into(),
        };
        let (placed, outdated) = place(&d, [&c]);
        assert_eq!(placed[0].0, (0, 4));
        assert!(outdated.is_empty());
        c.excerpt = "+other\n".into();
        let (placed, outdated) = place(&d, [&c]);
        assert!(placed.is_empty() && outdated.len() == 1);
    }

    #[test]
    fn export_groups_by_file() {
        let c = LineComment {
            id: 1,
            path: "src/a.rs".into(),
            side: Side::New,
            start: 2,
            end: 3,
            excerpt: "+```x\n".into(),
            text: " Rename this. ".into(),
        };
        let notes = BTreeMap::from([
            ("src/a.rs".to_owned(), "Split the module.".to_owned()),
            ("empty.rs".to_owned(), "  ".to_owned()),
        ]);
        let text = export("feature against main", &notes, &[c]);
        assert!(text.starts_with("# Review comments: feature against main"));
        assert!(text.contains(
            "## src/a.rs\n\nAbout the whole file:\nSplit the module.\n\n### Lines 2–3\n\
             ````diff\n+```x\n````\nRename this.\n"
        ));
        assert!(!text.contains("empty.rs"));
    }
}
