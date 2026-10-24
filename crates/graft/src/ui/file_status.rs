//! File status: staged files above unstaged files on the left, the selected
//! file's diff on the right, and the commit box across the bottom.

use egui::{
    Frame, Key, Margin, Modifiers, Panel, Rect, RichText, ScrollArea, Sense, Stroke, Ui, pos2, vec2,
};
use graft_core::status::{Change, StatusEntry, WorktreeStatus};

use super::history::{paint_change_letter, paint_path};
use super::icons::Icon;
use super::widgets::{
    ButtonKind, button, empty_state, paint_icon_button, section_heading, text_field,
};
use super::{conflicts, diff_view};
use crate::project::{Action, FileKey, Project};
use crate::theme::{self, RADIUS, color};

pub fn show(ui: &mut Ui, project: &mut Project, actions: &mut Vec<Action>) {
    let Some(status) = project.worktree().and_then(|wt| wt.status.clone()) else {
        empty_state(
            ui,
            "No status",
            "The status of this worktree could not be read.",
        );
        return;
    };

    Panel::bottom("commit-box")
        .frame(
            Frame::new()
                .fill(color::SIDEBAR_BG)
                .inner_margin(Margin::symmetric(14, 10)),
        )
        .show(ui, |ui| commit_box(ui, project, &status, actions));

    Panel::left("file-lists")
        .resizable(true)
        .default_size(380.0)
        .size_range(240.0..=720.0)
        .frame(
            Frame::new()
                .fill(color::APP_BG)
                .inner_margin(Margin::symmetric(8, 8)),
        )
        .show(ui, |ui| file_lists(ui, project, &status, actions));

    if status.entries.is_empty() {
        empty_state(ui, "Nothing to commit", "The working tree is clean.");
        return;
    }
    if project.conflict.is_some() {
        conflicts::resolver(ui, project, actions);
    } else {
        diff_view::show(ui, project, actions);
    }
}

fn file_lists(ui: &mut Ui, project: &Project, status: &WorktreeStatus, actions: &mut Vec<Action>) {
    let busy = project.is_operating();
    let selected = project.diff_view.as_ref().map(|v| &v.key);
    let staged: Vec<&StatusEntry> = status.staged().collect();
    let unstaged: Vec<&StatusEntry> = status.unstaged().collect();
    let conflicted: Vec<&StatusEntry> = status.conflicted().collect();

    ScrollArea::vertical()
        .id_salt("file-lists-scroll")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 1.0;
            if !conflicted.is_empty() {
                section(ui, "CONFLICTS", conflicted.len(), None, busy);
                for entry in &conflicted {
                    let key = FileKey::of(entry, false);
                    file_row(
                        ui,
                        entry,
                        Change::Unmerged,
                        &key,
                        selected == Some(&key),
                        None,
                        busy,
                        actions,
                    );
                }
                ui.add_space(10.0);
            }

            let all_staged: Vec<String> = staged.iter().map(|e| e.path.clone()).collect();
            if section(ui, "STAGED", staged.len(), Some("Unstage all"), busy) {
                actions.push(Action::UnstageFiles(all_staged));
            }
            if staged.is_empty() {
                hint(ui, "Stage files, hunks or lines to commit them.");
            }
            for entry in &staged {
                let key = FileKey::of(entry, true);
                file_row(
                    ui,
                    entry,
                    entry.index,
                    &key,
                    selected == Some(&key),
                    Some(false),
                    busy,
                    actions,
                );
            }
            ui.add_space(10.0);

            let all_unstaged: Vec<String> = unstaged.iter().map(|e| e.path.clone()).collect();
            if section(ui, "UNSTAGED", unstaged.len(), Some("Stage all"), busy) {
                actions.push(Action::StageFiles(all_unstaged));
            }
            if unstaged.is_empty() {
                hint(ui, "No unstaged changes.");
            }
            for entry in &unstaged {
                let key = FileKey::of(entry, false);
                file_row(
                    ui,
                    entry,
                    entry.unstaged_change(),
                    &key,
                    selected == Some(&key),
                    Some(true),
                    busy,
                    actions,
                );
            }
        });
}

/// Section heading with a count and an optional link on the right; returns
/// whether the link was clicked.
fn section(ui: &mut Ui, title: &str, count: usize, link: Option<&str>, busy: bool) -> bool {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 24.0), Sense::hover());
    let y = rect.center().y;
    section_heading(
        ui,
        ui.painter(),
        rect.left() + 4.0,
        y,
        &format!("{title}  {count}"),
    );
    let Some(link) = link.filter(|_| count > 0) else {
        return false;
    };
    let galley = ui
        .painter()
        .layout_no_wrap(link.to_owned(), theme::small(), color::AHEAD_FG);
    let r = Rect::from_min_size(
        pos2(rect.right() - galley.size().x - 8.0, rect.top() + 3.0),
        vec2(galley.size().x + 6.0, 18.0),
    );
    let response = ui.interact(
        r,
        ui.scope_id().with(("section-link", title)),
        Sense::click(),
    );
    let fg = if busy {
        color::TEXT_FAINT
    } else if response.hovered() {
        color::TEXT
    } else {
        color::AHEAD_FG
    };
    ui.painter()
        .galley(pos2(r.left() + 3.0, y - galley.size().y / 2.0), galley, fg);
    response.clicked() && !busy
}

fn hint(ui: &mut Ui, text: &str) {
    ui.add_space(2.0);
    ui.label(
        RichText::new(text)
            .color(color::TEXT_FAINT)
            .size(theme::SMALL_SIZE),
    );
    ui.add_space(2.0);
}

