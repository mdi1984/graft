//! The always-visible toolbar: Commit, Pull, Push, Fetch | Branch, Merge, Stash,
//! New worktree | Preview in main folder, Slash | search.
//!
//! A button that cannot do anything right now is drawn disabled; its hover
//! text says why.

use egui::{Align, Layout, Ui};
use graft_core::{RepoSnapshot, Upstream, Worktree};

use super::icons::Icon;
use super::widgets::{Tone, tool_button, toolbar_divider};
use crate::project::{Action, DialogKind, Project, RemoteOp, merge_dialog};

/// Below this width, buttons with an icon drop their label.
const COMPACT_WIDTH: f32 = 1120.0;

/// Whether a button can act now, and its hover text: what it does, or why not.
struct Can {
    ok: bool,
    hint: String,
}

fn yes(hint: impl Into<String>) -> Can {
    Can {
        ok: true,
        hint: hint.into(),
    }
}

fn no(hint: impl Into<String>) -> Can {
    Can {
        ok: false,
        hint: hint.into(),
    }
}

const FINISH_FIRST: &str = "Finish or abort what is in progress first";

pub fn show(ui: &mut Ui, project: &Project, actions: &mut Vec<Action>, search: &mut String) {
    let snap = project.snapshot.as_deref();
    let wt = project.worktree();
    let busy = project.is_operating() || snap.is_none();
    let ahead = wt.map_or(0, |wt| wt.ahead());
    let behind = wt.map_or(0, |wt| wt.behind());
    let compact = ui.available_width() < COMPACT_WIDTH;
    let label = |text: &'static str| if compact { "" } else { text };

    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;

        let can = can_commit(wt);
        if tool_button(
            ui,
            Some(Icon::Commit),
            label("Commit"),
            None,
            Tone::Normal,
            can.ok,
        )
        .on_hover_text(can.hint)
        .clicked()
        {
            actions.push(Action::ShowFileStatus);
        }
        let can = can_pull(snap, wt);
        let pull_badge = (behind > 0).then(|| behind.to_string());
        if tool_button(
            ui,
            Some(Icon::Pull),
            label("Pull"),
            pull_badge,
            Tone::Normal,
            can.ok,
        )
        .on_hover_text(can.hint)
        .clicked()
            && !busy
        {
            actions.push(Action::Remote(RemoteOp::Pull));
        }
        let can = can_push(snap, wt);
        let push_badge = (ahead > 0).then(|| ahead.to_string());
        if tool_button(
            ui,
            Some(Icon::Push),
            label("Push"),
            push_badge,
            Tone::Normal,
            can.ok,
        )
        .on_hover_text(can.hint)
        .clicked()
            && !busy
        {
            actions.push(Action::Remote(RemoteOp::Push));
        }
        let can = if has_remote(snap) {
            yes("Fetch all remotes")
        } else {
            no("No remote to fetch from. Add one under Remotes in the sidebar.")
        };
        if tool_button(
            ui,
            Some(Icon::Fetch),
            label("Fetch"),
            None,
            Tone::Normal,
            can.ok,
        )
        .on_hover_text(can.hint)
        .clicked()
            && !busy
        {
            actions.push(Action::Remote(RemoteOp::Fetch));
        }
        toolbar_divider(ui);

        let can = match wt {
            Some(wt) if wt.head.is_some() => yes("New branch from the current commit"),
            Some(_) => no("Make a first commit before creating branches"),
            None => no("Select a worktree first"),
        };
        if tool_button(ui, None, "Branch", None, Tone::Normal, can.ok)
            .on_hover_text(can.hint)
            .clicked()
            && !busy
            && let Some(wt) = wt
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
        let can = can_merge(snap, wt);
        if tool_button(
            ui,
            Some(Icon::Merge),
            label("Merge"),
            None,
            Tone::Normal,
            can.ok,
        )
        .on_hover_text(can.hint)
        .clicked()
            && !busy
            && let Some(kind) = merge_dialog(project, None)
        {
            actions.push(Action::OpenDialog(kind));
        }
        let can = if wt.is_some_and(|wt| wt.dirty() > 0) {
            yes("Put this worktree's changes aside")
        } else {
            no("Nothing to stash: the worktree is clean")
        };
        if tool_button(ui, None, "Stash", None, Tone::Normal, can.ok)
            .on_hover_text(can.hint)
            .clicked()
            && !busy
        {
            actions.push(Action::OpenDialog(DialogKind::Stash {
                message: String::new(),
                include_untracked: true,
                keep_index: false,
                staged_only: false,
            }));
        }
        if tool_button(ui, None, "New worktree", None, Tone::Normal, snap.is_some())
            .on_hover_text("Check out a branch in a second folder")
            .clicked()
            && !busy
        {
            actions.push(Action::NewWorktree(None));
        }
        toolbar_divider(ui);

        let (can, preview_action) = can_preview(snap, wt);
        if tool_button(
            ui,
            None,
            "Preview in main folder",
            None,
            Tone::Normal,
            can.ok,
        )
        .on_hover_text(can.hint)
        .clicked()
            && !busy
            && let Some(action) = preview_action
        {
            actions.push(action);
        }
        if tool_button(
            ui,
            Some(Icon::Slash),
            label("Slash"),
            None,
            Tone::Danger,
            true,
        )
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

fn has_remote(snap: Option<&RepoSnapshot>) -> bool {
    snap.is_some_and(|s| !s.remotes.is_empty())
}

fn can_commit(wt: Option<&Worktree>) -> Can {
    match wt {
        Some(wt) if wt.dirty() > 0 => yes("Commit: show the changes to stage and commit"),
        Some(_) => no("Nothing to commit: the worktree is clean"),
        None => no("Select a worktree first"),
    }
}

/// The checked-out branch and its upstream, or why there is none.
fn branch_upstream<'a>(
    snap: Option<&'a RepoSnapshot>,
    wt: Option<&'a Worktree>,
    verb: &str,
) -> Result<(&'a str, &'a Upstream), Can> {
    if !has_remote(snap) {
        return Err(no(format!(
            "No remote to {verb}. Add one under Remotes in the sidebar."
        )));
    }
    let Some(wt) = wt else {
        return Err(no("Select a worktree first"));
    };
    if wt.in_progress.is_some() {
        return Err(no(FINISH_FIRST));
    }
    let branch = wt
        .branch
        .as_deref()
        .ok_or_else(|| no(format!("Check out a branch to {verb}")))?;
    let upstream = snap
        .and_then(|s| s.branch(branch))
        .map(|b| &b.upstream)
        .ok_or_else(|| no(format!("Check out a branch to {verb}")))?;
    Ok((branch, upstream))
}

