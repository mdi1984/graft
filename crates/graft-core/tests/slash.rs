//! Slash against a real repository with a remote.

use std::path::Path;
use std::process::Command;

use graft_core::slash::{self, PROTECTED, Reason};
use graft_core::worktree::{self, Start};
use graft_core::{CancelToken, Console, Git, snapshot};

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

fn commit(dir: &Path, file: &str, text: &str) {
    std::fs::write(dir.join(file), text).unwrap();
    sh(dir, &["add", "."]);
    sh(dir, &["commit", "-q", "-m", file]);
}

#[test]
fn scan_execute_and_restore() {
    let tmp = tempfile::tempdir().unwrap();
    let remote = tmp.path().join("remote.git");
    let main = tmp.path().join("app");
    std::fs::create_dir(&main).unwrap();
    sh(
        tmp.path(),
        &["init", "-q", "--bare", "-b", "main", "remote.git"],
    );
    sh(&main, &["init", "-q", "-b", "main"]);
    sh(&main, &["config", "user.name", "Ada"]);
    sh(&main, &["config", "user.email", "ada@example.com"]);
    sh(
        &main,
        &["remote", "add", "origin", remote.to_str().unwrap()],
    );
    commit(&main, "a.txt", "a\n");
    sh(&main, &["push", "-q", "-u", "origin", "main"]);

    // gone: pushed, then deleted on the remote (a merged PR).
    sh(&main, &["switch", "-q", "-c", "pr/done"]);
    commit(&main, "pr.txt", "pr\n");
    sh(&main, &["push", "-q", "-u", "origin", "pr/done"]);
    sh(&main, &["push", "-q", "origin", "--delete", "pr/done"]);
    // never pushed, not merged.
    sh(&main, &["switch", "-q", "-c", "idea", "main"]);
    commit(&main, "idea.txt", "idea\n");
    sh(&main, &["switch", "-q", "main"]);
    // merged into main, in a clean worktree / in a dirty worktree.
    sh(&main, &["branch", "merged-clean"]);
    sh(&main, &["branch", "merged-dirty"]);
    sh(&main, &["branch", "release/1"]);
    let git = Git::new(&main, Console::default());
    let clean = tmp.path().join("wt-clean");
    let dirty = tmp.path().join("wt-dirty");
    let gone_folder = tmp.path().join("wt-missing");
    worktree::add(&git, &clean, &Start::Existing("merged-clean".into())).unwrap();
    worktree::add(&git, &dirty, &Start::Existing("merged-dirty".into())).unwrap();
    std::fs::write(dirty.join("a.txt"), "changed\n").unwrap();
    worktree::add(
        &git,
        &gone_folder,
        &Start::NewBranch {
            name: "tmp/missing".into(),
            base: "main".into(),
        },
    )
    .unwrap();
    std::fs::remove_dir_all(&gone_folder).unwrap();

    let plan = slash::scan(&git, PROTECTED, &CancelToken::default(), |_| {}).unwrap();
    assert_eq!(plan.default_branch.as_deref(), Some("main"));
    assert_eq!(plan.fetch_error, None);
    let find = |name: &str| {
        plan.candidates
            .iter()
            .find(|c| c.branch.as_deref() == Some(name))
    };
    assert_eq!(find("pr/done").unwrap().reason, Reason::Gone);
    assert!(find("pr/done").unwrap().suggested);
    let merged = find("merged-clean").unwrap();
    assert_eq!(merged.reason, Reason::Merged);
    let folder = merged.worktree.as_ref().and_then(|p| p.file_name());
    assert_eq!(folder, clean.file_name());
    let idea = find("idea").unwrap();
    assert_eq!(idea.reason, Reason::NeverPushed);
    assert!(!idea.suggested);
    assert!(find("main").is_none());
    assert!(find("release/1").is_none());
    assert!(find("merged-dirty").is_none());
    assert!(
        plan.kept
            .iter()
            .any(|(n, why)| n == "merged-dirty" && why.contains("uncommitted"))
    );
    assert!(plan.kept.iter().any(|(n, _)| n == "release/1"));
    assert!(
        plan.candidates
            .iter()
            .any(|c| c.reason == Reason::MissingWorktree)
    );

    let chosen: Vec<_> = plan
        .candidates
        .iter()
        .filter(|c| c.suggested)
        .cloned()
        .collect();
    let outcome = slash::execute(&git, &chosen, &CancelToken::default(), |_| {});
    assert!(outcome.errors.is_empty(), "{:?}", outcome.errors);
    assert!(!clean.exists());
    let snap = snapshot::load(&git).unwrap();
    assert!(snap.branch("pr/done").is_none());
    assert!(snap.branch("merged-clean").is_none());
    assert!(snap.branch("idea").is_some());
    assert!(snap.branch("merged-dirty").is_some());
    assert!(!snap.worktrees.iter().any(|wt| wt.prunable));

    let done = outcome
        .slashed
        .iter()
        .find(|s| s.branch == "pr/done")
        .unwrap();
    slash::restore(&git, done).unwrap();
    assert_eq!(
        snapshot::load(&git)
            .unwrap()
            .branch("pr/done")
            .unwrap()
            .target,
        done.target
    );
}

