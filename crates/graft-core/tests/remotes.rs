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

    let git = common::git(&dir);
    let snap = snapshot::load(&git).unwrap();
    let remotes: Vec<_> = snap
        .remotes
        .iter()
        .map(|r| (r.name.as_str(), r.fetch_url.as_str()))
        .collect();
    assert_eq!(
        remotes,
        [
            ("origin", "C:/My Repos/x.git"),
            ("work.mirror", "gh:me/app")
        ]
    );

    // Editing a remote without changing its URL keeps it as it was.
    remote::save_remote(&git, Some("origin"), "origin", "C:/My Repos/x.git", false).unwrap();
    assert_eq!(
        sh(&dir, &["config", "remote.origin.url"]),
        "C:/My Repos/x.git"
    );
}
