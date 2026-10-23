//! Left sidebar: repo picker, WORKSPACE, WORKTREES, BRANCHES, then
//! REMOTES / STASHES / TAGS.

use egui::{Rect, Response, ScrollArea, Sense, Stroke, StrokeKind, Ui, pos2, vec2};
use graft_core::{Branch, RepoSnapshot, Upstream, Worktree};

use super::icons::Icon;
use super::widgets::{
    self, Badge, ICON_SIZE, elided, paint_badges_right, paint_galley_left, paint_icon,
    paint_row_bg, section_heading,
};
use crate::app::{Nav, Tab, WorkspaceView};
use crate::theme::{self, RADIUS, color};

const SECTION_GAP: f32 = 14.0;
const PAD_X: f32 = 8.0;

pub fn show(ui: &mut Ui, repo: &RepoSnapshot, nav: &mut Nav) {
    ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 2.0;

            repo_picker(ui, repo).on_hover_text("Switching repositories is not wired up yet");
            ui.add_space(SECTION_GAP);

            heading(ui, "WORKSPACE", false);
            let in_workspace = nav.tab == Tab::Workspace;
            let dirty = repo.worktrees.get(nav.worktree).map_or(0, |wt| wt.dirty);
            let dirty_badge =
                (dirty > 0).then(|| Badge::new(ui, dirty.to_string(), widgets::DIRTY));
            let status_selected = in_workspace && nav.view == WorkspaceView::FileStatus;
            if nav_row(
                ui,
                Icon::FileStatus,
                "File status",
                status_selected,
                dirty_badge,
            )
            .clicked()
            {
                nav.open_file_status();
            }
            let history_selected = in_workspace && nav.view == WorkspaceView::History;
            if nav_row(ui, Icon::History, "History", history_selected, None).clicked() {
                nav.open_history();
            }
            ui.add_space(SECTION_GAP);

            if let Some(add) = heading(ui, "WORKTREES", true) {
                add.on_hover_text("New worktree (not wired up yet)");
            }
            for (index, wt) in repo.worktrees.iter().enumerate() {
                if worktree_row(ui, wt, index == nav.worktree).clicked() {
                    nav.open_worktree(index, wt.dirty > 0);
                }
            }
            ui.add_space(SECTION_GAP);

            heading(ui, "BRANCHES", false);
            for branch in &repo.branches {
                if branch_row(ui, repo, branch).clicked() {
                    nav.open_history();
                }
            }
            ui.add_space(SECTION_GAP);

            footer_row(ui, "REMOTES", &repo.remotes.join(", "));
            if footer_row(ui, "STASHES", &repo.stash_count.to_string()).clicked() {
                nav.tab = Tab::Stashes;
            }
            footer_row(ui, "TAGS", &repo.tag_count.to_string());
        });
}

fn row(ui: &mut Ui, height: f32) -> (Rect, Response) {
    ui.allocate_exact_size(vec2(ui.available_width(), height), Sense::click())
}

fn repo_picker(ui: &mut Ui, repo: &RepoSnapshot) -> Response {
    let (rect, response) = row(ui, 46.0);
    let painter = ui.painter();
    let fill = if response.hovered() {
        color::HOVER_ROW_BG
    } else {
        color::CONTROL_BG
    };
    painter.rect(
        rect,
        8,
        fill,
        Stroke::new(1.0, color::BORDER),
        StrokeKind::Inside,
    );

    let y = rect.center().y;
    let avatar = Rect::from_min_size(pos2(rect.left() + 10.0, y - 14.0), vec2(28.0, 28.0));
    painter.rect_filled(
        avatar,
        RADIUS,
        color::SELECTED_ROW_BG.lerp_to_gamma(color::ACCENT, 0.12),
    );
    let initial: String = repo.name.chars().take(1).collect();
    painter.text(
        avatar.center(),
        egui::Align2::CENTER_CENTER,
        initial,
        theme::semibold(theme::BODY_SIZE),
        color::ACCENT,
    );

    let text_x = avatar.right() + 10.0;
    let max_width = rect.right() - 28.0 - text_x;
    let name = elided(
        ui,
        &repo.name,
        theme::semibold(theme::BODY_SIZE),
        color::TEXT,
        max_width,
    );
    paint_galley_left(painter, text_x, y - 8.0, name, color::TEXT);
    let path = repo.path.display().to_string();
    let path = elided(ui, &path, theme::small(), color::TEXT_WEAK, max_width);
    paint_galley_left(painter, text_x, y + 8.0, path, color::TEXT_WEAK);
    paint_icon(
        painter,
        pos2(rect.right() - 10.0 - ICON_SIZE, y),
        Icon::ChevronDown,
        color::TEXT_WEAK,
    );
    response
}

/// Section heading; with `add_button`, also a "+" at the right whose response is returned.
fn heading(ui: &mut Ui, title: &str, add_button: bool) -> Option<Response> {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 22.0), Sense::hover());
    section_heading(ui, ui.painter(), rect.left() + 6.0, rect.center().y, title);
    if !add_button {
        return None;
    }
    let button =
        Rect::from_center_size(pos2(rect.right() - 12.0, rect.center().y), vec2(22.0, 22.0));
    let response = ui.interact(button, ui.scope_id().with(title), Sense::click());
    if response.hovered() {
        ui.painter().rect_filled(button, 4, color::CONTROL_HOVER_BG);
    }
    let fg = if response.hovered() {
        color::TEXT
    } else {
        color::TEXT_WEAK
    };
    paint_icon(
        ui.painter(),
        pos2(button.center().x - ICON_SIZE / 2.0, button.center().y),
        Icon::Plus,
        fg,
    );
    Some(response)
}

