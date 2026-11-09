//! Adding, editing and removing remotes.

mod common;

use common::sh;
use graft_core::console::Console;
use graft_core::{CancelToken, Git, remote, snapshot};

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

/// A pull git refuses fails with git's message (and as a failure in the
/// console), even when its output mentions conflicts: here a fetched
/// branch's name.
#[test]
fn a_failed_pull_is_not_taken_for_conflicts() {
    let tmp = tempfile::tempdir().unwrap();
    let server = common::init_bare(&tmp.path().join("server.git"));
    let dir = common::init(&tmp.path().join("app"));
    common::commit(&dir, "a.txt", "a\n", "a");
    sh(&dir, &["push", "-q", server.to_str().unwrap(), "main"]);
    let mine = common::clone(&server, &tmp.path().join("mine"));
    let theirs = common::clone(&server, &tmp.path().join("theirs"));
    common::commit(&theirs, "a.txt", "theirs\n", "theirs");
    sh(&theirs, &["push", "-q"]);
    sh(
        &theirs,
        &["push", "-q", "origin", "HEAD:fix-CONFLICT-detection"],
    );
    common::commit(&mine, "b.txt", "mine\n", "mine");
    sh(&mine, &["config", "pull.ff", "only"]);

    let console = Console::default();
    let git = Git::new(&mine, console.clone());
    let error = remote::pull(&git, &CancelToken::default(), |_| {}).unwrap_err();
    let error = format!("{error:#}");
    assert!(error.contains("fast-forward"), "{error}");
    let pull = console.snapshot().into_iter().last().unwrap();
    assert!(
        pull.stderr.contains("fix-CONFLICT-detection"),
        "{}",
        pull.stderr
    );
    assert!(pull.failed());
    assert!(!mine.join(".git").join("MERGE_HEAD").exists());
}

/// A clone of a server with branches `main` and `feature` (tracking
/// origin/feature, not checked out), and a second clone `theirs` that
/// pushed one more commit to `feature`. Returns (tmp, mine, theirs).
fn behind_on_feature() -> (tempfile::TempDir, std::path::PathBuf, std::path::PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let server = common::init_bare(&tmp.path().join("server.git"));
    let dir = common::init(&tmp.path().join("app"));
    common::commit(&dir, "a.txt", "a\n", "a");
    sh(
        &dir,
        &[
            "push",
            "-q",
            server.to_str().unwrap(),
            "main",
            "main:feature",
        ],
    );
    let mine = common::clone(&server, &tmp.path().join("mine"));
    sh(
        &mine,
        &["branch", "-q", "--track", "feature", "origin/feature"],
    );
    let theirs = common::clone(&server, &tmp.path().join("theirs"));
    sh(&theirs, &["switch", "-q", "feature"]);
    common::commit(&theirs, "b.txt", "b\n", "theirs");
    sh(&theirs, &["push", "-q"]);
    (tmp, mine, theirs)
}

/// Pulling a branch that is checked out nowhere moves it ahead to its
/// upstream, and leaves the checked-out branch and its files alone.
#[test]
fn pull_a_branch_that_is_not_checked_out() {
    let (_tmp, mine, theirs) = behind_on_feature();
    let main = common::rev(&mine, "main");
    let git = common::git(&mine);
    let stopped = remote::pull_branch(&git, "feature", &CancelToken::default(), |_| {});
    assert!(!stopped.unwrap());
    assert_eq!(common::rev(&mine, "feature"), common::rev(&theirs, "HEAD"));
    assert_eq!(
        common::rev(&mine, "origin/feature"),
        common::rev(&theirs, "HEAD")
    );
    assert_eq!(common::rev(&mine, "HEAD"), main);
    assert!(!mine.join("b.txt").exists());
}

/// A branch with commits its upstream lacks is not moved (nothing is
/// lost), and the error says to pull where it is checked out.
#[test]
fn pull_a_diverged_branch_that_is_not_checked_out() {
    let (_tmp, mine, _theirs) = behind_on_feature();
    sh(&mine, &["switch", "-q", "feature"]);
    common::commit(&mine, "c.txt", "c\n", "mine");
    sh(&mine, &["switch", "-q", "main"]);
    let before = common::rev(&mine, "feature");
    let git = common::git(&mine);
    let error = remote::pull_branch(&git, "feature", &CancelToken::default(), |_| {});
    let error = format!("{:#}", error.unwrap_err());
    assert!(
        error.contains("feature and its upstream have diverged"),
        "{error}"
    );
    assert_eq!(common::rev(&mine, "feature"), before);
}

