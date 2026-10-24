//! File history and blame against a real repository.

use std::path::Path;
use std::process::Command;

use graft_core::file_log;
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

fn commit(dir: &Path, author: &str, message: &str) {
    sh(dir, &["add", "-A"]);
    sh(
        dir,
        &[
            "-c",
            &format!("user.name={author}"),
            "-c",
            "user.email=x@example.com",
            "commit",
            "-q",
            "-m",
            message,
        ],
    );
}

#[test]
fn history_follows_renames_and_blame_names_authors() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    sh(dir, &["init", "-q", "-b", "main"]);
    std::fs::write(dir.join("old.txt"), "one\ntwo\nthree\n").unwrap();
    std::fs::write(dir.join("other.txt"), "x\n").unwrap();
    commit(dir, "Ada", "Add old");
    std::fs::write(dir.join("other.txt"), "y\n").unwrap();
    commit(dir, "Ada", "Touch other");
    sh(dir, &["mv", "old.txt", "new.txt"]);
    commit(dir, "Ada", "Rename");
    std::fs::write(dir.join("new.txt"), "one\nTWO\nthree\n").unwrap();
    commit(dir, "Grace", "Shout two");

    let git = Git::new(dir, Console::default());
    let log = file_log::log(&git, "new.txt", 100).unwrap();
    let summaries: Vec<&str> = log.iter().map(|c| c.summary.as_str()).collect();
    assert_eq!(summaries, ["Shout two", "Rename", "Add old"]);
    assert_eq!(log[0].path, "new.txt");
    assert_eq!(log[2].path, "old.txt");
    assert_eq!(log[0].author, "Grace");

    let head = log[0].id;
    let blame = file_log::blame(&git, head, "new.txt").unwrap();
    let texts: Vec<&str> = blame.lines.iter().map(|l| l.text.as_str()).collect();
    assert_eq!(texts, ["one", "TWO", "three"]);
    let author = |i: usize| blame.commits[&blame.lines[i].commit].author.as_str();
    assert_eq!(author(0), "Ada");
    assert_eq!(author(1), "Grace");
    assert_eq!(blame.commits[&blame.lines[1].commit].summary, "Shout two");
    assert!(!blame.binary);

    // Blame at an older commit, under the old name.
    let first = log[2].id;
    let old = file_log::blame(&git, first, "old.txt").unwrap();
    assert_eq!(old.lines[1].text, "two");
}
