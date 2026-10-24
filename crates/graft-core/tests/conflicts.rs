//! Merging with conflicts and resolving them, against a real throwaway repository.

use std::path::Path;
use std::process::Command;

use graft_core::conflicts::{self, Kind, Resolution};
use graft_core::{Console, Git, refs, status};

fn sh(dir: &Path, args: &[&str]) {
    let out = Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .expect("git runs");
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// main and topic both change line 2 of f.txt; topic also deletes gone.txt
/// which main changes.
fn diverged_repo(dir: &Path) {
    sh(dir, &["init", "-q", "-b", "main"]);
    sh(dir, &["config", "user.name", "Ada"]);
    sh(dir, &["config", "user.email", "ada@example.com"]);
    sh(dir, &["config", "core.autocrlf", "false"]);
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
    let git = Git::new(dir, Console::default());

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
    let git = Git::new(dir, Console::default());
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
    let git = Git::new(dir, Console::default());

    assert!(refs::cherry_pick(&git, "topic").unwrap());
    let progress = conflicts::in_progress(dir).unwrap();
    assert_eq!(progress.kind, Kind::CherryPick);
    assert!(progress.title.contains("topic work"), "{}", progress.title);
    conflicts::abort(&git, Kind::CherryPick).unwrap();
    assert_eq!(conflicts::in_progress(dir), None);

    // A merge that has nothing to do is not an error.
    assert!(!conflicts::merge(&git, "main", false).unwrap());
    // An unknown branch is an error, not a conflict.
    assert!(conflicts::merge(&git, "nope", false).is_err());
}
