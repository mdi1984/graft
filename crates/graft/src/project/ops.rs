//! Operations: git commands that change things. One runs at a time; when it
//! ends the project refreshes and the operation's own handler says what
//! happened.

use std::path::PathBuf;

use graft_core::refusal::{no_identity, no_remote};
use graft_core::{CancelToken, Git, Progress, is_cancelled, is_panic};

use super::Project;
use crate::jobs::JobResult;
use crate::running::Running;

/// Where an operation runs.
pub(super) enum At {
    /// The selected worktree.
    Selected,
    /// The main folder (the repository's own working tree).
    Main,
    Path(PathBuf),
}

impl Project {
    /// Git pointed at the selected worktree.
    pub(super) fn wt_git(&self) -> Git {
        self.git.at(&self.nav.worktree)
    }

    fn git_at(&self, at: At) -> Git {
        match at {
            At::Selected => self.wt_git(),
            At::Main => self.git.clone(),
            At::Path(path) => self.git.at(&path),
        }
    }

    pub fn is_operating(&self) -> bool {
        self.op.is_some()
    }

    /// Whether a new operation could start now, or why not.
    pub fn can_act(&self) -> Result<(), &'static str> {
        if self.snapshot.is_none() {
            Err("Still loading the repository")
        } else if let Some(op) = &self.op {
            Err(if op.can_cancel() {
                "Wait for the running operation to finish, or cancel it"
            } else {
                "Wait for the running operation to finish"
            })
        } else {
            Ok(())
        }
    }

    /// The running operation's progress, while it has any to show.
    pub fn op_progress(&self) -> Option<Progress> {
        self.op.as_ref().map(Running::progress)
    }

    /// Run a quick git command `at` a worktree, then refresh and hand the
    /// result to `done`. False (and nothing runs) while another operation
    /// runs.
    pub(super) fn run_op<R: JobResult>(
        &mut self,
        at: At,
        label: &str,
        job: impl FnOnce(Git) -> R + Send + 'static,
        done: impl FnOnce(&mut Project, R) + Send + 'static,
    ) -> bool {
        if self.op.is_some() {
            return false;
        }
        let git = self.git_at(at);
        self.op = Some(Running::new(format!("{label}…")));
        self.jobs
            .spawn(move || job(git), |p, result| p.end_op(result, done));
        true
    }

    /// Like [`Self::run_op`], for commands that take a while (fetch, push,
    /// Slash): they report progress and can be cancelled.
    pub(super) fn run_long<R: JobResult>(
        &mut self,
        at: At,
        label: &str,
        job: impl FnOnce(Git, &CancelToken, &dyn Fn(Progress)) -> R + Send + 'static,
        done: impl FnOnce(&mut Project, R) + Send + 'static,
    ) -> bool {
        if self.op.is_some() {
            return false;
        }
        let git = self.git_at(at);
        let (running, cancel) = Running::cancellable(format!("{label}…"));
        let report = running.reporter(self.jobs.waker());
        self.op = Some(running);
        self.jobs.spawn_long(
            move |_| job(git, &cancel, &report),
            |p, result| p.end_op(result, done),
        );
        true
    }

    fn end_op<R>(&mut self, result: R, done: impl FnOnce(&mut Project, R)) {
        self.op = None;
        self.refresh();
        done(self, result);
    }

    pub(super) fn cancel_op(&self) {
        if let Some(op) = &self.op {
            op.cancel();
        }
    }

    /// Say why something failed, in words the user can act on. A cancel is
    /// no failure; a panic was announced when it happened.
    pub(super) fn report(&mut self, error: anyhow::Error) {
        if is_cancelled(&error) || is_panic(&error) {
            return;
        }
        let text = format!("{error:#}");
        self.notices.push(if no_remote(&text) {
            "This repository has no remote yet. Add one under REMOTES in the sidebar \
             (Add remote…)."
                .to_owned()
        } else if no_identity(&text) {
            "Git doesn't know who you are yet. Set your name and email in Settings \
             (the gear at the top right)."
                .to_owned()
        } else {
            text
        });
    }
}

/// The usual ending of an operation: nothing to say, or why it failed.
pub(super) fn quietly(p: &mut Project, result: anyhow::Result<()>) {
    if let Err(e) = result {
        p.report(e);
    }
}

/// The usual ending of an operation: say `done`, or why it failed.
pub(super) fn say(done: String) -> impl FnOnce(&mut Project, anyhow::Result<()>) + Send {
    move |p, result| match result {
        Ok(()) => p.infos.push(done),
        Err(e) => p.report(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{git, open, wait};

    fn repo() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        git(dir.path(), &["init", "-q", "-b", "main"]);
        std::fs::write(dir.path().join("a.txt"), "a\n").unwrap();
        git(dir.path(), &["add", "a.txt"]);
        git(dir.path(), &["commit", "-q", "-m", "first"]);
        dir
    }

    #[test]
    fn a_panicking_operation_ends_itself_and_is_announced_once() {
        let dir = repo();
        let mut project = open(dir.path());
        let started = project.run_op(
            At::Main,
            "Exploding",
            |_| -> anyhow::Result<()> { panic!("boom") },
            quietly,
        );
        assert!(started);
        wait(&mut project, "the operation never ended", |p| {
            !p.is_operating()
        });
        assert_eq!(project.notices.len(), 1, "{:?}", project.notices);
        assert!(project.notices[0].contains("boom"));
    }

    #[test]
    fn another_jobs_panic_leaves_the_running_operation_alone() {
        let dir = repo();
        let mut project = open(dir.path());
        let (release, released) = crossbeam_channel::bounded::<()>(1);
        project.run_op(
            At::Main,
            "Waiting",
            move |_| -> anyhow::Result<()> {
                let _ = released.recv();
                Ok(())
            },
            quietly,
        );
        project
            .jobs
            .spawn(|| -> Option<()> { panic!("elsewhere") }, |_, _| {});
        wait(&mut project, "the panic was never announced", |p| {
            !p.notices.is_empty()
        });
        assert!(project.is_operating(), "the operation still runs");
        release.send(()).unwrap();
        wait(&mut project, "the operation never ended", |p| {
            !p.is_operating()
        });
        assert_eq!(project.notices.len(), 1);
    }
}