fn nav_row(ui: &mut Ui, icon: Icon, label: &str, selected: bool, badge: Option<Badge>) -> Response {
    let (rect, response) = row(ui, 28.0);
    let painter = ui.painter();
    paint_row_bg(painter, rect, &response, selected);
    let y = rect.center().y;
    paint_icon(painter, pos2(rect.left() + PAD_X, y), icon, color::TEXT);
    let text_x = rect.left() + PAD_X + ICON_SIZE + 8.0;
    let badges_x = paint_badges_right(
        painter,
        rect.right() - PAD_X,
        y,
        badge.into_iter().collect(),
    );
    let text = elided(
        ui,
        label,
        theme::body(),
        color::TEXT,
        badges_x - 6.0 - text_x,
    );
    paint_galley_left(painter, text_x, y, text, color::TEXT);
    response
}

fn worktree_row(ui: &mut Ui, wt: &Worktree, selected: bool) -> Response {
    let (rect, response) = row(ui, 42.0);
    let painter = ui.painter();
    paint_row_bg(painter, rect, &response, selected);

    let top_y = rect.top() + 14.0;
    let bottom_y = rect.top() + 29.0;
    let icon_color = if selected {
        color::ACCENT
    } else {
        color::TEXT_WEAK
    };
    paint_icon(
        painter,
        pos2(rect.left() + PAD_X, top_y),
        Icon::Folder,
        icon_color,
    );

    let mut badges = Vec::new();
    if wt.dirty > 0 {
        badges.push(Badge::new(ui, wt.dirty.to_string(), widgets::DIRTY));
    }
    if wt.ahead > 0 {
        badges.push(Badge::new(ui, format!("↑{}", wt.ahead), widgets::AHEAD));
    }
    let badges_x = paint_badges_right(painter, rect.right() - PAD_X, top_y, badges);

    let text_x = rect.left() + PAD_X + ICON_SIZE + 8.0;
    let branch = elided(
        ui,
        wt.label(),
        theme::mono(),
        color::TEXT,
        badges_x - 6.0 - text_x,
    );
    paint_galley_left(painter, text_x, top_y, branch, color::TEXT);
    let path = wt.path.display().to_string();
    let path = elided(
        ui,
        &path,
        theme::small(),
        color::TEXT_WEAK,
        rect.right() - PAD_X - text_x,
    );
    paint_galley_left(painter, text_x, bottom_y, path, color::TEXT_WEAK);

    response.on_hover_text(wt.path.display().to_string())
}

fn branch_row(ui: &mut Ui, repo: &RepoSnapshot, branch: &Branch) -> Response {
    let (rect, response) = row(ui, 26.0);
    let painter = ui.painter();
    paint_row_bg(painter, rect, &response, false);
    let y = rect.center().y;

    let checked_out = repo
        .worktrees
        .iter()
        .any(|wt| wt.branch.as_deref() == Some(branch.name.as_str()));
    let dot = if branch.is_head {
        color::ACCENT
    } else if branch.upstream == Upstream::Gone {
        color::TEXT_FAINT
    } else if checked_out {
        color::BRANCH_DOT_AGENT
    } else {
        color::AHEAD_FG
    };
    painter.circle_filled(pos2(rect.left() + PAD_X + 3.0, y), 3.0, dot);

    let meta = match branch.upstream {
        Upstream::None => "local".to_owned(),
        Upstream::Gone => "gone".to_owned(),
        Upstream::Tracking { ahead, behind } => match (ahead, behind) {
            (0, 0) => String::new(),
            (a, 0) => format!("↑{a}"),
            (0, b) => format!("↓{b}"),
            (a, b) => format!("↑{a} ↓{b}"),
        },
    };
    let meta = ui
        .painter()
        .layout_no_wrap(meta, theme::small(), color::TEXT_WEAK);
    let meta_x = rect.right() - PAD_X - meta.size().x;
    paint_galley_left(painter, meta_x, y, meta, color::TEXT_WEAK);

    let text_x = rect.left() + PAD_X + 14.0;
    let name = elided(
        ui,
        &branch.name,
        theme::mono(),
        color::TEXT,
        meta_x - 6.0 - text_x,
    );
    paint_galley_left(painter, text_x, y, name, color::TEXT);
    response
}

fn footer_row(ui: &mut Ui, title: &str, value: &str) -> Response {
    let (rect, response) = row(ui, 24.0);
    let painter = ui.painter();
    paint_row_bg(painter, rect, &response, false);
    let y = rect.center().y;
    section_heading(ui, painter, rect.left() + PAD_X, y, title);
    let value = ui
        .painter()
        .layout_no_wrap(value.to_owned(), theme::small(), color::TEXT_WEAK);
    let x = rect.right() - PAD_X - value.size().x;
    paint_galley_left(painter, x, y, value, color::TEXT_WEAK);
    response
}
