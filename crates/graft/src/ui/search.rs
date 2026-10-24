//! The toolbar search (Ctrl+K): type to find worktrees, branches, tags,
//! commits and changed files; Enter or a click jumps there.

use egui::{
    Align, Area, Frame, Id, Key, Layout, Margin, Modifiers, Order, Rect, RichText, ScrollArea,
    Sense, Stroke, TextEdit, Ui, vec2,
};

use super::icons::{self, Icon};
use crate::project::{Action, FileKey, Project};
use crate::theme::{self, RADIUS, color};

/// One search result.
struct Hit {
    kind: &'static str,
    title: String,
    detail: String,
    actions: Vec<Action>,
}

const LIMIT: usize = 8;

/// Every word of `query` occurs in one of `fields`.
fn matches(query: &[String], fields: &[&str]) -> bool {
    query.iter().all(|word| {
        fields
            .iter()
            .any(|f| f.to_lowercase().contains(word.as_str()))
    })
}

fn hits(project: &Project, text: &str) -> Vec<Hit> {
    let query: Vec<String> = text.split_whitespace().map(str::to_lowercase).collect();
    let mut out = Vec::new();
    if query.is_empty() {
        return out;
    }
    let Some(snap) = &project.snapshot else {
        return out;
    };

    for wt in snap
        .worktrees
        .iter()
        .filter(|wt| !wt.prunable)
        .filter(|wt| matches(&query, &[&wt.label(), &wt.path.to_string_lossy()]))
        .take(LIMIT)
    {
        out.push(Hit {
            kind: "Worktree",
            title: wt.label(),
            detail: wt.path.display().to_string(),
            actions: vec![Action::OpenWorktree(wt.path.clone())],
        });
    }
    for b in snap
        .branches
        .iter()
        .filter(|b| matches(&query, &[&b.name]))
        .take(LIMIT)
    {
        out.push(Hit {
            kind: "Branch",
            title: b.name.clone(),
            detail: graft_core::short(&b.target),
            actions: vec![Action::Reveal(b.target)],
        });
    }
    for b in snap
        .remote_branches
        .iter()
        .filter(|b| matches(&query, &[&b.name]))
        .take(LIMIT)
    {
        out.push(Hit {
            kind: "Remote",
            title: b.name.clone(),
            detail: graft_core::short(&b.target),
            actions: vec![Action::Reveal(b.target)],
        });
    }
    for t in snap
        .tags
        .iter()
        .filter(|t| matches(&query, &[&t.name]))
        .take(LIMIT)
    {
        out.push(Hit {
            kind: "Tag",
            title: t.name.clone(),
            detail: graft_core::short(&t.target),
            actions: vec![Action::Reveal(t.target)],
        });
    }

    if let Some(status) = project.worktree().and_then(|wt| wt.status.as_ref()) {
        for entry in status
            .entries
            .iter()
            .filter(|e| matches(&query, &[&e.path]))
            .take(LIMIT)
        {
            let staged = entry.is_staged() && !entry.is_unstaged();
            out.push(Hit {
                kind: "Changed",
                title: entry.path.clone(),
                detail: if staged { "staged" } else { "uncommitted" }.to_owned(),
                actions: vec![
                    Action::ShowFileStatus,
                    Action::SelectFile(FileKey::of(entry, staged)),
                ],
            });
        }
    }

    if let Some(history) = &project.history {
        let hex = text.trim().to_lowercase();
        let by_id = hex.len() >= 4 && hex.chars().all(|c| c.is_ascii_hexdigit());
        for c in history
            .commits
            .iter()
            .filter(|c| !c.is_worktree())
            .filter(|c| {
                (by_id && c.id.to_string().starts_with(&hex))
                    || matches(&query, &[&c.summary, &c.author])
            })
            .take(LIMIT * 2)
        {
            out.push(Hit {
                kind: "Commit",
                title: c.summary.clone(),
                detail: format!("{} · {}", graft_core::short(&c.id), c.author),
                actions: vec![Action::Reveal(c.id)],
            });
        }
    }
    out
}

