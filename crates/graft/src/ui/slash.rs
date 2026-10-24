//! The Slash dialog: a preview of the branches and worktrees that can go,
//! with checkboxes. Nothing is deleted until "Slash N" is clicked.

use egui::{Align, Id, Key, Layout, RichText, ScrollArea, Ui};
use graft_core::repo::Progress;
use graft_core::slash::{Candidate, Plan, Reason};

use super::widgets::{ButtonKind, button, elided_path, progress_bar, spinner};
use crate::project::{Action, SlashView, SlashedBranch};
use crate::theme::{self, color};

const WIDTH: f32 = 560.0;

pub fn show(
    ctx: &egui::Context,
    view: &SlashView,
    slashed: &[SlashedBranch],
    progress: Option<Progress>,
    actions: &mut Vec<Action>,
) {
    let response = super::widgets::modal(ctx, Id::unique("slash-modal")).show(ctx, |ui| {
        ui.set_width(WIDTH);
        ui.spacing_mut().item_spacing.y = 6.0;
        ui.label(
            RichText::new("Slash")
                .font(theme::semibold(15.0))
                .color(color::text()),
        );
        wrapped(
            ui,
            "Clean up branches and worktrees you are done with. Nothing is deleted until you confirm.",
            color::text_weak(),
        );
        ui.add_space(4.0);

        match &view.plan {
            None => {
                ui.horizontal(|ui| {
                    let (rect, _) =
                        ui.allocate_exact_size(egui::vec2(14.0, 14.0), egui::Sense::hover());
                    spinner(ui, rect.center(), 5.0, color::text_weak());
                    let line = progress
                        .as_ref()
                        .map(|p| p.line.clone())
                        .unwrap_or_else(|| "Fetching and scanning…".to_owned());
                    ui.label(RichText::new(line).color(color::text_weak()));
                });
                ui.add_space(8.0);
                footer(ui, |ui| {
                    if button(ui, "Cancel", ButtonKind::Secondary, true).clicked() {
                        actions.push(Action::CloseSlash);
                    }
                });
            }
            Some(Err(error)) => {
                wrapped(ui, &format!("Scanning failed: {error}"), color::removed());
                ui.add_space(8.0);
                footer(ui, |ui| {
                    if button(ui, "Close", ButtonKind::Secondary, true).clicked() {
                        actions.push(Action::CloseSlash);
                    }
                });
            }
            Some(Ok(plan)) => plan_ui(ui, view, plan, slashed, progress.as_ref(), actions),
        }
    });
    let running = view.running;
    if !running && (response.should_close() || ctx.input(|i| i.key_pressed(Key::Escape))) {
        actions.push(Action::CloseSlash);
    }
}

fn plan_ui(
    ui: &mut Ui,
    view: &SlashView,
    plan: &Plan,
    slashed: &[SlashedBranch],
    progress: Option<&Progress>,
    actions: &mut Vec<Action>,
) {
    if let Some(error) = &plan.fetch_error {
        let first = error.lines().next().unwrap_or_default();
        wrapped(
            ui,
            &format!("Could not fetch, so this uses what was fetched before. {first}"),
            color::dirty_fg(),
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
            let sections = [
                (
                    Reason::Gone,
                    "Upstream gone",
                    "Deleted on the remote, usually because the pull request was merged.",
                ),
                (
                    Reason::Merged,
                    "Merged",
                    "Everything on these branches is already in the default branch.",
                ),
                (
                    Reason::MissingWorktree,
                    "Missing worktrees",
                    "Worktrees whose folder was deleted. Only git's record of them goes.",
                ),
                (
                    Reason::NeverPushed,
                    "Never pushed",
                    "Not merged and never pushed: their commits exist only on this computer. \
                     They can still be restored from Recently slashed.",
                ),
            ];
            for (reason, title, detail) in sections {
                let rows: Vec<(usize, &Candidate)> = plan
                    .candidates
                    .iter()
                    .enumerate()
                    .filter(|(_, c)| c.reason == reason)
                    .collect();
                if rows.is_empty() {
                    continue;
                }
                ui.add_space(6.0);
                ui.label(
                    RichText::new(title.to_uppercase())
                        .color(color::text_faint())
                        .size(theme::SMALL_SIZE),
                );
                let detail_color = if reason == Reason::NeverPushed {
                    color::dirty_fg()
                } else {
                    color::text_faint()
                };
                small(ui, detail, detail_color);
                for (i, candidate) in rows {
                    candidate_row(ui, view, i, candidate, actions);
                }
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
                            small(ui, why, color::text_faint());
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
            wrapped(ui, error, color::removed());
        }
    }

    ui.add_space(8.0);
    if view.running {
        // Deleting many worktrees takes a while: say where it is.
        let line = progress.map_or("Starting…", |p| p.line.as_str());
        ui.label(RichText::new(line).color(color::text_mid()));
        progress_bar(ui, progress.and_then(|p| p.fraction), WIDTH);
        ui.add_space(8.0);
    }
    let count = view.checked.iter().filter(|on| **on).count();
    footer(ui, |ui| {
        let label = match count {
            0 => "Slash".to_owned(),
            n => format!("Slash {n}"),
        };
        if button(ui, &label, ButtonKind::Primary, count > 0 && !view.running).clicked() {
            actions.push(Action::RunSlash);
        }
        if view.running {
            // Stops after the current item; what is done stays restorable.
            if button(ui, "Stop", ButtonKind::Secondary, true)
                .on_hover_text("Stop after the current item")
                .clicked()
            {
                actions.push(Action::CancelOperation);
            }
        } else if button(ui, "Cancel", ButtonKind::Secondary, true).clicked() {
            actions.push(Action::CloseSlash);
        }
    });
}

fn candidate_row(
    ui: &mut Ui,
    view: &SlashView,
    index: usize,
    candidate: &Candidate,
    actions: &mut Vec<Action>,
) {
    let mut on = view.checked.get(index).copied().unwrap_or(false);
    let name = match (&candidate.branch, &candidate.worktree) {
        (Some(branch), _) => branch.clone(),
        (None, Some(path)) => path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.display().to_string()),
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
            small(ui, prefix, color::text_weak());
            let full = path.display().to_string();
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

fn recent_ui(ui: &mut Ui, slashed: &[SlashedBranch], running: bool, actions: &mut Vec<Action>) {
    let now = jiff::Timestamp::now().as_second();
    for (i, entry) in slashed.iter().enumerate() {
        ui.horizontal(|ui| {
            ui.label(
                RichText::new(&entry.branch)
                    .font(theme::mono())
                    .color(color::text()),
            );
            let sha: String = entry.target.chars().take(8).collect();
            small(
                ui,
                &format!("{sha} · {}", crate::time::relative(entry.at, now)),
                color::text_faint(),
            );
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if button(ui, "Restore", ButtonKind::Secondary, !running).clicked() {
                    actions.push(Action::RestoreSlashed(i));
                }
            });
        });
    }
}

fn footer(ui: &mut Ui, add: impl FnOnce(&mut Ui)) {
    ui.horizontal(|ui| {
        ui.with_layout(Layout::right_to_left(Align::Center), add);
    });
}

fn wrapped(ui: &mut Ui, text: &str, color: egui::Color32) {
    ui.add(egui::Label::new(RichText::new(text).color(color)).wrap());
}

fn small(ui: &mut Ui, text: &str, color: egui::Color32) {
    ui.add(egui::Label::new(RichText::new(text).color(color).size(theme::SMALL_SIZE)).wrap());
}
