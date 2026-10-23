//! History: the commit graph (virtualized, colored lanes, ref chips, an
//! "Uncommitted changes" row on top) and the commit details pane.

use egui::epaint::CubicBezierShape;
use egui::{
    Align2, Color32, Frame, Id, Key, Margin, Panel, Pos2, Rect, Response, RichText, ScrollArea,
    Sense, Stroke, Ui, pos2, vec2,
};
use graft_core::details::{CommitDetails, FileChange};
use graft_core::diff::Source;
use graft_core::history::{Commit, GraphRow, History, LineKind};
use graft_core::refs::ResetMode;
use graft_core::short;
use graft_core::status::Change;

use super::diff_view;
use super::icons::Icon;
use super::widgets::{
    ButtonKind, button, chip, elided, empty_state, icon_button, paint_galley_left, spinner,
};
use crate::project::{Action, DialogKind, FileKey, Project, RefKind, RefLabel};
use crate::theme::{self, color};
use crate::time;

const ROW_HEIGHT: f32 = 26.0;
const LANE_WIDTH: f32 = 14.0;
const MAX_GRAPH_WIDTH: f32 = 280.0;
const COMMIT_COL: f32 = 76.0;
const DATE_COL: f32 = 116.0;
const AUTHOR_COL: f32 = 150.0;

pub fn show(ui: &mut Ui, project: &Project, actions: &mut Vec<Action>) {
    Panel::right("commit-details")
        .resizable(true)
        .default_size(400.0)
        .size_range(300.0..=760.0)
        .frame(
            Frame::new()
                .fill(color::SIDEBAR_BG)
                .inner_margin(Margin::symmetric(16, 14)),
        )
        .show(ui, |ui| details_pane(ui, project, actions));

    let Some(history) = &project.history else {
        if project.history_loading || project.loading {
            loading(ui, "Loading history…");
        } else {
            empty_state(ui, "No history", "This repository has no commits yet.");
        }
        return;
    };
    if history.commits.is_empty() {
        empty_state(
            ui,
            "No commits yet",
            "Make the first commit from File status.",
        );
        return;
    }
    if let Some(view) = &project.commit_diff {
        let height = ui.available_height();
        Panel::bottom("history-diff")
            .exact_size((height * 0.55).max(160.0))
            .frame(Frame::new().fill(color::APP_BG))
            .show(ui, |ui| {
                diff_view::show_view(ui, view, project.is_operating(), actions)
            });
    }
    commit_list(ui, project, history, actions);
}

fn loading(ui: &mut Ui, text: &str) {
    let rect = ui.available_rect_before_wrap();
    spinner(ui, rect.center() - vec2(0.0, 16.0), 9.0, color::TEXT_WEAK);
    ui.painter().text(
        rect.center() + vec2(0.0, 10.0),
        Align2::CENTER_CENTER,
        text,
        theme::body(),
        color::TEXT_WEAK,
    );
}

struct Columns {
    graph: f32,
    author: f32,
}

