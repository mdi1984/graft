//! The always-visible toolbar: Commit, Pull, Push, Fetch | Branch, Merge, Stash,
//! New worktree | Preview in main folder, Slash | search.
//!
//! A button that cannot do anything right now is drawn disabled; its hover
//! text says why.

use egui::{Align, Layout, Ui};
use graft_core::remote::{self, PushPlan};
use graft_core::{RepoSnapshot, Upstream, Worktree};

use super::can::{self, Can, no, yes};
use super::icons::Icon;
use super::widgets::{Tone, fits, remember_width, tool_button, toolbar_divider};
use crate::project::{Action, DialogKind, Project, RemoteOp, merge_dialog};
use crate::text::plural;
use crate::ui::Actions;

pub fn show(
    ui: &mut Ui,
    projects: &[Project],
    index: usize,
    actions: &mut Actions,
    search: &mut String,
) {
    let project = &projects[index];
    let snap = project.snapshot.as_deref();
    let wt = project.worktree();
    let blocked = project.can_act().err();
    let ahead = wt.map_or(0, |wt| wt.ahead());
    let behind = wt.map_or(0, |wt| wt.behind());
    // Buttons with an icon drop their label when the labeled buttons (as
    // wide as last measured) leave the search less than its full width.
    let labeled = egui::Id::unique("toolbar-labeled-width");
    let compact = !fits(ui, labeled, ui.available_width() - super::search::MAX_WIDTH);
    let label = |text: &'static str| if compact { "" } else { text };

    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        let start = ui.cursor().left();

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
        let can = can_pull(snap, wt).unless(blocked);
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
        {
            actions.push(Action::Remote(RemoteOp::Pull));
        }
        let can = can_push(snap, wt).unless(blocked);
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
        {
            actions.push(Action::Remote(RemoteOp::Push));
        }
        let can = if has_remote(snap) {
            yes("Fetch all remotes")
        } else {
            no("No remote to fetch from. Add one under Remotes in the sidebar.")
        }
        .unless(blocked);
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
        {
            actions.push(Action::Remote(RemoteOp::Fetch));
        }
        toolbar_divider(ui);

        let can = match wt {
            Some(wt) if wt.head.is_some() => yes("New branch from the current commit"),
            Some(_) => no("Make a first commit before creating branches"),
            None => no("Select a worktree first"),
        }
        .unless(blocked);
        if tool_button(ui, None, "Branch", None, Tone::Normal, can.ok)
            .on_hover_text(can.hint)
            .clicked()
            && let Some(wt) = wt
        {
            actions.push(Action::OpenDialog(DialogKind::new_branch_at(wt)));
        }
        let can = can::merge(snap, wt).unless(blocked);
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
            && let Some(kind) = merge_dialog(project, None)
        {
            actions.push(Action::OpenDialog(kind));
        }
        let can = if wt.is_some_and(|wt| wt.is_dirty()) {
            yes("Put this worktree's changes aside")
        } else {
            no("Nothing to stash: the worktree is clean")
        }
        .unless(blocked);
        if tool_button(ui, None, "Stash", None, Tone::Normal, can.ok)
            .on_hover_text(can.hint)
            .clicked()
        {
            actions.push(Action::OpenDialog(DialogKind::stash()));
        }
        let can = can::new_worktree(project);
        if tool_button(ui, None, "New worktree", None, Tone::Normal, can.ok)
            .on_hover_text(can.hint)
            .clicked()
        {
            actions.push(Action::NewWorktree(None));
        }
        toolbar_divider(ui);

        let (can, preview_action) = match snap {
            Some(snap) => can::preview(snap, wt),
            None => (no("Select a worktree first"), None),
        };
        let can = can.unless(blocked);
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
            && let Some(action) = preview_action
        {
            actions.push(action);
        }
        let can =
            yes("Clean up finished branches and worktrees (shows a preview first)").unless(blocked);
        if tool_button(
            ui,
            Some(Icon::Slash),
            label("Slash"),
            None,
            Tone::Danger,
            can.ok,
        )
        .on_hover_text(can.hint)
        .clicked()
        {
            actions.push(Action::OpenSlash);
        }
        if !compact {
            remember_width(ui, labeled, ui.cursor().left() - start);
        }

        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            super::search::show(ui, projects, index, search, actions)
        });
    });
}

fn has_remote(snap: Option<&RepoSnapshot>) -> bool {
    snap.is_some_and(|s| !s.remotes.is_empty())
}

fn can_commit(wt: Option<&Worktree>) -> Can {
    match wt {
        Some(wt) if wt.is_dirty() => yes("Commit: show the changes to stage and commit"),
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
        return Err(no(can::FINISH_FIRST));
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
        Ok((branch, upstream)) => match remote::pull_check(branch, upstream) {
            Ok(name) => yes(format!("Pull {name} into {branch}")),
            Err(why) => no(why),
        },
    }
}

fn can_push(snap: Option<&RepoSnapshot>, wt: Option<&Worktree>) -> Can {
    let ahead = wt.map_or(0, |wt| wt.ahead());
    let (branch, upstream) = match branch_upstream(snap, wt, "push to") {
        Err(can) => return can,
        Ok(found) => found,
    };
    let remotes = snap
        .into_iter()
        .flat_map(|s| &s.remotes)
        .map(|r| r.name.as_str());
    match (remote::push_plan(upstream, remotes), upstream) {
        (Err(why), _) => no(why),
        (Ok(PushPlan::Upstream), Upstream::Tracking { name, .. }) if ahead == 0 => no(format!(
            "Nothing to push: {branch} has no commits that {name} lacks"
        )),
        (Ok(PushPlan::Upstream), Upstream::Tracking { name, .. }) => yes(format!(
            "Push {} to {name}",
            plural(ahead, "commit", "commits")
        )),
        (Ok(_), Upstream::Gone { name }) => yes(format!(
            "Push {branch} again ({name} was deleted on the remote)"
        )),
        (Ok(_), _) => yes(format!("Publish {branch} and track it")),
    }
}
