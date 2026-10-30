//! The strip of project tabs at the very top, like browser tabs: one per open
//! repository, a "+" for the start page, drag to reorder, middle-click to close.
//! When the tabs don't fit they scroll sideways (wheel, or the arrows at the
//! ends), and the "All tabs" list next to "+" finds any of them.
//! It is also the window's title bar (see `window_frame`).

use egui::{
    Area, CornerRadius, Frame, Key, Margin, Modifiers, Order, Rect, Response, ScrollArea, Sense,
    Stroke, Ui, UiBuilder, pos2, vec2,
};

use super::icons::Icon;
use super::widgets::{
    elided, elided_path, paint_galley_left, paint_icon_button, spinner, text_field,
};
use super::window_frame;
use crate::keys::{Command, Keymap};
use crate::theme::{self, RADIUS, color};

pub const HEIGHT: f32 = 40.0;
const TAB_HEIGHT: f32 = 32.0;
const MIN_WIDTH: f32 = 110.0;
const MAX_WIDTH: f32 = 220.0;
const GAP: f32 = 2.0;
const PAD_X: f32 = 12.0;
const CLOSE: f32 = 18.0;
/// The app icon at the left end of the strip.
const ICON: f32 = 18.0;
/// "+" and "All tabs".
const BUTTON: f32 = 26.0;
/// The arrows at the ends of a strip that scrolls.
const ARROW: f32 = 22.0;
const LIST_WIDTH: f32 = 380.0;
const ROW_HEIGHT: f32 = 40.0;

pub struct TabInfo {
    pub name: String,
    /// The repository's folder.
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
    OpenSettings,
    OpenAbout,
}

