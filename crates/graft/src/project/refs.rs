//! Branches, tags and commit actions: the small dialogs that ask for a name,
//! and what happens when git says no (unmerged branch, changes in the way).

use std::path::PathBuf;

use graft_core::refs::{self, RefKind, ResetMode, check_ref_name};
use graft_core::refusal::{blocked_by_changes, blocked_by_untracked, files_in_the_way, not_merged};
use graft_core::{Git, Oid, Worktree, conflicts, remote, stash};

use super::conflicts::after_conflicts;
use super::ops::{At, say};
use super::{Action, Confirm, Project, Selection, Tab, WorkspaceView};
use crate::jobs::shown_error;

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
        /// `url` is the push URL of a remote that has only that.
        push_only: bool,
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

impl DialogKind {
    /// A new branch at `start`, checked out once created.
    pub fn new_branch(start: impl Into<String>, start_label: impl Into<String>) -> Self {
        Self::NewBranch {
            name: String::new(),
            start: start.into(),
            start_label: start_label.into(),
            checkout: true,
        }
    }

    /// A new branch at `wt`'s HEAD: its branch, or its commit when detached.
    pub fn new_branch_at(wt: &Worktree) -> Self {
        match (&wt.branch, wt.head) {
            (Some(branch), _) => Self::new_branch(branch, branch),
            (None, Some(id)) => Self::new_branch(id.to_string(), graft_core::short(&id)),
            (None, None) => Self::new_branch("HEAD", "HEAD"),
        }
    }

    pub fn stash() -> Self {
        Self::Stash {
            message: String::new(),
            include_untracked: true,
            keep_index: false,
            staged_only: false,
        }
    }

    /// Add a remote; the first one is suggested as `origin`.
    pub fn add_remote(first: bool) -> Self {
        Self::Remote {
            original: None,
            name: if first { graft_core::ORIGIN } else { "" }.to_owned(),
            url: String::new(),
            push_only: false,
            fetch: true,
        }
    }

    pub fn edit_remote(remote: &graft_core::Remote) -> Self {
        Self::Remote {
            original: Some(remote.name.clone()),
            name: remote.name.clone(),
            url: remote.url.clone(),
            push_only: remote.push_only,
            fetch: false,
        }
    }

