//! Review: everything a worktree's branch changed since it forked from its
//! base branch, including uncommitted and untracked files. That is what a pull
//! request would show, plus the work that is not committed yet.

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

/// `git` points at the worktree under review.
pub fn load(git: &Git, base: &str) -> Result<Review> {
    let merge_base = git
        .read(["merge-base", "--end-of-options", base, "HEAD"])
        .with_context(|| format!("{base} and this branch have no common history"))?;
    let merge_base = merge_base.trim().to_owned();
    let commits = git
        .read(["rev-list", "--count", &format!("{merge_base}..HEAD")])?
        .trim()
        .parse()
        .unwrap_or(0);

    let common = ["-c", "core.quotepath=false", "diff", "-z", "-M"];
    let status = git.run(common.iter().copied().chain(["--name-status", &merge_base]))?;
    let numstat = git.run(common.iter().copied().chain(["--numstat", &merge_base]))?;
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

    let untracked = git.run([
        "-c",
        "core.quotepath=false",
        "ls-files",
        "--others",
        "--exclude-standard",
        "-z",
    ])?;
    for path in untracked
        .stdout
        .split(|b| *b == 0)
        .filter(|p| !p.is_empty())
    {
        let path = String::from_utf8_lossy(path).into_owned();
        let bytes = std::fs::read(git.cwd().join(&path)).unwrap_or_default();
        let binary = bytes.iter().take(8000).any(|b| *b == 0);
        let lines = bytes
            .split(|b| *b == b'\n')
            .filter(|l| !l.is_empty())
            .count();
        files.push(ReviewFile {
            change: FileChange {
                path,
                orig_path: None,
                change: Change::Added,
                added: (!binary).then_some(lines),
                removed: (!binary).then_some(0),
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
