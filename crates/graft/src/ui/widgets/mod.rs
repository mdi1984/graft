//! Custom-painted building blocks shared by every screen.

pub mod dialog;
mod form;

use std::sync::Arc;

use egui::text::{LayoutJob, TextFormat};
use egui::{
    Align2, Color32, FontId, Galley, Painter, Pos2, Rect, Response, Sense, Stroke, StrokeKind, Ui,
    Vec2, pos2, vec2,
};

pub use form::*;

use super::icons::{self, Icon};
use crate::theme::{self, RADIUS, color};

pub const ICON_SIZE: f32 = 14.0;

/// Lay out `text` on one line, cut with "…" if wider than `max_width`.
pub fn elided(ui: &Ui, text: &str, font: FontId, color: Color32, max_width: f32) -> Arc<Galley> {
    let mut job = LayoutJob::simple_singleline(text.to_owned(), font, color);
    job.wrap.max_width = max_width.max(0.0);
    job.wrap.max_rows = 1;
    job.wrap.break_anywhere = true;
    ui.painter().layout_job(job)
}

/// Like [`elided`], but for paths: drops leading folders first, so the end
/// of the path (the part that tells folders apart) stays readable.
pub fn elided_path(
    ui: &Ui,
    path: &str,
    font: FontId,
    color: Color32,
    max_width: f32,
) -> Arc<Galley> {
    let painter = ui.painter();
    let full = painter.layout_no_wrap(path.to_owned(), font.clone(), color);
    if full.size().x <= max_width {
        return full;
    }
    let parts: Vec<&str> = path.split(['\\', '/']).collect();
    // Linux paths (WSL) keep their slashes.
    let sep = if path.contains('\\') { "\\" } else { "/" };
    for skip in 1..parts.len() {
        let candidate = format!("…{sep}{}", parts[skip..].join(sep));
        let galley = painter.layout_no_wrap(candidate, font.clone(), color);
        if galley.size().x <= max_width {
            return galley;
        }
    }
    elided(ui, path, font, color, max_width)
}

/// Paint `galley` vertically centered on `y`, starting at `x`.
pub fn paint_galley_left(painter: &Painter, x: f32, y: f32, galley: Arc<Galley>) {
    paint_placeholder_left(painter, x, y, galley, color::text());
}

/// Like [`paint_galley_left`], for a galley laid out in
/// `Color32::PLACEHOLDER`: it gets `color` now, e.g. the hover color.
pub fn paint_placeholder_left(
    painter: &Painter,
    x: f32,
    y: f32,
    galley: Arc<Galley>,
    color: Color32,
) {
    let pos = pos2(x, y - galley.size().y / 2.0);
    painter.galley(pos, galley, color);
}

pub fn paint_icon(painter: &Painter, left_center: Pos2, icon: Icon, color: Color32) {
    let rect = Rect::from_min_size(
        pos2(left_center.x, left_center.y - ICON_SIZE / 2.0),
        vec2(ICON_SIZE, ICON_SIZE),
    );
    icons::paint(painter, rect, icon, color);
}

#[derive(Clone, Copy)]
pub struct BadgeColors {
    bg: Color32,
    fg: Color32,
}

pub fn dirty() -> BadgeColors {
    BadgeColors {
        bg: color::dirty_bg(),
        fg: color::dirty_fg(),
    }
}

pub fn ahead() -> BadgeColors {
    BadgeColors {
        bg: color::ahead_bg(),
        fg: color::ahead_fg(),
    }
}

pub fn neutral() -> BadgeColors {
    BadgeColors {
        bg: color::neutral_badge_bg(),
        fg: color::neutral_badge_fg(),
    }
}

/// A pill with a short count, e.g. a dirty-file count. Laid out but not painted.
pub struct Badge {
    galley: Arc<Galley>,
    colors: BadgeColors,
}

impl Badge {
    const PAD_X: f32 = 6.0;
    const HEIGHT: f32 = 16.0;

    pub fn new(ui: &Ui, text: impl Into<String>, colors: BadgeColors) -> Self {
        let galley = ui
            .painter()
            .layout_no_wrap(text.into(), theme::small(), colors.fg);
        Self { galley, colors }
    }

