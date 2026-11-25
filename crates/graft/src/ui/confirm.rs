//! A modal question before destructive actions (discard, delete).

use egui::RichText;

use super::widgets::dialog::{self, footer};
use super::widgets::{ButtonKind, title};
use crate::project::{Action, Confirm};
use crate::theme::color;
use crate::ui::Actions;

/// A project's question; the answer goes back as an action.
pub fn show(ctx: &egui::Context, confirm: &Confirm, actions: &mut Actions) {
    let (title, body, label) = (&confirm.title, &confirm.body, &confirm.confirm_label);
    match ask(ctx, title, body, label, confirm.enter_confirms()) {
        Some(true) => actions.push(Action::ConfirmYes),
        Some(false) => actions.push(Action::ConfirmNo),
        None => {}
    }
}

/// Ask `title`; `Some(true)` once confirmed with `confirm_label` (or
/// Enter, with `enter`), `Some(false)` once cancelled, `None` while it is
/// open.
pub fn ask(
    ctx: &egui::Context,
    title_text: &str,
    body: &str,
    confirm_label: &str,
    enter: bool,
) -> Option<bool> {
    let shown = dialog::show(ctx, "confirm-modal", 420.0, false, |ui| {
        ui.spacing_mut().item_spacing.y = 8.0;
        title(ui, title_text);
        ui.add_space(2.0);
        ui.add(egui::Label::new(RichText::new(body).color(color::text_mid())).wrap());
        footer(ui, confirm_label, ButtonKind::Danger, true, false, None)
    });
    let shown = if enter { shown.enter_confirms() } else { shown };
    let answer = shown.answer();
    if answer.submit {
        Some(true)
    } else if answer.cancel {
        Some(false)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Press `keys` in turn, one frame each (keys of one slice in the same
    /// frame), with a discard question open; the answer after the last.
    fn answer_after(enter: bool, frames: &[&[egui::Key]]) -> Option<bool> {
        let ctx = egui::Context::default();
        let frame = |keys: &[egui::Key]| {
            let events = keys
                .iter()
                .map(|key| egui::Event::Key {
                    key: *key,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: egui::Modifiers::NONE,
                })
                .collect();
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(800.0, 600.0),
                )),
                events,
                ..Default::default()
            };
            let mut answer = None;
            let output = ctx.run_ui(input, |ui| {
                answer = ask(ui.ctx(), "Discard changes?", "a.txt", "Discard", enter);
            });
            output.drop_without_applying_deltas();
            answer
        };
        frame(&[]);
        frame(&[]);
        let mut answer = None;
        for keys in frames {
            answer = frame(keys);
        }
        answer
    }

    /// Enter confirms a question that allows it, and only that one. With
    /// Cancel focused (Tab), Enter clicks Cancel and nothing is discarded;
    /// Escape in the same frame as Enter cancels too.
    #[test]
    fn enter_confirms_only_where_allowed_and_never_over_cancel() {
        use egui::Key::{Enter, Escape, Tab};
        assert_eq!(answer_after(true, &[&[Enter]]), Some(true));
        assert_eq!(answer_after(false, &[&[Enter]]), None);
        assert_eq!(answer_after(true, &[&[Tab], &[Tab], &[Enter]]), Some(false));
        assert_eq!(answer_after(true, &[&[Escape, Enter]]), Some(false));
        assert_eq!(answer_after(true, &[&[Tab], &[Escape, Enter]]), Some(false));
    }
}
