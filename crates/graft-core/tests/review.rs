//! Review against the merge-base, with uncommitted and untracked files.

mod common;

use common::sh;
use graft_core::diff::{self, Source};
use graft_core::{Change, review};

#[test]
fn branch_changes_since_fork_including_uncommitted() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = &common::init(&tmp.path().join("r"));
    std::fs::write(dir.join("a.txt"), "one\ntwo\n").unwrap();
    std::fs::write(dir.join("c.txt"), "keep\n").unwrap();
    std::fs::write(dir.join("gone.txt"), "bye\n").unwrap();
    sh(dir, &["add", "."]);
    sh(dir, &["commit", "-q", "-m", "base"]);
    sh(dir, &["switch", "-q", "-c", "feature"]);
    std::fs::write(dir.join("a.txt"), "one\nTWO\nthree\n").unwrap();
    std::fs::write(dir.join("b.txt"), "new\n").unwrap();
    sh(dir, &["rm", "-q", "gone.txt"]);
    sh(dir, &["add", "."]);
    sh(dir, &["commit", "-q", "-m", "feature work"]);
    // main moves on; that must not show up in the review.
    sh(dir, &["switch", "-q", "main"]);
    std::fs::write(dir.join("main-only.txt"), "x\n").unwrap();
    sh(dir, &["add", "."]);
    sh(dir, &["commit", "-q", "-m", "main work"]);
    sh(dir, &["switch", "-q", "feature"]);
    // Uncommitted and untracked work.
    std::fs::write(dir.join("c.txt"), "keep\nmore\n").unwrap();
    std::fs::write(dir.join("d.txt"), "draft\nnotes\n").unwrap();

    let git = common::git(dir);
    let r = review::load(&git, "main").unwrap();
    assert_eq!(r.commits, 1);
    let paths: Vec<&str> = r.files.iter().map(|f| f.change.path.as_str()).collect();
    assert_eq!(paths, ["a.txt", "b.txt", "c.txt", "d.txt", "gone.txt"]);
    let get = |p: &str| r.files.iter().find(|f| f.change.path == p).unwrap();
    assert_eq!(get("a.txt").change.added, Some(2));
    assert_eq!(get("a.txt").change.removed, Some(1));
    assert_eq!(get("b.txt").change.change, Change::Added);
    assert_eq!(get("gone.txt").change.change, Change::Deleted);
    assert!(get("gone.txt").content.is_empty());
    assert!(get("d.txt").untracked);
    assert!(get("d.txt").uncommitted && get("c.txt").uncommitted);
    assert!(!get("a.txt").uncommitted, "committed on the branch");
    assert_eq!(get("d.txt").change.added, Some(2));
    assert_eq!(r.added(), 2 + 1 + 1 + 2);

    // The diff of a file goes from the merge-base to the working tree.
    let d = diff::load(&git, &Source::Base(r.merge_base), "c.txt").unwrap();
    assert_eq!(d.added(), 1);

    // Viewed marks follow the content.
    let before = get("a.txt").viewed_key();
    let again = review::load(&git, "main").unwrap();
    assert_eq!(again.files[0].viewed_key(), before);
    std::fs::write(dir.join("a.txt"), "changed again\n").unwrap();
    let after = review::load(&git, "main").unwrap();
    assert_ne!(after.files[0].viewed_key(), before);

    assert!(review::load(&git, "nope").is_err());
}

#[test]
fn review_base_follows_the_remote_default_branch() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = &common::init(&tmp.path().join("r"));
    sh(dir, &["commit", "-q", "--allow-empty", "-m", "base"]);
    sh(dir, &["branch", "development"]);
    sh(dir, &["switch", "-q", "-c", "feature"]);
    let git = common::git(dir);
    let base = |current: &str| {
        let snap = graft_core::snapshot::load(&git).unwrap();
        review::default_base(&snap, Some(current))
    };
    // No remote: the usual names.
    assert_eq!(base("feature").as_deref(), Some("main"));

    // As after cloning a repository whose default branch is development.
    sh(
        dir,
        &["update-ref", "refs/remotes/origin/development", "HEAD"],
    );
    sh(
        dir,
        &[
            "symbolic-ref",
            "refs/remotes/origin/HEAD",
            "refs/remotes/origin/development",
        ],
    );
    assert_eq!(base("feature").as_deref(), Some("development"));
    // Reviewing the default branch itself: the next candidate.
    assert_eq!(base("development").as_deref(), Some("main"));

    // Without a local copy of it, origin/HEAD itself.
    sh(dir, &["switch", "-q", "development"]);
    sh(dir, &["branch", "-q", "-D", "main", "feature"]);
    assert_eq!(base("development").as_deref(), Some("origin/development"));
}

