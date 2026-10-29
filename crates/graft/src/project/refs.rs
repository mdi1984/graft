//! Branches, tags and commit actions: the small dialogs that ask for a name,
//! and what happens when git says no (unmerged branch, rejected push).

use graft_core::Oid;
use graft_core::refs::{self, NOT_MERGED, ResetMode};
use graft_core::remote::{self, is_rejected_push};
use graft_core::stash;

use super::changes::Then;
use super::{Action, Confirm, Project, RemoteOp, Tab};

/// A dialog that asks for a name or a choice before running a git command.
#[derive(Clone, Debug)]
pub enum DialogKind {
    NewBranch {
        name: String,
        /// Where it starts: a branch, tag or commit id.
        start: String,
        /// How to show `start` to the user.
        start_label: String,
        checkout: bool,
    },
    RenameBranch {
        old: String,
        name: String,
    },
    NewTag {
        name: String,
        message: String,
        target: String,
        target_label: String,
    },
    Reset {
        target: String,
        target_label: String,
        branch: String,
        mode: ResetMode,
    },
    Stash {
        message: String,
        include_untracked: bool,
        keep_index: bool,
        staged_only: bool,
    },
    StashBranch {
        id: Oid,
        label: String,
        name: String,
    },
    /// Add a remote (`original` is `None`) or edit one.
    Remote {
        original: Option<String>,
        name: String,
        url: String,
        /// Fetch from it after adding.
        fetch: bool,
    },
    /// Merge a branch into the selected worktree's branch.
    Merge {
        /// The selected worktree's branch.
        into: String,
        rev: String,
        /// Branches to pick from.
        choices: Vec<String>,
        filter: String,
        no_ff: bool,
    },
}

#[derive(Clone, Debug)]
pub struct Dialog {
    pub kind: DialogKind,
    pub error: Option<String>,
    pub submitting: bool,
    /// Focus the first field once.
    pub focus: bool,
}

impl Dialog {
    pub fn new(kind: DialogKind) -> Self {
        Self {
            kind,
            error: None,
            submitting: false,
            focus: true,
        }
    }
}

