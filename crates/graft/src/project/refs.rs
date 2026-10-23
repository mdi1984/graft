//! Branches, tags and commit actions: the small dialogs that ask for a name,
//! and what happens when git says no (unmerged branch, rejected push).

use graft_core::Oid;
use graft_core::refs::{self, NOT_MERGED, ResetMode};
use graft_core::remote::{self, is_rejected_push};
use graft_core::stash;

use super::changes::Then;
use super::{Action, Confirm, Project, RemoteOp};

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
                let done = format!("Switched to {branch}");
                self.run_op("Switching branch", Then::Info(done), move |git| {
                    refs::switch(&git, &branch)
                });
            }
            Action::CheckoutRemote { local, remote } => {
                let done = format!("Switched to {local}, tracking {remote}");
                self.run_op("Switching branch", Then::Info(done), move |git| {
                    refs::switch_tracking(&git, &local, &remote)
                });
            }
            Action::CheckoutDetached(id) => {
                let rev = id.to_string();
                let done = format!("Checked out {}", graft_core::short(&id));
                self.run_op("Checking out", Then::Info(done), move |git| {
                    refs::switch_detached(&git, &rev)
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
}
