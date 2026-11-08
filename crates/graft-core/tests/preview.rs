//! Preview in main against a real repository with a second worktree.

mod common;

use std::path::{Path, PathBuf};

use common::sh;
use graft_core::preview::Changed;
use graft_core::worktree::{self, Start};
use graft_core::{Git, Restore, preview};

#[test]
fn start_update_and_restore() {
    let tmp = tempfile::tempdir().unwrap();
    let main = common::init(&tmp.path().join("app"));
    sh(&main, &["config", "core.autocrlf", "true"]);
    std::fs::write(main.join("a.txt"), "one\r\ntwo\r\n").unwrap();
    sh(&main, &["add", "."]);
    sh(&main, &["commit", "-q", "-m", "a"]);

    let git = common::git(&main);
    let agent = tmp.path().join("agent");
    worktree::add(
        &git,
        &agent,
        &Start::NewBranch {
            name: "agent/x".into(),
            base: "main".into(),
        },
    )
    .unwrap();
    std::fs::write(agent.join("b.txt"), "committed\r\n").unwrap();
    sh(&agent, &["add", "."]);
    sh(&agent, &["commit", "-q", "-m", "b"]);
    let agent_head = sh(&agent, &["rev-parse", "HEAD"]);
    // Uncommitted: a modified file, a staged new file, an untracked folder.
    std::fs::write(agent.join("a.txt"), "one\r\ntwo\r\nthree\r\n").unwrap();
    std::fs::write(agent.join("staged.txt"), "staged\r\n").unwrap();
    sh(&agent, &["add", "staged.txt"]);
    std::fs::create_dir(agent.join("new")).unwrap();
    std::fs::write(agent.join("new").join("c.txt"), "untracked\r\n").unwrap();
    let agent_status = sh(&agent, &["status", "--porcelain"]);

    // Main must be clean.
    std::fs::write(main.join("a.txt"), "dirty\r\n").unwrap();
    let source = git.at(&agent);
    assert!(preview::start(&git, &source).is_err());
    sh(&main, &["checkout", "--", "a.txt"]);

    let shown = preview::start(&git, &source).unwrap();
    assert_eq!(shown.restore, Restore::Branch("main".into()));
    assert_eq!(sh(&main, &["rev-parse", "HEAD"]), agent_head);
    assert_eq!(
        std::fs::read(main.join("a.txt")).unwrap(),
        b"one\r\ntwo\r\nthree\r\n"
    );
    assert_eq!(std::fs::read(main.join("b.txt")).unwrap(), b"committed\r\n");
    assert!(main.join("staged.txt").is_file());
    assert!(main.join("new").join("c.txt").is_file());
    assert_eq!(preview::active(&main), Some(shown));
    // The agent's worktree and staging are untouched.
    assert_eq!(sh(&agent, &["status", "--porcelain"]), agent_status);
    assert!(preview::start(&git, &source).is_err());

    // The agent keeps working; update shows the new state.
    std::fs::remove_file(agent.join("new").join("c.txt")).unwrap();
    std::fs::write(agent.join("d.txt"), "later\r\n").unwrap();
    preview::update(&git).unwrap();
    assert!(!main.join("new").exists());
    assert!(main.join("d.txt").is_file());

    preview::restore_main(&git).unwrap();
    assert_eq!(sh(&main, &["symbolic-ref", "--short", "HEAD"]), "main");
    assert_eq!(sh(&main, &["status", "--porcelain"]), "");
    assert!(!main.join("d.txt").exists());
    assert!(!main.join("b.txt").exists());
    assert_eq!(preview::active(&main), None);
}

/// Main with one commit and a worktree `agent` on its own branch.
fn main_and_agent(tmp: &Path, gitignore: &str) -> (Git, PathBuf) {
    let main = common::init(&tmp.join("app"));
    common::commit(&main, ".gitignore", gitignore, "init");
    let git = common::git(&main);
    let agent = tmp.join("agent");
    worktree::add(
        &git,
        &agent,
        &Start::NewBranch {
            name: "agent/x".into(),
            base: "main".into(),
        },
    )
    .unwrap();
    (git, agent)
}

/// Restore main goes back to main also when a tag is named main too.
#[test]
fn a_tag_named_like_the_branch_does_not_confuse_restore() {
    let tmp = tempfile::tempdir().unwrap();
    let (git, agent) = main_and_agent(tmp.path(), "");
    sh(git.cwd(), &["tag", "main"]);
    std::fs::write(agent.join("new.txt"), "agent work\n").unwrap();

    let shown = preview::start(&git, &git.at(&agent)).unwrap();
    assert_eq!(shown.restore, Restore::Branch("main".into()));
    preview::restore_main(&git).unwrap();
    assert_eq!(sh(git.cwd(), &["symbolic-ref", "HEAD"]), "refs/heads/main");
    assert_eq!(sh(git.cwd(), &["status", "--porcelain"]), "");
}

