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
