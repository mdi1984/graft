//! Naming a tab group: a new one (from a tab's menu) or a rename.

use std::path::PathBuf;

use super::widgets::dialog::{self, footer};
use super::widgets::{color_swatches, field_label, note, text_field, title};
use crate::groups::GroupId;

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

    fn ready(&self) -> bool {
        !self.name.trim().is_empty()
    }
}

pub enum Outcome {
    Open,
    Cancel,
    Save,
}

pub fn show(ctx: &egui::Context, form: &mut GroupForm) -> Outcome {
    let new = matches!(form.target, GroupTarget::New(_));
    let shown = dialog::show(ctx, "group-dialog", WIDTH, false, |ui| {
        title(ui, if new { "New group" } else { "Rename group" });
        if new {
            note(
                ui,
                "Keeps repositories together, e.g. one customer's. Closing their tabs keeps the                  group, and the start page opens them all again.",
            );
        }
        field_label(ui, "Name");
        let field = text_field(ui, &mut form.name, "e.g. the customer's name", WIDTH);
        if std::mem::take(&mut form.focus) {
            field.request_focus();
        }
        field_label(ui, "Color");
        if let Some(c) = color_swatches(ui, form.color) {
            form.color = c;
        }
        let label = if new { "Create group" } else { "Rename" };
        footer(ui, label, form.ready(), false, None)
    });
    if shown.cancel || shown.inner.cancel {
        Outcome::Cancel
    } else if (shown.inner.primary || shown.enter) && form.ready() {
        Outcome::Save
    } else {
        Outcome::Open
    }
}
