//! Commit identity, for this repository only (the test must not touch the
//! user's global git config).

use std::process::Command;

use graft_core::identity::{self, Identity};
use graft_core::{Console, Git};

#[test]
fn save_and_load_local() {
    let tmp = tempfile::tempdir().unwrap();
    let ok = Command::new("git")
        .args(["init", "-q"])
        .current_dir(tmp.path())
        .status()
        .unwrap()
        .success();
    assert!(ok);
    let git = Git::new(tmp.path(), Console::default());
    let mine = Identity {
        name: "Ada Lovelace".into(),
        email: "ada@example.com".into(),
        local: true,
    };
    identity::save(&git, &mine).unwrap();
    assert_eq!(identity::load(&git).unwrap(), mine);

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