/// The marker lives in `.git`, where a repository's tooling can write: paths
/// in it that point outside main are never deleted by Restore main.
#[test]
fn restore_never_deletes_files_outside_main() {
    let tmp = tempfile::tempdir().unwrap();
    let (git, agent) = main_and_agent(tmp.path(), "");
    std::fs::write(agent.join("new.txt"), "agent work\n").unwrap();
    preview::start(&git, &git.at(&agent)).unwrap();

    let victim = tmp.path().join("victim.txt");
    std::fs::write(&victim, "keep\n").unwrap();
    let marker = git.cwd().join(".git").join("graft-preview");
    let mut text = std::fs::read_to_string(&marker).unwrap();
    text.push_str("added ../victim.txt\n");
    text.push_str(&format!("added {}\n", victim.display()));
    std::fs::write(&marker, text).unwrap();
    let shown = preview::active(git.cwd()).expect("still a preview");
    assert_eq!(shown.added, ["new.txt"]);

    preview::restore_main(&git).unwrap();
    assert!(victim.exists());
    assert!(!git.cwd().join("new.txt").exists());
    assert_eq!(sh(git.cwd(), &["status", "--porcelain"]), "");
}

/// A preview whose changes cannot be applied in main: main has an ignored
/// `local.cfg`; the agent un-ignores it and makes its own, so applying that
/// new file in main fails. Returns main's path.
fn failed_preview(tmp: &Path) -> PathBuf {
    let (git, agent) = main_and_agent(tmp, "local.cfg\n");
    let main = git.cwd().to_path_buf();
    std::fs::write(main.join("local.cfg"), "mine\n").unwrap();
    std::fs::write(agent.join(".gitignore"), "").unwrap();
    std::fs::write(agent.join("local.cfg"), "agent\n").unwrap();

    let err = preview::start(&git, &git.at(&agent)).unwrap_err();
    assert!(
        format!("{err:#}").contains("could not be applied"),
        "{err:#}"
    );
    main
}

/// When the worktree's changes cannot be applied, main goes back to its
/// branch and shows no preview.
#[test]
fn a_failed_apply_puts_main_back() {
    let tmp = tempfile::tempdir().unwrap();
    let main = failed_preview(tmp.path());
    assert_eq!(preview::active(&main), None);
    assert_eq!(sh(&main, &["symbolic-ref", "--short", "HEAD"]), "main");
    assert_eq!(sh(&main, &["status", "--porcelain"]), "");
}

/// Going back must not delete the user's own ignored file that was in the
/// way: the rollback removes only files the preview created.
#[test]
fn a_failed_apply_keeps_main_files() {
    let tmp = tempfile::tempdir().unwrap();
    let main = failed_preview(tmp.path());
    assert_eq!(
        std::fs::read_to_string(main.join("local.cfg"))
            .ok()
            .as_deref(),
        Some("mine\n")
    );
}

/// A user with `diff.noprefix` (or `diff.mnemonicPrefix`) still gets a
/// preview: the patch Graft makes keeps the prefixes `git apply` expects.
#[test]
fn preview_works_with_diff_prefix_settings() {
    for (key, value) in [("diff.noprefix", "true"), ("diff.mnemonicPrefix", "true")] {
        let tmp = tempfile::tempdir().unwrap();
        let main = common::init(&tmp.path().join("app"));
        sh(&main, &["config", key, value]);
        common::commit(&main, "a.txt", "one\n", "a");
        let git = common::git(&main);
        let agent = tmp.path().join("agent");
        worktree::add(
            &git,
            &agent,
            &Start::NewBranch {
                name: "agent/x".into(),
                base: "main".into(),
            },
        )
        .unwrap();
        std::fs::write(agent.join("a.txt"), "one\ntwo\n").unwrap();

        preview::start(&git, &git.at(&agent)).unwrap();
        assert_eq!(
            std::fs::read_to_string(main.join("a.txt")).unwrap(),
            "one\ntwo\n",
            "{key}"
        );
        preview::restore_main(&git).unwrap();
    }
}

