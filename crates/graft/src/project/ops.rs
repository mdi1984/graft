//! Operations: git commands that change things. One runs at a time; when it
//! ends the project refreshes and the operation's own handler says what
//! happened.

use std::path::{Path, PathBuf};

use graft_core::{CancelToken, Git, Progress};

use super::Project;
use crate::jobs::{JobResult, shown_error};
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

    /// The folder `at` names now.
    fn path_of(&self, at: At) -> PathBuf {
        match at {
            At::Selected => self.nav.worktree.clone(),
            At::Main => self.main_path(),
            At::Path(path) => path,
        }
    }

    pub fn is_operating(&self) -> bool {
        self.op.is_some()
    }

    /// Whether a new operation could start now, or why not.
    pub fn can_act(&self) -> Result<(), &'static str> {
        if self.snapshot.is_none() {
            Err("Still loading the repository")
        } else {
            self.busy().map_or(Ok(()), Err)
        }
    }

    /// Why no operation can start while one runs.
    fn busy(&self) -> Option<&'static str> {
        self.op.as_ref().map(|op| {
            if op.can_cancel() {
                "Wait for the running operation to finish, or cancel it"
            } else {
                "Wait for the running operation to finish"
            }
        })
    }

    /// False, after saying why, if `label` cannot start because another
    /// operation runs.
    fn may_start(&mut self, label: &str) -> bool {
        let Some(reason) = self.busy() else {
            return true;
        };
        self.notices
            .push(format!("{label} did not start. {reason}."));
        false
    }

    /// False, after saying why, while an operation runs: closing the tab
    /// would drop what it does when it ends (push after commit, conflicts,
    /// its error).
    pub fn may_close(&mut self) -> bool {
        let Some(wait) = self.wait_before_closing() else {
            return true;
        };
        let notice = format!("{wait} before closing the {} tab.", self.name);
        self.notices.push(notice);
        false
    }

    /// While an operation runs: what to wait for before closing, e.g.
    /// `Wait for "Pushing" to finish (or cancel it)`.
    pub fn wait_before_closing(&self) -> Option<String> {
        let op = self.op.as_ref()?;
        let what = op.label.trim_end_matches('…');
        let cancel = if op.can_cancel() {
            " (or cancel it)"
        } else {
            ""
        };
        Some(format!("Wait for \"{what}\" to finish{cancel}"))
    }

    /// The running operation's progress, while it has any to show.
    pub fn op_progress(&self) -> Option<Progress> {
        self.op.as_ref().map(Running::progress)
    }

    /// Run a quick git command `at` a worktree, then refresh and hand
    /// `done` the folder it ran in (resolved when it started) and the
    /// result. False (and nothing runs, which a notice says) while another
    /// operation runs.
    pub(super) fn run_op<R: JobResult>(
        &mut self,
        at: At,
        label: &str,
        job: impl FnOnce(Git) -> R + Send + 'static,
        done: impl FnOnce(&mut Project, PathBuf, R) + Send + 'static,
    ) -> bool {
        if !self.may_start(label) {
            return false;
        }
        let path = self.path_of(at);
        let git = self.git.at(&path);
        self.op = Some(Running::new(format!("{label}…")));
        self.jobs.spawn(
            move || job(git),
            |p, result| {
                p.end_op();
                done(p, path, result);
            },
        );
        true
    }

    /// Why a command that moves HEAD (see [`RepoSnapshot::head_blocked`])
    /// can't run in the worktree at `path` now.
    ///
    /// [`RepoSnapshot::head_blocked`]: graft_core::RepoSnapshot::head_blocked
    pub fn head_blocked(&self, path: &Path) -> Option<&'static str> {
        self.snapshot.as_ref()?.head_blocked(path)
    }

    /// [`Self::run_op`] for a command that moves HEAD where it runs (commit,
    /// check out, reset, cherry-pick, merge, …). False, after saying why,
    /// while [`Self::head_blocked`].
    pub(super) fn run_head_op<R: JobResult>(
        &mut self,
        at: At,
        label: &str,
        job: impl FnOnce(Git) -> R + Send + 'static,
        done: impl FnOnce(&mut Project, PathBuf, R) + Send + 'static,
    ) -> bool {
        let path = self.path_of(at);
        if let Some(reason) = self.head_blocked(&path) {
            self.notices
                .push(format!("{label} did not start. {reason}"));
            return false;
        }
        self.run_op(At::Path(path), label, job, done)
    }

    /// Like [`Self::run_op`], for commands that take a while (fetch, push,
    /// Slash): they report progress and can be cancelled.
    pub(super) fn run_long<R: JobResult>(
        &mut self,
        at: At,
        label: &str,
        job: impl FnOnce(Git, &CancelToken, &dyn Fn(Progress)) -> R + Send + 'static,
        done: impl FnOnce(&mut Project, PathBuf, R) + Send + 'static,
    ) -> bool {
        if !self.may_start(label) {
            return false;
        }
        let path = self.path_of(at);
        let git = self.git.at(&path);
        let (running, cancel) = Running::cancellable(format!("{label}…"));
        let report = running.reporter(self.jobs.waker());
        self.op = Some(running);
        self.jobs.spawn_long(
            move |_| job(git, &cancel, &report),
            |p, result| {
                p.end_op();
                done(p, path, result);
            },
        );
        true
    }

    fn end_op(&mut self) {
        self.op = None;
        self.refresh();
    }

    pub(super) fn cancel_op(&self) {
        if let Some(op) = &self.op {
            op.cancel();
        }
    }

    /// Say why something failed (see [`shown_error`]), as a toast.
    pub(super) fn report(&mut self, error: anyhow::Error) {
        self.notices.extend(shown_error(&error));
    }
}

/// The usual ending of an operation: nothing to say, or why it failed.
pub(super) fn quietly(p: &mut Project, _: PathBuf, result: anyhow::Result<()>) {
    if let Err(e) = result {
        p.report(e);
    }
}

/// The usual ending of an operation: say `done`, or why it failed.
pub(super) fn say(done: String) -> impl FnOnce(&mut Project, PathBuf, anyhow::Result<()>) + Send {
    move |p, _, result| match result {
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

    /// An operation asked for while another runs does not start, and says
    /// so (e.g. Settings → Save identity during a pull).
    #[test]
    fn an_operation_refused_while_another_runs_says_why() {
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
        let started = project.run_op(
            At::Main,
            "Saving identity",
            |_| -> anyhow::Result<()> { Ok(()) },
            quietly,
        );
        assert!(!started);
        assert_eq!(
            project.notices,
            ["Saving identity did not start. Wait for the running operation to finish."]
        );
        release.send(()).unwrap();
        wait(&mut project, "the operation never ended", |p| {
            !p.is_operating()
        });
    }

    /// Closing the tab waits for the running operation, whose end still
    /// has work to do (push after commit, conflicts, errors).
    #[test]
    fn a_tab_does_not_close_while_an_operation_runs() {
        let dir = repo();
        let mut project = open(dir.path());
        let (release, released) = crossbeam_channel::bounded::<()>(1);
        project.run_op(
            At::Main,
            "Merging",
            move |_| -> anyhow::Result<()> {
                let _ = released.recv();
                Ok(())
            },
            quietly,
        );
        assert!(!project.may_close());
        let notice = format!(
            "Wait for \"Merging\" to finish before closing the {} tab.",
            project.name
        );
        assert_eq!(project.notices, [notice]);
        release.send(()).unwrap();
        wait(&mut project, "the operation never ended", |p| {
            !p.is_operating()
        });
        assert!(project.may_close());
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
