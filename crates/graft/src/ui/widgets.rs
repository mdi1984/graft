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

pub const DIRTY: BadgeColors = BadgeColors {
    bg: color::DIRTY_BG,
    fg: color::DIRTY_FG,
};
pub const AHEAD: BadgeColors = BadgeColors {
    bg: color::AHEAD_BG,
    fg: color::AHEAD_FG,
};
pub const NEUTRAL: BadgeColors = BadgeColors {
    bg: color::NEUTRAL_BADGE_BG,
    fg: color::NEUTRAL_BADGE_FG,
};

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
        color: color::TEXT_WEAK,
        extra_letter_spacing: 0.7,
        ..Default::default()
    };
    let galley = ui
        .painter()
        .layout_job(LayoutJob::single_section(title.to_owned(), format));
    paint_galley_left(painter, x, y, galley, color::TEXT_WEAK);
}

/// Row background: selected, hovered, or nothing.
pub fn paint_row_bg(painter: &Painter, rect: Rect, response: &Response, selected: bool) {
    let fill = if selected {
        color::SELECTED_ROW_BG
    } else if response.hovered() {
        color::HOVER_ROW_BG
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
        Tone::Normal => color::TEXT,
        Tone::Danger => color::SLASH_FG,
    };
    let text = ui
        .painter()
        .layout_no_wrap(label.to_owned(), theme::body(), fg);
    let badge = badge.map(|b| Badge::new(ui, b, NEUTRAL));

    let mut width = 2.0 * PAD_X + text.size().x;
    if icon.is_some() {
        width += ICON_SIZE + GAP;
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
        (Tone::Normal, _, true) => (color::CONTROL_ACTIVE_BG, color::CONTROL_BORDER),
        (Tone::Normal, true, false) => (color::CONTROL_HOVER_BG, color::CONTROL_BORDER),
        (Tone::Normal, false, false) => (color::CONTROL_BG, color::CONTROL_BORDER),
        (Tone::Danger, hovered, pressed) => (
            if hovered || pressed {
                color::SLASH_HOVER_BG
            } else {
                color::SLASH_BG
            },
            color::SLASH_BORDER,
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
        x += ICON_SIZE + GAP;
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
        Stroke::new(1.0, color::CONTROL_BORDER),
    );
}

/// A tab label with an accent underline when selected.
pub fn tab(ui: &mut Ui, label: &str, badge: Option<Badge>, selected: bool) -> Response {
    const PAD_X: f32 = 12.0;
    const HEIGHT: f32 = 38.0;

    let (font, fg) = if selected {
        (theme::semibold(theme::BODY_SIZE), color::TEXT)
    } else {
        (theme::body(), color::TEXT_WEAK)
    };
    let text = ui.painter().layout_no_wrap(label.to_owned(), font, fg);
    let badge_width = badge.as_ref().map_or(0.0, |b| b.width() + 6.0);
    let size = vec2(2.0 * PAD_X + text.size().x + badge_width, HEIGHT);
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());

    let fg = if response.hovered() && !selected {
        color::TEXT
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
        painter.rect_filled(underline, 0.0, color::ACCENT);
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
        color::TEXT_MID,
    );
    painter.text(
        center + vec2(0.0, 12.0),
        Align2::CENTER_CENTER,
        detail,
        theme::body(),
        color::TEXT_WEAK,
    );
}
