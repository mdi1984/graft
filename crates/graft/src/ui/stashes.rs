//! The Stashes tab: every stash of the repository (stashes are shared by all
//! worktrees) with the branch and worktree it came from, its files and diffs,
//! and Apply / Pop / Apply to another worktree / Branch / Drop.

use egui::{Frame, Margin, Panel, RichText, ScrollArea, Sense, Ui, vec2};
use graft_core::diff::Source;
use graft_core::stash::Stash;
use graft_core::{RepoSnapshot, short};

use super::diff_view;
use super::history::file_row;
use super::widgets::{
    ButtonKind, button, elided, empty_state, paint_galley_left, paint_row_bg, spinner,
};
use crate::project::{Action, Confirm, DialogKind, FileKey, Project};
use crate::theme::{self, color};
use crate::time;

pub fn show(ui: &mut Ui, project: &Project, actions: &mut Vec<Action>) {
    let Some(stashes) = &project.stashes else {
        let rect = ui.available_rect_before_wrap();
        spinner(ui, rect.center(), 9.0, color::text_weak());
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
    // Room for the header and every file, up to half the height.
    let files = match &project.stash_files {
        Some(Ok(files)) => files.len(),
        _ => 1,
    };
    let height = (128.0 + files as f32 * 25.0).min(ui.available_height() * 0.5);
    Panel::top("stash-details")
        .exact_size(height)
        .frame(
            Frame::new()
                .fill(color::sidebar_bg())
                .inner_margin(Margin::symmetric(16, 12)),
        )
        .show(ui, |ui| details(ui, project, snap, stash, actions));

    match &project.commit_diff {
        Some(view) => diff_view::show_view(
            ui,
            view,
            project.is_operating(),
            project.settings.wrap_lines,
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
    actions: &mut Vec<Action>,
) {
    let now = time::now();
    ui.label(
        RichText::new(format!("STASHES  {}", stashes.len()))
            .font(theme::semibold(theme::SMALL_SIZE))
            .color(color::text_faint()),
    );
    ui.label(
        RichText::new("Shared by all worktrees of this repository.")
            .color(color::text_faint())
            .size(theme::SMALL_SIZE),
    );
    ui.add_space(6.0);
    ScrollArea::vertical()
        .id_salt("stash-list-scroll")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 2.0;
            for stash in stashes {
                let selected = project.selected_stash == Some(stash.id);
                let (rect, response) =
                    ui.allocate_exact_size(vec2(ui.available_width(), 46.0), Sense::click());
                let painter = ui.painter();
                paint_row_bg(painter, rect, &response, selected);
                let x = rect.left() + 10.0;
                let width = rect.width() - 20.0;
                let title = elided(ui, &stash.message, theme::body(), color::text(), width);
                paint_galley_left(painter, x, rect.top() + 15.0, title, color::text());
                let meta = format!(
                    "{} · {}",
                    origin(snap, stash),
                    time::relative(stash.time, now)
                );
                let meta = elided(ui, &meta, theme::small(), color::text_weak(), width);
                paint_galley_left(painter, x, rect.top() + 33.0, meta, color::text_weak());
                if response.clicked() {
                    actions.push(Action::SelectStash(stash.id));
                }
                response.context_menu(|ui| {
                    let busy = project.is_operating();
                    let here = project.nav.worktree.clone();
                    if ui
                        .add_enabled(!busy, egui::Button::new("Apply here"))
                        .clicked()
                    {
                        actions.push(apply(stash, here.clone(), false));
                        ui.close();
                    }
                    if ui
                        .add_enabled(!busy, egui::Button::new("Pop here"))
                        .clicked()
                    {
                        actions.push(apply(stash, here, true));
                        ui.close();
                    }
                    if ui.add_enabled(!busy, egui::Button::new("Drop…")).clicked() {
                        actions.push(drop(stash));
                        ui.close();
                    }
                });
            }
        });
}

/// "from main · in demo-agent" (the worktree that has the branch, if any).
fn origin(snap: &RepoSnapshot, stash: &Stash) -> String {
    match &stash.branch {
        Some(branch) => match snap.worktree_for_branch(branch) {
            Some(wt) => {
                let folder = wt
                    .path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                format!("from {branch} in {folder}")
            }
            None => format!("from {branch}"),
        },
        None => "from a detached HEAD".to_owned(),
    }
}

fn details(
    ui: &mut Ui,
    project: &Project,
    snap: &RepoSnapshot,
    stash: &Stash,
    actions: &mut Vec<Action>,
) {
    let busy = project.is_operating();
    ui.spacing_mut().item_spacing.y = 6.0;
    ui.label(
        RichText::new(&stash.message)
            .font(theme::semibold(15.0))
            .color(color::text()),
    );
    ui.label(
        RichText::new(format!(
            "stash@{{{}}} · {} · {} · {}",
            stash.index,
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
        if button(ui, "Apply", ButtonKind::Primary, !busy)
            .on_hover_text(format!("Apply to {here_label}; keep the stash"))
            .clicked()
        {
            actions.push(apply(stash, here_path.clone(), false));
        }
        if button(ui, "Pop", ButtonKind::Secondary, !busy)
            .on_hover_text(format!("Apply to {here_label} and drop the stash"))
            .clicked()
        {
            actions.push(apply(stash, here_path.clone(), true));
        }
        let others: Vec<_> = snap
            .worktrees
            .iter()
            .filter(|wt| !wt.prunable && !graft_core::same_path(&wt.path, &here_path))
            .collect();
        ui.add_enabled_ui(!busy && !others.is_empty(), |ui| {
            ui.menu_button("Apply to…", |ui| {
                for wt in others {
                    if ui
                        .button(format!("{}  ({})", wt.label(), wt.path.display()))
                        .clicked()
                    {
                        actions.push(apply(stash, wt.path.clone(), false));
                        ui.close();
                    }
                }
            });
        });
        if button(ui, "Branch…", ButtonKind::Secondary, !busy)
            .on_hover_text("New branch with this stash applied")
            .clicked()
        {
            actions.push(Action::OpenDialog(DialogKind::StashBranch {
                id: stash.id,
                label: format!("stash@{{{}}}", stash.index),
                name: String::new(),
            }));
        }
        if button(ui, "Drop…", ButtonKind::Secondary, !busy).clicked() {
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
    let open = project.commit_diff.as_ref().map(|v| &v.key);
    ScrollArea::vertical()
        .id_salt("stash-files")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 1.0;
            for file in files {
                let key = FileKey {
                    source: Source::Commit(file.commit),
                    path: file.change.path.clone(),
                };
                let untracked = file.commit != stash.id;
                let note = untracked.then_some("untracked");
                if file_row(ui, &file.change, open == Some(&key), note).clicked() {
                    actions.push(Action::SelectCommitFile(key));
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
        title: format!("Drop stash@{{{}}}?", stash.index),
        body: format!(
            "\"{}\" will be deleted. Its changes are gone unless you applied them somewhere.",
            stash.message
        ),
        confirm_label: "Drop stash".to_owned(),
        action: Action::DropStash(stash.id),
    }))
}
