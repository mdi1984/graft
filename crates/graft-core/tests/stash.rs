//! Stashes against a real throwaway repository.

mod common;

use common::{commit, sh};
use graft_core::stash::{self, Options};

#[test]
fn stash_push_list_apply_branch_drop() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = common::init(&tmp.path().join("r"));
    commit(&dir, "a.txt", "a\n", "init");
    let git = common::git(&dir);

    assert!(
        stash::push(&git, &Options::default()).is_err(),
        "nothing to stash"
    );
    std::fs::write(dir.join("a.txt"), "changed\n").unwrap();
    std::fs::write(dir.join("new.txt"), "new\n").unwrap();
    let options = Options {
        message: "half done".into(),
        include_untracked: true,
        ..Options::default()
    };
    stash::push(&git, &options).unwrap();
    assert!(!dir.join("new.txt").exists());
    assert_eq!(sh(&dir, &["status", "--porcelain"]), "");

    let list = stash::list(&git).unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].message, "half done");
    assert_eq!(list[0].branch.as_deref(), Some("main"));
    let mut files: Vec<String> = stash::files(&git, list[0].id)
        .unwrap()
        .into_iter()
        .map(|f| f.change.path)
        .collect();
    files.sort();
    assert_eq!(files, ["a.txt", "new.txt"]);

    stash::apply(&git, list[0].id, false).unwrap();
    assert!(dir.join("new.txt").exists());
    assert_eq!(stash::list(&git).unwrap().len(), 1, "apply keeps the stash");
    sh(&dir, &["reset", "-q", "--hard"]);
    sh(&dir, &["clean", "-qfd"]);

    stash::branch(&git, "from-stash", list[0].id).unwrap();
    assert_eq!(sh(&dir, &["symbolic-ref", "--short", "HEAD"]), "from-stash");
    assert_eq!(
        std::fs::read_to_string(dir.join("a.txt")).unwrap().trim(),
        "changed"
    );
    assert!(
        stash::list(&git).unwrap().is_empty(),
        "branch drops the stash"
    );
    assert!(stash::drop(&git, list[0].id).is_err());
}

#[test]
fn pop_applies_and_drops_and_drop_forgets() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = common::init(&tmp.path().join("r"));
    commit(&dir, "a.txt", "a\n", "init");
    let git = common::git(&dir);
    let push = |text: &str, message: &str| {
        std::fs::write(dir.join("a.txt"), text).unwrap();
        let options = Options {
            message: message.into(),
            ..Options::default()
        };
        stash::push(&git, &options).unwrap();
    };
    push("first\n", "first");
    push("second\n", "second");
    let list = stash::list(&git).unwrap();
    let messages: Vec<&str> = list.iter().map(|s| s.message.as_str()).collect();
    assert_eq!(messages, ["second", "first"], "newest first");

    // Dropping the older one by its id leaves the newer one.
    stash::drop(&git, list[1].id).unwrap();
    let list = stash::list(&git).unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].message, "second");

    stash::apply(&git, list[0].id, true).unwrap();
    assert_eq!(
        std::fs::read_to_string(dir.join("a.txt")).unwrap(),
        "second\n"
    );
    assert!(stash::list(&git).unwrap().is_empty(), "pop drops the stash");
}
