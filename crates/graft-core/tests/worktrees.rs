//! Creating and removing worktrees against a real throwaway repository.

mod common;

use common::sh;
use graft_core::snapshot;
use graft_core::worktree::{self, Start};

#[test]
fn add_copy_lock_and_remove() {
    let tmp = tempfile::tempdir().unwrap();
    let main = common::init(&tmp.path().join("app"));
    std::fs::write(
        main.join(".gitignore"),
        ".env\nconfig/*.local.json\ntarget/\n",
    )
    .unwrap();
    std::fs::write(main.join("readme.md"), "hi\n").unwrap();
    sh(&main, &["add", "."]);
    sh(&main, &["commit", "-q", "-m", "init"]);
    std::fs::write(main.join(".env"), "SECRET=1\n").unwrap();
    std::fs::create_dir_all(main.join("config")).unwrap();
    std::fs::write(main.join("config/app.local.json"), "{}").unwrap();
    std::fs::create_dir_all(main.join("target")).unwrap();
    std::fs::write(main.join("target/.env"), "no").unwrap();

    let git = common::git(&main);
    let path = worktree::default_path(&main, "feature/x");
    let start = Start::NewBranch {
        name: "feature/x".into(),
        base: "main".into(),
    };
    worktree::add(&git, &path, &start).unwrap();
    assert!(path.join("readme.md").exists());

    let mut copied = worktree::copy_ignored(&git, &path, worktree::DEFAULT_COPY_PATTERNS).unwrap();
    copied.sort();
    assert_eq!(copied, [".env", "config/app.local.json"]);
    assert_eq!(
        std::fs::read_to_string(path.join(".env")).unwrap(),
        "SECRET=1\n"
    );
    assert!(
        !path.join("target").exists(),
        "ignored folders are not copied"
    );

    // Adding into a non-empty folder fails with a clear message.
    let err = worktree::add(&git, &path, &Start::Existing("main".into())).unwrap_err();
    assert!(format!("{err:#}").contains("not empty"));

    let snap = snapshot::load(&git).unwrap();
    assert_eq!(snap.worktrees.len(), 2);

    worktree::set_locked(&git, &path, true).unwrap();
    assert!(snapshot::load(&git).unwrap().worktrees[1].locked);
    worktree::set_locked(&git, &path, false).unwrap();
    assert!(!snapshot::load(&git).unwrap().worktrees[1].locked);

    // Dirty: a plain remove refuses, force works.
    std::fs::write(path.join("readme.md"), "changed\n").unwrap();
    assert!(worktree::remove(&git, &path, false).is_err());
    worktree::remove(&git, &path, true).unwrap();
    assert!(!path.exists());
    assert_eq!(snapshot::load(&git).unwrap().worktrees.len(), 1);

    // Prune forgets a worktree whose folder was deleted by hand.
    let gone = worktree::default_path(&main, "gone");
    worktree::add(&git, &gone, &Start::Existing("feature/x".into())).unwrap();
    std::fs::remove_dir_all(&gone).unwrap();
    worktree::prune(&git).unwrap();
    assert_eq!(snapshot::load(&git).unwrap().worktrees.len(), 1);
}

/// A new branch or worktree started from a remote branch does not track
/// it (pushing would go there); checking a remote branch out to track it does.
#[test]
fn new_branches_from_a_remote_branch_have_no_upstream() {
    let tmp = tempfile::tempdir().unwrap();
    let server = common::init_bare(&tmp.path().join("server.git"));
    let seed = common::init(&tmp.path().join("seed"));
    common::commit(&seed, "a.txt", "a\n", "a");
    sh(&seed, &["push", "-q", server.to_str().unwrap(), "main"]);
    let main = common::clone(&server, &tmp.path().join("app"));
    let git = common::git(&main);
    let upstream = |branch: &str| {
        sh(
            &main,
            &[
                "for-each-ref",
                "--format=%(upstream:short)",
                &format!("refs/heads/{branch}"),
            ],
        )
    };

    let path = worktree::default_path(&main, "wt");
    let start = Start::NewBranch {
        name: "wt".into(),
        base: "origin/main".into(),
    };
    worktree::add(&git, &path, &start).unwrap();
    assert_eq!(upstream("wt"), "");

    graft_core::refs::create_branch(&git, "kept", "origin/main", false).unwrap();
    assert_eq!(upstream("kept"), "");
    graft_core::refs::create_branch(&git, "switched", "origin/main", true).unwrap();
    assert_eq!(upstream("switched"), "");

    graft_core::refs::switch_tracking(&git, "tracked", "origin/main").unwrap();
    assert_eq!(upstream("tracked"), "origin/main");
    let tracking = worktree::default_path(&main, "wt2");
    let start = Start::Tracking {
        name: "wt2".into(),
        remote: "origin/main".into(),
    };
    worktree::add(&git, &tracking, &start).unwrap();
    assert_eq!(upstream("wt2"), "origin/main");
}
