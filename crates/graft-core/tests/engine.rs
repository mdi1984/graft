//! End-to-end checks against real throwaway repositories.

use std::path::Path;
use std::process::Command;

use graft_core::history::{self, History};
use graft_core::{CancelToken, Console, Git, details, repo, snapshot};

/// Run git directly to build fixtures, with a fixed identity.
fn sh(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_AUTHOR_NAME", "Ada")
        .env("GIT_AUTHOR_EMAIL", "ada@example.com")
        .env("GIT_COMMITTER_NAME", "Ada")
        .env("GIT_COMMITTER_EMAIL", "ada@example.com")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .expect("git runs");
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_owned()
}

fn commit_file(dir: &Path, name: &str, content: &str, message: &str) {
    std::fs::write(dir.join(name), content).unwrap();
    sh(dir, &["add", name]);
    sh(dir, &["commit", "-q", "-m", message]);
}

fn fixture() -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("demo");
    std::fs::create_dir(&dir).unwrap();
    sh(&dir, &["init", "-q", "-b", "main"]);
    commit_file(&dir, "a.txt", "one\n", "First");
    commit_file(&dir, "b.txt", "two\n", "Second\n\nWith a body.");
    sh(&dir, &["switch", "-q", "-c", "feature"]);
    commit_file(&dir, "c.txt", "three\n", "Feature work");
    sh(&dir, &["switch", "-q", "main"]);
    commit_file(&dir, "a.txt", "one\nmore\n", "Main work");
    sh(
        &dir,
        &["merge", "-q", "--no-ff", "-m", "Merge feature", "feature"],
    );
    sh(&dir, &["tag", "-a", "v1", "-m", "Version 1"]);
    tmp
}

#[test]
fn locate_snapshot_history_details() {
    let tmp = fixture();
    let dir = tmp.path().join("demo");
    let console = Console::default();

    // A linked worktree resolves to its main repository.
    let wt = tmp.path().join("demo-wt");
    sh(
        &dir,
        &["worktree", "add", "-q", "-b", "agent", wt.to_str().unwrap()],
    );
    std::fs::write(wt.join("dirty.txt"), "x").unwrap();
    let loc = repo::locate(&wt, &console).unwrap();
    assert!(same(&loc.root, &dir), "{:?} vs {:?}", loc.root, dir);
    assert!(same(&loc.opened, &wt));

    let git = Git::new(&loc.root, console.clone());
    let snap = snapshot::load(&git).unwrap();
    assert_eq!(snap.name, "demo");
    assert_eq!(snap.worktrees.len(), 2);
    assert!(snap.worktrees[0].is_main);
    assert_eq!(snap.worktrees[0].dirty(), 0);
    assert_eq!(snap.worktrees[1].branch.as_deref(), Some("agent"));
    assert_eq!(snap.worktrees[1].dirty(), 1);
    let names: Vec<_> = snap.branches.iter().map(|b| b.name.as_str()).collect();
    assert_eq!(names, ["agent", "feature", "main"]);
    assert_eq!(snap.tags.len(), 1);
    assert!(snap.tags[0].annotated);

    // History over all branches: 5 commits, merge on top, two lanes.
    let tips: Vec<_> = snap.branches.iter().map(|b| b.target).collect();
    let commits = history::walk(&loc.root, &tips, 1000, &CancelToken::default()).unwrap();
    assert_eq!(commits.list.len(), 5);
    let head = snap.worktrees[0].head.unwrap();
    let h = History::build(&commits, Some(head));
    assert!(h.commits[0].is_worktree());
    assert_eq!(h.commits[1].summary, "Merge feature");
    assert_eq!(h.commits.last().unwrap().summary, "First");
    assert_eq!(h.lanes, 2);

    // Details of the merge list the files brought in from the feature branch.
    let d = details::load(&git, head).unwrap();
    assert_eq!(d.parents.len(), 2);
    assert_eq!(d.author, "Ada");
    assert_eq!(d.files.len(), 1);
    assert_eq!(d.files[0].path, "c.txt");
    assert_eq!(d.files[0].added, Some(1));

    let second = h.commits.iter().find(|c| c.summary == "Second").unwrap();
    let d = details::load(&git, second.id).unwrap();
    assert_eq!(d.body(), "With a body.");

    // Every command went through the console.
    assert!(!console.snapshot().is_empty());
}

#[test]
fn init_and_clone() {
    let tmp = fixture();
    let console = Console::default();

    let fresh = tmp.path().join("new repo");
    let loc = repo::init(&fresh, &console).unwrap();
    assert!(same(&loc.root, &fresh));
    let snap = snapshot::load(&Git::new(&loc.root, console.clone())).unwrap();
    assert!(snap.branches.is_empty());
    assert_eq!(snap.worktrees.len(), 1);

    let source = tmp.path().join("demo");
    let dest = tmp.path().join("cloned");
    let mut phases = Vec::new();
    let loc = repo::clone(
        source.to_str().unwrap(),
        &dest,
        &console,
        &CancelToken::default(),
        |p| phases.push(p.phase),
    )
    .unwrap();
    assert!(same(&loc.root, &dest));
    let snap = snapshot::load(&Git::new(&loc.root, console.clone())).unwrap();
    assert_eq!(snap.remotes.len(), 1);
    assert_eq!(snap.remotes[0].name, "origin");
    assert!(
        snap.remote_branches
            .iter()
            .any(|b| b.name == "origin/feature")
    );

    // Cloning into a non-empty folder is refused and leaves it alone.
    let err = repo::clone(
        source.to_str().unwrap(),
        &dest,
        &console,
        &CancelToken::default(),
        |_| {},
    )
    .unwrap_err();
    assert!(err.to_string().contains("not empty"));
    assert!(dest.join("a.txt").exists());

    assert!(repo::locate(tmp.path(), &console).is_err());
}

fn same(a: &Path, b: &Path) -> bool {
    let canon = |p: &Path| dunce(p.canonicalize().unwrap());
    canon(a) == canon(b)
}

fn dunce(p: std::path::PathBuf) -> String {
    p.to_string_lossy()
        .trim_start_matches(r"\\?\")
        .to_lowercase()
}
