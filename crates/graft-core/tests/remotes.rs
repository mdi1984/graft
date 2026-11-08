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