impl Project {
    pub(super) fn submit_dialog(&mut self) {
        let Some(dialog) = &mut self.dialog else {
            return;
        };
        if dialog.submitting {
            return;
        }
        let kind = dialog.kind.clone();
        // Problems git would only report after a round trip.
        let problem = match &kind {
            DialogKind::NewBranch { name, .. }
            | DialogKind::RenameBranch { name, .. }
            | DialogKind::StashBranch { name, .. } => {
                let name = name.trim();
                if name.is_empty() {
                    Some("Give the branch a name.".to_owned())
                } else if self
                    .snapshot
                    .as_ref()
                    .is_some_and(|s| s.branch(name).is_some())
                {
                    Some(format!("A branch named {name} already exists."))
                } else {
                    None
                }
            }
            DialogKind::NewTag { name, .. } => {
                let name = name.trim();
                if name.is_empty() {
                    Some("Give the tag a name.".to_owned())
                } else if self
                    .snapshot
                    .as_ref()
                    .is_some_and(|s| s.tags.iter().any(|t| t.name == name))
                {
                    Some(format!("A tag named {name} already exists."))
                } else {
                    None
                }
            }
            DialogKind::Reset { .. } | DialogKind::Stash { .. } => None,
            DialogKind::Merge { rev, .. } if rev.is_empty() => {
                Some("Pick what to merge.".to_owned())
            }
            DialogKind::Merge { .. } => None,
            DialogKind::Remote {
                original,
                name,
                url,
                ..
            } => {
                let name = name.trim();
                let taken = original.as_deref() != Some(name)
                    && self
                        .snapshot
                        .as_ref()
                        .is_some_and(|s| s.remotes.iter().any(|r| r.name == name));
                if name.is_empty() || name.contains(char::is_whitespace) {
                    Some("Give the remote a name without spaces, e.g. origin.".to_owned())
                } else if taken {
                    Some(format!("A remote named {name} already exists."))
                } else if url.trim().is_empty() {
                    Some("Enter the repository's URL.".to_owned())
                } else {
                    None
                }
            }
        };
        if let Some(problem) = problem {
            dialog.error = Some(problem);
            return;
        }
        dialog.error = None;
        dialog.submitting = true;
        match kind {
            DialogKind::NewBranch {
                name,
                start,
                checkout,
                ..
            } => {
                let name = name.trim().to_owned();
                let done = if checkout {
                    format!("Created and switched to {name}")
                } else {
                    format!("Created branch {name}")
                };
                self.run_op("Creating branch", Then::Dialog(done), move |git| {
                    refs::create_branch(&git, &name, &start, checkout)
                });
            }
            DialogKind::RenameBranch { old, name } => {
                let name = name.trim().to_owned();
                let done = format!("Renamed {old} to {name}");
                self.run_op("Renaming branch", Then::Dialog(done), move |git| {
                    refs::rename_branch(&git, &old, &name)
                });
            }
            DialogKind::NewTag {
                name,
                message,
                target,
                ..
            } => {
                let name = name.trim().to_owned();
                let done = format!("Created tag {name}");
                self.run_op("Creating tag", Then::Dialog(done), move |git| {
                    refs::create_tag(&git, &name, &target, Some(&message))
                });
            }
            DialogKind::Stash {
                message,
                include_untracked,
                keep_index,
                staged_only,
            } => {
                let options = stash::Options {
                    message,
                    include_untracked,
                    keep_index,
                    staged_only,
                };
                self.run_op("Stashing", Then::Dialog("Stashed".to_owned()), move |git| {
                    stash::push(&git, &options)
                });
            }
            DialogKind::StashBranch { id, name, .. } => {
                let name = name.trim().to_owned();
                let done = format!("Created {name} from the stash and switched to it");
                self.run_op("Creating branch", Then::Dialog(done), move |git| {
                    stash::branch(&git, &name, id)
                });
            }
            DialogKind::Merge {
                into, rev, no_ff, ..
            } => {
                dialog.submitting = false;
                self.apply_conflict_action(Action::Merge { rev, no_ff, into });
                if let Some(dialog) = &mut self.dialog {
                    dialog.submitting = self.op.is_some();
                }
            }
            DialogKind::Remote {
                original,
                name,
                url,
                fetch,
            } => {
                let name = name.trim().to_owned();
                let url = url.trim().to_owned();
                let done = match &original {
                    None => format!("Added remote {name}"),
                    Some(_) => format!("Saved remote {name}"),
                };
                let fetch = fetch && original.is_none();
                let label = if fetch {
                    "Adding remote and fetching"
                } else {
                    "Saving remote"
                };
                self.run_op(label, Then::Dialog(done), move |git| {
                    remote::save_remote(&git, original.as_deref(), &name, &url, fetch)
                });
            }
            DialogKind::Reset {
                target,
                target_label,
                branch,
                mode,
            } => {
                let done = format!("Reset {branch} to {target_label}");
                self.run_op("Resetting", Then::Dialog(done), move |git| {
                    refs::reset(&git, &target, mode)
                });
            }
        }
    }

    /// Something failed: show it where the user can act on it.
    pub(super) fn on_op_error(&mut self, then: Then, error: String) {
        if let Some(form) = &mut self.new_worktree
            && form.creating
        {
            form.creating = false;
            form.error = Some(error);
            return;
        }
        if let Some(dialog) = &mut self.dialog
            && dialog.submitting
        {
            dialog.submitting = false;
            dialog.error = Some(error);
            return;
        }
        match then {
            Then::DeletedBranch(name) if error.contains(NOT_MERGED) => {
                self.confirm = Some(Confirm {
                    title: format!("{name} is not merged"),
                    body: format!(
                        "Its commits are not in your current branch or its upstream. \
                         Deleting it anyway can lose those commits.\n\n{error}"
                    ),
                    confirm_label: "Delete anyway".to_owned(),
                    action: Action::DeleteBranch { name, force: true },
                });
            }
            Then::Switched(branch) if blocked_by_changes(&error) => {
                let untracked = error.contains("untracked working tree files");
                self.confirm = Some(Confirm {
                    title: format!("Your changes are in the way of {branch}"),
                    body: format!(
                        "Switching would overwrite uncommitted changes in your main folder: \
                         {}.\n\nGraft can stash them first; get them back later from \
                         Stashes (Apply or Pop).",
                        files_in_the_way(&error)
                    ),
                    confirm_label: "Stash and switch".to_owned(),
                    action: Action::StashAndCheckout { branch, untracked },
                });
            }
            Then::Pushed if is_rejected_push(&error) => {
                self.confirm = Some(Confirm {
                    title: "The remote has commits you don't have".to_owned(),
                    body: "Pull first to combine them with yours. Or force push to replace \
                           the remote branch with yours; Graft uses --force-with-lease, so it \
                           only goes through if nobody pushed since your last fetch."
                        .to_owned(),
                    confirm_label: "Force push".to_owned(),
                    action: Action::Remote(RemoteOp::ForcePush),
                });
            }
            _ if error.contains("No configured push destination")
                || error.contains("No remote repository specified") =>
            {
                self.notices.push(
                    "This repository has no remote yet. Add one under REMOTES in the sidebar \
                     (Add remote…)."
                        .to_owned(),
                );
            }
            _ if error.contains("Please tell me who you are")
                || error.contains("Author identity unknown") =>
            {
                self.notices.push(
                    "Git doesn't know who you are yet. Set your name and email in Settings \
                     (the gear at the top right)."
                        .to_owned(),
                );
            }
            _ => self.notices.push(error),
        }
    }

