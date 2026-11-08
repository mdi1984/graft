//! The Slash dialog: a preview of the branches and worktrees that can go,
//! with checkboxes. Nothing is deleted until "Slash N" is clicked.

use egui::{Align, Layout, RichText, ScrollArea, Ui};
use graft_core::Progress;
use graft_core::slash::{Candidate, Plan, Reason};

use super::widgets::dialog::{self, footer_row};
use super::widgets::{
    ButtonKind, button, elided_path, error_label, hint, inline_spinner, note, progress_bar,
    section_label, small_text, title, warning_label,
};
use crate::project::{Action, SlashView, SlashedBranch};
use crate::theme::{self, color};
use crate::ui::Actions;

const WIDTH: f32 = 560.0;

pub fn show(
    ctx: &egui::Context,
    view: &SlashView,
    slashed: &[SlashedBranch],
    progress: Option<Progress>,
    actions: &mut Actions,
) {
    let shown = dialog::show(ctx, "slash-modal", WIDTH, view.running, |ui| {
        title(ui, "Slash");
        note(
            ui,
            "Clean up branches and worktrees you are done with. Nothing is deleted until you confirm.",
        );
        ui.add_space(4.0);

        match &view.plan {
            None => {
                ui.horizontal(|ui| {
                    inline_spinner(ui);
                    let line = progress
                        .as_ref()
                        .map(|p| p.line.clone())
                        .unwrap_or_else(|| "Fetching and scanning…".to_owned());
                    ui.label(RichText::new(line).color(color::text_weak()));
                });
                let (_, cancel) = footer_row(
                    ui,
                    |_| {},
                    |ui| button(ui, "Cancel", ButtonKind::Secondary, true).clicked(),
                );
                if cancel {
                    actions.push(Action::CloseSlash);
                }
            }
            Some(Err(error)) => {
                error_label(ui, &format!("Scanning failed: {error}"));
                let (_, close) = footer_row(
                    ui,
                    |_| {},
                    |ui| button(ui, "Close", ButtonKind::Secondary, true).clicked(),
                );
                if close {
                    actions.push(Action::CloseSlash);
                }
            }
            Some(Ok(plan)) => plan_ui(ui, view, plan, slashed, progress.as_ref(), actions),
        }
    });
    if shown.cancel {
        actions.push(Action::CloseSlash);
    }
}

fn plan_ui(
    ui: &mut Ui,
    view: &SlashView,
    plan: &Plan,
    slashed: &[SlashedBranch],
    progress: Option<&Progress>,
    actions: &mut Actions,
) {
    if let Some(error) = &plan.fetch_error {
        let first = error.lines().next().unwrap_or_default();
        warning_label(
            ui,
            &format!("Could not fetch, so this uses what was fetched before. {first}"),
        );
    }

    ScrollArea::vertical()
        .max_height(380.0)
        .auto_shrink([false, true])
        .show(ui, |ui| {
            if plan.candidates.is_empty() {
                ui.add_space(4.0);
                ui.label(RichText::new("Nothing to clean up.").color(color::text()));
            }
            // One section per reason, in the plan's order: every candidate
            // shows, since "Slash" acts on every checked one.
            let mut first = 0;
            for rows in plan.candidates.chunk_by(|a, b| a.reason == b.reason) {
                let reason = rows[0].reason;
                let (title, detail) = section(reason);
                ui.add_space(6.0);
                section_label(ui, title);
                if reason == Reason::NeverPushed {
                    small_text(ui, detail, color::dirty_fg());
                } else {
                    hint(ui, detail);
                }
                for (i, candidate) in rows.iter().enumerate() {
                    candidate_row(ui, view, first + i, candidate, actions);
                }
                first += rows.len();
            }

            if !plan.kept.is_empty() {
                ui.add_space(6.0);
                egui::CollapsingHeader::new(
                    RichText::new(format!("Kept ({})", plan.kept.len())).color(color::text_mid()),
                )
                .id_salt("slash-kept")
                .show(ui, |ui| {
                    for (name, why) in &plan.kept {
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(name).font(theme::mono()).color(color::text()));
                            hint(ui, why);
                        });
                    }
                });
            }

            if !slashed.is_empty() {
                egui::CollapsingHeader::new(
                    RichText::new(format!("Recently slashed ({})", slashed.len()))
                        .color(color::text_mid()),
                )
                .id_salt("slash-recent")
                .show(ui, |ui| recent_ui(ui, slashed, view.running, actions));
            }
        });

    if let Some(outcome) = &view.outcome {
        for error in &outcome.errors {
            error_label(ui, error);
        }
    }

    if view.running {
        ui.add_space(8.0);
        // Deleting many worktrees takes a while: say where it is.
        let line = progress.map_or("Starting…", |p| p.line.as_str());
        ui.label(RichText::new(line).color(color::text_mid()));
        progress_bar(ui, progress.and_then(|p| p.fraction), WIDTH);
    }
    let count = view.checked.iter().filter(|on| **on).count();
    let ((), action) = footer_row(
        ui,
        |_| {},
        |ui| {
            let label = match count {
                0 => "Slash".to_owned(),
                n => format!("Slash {n}"),
            };
            if button(ui, &label, ButtonKind::Primary, count > 0 && !view.running).clicked() {
                return Some(Action::RunSlash);
            }
            if view.running {
                // Stops after the current item; what is done stays restorable.
                let stop = button(ui, "Stop", ButtonKind::Secondary, true)
                    .on_hover_text("Stop after the current item");
                stop.clicked().then_some(Action::CancelOperation)
            } else {
                let cancel = button(ui, "Cancel", ButtonKind::Secondary, true);
                cancel.clicked().then_some(Action::CloseSlash)
            }
        },
    );
    actions.extend(action);
}

