//! Commit identity, for this repository only (the test must not touch the
//! user's global git config).

mod common;

use common::sh;
use graft_core::identity::{self, Identity};

#[test]
fn save_and_load_local() {
    let tmp = tempfile::tempdir().unwrap();
    // A plain `git init`: the fixture's local identity would hide the
    // fallback checked below.
    sh(tmp.path(), &["init", "-q"]);
    let git = common::git(tmp.path());
    let mine = Identity {
        name: "Ada Lovelace".into(),
        email: "ada@example.com".into(),
        local: true,
    };
    identity::save(&git, &mine).unwrap();
    assert_eq!(identity::load(&git).unwrap(), mine);
    assert_eq!(
        sh(tmp.path(), &["config", "--local", "user.name"]),
        "Ada Lovelace",
        "stored in this repository"
    );

    // Clearing both local values falls back to whatever is global.
    identity::save(
        &git,
        &Identity {
            local: true,
            ..Identity::default()
        },
    )
    .unwrap();
    assert!(!identity::load(&git).unwrap().local);
}