    pub fn width(&self) -> f32 {
        (self.galley.size().x + 2.0 * Self::PAD_X).max(Self::HEIGHT)
    }

    /// Paint with the badge's left edge at `x`, vertically centered on `y`.
    pub fn paint(self, painter: &Painter, x: f32, y: f32) {
        let rect = Rect::from_min_size(
            pos2(x, y - Self::HEIGHT / 2.0),
            vec2(self.width(), Self::HEIGHT),
        );
        painter.rect_filled(rect, Self::HEIGHT / 2.0, self.colors.bg);
        painter.galley(
            rect.center() - self.galley.size() / 2.0,
            self.galley,
            self.colors.fg,
        );
    }
}

/// Paint badges right-aligned against `right`, returning the x where they start.
pub fn paint_badges_right(painter: &Painter, right: f32, y: f32, badges: Vec<Badge>) -> f32 {
    let mut x = right;
    for badge in badges.into_iter().rev() {
        x -= badge.width();
        badge.paint(painter, x, y);
        x -= 4.0;
    }
    x
}

/// Small letter-spaced heading over a section, painted at `x`, centered on
/// `y`. `title` comes in capitals: "WORKTREES", "FILES  12".
pub fn section_heading(ui: &Ui, painter: &Painter, x: f32, y: f32, title: &str) {
    let format = TextFormat {
        font_id: theme::semibold(theme::SMALL_SIZE),
        color: color::text_weak(),
        extra_letter_spacing: 0.7,
        ..Default::default()
    };
    let galley = ui
        .painter()
        .layout_job(LayoutJob::single_section(title.to_owned(), format));
    paint_galley_left(painter, x, y, galley);
}

/// A checkbox square: accent-filled with a tick when `checked`, else an
/// outline that darkens on hover.
pub fn paint_check(painter: &Painter, rect: Rect, checked: bool, hovered: bool) {
    if checked {
        painter.rect_filled(rect, 3, color::accent());
        // The tick scales with the box: 12 px gets a 2.5 px inset.
        let inset = rect.width() * 0.21;
        let y = rect.center().y;
        painter.line(
            vec![
                pos2(rect.left() + inset, y),
                pos2(rect.left() + 2.0 * inset, y + inset),
                pos2(rect.right() - inset, y - inset),
            ],
            Stroke::new(1.6, color::on_accent()),
        );
    } else {
        let border = if hovered {
            color::text_weak()
        } else {
            color::control_border()
        };
        painter.rect_stroke(rect, 3, Stroke::new(1.0, border), StrokeKind::Inside);
    }
}

/// A [`section_heading`] on a row of its own, in a laid-out panel.
pub fn section_heading_row(ui: &mut Ui, title: &str) {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 16.0), Sense::hover());
    section_heading(ui, ui.painter(), rect.left(), rect.center().y, title);
}

/// A folder that folds what is inside it: open until folded, and it stays
/// the way it was left (under `id`). `count` shows at the right; `marked`
/// highlights it while folded (it hides the current branch or open file);
/// `always_open` shows it open and ignores clicks (while a filter is on).
/// True while open.
#[allow(clippy::too_many_arguments)]
pub fn folder_row(
    ui: &mut Ui,
    id: egui::Id,
    name: &str,
    font: FontId,
    count: &str,
    indent: f32,
    marked: bool,
    always_open: bool,
) -> bool {
    let saved = ui.data_mut(|d| *d.get_persisted_mut_or(id, true));
    let mut open = saved || always_open;
    let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width(), 24.0), Sense::click());
    let painter = ui.painter();
    paint_row_bg(painter, rect, &response, marked && !open);
    let y = rect.center().y;
    let chevron = if open {
        Icon::ChevronDown
    } else {
        Icon::ChevronRight
    };
    let chevron_rect =
        Rect::from_center_size(pos2(rect.left() + indent + 7.0, y), vec2(10.0, 10.0));
    icons::paint(painter, chevron_rect, chevron, color::text_faint());
    let folder_rect =
        Rect::from_center_size(pos2(rect.left() + indent + 21.0, y), vec2(12.0, 12.0));
    icons::paint(painter, folder_rect, Icon::Folder, color::text_weak());

    let count = painter.layout_no_wrap(count.to_owned(), theme::small(), color::text_faint());
    let count_x = rect.right() - 8.0 - count.size().x;
    paint_galley_left(painter, count_x, y, count);
    let x = rect.left() + indent + 32.0;
    let label = elided(ui, name, font, color::text_mid(), count_x - 6.0 - x);
    paint_galley_left(ui.painter(), x, y, label);
    if response.clicked() && !always_open {
        open = !open;
        ui.data_mut(|d| d.insert_persisted(id, open));
    }
    open
}