/// Pulling a branch from a worktree it is not checked out in, while
/// another worktree has it: git refuses rather than move that worktree's
/// branch under its files.
#[test]
fn pull_a_branch_checked_out_in_another_worktree_from_elsewhere() {
    let (tmp, mine, _theirs) = behind_on_feature();
    let other = tmp.path().join("other");
    sh(
        &mine,
        &["worktree", "add", "-q", other.to_str().unwrap(), "feature"],
    );
    let before = common::rev(&mine, "feature");
    let git = common::git(&mine);
    let pulled = remote::pull_branch(&git, "feature", &CancelToken::default(), |_| {});
    let error = format!("{:#}", pulled.unwrap_err());
    assert!(error.contains("checked out at"), "{error}");
    assert_eq!(common::rev(&mine, "feature"), before);
    // From its own worktree it is a normal pull.
    let git = common::git(&other);
    let stopped = remote::pull_branch(&git, "feature", &CancelToken::default(), |_| {});
    assert!(!stopped.unwrap());
    assert!(other.join("b.txt").exists());
}

/// A branch without upstream says so.
#[test]
fn pull_a_branch_without_upstream() {
    let (_tmp, mine, _theirs) = behind_on_feature();
    sh(&mine, &["branch", "-q", "local-only"]);
    let git = common::git(&mine);
    let error = remote::pull_branch(&git, "local-only", &CancelToken::default(), |_| {});
    let error = format!("{:#}", error.unwrap_err());
    assert!(error.contains("no upstream"), "{error}");
}

/// With a fetch refspec without `+`, git refuses to move a force-pushed
/// origin/feature, but still moves feature ahead: the error says both.
#[test]
fn pull_a_branch_when_only_the_remote_tracking_branch_is_refused() {
    let (_tmp, mine, theirs) = behind_on_feature();
    let start = common::rev(&mine, "feature");
    sh(&mine, &["fetch", "-q"]);
    sh(&theirs, &["reset", "-q", "--hard", &start]);
    common::commit(&theirs, "z.txt", "z\n", "rewritten");
    sh(&theirs, &["push", "-q", "--force"]);
    sh(
        &mine,
        &[
            "config",
            "remote.origin.fetch",
            "refs/heads/*:refs/remotes/origin/*",
        ],
    );
    let git = common::git(&mine);
    let error = remote::pull_branch(&git, "feature", &CancelToken::default(), |_| {});
    let error = format!("{:#}", error.unwrap_err());
    assert!(error.contains("feature moved ahead, but"), "{error}");
    assert!(error.contains("origin/feature"), "{error}");
    assert_eq!(common::rev(&mine, "feature"), common::rev(&theirs, "HEAD"));
}

/// A branch whose upstream was force-pushed away from it is not moved, and
/// the error says the upstream was rewritten (not that the branch has
/// commits of its own).
#[test]
fn pull_a_branch_whose_upstream_was_force_pushed() {
    let (_tmp, mine, theirs) = behind_on_feature();
    let start = common::rev(&mine, "feature");
    let git = common::git(&mine);
    remote::pull_branch(&git, "feature", &CancelToken::default(), |_| {}).unwrap();
    let pulled = common::rev(&mine, "feature");
    sh(&theirs, &["reset", "-q", "--hard", &start]);
    common::commit(&theirs, "z.txt", "z\n", "rewritten");
    sh(&theirs, &["push", "-q", "--force"]);
    let error = remote::pull_branch(&git, "feature", &CancelToken::default(), |_| {});
    let error = format!("{:#}", error.unwrap_err());
    assert!(error.contains("rewritten on the remote"), "{error}");
    assert_eq!(common::rev(&mine, "feature"), pulled);
}

/// A branch whose upstream is another local branch (remote ".") moves
/// ahead to it.
#[test]
fn pull_a_branch_tracking_a_local_branch() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = common::init(&tmp.path().join("app"));
    common::commit(&dir, "a.txt", "a\n", "a");
    sh(&dir, &["branch", "-q", "--track", "follow", "main"]);
    common::commit(&dir, "b.txt", "b\n", "b");
    let git = common::git(&dir);
    let stopped = remote::pull_branch(&git, "follow", &CancelToken::default(), |_| {});
    assert!(!stopped.unwrap());
    assert_eq!(common::rev(&dir, "follow"), common::rev(&dir, "main"));
}
