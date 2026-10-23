//! The diff of the selected file in File status: hunks with Stage / Unstage /
//! Discard buttons, line numbers, word highlights, and per-line selection
//! ("N lines selected → Stage lines").

use egui::text::CCursor;
use egui::{
    Align2, Frame, Key, Margin, Panel, Rect, RichText, ScrollArea, Sense, Stroke, StrokeKind, Ui,
    pos2, vec2,
};
use graft_core::diff::{FileDiff, Kind, Source};
use graft_core::ops::LineOp;

use super::history::paint_change_letter;
use super::widgets::{ButtonKind, button, elided_path, empty_state, paint_galley_left, spinner};
use crate::project::{Action, Confirm, DiffView, Project};
use crate::theme::{self, color};

const ROW: f32 = 20.0;
const GUTTER: f32 = 24.0;
const NUM: f32 = 42.0;

pub fn show(ui: &mut Ui, project: &Project, actions: &mut Vec<Action>) {
    let Some(view) = &project.diff_view else {
        empty_state(
            ui,
            "No file selected",
            "Pick a file on the left to see its changes.",
        );
        return;
    };
    let busy = project.is_operating();
    header(ui, view, busy, actions);
    if !view.selection.is_empty() {
        selection_bar(ui, view, busy, actions);
    }

    let Some(diff) = &view.diff else {
        if let Some(error) = &view.error {
            empty_state(ui, "Could not load the diff", error);
        } else {
            let rect = ui.available_rect_before_wrap();
            spinner(ui, rect.center(), 9.0, color::TEXT_WEAK);
        }
        return;
    };
    if diff.binary {
        empty_state(
            ui,
            "Binary file",
            "There is no text diff to show for this file.",
        );
        return;
    }
    if diff.hunks.is_empty() {
        let detail = if view.key.source == Source::Unstaged && diff.header.starts_with(b"diff --cc")
        {
            "This file has a merge conflict. Resolving conflicts comes in a later step."
        } else {
            "Only the file mode or whitespace changed, or the file is empty."
        };
        empty_state(ui, "No line changes", detail);
        return;
    }
    if ui.input(|i| i.key_pressed(Key::Escape)) && !view.selection.is_empty() {
        actions.push(Action::ClearLines);
    }
    lines(ui, view, diff, busy, actions);
}

fn header(ui: &mut Ui, view: &DiffView, busy: bool, actions: &mut Vec<Action>) {
    Panel::top("diff-header")
        .frame(
            Frame::new()
                .fill(color::BAR_BG)
                .inner_margin(Margin::symmetric(12, 6)),
        )
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                let path = view.key.path.clone();
                let (stats, change) = match &view.diff {
                    Some(d) => (format!("+{}  −{}", d.added(), d.removed()), d),
                    None => (String::new(), &FileDiff::default()),
                };
                let letter = match view.key.source {
                    Source::Untracked => graft_core::status::Change::Added,
                    _ if change.new_file => graft_core::status::Change::Added,
                    _ if change.deleted_file => graft_core::status::Change::Deleted,
                    _ => graft_core::status::Change::Modified,
                };
                let (rect, _) = ui.allocate_exact_size(vec2(18.0, 30.0), Sense::hover());
                paint_change_letter(ui, rect.left(), rect.center().y, letter);

                let side = match view.key.source {
                    Source::Staged => "Staged",
                    Source::Unstaged => "Unstaged",
                    Source::Untracked => "Untracked",
                    Source::Commit(_) => "Commit",
                };
                let right_width = 230.0;
                let room = (ui.available_width() - right_width - 140.0).max(80.0);
                let galley = elided_path(
                    ui,
                    &path,
                    theme::semibold(theme::BODY_SIZE),
                    color::TEXT,
                    room,
                );
                let (rect, _) = ui.allocate_exact_size(vec2(galley.size().x, 30.0), Sense::hover());
                paint_galley_left(
                    ui.painter(),
                    rect.left(),
                    rect.center().y,
                    galley,
                    color::TEXT,
                );
                ui.label(
                    RichText::new(side)
                        .color(color::TEXT_FAINT)
                        .size(theme::SMALL_SIZE),
                );
                ui.label(
                    RichText::new(stats)
                        .color(color::TEXT_WEAK)
                        .size(theme::SMALL_SIZE),
                );

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let paths = vec![path.clone()];
                    match view.key.source {
                        Source::Staged => {
                            if button(ui, "Unstage file", ButtonKind::Secondary, !busy).clicked() {
                                actions.push(Action::UnstageFiles(paths));
                            }
                        }
                        Source::Unstaged | Source::Untracked => {
                            if button(ui, "Stage file", ButtonKind::Secondary, !busy).clicked() {
                                actions.push(Action::StageFiles(paths));
                            }
                            if button(ui, "Discard…", ButtonKind::Secondary, !busy).clicked() {
                                actions.push(discard_file(&view.key.source, path));
                            }
                        }
                        Source::Commit(_) => {}
                    }
                });
            });
        });
}

