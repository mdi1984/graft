//! Branches, tags and commit operations against a real throwaway repository.

mod common;

use common::{commit, rev, sh};
use graft_core::refs::{self, ResetMode};
use graft_core::snapshot;

#[test]
fn branches_tags_and_commit_ops() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = common::init(&tmp.path().join("r"));
    let git = common::git(&dir);
    commit(&dir, "a.txt", "a\n", "one");
    commit(&dir, "a.txt", "a\nb\n", "two");
    let branches = || -> Vec<String> {
        let snap = snapshot::load(&git).unwrap();
        snap.branches.into_iter().map(|b| b.name).collect()
    };

    assert!(refs::check_branch_name(&git, "bad name").is_err());
    assert!(refs::check_branch_name(&git, "bad..name").is_err());
    assert!(refs::create_branch(&git, "bad..name", "HEAD", false).is_err());
    assert_eq!(branches(), ["main"]);

    // Created at HEAD~1 and checked out.
    refs::create_branch(&git, "topic", "HEAD~1", true).unwrap();
    assert_eq!(sh(&dir, &["symbolic-ref", "--short", "HEAD"]), "topic");
    assert_eq!(rev(&dir, "topic"), rev(&dir, "main~1"));
    commit(&dir, "t.txt", "t\n", "topic work");
    refs::switch(&git, "main").unwrap();
    assert_eq!(sh(&dir, &["symbolic-ref", "--short", "HEAD"]), "main");

    // Not merged: plain delete refuses with a recognisable message.
    let err = format!(
        "{:#}",
        refs::delete_branch(&git, "topic", false).unwrap_err()
    );
    assert!(err.contains(refs::NOT_MERGED), "{err}");
    let topic = rev(&dir, "topic");
    refs::rename_branch(&git, "topic", "topic2").unwrap();
    assert_eq!(branches(), ["main", "topic2"]);
    assert_eq!(rev(&dir, "topic2"), topic);

    // Cherry-pick brings the commit's change; revert takes it away again,
    // both as new commits on main.
    assert!(!refs::cherry_pick(&git, "topic2").unwrap());
    assert!(dir.join("t.txt").exists());
    assert_eq!(sh(&dir, &["log", "-1", "--format=%s"]), "topic work");
    assert!(!refs::revert(&git, "HEAD").unwrap());
    assert!(!dir.join("t.txt").exists());
    assert_eq!(sh(&dir, &["rev-list", "--count", "HEAD"]), "4");
    refs::delete_branch(&git, "topic2", true).unwrap();
    assert_eq!(branches(), ["main"]);

    refs::create_tag(&git, "v1", "HEAD", Some("Release 1")).unwrap();
    refs::create_tag(&git, "light", "HEAD~1", None).unwrap();
    assert!(refs::create_tag(&git, "bad tag", "HEAD", None).is_err());
    let snap = snapshot::load(&git).unwrap();
    let tag = |name: &str| snap.tags.iter().find(|t| t.name == name).unwrap();
    assert!(tag("v1").annotated);
    assert_eq!(tag("v1").target.to_string(), rev(&dir, "HEAD"));
    assert!(!tag("light").annotated);
    assert_eq!(tag("light").target.to_string(), rev(&dir, "HEAD~1"));
    refs::delete_tag(&git, "light").unwrap();
    let names: Vec<String> = snapshot::load(&git)
        .unwrap()
        .tags
        .into_iter()
        .map(|t| t.name)
        .collect();
    assert_eq!(names, ["v1"]);

    // Undo: the commit goes, its changes stay staged.
    let before_undo = rev(&dir, "HEAD~1");
    refs::undo_last_commit(&git).unwrap();
    assert_eq!(rev(&dir, "HEAD"), before_undo);
    assert_eq!(sh(&dir, &["diff", "--cached", "--name-only"]), "t.txt");

    // Hard reset throws away staged and unstaged changes.
    std::fs::write(dir.join("a.txt"), "edited\n").unwrap();
    refs::reset(&git, "HEAD", ResetMode::Hard).unwrap();
    assert_eq!(sh(&dir, &["status", "--porcelain"]), "");
    assert_eq!(rev(&dir, "HEAD"), before_undo);

    // Detached at the tag's commit, no branch checked out.
    refs::switch_detached(&git, "v1").unwrap();
    assert_eq!(rev(&dir, "HEAD"), rev(&dir, "v1^{commit}"));
    let main = &snapshot::load(&git).unwrap().worktrees[0];
    assert_eq!(main.branch, None, "HEAD is detached");
}

#[test]
fn the_first_commit_cannot_be_undone() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = common::init(&tmp.path().join("r"));
    commit(&dir, "a.txt", "a\n", "only");
    let git = common::git(&dir);
    assert!(refs::undo_last_commit(&git).is_err());
    assert_eq!(sh(&dir, &["rev-list", "--count", "HEAD"]), "1");
}
