//! The always-visible toolbar: Commit, Pull, Push, Fetch | Branch, Merge, Stash,
//! New worktree | Preview in main, Slash | search.

use egui::{Align, Frame, Id, Key, Layout, Margin, Modifiers, Sense, Stroke, TextEdit, Ui, vec2};

use super::icons::{self, Icon};
use super::widgets::{Tone, tool_button, toolbar_divider};
use crate::project::{Action, DialogKind, Project, RemoteOp, Tab};
use crate::theme::{RADIUS, color};

const LATER: &str = "Coming in a later step";

pub fn show(ui: &mut Ui, project: &Project, actions: &mut Vec<Action>, search: &mut String) {
    let ahead = project.worktree().map_or(0, |wt| wt.ahead());

    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;

        if tool_button(ui, Some(Icon::Commit), "Commit", None, Tone::Normal).clicked() {
            actions.push(Action::ShowFileStatus);
        }
        let busy = project.is_operating() || project.snapshot.is_none();
        let behind = project.worktree().map_or(0, |wt| wt.behind());
        let pull_badge = (behind > 0).then(|| behind.to_string());
        if tool_button(ui, Some(Icon::Pull), "Pull", pull_badge, Tone::Normal)
            .on_hover_text("Pull the current branch from its upstream")
            .clicked()
            && !busy
        {
            actions.push(Action::Remote(RemoteOp::Pull));
        }
        let push_badge = (ahead > 0).then(|| ahead.to_string());
        if tool_button(ui, Some(Icon::Push), "Push", push_badge, Tone::Normal)
            .on_hover_text("Push the current branch (sets the upstream the first time)")
            .clicked()
            && !busy
        {
            actions.push(Action::Remote(RemoteOp::Push));
        }
        if tool_button(ui, Some(Icon::Fetch), "Fetch", None, Tone::Normal)
            .on_hover_text("Fetch all remotes")
            .clicked()
            && !busy
        {
            actions.push(Action::Remote(RemoteOp::Fetch));
        }
        toolbar_divider(ui);

        if tool_button(ui, None, "Branch", None, Tone::Normal)
            .on_hover_text("New branch from the current commit")
            .clicked()
            && !busy
            && let Some(wt) = project.worktree()
        {
            let (start, start_label) = match (&wt.branch, wt.head) {
                (Some(b), _) => (b.clone(), b.clone()),
                (None, Some(id)) => (id.to_string(), graft_core::short(&id)),
                (None, None) => ("HEAD".to_owned(), "HEAD".to_owned()),
            };
            actions.push(Action::OpenDialog(DialogKind::NewBranch {
                name: String::new(),
                start,
                start_label,
                checkout: true,
            }));
        }
        tool_button(ui, None, "Merge", None, Tone::Normal).on_hover_text(LATER);
        let dirty = project.worktree().is_some_and(|wt| wt.dirty() > 0);
        if tool_button(ui, None, "Stash", None, Tone::Normal)
            .on_hover_text(if dirty {
                "Put this worktree's changes aside"
            } else {
                "Nothing to stash: the worktree is clean"
            })
            .clicked()
            && !busy
        {
            if dirty {
                actions.push(Action::OpenDialog(DialogKind::Stash {
                    message: String::new(),
                    include_untracked: true,
                    keep_index: false,
                    staged_only: false,
                }));
            } else {
                actions.push(Action::SetTab(Tab::Stashes));
            }
        }
        if tool_button(ui, None, "New worktree", None, Tone::Normal)
            .on_hover_text("Check out a branch in a second folder")
            .clicked()
            && !busy
        {
            actions.push(Action::NewWorktree(None));
        }
        toolbar_divider(ui);

        tool_button(ui, None, "Preview in main", None, Tone::Normal).on_hover_text(LATER);
        tool_button(ui, Some(Icon::Slash), "Slash", None, Tone::Danger).on_hover_text(LATER);

        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            search_box(ui, search)
        });
    });
}

fn search_box(ui: &mut Ui, text: &mut String) {
    let id = Id::unique("toolbar-search");
    if ui.input_mut(|i| i.consume_key(Modifiers::COMMAND, Key::K)) {
        ui.memory_mut(|m| m.request_focus(id));
    }
    let focused = ui.memory(|m| m.has_focus(id));
    let border = if focused {
        color::ACCENT
    } else {
        color::CONTROL_BORDER
    };
    let width = ui.available_width().clamp(160.0, 300.0);

    Frame::new()
        .fill(color::APP_BG)
        .stroke(Stroke::new(1.0, border))
        .corner_radius(RADIUS)
        .inner_margin(Margin::symmetric(8, 0))
        .show(ui, |ui| {
            ui.set_width(width - 16.0);
            ui.set_height(28.0);
            ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                let (rect, _) = ui.allocate_exact_size(vec2(14.0, 14.0), Sense::hover());
                icons::paint(ui.painter(), rect, Icon::Search, color::TEXT_WEAK);
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
}
