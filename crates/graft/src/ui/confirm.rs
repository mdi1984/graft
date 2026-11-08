//! A modal question before destructive actions (discard, delete).

use egui::RichText;

use super::widgets::dialog::{self, footer};
use super::widgets::{ButtonKind, title};
use crate::project::{Action, Confirm};
use crate::theme::color;
use crate::ui::Actions;

pub fn show(ctx: &egui::Context, confirm: &Confirm, actions: &mut Actions) {
    let shown = dialog::show(ctx, "confirm-modal", 420.0, false, |ui| {
        ui.spacing_mut().item_spacing.y = 8.0;
        title(ui, &confirm.title);
        ui.add_space(2.0);
        ui.add(egui::Label::new(RichText::new(&confirm.body).color(color::text_mid())).wrap());
        footer(
            ui,
            &confirm.confirm_label,
            ButtonKind::Danger,
            true,
            false,
            None,
        )
    });
    let answer = shown.answer();
    if answer.submit {
        actions.push(Action::ConfirmYes);
    } else if answer.cancel {
        actions.push(Action::ConfirmNo);
    }
}