/// `active` is `None` while the start page is showing; it then gets a tab too.
/// `update` is a newer Graft's version, offered next to the settings button.
pub fn show(
    ui: &mut Ui,
    tabs: &[TabInfo],
    active: Option<usize>,
    update: Option<&str>,
    keys: &Keymap,
) -> Option<TabAction> {
    let mut action = None;
    let full = ui.available_rect_before_wrap();
    let bottom = full.top() + HEIGHT;
    let top = bottom - TAB_HEIGHT;
    let mid = top + TAB_HEIGHT / 2.0;
    let strip = Rect::from_min_max(full.min, pos2(full.right(), bottom));
    ui.allocate_rect(strip, Sense::hover());
    window_frame::drag_area(ui, strip);

    let icon = Rect::from_center_size(pos2(full.left() + 22.0, mid), vec2(ICON, ICON));
    window_frame::paint_app_icon(ui, icon);
    if ui
        .interact(
            icon.expand(4.0),
            ui.scope_id().with("about"),
            Sense::click(),
        )
        .on_hover_text("About Graft")
        .clicked()
    {
        action = Some(TabAction::OpenAbout);
    }
    let update_label = update.map(|v| {
        ui.painter().layout_no_wrap(
            format!("Update to {v}"),
            theme::semibold(theme::SMALL_SIZE),
            color::on_accent(),
        )
    });
    let update_width = update_label
        .as_ref()
        .map_or(0.0, |g| g.size().x + 20.0 + 8.0);
    let gear = Rect::from_center_size(
        pos2(full.right() - window_frame::BUTTONS_WIDTH - 22.0, mid),
        vec2(BUTTON, BUTTON),
    );

    // The tabs get what is left between the app icon and the buttons after
    // them; past their minimum width they scroll.
    let left = full.left() + 40.0;
    let buttons = if tabs.is_empty() {
        32.0
    } else {
        32.0 + BUTTON + 4.0
    };
    let room = (gear.left() - 8.0 - update_width - left - buttons).max(MIN_WIDTH);
    let count = tabs.len() + usize::from(active.is_none());
    let width = (room / count.max(1) as f32 - GAP).clamp(MIN_WIDTH, MAX_WIDTH);
    let step = width + GAP;
    let content = count as f32 * step;
    let overflow = content > room;
    let max_offset = (content - room).max(0.0);
    let view = Rect::from_min_max(pos2(left, top), pos2(left + content.min(room), bottom));

    let scroll_id = ui.scope_id().with("tab-scroll");
    let mut offset: f32 = ui.data(|d| d.get_temp(scroll_id)).unwrap_or(0.0);
    if overflow && ui.rect_contains_pointer(view) {
        let delta = ui.input(|i| i.smooth_scroll_delta);
        offset -= delta.x + delta.y;
    }
    // A tab that becomes active (click, shortcut, search, a new repository)
    // scrolls into view; otherwise the strip stays where the user left it.
    let follow_id = ui.scope_id().with("tab-follow");
    let shown = active.unwrap_or(tabs.len());
    if ui.data(|d| d.get_temp::<(usize, usize)>(follow_id)) != Some((shown, count)) {
        ui.data_mut(|d| d.insert_temp(follow_id, (shown, count)));
        let margin = if overflow { ARROW } else { 0.0 };
        let tab_left = shown as f32 * step;
        offset = offset
            .min(tab_left - margin)
            .max(tab_left + width + margin - room);
    }
    offset = offset.clamp(0.0, max_offset);

    // Tabs scrolled partly out of view are cut off at the strip's ends.
    let mut tab_ui = ui.new_child(UiBuilder::new().id_salt("project-tabs").max_rect(view));
    tab_ui.set_clip_rect(view.intersect(ui.clip_rect()));
    let tab_ui = &tab_ui;

    let mut x = left - offset;
    let mut rects = Vec::with_capacity(tabs.len());
    let mut dragged = None;
    for (i, tab) in tabs.iter().enumerate() {
        let rect = Rect::from_min_size(pos2(x, top), vec2(width, TAB_HEIGHT));
        rects.push(rect);
        // Keyed by folder, not position: a drag must stay with its tab
        // when a reorder moves it.
        let id = tab_ui.scope_id().with(("project-tab", &tab.tooltip));
        let response = tab_ui
            .interact(rect, id, Sense::click_and_drag())
            .on_hover_text(&tab.tooltip);
        let close_rect = close_rect(rect);
        let close = tab_ui.interact(close_rect, id.with("close"), Sense::click());
        let selected = active == Some(i);

        paint_tab(
            tab_ui,
            rect,
            selected,
            response.hovered() || close.hovered(),
        );
        let y = rect.center().y;
        let text_x = rect.left() + PAD_X + status_dot(tab_ui, rect.left() + PAD_X, y, tab);
        let show_close = selected || response.hovered() || close.hovered();
        let text_right = if show_close {
            close_rect.left() - 4.0
        } else {
            rect.right() - PAD_X
        };
        let (font, fg) = if selected {
            (theme::semibold(theme::BODY_SIZE), color::text())
        } else {
            (theme::body(), color::text_weak())
        };
        let name = elided(tab_ui, &tab.name, font, fg, text_right - text_x);
        paint_galley_left(tab_ui.painter(), text_x, y, name, fg);
        if show_close {
            paint_icon_button(tab_ui, close_rect, &close, Icon::Close);
        }

        if close.clicked() || response.middle_clicked() {
            action = Some(TabAction::Close(i));
        } else if (response.clicked() || response.drag_started()) && active != Some(i) {
            action = Some(TabAction::Activate(i));
        }
        if response.dragged() {
            dragged = Some(i);
        }
        x += step;
    }

    // Reorder: swap with a neighbour once the pointer passes its middle.
    if let (Some(i), Some(pointer)) = (dragged, ui.ctx().pointer_interact_pos()) {
        if i + 1 < rects.len() && pointer.x > rects[i + 1].center().x {
            action = Some(TabAction::Move { from: i, to: i + 1 });
        } else if i > 0 && pointer.x < rects[i - 1].center().x {
            action = Some(TabAction::Move { from: i, to: i - 1 });
        }
        // Dragging to an end of a scrolling strip scrolls it along.
        if overflow {
            if pointer.x < view.left() + ARROW {
                offset -= 8.0;
            } else if pointer.x > view.right() - ARROW {
                offset += 8.0;
            }
            ui.ctx().request_repaint();
        }
    }

    if active.is_none() {
        let rect = Rect::from_min_size(pos2(x, top), vec2(width, TAB_HEIGHT));
        let id = tab_ui.scope_id().with("start-tab");
        let close_rect = close_rect(rect);
        let close = tab_ui.interact(close_rect, id.with("close"), Sense::click());
        paint_tab(tab_ui, rect, true, false);
        let label = if tabs.is_empty() {
            "Welcome"
        } else {
            "New tab"
        };
        let galley = elided(
            tab_ui,
            label,
            theme::semibold(theme::BODY_SIZE),
            color::text(),
            close_rect.left() - rect.left() - PAD_X,
        );
        paint_galley_left(
            tab_ui.painter(),
            rect.left() + PAD_X,
            rect.center().y,
            galley,
            color::text(),
        );
        if !tabs.is_empty() {
            paint_icon_button(tab_ui, close_rect, &close, Icon::Close);
            if close.clicked() {
                action = Some(TabAction::CloseStart);
            }
        }
    }

    if overflow {
        if offset > 0.5 && arrow(ui, view, false).clicked() {
            offset -= step;
        }
        if offset < max_offset - 0.5 && arrow(ui, view, true).clicked() {
            offset += step;
        }
    }
    ui.data_mut(|d| d.insert_temp(scroll_id, offset.clamp(0.0, max_offset)));

    let plus = Rect::from_center_size(pos2(view.right() + 16.0, mid), vec2(BUTTON, BUTTON));
    let response = ui
        .interact(plus, ui.scope_id().with("new-tab"), Sense::click())
        .on_hover_text(keys.tip("Open a repository", Command::NewTab));
    paint_icon_button(ui, plus, &response, Icon::Plus);
    if response.clicked() {
        action = Some(TabAction::NewTab);
    }
    if !tabs.is_empty() {
        let rect = Rect::from_center_size(
            pos2(plus.right() + 4.0 + BUTTON / 2.0, mid),
            vec2(BUTTON, BUTTON),
        );
        let response = ui
            .interact(rect, ui.scope_id().with("all-tabs"), Sense::click())
            .on_hover_text(keys.tip("All tabs", Command::AllTabs));
        paint_icon_button(ui, rect, &response, Icon::ChevronDown);
        let toggle = response.clicked() || keys.pressed(ui.ctx(), Command::AllTabs);
        if let Some(i) = all_tabs(ui, &response, toggle, tabs, active) {
            action = Some(TabAction::Activate(i));
        }
    }

    let response = ui
        .interact(gear, ui.scope_id().with("settings"), Sense::click())
        .on_hover_text(keys.tip("Settings", Command::Settings));
    paint_icon_button(ui, gear, &response, Icon::Settings);
    if response.clicked() {
        action = Some(TabAction::OpenSettings);
    }
    if let Some(galley) = update_label {
        let pill = Rect::from_min_size(
            pos2(gear.left() - update_width, top + (TAB_HEIGHT - 22.0) / 2.0),
            vec2(update_width - 8.0, 22.0),
        );
        let response = ui
            .interact(pill, ui.scope_id().with("update"), Sense::click())
            .on_hover_text("A new version of Graft is ready to install");
        let fill = if response.hovered() {
            color::accent_hover()
        } else {
            color::accent()
        };
        ui.painter().rect_filled(pill, 11, fill);
        let text_pos = pill.center() - galley.size() / 2.0;
        ui.painter().galley(text_pos, galley, color::on_accent());
        if response.clicked() {
            action = Some(TabAction::OpenAbout);
        }
    }
    window_frame::caption_buttons(ui, strip, mid);
    action
}

