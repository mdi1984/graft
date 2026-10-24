//! The main area to the right of the sidebar: a header strip plus the
//! screen for the current tab and view.

use egui::{Align2, Frame, Margin, Panel, Rect, Sense, Ui, vec2};
use graft_core::Worktree;

use super::history;
use super::widgets::{ButtonKind, button, elided_path, empty_state, paint_galley_left, spinner};
use crate::project::{Action, Project, Tab, WorkspaceView};
use crate::theme::{self, color};

pub fn show(ui: &mut Ui, project: &Project, actions: &mut Vec<Action>) {
    if project.snapshot.is_none() {
        not_loaded(ui, project, actions);
        return;
    }
    let worktree = project.worktree();
    match project.nav.tab {
        Tab::Workspace => match project.nav.view {
            WorkspaceView::FileStatus => {
                header(ui, "File status", worktree);
                empty_state(
                    ui,
                    "File status",
                    "Staging files, hunks and lines comes in the next step.",
                );
            }
            WorkspaceView::History => {
                header(ui, "History", worktree);
                history::show(ui, project, actions);
            }
        },
        Tab::Review => {
            header(ui, "Review", worktree);
            empty_state(
                ui,
                "Review",
                "Review against the merge-base comes after the core steps.",
            );
        }
        Tab::Stashes => {
            header(ui, "Stashes", None);
            empty_state(
                ui,
                "Stashes",
                "Creating and applying stashes comes after the core steps.",
            );
        }
    }
}

/// Title, branch chip and worktree path.
fn header(ui: &mut Ui, title: &str, worktree: Option<&Worktree>) {
    Panel::top("main-header")
        .frame(
            Frame::new()
                .fill(color::SIDEBAR_BG.lerp_to_gamma(color::APP_BG, 0.5))
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
                color::TEXT,
            );
            let mut x = rect.left() + title.size().x + 10.0;
            paint_galley_left(painter, rect.left(), y, title, color::TEXT);

            let Some(wt) = worktree else { return };
            let chip_text = painter.layout_no_wrap(wt.label(), theme::mono(), color::DIRTY_FG);
            let chip = Rect::from_min_size(
                egui::pos2(x, y - 10.0),
                vec2(chip_text.size().x + 16.0, 20.0),
            );
            painter.rect_filled(chip, 5, color::DIRTY_BG.lerp_to_gamma(color::APP_BG, 0.3));
            paint_galley_left(painter, chip.left() + 8.0, y, chip_text, color::DIRTY_FG);
            x = chip.right() + 10.0;

            let path = wt.path.display().to_string();
            let path = elided_path(
                ui,
                &path,
                theme::small(),
                color::TEXT_WEAK,
                rect.right() - x,
            );
            paint_galley_left(painter, x, y, path, color::TEXT_WEAK);
        });
}

/// The project has no snapshot yet: loading, or it failed (folder gone?).
fn not_loaded(ui: &mut Ui, project: &Project, actions: &mut Vec<Action>) {
    let rect = ui.available_rect_before_wrap();
    let Some(error) = &project.error else {
        spinner(ui, rect.center() - vec2(0.0, 16.0), 9.0, color::TEXT_WEAK);
        ui.painter().text(
            rect.center() + vec2(0.0, 10.0),
            Align2::CENTER_CENTER,
            format!("Opening {}…", project.root.display()),
            theme::body(),
            color::TEXT_WEAK,
        );
        return;
    };
    let width = 460.0_f32.min(rect.width() - 32.0);
    let area = Rect::from_center_size(rect.center(), vec2(width, 160.0));
    ui.scope_builder(egui::UiBuilder::new().max_rect(area), |ui| {
        ui.label(
            egui::RichText::new(format!("Could not open {}", project.name))
                .font(theme::semibold(15.0))
                .color(color::TEXT),
        );
        ui.label(egui::RichText::new(project.root.display().to_string()).color(color::TEXT_WEAK));
        ui.add_space(4.0);
        ui.add(egui::Label::new(egui::RichText::new(error).color(color::REMOVED)).wrap());
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