    /// Whether submitting moves HEAD in the selected worktree (see
    /// [`Project::head_blocked`]).
    fn moves_head(&self) -> bool {
        match self {
            Self::NewBranch { checkout, .. } => *checkout,
            Self::Reset { .. } | Self::StashBranch { .. } | Self::Merge { .. } => true,
            Self::RenameBranch { .. }
            | Self::NewTag { .. }
            | Self::Stash { .. }
            | Self::Remote { .. } => false,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Dialog {
    pub kind: DialogKind,
    pub error: Option<String>,
    pub submitting: bool,
    /// Focus the first field once.
    pub focus: bool,
    /// What was selected when it opened.
    asked: Selection,
}

impl Dialog {
    pub fn new(kind: DialogKind, asked: Selection) -> Self {
        Self {
            kind,
            error: None,
            submitting: false,
            focus: true,
            asked,
        }
    }
}

/// The ending of an operation submitted from the open dialog: success
/// closes it, then `then` runs; an error shows in the dialog, which can be
/// tried again. Branch from stash switches the branch before it can fail,
/// so after its failure any branch will do; other dialogs still refuse a
/// branch that changed meanwhile.
fn from_dialog<T>(
    then: impl FnOnce(&mut Project, PathBuf, anyhow::Result<T>) + Send,
) -> impl FnOnce(&mut Project, PathBuf, anyhow::Result<T>) + Send {
    move |p, ran_in, result| {
        if let Err(e) = &result
            && let Some(dialog) = &mut p.dialog
        {
            dialog.submitting = false;
            dialog.error = shown_error(e);
            if matches!(dialog.kind, DialogKind::StashBranch { .. }) {
                dialog.asked.branch_changed_by_us();
            }
            return;
        }
        if result.is_ok() {
            p.dialog = None;
        }
        then(p, ran_in, result);
    }
}

/// The ending of a switch to `branch`: if local changes are in the way,
/// offer to stash them first.
fn switched(
    branch: String,
) -> impl FnOnce(&mut Project, PathBuf, anyhow::Result<Option<String>>) + Send {
    move |p, _, result| match result {
        Ok(Some(extra)) => p.infos.push(format!("Switched to {branch}. {extra}")),
        Ok(None) => p.infos.push(format!("Switched to {branch}")),
        Err(e) if blocked_by_changes(&format!("{e:#}")) => {
            let error = format!("{e:#}");
            p.ask(Confirm {
                title: format!("Your changes are in the way of {branch}"),
                body: format!(
                    "Switching would overwrite uncommitted changes in your main folder: \
                     {}.\n\nGraft can stash them first; get them back later from \
                     Stashes (Apply or Pop).",
                    files_in_the_way(&error)
                ),
                confirm_label: "Stash and switch".to_owned(),
                action: Action::StashAndCheckout {
                    branch,
                    untracked: blocked_by_untracked(&error),
                },
            });
        }
        Err(e) => p.report(e),
    }
}

impl Project {
    pub(super) fn open_dialog(&mut self, kind: DialogKind) {
        if !self.is_operating() {
            self.dialog = Some(Dialog::new(kind, self.selection()));
        }
    }

    pub(super) fn close_dialog(&mut self) {
        if !self.dialog.as_ref().is_some_and(|d| d.submitting) {
            self.dialog = None;
        }
    }

    /// Problems in the open dialog's fields that git would only report
    /// after a round trip.
    fn dialog_problem(&self, kind: &DialogKind) -> Option<String> {
        if kind.moves_head()
            && let Some(reason) = self.head_blocked(&self.nav.worktree)
        {
            return Some(reason.to_owned());
        }
        let snap = self.snapshot.as_deref();
        match kind {
            DialogKind::NewBranch { name, .. }
            | DialogKind::RenameBranch { name, .. }
            | DialogKind::StashBranch { name, .. } => {
                let name = name.trim();
                if let Err(problem) = check_ref_name(RefKind::Branch, name) {
                    Some(problem)
                } else if snap.is_some_and(|s| s.branch(name).is_some()) {
                    Some(format!("A branch named {name} already exists."))
                } else {
                    None
                }
            }
            DialogKind::NewTag { name, .. } => {
                let name = name.trim();
                if let Err(problem) = check_ref_name(RefKind::Tag, name) {
                    Some(problem)
                } else if snap.is_some_and(|s| s.tags.iter().any(|t| t.name == name)) {
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
                    && snap.is_some_and(|s| s.remotes.iter().any(|r| r.name == name));
                if let Err(problem) = check_ref_name(RefKind::Remote, name) {
                    Some(problem)
                } else if taken {
                    Some(format!("A remote named {name} already exists."))
                } else if url.trim().is_empty() {
                    Some("Enter the repository's URL.".to_owned())
                } else {
                    None
                }
            }
        }
    }

    pub(super) fn submit_dialog(&mut self) {
        let Some(dialog) = &self.dialog else { return };
        if dialog.submitting {
            return;
        }
        let (kind, asked) = (dialog.kind.clone(), dialog.asked.clone());
        if !self.still_selected(&asked) {
            self.dialog = None;
            return;
        }
        if let Some(problem) = self.dialog_problem(&kind) {
            if let Some(dialog) = &mut self.dialog {
                dialog.error = Some(problem);
            }
            return;
        }
        let started = self.start_dialog_op(kind);
        if started && let Some(dialog) = &mut self.dialog {
            dialog.error = None;
            dialog.submitting = true;
        }
    }

    /// Run what the dialog asks for; false if it could not start.
    fn start_dialog_op(&mut self, kind: DialogKind) -> bool {
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
                self.run_op(
                    At::Selected,
                    "Creating branch",
                    move |git| refs::create_branch(&git, &name, &start, checkout),
                    from_dialog(say(done)),
                )
            }
            DialogKind::RenameBranch { old, name } => {
                let name = name.trim().to_owned();
                let done = format!("Renamed {old} to {name}");
                self.run_op(
                    At::Selected,
                    "Renaming branch",
                    move |git| refs::rename_branch(&git, &old, &name),
                    from_dialog(say(done)),
                )
            }
            DialogKind::NewTag {
                name,
                message,
                target,
                ..
            } => {
                let name = name.trim().to_owned();
                let done = format!("Created tag {name}");
                self.run_op(
                    At::Selected,
                    "Creating tag",
                    move |git| refs::create_tag(&git, &name, &target, Some(&message)),
                    from_dialog(say(done)),
                )
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
                self.run_op(
                    At::Selected,
                    "Stashing",
                    move |git| stash::push(&git, &options),
                    from_dialog(say("Stashed".to_owned())),
                )
            }
            DialogKind::StashBranch { id, name, .. } => {
                let name = name.trim().to_owned();
                let done = format!("Created {name} from the stash and switched to it");
                self.run_op(
                    At::Selected,
                    "Creating branch",
                    move |git| stash::branch(&git, &name, id),
                    from_dialog(say(done)),
                )
            }
            DialogKind::Merge {
                into, rev, no_ff, ..
            } => {
                let (done, doing) = (
                    format!("Merged {rev} into {into}"),
                    format!("Merging {rev}"),
                );
                self.run_op(
                    At::Selected,
                    "Merging",
                    move |git| graft_core::conflicts::merge(&git, &rev, no_ff),
                    from_dialog(after_conflicts(done, doing, None)),
                )
            }
            DialogKind::Remote {
                original,
                name,
                url,
                push_only,
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
                self.run_op(
                    At::Selected,
                    label,
                    move |git| {
                        remote::save_remote(
                            &git,
                            original.as_deref(),
                            &name,
                            &url,
                            push_only,
                            fetch,
                        )
                    },
                    from_dialog(say(done)),
                )
            }
            DialogKind::Reset {
                target,
                target_label,
                branch,
                mode,
            } => {
                let done = format!("Reset {branch} to {target_label}");
                self.run_op(
                    At::Selected,
                    "Resetting",
                    move |git| refs::reset(&git, &target, mode),
                    from_dialog(say(done)),
                )
            }
        }
    }

    /// The main folder. Checkouts always happen there, never in a linked
    /// worktree: a worktree keeps its branch.
    pub fn main_worktree(&self) -> Option<&graft_core::Worktree> {
        self.snapshot.as_ref()?.main_worktree()
    }

    /// Why the main folder can't switch now, for a readable notice.
    fn checkout_blocked(&self) -> Option<&'static str> {
        self.main_worktree()?.in_progress.is_some().then_some(
            "A merge, rebase, cherry-pick or revert is still going on in the main folder. \
             Finish or abort it first, then check out.",
        )
    }

    /// " in the main folder" while a linked worktree is selected, so a menu
    /// says where a checkout lands; empty while the main folder is selected.
    pub fn in_main_suffix(&self) -> &'static str {
        if self.worktree().is_some_and(|wt| !wt.is_main) {
            " in the main folder"
        } else {
            ""
        }
    }