/// A spinner, error dot or dirty dot at `x`; returns the room it took.
fn status_dot(ui: &Ui, x: f32, y: f32, tab: &TabInfo) -> f32 {
    if tab.busy {
        spinner(ui, pos2(x + 4.0, y), 4.0, color::text_weak());
        14.0
    } else if tab.error {
        ui.painter()
            .circle_filled(pos2(x + 3.0, y), 3.5, color::removed());
        13.0
    } else if tab.dirty {
        ui.painter()
            .circle_filled(pos2(x + 3.0, y), 3.5, color::dirty_fg());
        13.0
    } else {
        0.0
    }
}

/// The scroll arrow at the left or right end of the tabs, painted over them.
fn arrow(ui: &Ui, view: Rect, right: bool) -> Response {
    let rect = if right {
        Rect::from_min_max(pos2(view.right() - ARROW, view.top()), view.right_bottom())
    } else {
        Rect::from_min_max(view.left_top(), pos2(view.left() + ARROW, view.bottom()))
    };
    let id = ui.scope_id().with(("tab-arrow", right));
    let response = ui.interact(rect, id, Sense::click());
    ui.painter().rect_filled(rect, 0, color::tabstrip_bg());
    let button = Rect::from_center_size(rect.center(), vec2(ARROW, ARROW));
    let icon = if right {
        Icon::ChevronRight
    } else {
        Icon::ChevronLeft
    };
    paint_icon_button(ui, button, &response, icon);
    response
}

