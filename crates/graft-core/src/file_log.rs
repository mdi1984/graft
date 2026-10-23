//! The history of one file (following renames) and who last changed each of
//! its lines (blame).

use std::collections::HashMap;

use anyhow::{Context, Result};

use crate::cli::Git;
use crate::model::Oid;

/// A commit that changed the file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileCommit {
    pub id: Oid,
    pub author: String,
    /// Author time, seconds since the Unix epoch.
    pub time: i64,
    pub summary: String,
    /// The file's path in this commit (it may have been renamed since).
    pub path: String,
}

/// The commits that changed `path`, newest first, up to `limit`.
pub fn log(git: &Git, path: &str, limit: usize) -> Result<Vec<FileCommit>> {
    let limit = format!("-n{limit}");
    let out = git.read([
        "-c",
        "core.quotepath=false",
        "log",
        "--follow",
        "--no-color",
        "--format=%x1e%H%x00%an%x00%at%x00%s",
        "--name-status",
        &limit,
        "--",
        path,
    ])?;
    let mut commits = Vec::new();
    for record in out.split('\x1e').filter(|r| !r.trim().is_empty()) {
        let mut lines = record.lines();
        let header = lines.next().unwrap_or_default();
        let mut fields = header.split('\0');
        let (Some(id), Some(author), Some(time), Some(summary)) =
            (fields.next(), fields.next(), fields.next(), fields.next())
        else {
            continue;
        };
        // "M\tpath", "R087\told\tnew": the last field is the path here.
        let here = lines
            .filter(|l| !l.is_empty())
            .filter_map(|l| l.rsplit('\t').next())
            .next()
            .unwrap_or(path);
        commits.push(FileCommit {
            id: Oid::from_hex(id.as_bytes()).context("bad commit id from git log")?,
            author: author.to_owned(),
            time: time.parse().unwrap_or(0),
            summary: summary.to_owned(),
            path: here.to_owned(),
        });
    }
    Ok(commits)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BlameCommit {
    pub author: String,
    pub time: i64,
    pub summary: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BlameLine {
    /// The commit that last changed this line.
    pub commit: Oid,
    pub text: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Blame {
    pub lines: Vec<BlameLine>,
    pub commits: HashMap<Oid, BlameCommit>,
    pub binary: bool,
}

/// Who last changed each line of `path` as of commit `at`.
pub fn blame(git: &Git, at: Oid, path: &str) -> Result<Blame> {
    let out = git.run(["blame", "--porcelain", &at.to_string(), "--", path])?;
    Ok(parse_blame(&out.stdout))
}

fn parse_blame(bytes: &[u8]) -> Blame {
    let mut blame = Blame::default();
    let mut current: Option<Oid> = None;
    let mut pending: Option<(Oid, BlameCommit)> = None;
    for raw in bytes.split(|&b| b == b'\n') {
        if let Some(content) = raw.strip_prefix(b"\t") {
            if content.contains(&0) {
                blame.binary = true;
            }
            if let Some((id, info)) = pending.take() {
                blame.commits.entry(id).or_insert(info);
            }
            if let Some(commit) = current {
                let text = String::from_utf8_lossy(content);
                blame.lines.push(BlameLine {
                    commit,
                    text: text.trim_end_matches('\r').to_owned(),
                });
            }
            continue;
        }
        let line = String::from_utf8_lossy(raw);
        let (key, value) = line.split_once(' ').unwrap_or((&line, ""));
        if key.len() == 40
            && key.bytes().all(|b| b.is_ascii_hexdigit())
            && let Ok(id) = Oid::from_hex(key.as_bytes())
        {
            current = Some(id);
            if !blame.commits.contains_key(&id) {
                pending = Some((
                    id,
                    BlameCommit {
                        author: String::new(),
                        time: 0,
                        summary: String::new(),
                    },
                ));
            }
            continue;
        }
        if let Some((_, info)) = &mut pending {
            match key {
                "author" => info.author = value.to_owned(),
                "author-time" => info.time = value.parse().unwrap_or(0),
                "summary" => info.summary = value.to_owned(),
                _ => {}
            }
        }
    }
    blame
}
