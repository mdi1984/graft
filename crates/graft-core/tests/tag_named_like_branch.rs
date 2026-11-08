//! A tag named like a branch (git prefers the tag when given the short
//! name) must not stand in for the branch anywhere.

mod common;

use common::{commit, rev, sh};
use graft_core::slash::{self, Reason, default_protected};
use graft_core::{Upstream, conflicts, refs, remote, review, snapshot, worktree};

#[test]
fn branches_are_never_taken_for_a_tag_of_the_same_name() {
    let tmp = tempfile::tempdir().unwrap();
    let origin = common::init_bare(&tmp.path().join("origin.git"));
    let dir = common::init(&tmp.path().join("app"));
    let git = common::git(&dir);
    sh(&dir, &["remote", "add", "origin", origin.to_str().unwrap()]);
    commit(&dir, "a.txt", "a\n", "old");
    // Tags named like the branch and like its upstream, on the old commit.
    sh(&dir, &["tag", "main"]);
    sh(&dir, &["tag", "origin/main"]);
    sh(&dir, &["tag", "topic"]);
    commit(&dir, "b.txt", "b\n", "new");
    sh(&dir, &["push", "-q", "-u", "origin", "refs/heads/main"]);
    commit(&dir, "c.txt", "c\n", "ahead");
    // topic forks from main's newest commit and is merged into it.
    sh(&dir, &["switch", "-q", "-c", "topic"]);
    commit(&dir, "t.txt", "t\n", "topic");
    sh(&dir, &["switch", "-q", "main"]);
    sh(
        &dir,
        &[
            "merge",
            "-q",
            "--no-ff",
            "-m",
            "merge topic",
            "refs/heads/topic",
        ],
    );
    // feature forks from main after that.
    sh(&dir, &["switch", "-q", "-c", "feature"]);
    commit(&dir, "f.txt", "f\n", "feature");

    let snap = snapshot::load(&git).unwrap();
    assert_eq!(snap.rev("main"), "refs/heads/main");
    assert_eq!(snap.rev("origin/main"), "refs/remotes/origin/main");
    assert_eq!(snap.rev("old-tag-free"), "old-tag-free");

    // Ahead/behind and the upstream's name.
    let main = snap.branch("main").unwrap();
    assert_eq!(
        main.upstream,
        Upstream::Tracking {
            name: "origin/main".into(),
            ahead: 3,
            behind: 0,
        }
    );

    // Review: the merge-base is main's newest commit, not the tag's.
    let r = review::load(&git, "main", &snap.rev("main")).unwrap();
    assert_eq!(r.merge_base.to_string(), rev(&dir, "refs/heads/main"));
    assert_eq!(r.commits, 1);
    assert_eq!(r.files.len(), 1);
    // Against the remote branch, by its name.
    let r = review::load(&git, "origin/main", &snap.rev("origin/main")).unwrap();
    assert_eq!(r.commits, 4);

    // Slash: topic is merged into main (the branch, not the old tag).
    let plan = slash::plan(&git, &snap, &default_protected()).unwrap();
    let topic = plan
        .candidates
        .iter()
        .find(|c| c.branch.as_deref() == Some("topic"))
        .unwrap_or_else(|| panic!("{plan:?}"));
    assert_eq!((topic.reason, topic.suggested), (Reason::Merged, true));

    // Merge and rebase take the branch.
    sh(&dir, &["switch", "-q", "-c", "other", "refs/tags/main"]);
    assert!(!conflicts::rebase(&git, &snap.rev("main")).unwrap());
    assert_eq!(rev(&dir, "HEAD"), rev(&dir, "refs/heads/main"));
    sh(&dir, &["reset", "-q", "--hard", "refs/tags/main"]);
    assert!(!conflicts::merge(&git, &snap.rev("feature"), false).unwrap());
    assert_eq!(rev(&dir, "HEAD"), rev(&dir, "refs/heads/feature"));

    // A new branch and a new worktree start at the branch.
    refs::create_branch(&git, "from-main", &snap.rev("main"), false).unwrap();
    assert_eq!(
        rev(&dir, "refs/heads/from-main"),
        rev(&dir, "refs/heads/main")
    );
    let wt = tmp.path().join("wt");
    let start = worktree::Start::NewBranch {
        name: "wt-branch".into(),
        base: snap.rev("main"),
    };
    worktree::add(&git, &wt, &start).unwrap();
    assert_eq!(rev(&wt, "HEAD"), rev(&dir, "refs/heads/main"));

    // Deleting a branch on the remote deletes the branch, not a tag there.
    sh(
        &dir,
        &[
            "push",
            "-q",
            "origin",
            "refs/heads/topic",
            "refs/tags/topic",
        ],
    );
    let cancel = graft_core::CancelToken::default();
    remote::delete_remote_branch(&git, "origin", "topic", &cancel, |_| {}).unwrap();
    let left = sh(
        &origin,
        &[
            "for-each-ref",
            "--format=%(refname)",
            "refs/heads/topic",
            "refs/tags/topic",
        ],
    );
    assert_eq!(left, "refs/tags/topic");
}
