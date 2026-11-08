//! The main area to the right of the sidebar: a header strip plus the
//! screen for the current tab and view.

use egui::{Frame, Margin, Panel, Rect, Sense, Ui, vec2};
use graft_core::{Worktree, paths};

use super::widgets::{
    ButtonKind, button, elided_path, error_label, loading, paint_galley_left, title,
};
use super::{can, conflicts, file_history, file_status, history, review, stashes};
use crate::project::{Action, Project, Tab, WorkspaceView};
use crate::theme::{self, color};
use crate::ui::{Actions, AppAction};

pub fn show(ui: &mut Ui, project: &mut Project, actions: &mut Actions) {
    let Some(snap) = project.snapshot.clone() else {
        not_loaded(ui, project, actions);
        return;
    };
    let worktree = project.worktree_in(&snap);
    preview_banner(ui, project, actions);
    match project.nav.tab {
        Tab::Workspace => match project.nav.view {
            view @ WorkspaceView::FileStatus => {
                header(ui, view.label(), worktree);
                if let Some(wt) = worktree {
                    conflicts::banner(ui, project, wt, actions);
                }
                file_status::show(ui, project, actions);
            }
            view @ WorkspaceView::FileHistory => {
                header(ui, view.label(), worktree);
                file_history::show(ui, project, actions);
            }
            view @ WorkspaceView::History => {
                header(ui, view.label(), worktree);
                if let Some(wt) = worktree {
                    conflicts::banner(ui, project, wt, actions);
                }
                history::show(ui, project, actions);
            }
        },
        Tab::Review => {
            header(ui, Tab::Review.label(), worktree);
            review::show(ui, project, actions);
        }
        Tab::Stashes => {
            header(ui, Tab::Stashes.label(), worktree);
            stashes::show(ui, project, actions);
        }
    }
}

/// The strip across the top while main shows another worktree.
fn preview_banner(ui: &mut Ui, project: &Project, actions: &mut Actions) {
    let Some(snap) = &project.snapshot else {
        return;
    };
    let Some(preview) = &snap.preview else { return };
    let source = snap.worktree_at(&preview.source);
    let name = source.map_or_else(|| paths::display(&preview.source), |wt| wt.label());
    let back = preview.restore.label();
    let blocked = project.can_act().err();
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
                    egui::RichText::new("Preview in main folder")
                        .font(theme::semibold(theme::BODY_SIZE))
                        .color(color::ahead_fg()),
                );
                ui.label(
                    egui::RichText::new(format!(
                        "The main folder shows {name} at {}, with its uncommitted changes.",
                        graft_core::short(&preview.head)
                    ))
                    .color(color::text_mid()),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let can = can::yes(format!(
                        "Go back to {back}. Changes made in the main folder during the preview are discarded."
                    ))
                    .unless(blocked);
                    if button(ui, "Restore main folder", ButtonKind::Primary, can.ok)
                        .on_hover_text(can.hint)
                        .clicked()
                    {
                        actions.push(Action::RestoreMain);
                    }
                    let (can, update) = match source {
                        Some(wt) => can::preview(snap, Some(wt)),
                        None => (can::no("The previewed worktree is gone"), None),
                    };
                    let can = can.unless(blocked);
                    if button(ui, "Update", ButtonKind::Secondary, can.ok)
                        .on_hover_text(can.hint)
                        .clicked()
                        && let Some(update) = update
                    {
                        actions.push(update);
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
                .fill(color::muted_bg())
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
            paint_galley_left(painter, rect.left(), y, title);

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
            paint_galley_left(painter, chip.left() + 8.0, y, chip_text);
            x = chip.right() + 10.0;

            let path = paths::display(&wt.path);
            let path = elided_path(
                ui,
                &path,
                theme::small(),
                color::text_weak(),
                rect.right() - x,
            );
            paint_galley_left(painter, x, y, path);
        });
}

/// The project has no snapshot yet: loading, or it failed (folder gone?).
fn not_loaded(ui: &mut Ui, project: &Project, actions: &mut Actions) {
    let Some(error) = &project.error else {
        let path = paths::display(&project.root);
        loading(ui, &format!("Opening {path}…"));
        return;
    };
    let rect = ui.available_rect_before_wrap();
    let width = 460.0_f32.min(rect.width() - 32.0);
    let area = Rect::from_center_size(rect.center(), vec2(width, 160.0));
    ui.scope_builder(egui::UiBuilder::new().max_rect(area), |ui| {
        title(ui, &format!("Could not open {}", project.name));
        ui.label(egui::RichText::new(paths::display(&project.root)).color(color::text_weak()));
        ui.add_space(4.0);
        error_label(ui, error);
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            if button(ui, "Try again", ButtonKind::Primary, true).clicked() {
                actions.push(Action::Refresh);
            }
            if button(ui, "Close tab", ButtonKind::Secondary, true).clicked() {
                actions.app(AppAction::Close);
            }
        });
    });
}