/// The "All tabs" list under its button: every open repository, filtered by
/// name or folder as you type. Returns the tab picked.
fn all_tabs(
    ui: &Ui,
    button: &Response,
    toggle: bool,
    tabs: &[TabInfo],
    active: Option<usize>,
) -> Option<usize> {
    let id = button.id.with("list");
    let open_id = id.with("open");
    let filter_id = id.with("filter");
    let selected_id = id.with("selected");
    let mut open: bool = ui.data(|d| d.get_temp(open_id)).unwrap_or(false);
    let opened = toggle && !open;
    if toggle {
        open = !open;
    }
    if opened {
        ui.data_mut(|d| {
            d.insert_temp(filter_id, String::new());
            d.insert_temp(selected_id, active.unwrap_or(0));
        });
    }
    if !open || ui.input(|i| i.key_pressed(Key::Escape)) {
        ui.data_mut(|d| d.insert_temp(open_id, false));
        return None;
    }

    let mut filter: String = ui.data(|d| d.get_temp(filter_id)).unwrap_or_default();
    let mut selected: usize = ui.data(|d| d.get_temp(selected_id)).unwrap_or(0);
    let mut chosen = None;
    let area = Area::new(id)
        .order(Order::Foreground)
        .fixed_pos(pos2(
            button.rect.right() - LIST_WIDTH,
            button.rect.bottom() + 6.0,
        ))
        .show(ui.ctx(), |ui| {
            Frame::new()
                .fill(color::sidebar_bg())
                .stroke(Stroke::new(1.0, color::border()))
                .corner_radius(RADIUS)
                .inner_margin(Margin::same(6))
                .show(ui, |ui| {
                    ui.set_width(LIST_WIDTH - 12.0);
                    let before = filter.clone();
                    let edit = text_field(ui, &mut filter, "Find a tab…", LIST_WIDTH - 12.0);
                    if opened {
                        edit.request_focus();
                    }
                    let needle = filter.trim().to_lowercase();
                    let hits: Vec<usize> = (0..tabs.len())
                        .filter(|&i| {
                            needle.is_empty()
                                || tabs[i].name.to_lowercase().contains(&needle)
                                || tabs[i].tooltip.to_lowercase().contains(&needle)
                        })
                        .collect();
                    // The selection is a tab index; keep it on a visible one.
                    let mut pos = hits.iter().position(|&i| i == selected).unwrap_or(0);
                    if filter != before {
                        pos = 0;
                    }
                    let mut scroll = opened || filter != before;
                    ui.input_mut(|i| {
                        if i.consume_key(Modifiers::NONE, Key::ArrowDown) {
                            pos = (pos + 1).min(hits.len().saturating_sub(1));
                            scroll = true;
                        }
                        if i.consume_key(Modifiers::NONE, Key::ArrowUp) {
                            pos = pos.saturating_sub(1);
                            scroll = true;
                        }
                    });
                    if ui.input(|i| i.key_pressed(Key::Enter)) {
                        chosen = hits.get(pos).copied();
                    }
                    ui.add_space(4.0);
                    if hits.is_empty() {
                        ui.label(
                            egui::RichText::new("No open tab matches").color(color::text_faint()),
                        );
                        return;
                    }
                    ScrollArea::vertical()
                        .max_height(ROW_HEIGHT * 10.0)
                        .auto_shrink([false, true])
                        .show(ui, |ui| {
                            ui.spacing_mut().item_spacing.y = 0.0;
                            for (k, &i) in hits.iter().enumerate() {
                                let response = list_row(ui, &tabs[i], k == pos, active == Some(i));
                                if k == pos && scroll {
                                    ui.scroll_to_rect(response.rect, None);
                                }
                                if response.clicked() {
                                    chosen = Some(i);
                                }
                                if response.hovered()
                                    && ui.input(|i| i.pointer.delta() != vec2(0.0, 0.0))
                                {
                                    pos = k;
                                }
                            }
                        });
                    selected = hits.get(pos).copied().unwrap_or(selected);
                });
        });

    // A click anywhere else closes the list; the button toggles it itself.
    let clicked_outside = ui.input(|i| i.pointer.any_pressed())
        && ui
            .ctx()
            .pointer_interact_pos()
            .is_some_and(|p| !area.response.rect.contains(p) && !button.rect.contains(p));
    let keep = chosen.is_none() && !clicked_outside;
    ui.data_mut(|d| {
        d.insert_temp(open_id, keep);
        d.insert_temp(filter_id, filter);
        d.insert_temp(selected_id, selected);
    });
    chosen
}

