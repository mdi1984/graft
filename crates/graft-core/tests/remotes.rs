//! Adding, editing and removing remotes.

mod common;

use common::sh;
use graft_core::{CancelToken, remote, snapshot};

#[test]
fn add_edit_and_remove() {
    let tmp = tempfile::tempdir().unwrap();
    let server = common::init_bare(&tmp.path().join("server.git"));
    let other = common::init_bare(&tmp.path().join("other.git"));
    let dir = common::init(&tmp.path().join("app"));
    common::commit(&dir, "a.txt", "a\n", "a");
    sh(&dir, &["push", "-q", server.to_str().unwrap(), "main"]);

    let git = common::git(&dir);
    let url = server.to_string_lossy().into_owned();
    remote::save_remote(&git, None, "origin", &url, false, true).unwrap();
    let snap = snapshot::load(&git).unwrap();
    assert_eq!(snap.remotes.len(), 1);
    assert!(snap.remote_branches.iter().any(|b| b.name == "origin/main"));

    let other_url = other.to_string_lossy().into_owned();
    remote::save_remote(&git, Some("origin"), "upstream", &other_url, false, false).unwrap();
    let snap = snapshot::load(&git).unwrap();
    assert_eq!(snap.remotes[0].name, "upstream");
    assert_eq!(snap.remotes[0].url, other_url);

    remote::remove_remote(&git, "upstream").unwrap();
    let snap = snapshot::load(&git).unwrap();
    assert!(snap.remotes.is_empty());
    assert!(snap.remote_branches.is_empty());

    // A new remote that can't be fetched is not kept.
    let missing = tmp.path().join("missing.git");
    let missing = missing.to_string_lossy().into_owned();
    assert!(remote::save_remote(&git, None, "backup", &missing, false, true).is_err());
    let snap = snapshot::load(&git).unwrap();
    assert!(snap.remotes.is_empty());
}

#[test]
fn urls_show_as_configured() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = common::init(&tmp.path().join("app"));
    sh(&dir, &["remote", "add", "origin", "C:/My Repos/x.git"]);
    sh(&dir, &["remote", "add", "work.mirror", "gh:me/app"]);
    sh(
        &dir,
        &["remote", "set-url", "--add", "work.mirror", "gh:me/other"],
    );
    sh(
        &dir,
        &["config", "url.https://github.com/.insteadOf", "gh:"],
    );
    // Only a push URL; and a push URL written before the URL.
    sh(&dir, &["config", "remote.backup.pushurl", "D:/backup.git"]);
    sh(&dir, &["config", "remote.both.pushurl", "D:/push.git"]);
    sh(&dir, &["config", "remote.both.url", "D:/fetch.git"]);

    let git = common::git(&dir);
    let snap = snapshot::load(&git).unwrap();
    let remotes: Vec<_> = snap
        .remotes
        .iter()
        .map(|r| (r.name.as_str(), r.url.as_str(), r.push_only))
        .collect();
    assert_eq!(
        remotes,
        [
            ("origin", "C:/My Repos/x.git", false),
            ("work.mirror", "gh:me/app", false),
            ("backup", "D:/backup.git", true),
            ("both", "D:/fetch.git", false),
        ]
    );

    // Editing a remote without changing its URL keeps it as it was.
    remote::save_remote(
        &git,
        Some("origin"),
        "origin",
        "C:/My Repos/x.git",
        false,
        false,
    )
    .unwrap();
    assert_eq!(
        sh(&dir, &["config", "remote.origin.url"]),
        "C:/My Repos/x.git"
    );

    // Saving a remote that has only a push URL changes that push URL and
    // adds no fetch URL.
    remote::save_remote(&git, Some("backup"), "backup", "E:/backup.git", true, false).unwrap();
    assert_eq!(
        sh(&dir, &["config", "--get-all", "remote.backup.pushurl"]),
        "E:/backup.git"
    );
    let snap = snapshot::load(&git).unwrap();
    let backup = snap.remotes.iter().find(|r| r.name == "backup").unwrap();
    assert_eq!(
        (backup.url.as_str(), backup.push_only),
        ("E:/backup.git", true)
    );
}

/// `git clone` writes no reflog for the remote-tracking branches, which
/// --force-if-includes needs: a rewritten branch of a fresh clone still
/// force pushes.
#[test]
fn force_push_from_a_fresh_clone() {
    let tmp = tempfile::tempdir().unwrap();
    let server = common::init_bare(&tmp.path().join("server.git"));
    let dir = common::init(&tmp.path().join("app"));
    common::commit(&dir, "a.txt", "a\n", "a");
    sh(&dir, &["push", "-q", server.to_str().unwrap(), "main:x"]);

    let clone = common::clone(&server, &tmp.path().join("clone"));
    sh(&clone, &["switch", "-q", "x"]);
    sh(&clone, &["commit", "-q", "--amend", "-m", "rewritten"]);
    let git = common::git(&clone);
    remote::force_push(&git, &CancelToken::default(), |_| {}).unwrap();
    assert_eq!(common::rev(&server, "x"), common::rev(&clone, "HEAD"));
}

/// A pull that stops with conflicts (merging or rebasing, as pull.rebase
/// says) reports them, rather than failing with fetch's progress lines.
#[test]
fn a_pull_that_conflicts_stops_for_resolving() {
    for rebase in [false, true] {
        let tmp = tempfile::tempdir().unwrap();
        let server = common::init_bare(&tmp.path().join("server.git"));
        let dir = common::init(&tmp.path().join("app"));
        common::commit(&dir, "a.txt", "a\n", "a");
        sh(&dir, &["push", "-q", server.to_str().unwrap(), "main"]);
        let mine = common::clone(&server, &tmp.path().join("mine"));
        let theirs = common::clone(&server, &tmp.path().join("theirs"));
        common::commit(&theirs, "a.txt", "theirs\n", "theirs");
        sh(&theirs, &["push", "-q"]);
        common::commit(&mine, "a.txt", "mine\n", "mine");
        sh(&mine, &["config", "pull.rebase", &rebase.to_string()]);

        let git = common::git(&mine);
        let stopped = remote::pull(&git, &CancelToken::default(), |_| {}).unwrap();
        assert!(stopped, "rebase: {rebase}");
        let marker = if rebase { "rebase-merge" } else { "MERGE_HEAD" };
        assert!(mine.join(".git").join(marker).exists(), "rebase: {rebase}");
        let status = sh(&mine, &["status", "--porcelain"]);
        assert_eq!(status, "UU a.txt", "rebase: {rebase}");
    }
    // A pull git refuses (local changes in the way) still fails.
    let tmp = tempfile::tempdir().unwrap();
    let server = common::init_bare(&tmp.path().join("server.git"));
    let dir = common::init(&tmp.path().join("app"));
    common::commit(&dir, "a.txt", "a\n", "a");
    sh(&dir, &["remote", "add", "origin", server.to_str().unwrap()]);
    sh(&dir, &["push", "-q", "-u", "origin", "main"]);
    let other = common::clone(&server, &tmp.path().join("other"));
    common::commit(&other, "a.txt", "other\n", "other");
    sh(&other, &["push", "-q"]);
    std::fs::write(dir.join("a.txt"), "edited\n").unwrap();
    let git = common::git(&dir);
    let error = remote::pull(&git, &CancelToken::default(), |_| {}).unwrap_err();
    assert!(format!("{error:#}").contains("overwritten"), "{error:#}");
}
