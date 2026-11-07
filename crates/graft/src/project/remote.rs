//! Fetch, pull and push, with progress and cancel, and what to offer when the
//! remote rejects a push.

use graft_core::remote::{self, is_rejected_push};
use graft_core::{CancelToken, Git, Progress};

use super::ops::{At, quietly, say};
use super::{Action, Confirm, Project};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RemoteOp {
    Fetch,
    Pull,
    Push,
    /// Push over a diverged upstream, with lease.
    ForcePush,
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
            Self::ForcePush => "Force pushing".to_owned(),
            Self::PushTag { tag, .. } => format!("Pushing tag {tag}"),
            Self::DeleteRemoteBranch { remote, branch } => {
                format!("Deleting {remote}/{branch}")
            }
        }
    }
}

impl Project {
    /// Talk to the remote from the selected worktree. False while another
    /// operation runs.
    pub(super) fn remote(&mut self, op: RemoteOp) -> bool {
        let label = op.label();
        let job = {
            let op = op.clone();
            move |git: Git, cancel: &CancelToken, report: &dyn Fn(Progress)| match op {
                RemoteOp::Fetch => remote::fetch(&git, false, cancel, report),
                RemoteOp::Pull => remote::pull(&git, cancel, report),
                RemoteOp::Push => remote::push(&git, cancel, report),
                RemoteOp::ForcePush => remote::force_push(&git, cancel, report),
                RemoteOp::PushTag { remote, tag } => {
                    remote::push_tag(&git, &remote, &tag, cancel, report)
                }
                RemoteOp::DeleteRemoteBranch { remote, branch } => {
                    remote::delete_remote_branch(&git, &remote, &branch, cancel, report)
                }
            }
        };
        match op {
            RemoteOp::Fetch => self.run_long(At::Selected, &label, job, quietly),
            RemoteOp::Pull => self.run_long(At::Selected, &label, job, say("Pulled".to_owned())),
            RemoteOp::Push => self.run_long(At::Selected, &label, job, Self::pushed),
            RemoteOp::ForcePush => {
                self.run_long(At::Selected, &label, job, say("Force pushed".to_owned()))
            }
            RemoteOp::PushTag { remote, tag } => self.run_long(
                At::Selected,
                &label,
                job,
                say(format!("Pushed tag {tag} to {remote}")),
            ),
            RemoteOp::DeleteRemoteBranch { remote, branch } => self.run_long(
                At::Selected,
                &label,
                job,
                say(format!("Deleted {remote}/{branch}")),
            ),
        }
    }

    /// A plain push ended; if the remote rejected it, offer to force push.
    fn pushed(&mut self, result: anyhow::Result<()>) {
        match result {
            Ok(()) => self.infos.push("Pushed".to_owned()),
            Err(e) if is_rejected_push(&format!("{e:#}")) => {
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
            Err(e) => self.report(e),
        }
    }
}