fn list_row(ui: &mut Ui, tab: &TabInfo, selected: bool, current: bool) -> Response {
    let (rect, response) =
        ui.allocate_exact_size(vec2(ui.available_width(), ROW_HEIGHT), Sense::click());
    let painter = ui.painter();
    if selected {
        painter.rect_filled(rect, RADIUS, color::hover_row_bg());
    }
    if current {
        let bar = Rect::from_min_size(
            pos2(rect.left() + 1.0, rect.top() + 9.0),
            vec2(3.0, rect.height() - 18.0),
        );
        painter.rect_filled(bar, 1.5, color::accent());
    }
    let left = rect.left() + 12.0;
    let top_y = rect.top() + 13.0;
    let text_x = left + status_dot(ui, left, top_y, tab);
    let name = elided(
        ui,
        &tab.name,
        theme::semibold(theme::BODY_SIZE),
        color::text(),
        rect.right() - 10.0 - text_x,
    );
    paint_galley_left(painter, text_x, top_y, name, color::text());
    let path = elided_path(
        ui,
        &tab.tooltip,
        theme::small(),
        color::text_weak(),
        rect.right() - 10.0 - left,
    );
    paint_galley_left(painter, left, rect.top() + 29.0, path, color::text_weak());
    response
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
        painter.rect_filled(rect, radius, color::bar_bg());
        painter.hline(
            rect.x_range(),
            rect.top() + 0.5,
            Stroke::new(1.0, color::accent().gamma_multiply(0.6)),
        );
    } else if hovered {
        painter.rect_filled(rect, radius, color::hover_row_bg());
    }
}
