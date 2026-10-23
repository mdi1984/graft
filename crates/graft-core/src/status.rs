//! Working tree status from `git status --porcelain=v2`, which matches git
//! exactly (fsmonitor, untracked cache, renames, conflicts, submodules).

use anyhow::Result;

use crate::cli::Git;
use crate::model::Oid;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WorktreeStatus {
    pub branch: Option<String>,
    pub head: Option<Oid>,
    pub upstream: Option<String>,
    pub ahead: usize,
    pub behind: usize,
    pub entries: Vec<StatusEntry>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StatusEntry {
    pub path: String,
    /// Source path of a rename or copy.
    pub orig_path: Option<String>,
    /// Change between HEAD and the index (the "staged" side).
    pub index: Change,
    /// Change between the index and the working tree (the "unstaged" side).
    pub worktree: Change,
    pub kind: EntryKind,
    pub submodule: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EntryKind {
    Tracked,
    Untracked,
    /// Merge conflict.
    Conflicted,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Change {
    None,
    Modified,
    TypeChanged,
    Added,
    Deleted,
    Renamed,
    Copied,
    Unmerged,
}

impl Change {
    fn from_code(c: u8) -> Self {
        match c {
            b'M' => Self::Modified,
            b'T' => Self::TypeChanged,
            b'A' => Self::Added,
            b'D' => Self::Deleted,
            b'R' => Self::Renamed,
            b'C' => Self::Copied,
            b'U' => Self::Unmerged,
            _ => Self::None,
        }
    }

    /// One-letter badge as shown in the file lists.
    pub fn letter(self) -> &'static str {
        match self {
            Self::None => "",
            Self::Modified => "M",
            Self::TypeChanged => "T",
            Self::Added => "A",
            Self::Deleted => "D",
            Self::Renamed => "R",
            Self::Copied => "C",
            Self::Unmerged => "U",
        }
    }
}

impl StatusEntry {
    pub fn is_staged(&self) -> bool {
        self.kind == EntryKind::Tracked && self.index != Change::None
    }

    pub fn is_unstaged(&self) -> bool {
        match self.kind {
            EntryKind::Untracked => true,
            EntryKind::Tracked => self.worktree != Change::None,
            EntryKind::Conflicted => false,
        }
    }

    pub fn is_conflicted(&self) -> bool {
        self.kind == EntryKind::Conflicted
    }

    /// Badge for the unstaged list ("?" counts as added).
    pub fn unstaged_change(&self) -> Change {
        match self.kind {
            EntryKind::Untracked => Change::Added,
            _ => self.worktree,
        }
    }
}

impl WorktreeStatus {
    pub fn staged(&self) -> impl Iterator<Item = &StatusEntry> {
        self.entries.iter().filter(|e| e.is_staged())
    }

    pub fn unstaged(&self) -> impl Iterator<Item = &StatusEntry> {
        self.entries.iter().filter(|e| e.is_unstaged())
    }

    pub fn conflicted(&self) -> impl Iterator<Item = &StatusEntry> {
        self.entries.iter().filter(|e| e.is_conflicted())
    }
}

/// Read the status of the worktree `git` points at.
pub fn load(git: &Git) -> Result<WorktreeStatus> {
    let out = git.run([
        "-c",
        "core.quotepath=false",
        "status",
        "--porcelain=v2",
        "--branch",
        "--untracked-files=all",
        "-z",
    ])?;
    Ok(parse(&out.stdout))
}

pub fn parse(bytes: &[u8]) -> WorktreeStatus {
    let mut status = WorktreeStatus::default();
    let mut fields = bytes.split(|&b| b == 0).filter(|f| !f.is_empty());
    while let Some(record) = fields.next() {
        let text = String::from_utf8_lossy(record);
        match record[0] {
            b'#' => parse_header(&text, &mut status),
            b'1' => {
                // 1 XY sub mH mI mW hH hI path
                let parts: Vec<&str> = text.splitn(9, ' ').collect();
                if let [_, xy, sub, _, _, _, _, _, path] = parts[..] {
                    status.entries.push(tracked(xy, sub, path, None));
                }
            }
            b'2' => {
                // 2 XY sub mH mI mW hH hI Xscore path, then NUL + original path
                let parts: Vec<&str> = text.splitn(10, ' ').collect();
                let orig = fields
                    .next()
                    .map(|f| String::from_utf8_lossy(f).into_owned());
                if let [_, xy, sub, _, _, _, _, _, _, path] = parts[..] {
                    status.entries.push(tracked(xy, sub, path, orig));
                }
            }
            b'u' => {
                // u XY sub m1 m2 m3 mW h1 h2 h3 path
                let parts: Vec<&str> = text.splitn(11, ' ').collect();
                if let [_, xy, sub, .., path] = parts[..] {
                    let mut entry = tracked(xy, sub, path, None);
                    entry.kind = EntryKind::Conflicted;
                    status.entries.push(entry);
                }
            }
            b'?' => status.entries.push(StatusEntry {
                path: text[2..].to_owned(),
                orig_path: None,
                index: Change::None,
                worktree: Change::Added,
                kind: EntryKind::Untracked,
                submodule: false,
            }),
            _ => {}
        }
    }
    status
}

fn tracked(xy: &str, sub: &str, path: &str, orig_path: Option<String>) -> StatusEntry {
    let xy = xy.as_bytes();
    StatusEntry {
        path: path.to_owned(),
        orig_path,
        index: Change::from_code(xy[0]),
        worktree: Change::from_code(xy[1]),
        kind: EntryKind::Tracked,
        submodule: sub.starts_with('S'),
    }
}

fn parse_header(line: &str, status: &mut WorktreeStatus) {
    let mut words = line.split(' ');
    let _hash = words.next();
    match words.next() {
        Some("branch.oid") => {
            status.head = words.next().and_then(|s| Oid::from_hex(s.as_bytes()).ok());
        }
        Some("branch.head") => {
            status.branch = words
                .next()
                .filter(|s| *s != "(detached)")
                .map(str::to_owned);
        }
        Some("branch.upstream") => status.upstream = words.next().map(str::to_owned),
        Some("branch.ab") => {
            for w in words {
                if let Some(n) = w.strip_prefix('+') {
                    status.ahead = n.parse().unwrap_or(0);
                } else if let Some(n) = w.strip_prefix('-') {
                    status.behind = n.parse().unwrap_or(0);
                }
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_all_record_kinds() {
        let raw = b"# branch.oid 1111111111111111111111111111111111111111\0\
# branch.head main\0# branch.upstream origin/main\0# branch.ab +2 -1\0\
1 .M N... 100644 100644 100644 aaaa bbbb src/lib.rs\0\
1 A. N... 000000 100644 100644 0000 cccc new file.txt\0\
2 R. N... 100644 100644 100644 aaaa aaaa R100 renamed.rs\0old.rs\0\
u UU N... 100644 100644 100644 100644 a b c conflict.rs\0\
? untracked.txt\0";
        let s = parse(raw);
        assert_eq!(s.branch.as_deref(), Some("main"));
        assert_eq!(s.upstream.as_deref(), Some("origin/main"));
        assert_eq!((s.ahead, s.behind), (2, 1));
        assert_eq!(s.entries.len(), 5);
        assert!(s.entries[0].is_unstaged() && !s.entries[0].is_staged());
        assert_eq!(s.entries[1].path, "new file.txt");
        assert!(s.entries[1].is_staged());
        assert_eq!(s.entries[2].orig_path.as_deref(), Some("old.rs"));
        assert_eq!(s.entries[2].index, Change::Renamed);
        assert!(s.entries[3].is_conflicted());
        assert_eq!(s.entries[4].kind, EntryKind::Untracked);
        assert_eq!(s.staged().count(), 2);
        assert_eq!(s.unstaged().count(), 2);
    }

    #[test]
    fn detached_head_has_no_branch() {
        let s = parse(
            b"# branch.oid 1111111111111111111111111111111111111111\0# branch.head (detached)\0",
        );
        assert!(s.branch.is_none());
        assert!(s.head.is_some());
    }
}