/// A list's section heading ("STAGED  3") with an optional link at the
/// right (`text`, hover `tip`); true when the link was clicked while
/// `can` allows it.
pub fn section_with_link(
    ui: &mut Ui,
    heading: &str,
    link: Option<(&str, &str)>,
    can: &super::can::Can,
) -> bool {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 24.0), Sense::hover());
    let y = rect.center().y;
    section_heading(ui, ui.painter(), rect.left() + 4.0, y, heading);
    let Some((link, tip)) = link else {
        return false;
    };
    let galley = ui
        .painter()
        .layout_no_wrap(link.to_owned(), theme::small(), Color32::PLACEHOLDER);
    let r = Rect::from_min_size(
        pos2(rect.right() - galley.size().x - 8.0, rect.top() + 3.0),
        vec2(galley.size().x + 6.0, 18.0),
    );
    let response = ui
        .interact(
            r,
            ui.scope_id().with(("section-link", heading)),
            Sense::click(),
        )
        .on_hover_text(can.blocked().unwrap_or(tip));
    let fg = if !can.ok {
        color::text_faint()
    } else if response.hovered() {
        color::text()
    } else {
        color::ahead_fg()
    };
    ui.painter()
        .galley(pos2(r.left() + 3.0, y - galley.size().y / 2.0), galley, fg);
    response.clicked() && can.ok
}

/// Height of a [`two_line_row`].
pub const TWO_LINE_HEIGHT: f32 = 46.0;

/// A clickable list row the full width: `title` above, the smaller `meta`
/// below. Its background is inset by `inset` from the row's edges.
pub fn two_line_row(ui: &mut Ui, inset: Vec2, selected: bool, title: &str, meta: &str) -> Response {
    const PAD_X: f32 = 10.0;
    let (rect, response) =
        ui.allocate_exact_size(vec2(ui.available_width(), TWO_LINE_HEIGHT), Sense::click());
    let bg = rect.shrink2(inset);
    paint_row_bg(ui.painter(), bg, &response, selected);
    let x = bg.left() + PAD_X;
    let width = bg.width() - 2.0 * PAD_X;
    let title = elided(ui, title, theme::body(), color::text(), width);
    paint_galley_left(ui.painter(), x, rect.top() + 15.0, title);
    let meta = elided(ui, meta, theme::small(), color::text_weak(), width);
    paint_galley_left(ui.painter(), x, rect.top() + 33.0, meta);
    response
}

/// Row background: selected, hovered, or nothing.
pub fn paint_row_bg(painter: &Painter, rect: Rect, response: &Response, selected: bool) {
    let fill = if selected {
        color::selected_row_bg()
    } else if response.hovered() {
        color::hover_row_bg()
    } else {
        return;
    };
    painter.rect_filled(rect, RADIUS, fill);
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    Normal,
    /// Orange outline for destructive tools (Slash).
    Danger,
}

