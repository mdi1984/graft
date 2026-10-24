//! Short messages in the bottom-right corner: errors from git, confirmations.

use egui::{Align2, Area, Frame, Id, Margin, Order, RichText, Sense, Stroke, vec2};

use crate::theme::{self, color};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Error,
    Info,
}

struct Toast {
    text: String,
    kind: Kind,
    /// egui time when it was shown, set on first paint.
    shown_at: Option<f64>,
}

#[derive(Default)]
pub struct Toasts {
    items: Vec<Toast>,
}

impl Toasts {
    pub fn error(&mut self, text: impl Into<String>) {
        self.push(text.into(), Kind::Error);
    }

    pub fn info(&mut self, text: impl Into<String>) {
        self.push(text.into(), Kind::Info);
    }

    fn push(&mut self, text: String, kind: Kind) {
        // The same message twice in a row is one message.
        if self.items.last().is_some_and(|t| t.text == text) {
            self.items.last_mut().expect("checked").shown_at = None;
            return;
        }
        self.items.push(Toast {
            text,
            kind,
            shown_at: None,
        });
        if self.items.len() > 4 {
            self.items.remove(0);
        }
    }

    pub fn show(&mut self, ctx: &egui::Context) {
        let now = ctx.input(|i| i.time);
        self.items.retain(|t| {
            let ttl = match t.kind {
                Kind::Error => 10.0,
                Kind::Info => 4.0,
            };
            t.shown_at.is_none_or(|at| now - at < ttl)
        });
        if self.items.is_empty() {
            return;
        }
        let mut dismiss = None;
        Area::new(Id::unique("toasts"))
            .anchor(Align2::RIGHT_BOTTOM, vec2(-16.0, -40.0))
            .order(Order::Foreground)
            .show(ctx, |ui| {
                ui.set_max_width(420.0);
                for (i, toast) in self.items.iter_mut().enumerate() {
                    toast.shown_at.get_or_insert(now);
                    let (accent, title) = match toast.kind {
                        Kind::Error => (color::removed(), "Something went wrong"),
                        Kind::Info => (color::accent(), "Done"),
                    };
                    let response = Frame::new()
                        .fill(color::control_bg())
                        .stroke(Stroke::new(1.0, accent.gamma_multiply(0.5)))
                        .corner_radius(8)
                        .inner_margin(Margin::symmetric(14, 10))
                        .show(ui, |ui| {
                            ui.set_min_width(260.0);
                            ui.label(
                                RichText::new(title)
                                    .font(theme::semibold(theme::BODY_SIZE))
                                    .color(accent),
                            );
                            ui.add(
                                egui::Label::new(RichText::new(&toast.text).color(color::text()))
                                    .wrap(),
                            );
                        })
                        .response
                        .interact(Sense::click())
                        .on_hover_text("Click to dismiss");
                    if response.clicked() {
                        dismiss = Some(i);
                    }
                    ui.add_space(6.0);
                }
            });
        if let Some(i) = dismiss {
            self.items.remove(i);
        }
        ctx.request_repaint_after(std::time::Duration::from_millis(500));
    }
}
