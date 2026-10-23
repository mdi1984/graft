//! Adding, editing and removing remotes.

use std::path::Path;
use std::process::Command;

use graft_core::remote;
use graft_core::{Console, Git, snapshot};

fn sh(dir: &Path, args: &[&str]) {
    let ok = Command::new("git")
        .args(args)
        .current_dir(dir)
        .status()
        .expect("git runs")
        .success();
    assert!(ok, "git {args:?} failed");
}

#[test]
fn add_edit_and_remove() {
    let tmp = tempfile::tempdir().unwrap();
    let server = tmp.path().join("server.git");
    let other = tmp.path().join("other.git");
    let dir = tmp.path().join("app");
    std::fs::create_dir(&dir).unwrap();
    sh(
        tmp.path(),
        &["init", "-q", "--bare", "-b", "main", "server.git"],
    );
    sh(
        tmp.path(),
        &["init", "-q", "--bare", "-b", "main", "other.git"],
    );
    sh(&dir, &["init", "-q", "-b", "main"]);
    std::fs::write(dir.join("a.txt"), "a\n").unwrap();
    sh(&dir, &["add", "."]);
    sh(
        &dir,
        &[
            "-c",
            "user.name=Ada",
            "-c",
            "user.email=a@example.com",
            "commit",
            "-q",
            "-m",
            "a",
        ],
    );
    sh(&dir, &["push", "-q", server.to_str().unwrap(), "main"]);

    let git = Git::new(&dir, Console::default());
    let url = server.to_string_lossy().into_owned();
    remote::save_remote(&git, None, "origin", &url, true).unwrap();
    let snap = snapshot::load(&git).unwrap();
    assert_eq!(snap.remotes.len(), 1);
    assert!(snap.remote_branches.iter().any(|b| b.name == "origin/main"));

    let other_url = other.to_string_lossy().into_owned();
    remote::save_remote(&git, Some("origin"), "upstream", &other_url, false).unwrap();
    let snap = snapshot::load(&git).unwrap();
    assert_eq!(snap.remotes[0].name, "upstream");
    assert_eq!(snap.remotes[0].fetch_url, other_url);

    remote::remove_remote(&git, "upstream").unwrap();
    let snap = snapshot::load(&git).unwrap();
    assert!(snap.remotes.is_empty());
    assert!(snap.remote_branches.is_empty());

    // A new remote that can't be fetched is not kept.
    let missing = tmp.path().join("missing.git");
    let missing = missing.to_string_lossy().into_owned();
    assert!(remote::save_remote(&git, None, "backup", &missing, true).is_err());
    let snap = snapshot::load(&git).unwrap();
    assert!(snap.remotes.is_empty());
}
