//! The frame every modal dialog shares: width, spacing, the footer with the
//! main action and Cancel, and which keys reach it.
//!
//! Escape and Enter go to the topmost dialog only, and not while one of its
//! popups (a dropdown, say) is open: that popup gets them.

use egui::{Align, Id, Key, Layout, Modifiers, RichText, Ui};

use super::form::inline_spinner;
use super::{ButtonKind, button};
use crate::theme::color;

pub struct Shown<R> {
    /// What the content returned.
    pub inner: R,
    /// Escape, or a click outside the dialog. Never while `busy`.
    pub cancel: bool,
    /// Enter, unless `busy`.
    pub enter: bool,
}

/// Show a dialog `width` wide with `add` as its content. While `busy`
/// (its action is running), Escape, Enter and outside clicks do nothing.
pub fn show<R>(
    ctx: &egui::Context,
    id: &str,
    width: f32,
    busy: bool,
    add: impl FnOnce(&mut Ui) -> R,
) -> Shown<R> {
    // egui's default frame uses the tight menu padding; dialogs get more room.
    let frame = egui::Frame::popup(&ctx.global_style()).inner_margin(egui::Margin {
        left: 22,
        right: 22,
        top: 18,
        bottom: 20,
    });
    let response = egui::Modal::new(Id::unique(id))
        .frame(frame)
        .show(ctx, |ui| {
            ui.set_width(width);
            ui.spacing_mut().item_spacing.y = 6.0;
            add(ui)
        });
    let has_keys = response.is_top_modal && !response.any_popup_open;
    let cancel = !busy && response.should_close();
    let enter = !busy && has_keys && ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Enter));
    Shown {
        inner: response.inner,
        cancel,
        enter,
    }
}

/// Which footer button was clicked.
#[derive(Clone, Copy, Default)]
pub struct Clicked {
    pub primary: bool,
    pub cancel: bool,
}

/// The footer: the main action (enabled when `ready`) and Cancel at the
/// right. While `busy` both are off and a spinner shows, with `busy_text`.
pub fn footer(
    ui: &mut Ui,
    primary: &str,
    ready: bool,
    busy: bool,
    busy_text: Option<&str>,
) -> Clicked {
    let mut clicked = Clicked::default();
    ui.add_space(8.0);
    ui.horizontal(|ui| {
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            clicked.primary = button(ui, primary, ButtonKind::Primary, ready && !busy).clicked();
            clicked.cancel = button(ui, "Cancel", ButtonKind::Secondary, !busy).clicked();
            if busy {
                inline_spinner(ui);
                if let Some(text) = busy_text {
                    ui.label(RichText::new(text).color(color::text_weak()));
                }
            }
        });
    });
    clicked
}
