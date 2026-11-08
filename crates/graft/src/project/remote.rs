//! Fetch, pull and push, with progress and cancel, and what to offer when the
//! remote rejects a push.

use std::path::PathBuf;

use graft_core::refusal::rejected_push;
use graft_core::{CancelToken, Git, Progress, remote};

use super::ops::{At, quietly, say};
use super::{Action, Confirm, Project};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RemoteOp {
    Fetch,
    Pull,
    Push,
    /// Push over a diverged upstream, with lease, from the worktree whose
    /// push the remote rejected (not whichever is selected by then).
    ForcePush {
        worktree: PathBuf,
    },
    PushTag {
        remote: String,
        tag: String,
    },
    DeleteRemoteBranch {
        remote: String,
        branch: String,
    },
}

impl RemoteOp {
    fn label(&self) -> String {
        match self {
            Self::Fetch => "Fetching".to_owned(),
            Self::Pull => "Pulling".to_owned(),
            Self::Push => "Pushing".to_owned(),
            Self::ForcePush { .. } => "Force pushing".to_owned(),
            Self::PushTag { tag, .. } => format!("Pushing tag {tag}"),
            Self::DeleteRemoteBranch { remote, branch } => {
                format!("Deleting {remote}/{branch}")
            }
        }
    }
}

/// How a remote operation ends: what it says, or offers.
type Ending = Box<dyn FnOnce(&mut Project, PathBuf, anyhow::Result<()>) + Send>;

impl Project {
    /// Talk to the remote from the selected worktree (a force push: from
    /// its own). Nothing happens while another operation runs.
    pub(super) fn remote(&mut self, op: RemoteOp) {
        let worktree = match &op {
            RemoteOp::ForcePush { worktree } => worktree.clone(),
            _ => self.nav.worktree.clone(),
        };
        self.remote_in(worktree, op);
    }

    /// Talk to the remote from `worktree`; its follow-ups (a force push
    /// after a rejection) stay there too.
    pub(super) fn remote_in(&mut self, worktree: PathBuf, op: RemoteOp) {
        let label = op.label();
        let done: Ending = match &op {
            RemoteOp::Fetch => Box::new(quietly),
            RemoteOp::Pull => Box::new(say("Pulled".to_owned())),
            RemoteOp::Push => Box::new(|p: &mut Project, ran_in, result| p.pushed(ran_in, result)),
            RemoteOp::ForcePush { .. } => Box::new(say("Force pushed".to_owned())),
            RemoteOp::PushTag { remote, tag } => {
                Box::new(say(format!("Pushed tag {tag} to {remote}")))
            }
            RemoteOp::DeleteRemoteBranch { remote, branch } => {
                Box::new(say(format!("Deleted {remote}/{branch}")))
            }
        };
        let job = move |git: Git, cancel: &CancelToken, report: &dyn Fn(Progress)| match op {
            RemoteOp::Fetch => remote::fetch(&git, false, cancel, report),
            RemoteOp::Pull => remote::pull(&git, cancel, report),
            RemoteOp::Push => remote::push(&git, cancel, report),
            RemoteOp::ForcePush { .. } => remote::force_push(&git, cancel, report),
            RemoteOp::PushTag { remote, tag } => {
                remote::push_tag(&git, &remote, &tag, cancel, report)
            }
            RemoteOp::DeleteRemoteBranch { remote, branch } => {
                remote::delete_remote_branch(&git, &remote, &branch, cancel, report)
            }
        };
        self.run_long(At::Path(worktree), &label, job, done);
    }

