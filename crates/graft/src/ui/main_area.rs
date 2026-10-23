//! The main area to the right of the sidebar. In step 1 every screen is a
//! header plus a placeholder; later steps fill them in.

use egui::{Frame, Margin, Panel, Rect, Sense, Ui, vec2};
use graft_core::{RepoSnapshot, Worktree};

use super::widgets::{elided, empty_state, paint_galley_left};
use crate::app::{Nav, Tab, WorkspaceView};
use crate::theme::{self, color};

pub fn show(ui: &mut Ui, repo: &RepoSnapshot, nav: &Nav) {
    let worktree = repo.worktrees.get(nav.worktree);
    match nav.tab {
        Tab::Workspace => match nav.view {
            WorkspaceView::FileStatus => {
                header(ui, "File status", worktree);
                empty_state(
                    ui,
                    "File status",
                    "Staged and unstaged files with their diff come in step 3.",
                );
            }
            WorkspaceView::History => {
                header(ui, "History", worktree);
                empty_state(ui, "History", "The commit graph comes in step 2.");
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
            let chip_text =
                painter.layout_no_wrap(wt.label().to_owned(), theme::mono(), color::DIRTY_FG);
            let chip = Rect::from_min_size(
                egui::pos2(x, y - 10.0),
                vec2(chip_text.size().x + 16.0, 20.0),
            );
            painter.rect_filled(chip, 5, color::DIRTY_BG.lerp_to_gamma(color::APP_BG, 0.3));
            paint_galley_left(painter, chip.left() + 8.0, y, chip_text, color::DIRTY_FG);
            x = chip.right() + 10.0;

            let path = wt.path.display().to_string();
            let path = elided(
                ui,
                &path,
                theme::small(),
                color::TEXT_WEAK,
                rect.right() - x,
            );
            paint_galley_left(painter, x, y, path, color::TEXT_WEAK);
        });
}
