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
    /// Go to the worktree that has `branch` checked out (main's during a
    /// preview, which git sees detached).
    GoTo {
        worktree: &'a Worktree,
        branch: String,
    },
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
            Some(worktree) => Some(Open::GoTo {
                worktree,
                branch: name.to_owned(),
            }),
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
            Open::GoTo { worktree, .. } => Action::OpenWorktree(worktree.path.clone()),
            Open::Checkout(name) => Action::Checkout(name.clone()),
            Open::Track { local, remote } => Action::CheckoutRemote {
                local: local.clone(),
                remote: remote.clone(),
            },
        }
    }

    /// Whether it can happen now: going to a worktree always can, a
    /// checkout only when [`can::checkout`].
    pub fn can(&self, project: &Project) -> Can {
        match self {
            Open::GoTo { .. } => can::yes(""),
            _ => can::checkout(project),
        }
    }

    /// The menu entry; `named` says which branch it is about.
    fn label(&self, project: &Project, named: bool) -> String {
        let in_main = project.in_main_suffix();
        match self {
            Open::GoTo { worktree, branch } => match (worktree.is_main, named) {
                (true, true) => format!("Go to {branch} in the main folder"),
                (true, false) => "Go to the main folder".to_owned(),
                (false, true) => format!("Open the worktree of {branch}"),
                (false, false) => "Open its worktree".to_owned(),
            },
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
            Open::GoTo { .. } => {}
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{git, open, wait};

    /// During a preview git sees main detached, but its branch's menu still
    /// names the branch.
    #[test]
    fn going_to_main_names_its_branch_during_a_preview() {
        let dir = tempfile::tempdir().unwrap();
        let main = dir.path().join("main");
        std::fs::create_dir_all(&main).unwrap();
        git(&main, &["init", "-q", "-b", "main"]);
        std::fs::write(main.join("a.txt"), "a\n").unwrap();
        git(&main, &["add", "a.txt"]);
        git(&main, &["commit", "-q", "-m", "first"]);
        let agent = dir.path().join("agent");
        let agent_arg = agent.to_str().unwrap();
        git(&main, &["worktree", "add", "-q", "-b", "agent", agent_arg]);
        let mut project = open(&main);
        let agent = project.snapshot.as_ref().unwrap().worktrees[1].path.clone();
        project.apply(Action::PreviewInMain(agent.clone()));
        wait(&mut project, "the preview never showed", |p| {
            p.snapshot.as_ref().is_some_and(|s| s.preview.is_some())
        });
        project.apply(Action::OpenWorktree(agent));
        let snap = project.snapshot.clone().unwrap();
        let open = Open::local(&project, &snap, "main").unwrap();
        assert_eq!(open.label(&project, true), "Go to main in the main folder");
    }
}