/// A bordered toolbar button: optional icon, label, optional count badge.
/// A disabled button is drawn faint and never reports a click, but still
/// shows its hover text (the reason it is disabled).
pub fn tool_button(
    ui: &mut Ui,
    icon: Option<Icon>,
    label: &str,
    badge: Option<String>,
    tone: Tone,
    enabled: bool,
) -> Response {
    const PAD_X: f32 = 10.0;
    const GAP: f32 = 6.0;
    const HEIGHT: f32 = 30.0;

    let fg = match tone {
        _ if !enabled => color::text_faint(),
        Tone::Normal => color::text(),
        Tone::Danger => color::slash_fg(),
    };
    let text = ui
        .painter()
        .layout_no_wrap(label.to_owned(), theme::body(), fg);
    let badge = badge.map(|b| Badge::new(ui, b, neutral()));

    // An empty label makes an icon-only button (compact toolbar).
    let icon_gap = if label.is_empty() { 0.0 } else { GAP };
    let mut width = 2.0 * PAD_X + text.size().x;
    if icon.is_some() {
        width += ICON_SIZE + icon_gap;
    }
    if let Some(badge) = &badge {
        width += GAP + badge.width();
    }

    let sense = if enabled {
        Sense::click()
    } else {
        Sense::hover()
    };
    let (rect, response) = ui.allocate_exact_size(vec2(width, HEIGHT), sense);
    if !ui.is_rect_visible(rect) {
        return response;
    }

    let pressed = response.is_pointer_button_down_on();
    let (fill, border) = match (tone, enabled && response.hovered(), pressed) {
        _ if !enabled => (color::bar_bg(), color::border()),
        (Tone::Normal, _, true) => (color::control_active_bg(), color::control_border()),
        (Tone::Normal, true, false) => (color::control_hover_bg(), color::control_border()),
        (Tone::Normal, false, false) => (color::control_bg(), color::control_border()),
        (Tone::Danger, hovered, pressed) => (
            if hovered || pressed {
                color::slash_hover_bg()
            } else {
                color::slash_bg()
            },
            color::slash_border(),
        ),
    };
    let painter = ui.painter();
    painter.rect(
        rect,
        RADIUS,
        fill,
        Stroke::new(1.0, border),
        StrokeKind::Inside,
    );

    let y = rect.center().y;
    let mut x = rect.left() + PAD_X;
    if let Some(icon) = icon {
        paint_icon(painter, pos2(x, y), icon, fg);
        x += ICON_SIZE + icon_gap;
    }
    let text_width = text.size().x;
    paint_galley_left(painter, x, y, text);
    x += text_width;
    if let Some(badge) = badge {
        badge.paint(painter, x + GAP, y);
    }
    response
}

/// A thin vertical divider between toolbar groups.
pub fn toolbar_divider(ui: &mut Ui) {
    let (rect, _) = ui.allocate_exact_size(vec2(9.0, 20.0), Sense::hover());
    ui.painter().vline(
        rect.center().x,
        rect.y_range(),
        Stroke::new(1.0, color::control_border()),
    );
}

/// A tab label with an accent underline when selected.
pub fn tab(ui: &mut Ui, label: &str, badge: Option<Badge>, selected: bool) -> Response {
    const PAD_X: f32 = 12.0;
    const HEIGHT: f32 = 38.0;

    let font = if selected {
        theme::semibold(theme::BODY_SIZE)
    } else {
        theme::body()
    };
    let text = ui
        .painter()
        .layout_no_wrap(label.to_owned(), font, Color32::PLACEHOLDER);
    let badge_width = badge.as_ref().map_or(0.0, |b| b.width() + 6.0);
    let size = vec2(2.0 * PAD_X + text.size().x + badge_width, HEIGHT);
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());

    let fg = if selected || response.hovered() {
        color::text()
    } else {
        color::text_weak()
    };
    let painter = ui.painter();
    let text_width = text.size().x;
    paint_placeholder_left(painter, rect.left() + PAD_X, rect.center().y, text, fg);
    if let Some(badge) = badge {
        badge.paint(
            painter,
            rect.left() + PAD_X + text_width + 6.0,
            rect.center().y,
        );
    }
    if selected {
        let underline =
            Rect::from_min_max(pos2(rect.left(), rect.bottom() - 2.0), rect.right_bottom());
        painter.rect_filled(underline, 0.0, color::accent());
    }
    response
}