    pub(super) fn apply_ref_action(&mut self, action: Action) {
        match action {
            Action::OpenDialog(kind) => {
                if !self.is_operating() {
                    self.dialog = Some(Dialog::new(kind));
                }
            }
            Action::CloseDialog => {
                if !self.dialog.as_ref().is_some_and(|d| d.submitting) {
                    self.dialog = None;
                }
            }
            Action::SubmitDialog => self.submit_dialog(),
            Action::Checkout(branch) => {
                let then = Then::Switched(branch.clone());
                self.checkout("Switching branch", then, move |git| {
                    refs::switch(&git, &branch).map(|()| None)
                });
            }
            Action::StashAndCheckout { branch, untracked } => {
                let then = Then::Switched(branch.clone());
                self.checkout("Switching branch", then, move |git| {
                    stash::push(
                        &git,
                        &stash::Options {
                            message: format!("Before switching to {branch}"),
                            include_untracked: untracked,
                            ..Default::default()
                        },
                    )?;
                    refs::switch(&git, &branch)?;
                    Ok(Some(format!(
                        "Your changes are in the stash \"Before switching to {branch}\""
                    )))
                });
            }
            Action::CheckoutRemote { local, remote } => {
                let done = format!("Switched to {local}, tracking {remote}");
                self.checkout("Switching branch", Then::Info(done), move |git| {
                    refs::switch_tracking(&git, &local, &remote).map(|()| None)
                });
            }
            Action::CheckoutDetached(id) => {
                let rev = id.to_string();
                let done = format!("Checked out {}", graft_core::short(&id));
                self.checkout("Checking out", Then::Info(done), move |git| {
                    refs::switch_detached(&git, &rev).map(|()| None)
                });
            }
            Action::DeleteBranch { name, force } => {
                let target = name.clone();
                self.run_op_at(
                    self.git.clone(),
                    "Deleting branch",
                    Then::DeletedBranch(name.clone()),
                    move |git| {
                        refs::delete_branch(&git, &target, force)?;
                        Ok(Some(format!("Deleted branch {target}")))
                    },
                );
            }
            Action::RemoveRemote(name) => {
                let done = format!("Removed remote {name}");
                self.run_op_at(
                    self.git.clone(),
                    "Removing remote",
                    Then::Info(done),
                    move |git| remote::remove_remote(&git, &name).map(|()| None),
                );
            }
            Action::DeleteTag(name) => {
                let done = format!("Deleted tag {name}");
                self.run_op("Deleting tag", Then::Info(done), move |git| {
                    refs::delete_tag(&git, &name)
                });
            }
            Action::CherryPick(id) => {
                let rev = id.to_string();
                let done = format!("Cherry-picked {}", graft_core::short(&id));
                let stopped = format!(
                    "Cherry-picking {} stopped with conflicts. Resolve them in File status, then Continue.",
                    graft_core::short(&id)
                );
                self.run_op_at(
                    self.wt_git(),
                    "Cherry-picking",
                    Then::MayConflict(done),
                    move |git| Ok(refs::cherry_pick(&git, &rev)?.then_some(stopped)),
                );
            }
            Action::Revert(id) => {
                let rev = id.to_string();
                let done = format!("Reverted {}", graft_core::short(&id));
                let stopped = format!(
                    "Reverting {} stopped with conflicts. Resolve them in File status, then Continue.",
                    graft_core::short(&id)
                );
                self.run_op_at(
                    self.wt_git(),
                    "Reverting",
                    Then::MayConflict(done),
                    move |git| Ok(refs::revert(&git, &rev)?.then_some(stopped)),
                );
            }
            Action::UndoCommit => {
                self.run_op(
                    "Undoing commit",
                    Then::Info("Undid the last commit; its changes are staged".to_owned()),
                    move |git| refs::undo_last_commit(&git),
                );
                self.nav.view = super::WorkspaceView::FileStatus;
            }
            _ => {}
        }
    }

