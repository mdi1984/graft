//! Full details of one commit: message, people, and the files it changed.

use anyhow::{Result, bail};

use crate::cli::Git;
use crate::model::Oid;
use crate::status::Change;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommitDetails {
    pub id: Oid,
    pub parents: Vec<Oid>,
    pub author: String,
    pub author_email: String,
    pub author_time: i64,
    pub committer: String,
    pub committer_email: String,
    pub commit_time: i64,
    pub message: String,
    /// Changes against the first parent (or the empty tree for a root commit).
    pub files: Vec<FileChange>,
}

impl CommitDetails {
    pub fn summary(&self) -> &str {
        self.message.lines().next().unwrap_or("")
    }

    /// The message without its first line and the blank line after it.
    pub fn body(&self) -> &str {
        match self.message.split_once('\n') {
            Some((_, rest)) => rest.trim_start_matches(['\n', '\r']).trim_end(),
            None => "",
        }
    }
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

pub fn load(git: &Git, id: Oid) -> Result<CommitDetails> {
    let hex = id.to_string();
    let out = git.run([
        "-c",
        "log.showSignature=false",
        "show",
        "-s",
        "--format=%H%x00%P%x00%an%x00%ae%x00%at%x00%cn%x00%ce%x00%ct%x00%B",
        &hex,
    ])?;
    let text = out.stdout_str();
    let f: Vec<&str> = text.splitn(9, '\0').collect();
    let [_, parents, an, ae, at, cn, ce, ct, message] = f[..] else {
        bail!("unexpected output from git show");
    };
    let parents: Vec<Oid> = parents
        .split_whitespace()
        .filter_map(|p| Oid::from_hex(p.as_bytes()).ok())
        .collect();
    let base = parents
        .first()
        .map(ToString::to_string)
        .unwrap_or_else(|| EMPTY_TREE.to_owned());
    let files = load_files(git, &base, &hex)?;
    Ok(CommitDetails {
        id,
        parents,
        author: an.to_owned(),
        author_email: ae.to_owned(),
        author_time: at.trim().parse().unwrap_or(0),
        committer: cn.to_owned(),
        committer_email: ce.to_owned(),
        commit_time: ct.trim().parse().unwrap_or(0),
        message: message.trim_end().to_owned(),
        files,
    })
}

const EMPTY_TREE: &str = "4b825dc642cb6eb9a060e54bf8d69288fbee4904";

fn load_files(git: &Git, from: &str, to: &str) -> Result<Vec<FileChange>> {
    let common = [
        "-c",
        "core.quotepath=false",
        "diff-tree",
        "-r",
        "-z",
        "-M",
        "--no-commit-id",
    ];
    // Side by side: each git start costs tens of milliseconds on Windows.
    let (status, numstat) = std::thread::scope(|s| {
        let numstat = s.spawn(|| git.run(common.iter().copied().chain(["--numstat", from, to])));
        let status = git.run(common.iter().copied().chain(["--name-status", from, to]));
        (status, numstat.join().expect("numstat thread"))
    });
    let (status, numstat) = (status?, numstat?);
    let mut files = parse_name_status(&status.stdout);
    apply_numstat(&mut files, &numstat.stdout);
    Ok(files)
}

pub(crate) fn parse_name_status(bytes: &[u8]) -> Vec<FileChange> {
    let mut files = Vec::new();
    let mut fields = bytes
        .split(|&b| b == 0)
        .map(|f| String::from_utf8_lossy(f).into_owned());
    while let Some(code) = fields.next() {
        if code.is_empty() {
            continue;
        }
        let change = match code.as_bytes()[0] {
            b'A' => Change::Added,
            b'D' => Change::Deleted,
            b'R' => Change::Renamed,
            b'C' => Change::Copied,
            b'T' => Change::TypeChanged,
            b'U' => Change::Unmerged,
            _ => Change::Modified,
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
pub(crate) fn apply_numstat(files: &mut [FileChange], bytes: &[u8]) {
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

    #[test]
    fn summary_and_body() {
        let d = CommitDetails {
            id: Oid::null(gix::hash::Kind::Sha1),
            parents: vec![],
            author: String::new(),
            author_email: String::new(),
            author_time: 0,
            committer: String::new(),
            committer_email: String::new(),
            commit_time: 0,
            message: "Title\n\nBody line\nmore".into(),
            files: vec![],
        };
        assert_eq!(d.summary(), "Title");
        assert_eq!(d.body(), "Body line\nmore");
    }
}