/// A title and a line of detail centered in the available space, for a
/// screen with nothing to show (nothing selected, nothing loaded, a
/// failure).
pub fn empty_state(ui: &mut Ui, title: &str, detail: &str) {
    let rect = ui.available_rect_before_wrap();
    let painter = ui.painter();
    let center = rect.center();
    painter.text(
        center - vec2(0.0, 12.0),
        Align2::CENTER_CENTER,
        title,
        theme::title(),
        color::text_mid(),
    );
    painter.text(
        center + vec2(0.0, 12.0),
        Align2::CENTER_CENTER,
        detail,
        theme::body(),
        color::text_weak(),
    );
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ButtonKind {
    /// Accent-filled: the one main action of a form.
    Primary,
    Secondary,
    /// Red-filled: the main action of a question before something destructive.
    Danger,
}

const BUTTON_PAD_X: f32 = 14.0;
const BUTTON_HEIGHT: f32 = 30.0;

fn button_galley(ui: &Ui, label: &str, kind: ButtonKind) -> Arc<Galley> {
    let font = match kind {
        ButtonKind::Primary | ButtonKind::Danger => theme::semibold(theme::BODY_SIZE),
        ButtonKind::Secondary => theme::body(),
    };
    ui.painter()
        .layout_no_wrap(label.to_owned(), font, Color32::PLACEHOLDER)
}

/// How wide [`button`] makes a button, e.g. to center a row of them.
pub fn button_width(ui: &Ui, label: &str, kind: ButtonKind) -> f32 {
    button_galley(ui, label, kind).size().x + 2.0 * BUTTON_PAD_X
}

/// A plain text button in Graft's style.
pub fn button(ui: &mut Ui, label: &str, kind: ButtonKind, enabled: bool) -> Response {
    // Placeholder color: `galley()` below paints it in the state's color.
    let text = button_galley(ui, label, kind);
    let size = vec2(text.size().x + 2.0 * BUTTON_PAD_X, BUTTON_HEIGHT);
    let sense = if enabled {
        Sense::click()
    } else {
        Sense::hover()
    };
    let (rect, response) = ui.allocate_exact_size(size, sense);
    let hovered = enabled && response.hovered();
    let (fill, border, fg) = match kind {
        ButtonKind::Primary | ButtonKind::Danger if !enabled => (
            color::control_bg(),
            color::control_border(),
            color::text_faint(),
        ),
        ButtonKind::Primary => (
            if hovered {
                color::accent_hover()
            } else {
                color::accent()
            },
            Color32::TRANSPARENT,
            color::on_accent(),
        ),
        ButtonKind::Danger => (
            if hovered {
                color::removed().gamma_multiply(1.1)
            } else {
                color::removed()
            },
            Color32::TRANSPARENT,
            color::on_accent(),
        ),
        ButtonKind::Secondary => (
            if hovered {
                color::control_hover_bg()
            } else {
                color::control_bg()
            },
            color::control_border(),
            if enabled {
                color::text()
            } else {
                color::text_faint()
            },
        ),
    };
    let painter = ui.painter();
    painter.rect(
        rect,
        RADIUS,
        fill,
        Stroke::new(1.0, border),
        StrokeKind::Inside,
    );
    painter.galley(rect.center() - text.size() / 2.0, text, fg);
    response
}

/// A borderless square button with just an icon, e.g. a close "×".
pub fn icon_button(ui: &mut Ui, icon: Icon, size: f32) -> Response {
    let (rect, response) = ui.allocate_exact_size(vec2(size, size), Sense::click());
    paint_icon_button(ui, rect, &response, icon);
    response
}

/// Paint an icon button into a rect that was already interacted with.
pub fn paint_icon_button(ui: &Ui, rect: Rect, response: &Response, icon: Icon) {
    paint_icon_button_when(ui, rect, response, icon, true);
}

/// Like [`paint_icon_button`]; a button that cannot act now (`enabled`
/// false) is drawn faint, without a hover fill.
pub fn paint_icon_button_when(ui: &Ui, rect: Rect, response: &Response, icon: Icon, enabled: bool) {
    let painter = ui.painter();
    let hovered = enabled && response.hovered();
    if hovered {
        painter.rect_filled(rect, 4, color::control_hover_bg());
    }
    let fg = if !enabled {
        color::text_faint()
    } else if hovered {
        color::text()
    } else {
        color::text_weak()
    };
    let icon_size = (rect.width() * 0.6).min(ICON_SIZE);
    let icon_rect = Rect::from_center_size(rect.center(), vec2(icon_size, icon_size));
    icons::paint(painter, icon_rect, icon, fg);
}

/// A single-line text input with Graft's frame. Returns the text edit's response.
pub fn text_field(ui: &mut Ui, text: &mut String, hint: &str, width: f32) -> Response {
    framed_field(ui, text, hint, width, false).0
}

/// Like [`text_field`], with a "×" inside its right end while it has
/// text. Returns the text edit's response and whether "×" was clicked.
pub fn clearable_field(ui: &mut Ui, text: &mut String, hint: &str, width: f32) -> (Response, bool) {
    framed_field(ui, text, hint, width, true)
}

/// Room for the "×" of a [`clearable_field`].
const CLEAR_SIZE: f32 = 22.0;

/// A field exactly `width` wide, border included.
fn framed_field(
    ui: &mut Ui,
    text: &mut String,
    hint: &str,
    width: f32,
    clearable: bool,
) -> (Response, bool) {
    const PAD_X: i8 = 8;
    const BORDER: f32 = 1.0;
    let id = ui.next_auto_id();
    let focused = ui.memory(|m| m.has_focus(id));
    let border = if focused {
        color::accent()
    } else {
        color::control_border()
    };
    // The "×" sits in the right margin, so text never runs under it.
    let right = if clearable {
        PAD_X / 2 + CLEAR_SIZE as i8
    } else {
        PAD_X
    };
    let margin = egui::Margin {
        left: PAD_X,
        right,
        top: 0,
        bottom: 0,
    };
    let frame = egui::Frame::new()
        .fill(color::app_bg())
        .stroke(Stroke::new(BORDER, border))
        .corner_radius(RADIUS)
        .inner_margin(margin)
        .show(ui, |ui| {
            // The border counts too: a field wider than `width` would make
            // a panel that fits its content grow every frame.
            ui.set_width(width - margin.sum().x - 2.0 * BORDER);
            ui.add(
                egui::TextEdit::singleline(text)
                    .id(id)
                    .frame(egui::Frame::NONE)
                    .margin(egui::Margin::ZERO)
                    .min_size(vec2(0.0, 30.0))
                    .align(Align2::LEFT_CENTER)
                    .hint_text(egui::RichText::new(hint).color(color::text_faint()))
                    .desired_width(f32::INFINITY),
            )
        });
    let mut cleared = false;
    if clearable && !text.is_empty() {
        let outer = frame.response.rect;
        let center = pos2(
            outer.right() - BORDER - 2.0 - CLEAR_SIZE / 2.0,
            outer.center().y,
        );
        let rect = Rect::from_center_size(center, vec2(CLEAR_SIZE, CLEAR_SIZE));
        let clear = ui.interact(rect, id.with("clear"), Sense::click());
        paint_icon_button(ui, rect, &clear, Icon::Close);
        cleared = clear.on_hover_text("Clear").clicked();
    }
    (frame.inner, cleared)
}

const CHIP_PAD_X: f32 = 5.0;
const CHIP_HEIGHT: f32 = 17.0;

/// The size of a [`chip`] around `galley`, when it has room.
pub fn chip_size(galley: &Galley) -> Vec2 {
    vec2(galley.size().x + 2.0 * CHIP_PAD_X, CHIP_HEIGHT)
}

/// A rounded label like a ref name on a commit. Returns its width.
pub fn chip(
    painter: &Painter,
    x: f32,
    y: f32,
    galley: Arc<Galley>,
    fg: Color32,
    max_width: f32,
) -> f32 {
    let width = chip_size(&galley).x.min(max_width);
    let rect = Rect::from_min_size(pos2(x, y - CHIP_HEIGHT / 2.0), vec2(width, CHIP_HEIGHT));
    painter.rect(
        rect,
        4,
        fg.gamma_multiply(0.14),
        Stroke::new(1.0, fg.gamma_multiply(0.35)),
        StrokeKind::Inside,
    );
    let clip = rect.shrink2(vec2(CHIP_PAD_X - 1.0, 0.0));
    painter.with_clip_rect(clip).galley(
        pos2(rect.left() + CHIP_PAD_X, y - galley.size().y / 2.0),
        galley,
        fg,
    );
    width
}

/// A rotating arc, for work in progress. Keeps the UI repainting while shown.
pub fn spinner(ui: &Ui, center: Pos2, radius: f32, color: Color32) {
    let t = ui.input(|i| i.time) as f32;
    let start = t * 5.0;
    let points: Vec<Pos2> = (0..=20)
        .map(|i| start + i as f32 / 20.0 * 4.2)
        .map(|a| center + radius * vec2(a.cos(), a.sin()))
        .collect();
    ui.painter()
        .add(egui::Shape::line(points, Stroke::new(1.6, color)));
    ui.ctx().request_repaint();
}

/// Whether a row whose content was measured under `id` (see
/// [`remember_width`]) fits into `width`; true until it was measured.
pub fn fits(ui: &Ui, id: egui::Id, width: f32) -> bool {
    ui.data(|d| d.get_temp::<f32>(id))
        .is_none_or(|needed| width >= needed)
}

/// Remember how wide a row's content was, for [`fits`] on the next frame.
pub fn remember_width(ui: &Ui, id: egui::Id, width: f32) {
    ui.data_mut(|d| d.insert_temp(id, width));
}

/// A thin progress bar; without a fraction it shows a moving segment.
pub fn progress_bar(ui: &mut Ui, fraction: Option<f32>, width: f32) {
    let (rect, _) = ui.allocate_exact_size(vec2(width, 4.0), Sense::hover());
    let painter = ui.painter();
    painter.rect_filled(rect, 2, color::control_bg());
    let fill = match fraction {
        Some(f) => Rect::from_min_size(
            rect.min,
            vec2(rect.width() * f.clamp(0.0, 1.0), rect.height()),
        ),
        None => {
            let t = (ui.input(|i| i.time) as f32 * 0.6).fract();
            let seg = rect.width() * 0.3;
            let x = rect.left() - seg + t * (rect.width() + seg);
            Rect::from_min_max(
                pos2(x.max(rect.left()), rect.top()),
                pos2((x + seg).min(rect.right()), rect.bottom()),
            )
        }
    };
    painter.rect_filled(fill, 2, color::accent());
    // Only the sliding bar moves on its own.
    if fraction.is_none() {
        ui.ctx().request_repaint();
    }
}

/// A row of the group colors; returns the one clicked.
pub fn color_swatches(ui: &mut Ui, current: usize) -> Option<usize> {
    let mut picked = None;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 2.0;
        for c in 0..color::GROUP_COUNT {
            let fill = color::group(c);
            let (rect, response) = ui.allocate_exact_size(vec2(20.0, 20.0), Sense::click());
            let painter = ui.painter();
            painter.circle_filled(rect.center(), 6.0, fill);
            if c == current || response.hovered() {
                painter.circle_stroke(rect.center(), 8.5, Stroke::new(1.5, color::text_mid()));
            }
            if response.clicked() {
                picked = Some(c);
            }
        }
    });
    picked
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Fields are exactly as wide as asked, border included: a wider one
    /// made the sidebar (a panel that fits its content) grow every frame.
    #[test]
    fn text_fields_are_exactly_as_wide_as_asked() {
        let ctx = egui::Context::default();
        theme::apply(&ctx, egui::ThemePreference::Dark);
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(800.0, 400.0))),
            ..Default::default()
        };
        let mut widths = Vec::new();
        let output = ctx.run_ui(input, |ui| {
            let mut empty = String::new();
            let mut typed = "login".to_owned();
            let field = ui.scope(|ui| text_field(ui, &mut empty, "Filter", 200.0));
            widths.push(field.response.rect.width());
            let field = ui.scope(|ui| clearable_field(ui, &mut typed, "Filter", 200.0));
            widths.push(field.response.rect.width());
        });
        output.drop_without_applying_deltas();
        assert_eq!(widths, [200.0, 200.0]);
    }
}