/// Ask before discarding a whole file.
pub fn discard_file(source: &Source, path: String) -> Action {
    let untracked = *source == Source::Untracked;
    let body = if untracked {
        format!("{path} is not tracked by git. It will be moved to the Recycle Bin.")
    } else {
        format!("All unstaged changes to {path} will be lost. Staged changes stay.")
    };
    let (tracked, untracked_paths) = if untracked {
        (Vec::new(), vec![path])
    } else {
        (vec![path], Vec::new())
    };
    Action::Ask(Box::new(Confirm {
        title: "Discard changes?".to_owned(),
        body,
        confirm_label: "Discard".to_owned(),
        action: Action::Discard {
            tracked,
            untracked: untracked_paths,
        },
    }))
}

fn selection_bar(ui: &mut Ui, view: &DiffView, busy: bool, actions: &mut Vec<Action>) {
    Panel::bottom("diff-selection")
        .frame(
            Frame::new()
                .fill(color::SELECTED_ROW_BG)
                .inner_margin(Margin::symmetric(12, 6)),
        )
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                let n = view.selection.len();
                ui.label(
                    RichText::new(format!("{n} line{} selected", if n == 1 { "" } else { "s" }))
                        .font(theme::semibold(theme::BODY_SIZE))
                        .color(color::TEXT),
                );
                ui.label(RichText::new("Shift-click or drag to select more · Esc clears").color(color::TEXT_FAINT).size(theme::SMALL_SIZE));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    match view.key.source {
                        Source::Staged => {
                            if button(ui, "Unstage lines", ButtonKind::Primary, !busy).clicked() {
                                actions.push(Action::LinesOp(LineOp::Unstage));
                            }
                        }
                        Source::Unstaged | Source::Untracked => {
                            if button(ui, "Stage lines", ButtonKind::Primary, !busy).clicked() {
                                actions.push(Action::LinesOp(LineOp::Stage));
                            }
                            if button(ui, "Discard lines…", ButtonKind::Secondary, !busy).clicked() {
                                actions.push(Action::Ask(Box::new(Confirm {
                                    title: "Discard the selected lines?".to_owned(),
                                    body: format!(
                                        "{n} selected line{} in {} will be reverted. This cannot be undone.",
                                        if n == 1 { "" } else { "s" },
                                        view.key.path
                                    ),
                                    confirm_label: "Discard lines".to_owned(),
                                    action: Action::LinesOp(LineOp::Discard),
                                })));
                            }
                        }
                        Source::Commit(_) => {}
                    }
                    if button(ui, "Clear", ButtonKind::Secondary, true).clicked() {
                        actions.push(Action::ClearLines);
                    }
                });
            });
        });
}

#[derive(Clone, Copy)]
enum Row {
    Hunk(usize),
    Line(usize, usize),
}

