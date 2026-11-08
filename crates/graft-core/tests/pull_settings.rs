//! Pull of a diverged branch without the system and global config, as in
//! WSL, where git has no pull.rebase default like Git for Windows'. A test
//! binary of its own: it changes the environment the engine's git sees.

mod common;

use std::path::Path;

use common::sh;
use graft_core::{CancelToken, remote};

/// `mine` and its upstream both have a commit the other lacks.
fn diverged(tmp: &Path) -> std::path::PathBuf {
    let server = common::init_bare(&tmp.join("server.git"));
    let dir = common::init(&tmp.join("app"));
    common::commit(&dir, "a.txt", "a\n", "a");
    sh(&dir, &["push", "-q", server.to_str().unwrap(), "main"]);
    let mine = common::clone(&server, &tmp.join("mine"));
    let theirs = common::clone(&server, &tmp.join("theirs"));
    common::commit(&theirs, "a.txt", "theirs\n", "theirs");
    sh(&theirs, &["push", "-q"]);
    common::commit(&mine, "b.txt", "mine\n", "mine");
    mine
}

/// With nothing set, a pull merges (git's old default) instead of git's
/// "Need to specify how to reconcile divergent branches"; whatever the user
/// set still decides.
#[test]
fn a_pull_without_settings_merges_and_settings_still_apply() {
    let tmp = tempfile::tempdir().unwrap();
    let global = tmp.path().join("empty-gitconfig");
    std::fs::write(&global, "").unwrap();
    // SAFETY: the only test in this binary, set before any thread starts.
    unsafe {
        std::env::set_var("GIT_CONFIG_NOSYSTEM", "1");
        std::env::set_var("GIT_CONFIG_GLOBAL", &global);
    }

    for setting in [
        None,
        Some(("pull.rebase", "true")),
        Some(("branch.main.rebase", "true")),
        Some(("pull.ff", "only")),
    ] {
        let case = tmp.path().join(setting.map_or("none", |(key, _)| key));
        let mine = diverged(&case);
        if let Some((key, value)) = setting {
            sh(&mine, &["config", key, value]);
        }
        let git = common::git(&mine);
        let pulled = remote::pull(&git, &CancelToken::default(), |_| {});
        if setting == Some(("pull.ff", "only")) {
            let error = format!("{:#}", pulled.unwrap_err());
            assert!(error.contains("fast-forward"), "{error}");
            continue;
        }
        assert!(!pulled.unwrap(), "{setting:?}: stopped");
        let parents = sh(&mine, &["log", "-1", "--format=%P"]);
        let merged = parents.split(' ').count() == 2;
        assert_eq!(merged, setting.is_none(), "{setting:?}: {parents}");
        assert_eq!(sh(&mine, &["show", "HEAD:a.txt"]), "theirs");
        assert_eq!(sh(&mine, &["show", "HEAD:b.txt"]), "mine");
    }
}
