//! Creating and removing worktrees against a real throwaway repository.

use std::path::Path;
use std::process::Command;

use graft_core::worktree::{self, Start};
use graft_core::{Console, Git, snapshot};

fn sh(dir: &Path, args: &[&str]) {
    let out = Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .expect("git runs");
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn add_copy_lock_and_remove() {
    let tmp = tempfile::tempdir().unwrap();
    let main = tmp.path().join("app");
    std::fs::create_dir(&main).unwrap();
    sh(&main, &["init", "-q", "-b", "main"]);
    sh(&main, &["config", "user.name", "Ada"]);
    sh(&main, &["config", "user.email", "ada@example.com"]);
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

    let git = Git::new(&main, Console::default());
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

#[test]
fn branches_tags_and_commit_ops() {
    use graft_core::refs::{self, ResetMode};
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("r");
    std::fs::create_dir(&dir).unwrap();
    sh(&dir, &["init", "-q", "-b", "main"]);
    sh(&dir, &["config", "user.name", "Ada"]);
    sh(&dir, &["config", "user.email", "ada@example.com"]);
    let git = Git::new(&dir, Console::default());
    let commit = |file: &str, text: &str, msg: &str| {
        std::fs::write(dir.join(file), text).unwrap();
        sh(&dir, &["add", file]);
        sh(&dir, &["commit", "-q", "-m", msg]);
    };
    commit("a.txt", "a\n", "one");
    commit("a.txt", "a\nb\n", "two");

    assert!(refs::check_branch_name(&git, "bad name").is_err());
    assert!(refs::check_branch_name(&git, "bad..name").is_err());
    refs::create_branch(&git, "topic", "HEAD~1", true).unwrap();
    commit("t.txt", "t\n", "topic work");
    refs::switch(&git, "main").unwrap();

    // Not merged: plain delete refuses with a recognisable message.
    let err = format!(
        "{:#}",
        refs::delete_branch(&git, "topic", false).unwrap_err()
    );
    assert!(err.contains(refs::NOT_MERGED), "{err}");
    refs::rename_branch(&git, "topic", "topic2").unwrap();

    refs::cherry_pick(&git, "topic2").unwrap();
    assert!(dir.join("t.txt").exists());
    refs::revert(&git, "HEAD").unwrap();
    assert!(!dir.join("t.txt").exists());
    refs::delete_branch(&git, "topic2", true).unwrap();

    refs::create_tag(&git, "v1", "HEAD", Some("Release 1")).unwrap();
    refs::create_tag(&git, "light", "HEAD~1", None).unwrap();
    assert!(refs::create_tag(&git, "bad tag", "HEAD", None).is_err());
    refs::delete_tag(&git, "light").unwrap();

    refs::undo_last_commit(&git).unwrap();
    let staged = Command::new("git")
        .args(["diff", "--cached", "--name-only"])
        .current_dir(&dir)
        .output()
        .unwrap();
    assert_eq!(String::from_utf8_lossy(&staged.stdout).trim(), "t.txt");
    refs::reset(&git, "HEAD", ResetMode::Hard).unwrap();
    refs::switch_detached(&git, "v1").unwrap();
}

#[test]
fn stash_push_list_apply_branch_drop() {
    use graft_core::stash::{self, Options};
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("r");
    std::fs::create_dir(&dir).unwrap();
    sh(&dir, &["init", "-q", "-b", "main"]);
    sh(&dir, &["config", "user.name", "Ada"]);
    sh(&dir, &["config", "user.email", "ada@example.com"]);
    std::fs::write(dir.join("a.txt"), "a\n").unwrap();
    sh(&dir, &["add", "."]);
    sh(&dir, &["commit", "-q", "-m", "init"]);
    let git = Git::new(&dir, Console::default());

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
