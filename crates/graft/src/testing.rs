//! Test helpers: git in throwaway repositories, and projects opened on them.

use std::path::Path;
use std::process::Command;
use std::sync::Arc;
use std::time::{Duration, Instant};

use graft_core::JobPool;
use graft_core::console::Console;

use crate::project::Project;

/// Run git in `dir` with a test identity; it must succeed.
pub fn git(dir: &Path, args: &[&str]) {
    let status = Command::new("git")
        .args([
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "-c",
            "commit.gpgsign=false",
        ])
        .args(args)
        .current_dir(dir)
        .status()
        .unwrap();
    assert!(status.success(), "git {args:?}");
}

/// Poll the project until `done` holds; fail with `what` after 20 seconds.
pub fn wait(project: &mut Project, what: &str, done: impl Fn(&Project) -> bool) {
    let start = Instant::now();
    loop {
        project.poll();
        if done(project) {
            return;
        }
        assert!(start.elapsed() < Duration::from_secs(20), "{what}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// Open the repository at `root` as a project, once its snapshot is loaded.
pub fn open(root: &Path) -> Project {
    let pool = JobPool::new(2, Arc::new(|| {}));
    let mut project = Project::new(
        root.to_path_buf(),
        root.to_path_buf(),
        Default::default(),
        Default::default(),
        Console::default(),
        pool,
    );
    wait(&mut project, "snapshot never came", |p| {
        p.snapshot.is_some()
    });
    project
}