fn commit_list(ui: &mut Ui, project: &Project, history: &History, actions: &mut Vec<Action>) {
    let width = ui.available_width();
    let cols = Columns {
        graph: (history.lanes.max(1) as f32 * LANE_WIDTH + 10.0).min(MAX_GRAPH_WIDTH),
        author: if width > 700.0 { AUTHOR_COL } else { 0.0 },
    };
    header_row(ui, &cols);

    let selected = project
        .selected
        .and_then(|id| history.index.get(&id).copied());
    keyboard_nav(ui, history, selected, project, actions);

    let list_id = Id::unique(("history-list", &project.root));
    let mut scroll = ScrollArea::vertical()
        .id_salt(list_id)
        .auto_shrink([false, false]);
    if project.scroll_to_selected.take()
        && let Some(i) = selected
    {
        let (offset, height) = ui
            .data(|d| d.get_temp::<(f32, f32)>(list_id))
            .unwrap_or((0.0, ui.available_height()));
        if let Some(target) = scroll_target(i, offset, height) {
            scroll = scroll.vertical_scroll_offset(target);
        }
    }

    let now = time::now();
    let worktree_dirty = project.worktree().map_or(0, |wt| wt.dirty());
    ui.spacing_mut().item_spacing.y = 0.0;
    let output = scroll.show_rows(ui, ROW_HEIGHT, history.commits.len(), |ui, range| {
        for i in range {
            let commit = &history.commits[i];
            let (rect, response) =
                ui.allocate_exact_size(vec2(ui.available_width(), ROW_HEIGHT), Sense::click());
            let is_selected = selected == Some(i);
            let labels = project
                .labels
                .get(&commit.id)
                .map(Vec::as_slice)
                .unwrap_or(&[]);
            paint_row(
                ui,
                rect,
                &response,
                is_selected,
                commit,
                &history.rows[i],
                labels,
                &cols,
                now,
                worktree_dirty,
            );
            if response.clicked() {
                actions.push(Action::Select(commit.id));
            }
            if response.double_clicked() && commit.is_worktree() {
                actions.push(Action::ShowFileStatus);
            }
            if !commit.is_worktree() {
                response.context_menu(|ui| commit_menu(ui, project, commit, actions));
            }
        }
    });
    ui.data_mut(|d| {
        d.insert_temp(
            list_id,
            (output.state.clamped_offset().y, output.inner_rect.height()),
        )
    });
}

/// New scroll offset that brings row `i` into view, or `None` if it is visible.
fn scroll_target(i: usize, offset: f32, height: f32) -> Option<f32> {
    let top = i as f32 * ROW_HEIGHT;
    let bottom = top + ROW_HEIGHT;
    if top >= offset && bottom <= offset + height {
        return None;
    }
    // Just off screen: scroll the minimum; far away: center it.
    if top < offset && offset - top < height / 2.0 {
        Some(top)
    } else if bottom > offset + height && bottom - (offset + height) < height / 2.0 {
        Some(bottom - height)
    } else {
        Some((top - height / 2.0 + ROW_HEIGHT / 2.0).max(0.0))
    }
}

fn keyboard_nav(
    ui: &Ui,
    history: &History,
    selected: Option<usize>,
    project: &Project,
    actions: &mut Vec<Action>,
) {
    if ui.memory(|m| m.focused().is_some()) {
        return;
    }
    let (up, down, page_up, page_down, home, end) = ui.input(|i| {
        (
            i.key_pressed(Key::ArrowUp),
            i.key_pressed(Key::ArrowDown),
            i.key_pressed(Key::PageUp),
            i.key_pressed(Key::PageDown),
            i.key_pressed(Key::Home),
            i.key_pressed(Key::End),
        )
    });
    let last = history.commits.len().saturating_sub(1);
    let current = selected.unwrap_or(0);
    let page = 20;
    let next = if up {
        current.saturating_sub(1)
    } else if down {
        (current + 1).min(last)
    } else if page_up {
        current.saturating_sub(page)
    } else if page_down {
        (current + page).min(last)
    } else if home {
        0
    } else if end {
        last
    } else {
        return;
    };
    if Some(next) != selected {
        actions.push(Action::Select(history.commits[next].id));
        project.scroll_to_selected.set(true);
    }
}

fn header_row(ui: &mut Ui, cols: &Columns) {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 28.0), Sense::hover());
    let painter = ui.painter();
    painter.rect_filled(rect, 0, color::APP_BG);
    painter.hline(
        rect.x_range(),
        rect.bottom() - 0.5,
        Stroke::new(1.0, color::BORDER),
    );
    let y = rect.center().y;
    let label = |x: f32, text: &str| {
        painter.text(
            pos2(x, y),
            Align2::LEFT_CENTER,
            text,
            theme::small(),
            color::TEXT_FAINT,
        );
    };
    label(rect.left() + 10.0, "Graph");
    label(rect.left() + cols.graph + 6.0, "Description");
    let mut x = rect.right() - COMMIT_COL;
    label(x, "Commit");
    x -= DATE_COL;
    label(x, "Date");
    if cols.author > 0.0 {
        label(x - cols.author, "Author");
    }
}

