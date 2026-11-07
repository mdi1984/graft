//! Merging with conflicts and resolving them, against a real throwaway repository.

mod common;

use std::path::Path;

use common::sh;
use graft_core::conflicts::{self, Kind, Resolution};
use graft_core::{refs, status};

/// main and topic both change line 2 of f.txt; topic also deletes gone.txt
/// which main changes.
fn diverged_repo(dir: &Path) {
    common::init(dir);
    std::fs::write(dir.join("f.txt"), "a\nb\nc\n").unwrap();
    std::fs::write(dir.join("gone.txt"), "x\n").unwrap();
    std::fs::write(dir.join("lock.json"), "{\"v\": 1}\n").unwrap();
    sh(dir, &["add", "."]);
    sh(dir, &["commit", "-q", "-m", "base"]);
    sh(dir, &["switch", "-q", "-c", "topic"]);
    std::fs::write(dir.join("f.txt"), "a\nB-topic\nc\n").unwrap();
    std::fs::write(dir.join("lock.json"), "{\"v\": 3}\n").unwrap();
    sh(dir, &["rm", "-q", "gone.txt"]);
    sh(dir, &["commit", "-q", "-am", "topic work"]);
    sh(dir, &["switch", "-q", "main"]);
    std::fs::write(dir.join("f.txt"), "a\nB-main\nc\n").unwrap();
    std::fs::write(dir.join("gone.txt"), "y\n").unwrap();
    std::fs::write(dir.join("lock.json"), "{\"v\": 2}\n").unwrap();
    sh(dir, &["commit", "-q", "-am", "main work"]);
}

#[test]
fn merge_resolve_and_continue() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    diverged_repo(dir);
    let git = common::git(dir);

    assert_eq!(conflicts::in_progress(dir), None);
    assert!(conflicts::merge(&git, "topic", false).unwrap());
    let progress = conflicts::in_progress(dir).expect("merge in progress");
    assert_eq!(progress.kind, Kind::Merge);
    assert!(progress.title.contains("topic"), "{}", progress.title);
    let st = status::load(&git).unwrap();
    assert_eq!(st.conflicted().count(), 3);

    // A text conflict: resolve with both sides.
    let file = conflicts::load(&git, "f.txt").unwrap();
    assert!(!file.binary);
    assert_eq!(file.conflicts(), 1);
    let content = conflicts::assemble(&file.segments, &[Some(Resolution::OursThenTheirs)]).unwrap();
    assert_eq!(content, b"a\nB-main\nB-topic\nc\n");
    conflicts::resolve_with(&git, &file, &content).unwrap();
    assert_eq!(
        std::fs::read(dir.join("f.txt")).unwrap(),
        b"a\nB-main\nB-topic\nc\n"
    );

    // Deleted on their side: taking theirs deletes it.
    let gone = conflicts::load(&git, "gone.txt").unwrap();
    assert!(gone.theirs.is_none() && gone.ours.is_some());
    assert!(gone.segments.is_empty());
    conflicts::take_side(&git, &gone, false).unwrap();
    assert!(!dir.join("gone.txt").exists());

    // Whole file: ours.
    let lock = conflicts::load(&git, "lock.json").unwrap();
    conflicts::take_side(&git, &lock, true).unwrap();
    assert_eq!(
        std::fs::read_to_string(dir.join("lock.json")).unwrap(),
        "{\"v\": 2}\n"
    );

    assert_eq!(status::load(&git).unwrap().conflicted().count(), 0);
    assert!(!conflicts::continue_op(&git, Kind::Merge).unwrap());
    assert_eq!(conflicts::in_progress(dir), None);
    let parents = git.read(["rev-list", "--parents", "-n1", "HEAD"]).unwrap();
    assert_eq!(parents.split_whitespace().count(), 3, "a merge commit");
}