fn lines(ui: &mut Ui, view: &DiffView, diff: &FileDiff, busy: bool, actions: &mut Vec<Action>) {
    let rows: Vec<Row> = diff
        .hunks
        .iter()
        .enumerate()
        .flat_map(|(h, hunk)| {
            std::iter::once(Row::Hunk(h)).chain((0..hunk.lines.len()).map(move |l| Row::Line(h, l)))
        })
        .collect();
    let char_width = ui
        .painter()
        .layout_no_wrap("M".to_owned(), theme::mono(), color::TEXT)
        .size()
        .x;
    let longest = diff
        .hunks
        .iter()
        .flat_map(|h| &h.lines)
        .map(|l| l.content.len())
        .max()
        .unwrap_or(0);
    let text_x = GUTTER + 2.0 * NUM + 16.0;
    let content_width = (text_x + longest as f32 * char_width + 40.0).max(ui.available_width());
    let (pressed, down, shift) = ui.input(|i| {
        (
            i.pointer.primary_pressed(),
            i.pointer.primary_down(),
            i.modifiers.shift,
        )
    });

    ui.spacing_mut().item_spacing.y = 0.0;
    ScrollArea::both()
        .id_salt(("diff-scroll", &view.key.path))
        .auto_shrink([false, false])
        .show_rows(ui, ROW, rows.len(), |ui, range| {
            for row in &rows[range] {
                let (rect, _) = ui.allocate_exact_size(vec2(content_width, ROW), Sense::hover());
                match *row {
                    Row::Hunk(h) => hunk_row(ui, rect, view, diff, h, busy, actions),
                    Row::Line(h, l) => {
                        let line = &diff.hunks[h].lines[l];
                        let selected = view.selection.contains(&(h, l));
                        let word = view.words.get(h).and_then(|w| w.get(l)).cloned().flatten();
                        paint_line(ui, rect, line, selected, word, text_x);
                        let hovered = ui.rect_contains_pointer(rect) && ui.is_rect_visible(rect);
                        let selectable =
                            line.is_change() && !matches!(view.key.source, Source::Commit(_));
                        if selectable && hovered {
                            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                            if pressed {
                                actions.push(Action::ClickLine {
                                    hunk: h,
                                    line: l,
                                    shift,
                                });
                            } else if down && view.anchor.is_some_and(|a| a != (h, l)) {
                                actions.push(Action::DragLine { hunk: h, line: l });
                            }
                        }
                    }
                }
            }
        });
}

fn hunk_row(
    ui: &mut Ui,
    rect: Rect,
    view: &DiffView,
    diff: &FileDiff,
    h: usize,
    busy: bool,
    actions: &mut Vec<Action>,
) {
    let hunk = &diff.hunks[h];
    let painter = ui.painter();
    painter.rect_filled(rect, 0, color::SIDEBAR_BG);
    let header = format!(
        "@@ -{},{} +{},{} @@ {}",
        hunk.old_start, hunk.old_count, hunk.new_start, hunk.new_count, hunk.section
    );
    let galley = painter.layout_no_wrap(header, theme::mono(), color::TEXT_FAINT);
    paint_galley_left(
        painter,
        rect.left() + GUTTER,
        rect.center().y,
        galley,
        color::TEXT_FAINT,
    );

    // Buttons stick to the visible right edge, not the scrolled content.
    let right = ui.clip_rect().right().min(rect.right()) - 8.0;
    let mut x = right;
    let mut small = |label: &str, action: Action, danger: bool| {
        let text = ui
            .painter()
            .layout_no_wrap(label.to_owned(), theme::small(), color::TEXT);
        let w = text.size().x + 16.0;
        x -= w;
        let r = Rect::from_min_size(pos2(x, rect.top() + 2.0), vec2(w, ROW - 4.0));
        x -= 6.0;
        let response = ui.interact(
            r,
            ui.scope_id().with(("hunk-btn", h, label)),
            Sense::click(),
        );
        let enabled = !busy;
        let fill = if response.hovered() && enabled {
            color::CONTROL_HOVER_BG
        } else {
            color::CONTROL_BG
        };
        let fg = match (enabled, danger) {
            (false, _) => color::TEXT_FAINT,
            (true, true) => color::REMOVED,
            (true, false) => color::TEXT,
        };
        ui.painter().rect(
            r,
            4,
            fill,
            Stroke::new(1.0, color::CONTROL_BORDER),
            StrokeKind::Inside,
        );
        ui.painter()
            .galley(r.center() - text.size() / 2.0, text, fg);
        if response.clicked() && enabled {
            actions.push(action);
        }
    };
    match view.key.source {
        Source::Staged => small("Unstage hunk", Action::HunkOp(h, LineOp::Unstage), false),
        Source::Unstaged | Source::Untracked => {
            small("Stage hunk", Action::HunkOp(h, LineOp::Stage), false);
            small(
                "Discard hunk",
                Action::Ask(Box::new(Confirm {
                    title: "Discard this hunk?".to_owned(),
                    body: format!(
                        "The changes in this hunk of {} will be reverted. This cannot be undone.",
                        view.key.path
                    ),
                    confirm_label: "Discard hunk".to_owned(),
                    action: Action::HunkOp(h, LineOp::Discard),
                })),
                true,
            );
        }
        Source::Commit(_) => {}
    }
}

