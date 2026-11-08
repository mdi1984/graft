//! Staging, unstaging and discarding lines, committing and pushing, against
//! real throwaway repositories (with core.autocrlf=true, as on Windows).

mod common;

use common::{rev, sh};
use graft_core::diff::{self, Kind, Source};
use graft_core::ops::{self, LineOp};
use graft_core::{CancelToken, Git, remote, status};

fn repo() -> (tempfile::TempDir, Git) {
    let tmp = tempfile::tempdir().unwrap();
    let dir = common::init(&tmp.path().join("r"));
    sh(&dir, &["config", "core.autocrlf", "true"]);
    common::commit(
        &dir,
        "a.txt",
        "one
two
three
four
five
",
        "init",
    );
    (tmp, common::git(&dir))
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
    assert_eq!(blob, "l1\nl3", "only the picked lines, stored with LF");

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

/// Selected lines of a new file go through git's own clean step: with
/// `* text=auto` and autocrlf off, git stores LF. A file whose filter fails
/// is not staged at all.
#[test]
fn untracked_partial_stage_follows_gitattributes() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = common::init(&tmp.path().join("r"));
    std::fs::write(
        dir.join(".gitattributes"),
        "* text=auto\n*.bad filter=bad\n",
    )
    .unwrap();
    common::commit(&dir, "a.txt", "a\n", "init");
    sh(&dir, &["add", ".gitattributes"]);
    sh(&dir, &["commit", "-q", "-m", "attributes"]);
    let git = common::git(&dir);

    std::fs::write(dir.join("new.txt"), "l1\r\nl2\r\nl3\r\n").unwrap();
    let d = diff::load(&git, &Source::Untracked, "new.txt").unwrap();
    ops::apply_lines(
        &git,
        &d,
        &Source::Untracked,
        LineOp::Stage,
        pick(&d, &["l1", "l3"]),
    )
    .unwrap();
    let blob = sh(&dir, &["cat-file", "-p", ":new.txt"]);
    assert_eq!(blob, "l1\nl3", "only the picked lines, stored with LF");
    let entry = sh(&dir, &["ls-files", "--stage", "--", "new.txt"]);
    assert!(entry.starts_with("100644 "), "{entry}");

    // A file whose filter fails has no diff to pick lines from.
    std::fs::write(dir.join("x.bad"), "x1\nx2\n").unwrap();
    sh(&dir, &["config", "filter.bad.clean", "false"]);
    sh(&dir, &["config", "filter.bad.required", "true"]);
    assert!(diff::load(&git, &Source::Untracked, "x.bad").is_err());
}

/// Picking lines of a new file works on git's form of it (here LF, and
/// UTF-8 for a UTF-16 file): staging stores exactly the picked lines, and
/// discarding writes the rest back as the file was on disk.
#[test]
fn untracked_lines_keep_the_files_form_on_disk() {
    let (_tmp, git) = repo();
    let dir = git.cwd().to_path_buf();
    std::fs::write(
        dir.join(".gitattributes"),
        "*.u16 working-tree-encoding=UTF-16LE-BOM\n",
    )
    .unwrap();
    let utf16 = |text: &str| -> Vec<u8> {
        let mut bytes = vec![0xFF, 0xFE];
        bytes.extend(text.encode_utf16().flat_map(u16::to_le_bytes));
        bytes
    };

    for (file, on_disk, staged, left) in [
        (
            "crlf.txt",
            b"l1\r\nl2\r\nl3\r\n".to_vec(),
            "l1\nl3",
            b"l1\r\nl3\r\n".to_vec(),
        ),
        (
            "text.u16",
            utf16("l1\r\nl2\r\nl3\r\n"),
            "l1\nl3",
            utf16("l1\r\nl3\r\n"),
        ),
    ] {
        std::fs::write(dir.join(file), &on_disk).unwrap();
        let d = diff::load(&git, &Source::Untracked, file).unwrap();
        assert_eq!(d.added(), 3, "{file}");
        ops::apply_lines(
            &git,
            &d,
            &Source::Untracked,
            LineOp::Stage,
            pick(&d, &["l1", "l3"]),
        )
        .unwrap();
        assert_eq!(
            sh(&dir, &["cat-file", "-p", &format!(":{file}")]),
            staged,
            "{file}"
        );
        sh(&dir, &["rm", "-q", "-f", "--cached", "--", file]);

        ops::apply_lines(
            &git,
            &d,
            &Source::Untracked,
            LineOp::Discard,
            pick(&d, &["l2"]),
        )
        .unwrap();
        assert_eq!(std::fs::read(dir.join(file)).unwrap(), left, "{file}");
    }
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
    assert_eq!(sh(&dir, &["log", "--format=%s"]), "Add b, reworded\ninit");

    // Push a branch without upstream to a bare "origin".
    let bare = common::init_bare(&tmp.path().join("origin.git"));
    sh(&dir, &["remote", "add", "origin", bare.to_str().unwrap()]);
    remote::push(&git, &CancelToken::default(), |_| {}).unwrap();
    assert_eq!(
        sh(&dir, &["rev-parse", "--abbrev-ref", "@{u}"]),
        "origin/main"
    );
    assert_eq!(rev(&bare, "main"), rev(&dir, "HEAD"));

    // Someone else pushes a commit: fetch brings it to origin/main only,
    // pull then moves main to it.
    let other = common::clone(&bare, &tmp.path().join("other"));
    common::commit(
        &other,
        "c.txt",
        "c
",
        "Add c elsewhere",
    );
    sh(&other, &["push", "-q", "origin", "main"]);
    let theirs = rev(&other, "HEAD");
    let ours = rev(&dir, "HEAD");
    remote::fetch(&git, false, &CancelToken::default(), |_| {}).unwrap();
    assert_eq!(rev(&dir, "origin/main"), theirs);
    assert_eq!(rev(&dir, "HEAD"), ours);
    remote::pull(&git, &CancelToken::default(), |_| {}).unwrap();
    assert_eq!(rev(&dir, "HEAD"), theirs);
    assert!(dir.join("c.txt").exists());
}