#[allow(clippy::too_many_arguments)]
fn paint_row(
    ui: &Ui,
    rect: Rect,
    response: &egui::Response,
    selected: bool,
    commit: &Commit,
    row: &GraphRow,
    labels: &[RefLabel],
    cols: &Columns,
    now: i64,
    worktree_dirty: usize,
) {
    let painter = ui.painter();
    let bg = if selected {
        color::SELECTED_ROW_BG
    } else if response.hovered() {
        color::HOVER_ROW_BG
    } else {
        color::APP_BG
    };
    if bg != color::APP_BG {
        painter.rect_filled(rect, 0, bg);
    }

    let graph_rect = Rect::from_min_size(rect.min, vec2(cols.graph, rect.height()));
    let is_head = labels.iter().any(|l| l.kind == RefKind::Head);
    paint_graph(ui, graph_rect, row, commit, is_head, bg);

    let y = rect.center().y;
    let mut right = rect.right() - COMMIT_COL;
    if !commit.is_worktree() {
        let sha = painter.layout_no_wrap(short(&commit.id), theme::mono(), color::TEXT_FAINT);
        paint_galley_left(painter, right, y, sha, color::TEXT_FAINT);
    }
    right -= DATE_COL;
    if !commit.is_worktree() {
        let date = time::relative(commit.time, now);
        let date = elided(ui, &date, theme::body(), color::TEXT_WEAK, DATE_COL - 8.0);
        paint_galley_left(painter, right, y, date, color::TEXT_WEAK);
    }
    if cols.author > 0.0 {
        right -= cols.author;
        if !commit.is_worktree() {
            let author = elided(
                ui,
                &commit.author,
                theme::body(),
                color::TEXT_MID,
                cols.author - 10.0,
            );
            paint_galley_left(painter, right, y, author, color::TEXT_MID);
        }
    }

    let mut x = rect.left() + cols.graph + 6.0;
    let desc_right = right - 10.0;
    let chips_max = ((desc_right - x) * 0.5).max(60.0);
    let chips_end = x + chips_max;
    for (n, label) in labels.iter().enumerate() {
        let room = chips_end - x;
        if room < 40.0 {
            let more = painter.layout_no_wrap(
                format!("+{}", labels.len() - n),
                theme::small(),
                color::TEXT_WEAK,
            );
            x += chip(painter, x, y, more, color::TEXT_WEAK, 40.0) + 4.0;
            break;
        }
        let (fg, font) = label_style(label);
        let galley = painter.layout_no_wrap(label.name.clone(), font, fg);
        x += chip(painter, x, y, galley, fg, room) + 4.0;
    }
    if !labels.is_empty() {
        x += 2.0;
    }

    if commit.is_worktree() {
        let text = format!(
            "Uncommitted changes · {worktree_dirty} file{}",
            if worktree_dirty == 1 { "" } else { "s" }
        );
        let galley = elided(
            ui,
            &text,
            theme::semibold(theme::BODY_SIZE),
            color::DIRTY_FG,
            desc_right - x,
        );
        paint_galley_left(painter, x, y, galley, color::DIRTY_FG);
    } else {
        let fg = if commit.parents.len() > 1 {
            color::TEXT_WEAK
        } else {
            color::TEXT
        };
        let summary = elided(ui, &commit.summary, theme::body(), fg, desc_right - x);
        paint_galley_left(painter, x, y, summary, fg);
    }
}

pub fn label_style(label: &RefLabel) -> (Color32, egui::FontId) {
    match label.kind {
        RefKind::Head => (color::ACCENT, theme::semibold(theme::SMALL_SIZE + 0.5)),
        RefKind::Local if label.in_worktree => (color::BRANCH_DOT_AGENT, theme::small()),
        RefKind::Local => (color::AHEAD_FG, theme::small()),
        RefKind::Remote => (color::TEXT_WEAK, theme::small()),
        RefKind::Tag => (color::TAG, theme::small()),
    }
}

