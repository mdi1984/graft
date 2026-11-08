//! Text and status pieces of dialogs and forms.

use egui::{Align2, Color32, RichText, Sense, Ui, vec2};

use super::spinner;
use crate::theme::{self, color};

/// The title of a dialog or a full-screen message.
pub fn title(ui: &mut Ui, text: &str) {
    ui.label(
        RichText::new(text)
            .font(theme::title())
            .color(color::text()),
    );
}

/// An explanation under a title, wrapped.
pub fn note(ui: &mut Ui, text: &str) {
    ui.add(egui::Label::new(RichText::new(text).color(color::text_weak())).wrap());
}

/// The small label above a field.
pub fn field_label(ui: &mut Ui, text: &str) {
    ui.add_space(2.0);
    ui.label(
        RichText::new(text)
            .color(color::text_mid())
            .size(theme::SMALL_SIZE),
    );
}

/// A small uppercase label over a group of settings or choices.
pub fn section_label(ui: &mut Ui, text: &str) {
    ui.label(
        RichText::new(text.to_uppercase())
            .color(color::text_faint())
            .size(theme::SMALL_SIZE),
    );
}

/// Small faint help text, wrapped.
pub fn hint(ui: &mut Ui, text: &str) {
    small_text(ui, text, color::text_faint());
}

/// Small text in `color`, wrapped.
pub fn small_text(ui: &mut Ui, text: &str, color: Color32) {
    ui.add(egui::Label::new(RichText::new(text).color(color).size(theme::SMALL_SIZE)).wrap());
}

/// What went wrong, wrapped.
pub fn error_label(ui: &mut Ui, text: &str) {
    ui.add(egui::Label::new(RichText::new(text).color(color::removed())).wrap());
}

/// A caution that is not an error, wrapped.
pub fn warning_label(ui: &mut Ui, text: &str) {
    ui.add(egui::Label::new(RichText::new(text).color(color::dirty_fg())).wrap());
}

/// A small spinner in the layout, e.g. next to a busy button.
pub fn inline_spinner(ui: &mut Ui) {
    let (rect, _) = ui.allocate_exact_size(vec2(14.0, 14.0), Sense::hover());
    spinner(ui, rect.center(), 5.0, color::text_weak());
}

/// A large spinner in the middle of the free space, with `text` under it.
pub fn loading(ui: &mut Ui, text: &str) {
    let rect = ui.available_rect_before_wrap();
    spinner(ui, rect.center() - vec2(0.0, 16.0), 9.0, color::text_weak());
    ui.painter().text(
        rect.center() + vec2(0.0, 10.0),
        Align2::CENTER_CENTER,
        text,
        theme::body(),
        color::text_weak(),
    );
}

/// A large spinner in the middle of the free space.
pub fn waiting(ui: &mut Ui) {
    let rect = ui.available_rect_before_wrap();
    spinner(ui, rect.center(), 9.0, color::text_weak());
}
