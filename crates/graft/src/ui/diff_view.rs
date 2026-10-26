//! The diff of the selected file in File status: hunks with Stage / Unstage /
//! Discard buttons, line numbers, word highlights, and per-line selection
//! ("N lines selected → Stage lines"). In Review, lines are picked to comment
//! on and comments show under the lines they are about. With wrapping on,
//! a long line takes as many rows as it needs instead of scrolling sideways.

use std::sync::Arc;

use egui::style::ScrollStyle;
use egui::text::{LayoutJob, TextFormat};
use egui::{
    Align, Align2, Color32, Frame, Galley, Key, Margin, Painter, Panel, Pos2, Rect, RichText,
    ScrollArea, Sense, Stroke, StrokeKind, Ui, pos2, vec2,
};
use graft_core::diff::{FileDiff, Kind, Source};
use graft_core::ops::LineOp;
use graft_core::syntax::{Spans, Token};

use super::history::paint_change_letter;
use super::icons::Icon;
use super::widgets::{
    ButtonKind, button, elided_path, empty_state, icon_button, paint_galley_left, spinner,
};
use crate::project::comments::{LineComment, Pick, Placed, Side};
use crate::project::{Action, Confirm, DiffView, Project};
use crate::theme::{self, color};

const ROW: f32 = 20.0;
const GUTTER: f32 = 24.0;
const NUM: f32 = 42.0;

/// Lines longer than this (minified code) are cut on screen: laying out and
/// drawing a 300k-character line takes longer than a frame.
const MAX_SHOWN_CHARS: usize = 2_000;

/// The text to draw for a line, and how many of its bytes that keeps.
fn shown(mut text: String) -> (String, usize) {
    let Some((cut, _)) = text.char_indices().nth(MAX_SHOWN_CHARS) else {
        let len = text.len();
        return (text, len);
    };
    let more = text[cut..].chars().count();
    text.truncate(cut);
    text.push_str(&format!("  … {more} more characters"));
    (text, cut)
}

fn token_color(token: Token) -> Color32 {
    match token {
        Token::Keyword => color::syn_keyword(),
        Token::String => color::syn_string(),
        Token::Comment => color::syn_comment(),
        Token::Number | Token::Constant => color::syn_number(),
        Token::Function => color::syn_function(),
        Token::Type => color::syn_type(),
    }
}

/// A line of code with its syntax colors; text without a token is `plain`.
/// With `wrap`, the line breaks into rows no wider than that, each one
/// [`ROW`] tall.
fn code_galley(
    painter: &Painter,
    text: String,
    spans: Option<&Spans>,
    plain: Color32,
    wrap: Option<f32>,
) -> Arc<Galley> {
    let spans = spans.filter(|s| !s.is_empty());
    if spans.is_none() && wrap.is_none() {
        return painter.layout_no_wrap(text, theme::mono(), plain);
    }
    let format = |c| {
        let mut format = TextFormat::simple(theme::mono(), c);
        if wrap.is_some() {
            format.line_height = Some(ROW);
            format.valign = Align::Center;
        }
        format
    };
    let mut job = LayoutJob::default();
    if let Some(width) = wrap {
        job.wrap.max_width = width;
    }
    let mut pos = 0;
    for (range, token) in spans.into_iter().flatten() {
        let (Some(before), Some(part)) = (text.get(pos..range.start), text.get(range.clone()))
        else {
            continue;
        };
        job.append(before, 0.0, format(plain));
        job.append(part, 0.0, format(token_color(*token)));
        pos = range.end;
    }
    job.append(text.get(pos..).unwrap_or_default(), 0.0, format(plain));
    painter.layout_job(job)
}

/// The changed words of a line: a stronger tint behind them, on every row
/// they cover. `range` is in bytes of `text`.
fn paint_word(
    painter: &Painter,
    galley: &Galley,
    origin: Pos2,
    text: &str,
    range: std::ops::Range<usize>,
    tint: Color32,
) {
    let start = text[..range.start].chars().count();
    let end = text[..range.end].chars().count();
    let mut index = 0;
    for row in &galley.rows {
        let mut span: Option<(f32, f32)> = None;
        for glyph in &row.glyphs {
            if (start..end).contains(&index) {
                let (left, right) = (glyph.pos.x, glyph.pos.x + glyph.advance_width);
                span = Some(span.map_or((left, right), |(l, r)| (l.min(left), r.max(right))));
            }
            index += 1;
        }
        if let Some((left, right)) = span {
            let top = origin.y + row.pos.y + 1.0;
            let bottom = origin.y + row.pos.y + row.size.y - 1.0;
            let r = Rect::from_min_max(
                pos2(origin.x + row.pos.x + left, top),
                pos2(origin.x + row.pos.x + right, bottom),
            );
            painter.rect_filled(r, 2, color::app_bg().lerp_to_gamma(tint, 0.35));
        }
    }
}

