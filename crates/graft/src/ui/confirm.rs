//! A modal question before destructive actions (discard, delete).

use egui::RichText;

use super::widgets::dialog::{self, footer};
use super::widgets::{ButtonKind, title};
use crate::project::{Action, Confirm};
use crate::theme::color;
use crate::ui::Actions;

/// A project's question; the answer goes back as an action.
pub fn show(ctx: &egui::Context, confirm: &Confirm, actions: &mut Actions) {
    match ask(ctx, &confirm.title, &confirm.body, &confirm.confirm_label) {
        Some(true) => actions.push(Action::ConfirmYes),
        Some(false) => actions.push(Action::ConfirmNo),
        None => {}
    }
}

/// Ask `title`; `Some(true)` once confirmed with `confirm_label`,
/// `Some(false)` once cancelled, `None` while it is open.
pub fn ask(ctx: &egui::Context, title_text: &str, body: &str, confirm_label: &str) -> Option<bool> {
    let shown = dialog::show(ctx, "confirm-modal", 420.0, false, |ui| {
        ui.spacing_mut().item_spacing.y = 8.0;
        title(ui, title_text);
        ui.add_space(2.0);
        ui.add(egui::Label::new(RichText::new(body).color(color::text_mid())).wrap());
        footer(ui, confirm_label, ButtonKind::Danger, true, false, None)
    });
    let answer = shown.answer();
    if answer.submit {
        Some(true)
    } else if answer.cancel {
        Some(false)
    } else {
        None
    }
}
