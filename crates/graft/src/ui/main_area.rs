//! The main area to the right of the sidebar: a header strip plus the
//! screen for the current tab and view.

use egui::{Align2, Frame, Margin, Panel, Rect, Sense, Ui, vec2};
use graft_core::Worktree;

use super::widgets::{ButtonKind, button, elided_path, paint_galley_left, spinner};
use super::{conflicts, file_history, file_status, history, review, stashes};
use crate::project::{Action, Project, Tab, WorkspaceView};
use crate::theme::{self, color};

pub fn show(ui: &mut Ui, project: &mut Project, actions: &mut Vec<Action>) {
    if project.snapshot.is_none() {
        not_loaded(ui, project, actions);
        return;
    }
    let worktree = project.worktree().cloned();
    let worktree = worktree.as_ref();
    preview_banner(ui, project, actions);
    match project.nav.tab {
        Tab::Workspace => match project.nav.view {
            WorkspaceView::FileStatus => {
                header(ui, "File status", worktree);
                if let Some(wt) = worktree {
                    conflicts::banner(ui, project, wt, actions);
                }
                file_status::show(ui, project, actions);
            }
            WorkspaceView::FileHistory => {
                header(ui, "File history", worktree);
                file_history::show(ui, project, actions);
            }
            WorkspaceView::History => {
                header(ui, "History", worktree);
                if let Some(wt) = worktree {
                    conflicts::banner(ui, project, wt, actions);
                }
                history::show(ui, project, actions);
            }
        },
        Tab::Review => {
            header(ui, "Review", worktree);
            review::show(ui, project, actions);
        }
        Tab::Stashes => {
            header(ui, "Stashes", worktree);
            stashes::show(ui, project, actions);
        }
    }
}

/// The strip across the top while main shows another worktree.
fn preview_banner(ui: &mut Ui, project: &Project, actions: &mut Vec<Action>) {
    let Some(snap) = &project.snapshot else {
        return;
    };
    let Some(preview) = &snap.preview else { return };
    let source = snap
        .worktrees
        .iter()
        .find(|wt| graft_core::same_path(&wt.path, &preview.source));
    let name = source.map_or_else(|| preview.source.display().to_string(), |wt| wt.label());
    let back = match &preview.restore {
        graft_core::preview::Restore::Branch(b) => b.clone(),
        graft_core::preview::Restore::Detached(id) => graft_core::short(id),
    };
    let busy = project.is_operating();
    Panel::top("preview-banner")
        .frame(
            Frame::new()
                .fill(color::ahead_bg())
                .inner_margin(Margin::symmetric(14, 8))
                .stroke(egui::Stroke::new(1.0, color::ahead_fg().gamma_multiply(0.4))),
        )
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new("Preview in main")
                        .font(theme::semibold(theme::BODY_SIZE))
                        .color(color::ahead_fg()),
                );
                ui.label(
                    egui::RichText::new(format!(
                        "The main worktree shows {name} at {}, with its uncommitted changes.",
                        graft_core::short(&preview.head)
                    ))
                    .color(color::text_mid()),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if button(ui, "Restore main", ButtonKind::Primary, !busy)
                        .on_hover_text(format!(
                            "Go back to {back}. Changes made in main during the preview are discarded."
                        ))
                        .clicked()
                    {
                        actions.push(Action::RestoreMain);
                    }
                    if button(ui, "Update", ButtonKind::Secondary, !busy && source.is_some())
                        .on_hover_text(format!("Show the current state of {name} again"))
                        .clicked()
                    {
                        actions.push(Action::UpdatePreview);
                    }
                });
            });
        });
}

/// Title, branch chip and worktree path.
fn header(ui: &mut Ui, title: &str, worktree: Option<&Worktree>) {
    Panel::top("main-header")
        .frame(
            Frame::new()
                .fill(color::sidebar_bg().lerp_to_gamma(color::app_bg(), 0.5))
                .inner_margin(Margin::symmetric(14, 0)),
        )
        .show(ui, |ui| {
            let (rect, _) =
                ui.allocate_exact_size(vec2(ui.available_width(), 38.0), Sense::hover());
            let painter = ui.painter();
            let y = rect.center().y;

            let title = painter.layout_no_wrap(
                title.to_owned(),
                theme::semibold(theme::BODY_SIZE),
                color::text(),
            );
            let mut x = rect.left() + title.size().x + 10.0;
            paint_galley_left(painter, rect.left(), y, title, color::text());

            let Some(wt) = worktree else { return };
            let chip_text = painter.layout_no_wrap(wt.label(), theme::mono(), color::dirty_fg());
            let chip = Rect::from_min_size(
                egui::pos2(x, y - 10.0),
                vec2(chip_text.size().x + 16.0, 20.0),
            );
            painter.rect_filled(
                chip,
                5,
                color::dirty_bg().lerp_to_gamma(color::app_bg(), 0.3),
            );
            paint_galley_left(painter, chip.left() + 8.0, y, chip_text, color::dirty_fg());
            x = chip.right() + 10.0;

            let path = wt.path.display().to_string();
            let path = elided_path(
                ui,
                &path,
                theme::small(),
                color::text_weak(),
                rect.right() - x,
            );
            paint_galley_left(painter, x, y, path, color::text_weak());
        });
}

/// The project has no snapshot yet: loading, or it failed (folder gone?).
fn not_loaded(ui: &mut Ui, project: &Project, actions: &mut Vec<Action>) {
    let rect = ui.available_rect_before_wrap();
    let Some(error) = &project.error else {
        spinner(ui, rect.center() - vec2(0.0, 16.0), 9.0, color::text_weak());
        ui.painter().text(
            rect.center() + vec2(0.0, 10.0),
            Align2::CENTER_CENTER,
            format!("Opening {}…", project.root.display()),
            theme::body(),
            color::text_weak(),
        );
        return;
    };
    let width = 460.0_f32.min(rect.width() - 32.0);
    let area = Rect::from_center_size(rect.center(), vec2(width, 160.0));
    ui.scope_builder(egui::UiBuilder::new().max_rect(area), |ui| {
        ui.label(
            egui::RichText::new(format!("Could not open {}", project.name))
                .font(theme::semibold(15.0))
                .color(color::text()),
        );
        ui.label(egui::RichText::new(project.root.display().to_string()).color(color::text_weak()));
        ui.add_space(4.0);
        ui.add(egui::Label::new(egui::RichText::new(error).color(color::removed())).wrap());
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            if button(ui, "Try again", ButtonKind::Primary, true).clicked() {
                actions.push(Action::Refresh);
            }
            if button(ui, "Close tab", ButtonKind::Secondary, true).clicked() {
                actions.push(Action::Close);
            }
        });
    });
}
