//! Throwaway repositories for the integration tests.
//!
//! Fixtures are built with [`sh`], which runs git with a fixed identity and
//! without the user's system or global config. Each repository from
//! [`init`] also gets local settings that override anything global the
//! engine's own git calls would otherwise pick up (hooks, signing, global
//! ignores, autocrlf), so the user's setup cannot change a test's outcome.

#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

use graft_core::Git;
use graft_core::console::Console;

/// An empty folder and an empty file outside any repository, shared by all
/// tests of a run: the global config and hooks path git gets.
fn empty() -> &'static (PathBuf, PathBuf) {
    static EMPTY: OnceLock<(PathBuf, PathBuf)> = OnceLock::new();
    EMPTY.get_or_init(|| {
        let dir = tempfile::tempdir().unwrap().keep();
        let file = dir.join("empty-gitconfig");
        std::fs::write(&file, "").unwrap();
        let hooks = dir.join("no-hooks");
        std::fs::create_dir(&hooks).unwrap();
        (file, hooks)
    })
}

/// Run git in `dir` to build or inspect a fixture; panics with git's
/// message when it fails. Returns stdout without trailing whitespace.
pub fn sh(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_AUTHOR_NAME", "Ada")
        .env("GIT_AUTHOR_EMAIL", "ada@example.com")
        .env("GIT_COMMITTER_NAME", "Ada")
        .env("GIT_COMMITTER_EMAIL", "ada@example.com")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", &empty().0)
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .output()
        .expect("git runs");
    assert!(
        out.status.success(),
        "git {args:?} in {} failed: {}",
        dir.display(),
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim_end().to_owned()
}

/// Local settings that win over the user's global config for every git
/// command run in the repository, including the engine's own.
fn isolate(dir: &Path) {
    let (file, hooks) = empty();
    let file = file.to_str().unwrap();
    let hooks = hooks.to_str().unwrap();
    for (key, value) in [
        ("user.name", "Ada"),
        ("user.email", "ada@example.com"),
        ("commit.gpgsign", "false"),
        ("tag.gpgsign", "false"),
        ("core.hooksPath", hooks),
        ("core.excludesFile", file),
        ("core.autocrlf", "false"),
    ] {
        sh(dir, &["config", key, value]);
    }
}

/// A new repository on `main` at `dir` (created if missing), with no commits.
pub fn init(dir: &Path) -> PathBuf {
    std::fs::create_dir_all(dir).unwrap();
    sh(dir, &["init", "-q", "-b", "main"]);
    isolate(dir);
    dir.to_path_buf()
}

/// A new bare repository at `dir`, e.g. to push to as `origin`.
pub fn init_bare(dir: &Path) -> PathBuf {
    std::fs::create_dir_all(dir).unwrap();
    sh(dir, &["init", "-q", "--bare", "-b", "main"]);
    isolate(dir);
    dir.to_path_buf()
}

/// A clone of `url` at `dest`, set up like [`init`].
pub fn clone(url: &Path, dest: &Path) -> PathBuf {
    let parent = dest.parent().unwrap();
    sh(
        parent,
        &["clone", "-q", url.to_str().unwrap(), dest.to_str().unwrap()],
    );
    isolate(dest);
    dest.to_path_buf()
}

/// Write `file` (relative to `dir`) and commit it with `message`.
pub fn commit(dir: &Path, file: &str, content: &str, message: &str) {
    let path = dir.join(file);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(path, content).unwrap();
    sh(dir, &["add", "--", file]);
    sh(dir, &["commit", "-q", "-m", message]);
}

/// The engine's handle on `dir`, with a fresh console.
pub fn git(dir: &Path) -> Git {
    Git::new(dir, Console::default())
}

/// `rev` in `dir` as a full id.
pub fn rev(dir: &Path, rev: &str) -> String {
    sh(dir, &["rev-parse", "--verify", rev])
}
