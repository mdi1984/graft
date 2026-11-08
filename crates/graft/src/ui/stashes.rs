//! The Stashes tab: every stash of the repository (stashes are shared by all
//! worktrees) with the branch and worktree it came from, its files and diffs,
//! and Apply / Pop / Apply to another worktree / Branch / Drop.

use egui::{Frame, Margin, Panel, RichText, ScrollArea, Ui, Vec2};
use graft_core::diff::Source;
use graft_core::paths::{self, folder_name};
use graft_core::stash::Stash;
use graft_core::{RepoSnapshot, short};

use super::branch_menu::can_item;
use super::can;
use super::diff_view;
use super::file_row::{Extras, FILE_ROW_HEIGHT, file_row};
use super::widgets::{
    ButtonKind, button, empty_state, hint, section_heading_row, two_line_row, waiting,
};
use crate::project::{Action, Confirm, DialogKind, FileKey, Project};
use crate::theme::{self, color};
use crate::time;
use crate::ui::Actions;

pub fn show(ui: &mut Ui, project: &Project, actions: &mut Actions) {
    let Some(stashes) = &project.stashes else {
        waiting(ui);
        return;
    };
    let stashes = match stashes {
        Ok(list) => list,
        Err(e) => {
            empty_state(ui, "Could not list stashes", e);
            return;
        }
    };
    if stashes.is_empty() {
        empty_state(
            ui,
            "No stashes",
            "Stash puts a worktree's changes aside and leaves it clean. Use Stash in the toolbar.",
        );
        return;
    }
    let Some(snap) = &project.snapshot else {
        return;
    };

    Panel::left("stash-list")
        .resizable(true)
        .default_size(360.0)
        .size_range(240.0..=600.0)
        .frame(
            Frame::new()
                .fill(color::app_bg())
                .inner_margin(Margin::symmetric(8, 8)),
        )
        .show(ui, |ui| list(ui, project, snap, stashes, actions));

    let Some(stash) = project
        .selected_stash
        .and_then(|id| stashes.iter().find(|s| s.id == id))
    else {
        empty_state(ui, "No stash selected", "Pick a stash on the left.");
        return;
    };
    // As tall as the header and every file, up to half the height.
    let max_height = ui.available_height() * 0.5 - 2.0 * DETAILS_MARGIN_Y;
    Panel::top("stash-details")
        .frame(
            Frame::new()
                .fill(color::sidebar_bg())
                .inner_margin(Margin::symmetric(16, DETAILS_MARGIN_Y as i8)),
        )
        .show(ui, |ui| {
            details(ui, project, snap, stash, max_height, actions)
        });

    match &project.stash_diff {
        Some(view) => diff_view::show_view(
            ui,
            view,
            &can::act(project, ""),
            project.settings(),
            actions,
        ),
        None => empty_state(ui, "", "Pick a file above to see its changes."),
    }
}

fn list(
    ui: &mut Ui,
    project: &Project,
    snap: &RepoSnapshot,
    stashes: &[Stash],
    actions: &mut Actions,
) {
    let now = time::now();
    section_heading_row(ui, &format!("STASHES  {}", stashes.len()));
    hint(ui, "Shared by all worktrees of this repository.");
    ui.add_space(6.0);
    ScrollArea::vertical()
        .id_salt("stash-list-scroll")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 2.0;
            for stash in stashes {
                let selected = project.selected_stash == Some(stash.id);
                let meta = format!(
                    "{} · {}",
                    origin(snap, stash),
                    time::relative(stash.time, now)
                );
                let response = two_line_row(ui, Vec2::ZERO, selected, &stash.message, &meta);
                if response.clicked() {
                    actions.push(Action::SelectStash(stash.id));
                }
                response.context_menu(|ui| {
                    let can = can::act(project, "");
                    let here = project.nav.worktree.clone();
                    let apply_here = apply(stash, here.clone(), false);
                    can_item(ui, "Apply here", &can, actions, apply_here);
                    can_item(ui, "Pop here", &can, actions, apply(stash, here, true));
                    can_item(ui, "Drop…", &can, actions, drop(stash));
                });
            }
        });
}

/// "from main · in demo-agent" (the worktree that has the branch, if any).
fn origin(snap: &RepoSnapshot, stash: &Stash) -> String {
    match &stash.branch {
        Some(branch) => match snap.worktree_for_branch(branch) {
            Some(wt) => {
                let folder = folder_name(&wt.path);
                format!("from {branch} in {folder}")
            }
            None => format!("from {branch}"),
        },
        None => "from a detached HEAD".to_owned(),
    }
}

const DETAILS_MARGIN_Y: f32 = 12.0;

