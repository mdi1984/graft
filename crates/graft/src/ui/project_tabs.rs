//! The strip of project tabs at the very top, like browser tabs: one per open
//! repository, a "+" for the start page, drag to reorder, middle-click to close.
//! When the tabs don't fit they scroll sideways (wheel, or the arrows at the
//! ends), and the "All tabs" list next to "+" finds any of them. Tabs can
//! be grouped (see `groups`): a colored label before the group's tabs,
//! which collapses them.
//! It is also the window's title bar (see `window_frame`).

use std::sync::Arc;

use egui::{
    Area, Color32, CornerRadius, Frame, Galley, Key, Margin, Modifiers, Order, Rect, Response,
    RichText, ScrollArea, Sense, Stroke, Ui, UiBuilder, pos2, vec2,
};

use super::icons::Icon;
use super::widgets::{
    elided, elided_path, paint_galley_left, paint_icon_button, spinner, text_field,
};
use super::window_frame;
use crate::groups::{self, GroupId, Item};
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
const LABEL_PAD: f32 = 10.0;
const LABEL_MAX_WIDTH: f32 = 180.0;

pub struct TabInfo {
    pub name: String,
    /// The repository's folder.
    pub tooltip: String,
    pub dirty: bool,
    pub busy: bool,
    pub error: bool,
    pub group: Option<GroupId>,
}

/// A tab group, as the strip shows it.
pub struct GroupInfo {
    pub id: GroupId,
    pub name: String,
    /// Index into `color::lanes()`.
    pub color: usize,
    pub collapsed: bool,
}

pub enum TabAction {
    Activate(usize),
    Close(usize),
    /// A dragged tab moves to `to` (see [`groups::Step`]).
    Move {
        from: usize,
        to: usize,
        group: Option<GroupId>,
    },
    /// Show the start page.
    NewTab,
    /// Close the start page tab.
    CloseStart,
    OpenSettings,
    OpenAbout,
    ToggleGroup(GroupId),
    /// Ask for a name, then put the tab into a new group.
    NewGroup(usize),
    SetGroup {
        tab: usize,
        group: Option<GroupId>,
    },
    RenameGroup(GroupId),
    RecolorGroup(GroupId, usize),
    /// Close the group's tabs; the group stays.
    CloseGroup(GroupId),
    /// Forget the group; its tabs stay open.
    Ungroup(GroupId),
}

