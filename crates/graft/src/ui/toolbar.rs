//! The always-visible toolbar: Commit, Pull, Push, Fetch | Branch, Merge, Stash,
//! New worktree | Preview in main, Slash | search.

use egui::{Align, Frame, Id, Key, Layout, Margin, Modifiers, Sense, Stroke, TextEdit, Ui, vec2};
use graft_core::RepoSnapshot;

use super::icons::{self, Icon};
use super::widgets::{Tone, tool_button, toolbar_divider};
use crate::app::{Nav, Tab};
use crate::theme::{RADIUS, color};

const LATER: &str = "Not wired up yet";

pub fn show(ui: &mut Ui, repo: &RepoSnapshot, nav: &mut Nav, search: &mut String) {
    let ahead = repo.worktrees.get(nav.worktree).map_or(0, |wt| wt.ahead);

    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;

        if tool_button(ui, Some(Icon::Commit), "Commit", None, Tone::Normal).clicked() {
            nav.open_file_status();
        }
        tool_button(ui, Some(Icon::Pull), "Pull", None, Tone::Normal).on_hover_text(LATER);
        let push_badge = (ahead > 0).then(|| ahead.to_string());
        tool_button(ui, Some(Icon::Push), "Push", push_badge, Tone::Normal).on_hover_text(LATER);
        tool_button(ui, Some(Icon::Fetch), "Fetch", None, Tone::Normal).on_hover_text(LATER);
        toolbar_divider(ui);

        tool_button(ui, None, "Branch", None, Tone::Normal).on_hover_text(LATER);
        tool_button(ui, None, "Merge", None, Tone::Normal).on_hover_text(LATER);
        if tool_button(ui, None, "Stash", None, Tone::Normal).clicked() {
            nav.tab = Tab::Stashes;
        }
        tool_button(ui, None, "New worktree", None, Tone::Normal).on_hover_text(LATER);
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
