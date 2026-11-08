//! Context menu entries for a branch, alike in the sidebar and in History,
//! and what opening one does (its first entry, and a double-click).
//! History lists several branches in one menu, so it asks for `named`
//! labels that say which branch an entry is about.

use egui::{Color32, Ui};
use graft_core::paths::same_path;
use graft_core::{RemoteBranch, RepoSnapshot, Worktree};

use crate::project::{Action, CheckedOut, Project};
use crate::theme::color;
use crate::ui::Actions;
use crate::ui::can::{self, Can};

/// A context menu entry that sends `action` and closes the menu; when it
/// cannot act now, hovering says why.
pub fn can_item(ui: &mut Ui, label: &str, can: &Can, actions: &mut Actions, action: Action) {
    if ui
        .add_enabled(can.ok, egui::Button::new(label))
        .on_disabled_hover_text(&can.hint)
        .clicked()
    {
        actions.push(action);
        ui.close();
    }
}

/// Where a branch is checked out, as the sidebar and the graph color it:
/// green in the main folder, orange in a linked worktree.
pub fn checkout_color(checked_out: CheckedOut) -> Color32 {
    match checked_out {
        CheckedOut::Main => color::accent(),
        CheckedOut::Worktree => color::branch_dot_agent(),
        CheckedOut::Nowhere => color::ahead_fg(),
    }
}

/// What opening a branch (double-click, or the first entry of its menu)
/// does.
pub enum Open<'a> {
    /// Go to the worktree that has it checked out.
    GoTo(&'a Worktree),
    /// Check out the local branch in the main folder.
    Checkout(String),
    /// Check out a remote branch as a new local branch that tracks it.
    Track { local: String, remote: String },
}

impl Open<'_> {
    /// Opening a local branch `name`; `None` for the selected worktree's
    /// own branch.
    pub fn local<'a>(project: &Project, repo: &'a RepoSnapshot, name: &str) -> Option<Open<'a>> {
        match repo.worktree_for_branch(name) {
            Some(wt) if same_path(&wt.path, &project.nav.worktree) => None,
            Some(wt) => Some(Open::GoTo(wt)),
            None => Some(Open::Checkout(name.to_owned())),
        }
    }

    /// Opening a remote branch: its local branch of the same name if there
    /// is one, else a new one that tracks it.
    pub fn remote<'a>(
        project: &Project,
        repo: &'a RepoSnapshot,
        branch: &RemoteBranch,
    ) -> Option<Open<'a>> {
        let local = branch.branch_name();
        if repo.branch(local).is_some() {
            return Self::local(project, repo, local);
        }
        Some(Open::Track {
            local: local.to_owned(),
            remote: branch.name.clone(),
        })
    }

    pub fn action(&self) -> Action {
        match self {
            Open::GoTo(wt) => Action::OpenWorktree(wt.path.clone()),
            Open::Checkout(name) => Action::Checkout(name.clone()),
            Open::Track { local, remote } => Action::CheckoutRemote {
                local: local.clone(),
                remote: remote.clone(),
            },
        }
    }

    /// Whether it can happen now: going to a worktree always can, a
    /// checkout waits for the running operation.
    pub fn can(&self, project: &Project) -> Can {
        match self {
            Open::GoTo(_) => can::yes(""),
            _ => can::act(project, ""),
        }
    }

    /// The menu entry; `named` says which branch it is about.
    fn label(&self, project: &Project, named: bool) -> String {
        let in_main = project.in_main_suffix();
        match self {
            Open::GoTo(wt) => {
                let name = wt.branch.as_deref().unwrap_or_default();
                match (wt.is_main, named) {
                    (true, true) => format!("Go to {name} in the main folder"),
                    (true, false) => "Go to the main folder".to_owned(),
                    (false, true) => format!("Open the worktree of {name}"),
                    (false, false) => "Open its worktree".to_owned(),
                }
            }
            Open::Checkout(name) => format!("Check out {name}{in_main}"),
            Open::Track { local, remote } if named => {
                format!("Check out {remote} as {local}{in_main}")
            }
            Open::Track { local, .. } => format!("Check out as {local}{in_main}"),
        }
    }

    /// The menu entry, then "New worktree…" unless the branch has one.
    fn items(&self, ui: &mut Ui, project: &Project, named: bool, actions: &mut Actions) {
        let label = self.label(project, named);
        can_item(ui, &label, &self.can(project), actions, self.action());
        let can = can::new_worktree(project);
        match self {
            Open::GoTo(_) => {}
            Open::Checkout(name) => new_worktree_item(ui, name, named, &can, actions),
            Open::Track { remote, .. } => new_worktree_item(ui, remote, named, &can, actions),
        }
    }
}

/// Open the local branch `name` (see [`Open::local`]), or open a new
/// worktree for it. Returns whether anything was offered.
pub fn local_items(
    ui: &mut Ui,
    project: &Project,
    repo: &RepoSnapshot,
    name: &str,
    named: bool,
    actions: &mut Actions,
) -> bool {
    let Some(open) = Open::local(project, repo, name) else {
        return false;
    };
    open.items(ui, project, named, actions);
    true
}

/// Open the remote branch (see [`Open::remote`]), or open a new worktree
/// for it. Returns whether anything was offered.
pub fn remote_items(
    ui: &mut Ui,
    project: &Project,
    repo: &RepoSnapshot,
    branch: &RemoteBranch,
    actions: &mut Actions,
) -> bool {
    let Some(open) = Open::remote(project, repo, branch) else {
        return false;
    };
    open.items(ui, project, false, actions);
    true
}

/// Check out the remote branch as a new local branch that tracks it, or open
/// a new worktree for it. Only for a remote branch without a local branch
/// of its name; returns whether anything was offered.
pub fn tracking_items(
    ui: &mut Ui,
    project: &Project,
    repo: &RepoSnapshot,
    branch: &RemoteBranch,
    named: bool,
    actions: &mut Actions,
) -> bool {
    let Some(open @ Open::Track { .. }) = Open::remote(project, repo, branch) else {
        return false;
    };
    open.items(ui, project, named, actions);
    true
}

fn new_worktree_item(ui: &mut Ui, name: &str, named: bool, can: &Can, actions: &mut Actions) {
    let label = if named {
        format!("New worktree for {name}…")
    } else {
        "New worktree for this branch…".to_owned()
    };
    can_item(
        ui,
        &label,
        can,
        actions,
        Action::NewWorktree(Some(name.to_owned())),
    );
}
