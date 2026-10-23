//! The strip of project tabs at the very top, like browser tabs: one per open
//! repository, a "+" for the start page, drag to reorder, middle-click to close.

use egui::{CornerRadius, Rect, Sense, Stroke, Ui, pos2, vec2};

use super::icons::Icon;
use super::widgets::{elided, paint_galley_left, paint_icon_button, spinner};
use crate::theme::{self, color};

pub const HEIGHT: f32 = 40.0;
const TAB_HEIGHT: f32 = 32.0;
const MIN_WIDTH: f32 = 110.0;
const MAX_WIDTH: f32 = 220.0;
const PAD_X: f32 = 12.0;
const CLOSE: f32 = 18.0;

pub struct TabInfo {
    pub name: String,
    pub tooltip: String,
    pub dirty: bool,
    pub busy: bool,
    pub error: bool,
}

pub enum TabAction {
    Activate(usize),
    Close(usize),
    Move {
        from: usize,
        to: usize,
    },
    /// Show the start page.
    NewTab,
    /// Close the start page tab.
    CloseStart,
}

/// `active` is `None` while the start page is showing; it then gets a tab too.
pub fn show(ui: &mut Ui, tabs: &[TabInfo], active: Option<usize>) -> Option<TabAction> {
    let mut action = None;
    let full = ui.available_rect_before_wrap();
    let bottom = full.top() + HEIGHT;
    let top = bottom - TAB_HEIGHT;
    ui.allocate_rect(
        Rect::from_min_max(full.min, pos2(full.right(), bottom)),
        Sense::hover(),
    );

    let count = tabs.len() + usize::from(active.is_none());
    let room = full.width() - 16.0 - 40.0;
    let width = (room / count.max(1) as f32).clamp(MIN_WIDTH, MAX_WIDTH);

    let mut x = full.left() + 8.0;
    let mut rects = Vec::with_capacity(tabs.len());
    let mut dragged = None;
    for (i, tab) in tabs.iter().enumerate() {
        let rect = Rect::from_min_size(pos2(x, top), vec2(width, TAB_HEIGHT));
        rects.push(rect);
        let id = ui.scope_id().with(("project-tab", i));
        let response = ui
            .interact(rect, id, Sense::click_and_drag())
            .on_hover_text(&tab.tooltip);
        let close_rect = close_rect(rect);
        let close = ui.interact(close_rect, id.with("close"), Sense::click());
        let selected = active == Some(i);

        paint_tab(ui, rect, selected, response.hovered() || close.hovered());
        let y = rect.center().y;
        let mut text_x = rect.left() + PAD_X;
        if tab.busy {
            spinner(ui, pos2(text_x + 4.0, y), 4.0, color::TEXT_WEAK);
            text_x += 14.0;
        } else if tab.error {
            ui.painter()
                .circle_filled(pos2(text_x + 3.0, y), 3.5, color::REMOVED);
            text_x += 13.0;
        } else if tab.dirty {
            ui.painter()
                .circle_filled(pos2(text_x + 3.0, y), 3.5, color::DIRTY_FG);
            text_x += 13.0;
        }
        let show_close = selected || response.hovered() || close.hovered();
        let text_right = if show_close {
            close_rect.left() - 4.0
        } else {
            rect.right() - PAD_X
        };
        let (font, fg) = if selected {
            (theme::semibold(theme::BODY_SIZE), color::TEXT)
        } else {
            (theme::body(), color::TEXT_WEAK)
        };
        let name = elided(ui, &tab.name, font, fg, text_right - text_x);
        paint_galley_left(ui.painter(), text_x, y, name, fg);
        if show_close {
            paint_icon_button(ui, close_rect, &close, Icon::Close);
        }

        if close.clicked() || response.middle_clicked() {
            action = Some(TabAction::Close(i));
        } else if (response.clicked() || response.drag_started()) && active != Some(i) {
            action = Some(TabAction::Activate(i));
        }
        if response.dragged() {
            dragged = Some(i);
        }
        x += width + 2.0;
    }

    // Reorder: swap with a neighbour once the pointer passes its middle.
    if let (Some(i), Some(pointer)) = (dragged, ui.ctx().pointer_interact_pos()) {
        if i + 1 < rects.len() && pointer.x > rects[i + 1].center().x {
            action = Some(TabAction::Move { from: i, to: i + 1 });
        } else if i > 0 && pointer.x < rects[i - 1].center().x {
            action = Some(TabAction::Move { from: i, to: i - 1 });
        }
    }

    if active.is_none() {
        let rect = Rect::from_min_size(pos2(x, top), vec2(width, TAB_HEIGHT));
        let id = ui.scope_id().with("start-tab");
        let close_rect = close_rect(rect);
        let close = ui.interact(close_rect, id.with("close"), Sense::click());
        paint_tab(ui, rect, true, false);
        let label = if tabs.is_empty() {
            "Welcome"
        } else {
            "New tab"
        };
        let galley = elided(
            ui,
            label,
            theme::semibold(theme::BODY_SIZE),
            color::TEXT,
            close_rect.left() - rect.left() - PAD_X,
        );
        paint_galley_left(
            ui.painter(),
            rect.left() + PAD_X,
            rect.center().y,
            galley,
            color::TEXT,
        );
        if !tabs.is_empty() {
            paint_icon_button(ui, close_rect, &close, Icon::Close);
            if close.clicked() {
                action = Some(TabAction::CloseStart);
            }
        }
        x += width + 2.0;
    }

    let plus = Rect::from_center_size(pos2(x + 16.0, top + TAB_HEIGHT / 2.0), vec2(26.0, 26.0));
    let response = ui
        .interact(plus, ui.scope_id().with("new-tab"), Sense::click())
        .on_hover_text("Open a repository  (Ctrl+T)");
    paint_icon_button(ui, plus, &response, Icon::Plus);
    if response.clicked() {
        action = Some(TabAction::NewTab);
    }
    action
}

fn close_rect(tab: Rect) -> Rect {
    Rect::from_center_size(
        pos2(tab.right() - 8.0 - CLOSE / 2.0, tab.center().y),
        vec2(CLOSE, CLOSE),
    )
}

fn paint_tab(ui: &Ui, rect: Rect, selected: bool, hovered: bool) {
    let painter = ui.painter();
    let radius = CornerRadius {
        nw: 7,
        ne: 7,
        sw: 0,
        se: 0,
    };
    if selected {
        painter.rect_filled(rect, radius, color::BAR_BG);
        painter.hline(
            rect.x_range(),
            rect.top() + 0.5,
            Stroke::new(1.0, color::ACCENT.gamma_multiply(0.6)),
        );
    } else if hovered {
        painter.rect_filled(rect, radius, color::HOVER_ROW_BG);
    }
}
