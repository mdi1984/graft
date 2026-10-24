//! The New worktree dialog: a new or existing branch, the folder, and which
//! ignored files (like `.env`) to copy over.

use egui::{Align, ComboBox, Id, Key, Layout, RichText, Ui};
use graft_core::RepoSnapshot;

use super::widgets::{ButtonKind, button, spinner, text_field};
use crate::project::{Action, BranchMode, NewWorktreeForm, checkout_choices};
use crate::theme::{self, color};

const WIDTH: f32 = 520.0;

pub fn show(
    ctx: &egui::Context,
    form: &mut NewWorktreeForm,
    snap: &RepoSnapshot,
    actions: &mut Vec<Action>,
) {
    let response = super::widgets::modal(ctx, Id::unique("new-worktree-modal")).show(ctx, |ui| {
        ui.set_width(WIDTH);
        ui.spacing_mut().item_spacing.y = 6.0;
        ui.label(
            RichText::new("New worktree")
                .font(theme::semibold(15.0))
                .color(color::text()),
        );
        ui.add(
            egui::Label::new(
                RichText::new(
                    "A second checkout of this repository in its own folder, so two \
                     branches can be worked on at the same time.",
                )
                .color(color::text_weak()),
            )
            .wrap(),
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
                let mut choices: Vec<String> = snap.branches.iter().map(|b| b.name.clone()).collect();
                choices.extend(snap.remote_branches.iter().map(|b| b.name.clone()));
                choices.extend(snap.tags.iter().map(|t| t.name.clone()));
                picker(ui, "wt-base", &mut form.base, &choices);
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
        ui.horizontal(|ui| {
            let browse_width = 92.0;
            let path = text_field(
                ui,
                &mut form.path,
                "Where the worktree goes",
                WIDTH - browse_width - 8.0,
            );
            if path.changed() {
                form.path_edited = true;
                form.error = None;
            }
            if button(ui, "Choose…", ButtonKind::Secondary, !form.creating).clicked() {
                actions.push(Action::BrowseWorktreeFolder);
            }
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
            ui.add(egui::Label::new(RichText::new(error).color(color::removed())).wrap());
        }
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let enter = ui.input(|i| i.key_pressed(Key::Enter));
                if (button(ui, "Create worktree", ButtonKind::Primary, !form.creating).clicked()
                    || enter)
                    && !form.creating
                {
                    actions.push(Action::CreateWorktree);
                }
                if button(ui, "Cancel", ButtonKind::Secondary, !form.creating).clicked() {
                    actions.push(Action::CloseNewWorktree);
                }
                if form.creating {
                    let (rect, _) = ui.allocate_exact_size(egui::vec2(14.0, 14.0), egui::Sense::hover());
                    spinner(ui, rect.center(), 5.0, color::text_weak());
                    ui.label(RichText::new("Creating…").color(color::text_weak()));
                }
            });
        });
    });
    if !form.creating && (response.should_close() || ctx.input(|i| i.key_pressed(Key::Escape))) {
        actions.push(Action::CloseNewWorktree);
    }
}

fn field_label(ui: &mut Ui, text: &str) {
    ui.add_space(2.0);
    ui.label(
        RichText::new(text)
            .color(color::text_mid())
            .size(theme::SMALL_SIZE),
    );
}

fn hint(ui: &mut Ui, text: &str) {
    ui.add(
        egui::Label::new(
            RichText::new(text)
                .color(color::text_faint())
                .size(theme::SMALL_SIZE),
        )
        .wrap(),
    );
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
