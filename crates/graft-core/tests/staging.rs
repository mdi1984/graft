//! Staging, unstaging and discarding lines, committing and pushing, against
//! real throwaway repositories (with core.autocrlf=true, as on Windows).

use std::path::Path;
use std::process::Command;

use graft_core::diff::{self, Kind, Source};
use graft_core::ops::{self, LineOp};
use graft_core::{CancelToken, Console, Git, remote, status};

fn sh(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_AUTHOR_NAME", "Ada")
        .env("GIT_AUTHOR_EMAIL", "ada@example.com")
        .env("GIT_COMMITTER_NAME", "Ada")
        .env("GIT_COMMITTER_EMAIL", "ada@example.com")
        .output()
        .expect("git runs");
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn repo() -> (tempfile::TempDir, Git) {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("r");
    std::fs::create_dir(&dir).unwrap();
    sh(&dir, &["init", "-q", "-b", "main"]);
    sh(&dir, &["config", "core.autocrlf", "true"]);
    sh(&dir, &["config", "user.name", "Ada"]);
    sh(&dir, &["config", "user.email", "ada@example.com"]);
    std::fs::write(dir.join("a.txt"), "one\r\ntwo\r\nthree\r\nfour\r\nfive\r\n").unwrap();
    sh(&dir, &["add", "a.txt"]);
    sh(&dir, &["commit", "-q", "-m", "init"]);
    let git = Git::new(&dir, Console::default());
    (tmp, git)
}

/// Indices of the changed lines whose text is in `texts`.
fn pick<'a>(d: &'a diff::FileDiff, texts: &'a [&str]) -> impl Fn(usize, usize) -> bool + 'a {
    move |h, l| {
        let line = &d.hunks[h].lines[l];
        line.kind != Kind::Context && texts.contains(&line.text().as_str())
    }
}

#[test]
fn stage_unstage_and_discard_lines_with_crlf() {
    let (_tmp, git) = repo();
    let dir = git.cwd().to_path_buf();
    // Change line 2 and line 4, the way a Windows editor writes files.
    std::fs::write(dir.join("a.txt"), "one\r\nTWO\r\nthree\r\nFOUR\r\nfive\r\n").unwrap();

    let d = diff::load(&git, &Source::Unstaged, "a.txt").unwrap();
    assert_eq!((d.added(), d.removed()), (2, 2));
    ops::apply_lines(
        &git,
        &d,
        &Source::Unstaged,
        LineOp::Stage,
        pick(&d, &["two", "TWO"]),
    )
    .unwrap();

    let staged = sh(&dir, &["diff", "--cached"]);
    assert!(staged.contains("-two") && staged.contains("+TWO"));
    assert!(!staged.contains("FOUR"), "{staged}");
    let unstaged = sh(&dir, &["diff"]);
    assert!(unstaged.contains("+FOUR") && !unstaged.contains("+TWO"));

    // Unstage it again.
    let d = diff::load(&git, &Source::Staged, "a.txt").unwrap();
    ops::apply_lines(&git, &d, &Source::Staged, LineOp::Unstage, |_, _| true).unwrap();
    assert!(sh(&dir, &["diff", "--cached"]).is_empty());

    // Discard only the change to line 4; line 2 stays changed, CRLF stays.
    let d = diff::load(&git, &Source::Unstaged, "a.txt").unwrap();
    ops::apply_lines(
        &git,
        &d,
        &Source::Unstaged,
        LineOp::Discard,
        pick(&d, &["four", "FOUR"]),
    )
    .unwrap();
    let content = std::fs::read_to_string(dir.join("a.txt")).unwrap();
    assert_eq!(content, "one\r\nTWO\r\nthree\r\nfour\r\nfive\r\n");
}

#[test]
fn untracked_partial_stage_and_whole_files() {
    let (_tmp, git) = repo();
    let dir = git.cwd().to_path_buf();
    std::fs::write(dir.join("new.txt"), "l1\r\nl2\r\nl3\r\n").unwrap();

    let d = diff::load(&git, &Source::Untracked, "new.txt").unwrap();
    assert!(d.new_file);
    assert_eq!(d.added(), 3);
    ops::apply_lines(
        &git,
        &d,
        &Source::Untracked,
        LineOp::Stage,
        pick(&d, &["l1", "l3"]),
    )
    .unwrap();
    let blob = sh(&dir, &["show", ":new.txt"]);
    assert_eq!(blob, "l1\nl3\n");

    // Whole-file unstage, stage and discard.
    ops::unstage_paths(&git, &["new.txt".into()]).unwrap();
    let s = status::load(&git).unwrap();
    assert!(
        s.entries
            .iter()
            .any(|e| e.path == "new.txt" && !e.is_staged())
    );
    std::fs::write(dir.join("a.txt"), "changed\r\n").unwrap();
    ops::stage_paths(&git, &["a.txt".into()]).unwrap();
    assert!(sh(&dir, &["diff", "--cached", "--name-only"]).contains("a.txt"));
    ops::unstage_paths(&git, &["a.txt".into()]).unwrap();
    ops::discard_paths(&git, &["a.txt".into()], &[]).unwrap();
    assert_eq!(
        std::fs::read_to_string(dir.join("a.txt")).unwrap(),
        "one\r\ntwo\r\nthree\r\nfour\r\nfive\r\n"
    );
}

#[test]
fn commit_amend_and_push() {
    let (tmp, git) = repo();
    let dir = git.cwd().to_path_buf();
    std::fs::write(dir.join("b.txt"), "b\n").unwrap();
    ops::stage_paths(&git, &["b.txt".into()]).unwrap();
    let summary = ops::commit(&git, "Add b\n\nWith a body.", false).unwrap();
    assert!(summary.contains("Add b"), "{summary}");
    assert_eq!(
        ops::last_commit_message(&git).unwrap().as_deref(),
        Some("Add b\n\nWith a body.")
    );
    ops::commit(&git, "Add b, reworded", true).unwrap();
    assert_eq!(sh(&dir, &["log", "--format=%s"]), "Add b, reworded\ninit\n");

    // Push a branch without upstream to a bare "origin".
    let bare = tmp.path().join("origin.git");
    sh(
        tmp.path(),
        &["init", "-q", "--bare", bare.to_str().unwrap()],
    );
    sh(&dir, &["remote", "add", "origin", bare.to_str().unwrap()]);
    remote::push(&git, &CancelToken::default(), |_| {}).unwrap();
    assert_eq!(
        sh(&dir, &["rev-parse", "--abbrev-ref", "@{u}"]).trim(),
        "origin/main"
    );
    remote::fetch(&git, &CancelToken::default(), |_| {}).unwrap();
    remote::pull(&git, &CancelToken::default(), |_| {}).unwrap();
}

#[test]
fn commit_diff_and_root_commit() {
    let (_tmp, git) = repo();
    let head = sh(git.cwd(), &["rev-parse", "HEAD"]);
    let id = graft_core::Oid::from_hex(head.trim().as_bytes()).unwrap();
    let d = diff::load(&git, &Source::Commit(id), "a.txt").unwrap();
    assert!(d.new_file);
    assert_eq!(d.added(), 5);
}

/// File names are never patterns: discarding `x[ab].txt` leaves `xa.txt` alone.
#[test]
fn discarding_a_file_with_glob_characters_touches_only_that_file() {
    let (_tmp, git) = repo();
    let dir = git.cwd().to_path_buf();
    std::fs::write(dir.join("x[ab].txt"), "one\n").unwrap();
    std::fs::write(dir.join("xa.txt"), "one\n").unwrap();
    sh(&dir, &["add", "."]);
    sh(&dir, &["commit", "-q", "-m", "two files"]);
    std::fs::write(dir.join("x[ab].txt"), "changed\n").unwrap();
    std::fs::write(dir.join("xa.txt"), "keep me\n").unwrap();

    let d = diff::load(&git, &Source::Unstaged, "x[ab].txt").unwrap();
    assert_eq!(d.path, "x[ab].txt");
    let texts: Vec<String> = d
        .hunks
        .iter()
        .flat_map(|h| &h.lines)
        .map(|l| l.text())
        .collect();
    assert!(texts.contains(&"changed".to_owned()), "{texts:?}");
    assert!(!texts.contains(&"keep me".to_owned()), "{texts:?}");

    ops::discard_paths(&git, &["x[ab].txt".to_owned()], &[]).unwrap();
    let read = |name: &str| std::fs::read_to_string(dir.join(name)).unwrap();
    assert_eq!(read("x[ab].txt").trim(), "one");
    assert_eq!(read("xa.txt").trim(), "keep me");
}