    /// Check out in the main folder, never in a linked worktree, and select
    /// the main folder so the user sees where it happened. Stays put on
    /// Review or with a drafted commit message, which selecting would reset.
    fn checkout<R: Send + 'static>(
        &mut self,
        label: &str,
        job: impl FnOnce(Git) -> anyhow::Result<R> + Send + 'static,
        done: impl FnOnce(&mut Project, PathBuf, anyhow::Result<R>) + Send + 'static,
    ) {
        if let Some(reason) = self.checkout_blocked() {
            self.notices.push(reason.to_owned());
            return;
        }
        if !self.run_head_op(At::Main, label, job, done) {
            return;
        }
        if self.nav.tab != Tab::Review
            && !self.commit_form.is_drafted()
            && let Some(main) = self.main_worktree()
            && !graft_core::paths::same_path(&main.path, &self.nav.worktree)
        {
            let main = main.path.clone();
            self.open_worktree(main);
        }
    }

    pub(super) fn switch(&mut self, branch: String) {
        let done = switched(branch.clone());
        self.checkout(
            "Switching branch",
            move |git| refs::switch(&git, &branch).map(|()| None),
            done,
        );
    }

    pub(super) fn stash_and_switch(&mut self, branch: String, untracked: bool) {
        let done = switched(branch.clone());
        self.checkout(
            "Switching branch",
            move |git| {
                let message = format!("Before switching to {branch}");
                stash::push(
                    &git,
                    &stash::Options {
                        message: message.clone(),
                        include_untracked: untracked,
                        ..Default::default()
                    },
                )?;
                refs::switch(&git, &branch)?;
                Ok(Some(format!("Your changes are in the stash \"{message}\"")))
            },
            done,
        );
    }

    pub(super) fn switch_tracking(&mut self, local: String, remote: String) {
        let done = format!("Switched to {local}, tracking {remote}");
        self.checkout(
            "Switching branch",
            move |git| refs::switch_tracking(&git, &local, &remote),
            say(done),
        );
    }

    pub(super) fn switch_detached(&mut self, id: Oid) {
        let done = format!("Checked out {}", graft_core::short(&id));
        self.checkout(
            "Checking out",
            move |git| refs::switch_detached(&git, &id.to_string()),
            say(done),
        );
    }

    pub(super) fn delete_branch(&mut self, name: String, force: bool) {
        let target = name.clone();
        self.run_op(
            At::Main,
            "Deleting branch",
            move |git| refs::delete_branch(&git, &target, force),
            move |p, _, result| match result {
                Ok(()) => p.infos.push(format!("Deleted branch {name}")),
                Err(e) if not_merged(&format!("{e:#}")) => {
                    p.ask(Confirm {
                        title: format!("{name} is not merged"),
                        body: format!(
                            "Its commits are not in your current branch or its upstream. \
                             Deleting it anyway can lose those commits.\n\n{e:#}"
                        ),
                        confirm_label: "Delete anyway".to_owned(),
                        action: Action::DeleteBranch { name, force: true },
                    });
                }
                Err(e) => p.report(e),
            },
        );
    }

    pub(super) fn remove_remote(&mut self, name: String) {
        let done = format!("Removed remote {name}");
        self.run_op(
            At::Main,
            "Removing remote",
            move |git| remote::remove_remote(&git, &name),
            say(done),
        );
    }

    pub(super) fn delete_tag(&mut self, name: String) {
        let done = format!("Deleted tag {name}");
        self.run_op(
            At::Selected,
            "Deleting tag",
            move |git| refs::delete_tag(&git, &name),
            say(done),
        );
    }

    pub(super) fn cherry_pick(&mut self, id: Oid) {
        let short = graft_core::short(&id);
        let done = format!("Cherry-picked {short}");
        let doing = format!("Cherry-picking {short}");
        self.run_head_op(
            At::Selected,
            "Cherry-picking",
            move |git| conflicts::cherry_pick(&git, &id.to_string()),
            after_conflicts(done, doing, None),
        );
    }

    pub(super) fn revert(&mut self, id: Oid) {
        let short = graft_core::short(&id);
        let done = format!("Reverted {short}");
        let doing = format!("Reverting {short}");
        self.run_head_op(
            At::Selected,
            "Reverting",
            move |git| conflicts::revert(&git, &id.to_string()),
            after_conflicts(done, doing, None),
        );
    }

    pub(super) fn undo_commit(&mut self) {
        let started = self.run_head_op(
            At::Selected,
            "Undoing commit",
            move |git| refs::undo_last_commit(&git),
            say("Undid the last commit; its changes are staged".to_owned()),
        );
        if started {
            self.nav.view = WorkspaceView::FileStatus;
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};
    use std::process::Command as Cmd;

    use super::*;
    use crate::testing::{git, open, wait};

    fn branch_at(project: &Project, path: &Path) -> Option<String> {
        project.snapshot.as_ref()?.worktree_at(path)?.branch.clone()
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

        let mut project = open(&main);
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

    /// After a merge stopped with conflicts, File status selects the first
    /// conflict (the top of the list), so the resolver opens, even when a
    /// clean file is staged too.
    #[test]
    fn a_conflicted_merge_opens_the_resolver() {
        let Setup {
            _dir,
            mut project,
            main,
            ..
        } = setup(|main| {
            std::fs::write(main.join("a.txt"), "main\n").unwrap();
            git(main, &["commit", "-q", "-am", "main"]);
            let merge = Cmd::new("git")
                .args(["merge", "other"])
                .current_dir(main)
                .output()
                .unwrap();
            assert!(!merge.status.success(), "the merge must stop");
            std::fs::write(main.join("b.txt"), "b\n").unwrap();
            git(main, &["add", "b.txt"]);
        });
        project.apply(Action::OpenWorktree(main));
        wait(&mut project, "the conflict never opened", |p| {
            p.conflict.is_some()
        });
        let selected = project.diff_view.as_ref().map(|v| v.key.path.as_str());
        assert_eq!(selected, Some("a.txt"));
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
        assert!(graft_core::paths::same_path(&project.nav.worktree, &main));
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
        wait(&mut project, "no stash offer", |p| p.confirm().is_some());
        let action = &project.confirm().unwrap().action;
        assert!(matches!(action, Action::StashAndCheckout { .. }));
        project.apply(Action::ConfirmYes);
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
        assert!(graft_core::paths::same_path(&project.nav.worktree, &linked));
        assert_eq!(project.commit_form.summary, "half-written");
    }

    /// Selecting another worktree on the Review tab drops the old
    /// worktree's File status diff, so nothing stages into the wrong one.
    #[test]
    fn review_worktree_switch_drops_the_old_diff() {
        let Setup {
            _dir,
            mut project,
            main,
            linked,
        } = setup(|_| {});
        std::fs::write(linked.join("b.txt"), "agent's work\n").unwrap();
        project.refresh();
        wait(&mut project, "no diff for the new file", |p| {
            p.diff_view.as_ref().is_some_and(|v| v.key.path == "b.txt")
        });
        project.apply(Action::SetTab(Tab::Review));
        project.apply(Action::OpenWorktree(main));
        assert_eq!(project.nav.tab, Tab::Review);
        assert!(project.diff_view.is_none(), "main is clean: no diff");
    }

    /// `git stash branch` switches to the new branch before it applies the
    /// stash. When applying fails, the dialog stays open and its OK still
    /// works, although the branch changed.
    #[test]
    fn branch_from_stash_can_be_tried_again_after_it_failed() {
        let Setup {
            _dir,
            mut project,
            main,
            ..
        } = setup(|main| {
            std::fs::write(main.join("u.txt"), "stashed\n").unwrap();
            git(main, &["stash", "push", "-q", "-u"]);
            std::fs::write(main.join("u.txt"), "in the way\n").unwrap();
        });
        project.apply(Action::OpenWorktree(main.clone()));
        let out = Cmd::new("git")
            .args(["rev-parse", "stash@{0}"])
            .current_dir(&main)
            .output()
            .unwrap();
        let id = Oid::from_hex(String::from_utf8_lossy(&out.stdout).trim().as_bytes()).unwrap();
        let named = |name: &str| DialogKind::StashBranch {
            id,
            label: "stash@{0}".to_owned(),
            name: name.to_owned(),
        };
        project.apply(Action::OpenDialog(named("first")));
        project.apply(Action::SubmitDialog);
        wait(&mut project, "the stash branch never failed", |p| {
            p.dialog.as_ref().is_some_and(|d| d.error.is_some())
                && branch_at(p, &main).as_deref() == Some("first")
        });

        std::fs::remove_file(main.join("u.txt")).unwrap();
        project.dialog.as_mut().unwrap().kind = named("second");
        project.apply(Action::SubmitDialog);
        assert!(project.notices.is_empty(), "{:?}", project.notices);
        wait(&mut project, "the stash branch never succeeded", |p| {
            p.dialog.is_none() && branch_at(p, &main).as_deref() == Some("second")
        });
        let restored = std::fs::read_to_string(main.join("u.txt")).unwrap();
        assert_eq!(restored.trim_end(), "stashed");
    }

    /// A failed merge leaves the dialog open; when the branch changes
    /// before the next try, OK refuses instead of merging into it.
    #[test]
    fn a_failed_dialog_still_refuses_a_changed_branch() {
        let Setup {
            _dir,
            mut project,
            main,
            ..
        } = setup(|_| {});
        project.apply(Action::OpenWorktree(main.clone()));
        project.apply(Action::OpenDialog(DialogKind::Merge {
            into: "main".to_owned(),
            rev: "no-such-branch".to_owned(),
            choices: Vec::new(),
            filter: String::new(),
            no_ff: false,
        }));
        project.apply(Action::SubmitDialog);
        wait(&mut project, "the merge never failed", |p| {
            p.dialog.as_ref().is_some_and(|d| d.error.is_some())
        });

        git(&main, &["switch", "-q", "other"]);
        project.refresh();
        wait(&mut project, "the switch never showed", |p| {
            branch_at(p, &main).as_deref() == Some("other")
        });
        project.dialog.as_mut().unwrap().kind = DialogKind::Merge {
            into: "main".to_owned(),
            rev: "agent".to_owned(),
            choices: Vec::new(),
            filter: String::new(),
            no_ff: false,
        };
        project.apply(Action::SubmitDialog);
        assert!(project.dialog.is_none());
        assert!(!project.is_operating());
        assert_eq!(project.notices.len(), 1, "{:?}", project.notices);
        assert!(project.notices[0].contains("changed while you were deciding"));
    }

    /// An action refused because another operation runs changes nothing.
    #[test]
    fn refused_operations_leave_the_screen_alone() {
        let Setup {
            _dir, mut project, ..
        } = setup(|_| {});
        project.nav.view = WorkspaceView::History;
        project.apply(Action::Checkout("other".to_owned()));
        assert!(project.is_operating());
        project.apply(Action::UndoCommit);
        assert_eq!(project.nav.view, WorkspaceView::History);
        project.dialog = Some(Dialog::new(DialogKind::stash(), project.selection()));
        project.apply(Action::SubmitDialog);
        assert!(!project.dialog.as_ref().unwrap().submitting);
        wait(&mut project, "checkout never ended", |p| !p.is_operating());
    }
}