fn lane_color(index: u8) -> Color32 {
    color::LANES[index as usize % color::LANES.len()]
}

fn paint_graph(ui: &Ui, rect: Rect, row: &GraphRow, commit: &Commit, is_head: bool, bg: Color32) {
    let painter = ui.painter().with_clip_rect(rect.intersect(ui.clip_rect()));
    let lane_x = |lane: u16| rect.left() + 6.0 + lane as f32 * LANE_WIDTH + LANE_WIDTH / 2.0;
    let (top, bottom, cy) = (rect.top(), rect.bottom(), rect.center().y);
    let node = pos2(lane_x(row.lane), cy);
    let width = 1.6;

    for line in &row.lines {
        let color = if commit.is_worktree() {
            color::TEXT_FAINT
        } else {
            lane_color(line.color)
        };
        let stroke = Stroke::new(width, color);
        let x = lane_x(line.lane);
        match line.kind {
            LineKind::Pass => {
                painter.line_segment([pos2(x, top), pos2(x, bottom)], stroke);
            }
            LineKind::In => {
                curve(&painter, pos2(x, top), node, true, stroke);
            }
            LineKind::Out => {
                curve(&painter, node, pos2(x, bottom), false, stroke);
            }
        }
    }

    let color = lane_color(row.color);
    if commit.is_worktree() {
        painter.circle(node, 4.5, bg, Stroke::new(1.6, color::DIRTY_FG));
    } else if is_head {
        painter.circle(node, 6.0, bg, Stroke::new(2.0, color));
        painter.circle_filled(node, 3.0, color);
    } else if commit.parents.len() > 1 {
        painter.circle(node, 3.5, color, Stroke::new(1.5, bg));
    } else {
        painter.circle(node, 4.5, color, Stroke::new(1.5, bg));
    }
}

/// A line between two points that bends like an elbow when the lanes differ.
/// `vertical_first`: go down first, then sideways (for lines entering a node).
fn curve(painter: &egui::Painter, from: Pos2, to: Pos2, vertical_first: bool, stroke: Stroke) {
    if (from.x - to.x).abs() < 0.5 {
        painter.line_segment([from, to], stroke);
        return;
    }
    let corner = if vertical_first {
        pos2(from.x, to.y)
    } else {
        pos2(to.x, from.y)
    };
    painter.add(CubicBezierShape::from_points_stroke(
        [from, corner, corner, to],
        false,
        Color32::TRANSPARENT,
        stroke,
    ));
}

fn commit_menu(ui: &mut Ui, project: &Project, commit: &Commit, actions: &mut Vec<Action>) {
    let busy = project.is_operating();
    let wt = project.worktree();
    let branch = wt.and_then(|wt| wt.branch.clone());
    let is_head = wt.and_then(|wt| wt.head) == Some(commit.id);
    let short_id = short(&commit.id);
    let mut item = |ui: &mut Ui, label: &str, enabled: bool, action: Action| {
        if ui.add_enabled(enabled, egui::Button::new(label)).clicked() {
            actions.push(action);
            ui.close();
        }
    };

    item(
        ui,
        "New branch here…",
        !busy,
        Action::OpenDialog(DialogKind::NewBranch {
            name: String::new(),
            start: commit.id.to_string(),
            start_label: format!("{short_id} {}", commit.summary),
            checkout: true,
        }),
    );
    item(
        ui,
        "New tag here…",
        !busy,
        Action::OpenDialog(DialogKind::NewTag {
            name: String::new(),
            message: String::new(),
            target: commit.id.to_string(),
            target_label: format!("{short_id} {}", commit.summary),
        }),
    );
    item(
        ui,
        "Check out this commit (detached)",
        !busy && !is_head,
        Action::CheckoutDetached(commit.id),
    );
    ui.separator();
    let onto = branch.as_deref().unwrap_or("HEAD");
    item(
        ui,
        &format!("Cherry-pick onto {onto}"),
        !busy && !is_head,
        Action::CherryPick(commit.id),
    );
    item(ui, "Revert this commit", !busy, Action::Revert(commit.id));
    if let Some(branch) = &branch {
        item(
            ui,
            &format!("Reset {branch} to here…"),
            !busy && !is_head,
            Action::OpenDialog(DialogKind::Reset {
                target: commit.id.to_string(),
                target_label: short_id.clone(),
                branch: branch.clone(),
                mode: ResetMode::Mixed,
            }),
        );
    }
    if is_head && commit.parents.len() == 1 {
        item(
            ui,
            "Undo this commit (keep changes)",
            !busy,
            Action::UndoCommit,
        );
    }
    ui.separator();
    if ui.button("Copy commit hash").clicked() {
        ui.ctx().copy_text(commit.id.to_string());
        ui.close();
    }
    if ui.button("Copy short hash").clicked() {
        ui.ctx().copy_text(short(&commit.id));
        ui.close();
    }
    if ui.button("Copy summary").clicked() {
        ui.ctx().copy_text(commit.summary.clone());
        ui.close();
    }
}