/// `active` is `None` while the start page is showing; it then gets a tab too.
/// `update` is a newer Graft's version, offered next to the settings button.
pub fn show(
    ui: &mut Ui,
    tabs: &[TabInfo],
    tab_groups: &[GroupInfo],
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

    let group_info = |id: GroupId| tab_groups.iter().find(|g| g.id == id);
    let collapsed = |id: GroupId| group_info(id).is_some_and(|g| g.collapsed);
    let memberships: Vec<Option<GroupId>> = tabs.iter().map(|t| t.group).collect();
    let items = groups::visible(&memberships, collapsed, active);

    // The tabs get what is left between the app icon and the buttons after
    // them, besides the group labels; past their minimum width they scroll.
    let labels: Vec<Option<Label>> = items
        .iter()
        .map(|item| match *item {
            Item::Label(id) => group_info(id).map(|g| Label::new(ui, g, tabs)),
            Item::Tab(_) => None,
        })
        .collect();
    let left = full.left() + 40.0;
    let buttons = if tabs.is_empty() {
        32.0
    } else {
        32.0 + BUTTON + 4.0
    };
    let room = (gear.left() - 8.0 - update_width - left - buttons).max(MIN_WIDTH);
    let tab_count =
        items.iter().filter(|i| matches!(i, Item::Tab(_))).count() + usize::from(active.is_none());
    let labels_width: f32 = labels.iter().flatten().map(|l| l.width + GAP).sum();
    let width = ((room - labels_width) / tab_count.max(1) as f32 - GAP).clamp(MIN_WIDTH, MAX_WIDTH);
    let step = width + GAP;
    // Where each item starts, from the strip's start. The start page's tab
    // comes last.
    let mut starts = Vec::with_capacity(items.len());
    let mut content = 0.0;
    for (item, label) in items.iter().zip(&labels) {
        starts.push(content);
        content += match item {
            Item::Label(_) => label.as_ref().map_or(0.0, |l| l.width + GAP),
            Item::Tab(_) => step,
        };
    }
    let start_tab_at = content;
    if active.is_none() {
        content += step;
    }
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
    let follow = (active.unwrap_or(tabs.len()), items.len());
    if ui.data(|d| d.get_temp::<(usize, usize)>(follow_id)) != Some(follow) {
        ui.data_mut(|d| d.insert_temp(follow_id, follow));
        let margin = if overflow { ARROW } else { 0.0 };
        let tab_left = active
            .and_then(|a| items.iter().position(|it| *it == Item::Tab(a)))
            .map_or(start_tab_at, |k| starts[k]);
        offset = offset
            .min(tab_left - margin)
            .max(tab_left + width + margin - room);
    }
    offset = offset.clamp(0.0, max_offset);

    // Tabs scrolled partly out of view are cut off at the strip's ends.
    let mut tab_ui = ui.new_child(UiBuilder::new().id_salt("project-tabs").max_rect(view));
    tab_ui.set_clip_rect(view.intersect(ui.clip_rect()));
    let tab_ui = &tab_ui;

    let mut rects = Vec::with_capacity(items.len());
    let mut dragged = None;
    for (k, item) in items.iter().enumerate() {
        let x = left - offset + starts[k];
        let i = match *item {
            Item::Tab(i) => i,
            Item::Label(_) => {
                let rect = match &labels[k] {
                    Some(label) => {
                        let rect = Rect::from_min_size(pos2(x, top), vec2(label.width, TAB_HEIGHT));
                        if let Some(a) = label.show(tab_ui, rect, tab_groups) {
                            action = Some(a);
                        }
                        rect
                    }
                    None => Rect::NOTHING,
                };
                rects.push(rect);
                continue;
            }
        };
        let tab = &tabs[i];
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
        if let Some(g) = tab.group.and_then(group_info) {
            tab_ui.painter().hline(
                rect.x_range(),
                rect.bottom() - 1.0,
                Stroke::new(2.0, group_color(g.color)),
            );
        }
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
        response.context_menu(|ui| {
            if let Some(a) = tab_menu(ui, i, tab, tab_groups) {
                action = Some(a);
            }
        });
        if response.dragged() {
            dragged = Some((k, i));
        }
    }

    // Reorder: step past a neighbour (a tab or a group's label) once the
    // pointer passes its middle. The last tab of a group at the very end
    // leaves it once the pointer passes its own end.
    let hold_id = ui.scope_id().with("tab-drag-hold");
    if let (Some((k, i)), Some(pointer)) = (dragged, ui.ctx().pointer_interact_pos()) {
        let right = match rects.get(k + 1) {
            Some(next) => pointer.x > next.center().x,
            None => tabs[i].group.is_some() && pointer.x > rects[k].right(),
        };
        let back = k > 0 && pointer.x < rects[k - 1].center().x;
        // Joining or leaving a group can leave the tab where it is, with the
        // pointer still past the neighbour: the next step waits until the
        // pointer has moved on by half a tab.
        let hold: Option<f32> = ui.data(|d| d.get_temp(hold_id));
        let held = hold.is_some_and(|x| (pointer.x - x).abs() < step / 2.0);
        if (right || back)
            && !held
            && let Some(next) = groups::drag_step(&memberships, &items, collapsed, i, right)
        {
            if next.to == i {
                ui.data_mut(|d| d.insert_temp(hold_id, pointer.x));
            } else {
                ui.data_mut(|d| d.remove::<f32>(hold_id));
            }
            action = Some(TabAction::Move {
                from: i,
                to: next.to,
                group: next.group,
            });
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
    } else {
        ui.data_mut(|d| d.remove::<f32>(hold_id));
    }

    if active.is_none() {
        let x = left - offset + start_tab_at;
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
        if let Some(i) = all_tabs(ui, &response, toggle, tabs, tab_groups, active) {
            action = Some(TabAction::Activate(i));
        }
    } else {
        // No tabs, no list: it must not pop up again with the next one.
        let open_id = ui.scope_id().with("all-tabs").with("list").with("open");
        ui.data_mut(|d| d.insert_temp(open_id, false));
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
/// name, folder or group as you type. Returns the tab picked.
fn all_tabs(
    ui: &Ui,
    button: &Response,
    toggle: bool,
    tabs: &[TabInfo],
    tab_groups: &[GroupInfo],
    active: Option<usize>,
) -> Option<usize> {
    let group_of = |tab: &TabInfo| {
        tab.group
            .and_then(|id| tab_groups.iter().find(|g| g.id == id))
    };
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
    // Escape closes the list and nothing else.
    if !open || ui.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape)) {
        ui.data_mut(|d| d.insert_temp(open_id, false));
        return None;
    }

    let mut filter: String = ui.data(|d| d.get_temp(filter_id)).unwrap_or_default();
    let mut selected: usize = ui.data(|d| d.get_temp(selected_id)).unwrap_or(0);
    let mut chosen = None;
    let mut focus_moved = false;
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
                    // The list takes keys only while its field has the
                    // focus. Focus moved to another field (Ctrl+K, say)
                    // closes it; a click on the list's padding gives the
                    // focus back.
                    let focused = edit.has_focus() || edit.lost_focus();
                    let other = ui.memory(|m| m.focused().is_some_and(|f| f != edit.id));
                    if !opened && other {
                        focus_moved = true;
                    } else if !edit.has_focus() {
                        edit.request_focus();
                    }
                    let needle = filter.trim().to_lowercase();
                    let hits: Vec<usize> = (0..tabs.len())
                        .filter(|&i| {
                            needle.is_empty()
                                || tabs[i].name.to_lowercase().contains(&needle)
                                || tabs[i].tooltip.to_lowercase().contains(&needle)
                                || group_of(&tabs[i])
                                    .is_some_and(|g| g.name.to_lowercase().contains(&needle))
                        })
                        .collect();
                    // The selection is a tab index; keep it on a visible one.
                    let mut pos = hits.iter().position(|&i| i == selected).unwrap_or(0);
                    if filter != before {
                        pos = 0;
                    }
                    let mut scroll = opened || filter != before;
                    if focused {
                        ui.input_mut(|i| {
                            if i.consume_key(Modifiers::NONE, Key::ArrowDown) {
                                pos = (pos + 1).min(hits.len().saturating_sub(1));
                                scroll = true;
                            }
                            if i.consume_key(Modifiers::NONE, Key::ArrowUp) {
                                pos = pos.saturating_sub(1);
                                scroll = true;
                            }
                            if i.consume_key(Modifiers::NONE, Key::Enter) {
                                chosen = hits.get(pos).copied();
                            }
                        });
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
                                let tab = &tabs[i];
                                let response =
                                    list_row(ui, tab, group_of(tab), k == pos, active == Some(i));
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
    let keep = chosen.is_none() && !clicked_outside && !focus_moved;
    ui.data_mut(|d| {
        d.insert_temp(open_id, keep);
        d.insert_temp(filter_id, filter);
        d.insert_temp(selected_id, selected);
    });
    chosen
}

fn list_row(
    ui: &mut Ui,
    tab: &TabInfo,
    group: Option<&GroupInfo>,
    selected: bool,
    current: bool,
) -> Response {
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
    // The group's name, in its color, at the right end.
    let mut name_right = rect.right() - 10.0;
    if let Some(g) = group {
        let fg = group_color(g.color);
        let galley = elided(ui, &g.name, theme::small(), fg, 120.0);
        let x = name_right - galley.size().x;
        paint_galley_left(painter, x, top_y, galley, fg);
        name_right = x - 8.0;
    }
    let name = elided(
        ui,
        &tab.name,
        theme::semibold(theme::BODY_SIZE),
        color::text(),
        name_right - text_x,
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

/// A group's color.
pub fn group_color(index: usize) -> Color32 {
    let lanes = color::lanes();
    lanes[index % lanes.len()]
}

/// A group's label in the strip: its name on a tint of its color. Click to
/// collapse or expand; right-click for the group's menu.
struct Label {
    id: GroupId,
    color: Color32,
    galley: Arc<Galley>,
    /// Collapsed, with changes in one of its tabs.
    dirty: bool,
    collapsed: bool,
    width: f32,
}

impl Label {
    fn new(ui: &Ui, group: &GroupInfo, tabs: &[TabInfo]) -> Self {
        let color = group_color(group.color);
        let members = || tabs.iter().filter(|t| t.group == Some(group.id));
        // Collapsed, the label says how many tabs it hides.
        let text = if group.collapsed {
            format!("{}  {}", group.name, members().count())
        } else {
            group.name.clone()
        };
        let galley =
            ui.painter()
                .layout_no_wrap(text, theme::semibold(theme::SMALL_SIZE + 1.0), color);
        let dirty = group.collapsed && members().any(|t| t.dirty);
        let width = (galley.size().x + 2.0 * LABEL_PAD + if dirty { 11.0 } else { 0.0 })
            .min(LABEL_MAX_WIDTH);
        Self {
            id: group.id,
            color,
            galley,
            dirty,
            collapsed: group.collapsed,
            width,
        }
    }

    fn show(&self, ui: &Ui, rect: Rect, groups: &[GroupInfo]) -> Option<TabAction> {
        let id = ui.scope_id().with(("group-label", self.id));
        let response = ui
            .interact(rect, id, Sense::click())
            .on_hover_text(if self.collapsed {
                "Show this group's tabs"
            } else {
                "Collapse this group"
            });
        let chip = Rect::from_center_size(rect.center(), vec2(rect.width(), 22.0));
        let tint = if response.hovered() { 0.32 } else { 0.2 };
        let painter = ui.painter();
        painter.rect_filled(chip, RADIUS, self.color.gamma_multiply(tint));
        let mut x = chip.left() + LABEL_PAD;
        if self.dirty {
            painter.circle_filled(pos2(x + 3.0, chip.center().y), 3.0, color::dirty_fg());
            x += 11.0;
        }
        let clip = chip.shrink2(vec2(LABEL_PAD - 2.0, 0.0));
        painter
            .with_clip_rect(clip.intersect(ui.clip_rect()))
            .galley(
                pos2(x, chip.center().y - self.galley.size().y / 2.0),
                self.galley.clone(),
                self.color,
            );
        let mut action = None;
        if response.clicked() {
            action = Some(TabAction::ToggleGroup(self.id));
        }
        response.context_menu(|ui| {
            if let Some(a) = group_menu(ui, self.id, groups) {
                action = Some(a);
            }
        });
        action
    }
}

fn tab_menu(ui: &mut Ui, i: usize, tab: &TabInfo, groups: &[GroupInfo]) -> Option<TabAction> {
    let mut action = None;
    ui.menu_button("Add to group", |ui| {
        if ui.button("New group…").clicked() {
            action = Some(TabAction::NewGroup(i));
            ui.close();
        }
        let others: Vec<&GroupInfo> = groups.iter().filter(|g| Some(g.id) != tab.group).collect();
        if !others.is_empty() {
            ui.separator();
        }
        for g in others {
            let name = RichText::new(&g.name).color(group_color(g.color));
            if ui.button(name).clicked() {
                action = Some(TabAction::SetGroup {
                    tab: i,
                    group: Some(g.id),
                });
                ui.close();
            }
        }
    });
    if tab.group.is_some() && ui.button("Remove from group").clicked() {
        action = Some(TabAction::SetGroup {
            tab: i,
            group: None,
        });
        ui.close();
    }
    ui.separator();
    if ui.button("Close tab").clicked() {
        action = Some(TabAction::Close(i));
        ui.close();
    }
    action
}

fn group_menu(ui: &mut Ui, id: GroupId, groups: &[GroupInfo]) -> Option<TabAction> {
    let current = groups.iter().find(|g| g.id == id)?.color;
    let mut action = None;
    if ui.button("Rename…").clicked() {
        action = Some(TabAction::RenameGroup(id));
        ui.close();
    }
    if let Some(c) = color_swatches(ui, current) {
        action = Some(TabAction::RecolorGroup(id, c));
        ui.close();
    }
    ui.separator();
    if ui.button("Close its tabs").clicked() {
        action = Some(TabAction::CloseGroup(id));
        ui.close();
    }
    if ui.button("Ungroup").clicked() {
        action = Some(TabAction::Ungroup(id));
        ui.close();
    }
    action
}

/// A row of the group colors; returns the one clicked.
pub fn color_swatches(ui: &mut Ui, current: usize) -> Option<usize> {
    let mut picked = None;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 2.0;
        for (c, fill) in color::lanes().into_iter().enumerate() {
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