#[test]
fn commit_diff_and_root_commit() {
    let (_tmp, git) = repo();
    let head = rev(git.cwd(), "HEAD");
    let id = graft_core::Oid::from_hex(head.as_bytes()).unwrap();
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

/// Unchanged lines shown between hunks (or the whole file) come from git's
/// copy of the file, so staging, unstaging and discarding still apply with
/// CRLF in the working tree.
#[test]
fn expanded_diffs_stage_unstage_and_discard() {
    let (_tmp, git) = repo();
    let dir = git.cwd().to_path_buf();
    let lines = |changed: &[(usize, &str)]| -> String {
        (1..=12)
            .map(|n| {
                let text = changed
                    .iter()
                    .find(|(c, _)| *c == n)
                    .map_or(n.to_string(), |(_, t)| t.to_string());
                format!("{text}\r\n")
            })
            .collect()
    };
    std::fs::write(dir.join("n.txt"), lines(&[])).unwrap();
    sh(&dir, &["add", "n.txt"]);
    sh(&dir, &["commit", "-q", "-m", "numbers"]);
    std::fs::write(dir.join("n.txt"), lines(&[(2, "two"), (11, "eleven")])).unwrap();

    // With 3 lines of context, lines 6..8 are hidden.
    let (d, gaps) =
        diff::load_expanded(&git, &Source::Unstaged, "n.txt", |_, _| false, |_, _| false).unwrap();
    let gaps = gaps.expect("expandable");
    assert_eq!(d.hunks.len(), 2);
    assert_eq!(gaps.len(), 1);
    assert_eq!(gaps[0].lines, 6..8);

    // The whole file: stage the second hunk only.
    let (d, gaps) =
        diff::load_expanded(&git, &Source::Unstaged, "n.txt", |_, _| true, |_, _| false).unwrap();
    assert_eq!(gaps, Some(Vec::new()));
    let shown: usize = d.hunks.iter().map(|h| h.lines.len()).sum();
    assert_eq!(shown, 14);
    ops::apply_lines(&git, &d, &Source::Unstaged, LineOp::Stage, |h, _| h == 1).unwrap();
    let staged = sh(&dir, &["diff", "--cached"]);
    assert!(
        staged.contains("+eleven") && !staged.contains("+two"),
        "{staged}"
    );

    // Unstage it again from the whole staged file.
    let (d, _) =
        diff::load_expanded(&git, &Source::Staged, "n.txt", |_, _| true, |_, _| false).unwrap();
    assert_eq!(d.hunks[0].lines.len(), 13);
    ops::apply_lines(&git, &d, &Source::Staged, LineOp::Unstage, |_, _| true).unwrap();
    assert_eq!(sh(&dir, &["diff", "--cached"]), "");

    // Discard "two" from the expanded unstaged diff; CRLF stays.
    let (d, _) =
        diff::load_expanded(&git, &Source::Unstaged, "n.txt", |_, _| true, |_, _| false).unwrap();
    ops::apply_lines(&git, &d, &Source::Unstaged, LineOp::Discard, |h, _| h == 0).unwrap();
    let now = std::fs::read(dir.join("n.txt")).unwrap();
    assert_eq!(now, lines(&[(11, "eleven")]).into_bytes());

    // Commits and new files.
    sh(&dir, &["commit", "-q", "-am", "eleven"]);
    let head = rev(&dir, "HEAD");
    let id = graft_core::Oid::from_hex(head.as_bytes()).unwrap();
    let (d, gaps) = diff::load_expanded(
        &git,
        &Source::Commit(id),
        "n.txt",
        |_, _| false,
        |_, _| false,
    )
    .unwrap();
    assert_eq!(d.added(), 1);
    assert_eq!(gaps.map(|g| g.len()), Some(1));
    let (_, gaps) =
        diff::load_expanded(&git, &Source::Base(id), "n.txt", |_, _| false, |_, _| false).unwrap();
    assert_eq!(gaps, None, "no changes, nothing to expand");
    std::fs::write(dir.join("new.txt"), "fresh\n").unwrap();
    let (_, gaps) = diff::load_expanded(
        &git,
        &Source::Untracked,
        "new.txt",
        |_, _| false,
        |_, _| false,
    )
    .unwrap();
    assert_eq!(gaps, None);
}

/// A user with `log.showSignature` on still amends just the message: the
/// signature check git prints is not read as part of it.
#[test]
fn amend_message_of_a_signed_commit_has_no_signature_lines() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = common::init(&tmp.path().join("r"));
    let key = tmp.path().join("key");
    let made = std::process::Command::new("ssh-keygen")
        .args(["-q", "-t", "ed25519", "-N", "", "-C", "test", "-f"])
        .arg(&key)
        .status();
    if !made.is_ok_and(|s| s.success()) {
        eprintln!("ssh-keygen is not available; skipping");
        return;
    }
    sh(&dir, &["config", "gpg.format", "ssh"]);
    sh(&dir, &["config", "user.signingkey", key.to_str().unwrap()]);
    sh(
        &dir,
        &["commit", "-q", "-S", "--allow-empty", "-m", "Signed"],
    );
    sh(&dir, &["config", "log.showSignature", "true"]);
    let git = common::git(&dir);
    assert_eq!(
        ops::last_commit_message(&git).unwrap().as_deref(),
        Some("Signed")
    );
}