/// One file. `stage`: `Some(true)` offers staging, `Some(false)` unstaging.
#[allow(clippy::too_many_arguments)]
fn file_row(
    ui: &mut Ui,
    entry: &StatusEntry,
    change: Change,
    key: &FileKey,
    selected: bool,
    stage: Option<bool>,
    busy: bool,
    actions: &mut Vec<Action>,
) {
    let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width(), 26.0), Sense::click());
    let painter = ui.painter();
    if selected {
        painter.rect_filled(rect, RADIUS, color::SELECTED_ROW_BG);
    } else if response.hovered() {
        painter.rect_filled(rect, RADIUS, color::HOVER_ROW_BG);
    }
    let y = rect.center().y;
    paint_change_letter(ui, rect.left() + 6.0, y, change);

    let button_rect = Rect::from_center_size(pos2(rect.right() - 14.0, y), vec2(20.0, 20.0));
    let show_button = stage.is_some() && (response.hovered() || selected) && !busy;
    let x = rect.left() + 30.0;
    let right = if show_button {
        button_rect.left() - 6.0
    } else {
        rect.right() - 6.0
    };
    paint_path(ui, x, y, &entry.path, right - x);

    let paths = vec![entry.path.clone()];
    let toggle = |actions: &mut Vec<Action>| match stage {
        Some(true) => actions.push(Action::StageFiles(paths.clone())),
        Some(false) => actions.push(Action::UnstageFiles(paths.clone())),
        None => {}
    };
    let mut button_clicked = false;
    if show_button {
        let b = ui.interact(
            button_rect,
            ui.scope_id()
                .with(("stage-btn", &key.path, key.is_staged())),
            Sense::click(),
        );
        let icon = if stage == Some(true) {
            Icon::Plus
        } else {
            Icon::Minus
        };
        paint_icon_button(ui, button_rect, &b, icon);
        let tip = if stage == Some(true) {
            "Stage"
        } else {
            "Unstage"
        };
        if b.on_hover_text(tip).clicked() {
            button_clicked = true;
            toggle(actions);
        }
    }
    if !button_clicked {
        if response.double_clicked() && !busy {
            toggle(actions);
        } else if response.clicked() {
            actions.push(Action::SelectFile(key.clone()));
        }
    }
    let tip = match &entry.orig_path {
        Some(orig) => format!("{orig} → {}", entry.path),
        None => entry.path.clone(),
    };
    let response = response.on_hover_text(tip);
    response.context_menu(|ui| {
        match stage {
            Some(true) => {
                if ui.button("Stage").clicked() {
                    toggle(actions);
                    ui.close();
                }
                if ui.button("Discard changes…").clicked() {
                    actions.push(diff_view::discard_file(&key.source, entry.path.clone()));
                    ui.close();
                }
            }
            Some(false) if ui.button("Unstage").clicked() => {
                toggle(actions);
                ui.close();
            }
            _ => {}
        }
        ui.separator();
        if ui.button("Copy path").clicked() {
            ui.ctx().copy_text(entry.path.clone());
            ui.close();
        }
        if ui.button("Open file").clicked() {
            actions.push(Action::OpenPath(entry.path.clone()));
            ui.close();
        }
        if ui.button("Show in Explorer").clicked() {
            actions.push(Action::RevealPath(entry.path.clone()));
            ui.close();
        }
    });
}

fn commit_box(
    ui: &mut Ui,
    project: &mut Project,
    status: &WorktreeStatus,
    actions: &mut Vec<Action>,
) {
    let staged = status.staged().count();
    let busy = project.is_operating();
    let form = &mut project.commit_form;
    let width = ui.available_width();

    ui.spacing_mut().item_spacing.y = 6.0;
    ui.horizontal(|ui| {
        let counter_width = 40.0;
        text_field(
            ui,
            &mut form.summary,
            "Commit summary",
            width - counter_width - 6.0,
        );
        let len = form.summary.chars().count();
        let fg = if len > 72 {
            color::DIRTY_FG
        } else {
            color::TEXT_FAINT
        };
        ui.label(
            RichText::new(len.to_string())
                .color(fg)
                .size(theme::SMALL_SIZE),
        )
        .on_hover_text("Summaries read best under 72 characters");
    });
    Frame::new()
        .fill(color::APP_BG)
        .stroke(Stroke::new(1.0, color::CONTROL_BORDER))
        .corner_radius(RADIUS)
        .inner_margin(Margin::symmetric(8, 6))
        .show(ui, |ui| {
            ui.add(
                egui::TextEdit::multiline(&mut form.body)
                    .frame(Frame::NONE)
                    .hint_text(RichText::new("Description (optional)").color(color::TEXT_FAINT))
                    .desired_rows(3)
                    .desired_width(f32::INFINITY),
            );
        });
    ui.horizontal(|ui| {
        let mut amend = form.amend;
        if ui.checkbox(&mut amend, "Amend last commit").changed() {
            actions.push(Action::ToggleAmend);
        }
        ui.checkbox(&mut form.push_after, "Push after commit");
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let ready = !form.summary.trim().is_empty() && (staged > 0 || form.amend) && !busy;
            let label = if form.amend { "Amend commit" } else { "Commit" };
            let clicked = button(ui, label, ButtonKind::Primary, ready)
                .on_hover_text("Ctrl+Enter")
                .clicked();
            let shortcut = ui.input_mut(|i| i.consume_key(Modifiers::COMMAND, Key::Enter));
            if ready && (clicked || shortcut) {
                actions.push(Action::Commit);
            }
            let note = match (staged, form.amend) {
                (0, false) => "Nothing staged yet".to_owned(),
                (0, true) => "Rewords the last commit".to_owned(),
                (n, _) => format!("{n} file{} staged", if n == 1 { "" } else { "s" }),
            };
            ui.label(RichText::new(note).color(color::TEXT_WEAK));
        });
    });
}