fn can_pull(snap: Option<&RepoSnapshot>, wt: Option<&Worktree>) -> Can {
    match branch_upstream(snap, wt, "pull from") {
        Err(can) => can,
        Ok((branch, Upstream::Tracking { name, .. })) => yes(format!("Pull {name} into {branch}")),
        Ok((branch, Upstream::None)) => no(format!(
            "{branch} has no upstream yet, so there is nothing to pull. Push publishes it."
        )),
        Ok((_, Upstream::Gone { name })) => no(format!("{name} was deleted on the remote")),
    }
}

fn can_push(snap: Option<&RepoSnapshot>, wt: Option<&Worktree>) -> Can {
    let ahead = wt.map_or(0, |wt| wt.ahead());
    match branch_upstream(snap, wt, "push to") {
        Err(can) => can,
        Ok((branch, Upstream::Tracking { name, .. })) if ahead == 0 => no(format!(
            "Nothing to push: {branch} has no commits that {name} lacks"
        )),
        Ok((_, Upstream::Tracking { name, .. })) => {
            let commits = if ahead == 1 { "commit" } else { "commits" };
            yes(format!("Push {ahead} {commits} to {name}"))
        }
        Ok((branch, Upstream::None)) => yes(format!("Publish {branch} and track it")),
        Ok((branch, Upstream::Gone { name })) => yes(format!(
            "Push {branch} again ({name} was deleted on the remote)"
        )),
    }
}

fn can_merge(snap: Option<&RepoSnapshot>, wt: Option<&Worktree>) -> Can {
    let Some(wt) = wt else {
        return no("Select a worktree first");
    };
    let Some(branch) = wt.branch.as_deref() else {
        return no("Check out a branch to merge into");
    };
    if wt.in_progress.is_some() {
        return no(FINISH_FIRST);
    }
    let other = snap.is_some_and(|s| {
        s.branches.iter().any(|b| b.name != branch) || !s.remote_branches.is_empty()
    });
    if other {
        yes(format!("Merge another branch into {branch}"))
    } else {
        no("There is no other branch to merge")
    }
}

fn can_preview(snap: Option<&RepoSnapshot>, wt: Option<&Worktree>) -> (Can, Option<Action>) {
    let Some(snap) = snap else {
        return (no("Select a worktree first"), None);
    };
    match (snap.preview.as_ref(), wt) {
        (Some(p), Some(wt)) if graft_core::same_path(&p.source, &wt.path) => (
            yes("The main folder shows this worktree: show its current state again"),
            Some(Action::UpdatePreview),
        ),
        (Some(_), _) => (
            no("The main folder already shows a preview. Restore it first."),
            None,
        ),
        (None, Some(wt)) if wt.is_main => (
            no("Select another worktree to show its state in the main folder"),
            None,
        ),
        (None, Some(_)) if snap.main_worktree().is_some_and(|m| m.dirty() > 0) => (
            no("The main folder has uncommitted changes. Commit or stash them first."),
            None,
        ),
        (None, Some(wt)) => (
            yes(
                "Check out this worktree's commit plus its uncommitted changes in the main \
                 folder, e.g. to run it from there",
            ),
            Some(Action::PreviewInMain(wt.path.clone())),
        ),
        (None, None) => (no("Select a worktree first"), None),
    }
}
