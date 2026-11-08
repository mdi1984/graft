//! Working tree status from `git status --porcelain=v2`, which matches git
//! exactly (fsmonitor, untracked cache, renames, conflicts, submodules).

use anyhow::Result;

use crate::cli::Git;
use crate::model::{Change, EntryKind, StatusEntry, WorktreeStatus};

/// Read the status of the worktree `git` points at.
pub fn load(git: &Git) -> Result<WorktreeStatus> {
    let out = git.run([
        "status",
        "--porcelain=v2",
        "--branch",
        "--untracked-files=all",
        "-z",
    ])?;
    Ok(parse(&out.stdout))
}

fn parse(bytes: &[u8]) -> WorktreeStatus {
    let mut status = WorktreeStatus::default();
    let mut fields = bytes.split(|&b| b == 0).filter(|f| !f.is_empty());
    while let Some(record) = fields.next() {
        let text = String::from_utf8_lossy(record);
        match record[0] {
            b'#' => parse_header(&text, &mut status),
            b'1' => {
                // 1 XY sub mH mI mW hH hI path
                let parts: Vec<&str> = text.splitn(9, ' ').collect();
                if let [_, xy, _, _, _, _, _, _, path] = parts[..] {
                    status.entries.push(tracked(xy, path, None));
                }
            }
            b'2' => {
                // 2 XY sub mH mI mW hH hI Xscore path, then NUL + original path
                let parts: Vec<&str> = text.splitn(10, ' ').collect();
                let orig = fields
                    .next()
                    .map(|f| String::from_utf8_lossy(f).into_owned());
                if let [_, xy, _, _, _, _, _, _, _, path] = parts[..] {
                    status.entries.push(tracked(xy, path, orig));
                }
            }
            b'u' => {
                // u XY sub m1 m2 m3 mW h1 h2 h3 path
                let parts: Vec<&str> = text.splitn(11, ' ').collect();
                if let [_, xy, .., path] = parts[..] {
                    let mut entry = tracked(xy, path, None);
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
            }),
            _ => {}
        }
    }
    status
}

fn tracked(xy: &str, path: &str, orig_path: Option<String>) -> StatusEntry {
    let xy = xy.as_bytes();
    StatusEntry {
        path: path.to_owned(),
        orig_path,
        index: Change::from_code(xy[0]),
        worktree: Change::from_code(xy[1]),
        kind: EntryKind::Tracked,
    }
}

/// `# branch.ab +ahead -behind`. Branch and HEAD come from `worktree list`.
fn parse_header(line: &str, status: &mut WorktreeStatus) {
    let Some(counts) = line.strip_prefix("# branch.ab ") else {
        return;
    };
    for w in counts.split(' ') {
        if let Some(n) = w.strip_prefix('+') {
            status.ahead = n.parse().unwrap_or(0);
        } else if let Some(n) = w.strip_prefix('-') {
            status.behind = n.parse().unwrap_or(0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::FileList;

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
        assert_eq!(s.entries[3].shown_change(), Change::Unmerged);
        assert_eq!(s.entries[1].shown_change(), Change::Added);
    }

    /// Conflicts first, then staged, then unstaged; a file changed on both
    /// sides is listed twice.
    #[test]
    fn listed_in_display_order() {
        let raw = b"1 MM N... 100644 100644 100644 aaaa bbbb both.rs\0\
? new.txt\0\
u UU N... 100644 100644 100644 100644 a b c conflict.rs\0";
        let s = parse(raw);
        let rows: Vec<(FileList, &str)> = s.listed().map(|(l, e)| (l, e.path.as_str())).collect();
        assert_eq!(
            rows,
            [
                (FileList::Conflicts, "conflict.rs"),
                (FileList::Staged, "both.rs"),
                (FileList::Unstaged, "both.rs"),
                (FileList::Unstaged, "new.txt"),
            ]
        );
    }
}
