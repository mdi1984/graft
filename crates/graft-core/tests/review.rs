//! Review against the merge-base, with uncommitted and untracked files.

use std::path::Path;
use std::process::Command;

use graft_core::diff::{self, Source};
use graft_core::status::Change;
use graft_core::{Console, Git, review};

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

#[test]
fn branch_changes_since_fork_including_uncommitted() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    sh(dir, &["init", "-q", "-b", "main"]);
    sh(dir, &["config", "user.name", "Ada"]);
    sh(dir, &["config", "user.email", "ada@example.com"]);
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

    let git = Git::new(dir, Console::default());
    let branches = vec!["feature".to_owned(), "main".to_owned()];
    assert_eq!(
        review::default_base(&git, &branches, Some("feature")).as_deref(),
        Some("main")
    );
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
fn default_branch_follows_the_remote() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    sh(dir, &["init", "-q", "-b", "main"]);
    sh(dir, &["config", "user.name", "Ada"]);
    sh(dir, &["config", "user.email", "ada@example.com"]);
    sh(dir, &["commit", "-q", "--allow-empty", "-m", "base"]);
    sh(dir, &["branch", "development"]);
    sh(dir, &["switch", "-q", "-c", "feature"]);
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

    let git = Git::new(dir, Console::default());
    let snap = graft_core::snapshot::load(&git).unwrap();
    assert_eq!(snap.remote_default.as_deref(), Some("development"));
    assert_eq!(snap.default_branch(), Some("development"));
    let branches = vec![
        "development".to_owned(),
        "feature".to_owned(),
        "main".to_owned(),
    ];
    assert_eq!(
        review::default_base(&git, &branches, Some("feature")).as_deref(),
        Some("development")
    );
}
