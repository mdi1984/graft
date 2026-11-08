//! The status bar at the bottom and the git console it opens: every git
//! command Graft ran, with exit code, duration and output.

use std::time::UNIX_EPOCH;

use egui::{Align, Color32, Layout, RichText, ScrollArea, Sense, Ui, pos2, vec2};
use graft_core::console::Entry;

use super::icons::Icon;
use super::widgets::{
    ICON_SIZE, elided, icon_button, paint_galley_left, paint_icon, paint_placeholder_left, spinner,
};
use crate::keys::{Command, Keymap};
use crate::theme::{self, color};

pub const STATUS_HEIGHT: f32 = 26.0;

/// The latest console entries, re-read only when the console changed.
/// Entries are told apart by their `seq`, which stays put when old ones
/// drop out of the console.
#[derive(Default)]
pub struct ConsoleView {
    next_seq: u64,
    pub entries: Vec<Entry>,
    /// Entries before this `seq` were seen (console open).
    seen: u64,
    /// Entries before this `seq` are hidden ("Clear").
    cleared: u64,
}

impl ConsoleView {
    pub fn sync(&mut self, console: &graft_core::console::Console) {
        let next_seq = console.next_seq();
        if next_seq != self.next_seq {
            self.next_seq = next_seq;
            self.entries = console.snapshot();
        }
    }

    pub fn unseen_failures(&self) -> usize {
        self.entries
            .iter()
            .filter(|e| e.seq >= self.seen && e.failed())
            .count()
    }

    pub fn mark_seen(&mut self) {
        self.seen = self.next_seq;
    }

    fn shown(&self) -> impl Iterator<Item = &Entry> {
        self.entries.iter().filter(|e| e.seq >= self.cleared)
    }
}

/// The bottom status bar: current activity on the left, console toggle right.
/// Returns true when the user clicked "Cancel" on a cancellable `activity`.
pub fn status_bar(
    ui: &mut Ui,
    activity: Option<&str>,
    cancellable: bool,
    view: &ConsoleView,
    console_open: &mut bool,
    keys: &Keymap,
) -> bool {
    let mut cancel = false;
    let (rect, _) =
        ui.allocate_exact_size(vec2(ui.available_width(), STATUS_HEIGHT), Sense::hover());
    let y = rect.center().y;
    let painter = ui.painter();
    let mut x = rect.left() + 4.0;
    if let Some(activity) = activity {
        spinner(ui, pos2(x + 5.0, y), 5.0, color::text_weak());
        x += 16.0;
        let text = elided(
            ui,
            activity,
            theme::small(),
            color::text_weak(),
            rect.width() / 2.0,
        );
        let width = text.size().x;
        paint_galley_left(painter, x, y, text);
        if cancellable {
            let galley =
                painter.layout_no_wrap("Cancel".to_owned(), theme::small(), Color32::PLACEHOLDER);
            let r = egui::Rect::from_min_size(
                pos2(x + width + 10.0, rect.top() + 4.0),
                vec2(galley.size().x + 8.0, rect.height() - 8.0),
            );
            let response = ui.interact(r, ui.scope_id().with("cancel-activity"), Sense::click());
            let fg = if response.hovered() {
                color::text()
            } else {
                color::ahead_fg()
            };
            paint_placeholder_left(painter, r.left() + 4.0, y, galley, fg);
            cancel = response.clicked();
        }
    } else if let Some(last) = view.entries.last() {
        let text = format!("Last: {}", last.command);
        let text = elided(
            ui,
            &text,
            theme::small(),
            color::text_faint(),
            rect.width() / 2.0,
        );
        paint_galley_left(painter, x, y, text);
    }

    let failures = view.unseen_failures();
    let label = if failures > 0 {
        format!("Console · {failures} failed")
    } else {
        "Console".to_owned()
    };
    let fg = if failures > 0 {
        color::removed()
    } else {
        color::text_weak()
    };
    let galley = painter.layout_no_wrap(label, theme::small(), fg);
    let width = galley.size().x + ICON_SIZE + 22.0;
    let button = egui::Rect::from_min_size(
        pos2(rect.right() - width, rect.top() + 2.0),
        vec2(width, rect.height() - 4.0),
    );
    let response = ui
        .interact(button, ui.scope_id().with("console-toggle"), Sense::click())
        .on_hover_text(keys.tip("Git commands Graft ran", Command::Console));
    if response.hovered() || *console_open {
        painter.rect_filled(button, 4, color::control_hover_bg());
    }
    paint_icon(painter, pos2(button.left() + 8.0, y), Icon::Terminal, fg);
    paint_galley_left(painter, button.left() + 14.0 + ICON_SIZE, y, galley);
    if response.clicked() {
        *console_open = !*console_open;
    }
    cancel
}

