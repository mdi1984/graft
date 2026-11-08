//! Small dialogs: new branch, rename branch, new tag, stash, branch from a
//! stash, reset, add or edit a remote, and merge.

use egui::{RichText, Ui};
use graft_core::refs::ResetMode;

use super::widgets::dialog::{self, footer};
use super::widgets::{ButtonKind, error_label, field_label, note, text_field, title};
use crate::project::{Action, Dialog, DialogKind};
use crate::theme::{self, color};
use crate::ui::Actions;

const WIDTH: f32 = 440.0;

pub fn show(ctx: &egui::Context, dialog: &mut Dialog, actions: &mut Actions) {
    let busy = dialog.submitting;
    let shown = dialog::show(ctx, "ref-dialog", WIDTH, busy, |ui| {
        let focus = std::mem::take(&mut dialog.focus);
        // Any edit clears the last error.
        let mut edited = false;
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
                edited |= focused_field(ui, name, "e.g. feature/login", WIDTH, focus);
                edited |= ui.checkbox(checkout, "Switch to it").changed();
                "Create branch"
            }
            DialogKind::RenameBranch { old, name } => {
                title(ui, &format!("Rename {old}"));
                field_label(ui, "New name");
                edited |= focused_field(ui, name, old, WIDTH, focus);
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
                edited |= focused_field(ui, name, "e.g. v1.2.0", WIDTH, focus);
                field_label(ui, "Message (optional; makes an annotated tag)");
                edited |= text_field(ui, message, "Release notes, …", WIDTH).changed();
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
                edited |= focused_field(ui, message, "What you were doing", WIDTH, focus);
                ui.add_space(4.0);
                edited |= ui
                    .checkbox(include_untracked, "Include untracked files")
                    .changed();
                edited |= ui
                    .add_enabled_ui(!*staged_only, |ui| {
                        ui.checkbox(keep_index, "Keep staged changes in place too")
                            .changed()
                    })
                    .inner;
                edited |= ui
                    .checkbox(staged_only, "Only stash what is staged")
                    .changed();
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
                edited |= focused_field(ui, name, "e.g. wip/idea", WIDTH, focus);
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
                edited |= choice(
                    ui,
                    mode,
                    ResetMode::Soft,
                    "Soft",
                    "Keep their changes, staged",
                );
                edited |= choice(
                    ui,
                    mode,
                    ResetMode::Mixed,
                    "Mixed",
                    "Keep their changes, unstaged",
                );
                edited |= choice(
                    ui,
                    mode,
                    ResetMode::Hard,
                    "Hard",
                    "Throw away their changes and all uncommitted work",
                );
                if *mode == ResetMode::Hard {
                    ui.label(
                        RichText::new("Uncommitted changes in this worktree will be lost.")
                            .color(color::removed()),
                    );
                }
                "Reset"
            }
            DialogKind::Remote {
                original,
                name,
                url,
                push_only,
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
                edited |= focused_field(ui, name, "origin", WIDTH, focus);
                field_label(ui, if *push_only { "Push URL" } else { "URL" });
                edited |=
                    text_field(ui, url, "https://github.com/you/repository.git", WIDTH).changed();
                if original.is_none() {
                    ui.add_space(4.0);
                    edited |= ui
                        .checkbox(fetch, "Fetch its branches right away")
                        .changed();
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
                edited |= focused_field(ui, filter, "Filter branches", WIDTH, focus);
                let needle = filter.trim().to_lowercase();
                egui::Frame::new()
                    .fill(color::app_bg())
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
                                        edited = true;
                                    }
                                }
                                if shown == 0 {
                                    ui.label(
                                        RichText::new("No branch matches.")
                                            .color(color::text_weak()),
                                    );
                                }
                            });
                    });
                edited |= ui
                    .checkbox(no_ff, "Always create a merge commit (--no-ff)")
                    .changed();
                "Merge"
            }
        };
        if edited {
            dialog.error = None;
        }
        if let Some(error) = &dialog.error {
            error_label(ui, error);
        }
        footer(
            ui,
            confirm_label,
            submit_kind(&dialog.kind),
            true,
            busy,
            None,
        )
    });
    let answer = shown.answer();
    if answer.submit {
        actions.push(Action::SubmitDialog);
    }
    if answer.cancel {
        actions.push(Action::CloseDialog);
    }
}

/// How the dialog's main button looks: Danger for what throws work away
/// (a hard reset), which Enter then does not run.
fn submit_kind(kind: &DialogKind) -> ButtonKind {
    match kind {
        DialogKind::Reset {
            mode: ResetMode::Hard,
            ..
        } => ButtonKind::Danger,
        _ => ButtonKind::Primary,
    }
}

/// A text field that takes the focus when `focus`; returns whether it changed.
fn focused_field(ui: &mut Ui, text: &mut String, hint: &str, width: f32, focus: bool) -> bool {
    let field = text_field(ui, text, hint, width);
    if focus {
        field.request_focus();
    }
    field.changed()
}

/// A reset mode to pick; returns whether it was picked just now.
fn choice(
    ui: &mut Ui,
    value: &mut ResetMode,
    option: ResetMode,
    label: &str,
    detail: &str,
) -> bool {
    ui.horizontal(|ui| {
        let picked = ui.radio_value(value, option, RichText::new(label).color(color::text()));
        ui.label(RichText::new(detail).color(color::text_weak()));
        picked.changed()
    })
    .inner
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reset(mode: ResetMode) -> DialogKind {
        DialogKind::Reset {
            target: "abc".to_owned(),
            target_label: "abc".to_owned(),
            branch: "main".to_owned(),
            mode,
        }
    }

    /// Only a hard reset is Danger, so Enter cannot run it.
    #[test]
    fn hard_reset_is_danger() {
        assert_eq!(submit_kind(&reset(ResetMode::Hard)), ButtonKind::Danger);
        assert_eq!(submit_kind(&reset(ResetMode::Mixed)), ButtonKind::Primary);
        assert_eq!(submit_kind(&DialogKind::stash()), ButtonKind::Primary);
    }
}
