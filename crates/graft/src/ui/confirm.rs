//! A modal question before destructive actions (discard, delete).

use egui::{Id, Key, RichText};

use super::widgets::{ButtonKind, button};
use crate::project::{Action, Confirm};
use crate::theme::{self, color};

pub fn show(ctx: &egui::Context, confirm: &Confirm, actions: &mut Vec<Action>) {
    let response = super::widgets::modal(ctx, Id::unique("confirm-modal")).show(ctx, |ui| {
        ui.set_width(420.0);
        ui.spacing_mut().item_spacing.y = 8.0;
        ui.label(
            RichText::new(&confirm.title)
                .font(theme::semibold(15.0))
                .color(color::TEXT),
        );
        ui.add_space(2.0);
        ui.add(egui::Label::new(RichText::new(&confirm.body).color(color::TEXT_MID)).wrap());
        ui.add_space(10.0);
        ui.horizontal(|ui| {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if danger_button(ui, &confirm.confirm_label).clicked() {
                    actions.push(Action::ConfirmYes);
                }
                if button(ui, "Cancel", ButtonKind::Secondary, true).clicked() {
                    actions.push(Action::ConfirmNo);
                }
            });
        });
    });
    if response.should_close() || ctx.input(|i| i.key_pressed(Key::Escape)) {
        actions.push(Action::ConfirmNo);
    }
}

fn danger_button(ui: &mut egui::Ui, label: &str) -> egui::Response {
    let text = ui.painter().layout_no_wrap(
        label.to_owned(),
        theme::semibold(theme::BODY_SIZE),
        color::ON_ACCENT,
    );
    let size = egui::vec2(text.size().x + 28.0, 30.0);
    let (rect, response) = ui.allocate_exact_size(size, egui::Sense::click());
    let fill = if response.hovered() {
        color::REMOVED.gamma_multiply(1.1)
    } else {
        color::REMOVED
    };
    ui.painter().rect_filled(rect, theme::RADIUS, fill);
    ui.painter()
        .galley(rect.center() - text.size() / 2.0, text, color::ON_ACCENT);
    response
}
