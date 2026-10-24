//! Small dialogs: new branch, rename branch, new tag, reset.

use egui::{Align, Id, Key, Layout, RichText, Ui};
use graft_core::refs::ResetMode;

use super::widgets::{ButtonKind, button, spinner, text_field};
use crate::project::{Action, Dialog, DialogKind};
use crate::theme::{self, color};

const WIDTH: f32 = 440.0;

pub fn show(ctx: &egui::Context, dialog: &mut Dialog, actions: &mut Vec<Action>) {
    let busy = dialog.submitting;
    let response = egui::Modal::new(Id::unique("ref-dialog")).show(ctx, |ui| {
        ui.set_width(WIDTH);
        ui.spacing_mut().item_spacing.y = 6.0;
        let focus = std::mem::take(&mut dialog.focus);
        let before = format!("{:?}", dialog.kind);
        let confirm_label = match &mut dialog.kind {
            DialogKind::NewBranch {
                name,
                start_label,
                checkout,
                ..
            } => {
                title(ui, "New branch");
                note(ui, &format!("Starts at {start_label}."));
                field_label(ui, "Name");
                let r = text_field(ui, name, "e.g. feature/login", WIDTH);
                if focus {
                    r.request_focus();
                }
                ui.checkbox(checkout, "Switch to it");
                "Create branch"
            }
            DialogKind::RenameBranch { old, name } => {
                title(ui, &format!("Rename {old}"));
                field_label(ui, "New name");
                let r = text_field(ui, name, old, WIDTH);
                if focus {
                    r.request_focus();
                }
                note(
                    ui,
                    "Its upstream setting moves along; the remote branch keeps its name.",
                );
                "Rename"
            }
            DialogKind::NewTag {
                name,
                message,
                target_label,
                ..
            } => {
                title(ui, "New tag");
                note(ui, &format!("Tags {target_label}."));
                field_label(ui, "Name");
                let r = text_field(ui, name, "e.g. v1.2.0", WIDTH);
                if focus {
                    r.request_focus();
                }
                field_label(ui, "Message (optional; makes an annotated tag)");
                text_field(ui, message, "Release notes, …", WIDTH);
                "Create tag"
            }
            DialogKind::Stash {
                message,
                include_untracked,
                keep_index,
                staged_only,
            } => {
                title(ui, "Stash changes");
                note(
                    ui,
                    "Puts the changes of this worktree aside and leaves it clean. \
                     Bring them back from the Stashes tab, here or in another worktree.",
                );
                field_label(ui, "Message (optional)");
                let r = text_field(ui, message, "What you were doing", WIDTH);
                if focus {
                    r.request_focus();
                }
                ui.add_space(4.0);
                ui.checkbox(include_untracked, "Include untracked files");
                ui.add_enabled_ui(!*staged_only, |ui| {
                    ui.checkbox(keep_index, "Keep staged changes in place too");
                });
                ui.checkbox(staged_only, "Only stash what is staged");
                if *staged_only {
                    *keep_index = false;
                }
                "Stash"
            }
            DialogKind::StashBranch { label, name, .. } => {
                title(ui, "New branch from stash");
                note(
                    ui,
                    &format!(
                        "Creates the branch at the commit {label} was made on, switches to it, \
                         applies the stash there and drops it. Handy when the stash no longer \
                         applies cleanly."
                    ),
                );
                field_label(ui, "Branch name");
                let r = text_field(ui, name, "e.g. wip/idea", WIDTH);
                if focus {
                    r.request_focus();
                }
                "Create branch"
            }
            DialogKind::Reset {
                target_label,
                branch,
                mode,
                ..
            } => {
                title(ui, &format!("Reset {branch} to {target_label}?"));
                note(
                    ui,
                    "Moves the branch to this commit. The commits after it disappear from \
                     the branch (they stay reachable for a while through the reflog).",
                );
                ui.add_space(4.0);
                choice(
                    ui,
                    mode,
                    ResetMode::Soft,
                    "Soft",
                    "Keep their changes, staged",
                );
                choice(
                    ui,
                    mode,
                    ResetMode::Mixed,
                    "Mixed",
                    "Keep their changes, unstaged",
                );
                choice(
                    ui,
                    mode,
                    ResetMode::Hard,
                    "Hard",
                    "Throw away their changes and all uncommitted work",
                );
                if *mode == ResetMode::Hard {
                    ui.label(
                        RichText::new("Uncommitted changes in this worktree will be lost.")
                            .color(color::REMOVED),
                    );
                }
                "Reset"
            }
            DialogKind::Remote {
                original,
                name,
                url,
                fetch,
            } => {
                match original {
                    None => {
                        title(ui, "Add remote");
                        note(
                            ui,
                            "A copy of this repository somewhere else, usually on GitHub or \
                             another server. Push and pull talk to it.",
                        );
                    }
                    Some(old) => title(ui, &format!("Edit remote {old}")),
                }
                field_label(ui, "Name");
                let r = text_field(ui, name, "origin", WIDTH);
                if focus {
                    r.request_focus();
                }
                field_label(ui, "URL");
                text_field(ui, url, "https://github.com/you/repository.git", WIDTH);
                if original.is_none() {
                    ui.add_space(4.0);
                    ui.checkbox(fetch, "Fetch its branches right away");
                    "Add remote"
                } else {
                    "Save"
                }
            }
            DialogKind::Merge {
                into,
                rev,
                choices,
                filter,
                no_ff,
            } => {
                title(ui, &format!("Merge into {into}"));
                note(
                    ui,
                    "Brings the commits of another branch into this one. If both changed \
                     the same lines, the merge stops and you resolve the conflicts first.",
                );
                field_label(ui, "Branch to merge");
                let r = text_field(ui, filter, "Filter branches", WIDTH);
                if focus {
                    r.request_focus();
                }
                let needle = filter.trim().to_lowercase();
                egui::Frame::new()
                    .fill(color::APP_BG)
                    .corner_radius(theme::RADIUS)
                    .inner_margin(4)
                    .show(ui, |ui| {
                        egui::ScrollArea::vertical()
                            .id_salt("merge-choices")
                            .max_height(220.0)
                            .auto_shrink([false, true])
                            .show(ui, |ui| {
                                ui.set_width(WIDTH - 8.0);
                                let mut shown = 0;
                                for choice in choices
                                    .iter()
                                    .filter(|c| c.to_lowercase().contains(&needle))
                                {
                                    shown += 1;
                                    let selected = rev == choice;
                                    let text = RichText::new(choice).font(theme::mono());
                                    let row = egui::Button::selectable(selected, text)
                                        .min_size(egui::vec2(ui.available_width(), 24.0));
                                    if ui.add(row).clicked() {
                                        *rev = choice.clone();
                                    }
                                }
                                if shown == 0 {
                                    ui.label(
                                        RichText::new("No branch matches.").color(color::TEXT_WEAK),
                                    );
                                }
                            });
                    });
                ui.checkbox(no_ff, "Always create a merge commit (--no-ff)");
                "Merge"
            }
        };
        if before != format!("{:?}", dialog.kind) {
            dialog.error = None;
        }
        if let Some(error) = &dialog.error {
            ui.add(egui::Label::new(RichText::new(error).color(color::REMOVED)).wrap());
        }
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let enter = ui.input(|i| i.key_pressed(Key::Enter));
                if (button(ui, confirm_label, ButtonKind::Primary, !busy).clicked() || enter)
                    && !busy
                {
                    actions.push(Action::SubmitDialog);
                }
                if button(ui, "Cancel", ButtonKind::Secondary, !busy).clicked() {
                    actions.push(Action::CloseDialog);
                }
                if busy {
                    let (rect, _) =
                        ui.allocate_exact_size(egui::vec2(14.0, 14.0), egui::Sense::hover());
                    spinner(ui, rect.center(), 5.0, color::TEXT_WEAK);
                }
            });
        });
    });
    if !busy && (response.should_close() || ctx.input(|i| i.key_pressed(Key::Escape))) {
        actions.push(Action::CloseDialog);
    }
}

fn title(ui: &mut Ui, text: &str) {
    ui.label(
        RichText::new(text)
            .font(theme::semibold(15.0))
            .color(color::TEXT),
    );
}

fn note(ui: &mut Ui, text: &str) {
    ui.add(egui::Label::new(RichText::new(text).color(color::TEXT_WEAK)).wrap());
}

fn field_label(ui: &mut Ui, text: &str) {
    ui.add_space(2.0);
    ui.label(
        RichText::new(text)
            .color(color::TEXT_MID)
            .size(theme::SMALL_SIZE),
    );
}

fn choice(ui: &mut Ui, value: &mut ResetMode, option: ResetMode, label: &str, detail: &str) {
    ui.horizontal(|ui| {
        ui.radio_value(value, option, RichText::new(label).color(color::TEXT));
        ui.label(RichText::new(detail).color(color::TEXT_WEAK));
    });
}
