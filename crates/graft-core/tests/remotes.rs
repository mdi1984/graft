//! Adding, editing and removing remotes.

mod common;

use common::sh;
use graft_core::{remote, snapshot};

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
