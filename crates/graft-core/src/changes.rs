//! The files that changed between two trees, with line counts: what a
//! commit, a stash or a review shows in its file list.

use anyhow::Result;

use crate::cli::Git;
use crate::model::Change;

/// git's id of the empty tree: what a root commit is compared with.
pub(crate) const EMPTY_TREE: &str = "4b825dc642cb6eb9a060e54bf8d69288fbee4904";

/// How much of a file git looks at to call it binary.
const BINARY_PROBE: usize = 8000;

/// Whether `content` is binary the way git judges it: a NUL byte near the start.
pub(crate) fn is_binary(content: &[u8]) -> bool {
    content.iter().take(BINARY_PROBE).any(|b| *b == 0)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileChange {
    pub path: String,
    pub orig_path: Option<String>,
    pub change: Change,
    /// `None` for binary files.
    pub added: Option<usize>,
    pub removed: Option<usize>,
}

/// The files changed from `from` to `to` (commits or trees); `to: None`
/// means the worktree, uncommitted changes included (untracked files not).
pub fn between(git: &Git, from: &str, to: Option<&str>) -> Result<Vec<FileChange>> {
    let mut common = vec!["-c", "core.quotepath=false"];
    match to {
        Some(_) => common.extend(["diff-tree", "-r", "-z", "-M", "--no-commit-id"]),
        None => common.extend(["diff", "-z", "-M"]),
    }
    let args = |kind: &'static str| {
        let mut args = common.clone();
        args.push(kind);
        args.push(from);
        args.extend(to);
        args
    };
    // Side by side: each git start costs tens of milliseconds on Windows.
    let (status, numstat) = std::thread::scope(|s| {
        let numstat = s.spawn(|| git.run(args("--numstat")));
        let status = git.run(args("--name-status"));
        (status, numstat.join().expect("numstat thread"))
    });
    let (status, numstat) = (status?, numstat?);
    let mut files = parse_name_status(&status.stdout);
    apply_numstat(&mut files, &numstat.stdout);
    Ok(files)
}

fn parse_name_status(bytes: &[u8]) -> Vec<FileChange> {
    let mut files = Vec::new();
    let mut fields = bytes
        .split(|&b| b == 0)
        .map(|f| String::from_utf8_lossy(f).into_owned());
    while let Some(code) = fields.next() {
        if code.is_empty() {
            continue;
        }
        let change = match Change::from_code(code.as_bytes()[0]) {
            Change::None => Change::Modified,
            change => change,
        };
        let (orig_path, path) = if matches!(change, Change::Renamed | Change::Copied) {
            let orig = fields.next().unwrap_or_default();
            (Some(orig), fields.next().unwrap_or_default())
        } else {
            (None, fields.next().unwrap_or_default())
        };
        files.push(FileChange {
            path,
            orig_path,
            change,
            added: None,
            removed: None,
        });
    }
    files
}

/// `--numstat -z`: "added\tremoved\tpath\0", or for renames
/// "added\tremoved\t\0old\0new\0". Binary files show "-".
fn apply_numstat(files: &mut [FileChange], bytes: &[u8]) {
    // By path: a commit can touch tens of thousands of files.
    let index: std::collections::HashMap<String, usize> = files
        .iter()
        .enumerate()
        .map(|(i, f)| (f.path.clone(), i))
        .collect();
    let mut fields = bytes
        .split(|&b| b == 0)
        .map(|f| String::from_utf8_lossy(f).into_owned());
    while let Some(record) = fields.next() {
        if record.is_empty() {
            continue;
        }
        let mut parts = record.splitn(3, '\t');
        let added = parts.next().and_then(|n| n.parse().ok());
        let removed = parts.next().and_then(|n| n.parse().ok());
        let mut path = parts.next().unwrap_or_default().to_owned();
        if path.is_empty() {
            let _old = fields.next();
            path = fields.next().unwrap_or_default();
        }
        if let Some(&i) = index.get(&path) {
            files[i].added = added;
            files[i].removed = removed;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn name_status_and_numstat() {
        let ns = b"M\0src/a.rs\0R100\0old.rs\0new.rs\0A\0img.png\0";
        let mut files = parse_name_status(ns);
        assert_eq!(files.len(), 3);
        assert_eq!(files[1].orig_path.as_deref(), Some("old.rs"));
        assert_eq!(files[1].path, "new.rs");
        apply_numstat(&mut files, b"3\t1\tsrc/a.rs\0" as &[u8]);
        apply_numstat(&mut files, b"0\t0\t\0old.rs\0new.rs\0-\t-\timg.png\0");
        assert_eq!((files[0].added, files[0].removed), (Some(3), Some(1)));
        assert_eq!(files[1].added, Some(0));
        assert_eq!(files[2].added, None);
    }
}
