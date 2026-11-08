//! Context menu entries for a branch, alike in the sidebar and in History.
//! History lists several branches in one menu, so it asks for `named`
//! labels that say which branch an entry is about.

use egui::{Color32, Ui};
use graft_core::paths::same_path;
use graft_core::{RemoteBranch, RepoSnapshot};

use crate::project::{Action, CheckedOut, Project};
use crate::theme::color;
use crate::ui::Actions;

/// A context menu entry that sends `action` and closes the menu.
pub fn menu_item(ui: &mut Ui, label: &str, enabled: bool, actions: &mut Actions, action: Action) {
    if ui.add_enabled(enabled, egui::Button::new(label)).clicked() {
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

/// Go to the worktree that has the local branch `name`, or check it out or
/// open a new worktree for it. Nothing for the selected worktree's own
/// branch. Returns whether anything was offered.
pub fn local_items(
    ui: &mut Ui,
    project: &Project,
    repo: &RepoSnapshot,
    name: &str,
    named: bool,
    actions: &mut Actions,
) -> bool {
    match repo.worktree_for_branch(name) {
        Some(wt) if same_path(&wt.path, &project.nav.worktree) => false,
        Some(wt) => {
            let label = match (wt.is_main, named) {
                (true, true) => format!("Go to {name} in the main folder"),
                (true, false) => "Go to the main folder".to_owned(),
                (false, true) => format!("Open the worktree of {name}"),
                (false, false) => "Open its worktree".to_owned(),
            };
            menu_item(
                ui,
                &label,
                true,
                actions,
                Action::OpenWorktree(wt.path.clone()),
            );
            true
        }
        None => {
            let busy = project.is_operating();
            menu_item(
                ui,
                &format!("Check out {name}{}", project.in_main_suffix()),
                !busy,
                actions,
                Action::Checkout(name.to_owned()),
            );
            new_worktree_item(ui, name, named, busy, actions);
            true
        }
    }
}

/// Check out the remote branch as a new local branch that tracks it, or open
/// a new worktree for it. For remote branches without a local branch of
/// their name.
pub fn tracking_items(
    ui: &mut Ui,
    project: &Project,
    branch: &RemoteBranch,
    named: bool,
    actions: &mut Actions,
) {
    let busy = project.is_operating();
    let local = branch.branch_name();
    let label = if named {
        format!("Check out {} as {local}", branch.name)
    } else {
        format!("Check out as {local}")
    };
    menu_item(
        ui,
        &format!("{label}{}", project.in_main_suffix()),
        !busy,
        actions,
        Action::CheckoutRemote {
            local: local.to_owned(),
            remote: branch.name.clone(),
        },
    );
    new_worktree_item(ui, &branch.name, named, busy, actions);
}

fn new_worktree_item(ui: &mut Ui, name: &str, named: bool, busy: bool, actions: &mut Actions) {
    let label = if named {
        format!("New worktree for {name}…")
    } else {
        "New worktree for this branch…".to_owned()
    };
    menu_item(
        ui,
        &label,
        !busy,
        actions,
        Action::NewWorktree(Some(name.to_owned())),
    );
}
