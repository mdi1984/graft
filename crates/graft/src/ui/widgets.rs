//! Custom-painted building blocks shared by the toolbar, tabs and sidebar.

use std::sync::Arc;

use egui::text::{LayoutJob, TextFormat};
use egui::{
    Align2, Color32, FontId, Galley, Painter, Pos2, Rect, Response, Sense, Stroke, StrokeKind, Ui,
    pos2, vec2,
};

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
    for skip in 1..parts.len() {
        let candidate = format!("…\\{}", parts[skip..].join("\\"));
        let galley = painter.layout_no_wrap(candidate, font.clone(), color);
        if galley.size().x <= max_width {
            return galley;
        }
    }
    elided(ui, path, font, color, max_width)
}

/// Paint `galley` vertically centered on `y`, starting at `x`.
pub fn paint_galley_left(painter: &Painter, x: f32, y: f32, galley: Arc<Galley>, color: Color32) {
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
    pub bg: Color32,
    pub fg: Color32,
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

/// Small uppercase heading for a sidebar section, e.g. "WORKTREES".
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
    paint_galley_left(painter, x, y, galley, color::text_weak());
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
pub fn tool_button(
    ui: &mut Ui,
    icon: Option<Icon>,
    label: &str,
    badge: Option<String>,
    tone: Tone,
) -> Response {
    const PAD_X: f32 = 10.0;
    const GAP: f32 = 6.0;
    const HEIGHT: f32 = 30.0;

    let fg = match tone {
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

    let (rect, response) = ui.allocate_exact_size(vec2(width, HEIGHT), Sense::click());
    if !ui.is_rect_visible(rect) {
        return response;
    }

    let pressed = response.is_pointer_button_down_on();
    let (fill, border) = match (tone, response.hovered(), pressed) {
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
    paint_galley_left(painter, x, y, text, fg);
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

    let (font, fg) = if selected {
        (theme::semibold(theme::BODY_SIZE), color::text())
    } else {
        (theme::body(), color::text_weak())
    };
    let text = ui.painter().layout_no_wrap(label.to_owned(), font, fg);
    let badge_width = badge.as_ref().map_or(0.0, |b| b.width() + 6.0);
    let size = vec2(2.0 * PAD_X + text.size().x + badge_width, HEIGHT);
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());

    let fg = if response.hovered() && !selected {
        color::text()
    } else {
        fg
    };
    let painter = ui.painter();
    let text_width = text.size().x;
    paint_galley_left(painter, rect.left() + PAD_X, rect.center().y, text, fg);
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

/// Text centered in the available space, for screens that are not built yet.
pub fn empty_state(ui: &mut Ui, title: &str, detail: &str) {
    let rect = ui.available_rect_before_wrap();
    let painter = ui.painter();
    let center = rect.center();
    painter.text(
        center - vec2(0.0, 12.0),
        Align2::CENTER_CENTER,
        title,
        theme::semibold(15.0),
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

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ButtonKind {
    /// Accent-filled: the one main action of a form.
    Primary,
    Secondary,
}

/// A plain text button in Graft's style.
pub fn button(ui: &mut Ui, label: &str, kind: ButtonKind, enabled: bool) -> Response {
    const PAD_X: f32 = 14.0;
    const HEIGHT: f32 = 30.0;
    let font = match kind {
        ButtonKind::Primary => theme::semibold(theme::BODY_SIZE),
        ButtonKind::Secondary => theme::body(),
    };
    // Placeholder color: `galley()` below paints it in the state's color.
    let text = ui
        .painter()
        .layout_no_wrap(label.to_owned(), font, Color32::PLACEHOLDER);
    let size = vec2(text.size().x + 2.0 * PAD_X, HEIGHT);
    let sense = if enabled {
        Sense::click()
    } else {
        Sense::hover()
    };
    let (rect, response) = ui.allocate_exact_size(size, sense);
    let hovered = enabled && response.hovered();
    let (fill, border, fg) = match kind {
        ButtonKind::Primary if !enabled => (
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
    let painter = ui.painter();
    if response.hovered() {
        painter.rect_filled(rect, 4, color::control_hover_bg());
    }
    let fg = if response.hovered() {
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
    let id = ui.next_auto_id();
    let focused = ui.memory(|m| m.has_focus(id));
    let border = if focused {
        color::accent()
    } else {
        color::control_border()
    };
    egui::Frame::new()
        .fill(color::app_bg())
        .stroke(Stroke::new(1.0, border))
        .corner_radius(RADIUS)
        .inner_margin(egui::Margin::symmetric(8, 0))
        .show(ui, |ui| {
            ui.set_width(width - 16.0);
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
        })
        .inner
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
    const PAD_X: f32 = 5.0;
    const HEIGHT: f32 = 17.0;
    let width = (galley.size().x + 2.0 * PAD_X).min(max_width);
    let rect = Rect::from_min_size(pos2(x, y - HEIGHT / 2.0), vec2(width, HEIGHT));
    painter.rect(
        rect,
        4,
        fg.gamma_multiply(0.14),
        Stroke::new(1.0, fg.gamma_multiply(0.35)),
        StrokeKind::Inside,
    );
    let clip = rect.shrink2(vec2(PAD_X - 1.0, 0.0));
    painter.with_clip_rect(clip).galley(
        pos2(rect.left() + PAD_X, y - galley.size().y / 2.0),
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
    ui.ctx().request_repaint();
}

/// A modal dialog with Graft's frame. egui's default frame uses the tight
/// menu padding; dialogs get more room.
pub fn modal(ctx: &egui::Context, id: egui::Id) -> egui::Modal {
    let frame = egui::Frame::popup(&ctx.global_style()).inner_margin(egui::Margin {
        left: 22,
        right: 22,
        top: 18,
        bottom: 20,
    });
    egui::Modal::new(id).frame(frame)
}
