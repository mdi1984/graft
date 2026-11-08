//! Merging with conflicts and resolving them, against a real throwaway repository.

mod common;

use std::path::Path;

use common::sh;
use graft_core::conflicts::{self, Resolution};
use graft_core::{Operation, repo, status};

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

    assert_eq!(repo::in_progress(dir), None);
    assert!(conflicts::merge(&git, "topic", false).unwrap());
    let progress = repo::in_progress(dir).expect("merge in progress");
    assert_eq!(progress.kind, Operation::Merge);
    assert!(progress.title.contains("topic"), "{}", progress.title);
    let st = status::load(&git).unwrap();
    assert_eq!(st.conflicted().count(), 3);

    // A text conflict: resolve with both sides.
    let file = conflicts::load(&git, "f.txt").unwrap();
    assert!(!file.binary);
    assert_eq!(file.conflicts(), 1);
    // Saving notices a hand edit made after loading.
    assert!(!conflicts::edited_since_load(&git, &file));
    let loaded = std::fs::read(dir.join("f.txt")).unwrap();
    std::fs::write(dir.join("f.txt"), "fixed by hand\n").unwrap();
    assert!(conflicts::edited_since_load(&git, &file));
    std::fs::write(dir.join("f.txt"), loaded).unwrap();
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
    // Continue never opens the user's editor (one that fails here).
    sh(dir, &["config", "core.editor", "false"]);
    assert!(!conflicts::continue_op(&git, Operation::Merge).unwrap());
    assert_eq!(repo::in_progress(dir), None);
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

    conflicts::abort(&git, Operation::Merge).unwrap();
    assert_eq!(repo::in_progress(dir), None);
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

    assert!(conflicts::cherry_pick(&git, "topic").unwrap());
    let progress = repo::in_progress(dir).unwrap();
    assert_eq!(progress.kind, Operation::CherryPick);
    assert_eq!(progress.title, "Cherry-picking: topic work");
    conflicts::abort(&git, Operation::CherryPick).unwrap();
    assert_eq!(repo::in_progress(dir), None);

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
    let progress = repo::in_progress(dir).expect("rebase in progress");
    assert_eq!(progress.kind, Operation::Rebase);
    assert_eq!(progress.title, "Rebasing topic");
    // Continuing with conflicts left is refused and the rebase stays.
    assert!(conflicts::continue_op(&git, Operation::Rebase).is_err());
    assert!(repo::in_progress(dir).is_some());

    for path in ["f.txt", "gone.txt", "lock.json"] {
        let file = conflicts::load(&git, path).unwrap();
        // In a rebase "theirs" is the commit being replayed (topic's).
        conflicts::take_side(&git, &file, false).unwrap();
    }
    assert!(!conflicts::continue_op(&git, Operation::Rebase).unwrap());
    assert_eq!(repo::in_progress(dir), None);
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
    conflicts::abort(&git, Operation::Rebase).unwrap();
    assert_eq!(repo::in_progress(dir), None);
    assert_eq!(sh(dir, &["symbolic-ref", "--short", "HEAD"]), "topic");
    assert_eq!(common::rev(dir, "HEAD"), before);
    assert_eq!(sh(dir, &["status", "--porcelain"]), "");
}

#[test]
fn markdown_underlines_are_content_not_markers() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    common::init(dir);
    std::fs::write(dir.join("doc.md"), "Title\nx\n").unwrap();
    sh(dir, &["add", "."]);
    sh(dir, &["commit", "-q", "-m", "base"]);
    sh(dir, &["switch", "-q", "-c", "topic"]);
    std::fs::write(dir.join("doc.md"), "Title\n----------\ntheirs\n").unwrap();
    sh(dir, &["commit", "-q", "-am", "topic"]);
    sh(dir, &["switch", "-q", "main"]);
    std::fs::write(dir.join("doc.md"), "Title\n==========\nours\n").unwrap();
    sh(dir, &["commit", "-q", "-am", "main"]);
    let git = common::git(dir);

    assert!(conflicts::merge(&git, "topic", false).unwrap());
    let file = conflicts::load(&git, "doc.md").unwrap();
    assert_eq!(file.conflicts(), 1);
    let theirs = conflicts::assemble(&file.segments, &[Some(Resolution::Theirs)]).unwrap();
    assert_eq!(theirs, b"Title\n----------\ntheirs\n");
    let ours = conflicts::assemble(&file.segments, &[Some(Resolution::Ours)]).unwrap();
    assert_eq!(ours, b"Title\n==========\nours\n");
}