pub fn show(ui: &mut Ui, project: &Project, text: &mut String, actions: &mut Vec<Action>) {
    let id = Id::unique("toolbar-search");
    let selected_id = id.with("selected");
    let popup_id = id.with("popup-rect");
    if ui.input_mut(|i| i.consume_key(Modifiers::COMMAND, Key::K)) {
        ui.memory_mut(|m| m.request_focus(id));
        // Select the old query so typing replaces it.
        let mut state = egui::text_edit::TextEditState::load(ui.ctx(), id).unwrap_or_default();
        let all = egui::text::CCursorRange::two(
            egui::text::CCursor::new(0),
            egui::text::CCursor::new(text.chars().count()),
        );
        state.cursor.set_char_range(Some(all));
        state.store(ui.ctx(), id);
    }
    let focused = ui.memory(|m| m.has_focus(id));
    // egui drops the focus as soon as Escape is pressed, so remember it.
    let focused_id = id.with("was-focused");
    let was_focused: bool = ui.data(|d| d.get_temp(focused_id)).unwrap_or(false);
    ui.data_mut(|d| d.insert_temp(focused_id, focused));
    if was_focused && !focused && ui.input(|i| i.key_pressed(Key::Escape)) {
        text.clear();
    }
    let border = if focused {
        color::accent()
    } else {
        color::control_border()
    };
    let width = ui.available_width().clamp(160.0, 300.0);

    let before = text.clone();
    let frame = Frame::new()
        .fill(color::app_bg())
        .stroke(Stroke::new(1.0, border))
        .corner_radius(RADIUS)
        .inner_margin(Margin::symmetric(8, 0))
        .show(ui, |ui| {
            ui.set_width(width - 16.0);
            ui.set_height(28.0);
            ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                let (rect, _) = ui.allocate_exact_size(vec2(14.0, 14.0), Sense::hover());
                icons::paint(ui.painter(), rect, Icon::Search, color::text_weak());
                ui.add(
                    TextEdit::singleline(text)
                        .id(id)
                        .frame(Frame::NONE)
                        .margin(Margin::ZERO)
                        .min_size(vec2(0.0, 28.0))
                        .align(egui::Align2::LEFT_CENTER)
                        .hint_text("Commits, branches, files…  Ctrl+K")
                        .desired_width(f32::INFINITY),
                );
            });
        });
    if *text != before {
        ui.data_mut(|d| d.insert_temp(selected_id, 0usize));
        // The result list sizes itself from the previous frame: draw again.
        ui.ctx().request_repaint();
    }

    // Keep the list open while the pointer is on it, so a click lands.
    let last_popup: Option<Rect> = ui.data(|d| d.get_temp(popup_id));
    let over_popup = last_popup
        .zip(ui.ctx().pointer_hover_pos())
        .is_some_and(|(r, p)| r.contains(p));
    if text.trim().is_empty() || !(focused || over_popup) {
        ui.data_mut(|d| d.remove::<Rect>(popup_id));
        return;
    }

    let results = hits(project, text);
    let mut selected: usize = ui.data(|d| d.get_temp(selected_id)).unwrap_or(0);
    if focused {
        ui.input_mut(|i| {
            if i.consume_key(Modifiers::NONE, Key::ArrowDown) {
                selected += 1;
            }
            if i.consume_key(Modifiers::NONE, Key::ArrowUp) {
                selected = selected.saturating_sub(1);
            }
        });
    }
    selected = selected.min(results.len().saturating_sub(1));
    let enter = focused && ui.input(|i| i.key_pressed(Key::Enter));
    let mut chosen = enter.then_some(selected);

    let anchor = frame.response.rect;
    let popup_width = 460.0f32.max(anchor.width());
    let area = Area::new(id.with("popup"))
        .order(Order::Foreground)
        .fixed_pos(egui::pos2(
            anchor.right() - popup_width,
            anchor.bottom() + 4.0,
        ))
        .show(ui.ctx(), |ui| {
            Frame::new()
                .fill(color::sidebar_bg())
                .stroke(Stroke::new(1.0, color::border()))
                .corner_radius(RADIUS)
                .inner_margin(Margin::same(6))
                .show(ui, |ui| {
                    ui.set_width(popup_width - 12.0);
                    if results.is_empty() {
                        ui.label(RichText::new("No matches").color(color::text_faint()));
                        return;
                    }
                    // The popup keeps last frame's size; ask for this list's height.
                    let spacing = ui.spacing().item_spacing.y;
                    let height = (results.len() as f32 * (30.0 + spacing)).min(420.0);
                    ScrollArea::vertical()
                        .max_height(420.0)
                        .min_scrolled_height(height)
                        .show(ui, |ui| {
                            for (i, hit) in results.iter().enumerate() {
                                let response = row(ui, hit, i == selected);
                                if i == selected && (*text != before || chosen.is_none()) {
                                    response.scroll_to_me(None);
                                }
                                if response.clicked() {
                                    chosen = Some(i);
                                }
                                if response.hovered()
                                    && ui.input(|i| i.pointer.delta() != vec2(0.0, 0.0))
                                {
                                    selected = i;
                                }
                            }
                        });
                });
        });
    ui.data_mut(|d| {
        d.insert_temp(popup_id, area.response.rect);
        d.insert_temp(selected_id, selected);
    });

    if let Some(i) = chosen
        && let Some(hit) = results.get(i)
    {
        actions.extend(hit.actions.iter().cloned());
        text.clear();
        ui.memory_mut(|m| m.surrender_focus(id));
        ui.data_mut(|d| d.remove::<Rect>(popup_id));
    }
}

fn row(ui: &mut Ui, hit: &Hit, selected: bool) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width(), 30.0), Sense::click());
    let painter = ui.painter();
    if selected {
        painter.rect_filled(rect, RADIUS, color::selected_row_bg());
    } else if response.hovered() {
        painter.rect_filled(rect, RADIUS, color::hover_row_bg());
    }
    let y = rect.center().y;
    let kind = painter.layout_no_wrap(hit.kind.to_owned(), theme::small(), color::text_faint());
    painter.galley(
        egui::pos2(rect.left() + 8.0, y - kind.size().y / 2.0),
        kind,
        color::text_faint(),
    );
    let detail_width = 150.0;
    let font = if matches!(hit.kind, "Commit" | "Changed") {
        theme::body()
    } else {
        theme::mono()
    };
    let title = super::widgets::elided(
        ui,
        &hit.title,
        font,
        color::text(),
        rect.width() - 80.0 - detail_width - 16.0,
    );
    super::widgets::paint_galley_left(painter, rect.left() + 80.0, y, title, color::text());
    let detail = super::widgets::elided_path(
        ui,
        &hit.detail,
        theme::small(),
        color::text_weak(),
        detail_width,
    );
    let x = rect.right() - 8.0 - detail.size().x;
    super::widgets::paint_galley_left(painter, x, y, detail, color::text_weak());
    response
}
