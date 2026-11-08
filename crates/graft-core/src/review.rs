//! Review: everything a worktree's branch changed since it forked from its
//! base branch, including uncommitted and untracked files. That is what a pull
//! request would show, plus the work that is not committed yet.

use std::collections::HashSet;
use std::io::Read;

use anyhow::{Context, Result, bail};

use crate::changes::{FileChange, between, count_lines, is_binary};
use crate::cli::Git;
use crate::model::{Change, ORIGIN, Oid, RepoSnapshot};
use crate::paths::display;
use crate::repo::in_progress;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReviewFile {
    pub change: FileChange,
    pub untracked: bool,
    /// Its worktree version is not committed: changed since HEAD (staged
    /// or not), or untracked.
    pub uncommitted: bool,
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
    /// The worktree's HEAD it was loaded for: Merge merges only that.
    pub head: Oid,
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

/// The branch to review `current` against when the user has not picked
/// one: the first default branch candidate that is not `current` itself,
/// else origin/HEAD.
pub fn default_base(snap: &RepoSnapshot, current: Option<&str>) -> Option<String> {
    snap.default_branches()
        .find(|name| current != Some(*name))
        .map(str::to_owned)
        .or_else(|| {
            snap.remote_default
                .as_ref()
                .map(|branch| format!("{ORIGIN}/{branch}"))
        })
}

/// Untracked files bigger than this are not read to count their lines.
const MAX_COUNTED: u64 = 8 * 1024 * 1024;

/// `git` points at the worktree under review.
pub fn load(git: &Git, base: &str) -> Result<Review> {
    let head = git.rev("HEAD")?.context("This branch has no commits yet")?;
    let head_hex = head.to_string();
    let merge_base = git
        .read(["merge-base", "--end-of-options", base, &head_hex])
        .with_context(|| format!("{base} and this branch have no common history"))?;
    let merge_base = merge_base.trim().to_owned();
    // The rest runs side by side: each git start costs tens of milliseconds
    // on Windows.
    let (commits, changes, untracked, uncommitted) = std::thread::scope(|s| {
        let range = format!("{merge_base}..{head_hex}");
        let commits = s.spawn(move || git.read(["rev-list", "--count", &range]));
        let uncommitted = s.spawn(|| git.run(["diff", "--name-only", "-z", "HEAD"]));
        let untracked = s.spawn(|| git.run(["ls-files", "--others", "--exclude-standard", "-z"]));
        let changes = between(git, &merge_base, None);
        (
            commits.join().expect("rev-list thread"),
            changes,
            untracked.join().expect("ls-files thread"),
            uncommitted.join().expect("diff thread"),
        )
    });
    let commits = commits?.trim().parse().unwrap_or(0);
    let uncommitted = uncommitted?;
    let uncommitted: HashSet<&[u8]> = uncommitted
        .stdout
        .split(|b| *b == 0)
        .filter(|p| !p.is_empty())
        .collect();
    let mut files: Vec<ReviewFile> = changes?
        .into_iter()
        .map(|change| ReviewFile {
            uncommitted: uncommitted.contains(change.path.as_bytes()),
            change,
            untracked: false,
            content: String::new(),
        })
        .collect();

    // A trailing `/` is an untracked repository inside the worktree: a
    // folder, not a file to show or hash.
    for path in untracked?
        .stdout
        .split(|b| *b == 0)
        .filter(|p| !p.is_empty() && !p.ends_with(b"/"))
    {
        let path = String::from_utf8_lossy(path).into_owned();
        // Only the start of a huge file (a stray dump or build output): its
        // line count is then left out.
        let mut bytes = Vec::new();
        let complete = std::fs::File::open(git.cwd().join(&path))
            .and_then(|f| f.take(MAX_COUNTED + 1).read_to_end(&mut bytes))
            .is_ok_and(|n| n as u64 <= MAX_COUNTED);
        let lines = count_lines(&bytes);
        let counted = complete && !is_binary(&bytes);
        files.push(ReviewFile {
            change: FileChange {
                path,
                orig_path: None,
                change: Change::Added,
                added: counted.then_some(lines),
                removed: counted.then_some(0),
            },
            untracked: true,
            uncommitted: true,
            content: String::new(),
        });
    }
    files.sort_by(|a, b| a.change.path.cmp(&b.change.path));

    // Content hashes of every file that still exists, in one call. A
    // folder (a submodule) has no content to hash; its mark stays keyed by
    // path alone.
    let present: Vec<usize> = (0..files.len())
        .filter(|&i| {
            files[i].change.change != Change::Deleted
                && !git.cwd().join(&files[i].change.path).is_dir()
        })
        .collect();
    if !present.is_empty() {
        let mut input = Vec::new();
        for &i in &present {
            input.extend_from_slice(files[i].change.path.as_bytes());
            input.push(b'\n');
        }
        let out = git.run_input(["hash-object", "--stdin-paths"], &input)?;
        for (&i, hash) in present.iter().zip(out.stdout_str().lines()) {
            files[i].content = hash.trim().to_owned();
        }
    }

    Ok(Review {
        base: base.to_owned(),
        merge_base: Oid::from_hex(merge_base.as_bytes())
            .with_context(|| format!("unexpected merge-base {merge_base}"))?,
        head,
        commits,
        files,
    })
}

/// Merge the reviewed `branch` into `into`, if `into` is still checked out
/// where `git` points with nothing in progress there (it may have switched
/// while the user was asked), and `branch` is still at the reviewed `head`:
/// commits made after the review are never merged unseen. Ok(true):
/// stopped with conflicts.
pub fn merge(git: &Git, branch: &str, into: &str, head: Oid) -> Result<bool> {
    let place = display(git.cwd());
    if let Some(op) = in_progress(git.cwd()) {
        bail!(
            "Nothing was merged: a {} is in progress in {place}. Finish or abort it, then              merge again.",
            op.kind.noun().to_lowercase()
        );
    }
    if git.current_branch()?.as_deref() != Some(into) {
        bail!(
            "Nothing was merged: {into} is no longer checked out in {place}. Check it out              there, then merge again."
        );
    }
    if git.rev(&format!("refs/heads/{branch}"))? != Some(head) {
        bail!(
            "Nothing was merged: {branch} moved since you reviewed it. Look at its new              commits in Review, then merge again."
        );
    }
    crate::conflicts::merge(git, branch, false)
}