/// A section's title and what it means.
fn section(reason: Reason) -> (&'static str, &'static str) {
    match reason {
        Reason::Gone => (
            "Upstream gone",
            "Deleted on the remote, usually because the pull request was merged.",
        ),
        Reason::Merged => (
            "Merged",
            "Everything on these branches is already in the default branch.",
        ),
        Reason::MissingWorktree => (
            "Missing worktrees",
            "Worktrees whose folder was deleted. Only git's record of them goes.",
        ),
        Reason::NeverPushed => (
            "Never pushed",
            "Not merged and never pushed: their commits exist only on this computer. \
             They can still be restored from Recently slashed.",
        ),
    }
}

fn candidate_row(
    ui: &mut Ui,
    view: &SlashView,
    index: usize,
    candidate: &Candidate,
    actions: &mut Actions,
) {
    let mut on = view.checked.get(index).copied().unwrap_or(false);
    let name = match (&candidate.branch, &candidate.worktree) {
        (Some(branch), _) => branch.clone(),
        (None, Some(path)) => graft_core::paths::folder_name(path),
        (None, None) => return,
    };
    ui.add_enabled_ui(!view.running, |ui| {
        let text = RichText::new(name).font(theme::mono()).color(color::text());
        if ui.checkbox(&mut on, text).changed() {
            actions.push(Action::ToggleSlashItem(index));
        }
    });
    if let Some(path) = &candidate.worktree {
        let prefix = if candidate.branch.is_some() {
            "Also removes its worktree "
        } else {
            "Was at "
        };
        ui.horizontal(|ui| {
            ui.add_space(24.0);
            ui.spacing_mut().item_spacing.x = 0.0;
            small_text(ui, prefix, color::text_weak());
            let full = graft_core::paths::display(path);
            let galley = elided_path(
                ui,
                &full,
                theme::small(),
                color::text_weak(),
                ui.available_width(),
            );
            ui.label(galley).on_hover_text(full);
        });
    }
}

fn recent_ui(ui: &mut Ui, slashed: &[SlashedBranch], running: bool, actions: &mut Actions) {
    let now = crate::time::now();
    for (i, entry) in slashed.iter().enumerate() {
        ui.horizontal(|ui| {
            ui.label(
                RichText::new(&entry.branch)
                    .font(theme::mono())
                    .color(color::text()),
            );
            let sha = graft_core::short(&entry.target);
            hint(
                ui,
                &format!("{sha} · {}", crate::time::relative(entry.at, now)),
            );
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if button(ui, "Restore", ButtonKind::Secondary, !running).clicked() {
                    actions.push(Action::RestoreSlashed(i));
                }
            });
        });
    }
}