/// A line exactly like git's usual separator (a Markdown underline of a
/// seven-letter heading) is content too, and "Use theirs" stages theirs.
#[test]
fn a_line_like_a_marker_is_content() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    common::init(dir);
    std::fs::write(dir.join("doc.md"), "base text\n").unwrap();
    sh(dir, &["add", "."]);
    sh(dir, &["commit", "-q", "-m", "base"]);
    sh(dir, &["switch", "-q", "-c", "topic"]);
    std::fs::write(dir.join("doc.md"), "theirs text\n").unwrap();
    sh(dir, &["commit", "-q", "-am", "topic"]);
    sh(dir, &["switch", "-q", "main"]);
    std::fs::write(dir.join("doc.md"), "Example\n=======\nours text\n").unwrap();
    sh(dir, &["commit", "-q", "-am", "main"]);
    let git = common::git(dir);

    assert!(conflicts::merge(&git, "topic", false).unwrap());
    let file = conflicts::load(&git, "doc.md").unwrap();
    assert_eq!(file.conflicts(), 1);
    let ours = conflicts::assemble(&file.segments, &[Some(Resolution::Ours)]).unwrap();
    assert_eq!(ours, b"Example\n=======\nours text\n");
    let theirs = conflicts::assemble(&file.segments, &[Some(Resolution::Theirs)]).unwrap();
    assert_eq!(theirs, b"theirs text\n");
    conflicts::resolve_with(&git, &file, &theirs).unwrap();
    assert_eq!(sh(dir, &["show", ":doc.md"]), "theirs text");

    // Marker lines that still reach a result are refused, never staged.
    let marker = format!("{} theirs\n", ">".repeat(41));
    let error = conflicts::resolve_with(&git, &file, marker.as_bytes()).unwrap_err();
    assert!(
        format!("{error:#}").contains("conflict markers"),
        "{error:#}"
    );
    assert_eq!(sh(dir, &["show", ":doc.md"]), "theirs text");

    // A line as long as the separator, but without a label (a banner, an
    // underline), is content.
    let banner = format!("Heading\n{}\n", "=".repeat(41));
    conflicts::resolve_with(&git, &file, banner.as_bytes()).unwrap();
    assert_eq!(
        sh(dir, &["show", ":doc.md"]),
        format!("Heading\n{}", "=".repeat(41))
    );
}

/// A branch passed by its full ref is named by its short name in the merge
/// commit and in conflict markers; with a tag named alike it stays full, so
/// git still takes the branch.
#[test]
fn merges_name_branches_by_their_short_name() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    diverged_repo(dir);
    let git = common::git(dir);
    sh(dir, &["switch", "-q", "-c", "clean", "topic~1"]);
    common::commit(dir, "new.txt", "n\n", "new");
    sh(dir, &["switch", "-q", "main"]);

    assert!(!conflicts::merge(&git, "refs/heads/clean", false).unwrap());
    assert_eq!(
        sh(dir, &["log", "-1", "--format=%s"]),
        "Merge branch 'clean'"
    );

    assert!(conflicts::merge(&git, "refs/heads/topic", false).unwrap());
    let f = std::fs::read_to_string(dir.join("f.txt")).unwrap();
    assert!(f.contains("\n>>>>>>> topic\n"), "{f}");
    assert_eq!(
        repo::in_progress(dir).unwrap().title,
        "Merge branch 'topic'"
    );
    conflicts::abort(&git, Operation::Merge).unwrap();

    sh(dir, &["tag", "topic", "main~1"]);
    assert!(conflicts::merge(&git, "refs/heads/topic", false).unwrap());
    let f = std::fs::read_to_string(dir.join("f.txt")).unwrap();
    assert!(f.contains("\n>>>>>>> refs/heads/topic\n"), "{f}");
}