    /// A plain push from `worktree` ended; if the remote rejected it,
    /// offer to force push from there.
    fn pushed(&mut self, worktree: PathBuf, result: anyhow::Result<()>) {
        match result {
            Ok(()) => self.infos.push("Pushed".to_owned()),
            Err(e) if rejected_push(&format!("{e:#}")) => {
                self.ask(Confirm {
                    title: "The remote has commits you don't have".to_owned(),
                    body: "Pull first to combine them with yours. Or force push to replace \
                           the remote branch with yours; Graft uses --force-with-lease, so it \
                           only goes through if nobody pushed since your last fetch."
                        .to_owned(),
                    confirm_label: "Force push".to_owned(),
                    action: Action::Remote(RemoteOp::ForcePush { worktree }),
                });
            }
            Err(e) => self.report(e),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::process::Command;

    use graft_core::paths::same_path;

    use super::*;
    use crate::testing::{git, open, wait};

    /// The commit `branch` points at in `repo`, or empty.
    fn tip(repo: &Path, branch: &str) -> String {
        let out = Command::new("git")
            .args(["rev-parse", "--verify", "--quiet", branch])
            .current_dir(repo)
            .output()
            .unwrap();
        String::from_utf8_lossy(&out.stdout).trim().to_owned()
    }

    struct Setup {
        _dir: tempfile::TempDir,
        project: Project,
        origin: PathBuf,
        a: PathBuf,
        b: PathBuf,
    }

    /// A repo with a bare `origin` and two linked worktrees on branches
    /// `a` and `b`; `a` is selected.
    fn setup() -> Setup {
        let dir = tempfile::tempdir().unwrap();
        let main = dir.path().join("main");
        let origin = dir.path().join("origin.git");
        std::fs::create_dir_all(&main).unwrap();
        git(
            dir.path(),
            &["init", "-q", "--bare", "-b", "main", "origin.git"],
        );
        git(&main, &["init", "-q", "-b", "main"]);
        // Graft's own commits use these (whatever the machine's setup).
        for (key, value) in [
            ("user.name", "t"),
            ("user.email", "t@t"),
            ("commit.gpgsign", "false"),
        ] {
            git(&main, &["config", key, value]);
        }
        std::fs::write(main.join("a.txt"), "a\n").unwrap();
        git(&main, &["add", "a.txt"]);
        git(&main, &["commit", "-q", "-m", "first"]);
        git(
            &main,
            &["remote", "add", "origin", origin.to_str().unwrap()],
        );
        for name in ["a", "b"] {
            let path = dir.path().join(name);
            git(
                &main,
                &["worktree", "add", "-q", "-b", name, path.to_str().unwrap()],
            );
        }
        let mut project = open(&main);
        let snap = project.snapshot.clone().unwrap();
        let path = |branch: &str| {
            let wt = snap.worktrees.iter();
            let mut wt = wt.filter(|wt| wt.branch.as_deref() == Some(branch));
            wt.next().unwrap().path.clone()
        };
        let (a, b) = (path("a"), path("b"));
        project.apply(Action::OpenWorktree(a.clone()));
        Setup {
            _dir: dir,
            project,
            origin,
            a,
            b,
        }
    }

    /// Commit with "Push after commit" in worktree a, then select b before
    /// it ends: a's branch is pushed, and b's drafted message stays.
    #[test]
    fn push_after_commit_stays_in_its_worktree() {
        let Setup {
            _dir,
            mut project,
            origin,
            a,
            b,
        } = setup();
        std::fs::write(a.join("new.txt"), "new\n").unwrap();
        git(&a, &["add", "new.txt"]);
        project.refresh();
        wait(&mut project, "the staged file never showed", |p| {
            p.worktree()
                .and_then(|wt| wt.status.as_ref())
                .is_some_and(|s| s.staged().count() == 1)
        });
        project.commit_form.summary = "a's work".to_owned();
        project.commit_form.push_after = true;
        project.apply(Action::Commit);
        assert!(project.is_operating());
        project.apply(Action::OpenWorktree(b.clone()));
        project.commit_form.summary = "b's draft".to_owned();
        wait(&mut project, "a was never pushed", |p| {
            !p.is_operating() && !tip(&origin, "a").is_empty()
        });
        assert!(project.notices.is_empty(), "{:?}", project.notices);
        assert_eq!(tip(&origin, "a"), tip(&a, "HEAD"));
        assert_eq!(tip(&origin, "b"), "", "b was not pushed");
        assert!(same_path(&project.nav.worktree, &b));
        assert_eq!(project.commit_form.summary, "b's draft");
    }

    /// A push from a that the remote rejects offers a force push from a,
    /// even with b selected by then.
    #[test]
    fn force_push_after_a_rejection_targets_the_pushed_worktree() {
        let Setup {
            _dir,
            mut project,
            origin,
            a,
            b,
        } = setup();
        git(&a, &["push", "-q", "-u", "origin", "a"]);
        // a's tip on origin is no longer in a's history.
        git(&a, &["commit", "-q", "--amend", "-m", "rewritten"]);
        project.apply(Action::Remote(RemoteOp::Push));
        project.apply(Action::OpenWorktree(b.clone()));
        wait(&mut project, "no force push offer", |p| {
            p.confirm().is_some()
        });
        let action = &project.confirm().unwrap().action;
        let Action::Remote(RemoteOp::ForcePush { worktree }) = action else {
            panic!("{action:?}");
        };
        assert!(same_path(worktree, &a));
        project.apply(Action::ConfirmYes);
        wait(&mut project, "the force push never ended", |p| {
            !p.is_operating()
        });
        assert!(project.notices.is_empty(), "{:?}", project.notices);
        assert_eq!(tip(&origin, "a"), tip(&a, "HEAD"));
        assert_eq!(tip(&origin, "b"), "", "b was not pushed");
    }
}
