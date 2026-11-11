//! Fetch, pull and push, with progress and cancel, and what to offer when the
//! remote rejects a push.

use std::path::PathBuf;

use graft_core::refusal::rejected_push;
use graft_core::{CancelToken, Git, Progress, remote};

use super::conflicts::after_conflicts;
use super::ops::{At, quietly, say};
use super::{Action, Confirm, Project};
use crate::jobs::shown_error;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RemoteOp {
    Fetch,
    Pull,
    /// Pull this branch, wherever it is checked out, or nowhere (see
    /// [`Project::pull_branch`]).
    PullBranch {
        branch: String,
    },
    Push,
    /// Push over a diverged upstream, with lease, from the worktree whose
    /// push the remote rejected (not whichever is selected by then).
    ForcePush {
        worktree: PathBuf,
    },
    PushTag {
        remote: String,
        tag: String,
        /// Just created by the New tag dialog: a failed push says the tag
        /// is still there.
        created: bool,
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
            Self::PullBranch { branch } => format!("Pulling {branch}"),
            Self::Push => "Pushing".to_owned(),
            Self::ForcePush { .. } => "Force pushing".to_owned(),
            Self::PushTag { tag, .. } => format!("Pushing tag {tag}"),
            Self::DeleteRemoteBranch { remote, branch } => {
                format!("Deleting {remote}/{branch}")
            }
        }
    }
}

/// How a remote operation ends: what it says, or offers. Ok(true): a pull
/// stopped with conflicts.
type Ending = Box<dyn FnOnce(&mut Project, PathBuf, anyhow::Result<bool>) + Send>;

/// The ending of an operation that cannot stop with conflicts.
fn plain(end: impl FnOnce(&mut Project, PathBuf, anyhow::Result<()>) + Send + 'static) -> Ending {
    Box::new(move |p, ran_in, result| end(p, ran_in, result.map(|_| ())))
}

/// How a pull ends: like a merge when it stops with conflicts; otherwise
/// the view stays (unlike a merge, a pull is often done mid-work).
fn pulled(done: String) -> Ending {
    Box::new(move |p: &mut Project, ran_in, result| match result {
        Ok(false) => p.infos.push(done),
        result => after_conflicts(done, "Pulling".into(), None)(p, ran_in, result),
    })
}

/// The ending of pushing a tag the New tag dialog just created: if the push
/// fails, the tag is still there and can be pushed again.
fn pushed_new_tag(
    remote: String,
    tag: String,
) -> impl FnOnce(&mut Project, PathBuf, anyhow::Result<()>) + Send {
    move |p, _, result| match result {
        Ok(()) => p
            .infos
            .push(format!("Created tag {tag} and pushed it to {remote}")),
        Err(e) => {
            if let Some(error) = shown_error(&e) {
                p.notices.push(format!(
                    "Created tag {tag}, but pushing it to {remote} failed. The tag is still \
                     here; push it again from its menu under TAGS in the sidebar.\n\n{error}"
                ));
            } else {
                // Cancelled: the tag stays, unpushed.
                p.infos
                    .push(format!("Created tag {tag}; it was not pushed"));
            }
        }
    }
}

impl Project {
    /// Pull `branch` (a branch's menu) if [`remote::pull_plan`] allows it:
    /// in the worktree that has it checked out, else a fast-forward from the
    /// main folder (any worktree's folder may be gone). The job checks again
    /// where it is checked out, see [`remote::pull_branch`].
    fn pull_branch(&mut self, branch: String) {
        let Some(snap) = &self.snapshot else { return };
        match remote::pull_plan(snap, &branch, &self.nav.worktree) {
            Ok(plan) => {
                let worktree = plan.worktree.unwrap_or_else(|| self.main_path());
                self.remote_in(worktree, RemoteOp::PullBranch { branch });
            }
            Err(why) => self.notices.push(why),
        }
    }