/// How many rows a line takes when wrapped at `width` (one without wrapping).
fn line_rows(
    painter: &Painter,
    view: &DiffView,
    diff: &FileDiff,
    at: (usize, usize),
    width: Option<f32>,
    char_width: f32,
) -> usize {
    let Some(width) = width else {
        return 1;
    };
    let line = &diff.hunks[at.0].lines[at.1];
    // Tabs show as four spaces; anything shorter than the row fits for sure.
    let tabs = line.content.iter().filter(|&&b| b == b'\t').count();
    if (line.content.len() + 3 * tabs) as f32 * char_width <= width {
        return 1;
    }
    let (text, _) = shown(line.text());
    let spans = spans_at(view, at.0, at.1);
    code_galley(painter, text, spans, color::text(), Some(width))
        .rows
        .len()
        .max(1)
}

/// Diff scroll bars always show and take their own room, so they are easy
/// to find and dragging them never lands on a line.
fn diff_scroll_style(ui: &mut Ui) {
    ui.spacing_mut().scroll = ScrollStyle {
        bar_width: 8.0,
        ..ScrollStyle::solid()
    };
    // The handle uses the widget fills, which barely show on the background.
    let widgets = &mut ui.visuals_mut().widgets;
    widgets.inactive.bg_fill = color::text_faint().gamma_multiply(0.6);
    widgets.hovered.bg_fill = color::text_weak();
    widgets.active.bg_fill = color::text_weak();
}

fn char_width(ui: &Ui) -> f32 {
    ui.painter()
        .layout_no_wrap("M".to_owned(), theme::mono(), color::text())
        .size()
        .x
}

fn spans_at(view: &DiffView, h: usize, l: usize) -> Option<&Spans> {
    view.highlights.syntax.get(h).and_then(|s| s.get(l))
}

/// The Review tab's extras on a diff: lines picked to comment on, and
/// comments shown under the line they end on.
pub struct Marks<'a> {
    pub pick: Option<Pick>,
    pub placed: Vec<Placed<'a>>,
}

/// A comment laid out to sit under its line, `rows` rows tall.
struct Block<'a> {
    at: (usize, usize),
    comment: &'a LineComment,
    galley: Arc<Galley>,
    rows: usize,
}

const COMMENT_PAD: f32 = 8.0;
const COMMENT_HEADER: f32 = 18.0;
/// Room right of a comment block (scroll bar).
const COMMENT_RIGHT: f32 = 16.0;

fn blocks<'a>(ui: &Ui, marks: Option<&Marks<'a>>, width: f32) -> Vec<Block<'a>> {
    let Some(marks) = marks else {
        return Vec::new();
    };
    let wrap = (width - GUTTER - COMMENT_RIGHT - 2.0 * COMMENT_PAD - 3.0).max(120.0);
    marks
        .placed
        .iter()
        .map(|&(at, comment)| {
            let galley =
                ui.painter()
                    .layout(comment.text.clone(), theme::body(), color::text(), wrap);
            let height = COMMENT_HEADER + galley.size().y + 2.0 * COMMENT_PAD + 6.0;
            Block {
                at,
                comment,
                galley,
                rows: (height / ROW).ceil() as usize,
            }
        })
        .collect()
}