/// The selected stash: what it is, what to do with it, and its files, in at
/// most `max_height`.
fn details(
    ui: &mut Ui,
    project: &Project,
    snap: &RepoSnapshot,
    stash: &Stash,
    max_height: f32,
    actions: &mut Actions,
) {
    ui.spacing_mut().item_spacing.y = 6.0;
    ui.label(
        RichText::new(&stash.message)
            .font(theme::title())
            .color(color::text()),
    );
    ui.label(
        RichText::new(format!(
            "{} · {} · {} · {}",
            stash.name(),
            short(&stash.id),
            origin(snap, stash),
            time::absolute(stash.time)
        ))
        .color(color::text_weak())
        .size(theme::SMALL_SIZE),
    );

    let here = project.worktree();
    let here_label = here.map(|wt| wt.label()).unwrap_or_default();
    ui.horizontal(|ui| {
        let here_path = project.nav.worktree.clone();
        let can = can::act(project, format!("Apply to {here_label}; keep the stash"));
        if button(ui, "Apply", ButtonKind::Primary, can.ok)
            .on_hover_text(can.hint)
            .clicked()
        {
            actions.push(apply(stash, here_path.clone(), false));
        }
        let can = can::act(project, format!("Apply to {here_label} and drop the stash"));
        if button(ui, "Pop", ButtonKind::Secondary, can.ok)
            .on_hover_text(can.hint)
            .clicked()
        {
            actions.push(apply(stash, here_path.clone(), true));
        }
        let others: Vec<_> = snap
            .worktrees
            .iter()
            .filter(|wt| !wt.prunable && !graft_core::paths::same_path(&wt.path, &here_path))
            .collect();
        let can = if others.is_empty() {
            can::no("There is no other worktree")
        } else {
            can::act(project, "")
        };
        let apply_to = ui.add_enabled_ui(can.ok, |ui| {
            ui.menu_button("Apply to…", |ui| {
                for wt in others {
                    if ui
                        .button(format!("{}  ({})", wt.label(), paths::display(&wt.path)))
                        .clicked()
                    {
                        actions.push(apply(stash, wt.path.clone(), false));
                        ui.close();
                    }
                }
            })
            .response
        });
        can.why_not(apply_to.inner);
        let can = can::moves_head(
            project,
            &project.nav.worktree,
            "New branch with this stash applied",
        );
        if button(ui, "Branch…", ButtonKind::Secondary, can.ok)
            .on_hover_text(can.hint)
            .clicked()
        {
            actions.push(Action::OpenDialog(DialogKind::StashBranch {
                id: stash.id,
                label: stash.name(),
                name: String::new(),
            }));
        }
        let can = can::act(project, "");
        if can
            .why_not(button(ui, "Drop…", ButtonKind::Secondary, can.ok))
            .clicked()
        {
            actions.push(drop(stash));
        }
    });
    ui.label(
        RichText::new(format!(
            "Apply and Pop go to the selected worktree: {here_label}"
        ))
        .color(color::text_faint())
        .size(theme::SMALL_SIZE),
    );
    ui.add_space(4.0);

    let files = match &project.stash_files {
        None => {
            ui.label(RichText::new("Loading files…").color(color::text_weak()));
            return;
        }
        Some(Err(e)) => {
            ui.label(RichText::new(e).color(color::removed()));
            return;
        }
        Some(Ok(files)) => files,
    };
    let open = project.stash_diff.as_ref().map(|v| &v.key);
    let room = (max_height - ui.min_rect().height()).max(FILE_ROW_HEIGHT);
    ScrollArea::vertical()
        .id_salt("stash-files")
        .max_height(room)
        .auto_shrink([false, true])
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 1.0;
            for file in files {
                let key = FileKey {
                    source: Source::Commit(file.commit),
                    path: file.change.path.clone(),
                };
                let extras = Extras {
                    note: file
                        .is_untracked(stash)
                        .then(|| ("untracked", color::text_faint())),
                    ..Extras::default()
                };
                if file_row(ui, &file.change, open == Some(&key), extras).clicked() {
                    actions.push(Action::SelectStashFile(key));
                }
            }
        });
}

fn apply(stash: &Stash, worktree: std::path::PathBuf, pop: bool) -> Action {
    Action::ApplyStash {
        id: stash.id,
        worktree,
        pop,
    }
}

fn drop(stash: &Stash) -> Action {
    Action::Ask(Box::new(Confirm {
        title: format!("Drop {}?", stash.name()),
        body: format!(
            "\"{}\" will be deleted. Its changes are gone unless you applied them somewhere.",
            stash.message
        ),
        confirm_label: "Drop stash".to_owned(),
        action: Action::DropStash(stash.id),
    }))
}
