//! The always-visible toolbar: Commit, Pull, Push, Fetch | Branch, Merge, Stash,
//! New worktree | Preview in main, Slash | search.

use egui::{Align, Layout, Ui};

use super::icons::Icon;
use super::widgets::{Tone, tool_button, toolbar_divider};
use crate::project::{Action, DialogKind, Project, RemoteOp, Tab, merge_dialog};

pub fn show(ui: &mut Ui, project: &Project, actions: &mut Vec<Action>, search: &mut String) {
    let ahead = project.worktree().map_or(0, |wt| wt.ahead());

    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;

        if tool_button(ui, Some(Icon::Commit), "Commit", None, Tone::Normal).clicked() {
            actions.push(Action::ShowFileStatus);
        }
        let busy = project.is_operating() || project.snapshot.is_none();
        let behind = project.worktree().map_or(0, |wt| wt.behind());
        let pull_badge = (behind > 0).then(|| behind.to_string());
        if tool_button(ui, Some(Icon::Pull), "Pull", pull_badge, Tone::Normal)
            .on_hover_text("Pull the current branch from its upstream")
            .clicked()
            && !busy
        {
            actions.push(Action::Remote(RemoteOp::Pull));
        }
        let push_badge = (ahead > 0).then(|| ahead.to_string());
        if tool_button(ui, Some(Icon::Push), "Push", push_badge, Tone::Normal)
            .on_hover_text("Push the current branch (sets the upstream the first time)")
            .clicked()
            && !busy
        {
            actions.push(Action::Remote(RemoteOp::Push));
        }
        if tool_button(ui, Some(Icon::Fetch), "Fetch", None, Tone::Normal)
            .on_hover_text("Fetch all remotes")
            .clicked()
            && !busy
        {
            actions.push(Action::Remote(RemoteOp::Fetch));
        }
        toolbar_divider(ui);

        if tool_button(ui, None, "Branch", None, Tone::Normal)
            .on_hover_text("New branch from the current commit")
            .clicked()
            && !busy
            && let Some(wt) = project.worktree()
        {
            let (start, start_label) = match (&wt.branch, wt.head) {
                (Some(b), _) => (b.clone(), b.clone()),
                (None, Some(id)) => (id.to_string(), graft_core::short(&id)),
                (None, None) => ("HEAD".to_owned(), "HEAD".to_owned()),
            };
            actions.push(Action::OpenDialog(DialogKind::NewBranch {
                name: String::new(),
                start,
                start_label,
                checkout: true,
            }));
        }
        let wt = project.worktree();
        let merge_hint = match wt {
            Some(wt) if wt.in_progress.is_some() => "Finish or abort what is in progress first",
            Some(wt) if wt.branch.is_none() => "Check out a branch to merge into",
            _ => "Merge another branch into the current one",
        };
        let can_merge = wt.is_some_and(|wt| wt.branch.is_some() && wt.in_progress.is_none());
        if tool_button(ui, Some(Icon::Merge), "Merge", None, Tone::Normal)
            .on_hover_text(merge_hint)
            .clicked()
            && !busy
            && can_merge
            && let Some(kind) = merge_dialog(project, None)
        {
            actions.push(Action::OpenDialog(kind));
        }
        let dirty = project.worktree().is_some_and(|wt| wt.dirty() > 0);
        if tool_button(ui, None, "Stash", None, Tone::Normal)
            .on_hover_text(if dirty {
                "Put this worktree's changes aside"
            } else {
                "Nothing to stash: the worktree is clean"
            })
            .clicked()
            && !busy
        {
            if dirty {
                actions.push(Action::OpenDialog(DialogKind::Stash {
                    message: String::new(),
                    include_untracked: true,
                    keep_index: false,
                    staged_only: false,
                }));
            } else {
                actions.push(Action::SetTab(Tab::Stashes));
            }
        }
        if tool_button(ui, None, "New worktree", None, Tone::Normal)
            .on_hover_text("Check out a branch in a second folder")
            .clicked()
            && !busy
        {
            actions.push(Action::NewWorktree(None));
        }
        toolbar_divider(ui);

        let preview = project.snapshot.as_ref().and_then(|s| s.preview.as_ref());
        let (preview_hint, preview_action) = match (preview, wt) {
            (Some(p), Some(wt)) if graft_core::same_path(&p.source, &wt.path) => (
                "Main shows this worktree: show its current state again",
                Some(Action::UpdatePreview),
            ),
            (Some(_), _) => ("Main already shows a preview. Restore main first.", None),
            (None, Some(wt)) if wt.is_main => (
                "Select another worktree to show its state in the main worktree",
                None,
            ),
            (None, Some(wt)) => (
                "Check out this worktree's commit plus its uncommitted changes in the \
                 main worktree, e.g. to run it from there. Main must be clean.",
                Some(Action::PreviewInMain(wt.path.clone())),
            ),
            (None, None) => ("Select a worktree first", None),
        };
        if tool_button(ui, None, "Preview in main", None, Tone::Normal)
            .on_hover_text(preview_hint)
            .clicked()
            && !busy
            && let Some(action) = preview_action
        {
            actions.push(action);
        }
        if tool_button(ui, Some(Icon::Slash), "Slash", None, Tone::Danger)
            .on_hover_text("Clean up finished branches and worktrees (shows a preview first)")
            .clicked()
            && !busy
        {
            actions.push(Action::OpenSlash);
        }

        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            super::search::show(ui, project, search, actions)
        });
    });
}