    /// Check out in the main folder, never in a linked worktree, and select
    /// the main folder so the user sees where it happened. Stays put on
    /// Review or with a drafted commit message, which selecting would reset.
    fn checkout(
        &mut self,
        label: &str,
        then: Then,
        job: impl FnOnce(graft_core::Git) -> anyhow::Result<Option<String>> + Send + 'static,
    ) {
        if self.is_operating() {
            return;
        }
        if let Some(reason) = self.checkout_blocked() {
            self.notices.push(reason.to_owned());
            return;
        }
        let draft =
            !self.commit_form.summary.trim().is_empty() || !self.commit_form.body.trim().is_empty();
        if self.nav.tab != Tab::Review
            && !draft
            && let Some(main) = self.main_worktree()
            && !graft_core::same_path(&main.path, &self.nav.worktree)
        {
            let main = main.path.clone();
            self.open_worktree(main);
        }
        // `self.git` works in the main folder.
        self.run_op_at(self.git.clone(), label, then, job);
    }
}

/// Git refused to switch branches because local changes would be lost.
fn blocked_by_changes(error: &str) -> bool {
    error.contains("would be overwritten by checkout")
        || error.contains("Please commit your changes or stash them")
}

/// The files git lists between "…would be overwritten by checkout:" and
/// "Please …", joined for a sentence.
fn files_in_the_way(error: &str) -> String {
    let files: Vec<&str> = error
        .lines()
        .skip_while(|l| !l.contains("would be overwritten"))
        .skip(1)
        .map(str::trim)
        .take_while(|l| !l.starts_with("Please") && !l.starts_with("Aborting"))
        .filter(|l| !l.is_empty())
        .collect();
    match files.len() {
        0 => "some files".to_owned(),
        1..=5 => files.join(", "),
        n => format!("{} and {} more", files[..4].join(", "), n - 4),
    }
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};
    use std::process::Command as Cmd;
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    use graft_core::Console;
    use graft_core::jobs::JobPool;

    use super::*;

    fn git(dir: &Path, args: &[&str]) {
        let status = Cmd::new("git")
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

    /// Poll until `done` holds for the project.
    fn wait(project: &mut Project, what: &str, done: impl Fn(&Project) -> bool) {
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

    fn branch_at(project: &Project, path: &Path) -> Option<String> {
        project
            .snapshot
            .as_ref()?
            .worktrees
            .iter()
            .find(|wt| graft_core::same_path(&wt.path, path))?
            .branch
            .clone()
    }

    struct Setup {
        _dir: tempfile::TempDir,
        project: Project,
        main: PathBuf,
        linked: PathBuf,
    }

    /// A repo on `main` with a branch `other` (a.txt differs) and a linked
    /// worktree on `agent`, which is selected. `prepare` runs in the main
    /// folder before Graft opens it.
    fn setup(prepare: impl FnOnce(&Path)) -> Setup {
        let dir = tempfile::tempdir().unwrap();
        let main = dir.path().join("main");
        let linked = dir.path().join("linked");
        std::fs::create_dir_all(&main).unwrap();
        git(&main, &["init", "-q", "-b", "main"]);
        std::fs::write(main.join("a.txt"), "a\n").unwrap();
        git(&main, &["add", "a.txt"]);
        git(&main, &["commit", "-q", "-m", "first"]);
        git(&main, &["switch", "-q", "-c", "other"]);
        std::fs::write(main.join("a.txt"), "other\n").unwrap();
        git(&main, &["commit", "-q", "-am", "other"]);
        git(&main, &["switch", "-q", "main"]);
        let linked_arg = linked.to_str().unwrap();
        git(&main, &["worktree", "add", "-q", "-b", "agent", linked_arg]);
        prepare(&main);

        let pool = JobPool::new(2, Arc::new(|| {}));
        let mut project = Project::new(main.clone(), main.clone(), Console::default(), pool);
        wait(&mut project, "snapshot never came", |p| {
            p.snapshot.is_some()
        });
        // Git's spelling of the folders (the temp folder may be a short name).
        let snap = project.snapshot.clone().unwrap();
        let main = snap.main_worktree().unwrap().path.clone();
        let linked = snap.worktrees.iter().find(|wt| !wt.is_main).unwrap();
        let linked = linked.path.clone();
        project.apply(Action::OpenWorktree(linked.clone()));
        assert_eq!(project.in_main_suffix(), " in the main folder");
        Setup {
            _dir: dir,
            project,
            main,
            linked,
        }
    }

    /// With a linked worktree selected, Check out switches the main folder,
    /// selects it, and leaves the worktree on its branch.
    #[test]
    fn checkout_lands_in_the_main_folder() {
        let Setup {
            _dir,
            mut project,
            main,
            linked,
        } = setup(|_| {});
        project.apply(Action::Checkout("other".to_owned()));
        assert!(graft_core::same_path(&project.nav.worktree, &main));
        wait(&mut project, "main never switched", |p| {
            !p.is_operating() && branch_at(p, &main).as_deref() == Some("other")
        });
        assert_eq!(branch_at(&project, &linked).as_deref(), Some("agent"));
        assert_eq!(project.in_main_suffix(), "");
        assert!(project.notices.is_empty(), "{:?}", project.notices);
    }

    /// Changes in the main folder block the switch; "Stash and switch"
    /// stashes them there, not in the selected worktree.
    #[test]
    fn stash_and_switch_works_in_the_main_folder() {
        let Setup {
            _dir,
            mut project,
            main,
            linked,
        } = setup(|main| std::fs::write(main.join("a.txt"), "local\n").unwrap());
        std::fs::write(linked.join("b.txt"), "agent's work\n").unwrap();
        project.apply(Action::Checkout("other".to_owned()));
        wait(&mut project, "no stash offer", |p| p.confirm.is_some());
        let action = project.confirm.take().unwrap().action;
        assert!(matches!(action, Action::StashAndCheckout { .. }));
        project.apply(action);
        wait(&mut project, "main never switched", |p| {
            !p.is_operating() && branch_at(p, &main).as_deref() == Some("other")
        });
        assert!(project.notices.is_empty(), "{:?}", project.notices);
        let stashes = Cmd::new("git")
            .args(["stash", "list"])
            .current_dir(&main)
            .output()
            .unwrap();
        assert_eq!(String::from_utf8_lossy(&stashes.stdout).lines().count(), 1);
        // The worktree kept its branch and its own uncommitted file.
        assert_eq!(branch_at(&project, &linked).as_deref(), Some("agent"));
        assert!(linked.join("b.txt").exists());
    }

    /// A merge stopped in the main folder: Check out says why it can't,
    /// and nothing switches.
    #[test]
    fn no_checkout_while_main_is_mid_merge() {
        let Setup {
            _dir,
            mut project,
            main,
            ..
        } = setup(|main| {
            std::fs::write(main.join("a.txt"), "main\n").unwrap();
            git(main, &["commit", "-q", "-am", "main"]);
            let merge = Cmd::new("git")
                .args(["-c", "user.name=t", "-c", "user.email=t@t"])
                .args(["merge", "-q", "other"])
                .current_dir(main)
                .output()
                .unwrap();
            assert!(!merge.status.success(), "the merge should conflict");
        });
        project.apply(Action::Checkout("other".to_owned()));
        assert!(!project.is_operating());
        assert_eq!(project.notices.len(), 1);
        assert!(project.notices[0].contains("Finish or abort"));
        assert_eq!(branch_at(&project, &main).as_deref(), Some("main"));
    }

    /// A drafted commit message in the selected worktree survives a
    /// checkout: the main folder switches, the selection stays.
    #[test]
    fn checkout_keeps_a_drafted_message() {
        let Setup {
            _dir,
            mut project,
            main,
            linked,
        } = setup(|_| {});
        project.commit_form.summary = "half-written".to_owned();
        project.apply(Action::Checkout("other".to_owned()));
        wait(&mut project, "main never switched", |p| {
            !p.is_operating() && branch_at(p, &main).as_deref() == Some("other")
        });
        assert!(graft_core::same_path(&project.nav.worktree, &linked));
        assert_eq!(project.commit_form.summary, "half-written");
    }

    #[test]
    fn lists_files_in_the_way() {
        let error = "error: Your local changes to the following files would be overwritten \
                     by checkout:\n\tsrc/a.rs\n\tb.txt\nPlease commit your changes or stash \
                     them before you switch branches.\nAborting";
        assert!(blocked_by_changes(error));
        assert_eq!(files_in_the_way(error), "src/a.rs, b.txt");
        assert_eq!(files_in_the_way("fatal: something else"), "some files");
    }
}