/// Row `part` of a comment block. Every row paints the whole block, clipped
/// to itself, so the block scrolls in and out smoothly.
fn comment_row(ui: &Ui, rect: Rect, block: &Block, part: usize, actions: &mut Vec<Action>) {
    let clip = ui.clip_rect();
    let left = clip.left().max(rect.left()) + GUTTER;
    let right = clip.right().min(rect.right()) - COMMENT_RIGHT;
    let top = rect.top() - part as f32 * ROW + 3.0;
    let bottom = top + block.rows as f32 * ROW - 6.0;
    let block_rect = Rect::from_min_max(pos2(left, top), pos2(right, bottom));
    let hovered = ui.rect_contains_pointer(block_rect.intersect(clip));
    let painter = ui.painter().with_clip_rect(rect.intersect(clip));
    painter.rect(
        block_rect,
        6,
        if hovered {
            color::control_hover_bg()
        } else {
            color::sidebar_bg()
        },
        Stroke::new(1.0, color::border()),
        StrokeKind::Inside,
    );
    painter.rect_filled(
        Rect::from_min_size(block_rect.min, vec2(3.0, block_rect.height())),
        egui::CornerRadius {
            nw: 6,
            sw: 6,
            ..Default::default()
        },
        color::accent(),
    );
    let x = left + 3.0 + COMMENT_PAD;
    painter.text(
        pos2(x, top + COMMENT_PAD),
        Align2::LEFT_TOP,
        block.comment.lines_label(),
        theme::small(),
        color::text_faint(),
    );
    if hovered {
        painter.text(
            pos2(right - COMMENT_PAD, top + COMMENT_PAD),
            Align2::RIGHT_TOP,
            "Click to edit or delete",
            theme::small(),
            color::text_faint(),
        );
    }
    painter.galley(
        pos2(x, top + COMMENT_PAD + COMMENT_HEADER),
        block.galley.clone(),
        color::text(),
    );
    let response = ui.interact(
        rect.intersect(block_rect),
        ui.scope_id().with(("comment", block.comment.id, part)),
        Sense::click(),
    );
    if hovered {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    if response.clicked() {
        actions.push(Action::EditComment(block.comment.id));
    }
}

/// Picked lines: a light accent wash and a bar at the left edge.
fn paint_picked(ui: &Ui, rect: Rect) {
    let painter = ui.painter().with_clip_rect(rect.intersect(ui.clip_rect()));
    painter.rect_filled(rect, 0, color::accent().gamma_multiply(0.14));
    painter.rect_filled(
        Rect::from_min_size(rect.min, vec2(3.0, rect.height())),
        0,
        color::accent(),
    );
}

/// Mouse state for picking lines: pressed this frame, held, shift.
#[derive(Clone, Copy)]
struct Pointer {
    pressed: bool,
    down: bool,
    shift: bool,
}

/// Call inside the scroll area: a press that started outside its lines
/// (on a scroll bar, say) neither picks nor extends.
fn pointer(ui: &Ui) -> Pointer {
    let lines = ui.clip_rect();
    ui.input(|i| {
        let on_lines = i.pointer.press_origin().is_some_and(|p| lines.contains(p));
        Pointer {
            pressed: i.pointer.primary_pressed() && on_lines,
            down: i.pointer.primary_down() && on_lines,
            shift: i.modifiers.shift,
        }
    })
}

/// Click picks a line, Shift-click or dragging extends the pick.
fn pick_on_hover(
    ui: &Ui,
    marks: &Marks,
    at: (usize, usize),
    side: Option<Side>,
    p: Pointer,
    actions: &mut Vec<Action>,
) {
    ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    let same_side = marks.pick.is_some_and(|pick| pick.side == side);
    if p.pressed {
        actions.push(Action::PickLine {
            at,
            side,
            extend: p.shift && same_side,
        });
    } else if p.down && same_side && marks.pick.is_some_and(|pick| pick.cursor != at) {
        actions.push(Action::PickLine {
            at,
            side,
            extend: true,
        });
    }
}

pub fn show(ui: &mut Ui, project: &Project, actions: &mut Vec<Action>) {
    let Some(view) = &project.diff_view else {
        empty_state(
            ui,
            "No file selected",
            "Pick a file on the left to see its changes.",
        );
        return;
    };
    let wrap = project.settings.wrap_lines;
    show_view(ui, view, project.is_operating(), wrap, actions);
}

/// Any file diff: File status (with staging) or a commit or stash (read-only,
/// with a close button).
pub fn show_view(ui: &mut Ui, view: &DiffView, busy: bool, wrap: bool, actions: &mut Vec<Action>) {
    if matches!(view.key.source, Source::Commit(_)) && ui.input(|i| i.key_pressed(Key::Escape)) {
        actions.push(Action::CloseCommitFile);
    }
    header(ui, view, busy, actions);
    if !view.selection.is_empty() {
        selection_bar(ui, view, busy, actions);
    }
    body(ui, view, busy, false, wrap, None, actions);
}

/// The diff without its header: unified, or side by side with `split`;
/// long lines wrap with `wrap`. With `marks` (Review), clicking lines picks
/// them to comment on.
pub fn body(
    ui: &mut Ui,
    view: &DiffView,
    busy: bool,
    split: bool,
    wrap: bool,
    marks: Option<&Marks>,
    actions: &mut Vec<Action>,
) {
    let Some(diff) = &view.diff else {
        if let Some(error) = &view.error {
            empty_state(ui, "Could not load the diff", error);
        } else {
            let rect = ui.available_rect_before_wrap();
            spinner(ui, rect.center(), 9.0, color::text_weak());
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
            "This file has a merge conflict."
        } else {
            "Only the file mode or whitespace changed, or the file is empty."
        };
        empty_state(ui, "No line changes", detail);
        return;
    }
    if ui.input(|i| i.key_pressed(Key::Escape)) && !view.selection.is_empty() {
        actions.push(Action::ClearLines);
    }
    diff_scroll_style(ui);
    if split {
        side_by_side(ui, view, diff, wrap, marks, actions);
    } else {
        lines(ui, view, diff, busy, wrap, marks, actions);
    }
}

/// A row of the side-by-side view: old line on the left, new on the right.
#[derive(Clone, Copy)]
enum Pair {
    Hunk(usize),
    /// Row `.2` of an old and a new line (more than one when they wrap).
    Lines(Option<(usize, usize)>, Option<(usize, usize)>, usize),
    /// Row `.1` of comment block `.0`.
    Comment(usize, usize),
}

/// The rows of the side-by-side view; `rows_of` says how many rows a line
/// takes.
fn pair_rows(
    diff: &FileDiff,
    blocks: &[Block],
    rows_of: impl Fn((usize, usize)) -> usize,
) -> Vec<Pair> {
    let mut rows = Vec::new();
    let push = |rows: &mut Vec<Pair>, pair: Pair| {
        let Pair::Lines(left, right, _) = pair else {
            rows.push(pair);
            return;
        };
        let parts = [left, right].into_iter().flatten().map(&rows_of).max();
        rows.extend((0..parts.unwrap_or(1)).map(|part| Pair::Lines(left, right, part)));
        for (b, block) in blocks.iter().enumerate() {
            if left == Some(block.at) || right == Some(block.at) {
                rows.extend((0..block.rows).map(|part| Pair::Comment(b, part)));
            }
        }
    };
    for (h, hunk) in diff.hunks.iter().enumerate() {
        rows.push(Pair::Hunk(h));
        let mut l = 0;
        while l < hunk.lines.len() {
            if hunk.lines[l].kind == Kind::Context {
                push(&mut rows, Pair::Lines(Some((h, l)), Some((h, l)), 0));
                l += 1;
                continue;
            }
            // A change block: removed lines, then added lines, side by side.
            let mut removed = Vec::new();
            let mut added = Vec::new();
            while l < hunk.lines.len() && hunk.lines[l].kind != Kind::Context {
                match hunk.lines[l].kind {
                    Kind::Removed => removed.push((h, l)),
                    _ => added.push((h, l)),
                }
                l += 1;
            }
            for i in 0..removed.len().max(added.len()) {
                push(
                    &mut rows,
                    Pair::Lines(removed.get(i).copied(), added.get(i).copied(), 0),
                );
            }
        }
    }
    rows
}

fn side_by_side(
    ui: &mut Ui,
    view: &DiffView,
    diff: &FileDiff,
    wrap: bool,
    marks: Option<&Marks>,
    actions: &mut Vec<Action>,
) {
    let width = ui.available_width() - ui.spacing().scroll.allocated_width();
    let blocks = blocks(ui, marks, width);
    // Each half has its line numbers, then the text up to the middle line.
    let wrap_width = wrap.then_some((width / 2.0 - 1.0 - NUM - 16.0).max(80.0));
    let char_width = char_width(ui);
    let painter = ui.painter().clone();
    let rows = pair_rows(diff, &blocks, |at| {
        line_rows(&painter, view, diff, at, wrap_width, char_width)
    });
    ui.spacing_mut().item_spacing.y = 0.0;
    ScrollArea::vertical()
        .id_salt(("diff-split-scroll", &view.key.path))
        .auto_shrink([false, false])
        .show_rows(ui, ROW, rows.len(), |ui, range| {
            let p = pointer(ui);
            for row in &rows[range] {
                let (rect, _) =
                    ui.allocate_exact_size(vec2(ui.available_width(), ROW), Sense::hover());
                match *row {
                    Pair::Hunk(h) => {
                        let hunk = &diff.hunks[h];
                        let painter = ui.painter();
                        painter.rect_filled(rect, 0, color::sidebar_bg());
                        let header = format!(
                            "@@ -{},{} +{},{} @@ {}",
                            hunk.old_start,
                            hunk.old_count,
                            hunk.new_start,
                            hunk.new_count,
                            hunk.section
                        );
                        let galley =
                            painter.layout_no_wrap(header, theme::mono(), color::text_faint());
                        paint_galley_left(
                            painter,
                            rect.left() + 8.0,
                            rect.center().y,
                            galley,
                            color::text_faint(),
                        );
                    }
                    Pair::Lines(left, right, part) => {
                        let mid = rect.center().x;
                        let left_rect = Rect::from_min_max(rect.min, pos2(mid - 1.0, rect.max.y));
                        let right_rect = Rect::from_min_max(pos2(mid + 1.0, rect.min.y), rect.max);
                        for (r, at, old) in [(left_rect, left, true), (right_rect, right, false)] {
                            half(ui, r, view, diff, at, old, part, wrap_width);
                            let (Some(marks), Some(at)) = (marks, at) else {
                                continue;
                            };
                            let side = Some(if old { Side::Old } else { Side::New });
                            let kind = diff.hunks[at.0].lines[at.1].kind;
                            let picked = marks
                                .pick
                                .is_some_and(|p| p.side.is_none_or(|s| Some(s) == side))
                                && marks.pick.is_some_and(|p| p.contains(at, kind));
                            if picked {
                                paint_picked(ui, r);
                            }
                            if ui.rect_contains_pointer(r) {
                                pick_on_hover(ui, marks, at, side, p, actions);
                            }
                        }
                        ui.painter().line_segment(
                            [pos2(mid, rect.top()), pos2(mid, rect.bottom())],
                            Stroke::new(1.0, color::border()),
                        );
                    }
                    Pair::Comment(b, part) => comment_row(ui, rect, &blocks[b], part, actions),
                }
            }
        });
}

/// Row `part` of one side of a side-by-side row. A missing line is a
/// hatched gap; a wrapped line paints all of its text, clipped to this row.
#[allow(clippy::too_many_arguments)]
fn half(
    ui: &Ui,
    rect: Rect,
    view: &DiffView,
    diff: &FileDiff,
    at: Option<(usize, usize)>,
    old: bool,
    part: usize,
    wrap: Option<f32>,
) {
    let painter = ui.painter().with_clip_rect(rect.intersect(ui.clip_rect()));
    let Some((h, l)) = at else {
        painter.rect_filled(
            rect,
            0,
            color::sidebar_bg().lerp_to_gamma(color::app_bg(), 0.4),
        );
        return;
    };
    let line = &diff.hunks[h].lines[l];
    let tint = match line.kind {
        Kind::Added => Some(color::added()),
        Kind::Removed => Some(color::removed()),
        Kind::Context => None,
    };
    if let Some(tint) = tint {
        painter.rect_filled(rect, 0, color::app_bg().lerp_to_gamma(tint, 0.09));
    }
    let y = rect.center().y;
    let number = if old { line.old_no } else { line.new_no };
    if let Some(n) = number
        && part == 0
    {
        painter.text(
            pos2(rect.left() + NUM - 6.0, y),
            Align2::RIGHT_CENTER,
            n.to_string(),
            theme::mono(),
            color::text_faint(),
        );
    }
    let (text, kept) = shown(line.text());
    let text_color = if tint.is_some() {
        color::text()
    } else {
        color::text_mid()
    };
    let galley = code_galley(
        &painter,
        text.clone(),
        spans_at(view, h, l),
        text_color,
        wrap,
    );
    let origin = match wrap {
        Some(_) => pos2(rect.left() + NUM + 8.0, rect.top() - part as f32 * ROW),
        None => pos2(rect.left() + NUM + 8.0, y - galley.size().y / 2.0),
    };
    let word = view
        .highlights
        .words
        .get(h)
        .and_then(|w| w.get(l))
        .cloned()
        .flatten();
    if let (Some(range), Some(tint)) = (word, tint)
        && !range.is_empty()
        && range.end <= kept
    {
        paint_word(&painter, &galley, origin, &text, range, tint);
    }
    let too_wide = wrap.is_none() && origin.x + galley.size().x > rect.right();
    painter.galley(origin, galley, text_color);
    if too_wide && ui.rect_contains_pointer(rect) {
        egui::Tooltip::always_open(
            ui.ctx().clone(),
            ui.layer_id(),
            ui.scope_id().with(("line-tip", h, l, old)),
            egui::PopupAnchor::Pointer,
        )
        .show(|ui| ui.label(RichText::new(&text).font(theme::mono())));
    }
}

fn header(ui: &mut Ui, view: &DiffView, busy: bool, actions: &mut Vec<Action>) {
    Panel::top("diff-header")
        .frame(
            Frame::new()
                .fill(color::bar_bg())
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
                    Source::Base(_) => "Review",
                };
                let right_width = 230.0;
                let room = (ui.available_width() - right_width - 140.0).max(80.0);
                let galley = elided_path(
                    ui,
                    &path,
                    theme::semibold(theme::BODY_SIZE),
                    color::text(),
                    room,
                );
                let (rect, _) = ui.allocate_exact_size(vec2(galley.size().x, 30.0), Sense::hover());
                paint_galley_left(
                    ui.painter(),
                    rect.left(),
                    rect.center().y,
                    galley,
                    color::text(),
                );
                ui.label(
                    RichText::new(side)
                        .color(color::text_faint())
                        .size(theme::SMALL_SIZE),
                );
                ui.label(
                    RichText::new(stats)
                        .color(color::text_weak())
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
                        Source::Commit(_) | Source::Base(_) => {
                            if icon_button(ui, Icon::Close, 26.0)
                                .on_hover_text("Close the diff (Esc)")
                                .clicked()
                            {
                                actions.push(Action::CloseCommitFile);
                            }
                        }
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
                .fill(color::selected_row_bg())
                .inner_margin(Margin::symmetric(12, 6)),
        )
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                let n = view.selection.len();
                ui.label(
                    RichText::new(format!("{n} line{} selected", if n == 1 { "" } else { "s" }))
                        .font(theme::semibold(theme::BODY_SIZE))
                        .color(color::text()),
                );
                ui.label(RichText::new("Shift-click or drag to select more · Esc clears").color(color::text_faint()).size(theme::SMALL_SIZE));
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
                        Source::Commit(_) | Source::Base(_) => {}
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
    /// Row `.2` of a line (more than one when it wraps).
    Line(usize, usize, usize),
    /// Row `.1` of comment block `.0`.
    Comment(usize, usize),
}

fn lines(
    ui: &mut Ui,
    view: &DiffView,
    diff: &FileDiff,
    busy: bool,
    wrap: bool,
    marks: Option<&Marks>,
    actions: &mut Vec<Action>,
) {
    let width = ui.available_width() - ui.spacing().scroll.allocated_width();
    let blocks = blocks(ui, marks, width);
    let text_x = GUTTER + 2.0 * NUM + 16.0;
    let wrap_width = wrap.then_some((width - text_x - 8.0).max(80.0));
    let char_width = char_width(ui);
    let painter = ui.painter().clone();
    let mut rows = Vec::new();
    for (h, hunk) in diff.hunks.iter().enumerate() {
        rows.push(Row::Hunk(h));
        for l in 0..hunk.lines.len() {
            let parts = line_rows(&painter, view, diff, (h, l), wrap_width, char_width);
            rows.extend((0..parts).map(|part| Row::Line(h, l, part)));
            for (b, block) in blocks.iter().enumerate() {
                if block.at == (h, l) {
                    rows.extend((0..block.rows).map(|part| Row::Comment(b, part)));
                }
            }
        }
    }
    let content_width = if wrap {
        width
    } else {
        let longest = diff
            .hunks
            .iter()
            .flat_map(|h| &h.lines)
            .map(|l| l.content.len())
            .max()
            .unwrap_or(0);
        (text_x + longest as f32 * char_width + 40.0).max(width)
    };

    ui.spacing_mut().item_spacing.y = 0.0;
    let scroll = if wrap {
        ScrollArea::vertical()
    } else {
        ScrollArea::both()
    };
    scroll
        .id_salt(("diff-scroll", &view.key.path))
        .auto_shrink([false, false])
        .show_rows(ui, ROW, rows.len(), |ui, range| {
            let p = pointer(ui);
            for row in &rows[range] {
                let (rect, _) = ui.allocate_exact_size(vec2(content_width, ROW), Sense::hover());
                match *row {
                    Row::Hunk(h) => hunk_row(ui, rect, view, diff, h, busy, actions),
                    Row::Comment(b, part) => comment_row(ui, rect, &blocks[b], part, actions),
                    Row::Line(h, l, part) if marks.is_some() => {
                        let marks = marks.expect("checked");
                        let line = &diff.hunks[h].lines[l];
                        let word = view
                            .highlights
                            .words
                            .get(h)
                            .and_then(|w| w.get(l))
                            .cloned()
                            .flatten();
                        let text = LineText {
                            spans: spans_at(view, h, l),
                            word,
                            x: text_x,
                            part,
                            wrap: wrap_width,
                        };
                        paint_line(ui, rect, line, &text, false, false);
                        if marks.pick.is_some_and(|p| p.contains((h, l), line.kind)) {
                            paint_picked(ui, rect);
                        }
                        if ui.rect_contains_pointer(rect) && ui.is_rect_visible(rect) {
                            pick_on_hover(ui, marks, (h, l), None, p, actions);
                        }
                    }
                    Row::Line(h, l, part) => {
                        let line = &diff.hunks[h].lines[l];
                        let selected = view.selection.contains(&(h, l));
                        let word = view
                            .highlights
                            .words
                            .get(h)
                            .and_then(|w| w.get(l))
                            .cloned()
                            .flatten();
                        let editable = !view.read_only;
                        let text = LineText {
                            spans: spans_at(view, h, l),
                            word,
                            x: text_x,
                            part,
                            wrap: wrap_width,
                        };
                        paint_line(ui, rect, line, &text, selected, editable);
                        let hovered = ui.rect_contains_pointer(rect) && ui.is_rect_visible(rect);
                        let selectable = line.is_change() && editable;
                        if selectable && hovered {
                            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                            if p.pressed {
                                actions.push(Action::ClickLine {
                                    hunk: h,
                                    line: l,
                                    shift: p.shift,
                                });
                            } else if p.down && view.anchor.is_some_and(|a| a != (h, l)) {
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
    painter.rect_filled(rect, 0, color::sidebar_bg());
    let header = format!(
        "@@ -{},{} +{},{} @@ {}",
        hunk.old_start, hunk.old_count, hunk.new_start, hunk.new_count, hunk.section
    );
    let galley = painter.layout_no_wrap(header, theme::mono(), color::text_faint());
    paint_galley_left(
        painter,
        rect.left() + GUTTER,
        rect.center().y,
        galley,
        color::text_faint(),
    );

    if view.read_only {
        return;
    }
    // Buttons stick to the visible right edge, not the scrolled content.
    let right = ui.clip_rect().right().min(rect.right()) - 8.0;
    let mut x = right;
    let mut small = |label: &str, action: Action, danger: bool| {
        let text = ui
            .painter()
            .layout_no_wrap(label.to_owned(), theme::small(), color::text());
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
            color::control_hover_bg()
        } else {
            color::control_bg()
        };
        let fg = match (enabled, danger) {
            (false, _) => color::text_faint(),
            (true, true) => color::removed(),
            (true, false) => color::text(),
        };
        ui.painter().rect(
            r,
            4,
            fill,
            Stroke::new(1.0, color::control_border()),
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
        Source::Commit(_) | Source::Base(_) => {}
    }
}

/// What to draw as a line's text: its syntax colors and changed words,
/// where the text starts, and which of its wrapped rows this is.
struct LineText<'a> {
    spans: Option<&'a Spans>,
    word: Option<std::ops::Range<usize>>,
    x: f32,
    part: usize,
    wrap: Option<f32>,
}

/// Row `text.part` of a line. The gutter shows on its first row only; a
/// wrapped line paints all of its text, clipped to this row.
fn paint_line(
    ui: &Ui,
    rect: Rect,
    line: &graft_core::diff::Line,
    text: &LineText,
    selected: bool,
    selectable: bool,
) {
    let painter = ui.painter().with_clip_rect(rect.intersect(ui.clip_rect()));
    let tint = match line.kind {
        Kind::Added => Some(color::added()),
        Kind::Removed => Some(color::removed()),
        Kind::Context => None,
    };
    if let Some(tint) = tint {
        let strength = if selected { 0.22 } else { 0.09 };
        painter.rect_filled(rect, 0, color::app_bg().lerp_to_gamma(tint, strength));
    }
    let y = rect.center().y;
    let first = text.part == 0;

    // Selection box in the gutter.
    if line.is_change() && selectable && first {
        let b = Rect::from_center_size(pos2(rect.left() + GUTTER / 2.0, y), vec2(12.0, 12.0));
        if selected {
            painter.rect_filled(b, 3, color::accent());
            painter.line(
                vec![
                    pos2(b.left() + 2.5, y),
                    pos2(b.left() + 5.0, y + 2.5),
                    pos2(b.right() - 2.5, y - 2.5),
                ],
                Stroke::new(1.6, color::on_accent()),
            );
        } else if ui.rect_contains_pointer(rect) {
            painter.rect_stroke(
                b,
                3,
                Stroke::new(1.0, color::text_weak()),
                StrokeKind::Inside,
            );
        } else {
            painter.rect_stroke(
                b,
                3,
                Stroke::new(1.0, color::control_border()),
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
                color::text_faint(),
            );
        }
    };
    let (marker, fg) = match line.kind {
        Kind::Added => ("+", color::added()),
        Kind::Removed => ("−", color::removed()),
        Kind::Context => ("", color::text_faint()),
    };
    if first {
        num(line.old_no, rect.left() + GUTTER + NUM - 6.0);
        num(line.new_no, rect.left() + GUTTER + 2.0 * NUM - 6.0);
        painter.text(
            pos2(rect.left() + GUTTER + 2.0 * NUM + 4.0, y),
            Align2::LEFT_CENTER,
            marker,
            theme::mono(),
            fg,
        );
    }

    let (shown_text, kept) = shown(line.text());
    let text_color = if line.kind == Kind::Context {
        color::text_mid()
    } else {
        color::text()
    };
    let galley = code_galley(
        &painter,
        shown_text.clone(),
        text.spans,
        text_color,
        text.wrap,
    );
    let origin = match text.wrap {
        Some(_) => pos2(rect.left() + text.x, rect.top() - text.part as f32 * ROW),
        None => pos2(rect.left() + text.x, y - galley.size().y / 2.0),
    };
    if let (Some(range), Some(tint)) = (text.word.clone(), tint)
        && !range.is_empty()
        && range.end <= kept
    {
        paint_word(&painter, &galley, origin, &shown_text, range, tint);
    }
    // The note goes after the end of the text, on its last row.
    let end = galley
        .rows
        .last()
        .map_or(pos2(0.0, galley.size().y / 2.0), |row| {
            row.rect().right_center()
        });
    painter.galley(origin, galley, text_color);
    if line.no_newline {
        painter.text(
            origin + end.to_vec2() + vec2(8.0, 0.0),
            Align2::LEFT_CENTER,
            "no newline at end of file",
            theme::small(),
            color::text_faint(),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_wrapped_pair_takes_the_rows_of_its_longer_side() {
        let line = |kind, text: &str| graft_core::diff::Line {
            kind,
            old_no: None,
            new_no: None,
            content: text.as_bytes().to_vec(),
            no_newline: false,
        };
        let diff = FileDiff {
            hunks: vec![graft_core::diff::Hunk {
                old_start: 1,
                old_count: 2,
                new_start: 1,
                new_count: 2,
                section: String::new(),
                lines: vec![
                    line(Kind::Context, "same"),
                    line(Kind::Removed, "a long old line"),
                    line(Kind::Added, "new"),
                ],
            }],
            ..Default::default()
        };
        let rows = pair_rows(&diff, &[], |(_, l)| if l == 1 { 3 } else { 1 });
        let shape: Vec<String> = rows
            .iter()
            .map(|row| match row {
                Pair::Hunk(h) => format!("hunk {h}"),
                Pair::Lines(old, new, part) => format!("{old:?} {new:?} {part}"),
                Pair::Comment(b, part) => format!("comment {b} {part}"),
            })
            .collect();
        assert_eq!(
            shape,
            [
                "hunk 0",
                "Some((0, 0)) Some((0, 0)) 0",
                "Some((0, 1)) Some((0, 2)) 0",
                "Some((0, 1)) Some((0, 2)) 1",
                "Some((0, 1)) Some((0, 2)) 2",
            ]
        );
    }

    #[test]
    fn long_lines_are_cut_for_display() {
        assert_eq!(shown("short".into()), ("short".to_owned(), 5));
        let long = "é".repeat(MAX_SHOWN_CHARS + 5);
        let (text, kept) = shown(long);
        assert_eq!(kept, MAX_SHOWN_CHARS * 2);
        assert!(text.ends_with("… 5 more characters"), "{text}");
    }
}