fn paint_line(
    ui: &Ui,
    rect: Rect,
    line: &graft_core::diff::Line,
    selected: bool,
    word: Option<std::ops::Range<usize>>,
    text_x: f32,
) {
    let painter = ui.painter();
    let tint = match line.kind {
        Kind::Added => Some(color::ADDED),
        Kind::Removed => Some(color::REMOVED),
        Kind::Context => None,
    };
    if let Some(tint) = tint {
        let strength = if selected { 0.22 } else { 0.09 };
        painter.rect_filled(rect, 0, color::APP_BG.lerp_to_gamma(tint, strength));
    }
    let y = rect.center().y;

    // Selection box in the gutter.
    if line.is_change() {
        let b = Rect::from_center_size(pos2(rect.left() + GUTTER / 2.0, y), vec2(12.0, 12.0));
        if selected {
            painter.rect_filled(b, 3, color::ACCENT);
            painter.line(
                vec![
                    pos2(b.left() + 2.5, y),
                    pos2(b.left() + 5.0, y + 2.5),
                    pos2(b.right() - 2.5, y - 2.5),
                ],
                Stroke::new(1.6, color::ON_ACCENT),
            );
        } else if ui.rect_contains_pointer(rect) {
            painter.rect_stroke(b, 3, Stroke::new(1.0, color::TEXT_WEAK), StrokeKind::Inside);
        } else {
            painter.rect_stroke(
                b,
                3,
                Stroke::new(1.0, color::CONTROL_BORDER),
                StrokeKind::Inside,
            );
        }
    }

    let num = |n: Option<u32>, right: f32| {
        if let Some(n) = n {
            painter.text(
                pos2(right, y),
                Align2::RIGHT_CENTER,
                n.to_string(),
                theme::mono(),
                color::TEXT_FAINT,
            );
        }
    };
    num(line.old_no, rect.left() + GUTTER + NUM - 6.0);
    num(line.new_no, rect.left() + GUTTER + 2.0 * NUM - 6.0);

    let (marker, fg) = match line.kind {
        Kind::Added => ("+", color::ADDED),
        Kind::Removed => ("−", color::REMOVED),
        Kind::Context => ("", color::TEXT_FAINT),
    };
    painter.text(
        pos2(rect.left() + GUTTER + 2.0 * NUM + 4.0, y),
        Align2::LEFT_CENTER,
        marker,
        theme::mono(),
        fg,
    );

    let text = line.text();
    let text_color = if line.kind == Kind::Context {
        color::TEXT_MID
    } else {
        color::TEXT
    };
    let galley = painter.layout_no_wrap(text.clone(), theme::mono(), text_color);
    let origin = pos2(rect.left() + text_x, y - galley.size().y / 2.0);
    if let (Some(range), Some(tint)) = (word, tint)
        && !range.is_empty()
    {
        let start = text[..range.start].chars().count();
        let end = text[..range.end].chars().count();
        let a = galley.pos_from_cursor(CCursor::new(start));
        let b = galley.pos_from_cursor(CCursor::new(end));
        let r = Rect::from_min_max(
            origin + vec2(a.left(), 1.0),
            origin + vec2(b.left(), galley.size().y - 1.0),
        );
        painter.rect_filled(r, 2, color::APP_BG.lerp_to_gamma(tint, 0.35));
    }
    let text_width = galley.size().x;
    painter.galley(origin, galley, text_color);
    if line.no_newline {
        let x = origin.x + text_width + 8.0;
        painter.text(
            pos2(x, y),
            Align2::LEFT_CENTER,
            "no newline at end of file",
            theme::small(),
            color::TEXT_FAINT,
        );
    }
}
