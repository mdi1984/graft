//! The New worktree dialog: a new or existing branch, the folder, and which
//! ignored files (like `.env`) to copy over.

use egui::{Align, ComboBox, Id, Layout, RichText, Ui};
use graft_core::RepoSnapshot;

use super::widgets::dialog::{self, footer};
use super::widgets::{ButtonKind, button, error_label, field_label, hint, note, text_field, title};
use crate::project::{Action, BranchMode, NewWorktreeForm, checkout_choices, start_choices};
use crate::theme;
use crate::ui::Actions;

const WIDTH: f32 = 520.0;

pub fn show(
    ctx: &egui::Context,
    form: &mut NewWorktreeForm,
    snap: &RepoSnapshot,
    actions: &mut Actions,
) {
    let shown = dialog::show(ctx, "new-worktree-modal", WIDTH, form.creating, |ui| {
        title(ui, "New worktree");
        note(
            ui,
            "A second checkout of this repository in its own folder, so two branches can be \
             worked on at the same time.",
        );
        ui.add_space(6.0);

        let before = (form.mode, form.name.clone(), form.existing.clone());
        ui.horizontal(|ui| {
            ui.selectable_value(&mut form.mode, BranchMode::New, "New branch");
            ui.selectable_value(&mut form.mode, BranchMode::Existing, "Existing branch");
        });
        ui.add_space(4.0);
        match form.mode {
            BranchMode::New => {
                field_label(ui, "Branch name");
                let name = text_field(ui, &mut form.name, "e.g. feature/login", WIDTH);
                if std::mem::take(&mut form.focus) {
                    name.request_focus();
                }
                field_label(ui, "Starts from");
                picker(ui, "wt-base", &mut form.base, &start_choices(snap));
            }
            BranchMode::Existing => {
                form.focus = false;
                field_label(ui, "Branch");
                let choices = checkout_choices(snap);
                if choices.is_empty() {
                    hint(ui, "Every branch is already checked out in a worktree.");
                } else {
                    picker(ui, "wt-existing", &mut form.existing, &choices);
                    if snap.remote_branches.iter().any(|b| b.name == form.existing) {
                        hint(ui, "Creates a local branch that tracks it.");
                    }
                }
            }
        }
        if before != (form.mode, form.name.clone(), form.existing.clone()) {
            form.error = None;
            form.sync_path(snap);
        }

        field_label(ui, "Folder");
        // Right to left: the field takes what the button leaves.
        ui.horizontal(|ui| {
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if button(ui, "Choose…", ButtonKind::Secondary, !form.creating).clicked() {
                    actions.push(Action::BrowseWorktreeFolder);
                }
                let width = ui.available_width();
                let path = text_field(ui, &mut form.path, "Where the worktree goes", width);
                if path.changed() {
                    form.path_edited = true;
                    form.error = None;
                }
            });
        });

        ui.add_space(4.0);
        ui.checkbox(&mut form.copy_ignored, "Copy ignored files into it");
        ui.add_enabled_ui(form.copy_ignored, |ui| {
            text_field(ui, &mut form.patterns, ".env, .env.*", WIDTH);
        });
        hint(
            ui,
            "Files git ignores but the app needs to run. Comma-separated names; * matches anything.",
        );

        if let Some(error) = &form.error {
            ui.add_space(2.0);
            error_label(ui, error);
        }
        let creating = form.creating;
        footer(
            ui,
            "Create worktree",
            ButtonKind::Primary,
            true,
            creating,
            Some("Creating…"),
        )
    });
    let answer = shown.answer();
    if answer.submit {
        actions.push(Action::CreateWorktree);
    }
    if answer.cancel {
        actions.push(Action::CloseNewWorktree);
    }
}

/// A dropdown of branch names with a filter box at the top.
fn picker(ui: &mut Ui, id: &str, value: &mut String, choices: &[String]) {
    let filter_id = Id::unique((id, "filter"));
    let selected = if value.is_empty() {
        "Pick a branch".to_owned()
    } else {
        value.clone()
    };
    ComboBox::from_id_salt(id)
        .selected_text(RichText::new(selected).font(theme::mono()))
        .width(WIDTH)
        .height(320.0)
        .show_ui(ui, |ui| {
            let mut filter: String = ui.data_mut(|d| d.get_temp(filter_id).unwrap_or_default());
            let response = ui.add(
                egui::TextEdit::singleline(&mut filter)
                    .hint_text("Filter")
                    .desired_width(f32::INFINITY),
            );
            if !response.has_focus() && filter.is_empty() {
                response.request_focus();
            }
            ui.data_mut(|d| d.insert_temp(filter_id, filter.clone()));
            let needle = filter.to_lowercase();
            for choice in choices
                .iter()
                .filter(|c| needle.is_empty() || c.to_lowercase().contains(&needle))
            {
                let text = RichText::new(choice).font(theme::mono());
                if ui.selectable_label(value == choice, text).clicked() {
                    *value = choice.clone();
                    ui.data_mut(|d| d.remove::<String>(filter_id));
                }
            }
        });
}
