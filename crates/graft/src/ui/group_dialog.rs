//! Naming a tab group: a new one (from a tab's menu) or a rename.

use std::path::PathBuf;

use egui::{Align, Id, Key, Layout, RichText};

use super::project_tabs::color_swatches;
use super::widgets::{ButtonKind, button, text_field};
use crate::groups::GroupId;
use crate::theme::{self, color};

const WIDTH: f32 = 400.0;

pub enum GroupTarget {
    /// A new group, starting with this repository.
    New(PathBuf),
    Rename(GroupId),
}

pub struct GroupForm {
    pub target: GroupTarget,
    pub name: String,
    pub color: usize,
    focus: bool,
}

impl GroupForm {
    pub fn new(target: GroupTarget, name: String, color: usize) -> Self {
        Self {
            target,
            name,
            color,
            focus: true,
        }
    }
}

pub enum Outcome {
    Open,
    Cancel,
    Save,
}

pub fn show(ctx: &egui::Context, form: &mut GroupForm) -> Outcome {
    let mut outcome = Outcome::Open;
    let new = matches!(form.target, GroupTarget::New(_));
    let response = super::widgets::modal(ctx, Id::unique("group-dialog")).show(ctx, |ui| {
        ui.set_width(WIDTH);
        ui.spacing_mut().item_spacing.y = 6.0;
        let title = if new { "New group" } else { "Rename group" };
        ui.label(
            RichText::new(title)
                .font(theme::semibold(15.0))
                .color(color::text()),
        );
        if new {
            ui.add(
                egui::Label::new(
                    RichText::new(
                        "Keeps repositories together, e.g. one customer's. Closing their tabs \
                         keeps the group, and the start page opens them all again.",
                    )
                    .color(color::text_weak()),
                )
                .wrap(),
            );
        }
        ui.add_space(2.0);
        ui.label(
            RichText::new("Name")
                .color(color::text_mid())
                .size(theme::SMALL_SIZE),
        );
        let field = text_field(ui, &mut form.name, "e.g. the customer's name", WIDTH);
        if std::mem::take(&mut form.focus) {
            field.request_focus();
        }
        ui.label(
            RichText::new("Color")
                .color(color::text_mid())
                .size(theme::SMALL_SIZE),
        );
        if let Some(c) = color_swatches(ui, form.color) {
            form.color = c;
        }
        let ready = !form.name.trim().is_empty();
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let label = if new { "Create group" } else { "Rename" };
                let enter = ui.input(|i| i.key_pressed(Key::Enter));
                if (button(ui, label, ButtonKind::Primary, ready).clicked() || enter) && ready {
                    outcome = Outcome::Save;
                }
                if button(ui, "Cancel", ButtonKind::Secondary, true).clicked() {
                    outcome = Outcome::Cancel;
                }
            });
        });
    });
    if response.should_close() || ctx.input(|i| i.key_pressed(Key::Escape)) {
        outcome = Outcome::Cancel;
    }
    outcome
}