// ---------------------------------------------------------------------------
// Details pane

fn details_pane(ui: &mut Ui, project: &Project, actions: &mut Vec<Action>) {
    let Some(id) = project.selected else {
        empty_state(ui, "No commit selected", "Pick a commit on the left.");
        return;
    };
    if id.is_null() {
        uncommitted(ui, project, actions);
        return;
    }
    match (&project.details, &project.details_error) {
        (Some(d), _) if d.id == id => {
            ScrollArea::vertical()
                .id_salt("commit-details-scroll")
                .auto_shrink([false, false])
                .show(ui, |ui| commit_details(ui, d, project, actions));
        }
        (_, Some(error)) => {
            ui.label(RichText::new(error).color(color::REMOVED));
        }
        _ => loading(ui, "Loading commit…"),
    }
}

fn commit_details(ui: &mut Ui, d: &CommitDetails, project: &Project, actions: &mut Vec<Action>) {
    ui.spacing_mut().item_spacing.y = 6.0;
    ui.add(
        egui::Label::new(
            RichText::new(d.summary())
                .font(theme::semibold(15.0))
                .color(color::TEXT),
        )
        .wrap(),
    );
    let body = d.body();
    if !body.is_empty() {
        ui.add(egui::Label::new(RichText::new(body).color(color::TEXT_MID)).wrap());
    }
    if let Some(labels) = project.labels.get(&d.id) {
        ui.add_space(2.0);
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = vec2(4.0, 4.0);
            for label in labels {
                let (fg, font) = label_style(label);
                let galley = ui.painter().layout_no_wrap(label.name.clone(), font, fg);
                let width = galley.size().x + 10.0;
                let (rect, _) = ui.allocate_exact_size(vec2(width, 18.0), Sense::hover());
                chip(
                    ui.painter(),
                    rect.left(),
                    rect.center().y,
                    galley,
                    fg,
                    width,
                );
            }
        });
    }
    ui.add_space(8.0);

    egui::Grid::new("commit-meta")
        .num_columns(2)
        .spacing(vec2(14.0, 6.0))
        .show(ui, |ui| {
            meta_key(ui, "Commit");
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(d.id.to_string())
                        .font(theme::mono())
                        .color(color::TEXT_MID),
                );
                if icon_button(ui, Icon::Copy, 20.0)
                    .on_hover_text("Copy hash")
                    .clicked()
                {
                    ui.ctx().copy_text(d.id.to_string());
                }
            });
            ui.end_row();

            if !d.parents.is_empty() {
                meta_key(
                    ui,
                    if d.parents.len() > 1 {
                        "Parents"
                    } else {
                        "Parent"
                    },
                );
                ui.horizontal(|ui| {
                    for parent in &d.parents {
                        let link = ui.add(
                            egui::Label::new(
                                RichText::new(short(parent))
                                    .font(theme::mono())
                                    .color(color::AHEAD_FG),
                            )
                            .sense(Sense::click()),
                        );
                        if link
                            .on_hover_cursor(egui::CursorIcon::PointingHand)
                            .clicked()
                        {
                            actions.push(Action::Reveal(*parent));
                        }
                    }
                });
                ui.end_row();
            }

            meta_key(ui, "Author");
            person(ui, &d.author, &d.author_email);
            ui.end_row();

            meta_key(ui, "Date");
            ui.label(
                RichText::new(format!(
                    "{}  ·  {}",
                    time::absolute(d.author_time),
                    time::relative(d.author_time, time::now())
                ))
                .color(color::TEXT_MID),
            );
            ui.end_row();

            if d.committer != d.author || (d.commit_time - d.author_time).abs() > 60 {
                meta_key(ui, "Committed");
                ui.vertical(|ui| {
                    person(ui, &d.committer, &d.committer_email);
                    ui.label(RichText::new(time::absolute(d.commit_time)).color(color::TEXT_WEAK));
                });
                ui.end_row();
            }
        });

    ui.add_space(10.0);
    let added: usize = d.files.iter().filter_map(|f| f.added).sum();
    let removed: usize = d.files.iter().filter_map(|f| f.removed).sum();
    ui.horizontal(|ui| {
        ui.label(
            RichText::new(format!(
                "{} file{} changed",
                d.files.len(),
                if d.files.len() == 1 { "" } else { "s" }
            ))
            .font(theme::semibold(theme::BODY_SIZE))
            .color(color::TEXT),
        );
        ui.label(RichText::new(format!("+{added}")).color(color::ADDED));
        ui.label(RichText::new(format!("−{removed}")).color(color::REMOVED));
    });
    if d.parents.len() > 1 {
        ui.label(
            RichText::new("Merge commit: changes compared with the first parent.")
                .color(color::TEXT_FAINT)
                .size(theme::SMALL_SIZE),
        );
    }
    let open = project.commit_diff.as_ref().map(|v| &v.key);
    for file in &d.files {
        let key = FileKey {
            source: Source::Commit(d.id),
            path: file.path.clone(),
        };
        let response = file_row(ui, file, open == Some(&key), None);
        response.context_menu(|ui| {
            if ui.button("File history").clicked() {
                actions.push(Action::OpenFileHistory(file.path.clone()));
                ui.close();
            }
            if ui.button("Copy path").clicked() {
                ui.ctx().copy_text(file.path.clone());
                ui.close();
            }
        });
        if response.clicked() {
            actions.push(Action::SelectCommitFile(key));
        }
    }
}