/// The console panel's content.
pub fn console(ui: &mut Ui, view: &mut ConsoleView, open: &mut bool) {
    view.mark_seen();
    ui.horizontal(|ui| {
        ui.label(
            RichText::new("Git console")
                .font(theme::semibold(theme::BODY_SIZE))
                .color(color::text()),
        );
        ui.label(
            RichText::new("Every git command Graft runs, newest at the bottom.")
                .color(color::text_faint())
                .size(theme::SMALL_SIZE),
        );
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if icon_button(ui, Icon::Close, 22.0)
                .on_hover_text("Close")
                .clicked()
            {
                *open = false;
            }
            if ui.button("Clear").clicked() {
                view.cleared = view.next_seq;
            }
        });
    });
    ui.separator();
    ScrollArea::vertical()
        .id_salt("console-scroll")
        .auto_shrink([false, false])
        .stick_to_bottom(true)
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 2.0;
            let mut any = false;
            for entry in view.shown() {
                any = true;
                entry_row(ui, entry);
            }
            if !any {
                ui.label(RichText::new("No commands yet.").color(color::text_faint()));
            }
        });
}

fn entry_row(ui: &mut Ui, entry: &Entry) {
    let id = ui.scope_id().with(("console-entry", entry.seq));
    let mut expanded = ui.data(|d| d.get_temp::<bool>(id)).unwrap_or(false);
    let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width(), 22.0), Sense::click());
    if response.hovered() {
        ui.painter().rect_filled(rect, 3, color::hover_row_bg());
    }
    let y = rect.center().y;
    let painter = ui.painter();
    let secs = entry
        .at
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let clock = jiff::Timestamp::from_second(secs)
        .map(|t| {
            t.to_zoned(jiff::tz::TimeZone::system())
                .strftime("%H:%M:%S")
                .to_string()
        })
        .unwrap_or_default();
    let mut x = rect.left() + 4.0;
    let time = painter.layout_no_wrap(clock, theme::mono(), color::text_faint());
    let time_width = time.size().x;
    paint_galley_left(painter, x, y, time);
    x += time_width + 10.0;
    let (mark, mark_color) = if entry.failed() {
        (format!("exit {}", entry.status), color::removed())
    } else {
        ("ok".to_owned(), color::added())
    };
    let mark = painter.layout_no_wrap(mark, theme::small(), mark_color);
    paint_galley_left(painter, x, y, mark);
    x += 48.0;
    let duration = painter.layout_no_wrap(
        format!("{} ms", entry.duration.as_millis()),
        theme::small(),
        color::text_faint(),
    );
    let duration_x = rect.right() - 6.0 - duration.size().x;
    paint_galley_left(painter, duration_x, y, duration);
    let command = elided(
        ui,
        &entry.command,
        theme::mono(),
        color::text_mid(),
        duration_x - 10.0 - x,
    );
    paint_galley_left(painter, x, y, command);
    if response
        .on_hover_text(format!("in {}", entry.cwd))
        .clicked()
    {
        expanded = !expanded;
        ui.data_mut(|d| d.insert_temp(id, expanded));
    }
    if expanded {
        let mut text = String::new();
        if !entry.stdout.trim().is_empty() {
            text.push_str(entry.stdout.trim_end());
        }
        if !entry.stderr.trim().is_empty() {
            if !text.is_empty() {
                text.push('\n');
            }
            text.push_str(entry.stderr.trim_end());
        }
        if text.is_empty() {
            text.push_str("(no output)");
        }
        egui::Frame::new()
            .fill(color::app_bg())
            .corner_radius(4)
            .inner_margin(egui::Margin::symmetric(10, 6))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.add(
                    egui::Label::new(
                        RichText::new(text)
                            .font(theme::mono())
                            .color(color::text_weak()),
                    )
                    .selectable(true)
                    .wrap(),
                );
            });
    }
}
