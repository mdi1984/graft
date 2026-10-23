//! Preview in main against a real repository with a second worktree.

use std::path::Path;
use std::process::Command;

use graft_core::preview::{self, Restore};
use graft_core::worktree::{self, Start};
use graft_core::{Console, Git};

fn sh(dir: &Path, args: &[&str]) -> String {
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
    String::from_utf8_lossy(&out.stdout).trim().to_owned()
}

#[test]
fn start_update_and_restore() {
    let tmp = tempfile::tempdir().unwrap();
    let main = tmp.path().join("app");
    std::fs::create_dir(&main).unwrap();
    sh(&main, &["init", "-q", "-b", "main"]);
    sh(&main, &["config", "user.name", "Ada"]);
    sh(&main, &["config", "user.email", "ada@example.com"]);
    sh(&main, &["config", "core.autocrlf", "true"]);
    std::fs::write(main.join("a.txt"), "one\r\ntwo\r\n").unwrap();
    sh(&main, &["add", "."]);
    sh(&main, &["commit", "-q", "-m", "a"]);

    let git = Git::new(&main, Console::default());
    let agent = tmp.path().join("agent");
    worktree::add(
        &git,
        &agent,
        &Start::NewBranch {
            name: "agent/x".into(),
            base: "main".into(),
        },
    )
    .unwrap();
    std::fs::write(agent.join("b.txt"), "committed\r\n").unwrap();
    sh(&agent, &["add", "."]);
    sh(&agent, &["commit", "-q", "-m", "b"]);
    let agent_head = sh(&agent, &["rev-parse", "HEAD"]);
    // Uncommitted: a modified file, a staged new file, an untracked folder.
    std::fs::write(agent.join("a.txt"), "one\r\ntwo\r\nthree\r\n").unwrap();
    std::fs::write(agent.join("staged.txt"), "staged\r\n").unwrap();
    sh(&agent, &["add", "staged.txt"]);
    std::fs::create_dir(agent.join("new")).unwrap();
    std::fs::write(agent.join("new").join("c.txt"), "untracked\r\n").unwrap();
    let agent_status = sh(&agent, &["status", "--porcelain"]);

    // Main must be clean.
    std::fs::write(main.join("a.txt"), "dirty\r\n").unwrap();
    let source = git.at(&agent);
    assert!(preview::start(&git, &source).is_err());
    sh(&main, &["checkout", "--", "a.txt"]);

    let shown = preview::start(&git, &source).unwrap();
    assert_eq!(shown.restore, Restore::Branch("main".into()));
    assert_eq!(sh(&main, &["rev-parse", "HEAD"]), agent_head);
    assert_eq!(
        std::fs::read(main.join("a.txt")).unwrap(),
        b"one\r\ntwo\r\nthree\r\n"
    );
    assert_eq!(std::fs::read(main.join("b.txt")).unwrap(), b"committed\r\n");
    assert!(main.join("staged.txt").is_file());
    assert!(main.join("new").join("c.txt").is_file());
    assert_eq!(preview::active(&main), Some(shown));
    // The agent's worktree and staging are untouched.
    assert_eq!(sh(&agent, &["status", "--porcelain"]), agent_status);
    assert!(preview::start(&git, &source).is_err());

    // The agent keeps working; update shows the new state.
    std::fs::remove_file(agent.join("new").join("c.txt")).unwrap();
    std::fs::write(agent.join("d.txt"), "later\r\n").unwrap();
    preview::update(&git).unwrap();
    assert!(!main.join("new").exists());
    assert!(main.join("d.txt").is_file());

    preview::restore_main(&git).unwrap();
    assert_eq!(sh(&main, &["symbolic-ref", "--short", "HEAD"]), "main");
    assert_eq!(sh(&main, &["status", "--porcelain"]), "");
    assert!(!main.join("d.txt").exists());
    assert!(!main.join("b.txt").exists());
    assert_eq!(preview::active(&main), None);
}