fn meta_key(ui: &mut Ui, key: &str) {
    ui.label(RichText::new(key).color(color::TEXT_FAINT));
}

fn person(ui: &mut Ui, name: &str, email: &str) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        ui.label(RichText::new(name).color(color::TEXT));
        ui.label(RichText::new(email).color(color::TEXT_WEAK));
    });
}

pub fn change_color(change: Change) -> Color32 {
    match change {
        Change::Added => color::ADDED,
        Change::Deleted | Change::Unmerged => color::REMOVED,
        Change::Renamed | Change::Copied => color::LANES[3],
        Change::TypeChanged => color::TEXT_WEAK,
        Change::Modified | Change::None => color::DIRTY_FG,
    }
}

/// A file path as "name" in front and the folder faint behind it.
pub fn paint_path(ui: &Ui, x: f32, y: f32, path: &str, max_width: f32) {
    let (dir, name) = match path.rsplit_once('/') {
        Some((dir, name)) => (dir, name),
        None => ("", path),
    };
    let name = elided(ui, name, theme::body(), color::TEXT, max_width);
    let name_width = name.size().x;
    paint_galley_left(ui.painter(), x, y, name, color::TEXT);
    let room = max_width - name_width - 8.0;
    if !dir.is_empty() && room > 30.0 {
        let dir = elided(ui, dir, theme::small(), color::TEXT_FAINT, room);
        paint_galley_left(
            ui.painter(),
            x + name_width + 8.0,
            y,
            dir,
            color::TEXT_FAINT,
        );
    }
}