/// Main as the preview left it can be restored without asking; an edit, a
/// staged new file or a deleted file in main since is noticed (leaving
/// would lose it), and so is a marker from an older Graft, which never
/// recorded what the preview left. A new file in main that leaving keeps
/// is not a change.
#[test]
fn changes_in_main_since_the_preview_are_noticed() {
    let tmp = tempfile::tempdir().unwrap();
    let (git, agent) = main_and_agent(tmp.path(), "build/\n");
    let main = git.cwd().to_path_buf();
    std::fs::write(agent.join("a.txt"), "agent\n").unwrap();
    std::fs::write(agent.join("new.txt"), "agent's new file\n").unwrap();
    preview::start(&git, &git.at(&agent)).unwrap();
    assert_eq!(preview::main_changed(&git).unwrap(), None);

    // Ignored files (build output) are not changes.
    std::fs::create_dir(main.join("build")).unwrap();
    std::fs::write(main.join("build").join("app.exe"), "binary").unwrap();
    assert_eq!(preview::main_changed(&git).unwrap(), None);

    std::fs::write(main.join("a.txt"), "edited in main\n").unwrap();
    assert_eq!(preview::main_changed(&git).unwrap(), Some(Changed::Files));
    std::fs::write(main.join("a.txt"), "agent\n").unwrap();
    assert_eq!(preview::main_changed(&git).unwrap(), None);
    std::fs::write(main.join("mine.txt"), "new in main\n").unwrap();
    assert_eq!(preview::main_changed(&git).unwrap(), None);
    sh(&main, &["add", "mine.txt"]);
    assert_eq!(preview::main_changed(&git).unwrap(), Some(Changed::Files));
    sh(&main, &["rm", "-q", "--cached", "mine.txt"]);
    assert_eq!(preview::main_changed(&git).unwrap(), None);
    std::fs::write(main.join("new.txt"), "edited in main\n").unwrap();
    assert_eq!(preview::main_changed(&git).unwrap(), Some(Changed::Files));
    std::fs::remove_file(main.join("new.txt")).unwrap();
    assert_eq!(preview::main_changed(&git).unwrap(), Some(Changed::Files));

    // Update shows the agent's state again: unchanged from there.
    preview::update(&git).unwrap();
    assert_eq!(preview::main_changed(&git).unwrap(), None);

    let marker = main.join(".git").join("graft-preview");
    let text = std::fs::read_to_string(&marker).unwrap();
    let old: String = text
        .lines()
        .filter(|l| !l.starts_with("shown "))
        .map(|l| format!("{l}\n"))
        .collect();
    assert_ne!(old, text);
    std::fs::write(&marker, old).unwrap();
    assert_eq!(preview::main_changed(&git).unwrap(), Some(Changed::Files));
    preview::restore_main(&git).unwrap();
    assert!(preview::main_changed(&git).is_err(), "no preview");
}

/// A worktree's change to a file with a textconv driver shows in main as
/// the stored bytes.
#[test]
fn a_file_with_a_textconv_driver_is_previewed() {
    let tmp = tempfile::tempdir().unwrap();
    let (git, agent) = main_and_agent(tmp.path(), "");
    let main = git.cwd().to_path_buf();
    common::upper_textconv(&main);
    common::commit(&agent, "a.dat", "one\ntwo\n", "dat");
    std::fs::write(agent.join("a.dat"), "one\nthree\n").unwrap();
    preview::start(&git, &git.at(&agent)).unwrap();
    assert_eq!(std::fs::read(main.join("a.dat")).unwrap(), b"one\nthree\n");
    preview::restore_main(&git).unwrap();
}

/// A commit made in main during the preview moves its HEAD: leaving would
/// leave the commit behind, even with main's files as the preview left them.
#[test]
fn a_commit_in_main_since_the_preview_is_noticed() {
    let tmp = tempfile::tempdir().unwrap();
    let (git, agent) = main_and_agent(tmp.path(), "build/\n");
    let main = git.cwd().to_path_buf();
    std::fs::write(agent.join("a.txt"), "agent\n").unwrap();
    preview::start(&git, &git.at(&agent)).unwrap();
    sh(&main, &["add", "-A"]);
    sh(&main, &["commit", "-q", "-m", "made in main"]);
    assert_eq!(preview::main_changed(&git).unwrap(), Some(Changed::Head));
    preview::restore_main(&git).unwrap();
}

/// A file in main that git cannot read (locked by an app running from
/// main) leaves it unknown whether leaving would lose changes: that counts
/// as changed. A locked new file that leaving keeps is never read.
#[cfg(windows)]
#[test]
fn a_file_git_cannot_read_counts_as_changed() {
    use std::os::windows::fs::OpenOptionsExt;
    let tmp = tempfile::tempdir().unwrap();
    let (git, agent) = main_and_agent(tmp.path(), "build/\n");
    let main = git.cwd().to_path_buf();
    std::fs::write(agent.join("a.txt"), "agent\n").unwrap();
    preview::start(&git, &git.at(&agent)).unwrap();
    let lock = |path: &Path| {
        std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .share_mode(0)
            .open(path)
            .unwrap()
    };

    let db = lock(&main.join("app.db"));
    assert_eq!(preview::main_changed(&git).unwrap(), None);

    // Same content, but git has to read it again to know.
    let later = std::time::SystemTime::now() + std::time::Duration::from_secs(60);
    let tracked = main.join(".gitignore");
    let file = std::fs::OpenOptions::new()
        .write(true)
        .open(&tracked)
        .unwrap();
    file.set_modified(later).unwrap();
    drop(file);
    let locked = lock(&tracked);
    assert_eq!(preview::main_changed(&git).unwrap(), Some(Changed::Files));
    drop((db, locked));
    assert_eq!(preview::main_changed(&git).unwrap(), None);
    preview::restore_main(&git).unwrap();
}