/// A file added with intent (`git add -N`) has no content in the index: it
/// is a new file. Discarding some of its lines keeps the rest, discarding
/// all of it moves it to the Recycle Bin and out of the index, never
/// emptying or deleting it for good.
#[test]
fn discarding_a_file_added_with_intent_keeps_its_content() {
    let (_tmp, git) = repo();
    let dir = git.cwd().to_path_buf();
    std::fs::write(dir.join("intent.txt"), "l1\r\nl2\r\nl3\r\n").unwrap();
    sh(&dir, &["add", "-N", "intent.txt"]);
    let s = status::load(&git).unwrap();
    let entry = s.entries.iter().find(|e| e.path == "intent.txt").unwrap();
    assert_eq!(entry.kind, graft_core::EntryKind::Untracked);
    assert!(entry.is_unstaged() && !entry.is_staged());

    let d = diff::load(&git, &Source::Untracked, "intent.txt").unwrap();
    ops::apply_lines(
        &git,
        &d,
        &Source::Untracked,
        LineOp::Discard,
        pick(&d, &["l2"]),
    )
    .unwrap();
    assert_eq!(
        std::fs::read(dir.join("intent.txt")).unwrap(),
        b"l1\r\nl3\r\n"
    );

    let d = diff::load(&git, &Source::Untracked, "intent.txt").unwrap();
    ops::apply_lines(&git, &d, &Source::Untracked, LineOp::Discard, |_, _| true).unwrap();
    assert!(!dir.join("intent.txt").exists());
    assert_eq!(sh(&dir, &["ls-files", "--", "intent.txt"]), "");
    assert!(status::load(&git).unwrap().entries.is_empty());

    // It is in the Recycle Bin: put it back.
    let here = std::fs::canonicalize(&dir).unwrap();
    let trashed: Vec<_> = trash::os_limited::list()
        .unwrap()
        .into_iter()
        .filter(|item| {
            item.name == "intent.txt"
                && std::fs::canonicalize(&item.original_parent).is_ok_and(|p| p == here)
        })
        .collect();
    assert_eq!(trashed.len(), 1);
    trash::os_limited::restore_all(trashed).unwrap();
    assert_eq!(
        std::fs::read(dir.join("intent.txt")).unwrap(),
        b"l1\r\nl3\r\n"
    );
}

/// Discarding lines of a new LF file under core.autocrlf=true writes LF
/// back, not CRLF: the file keeps the line endings it has on disk.
#[test]
fn discarding_lines_of_a_new_lf_file_keeps_lf() {
    let (_tmp, git) = repo();
    let dir = git.cwd().to_path_buf();
    std::fs::write(dir.join("lf.txt"), "l1\nl2\nl3\n").unwrap();
    let d = diff::load(&git, &Source::Untracked, "lf.txt").unwrap();
    ops::apply_lines(
        &git,
        &d,
        &Source::Untracked,
        LineOp::Discard,
        pick(&d, &["l2"]),
    )
    .unwrap();
    assert_eq!(std::fs::read(dir.join("lf.txt")).unwrap(), b"l1\nl3\n");
}

/// Lines starting with '#' are part of the message, not comments.
#[test]
fn commit_keeps_lines_starting_with_a_hash() {
    let (_tmp, git) = repo();
    let dir = git.cwd().to_path_buf();
    std::fs::write(dir.join("b.txt"), "b\n").unwrap();
    ops::stage_paths(&git, &["b.txt".into()]).unwrap();
    let message = "#42 Fix crash\n\n# Why\n\nIt crashed.";
    ops::commit(&git, message, false).unwrap();
    assert_eq!(
        ops::last_commit_message(&git).unwrap().as_deref(),
        Some(message)
    );
    ops::commit(&git, "#42 Fix crash, reworded\n\n# Why", true).unwrap();
    assert_eq!(
        sh(&dir, &["log", "-1", "--format=%B"]),
        "#42 Fix crash, reworded\n\n# Why"
    );
}