pub fn paint_change_letter(ui: &Ui, x: f32, y: f32, change: Change) {
    let c = change_color(change);
    let rect = Rect::from_min_size(pos2(x, y - 8.0), vec2(16.0, 16.0));
    ui.painter().rect_filled(rect, 3, c.gamma_multiply(0.16));
    ui.painter().text(
        rect.center(),
        Align2::CENTER_CENTER,
        change.letter(),
        theme::semibold(theme::SMALL_SIZE),
        c,
    );
}

/// A changed file of a commit or stash; `note` replaces the line counts.
pub fn file_row(ui: &mut Ui, file: &FileChange, selected: bool, note: Option<&str>) -> Response {
    let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width(), 24.0), Sense::click());
    if selected {
        ui.painter().rect_filled(rect, 4, color::SELECTED_ROW_BG);
    } else if response.hovered() {
        ui.painter().rect_filled(rect, 4, color::HOVER_ROW_BG);
    }
    let y = rect.center().y;
    paint_change_letter(ui, rect.left() + 4.0, y, file.change);

    let stats = match (note, file.added, file.removed) {
        (Some(note), _, _) => note.to_owned(),
        (None, Some(a), Some(r)) => format!("+{a} −{r}"),
        _ => "binary".to_owned(),
    };
    let stats = ui
        .painter()
        .layout_no_wrap(stats, theme::small(), color::TEXT_FAINT);
    let stats_x = rect.right() - 4.0 - stats.size().x;
    paint_galley_left(ui.painter(), stats_x, y, stats, color::TEXT_FAINT);

    let x = rect.left() + 28.0;
    paint_path(ui, x, y, &file.path, stats_x - 8.0 - x);
    let tip = match &file.orig_path {
        Some(orig) => format!("{orig} → {}", file.path),
        None => file.path.clone(),
    };
    response.on_hover_text(tip)
}

fn uncommitted(ui: &mut Ui, project: &Project, actions: &mut Vec<Action>) {
    ui.spacing_mut().item_spacing.y = 6.0;
    ui.label(
        RichText::new("Uncommitted changes")
            .font(theme::semibold(15.0))
            .color(color::TEXT),
    );
    let Some(wt) = project.worktree() else { return };
    ui.label(RichText::new(wt.path.display().to_string()).color(color::TEXT_WEAK));
    let Some(status) = &wt.status else { return };
    let staged = status.staged().count();
    let unstaged = status.unstaged().count();
    let conflicted = status.conflicted().count();
    ui.horizontal(|ui| {
        ui.label(RichText::new(format!("{staged} staged")).color(color::ADDED));
        ui.label(RichText::new(format!("{unstaged} unstaged")).color(color::DIRTY_FG));
        if conflicted > 0 {
            ui.label(RichText::new(format!("{conflicted} conflicted")).color(color::REMOVED));
        }
    });
    ui.add_space(4.0);
    if button(ui, "Open File status", ButtonKind::Primary, true).clicked() {
        actions.push(Action::ShowFileStatus);
    }
    ui.add_space(8.0);
    ScrollArea::vertical()
        .id_salt("uncommitted-files")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            for entry in &status.entries {
                let (rect, response) =
                    ui.allocate_exact_size(vec2(ui.available_width(), 24.0), Sense::hover());
                if response.hovered() {
                    ui.painter().rect_filled(rect, 4, color::HOVER_ROW_BG);
                }
                let change = if entry.is_conflicted() {
                    Change::Unmerged
                } else if entry.is_staged() {
                    entry.index
                } else {
                    entry.unstaged_change()
                };
                let y = rect.center().y;
                paint_change_letter(ui, rect.left() + 4.0, y, change);
                let x = rect.left() + 28.0;
                paint_path(ui, x, y, &entry.path, rect.right() - x - 4.0);
            }
        });
}