/// Ignored build output (node_modules, target…) nested deeper than Windows'
/// old 260-character limit.
fn deep_ignored_files(worktree: &Path) {
    let mut dir = worktree.join("node_modules");
    while dir.as_os_str().len() < 320 {
        dir = dir.join("a-rather-long-package-name");
    }
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("index.js"), "module.exports = 1;\n").unwrap();
}

fn repo_with_merged_branch(tmp: &Path) -> (Git, std::path::PathBuf) {
    let main = tmp.join("app");
    std::fs::create_dir(&main).unwrap();
    sh(&main, &["init", "-q", "-b", "main"]);
    sh(&main, &["config", "user.name", "Ada"]);
    sh(&main, &["config", "user.email", "ada@example.com"]);
    commit(&main, ".gitignore", "node_modules/\n");
    sh(&main, &["branch", "merged-deep"]);
    let git = Git::new(&main, Console::default());
    let wt = tmp.join("wt-deep");
    worktree::add(&git, &wt, &Start::Existing("merged-deep".into())).unwrap();
    deep_ignored_files(&wt);
    (git, wt)
}

#[test]
fn worktrees_with_very_long_paths_are_slashed() {
    let tmp = tempfile::tempdir().unwrap();
    let (git, wt) = repo_with_merged_branch(tmp.path());

    let snap = snapshot::load(&git).unwrap();
    let plan = slash::plan(&git, &snap, PROTECTED).unwrap();
    let chosen: Vec<_> = plan
        .candidates
        .into_iter()
        .filter(|c| c.branch.as_deref() == Some("merged-deep"))
        .collect();
    assert_eq!(chosen.len(), 1);
    assert!(chosen[0].worktree.is_some());

    let outcome = slash::execute(&git, &chosen, &CancelToken::default(), |_| {});
    assert!(outcome.errors.is_empty(), "{:?}", outcome.errors);
    assert_eq!(outcome.worktrees_removed, 1);
    assert_eq!(outcome.slashed.len(), 1);
    assert!(!wt.exists());
}

#[test]
fn a_dirty_worktree_with_long_paths_is_kept() {
    let tmp = tempfile::tempdir().unwrap();
    let (git, wt) = repo_with_merged_branch(tmp.path());
    std::fs::write(wt.join("notes.txt"), "not committed\n").unwrap();

    assert!(worktree::remove(&git, &wt, false).is_err());
    assert!(wt.join("notes.txt").exists());
}

fn repo_with_merged_branches(tmp: &Path, count: usize) -> (Git, Vec<slash::Candidate>) {
    let main = tmp.join("app");
    std::fs::create_dir(&main).unwrap();
    sh(&main, &["init", "-q", "-b", "main"]);
    sh(&main, &["config", "user.name", "Ada"]);
    sh(&main, &["config", "user.email", "ada@example.com"]);
    commit(&main, "a.txt", "a\n");
    for i in 0..count {
        sh(&main, &["branch", &format!("done/{i}")]);
    }
    let git = Git::new(&main, Console::default());
    let snap = snapshot::load(&git).unwrap();
    let mut chosen = slash::plan(&git, &snap, PROTECTED).unwrap().candidates;
    chosen.sort_by(|a, b| a.branch.cmp(&b.branch));
    assert_eq!(chosen.len(), count);
    (git, chosen)
}

#[test]
fn every_step_is_reported() {
    let tmp = tempfile::tempdir().unwrap();
    let (git, chosen) = repo_with_merged_branches(tmp.path(), 5);
    let mut lines = Vec::new();
    let outcome = slash::execute(&git, &chosen, &CancelToken::default(), |p| {
        lines.push((p.line, p.fraction));
    });
    assert_eq!(outcome.slashed.len(), 5);
    assert!(!outcome.cancelled);
    assert_eq!(lines.len(), 5);
    assert_eq!(
        lines[0],
        ("Deleting branch done/0 (1 of 5)".to_owned(), Some(0.0))
    );
    assert_eq!(
        lines[4],
        ("Deleting branch done/4 (5 of 5)".to_owned(), Some(0.8))
    );
}

#[test]
fn cancelling_stops_after_the_current_step() {
    let tmp = tempfile::tempdir().unwrap();
    let (git, chosen) = repo_with_merged_branches(tmp.path(), 5);
    let cancel = CancelToken::default();
    let mut steps = 0;
    let outcome = slash::execute(&git, &chosen, &cancel, |_| {
        steps += 1;
        if steps == 2 {
            cancel.cancel();
        }
    });
    // The step that was running finishes; nothing after it starts.
    assert!(outcome.cancelled);
    assert_eq!(outcome.slashed.len(), 2);
    let left = git.read(["branch", "--list", "done/*"]).unwrap();
    assert_eq!(left.lines().count(), 3);
}