    /// Talk to the remote from the selected worktree (a force push: from
    /// its own). Nothing happens while another operation runs.
    pub(super) fn remote(&mut self, op: RemoteOp) {
        let worktree = match op {
            RemoteOp::PullBranch { branch } => return self.pull_branch(branch),
            RemoteOp::ForcePush { ref worktree } => worktree.clone(),
            _ => self.nav.worktree.clone(),
        };
        self.remote_in(worktree, op);
    }

    /// Talk to the remote from `worktree`; its follow-ups (a force push
    /// after a rejection) stay there too.
    pub(super) fn remote_in(&mut self, worktree: PathBuf, op: RemoteOp) {
        let label = op.label();
        let done: Ending = match &op {
            RemoteOp::Fetch => plain(quietly),
            // Like a merge when it stops with conflicts; otherwise the view
            // stays (unlike a merge, a pull is often done mid-work).
            RemoteOp::Pull => pulled("Pulled".to_owned()),
            RemoteOp::PullBranch { branch } => pulled(format!("Pulled {branch}")),
            RemoteOp::Push => plain(|p: &mut Project, ran_in, result| p.pushed(ran_in, result)),
            RemoteOp::ForcePush { .. } => plain(say("Force pushed".to_owned())),
            RemoteOp::PushTag {
                remote,
                tag,
                created: false,
            } => plain(say(format!("Pushed tag {tag} to {remote}"))),
            RemoteOp::PushTag {
                remote,
                tag,
                created: true,
            } => plain(pushed_new_tag(remote.clone(), tag.clone())),
            RemoteOp::DeleteRemoteBranch { remote, branch } => {
                plain(say(format!("Deleted {remote}/{branch}")))
            }
        };
        let job = move |git: Git, cancel: &CancelToken, report: &dyn Fn(Progress)| {
            let ran = match op {
                RemoteOp::Pull => return remote::pull(&git, cancel, report),
                RemoteOp::PullBranch { branch } => {
                    return remote::pull_branch(&git, &branch, cancel, report);
                }
                RemoteOp::Fetch => remote::fetch(&git, false, cancel, report),
                RemoteOp::Push => remote::push(&git, cancel, report),
                RemoteOp::ForcePush { .. } => remote::force_push(&git, cancel, report),
                RemoteOp::PushTag { remote, tag, .. } => {
                    remote::push_tag(&git, &remote, &tag, cancel, report)
                }
                RemoteOp::DeleteRemoteBranch { remote, branch } => {
                    remote::delete_remote_branch(&git, &remote, &branch, cancel, report)
                }
            };
            ran.map(|()| false)
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
                           the remote branch with yours; Graft uses --force-with-lease and \
                           --force-if-includes, so it is refused if the remote has commits \
                           that were never in your branch, even ones fetched since."
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
    use graft_core::paths::same_path;

    use super::*;
    use crate::testing::{git, open, tip, wait};

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

    /// Submit the New tag dialog for `tag` at `target`, with "Push to
    /// origin" ticked or not, and wait until everything it started ended.
    fn new_tag(project: &mut Project, tag: &str, target: &str, push: bool) {
        let snap = project.snapshot.clone().unwrap();
        let mut kind = crate::project::DialogKind::new_tag(target, target, &snap, push);
        if let crate::project::DialogKind::NewTag { name, message, .. } = &mut kind {
            *name = tag.to_owned();
            *message = "Release".to_owned();
        }
        project.apply(Action::OpenDialog(kind));
        project.apply(Action::SubmitDialog);
        wait(project, "the new tag never ended", |p| {
            !p.is_operating() && p.dialog.as_ref().is_none_or(|d| !d.submitting)
        });
    }

    /// New tag with "Push to origin" ticked creates the tag and pushes it;
    /// unticked, origin never gets it.
    #[test]
    fn a_new_tag_is_pushed_only_when_asked() {
        let Setup {
            _dir,
            mut project,
            origin,
            a,
            ..
        } = setup();
        new_tag(&mut project, "v1", "HEAD", true);
        assert!(project.notices.is_empty(), "{:?}", project.notices);
        assert!(project.dialog.is_none());
        assert_eq!(tip(&origin, "refs/tags/v1"), tip(&a, "refs/tags/v1"));
        assert!(!tip(&origin, "refs/tags/v1").is_empty());
        let said = project.infos.last().unwrap();
        assert_eq!(said, "Created tag v1 and pushed it to origin");

        new_tag(&mut project, "v2", "HEAD", false);
        assert!(project.notices.is_empty(), "{:?}", project.notices);
        assert!(!tip(&a, "refs/tags/v2").is_empty());
        assert_eq!(tip(&origin, "refs/tags/v2"), "", "v2 was not pushed");
        assert_eq!(project.infos.last().unwrap(), "Created tag v2");
    }

    /// When git refuses to create the tag, the dialog shows why and stays
    /// open, and nothing is pushed.
    #[test]
    fn a_tag_that_could_not_be_created_is_not_pushed() {
        let Setup {
            _dir,
            mut project,
            origin,
            a,
            ..
        } = setup();
        let missing = "0123456789abcdef0123456789abcdef01234567";
        new_tag(&mut project, "v1", missing, true);
        let dialog = project.dialog.as_ref().expect("the dialog stays open");
        assert!(dialog.error.is_some());
        assert_eq!(tip(&a, "refs/tags/v1"), "");
        assert_eq!(tip(&origin, "refs/tags/v1"), "");
        // A push of the missing tag would have failed with a notice.
        assert!(project.notices.is_empty(), "{:?}", project.notices);
    }

    /// When the push fails, the tag stays, and the notice says so and where
    /// to push it again.
    #[test]
    fn a_new_tag_whose_push_failed_stays() {
        let Setup {
            _dir: dir,
            mut project,
            a,
            ..
        } = setup();
        let gone = dir.path().join("gone.git");
        git(&a, &["remote", "set-url", "origin", gone.to_str().unwrap()]);
        new_tag(&mut project, "v1", "HEAD", true);
        assert!(project.dialog.is_none());
        assert!(!tip(&a, "refs/tags/v1").is_empty());
        assert_eq!(project.notices.len(), 1, "{:?}", project.notices);
        let notice = &project.notices[0];
        assert!(notice.contains("still here"), "{notice}");
        assert!(notice.contains("TAGS in the sidebar"), "{notice}");
    }

    /// The dialog offers to push only with a remote to push to, ticked as
    /// last chosen.
    #[test]
    fn the_new_tag_dialog_offers_the_tag_remote() {
        let Setup { _dir, project, .. } = setup();
        let mut snap = (*project.snapshot.clone().unwrap()).clone();
        let kind = crate::project::DialogKind::new_tag("HEAD", "HEAD", &snap, true);
        let crate::project::DialogKind::NewTag { remote, push, .. } = kind else {
            unreachable!()
        };
        assert_eq!((remote.as_deref(), push), (Some("origin"), true));
        snap.remotes.clear();
        let kind = crate::project::DialogKind::new_tag("HEAD", "HEAD", &snap, false);
        let crate::project::DialogKind::NewTag { remote, .. } = kind else {
            unreachable!()
        };
        assert_eq!(remote, None);
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

    /// Someone else pushes to a, a background fetch brings their commit in,
    /// and a's force push is refused instead of replacing it.
    #[test]
    fn force_push_keeps_commits_that_were_only_fetched() {
        let Setup {
            _dir: dir,
            mut project,
            origin,
            a,
            ..
        } = setup();
        git(&a, &["push", "-q", "-u", "origin", "a"]);
        let colleague = dir.path().join("colleague");
        let url = origin.to_str().unwrap();
        git(dir.path(), &["clone", "-q", "-b", "a", url, "colleague"]);
        std::fs::write(colleague.join("theirs.txt"), "theirs\n").unwrap();
        git(&colleague, &["add", "theirs.txt"]);
        git(&colleague, &["commit", "-q", "-m", "theirs"]);
        git(&colleague, &["push", "-q", "origin", "a"]);
        let theirs = tip(&colleague, "HEAD");
        git(&a, &["commit", "-q", "--amend", "-m", "rewritten"]);
        git(&a, &["fetch", "-q", "origin"]);
        project.apply(Action::Remote(RemoteOp::Push));
        wait(&mut project, "no force push offer", |p| {
            p.confirm().is_some()
        });
        project.apply(Action::ConfirmYes);
        wait(&mut project, "the force push never ended", |p| {
            !p.is_operating()
        });
        assert_eq!(project.notices.len(), 1, "{:?}", project.notices);
        assert_eq!(tip(&origin, "a"), theirs);
    }

    /// A pull of a that stops with conflicts says so and opens them in a's
    /// File status, even with b selected by then.
    #[test]
    fn a_pull_with_conflicts_opens_them() {
        let Setup {
            _dir: dir,
            mut project,
            origin,
            a,
            b,
        } = setup();
        git(&a, &["push", "-q", "-u", "origin", "a"]);
        let colleague = dir.path().join("colleague");
        let url = origin.to_str().unwrap();
        git(dir.path(), &["clone", "-q", "-b", "a", url, "colleague"]);
        std::fs::write(colleague.join("a.txt"), "theirs\n").unwrap();
        git(&colleague, &["commit", "-q", "-am", "theirs"]);
        git(&colleague, &["push", "-q", "origin", "a"]);
        std::fs::write(a.join("a.txt"), "mine\n").unwrap();
        git(&a, &["commit", "-q", "-am", "mine"]);
        project.apply(Action::Remote(RemoteOp::Pull));
        project.apply(Action::OpenWorktree(b));
        wait(&mut project, "the pull never ended", |p| !p.is_operating());
        assert!(project.notices.is_empty(), "{:?}", project.notices);
        let said = project.infos.last().unwrap();
        assert!(said.contains("Pulling stopped with conflicts"), "{said}");
        assert!(same_path(&project.nav.worktree, &a));
        assert_eq!(project.nav.view, crate::project::WorkspaceView::FileStatus);
    }
    /// Pull from b's menu with a selected: b's worktree pulls (merging),
    /// and a branch checked out nowhere moves ahead; a stays as it was.
    #[test]
    fn pull_a_branch_from_its_menu() {
        let Setup {
            _dir: dir,
            mut project,
            origin,
            a,
            b,
        } = setup();
        git(&b, &["push", "-q", "-u", "origin", "b"]);
        git(&b, &["push", "-q", "origin", "b:c"]);
        git(&b, &["branch", "-q", "--track", "c", "origin/c"]);
        let colleague = dir.path().join("colleague");
        let url = origin.to_str().unwrap();
        git(dir.path(), &["clone", "-q", "-b", "b", url, "colleague"]);
        std::fs::write(
            colleague.join("theirs.txt"),
            "theirs
",
        )
        .unwrap();
        git(&colleague, &["add", "theirs.txt"]);
        git(&colleague, &["commit", "-q", "-m", "theirs"]);
        git(&colleague, &["push", "-q", "origin", "HEAD:b", "HEAD:c"]);
        let theirs = tip(&colleague, "HEAD");
        let a_before = tip(&a, "HEAD");
        project.refresh();
        wait(&mut project, "b and c never showed their upstreams", |p| {
            let snap = p.snapshot.as_ref().unwrap();
            let tracks = |name: &str| {
                let up = snap.branch(name).map(|b| &b.upstream);
                matches!(up, Some(graft_core::Upstream::Tracking { .. }))
            };
            tracks("b") && tracks("c")
        });
        for branch in ["b", "c"] {
            let branch = branch.to_owned();
            project.apply(Action::Remote(RemoteOp::PullBranch { branch }));
            wait(&mut project, "the pull never ended", |p| !p.is_operating());
            assert!(project.notices.is_empty(), "{:?}", project.notices);
        }
        assert_eq!(tip(&b, "b"), theirs);
        assert!(b.join("theirs.txt").exists());
        assert_eq!(tip(&a, "c"), theirs);
        assert_eq!(tip(&a, "HEAD"), a_before);
        assert!(!a.join("theirs.txt").exists());
        assert!(same_path(&project.nav.worktree, &a));
    }
}
