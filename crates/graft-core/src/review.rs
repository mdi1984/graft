//! Review: everything a worktree's branch changed since it forked from its
//! base branch, including uncommitted and untracked files. That is what a pull
//! request would show, plus the work that is not committed yet.

use std::io::Read;

use anyhow::{Context, Result};

use crate::cli::Git;
use crate::details::{FileChange, apply_numstat, parse_name_status};
use crate::model::Oid;
use crate::status::Change;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReviewFile {
    pub change: FileChange,
    pub untracked: bool,
    /// Hash of the file's current content (empty when deleted). A "Viewed"
    /// mark is tied to it, so it goes away when the file changes again.
    pub content: String,
}

impl ReviewFile {
    /// What a "Viewed" mark is stored under.
    pub fn viewed_key(&self) -> String {
        format!("{}:{}", self.change.path, self.content)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Review {
    pub base: String,
    pub merge_base: Oid,
    /// Commits on the branch since the merge-base.
    pub commits: usize,
    pub files: Vec<ReviewFile>,
}

impl Review {
    pub fn added(&self) -> usize {
        self.files.iter().filter_map(|f| f.change.added).sum()
    }

    pub fn removed(&self) -> usize {
        self.files.iter().filter_map(|f| f.change.removed).sum()
    }
}

/// The branch to review against when the user has not picked one: the local
/// copy of the remote's default branch (origin/HEAD), else main, master,
/// trunk or develop, else origin/HEAD itself.
pub fn default_base(git: &Git, branches: &[String], current: Option<&str>) -> Option<String> {
    // Exit code 1 just means there is no origin/HEAD.
    let remote_head = git
        .run_expecting(
            ["symbolic-ref", "-q", "--short", "refs/remotes/origin/HEAD"],
            None,
            |o| o.status <= 1,
        )
        .ok()
        .filter(|out| out.success())
        .map(|out| out.stdout_str().trim().to_owned())
        .filter(|head| !head.is_empty());
    let remote_default = remote_head
        .as_deref()
        .and_then(|h| h.strip_prefix("origin/"));
    for name in remote_default
        .into_iter()
        .chain(["main", "master", "trunk", "develop"])
    {
        if branches.iter().any(|b| b == name) && current != Some(name) {
            return Some(name.to_owned());
        }
    }
    remote_head
}

/// Untracked files bigger than this are not read to count their lines.
const MAX_COUNTED: u64 = 8 * 1024 * 1024;

/// `git` points at the worktree under review.
pub fn load(git: &Git, base: &str) -> Result<Review> {
    let merge_base = git
        .read(["merge-base", "--end-of-options", base, "HEAD"])
        .with_context(|| format!("{base} and this branch have no common history"))?;
    let merge_base = merge_base.trim().to_owned();
    // The rest runs side by side: each git start costs tens of milliseconds
    // on Windows.
    let common = ["-c", "core.quotepath=false", "diff", "-z", "-M"];
    let (commits, status, numstat, untracked) = std::thread::scope(|s| {
        let range = format!("{merge_base}..HEAD");
        let commits = s.spawn(move || git.read(["rev-list", "--count", &range]));
        let numstat = s.spawn(|| git.run(common.iter().copied().chain(["--numstat", &merge_base])));
        let untracked = s.spawn(|| {
            git.run([
                "-c",
                "core.quotepath=false",
                "ls-files",
                "--others",
                "--exclude-standard",
                "-z",
            ])
        });
        let status = git.run(common.iter().copied().chain(["--name-status", &merge_base]));
        (
            commits.join().expect("rev-list thread"),
            status,
            numstat.join().expect("numstat thread"),
            untracked.join().expect("ls-files thread"),
        )
    });
    let commits = commits?.trim().parse().unwrap_or(0);
    let (status, numstat, untracked) = (status?, numstat?, untracked?);
    let mut changes = parse_name_status(&status.stdout);
    apply_numstat(&mut changes, &numstat.stdout);
    let mut files: Vec<ReviewFile> = changes
        .into_iter()
        .map(|change| ReviewFile {
            change,
            untracked: false,
            content: String::new(),
        })
        .collect();

    for path in untracked
        .stdout
        .split(|b| *b == 0)
        .filter(|p| !p.is_empty())
    {
        let path = String::from_utf8_lossy(path).into_owned();
        // Only the start of a huge file (a stray dump or build output): its
        // line count is then left out.
        let mut bytes = Vec::new();
        let complete = std::fs::File::open(git.cwd().join(&path))
            .and_then(|f| f.take(MAX_COUNTED + 1).read_to_end(&mut bytes))
            .is_ok_and(|n| n as u64 <= MAX_COUNTED);
        let binary = bytes.iter().take(8000).any(|b| *b == 0);
        let lines = bytes
            .split(|b| *b == b'\n')
            .filter(|l| !l.is_empty())
            .count();
        let counted = complete && !binary;
        files.push(ReviewFile {
            change: FileChange {
                path,
                orig_path: None,
                change: Change::Added,
                added: counted.then_some(lines),
                removed: counted.then_some(0),
            },
            untracked: true,
            content: String::new(),
        });
    }
    files.sort_by(|a, b| a.change.path.cmp(&b.change.path));

    // Content hashes of everything that still exists, in one call.
    let present: Vec<usize> = (0..files.len())
        .filter(|&i| files[i].change.change != Change::Deleted)
        .collect();
    if !present.is_empty() {
        let mut input = Vec::new();
        for &i in &present {
            input.extend_from_slice(files[i].change.path.as_bytes());
            input.push(b'\n');
        }
        let out = git.run_with_input(["hash-object", "--stdin-paths"], Some(&input))?;
        for (&i, hash) in present.iter().zip(out.stdout_str().lines()) {
            files[i].content = hash.trim().to_owned();
        }
    }

    Ok(Review {
        base: base.to_owned(),
        merge_base: Oid::from_hex(merge_base.as_bytes())
            .with_context(|| format!("unexpected merge-base {merge_base}"))?,
        commits,
        files,
    })
}