/// Blank lines of an untracked file count as added lines, as git counts
/// them for a committed file.
#[test]
fn untracked_blank_lines_count() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = &common::init(&tmp.path().join("r"));
    common::commit(dir, "a.txt", "a\n", "base");
    sh(dir, &["switch", "-q", "-c", "feature"]);
    std::fs::write(dir.join("new.txt"), "one\n\ntwo\n\n").unwrap();
    common::commit(dir, "committed.txt", "one\n\ntwo\n\n", "same text");

    let r = review::load(&common::git(dir), "main").unwrap();
    let get = |p: &str| r.files.iter().find(|f| f.change.path == p).unwrap();
    assert_eq!(get("committed.txt").change.added, Some(4));
    assert_eq!(get("new.txt").change.added, Some(4));
}

/// A submodule (a folder in the list) and an untracked repository inside
/// the worktree do not break the review.
#[test]
fn folders_do_not_break_the_review() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = &common::init(&tmp.path().join("r"));
    common::commit(dir, "a.txt", "a\n", "base");
    sh(dir, &["switch", "-q", "-c", "feature"]);
    // A nested repository committed as a gitlink, like a submodule.
    let sub = common::init(&dir.join("sub"));
    common::commit(&sub, "s.txt", "s\n", "sub");
    sh(dir, &["add", "sub"]);
    sh(dir, &["commit", "-q", "-m", "add sub"]);
    // An untracked repository: git lists it as `nested/`.
    let nested = common::init(&dir.join("nested"));
    common::commit(&nested, "n.txt", "n\n", "nested");
    std::fs::write(dir.join("b.txt"), "b\n").unwrap();

    let r = review::load(&common::git(dir), "main").unwrap();
    let paths: Vec<&str> = r.files.iter().map(|f| f.change.path.as_str()).collect();
    assert_eq!(paths, ["b.txt", "sub"]);
    let b = r.files.iter().find(|f| f.change.path == "b.txt").unwrap();
    assert!(!b.content.is_empty(), "files still get their content hash");
}

/// Merge merges what was reviewed: once the branch moved on (an agent
/// committed after the review), nothing is merged until it is reviewed
/// again.
#[test]
fn merge_takes_only_the_reviewed_commits() {
    let tmp = tempfile::tempdir().unwrap();
    let main = &common::init(&tmp.path().join("r"));
    common::commit(main, "a.txt", "a\n", "base");
    let agent = tmp.path().join("agent");
    let agent_arg = agent.to_str().unwrap();
    sh(main, &["worktree", "add", "-q", "-b", "feature", agent_arg]);
    common::commit(&agent, "b.txt", "b\n", "reviewed");

    let r = review::load(&common::git(&agent), "main").unwrap();
    assert_eq!(r.head.to_string(), common::rev(&agent, "HEAD"));
    common::commit(&agent, "c.txt", "c\n", "after the review");
    let before = common::rev(main, "HEAD");
    let err = review::merge(&common::git(main), "feature", r.head).unwrap_err();
    assert!(
        format!("{err:#}").contains("moved since you reviewed it"),
        "{err:#}"
    );
    assert_eq!(common::rev(main, "HEAD"), before);

    let r = review::load(&common::git(&agent), "main").unwrap();
    let stopped = review::merge(&common::git(main), "feature", r.head).unwrap();
    assert!(!stopped);
    assert_eq!(common::rev(main, "HEAD"), common::rev(&agent, "HEAD"));
}