#[test]
fn markers_block_mark_resolved_and_abort_restores() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    diverged_repo(dir);
    let git = common::git(dir);
    let before = git.read(["rev-parse", "HEAD"]).unwrap();

    assert!(conflicts::merge(&git, "topic", true).unwrap());
    let err = conflicts::mark_resolved(&git, "f.txt").unwrap_err();
    assert!(err.to_string().contains("conflict markers"), "{err}");
    std::fs::write(dir.join("f.txt"), "a\nhand\nc\n").unwrap();
    conflicts::mark_resolved(&git, "f.txt").unwrap();

    conflicts::abort(&git, Kind::Merge).unwrap();
    assert_eq!(conflicts::in_progress(dir), None);
    assert_eq!(git.read(["rev-parse", "HEAD"]).unwrap(), before);
    assert_eq!(
        std::fs::read_to_string(dir.join("f.txt")).unwrap(),
        "a\nB-main\nc\n"
    );
}

#[test]
fn cherry_pick_stops_on_conflict() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    diverged_repo(dir);
    let git = common::git(dir);

    assert!(refs::cherry_pick(&git, "topic").unwrap());
    let progress = conflicts::in_progress(dir).unwrap();
    assert_eq!(progress.kind, Kind::CherryPick);
    assert_eq!(progress.title, "Cherry-picking: topic work");
    conflicts::abort(&git, Kind::CherryPick).unwrap();
    assert_eq!(conflicts::in_progress(dir), None);

    // A merge that has nothing to do is not an error.
    assert!(!conflicts::merge(&git, "main", false).unwrap());
    // An unknown branch is an error, not a conflict.
    assert!(conflicts::merge(&git, "nope", false).is_err());
}

/// A rebase stops at the conflicting commit; once resolved, continue
/// finishes it and the branch sits on top of the other one.
#[test]
fn rebase_stops_resolve_and_continue() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    diverged_repo(dir);
    let git = common::git(dir);
    sh(dir, &["switch", "-q", "topic"]);
    let main = common::rev(dir, "main");

    assert!(conflicts::rebase(&git, "main").unwrap());
    let progress = conflicts::in_progress(dir).expect("rebase in progress");
    assert_eq!(progress.kind, Kind::Rebase);
    assert_eq!(progress.title, "Rebasing topic");
    // Continuing with conflicts left is refused and the rebase stays.
    assert!(conflicts::continue_op(&git, Kind::Rebase).is_err());
    assert!(conflicts::in_progress(dir).is_some());

    for path in ["f.txt", "gone.txt", "lock.json"] {
        let file = conflicts::load(&git, path).unwrap();
        // In a rebase "theirs" is the commit being replayed (topic's).
        conflicts::take_side(&git, &file, false).unwrap();
    }
    assert!(!conflicts::continue_op(&git, Kind::Rebase).unwrap());
    assert_eq!(conflicts::in_progress(dir), None);
    assert_eq!(sh(dir, &["symbolic-ref", "--short", "HEAD"]), "topic");
    assert_eq!(common::rev(dir, "HEAD~1"), main);
    assert_eq!(
        std::fs::read_to_string(dir.join("f.txt")).unwrap(),
        "a\nB-topic\nc\n"
    );
    assert!(!dir.join("gone.txt").exists());
}

#[test]
fn rebase_abort_puts_the_branch_back() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    diverged_repo(dir);
    let git = common::git(dir);
    sh(dir, &["switch", "-q", "topic"]);
    let before = common::rev(dir, "HEAD");

    assert!(conflicts::rebase(&git, "main").unwrap());
    conflicts::abort(&git, Kind::Rebase).unwrap();
    assert_eq!(conflicts::in_progress(dir), None);
    assert_eq!(sh(dir, &["symbolic-ref", "--short", "HEAD"]), "topic");
    assert_eq!(common::rev(dir, "HEAD"), before);
    assert_eq!(sh(dir, &["status", "--porcelain"]), "");
}
