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
    inner: R,
    /// Escape, or a click outside the dialog. Never while `busy`.
    pub cancel: bool,
    /// Enter, unless `busy`.
    enter: bool,
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

/// How a dialog with a [`footer`] was answered.
#[derive(Clone, Copy, Default)]
pub struct Answer {
    /// The main action: its button, or Enter (see [`Shown::answer`]).
    pub submit: bool,
    /// Its Cancel button, or (after [`Shown::answer`]) Escape or a click
    /// outside the dialog.
    pub cancel: bool,
    /// Enter may submit: the main button is enabled and not destructive.
    enter_submits: bool,
}

impl Shown<Answer> {
    /// The footer's answer with the keys and outside clicks merged in.
    pub fn answer(&self) -> Answer {
        Answer {
            submit: self.inner.submit || (self.enter && self.inner.enter_submits),
            cancel: self.inner.cancel || self.cancel,
            enter_submits: self.inner.enter_submits,
        }
    }
}

/// The footer: the main action (enabled when `ready`) and Cancel at the
/// right. While `busy` both are off and a spinner shows, with `busy_text`.
pub fn footer(
    ui: &mut Ui,
    primary: &str,
    kind: ButtonKind,
    ready: bool,
    busy: bool,
    busy_text: Option<&str>,
) -> Answer {
    footer_row(
        ui,
        |_| {},
        |ui| main_buttons(ui, primary, kind, ready, busy, busy_text),
    )
    .1
}

/// The main action and Cancel, right to left, for the right side of a
/// [`footer_row`]. Enter never answers a [`ButtonKind::Danger`] action.
pub fn main_buttons(
    ui: &mut Ui,
    primary: &str,
    kind: ButtonKind,
    ready: bool,
    busy: bool,
    busy_text: Option<&str>,
) -> Answer {
    let enabled = ready && !busy;
    let answer = Answer {
        submit: button(ui, primary, kind, enabled).clicked(),
        cancel: button(ui, "Cancel", ButtonKind::Secondary, !busy).clicked(),
        enter_submits: enabled && kind != ButtonKind::Danger,
    };
    if busy {
        inline_spinner(ui);
        if let Some(text) = busy_text {
            ui.label(RichText::new(text).color(color::text_weak()));
        }
    }
    answer
}

/// A dialog's bottom row: `left` from the left edge, `right` from the right
/// edge leftwards, so its first button sits in the corner. Returns what
/// both returned.
pub fn footer_row<L, R>(
    ui: &mut Ui,
    left: impl FnOnce(&mut Ui) -> L,
    right: impl FnOnce(&mut Ui) -> R,
) -> (L, R) {
    ui.add_space(8.0);
    ui.horizontal(|ui| {
        let left = left(ui);
        let right = ui.with_layout(Layout::right_to_left(Align::Center), right);
        (left, right.inner)
    })
    .inner
}
