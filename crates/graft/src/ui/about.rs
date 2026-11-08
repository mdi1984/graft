//! The About dialog: version, license, and updates.

use egui::{Align, Layout, RichText, Sense, Ui, vec2};

use super::icons::paint_app_icon;
use super::widgets::dialog::{self, footer_row};
use super::widgets::{ButtonKind, button, error_label, inline_spinner};
use crate::theme::{self, color};
use crate::update::{self, Status};

const WIDTH: f32 = 420.0;

pub enum AboutAction {
    Check,
    Install,
    /// Open a web page (the release notes, the repository).
    Open(String),
    Close,
}

pub fn show(ctx: &egui::Context, status: &Status) -> Option<AboutAction> {
    let mut action = None;
    let shown = dialog::show(ctx, "about-modal", WIDTH, false, |ui| {
        ui.horizontal(|ui| {
            let (rect, _) = ui.allocate_exact_size(vec2(48.0, 48.0), Sense::hover());
            paint_app_icon(ui, rect);
            ui.add_space(8.0);
            ui.vertical(|ui| {
                ui.label(
                    RichText::new("Graft")
                        .font(theme::semibold(18.0))
                        .color(color::text()),
                );
                ui.label(
                    RichText::new(format!("Version {}", update::CURRENT)).color(color::text_mid()),
                );
            });
        });
        ui.add_space(4.0);
        ui.label(RichText::new("An opinionated git client for Windows.").color(color::text()));
        ui.label(
            RichText::new("MIT License")
                .color(color::text_faint())
                .size(theme::SMALL_SIZE),
        );

        ui.add_space(10.0);
        updates(ui, status, &mut action);

        ui.add_space(4.0);
        let (github, close) = footer_row(
            ui,
            |ui| button(ui, "GitHub", ButtonKind::Secondary, true).clicked(),
            |ui| button(ui, "Close", ButtonKind::Secondary, true).clicked(),
        );
        if github {
            action = Some(AboutAction::Open(update::REPO_URL.to_owned()));
        } else if close {
            action = Some(AboutAction::Close);
        }
    });
    if action.is_none() && shown.cancel {
        action = Some(AboutAction::Close);
    }
    action
}

fn updates(ui: &mut Ui, status: &Status, action: &mut Option<AboutAction>) {
    let busy = |ui: &mut Ui, text: String| {
        ui.horizontal(|ui| {
            inline_spinner(ui);
            ui.label(RichText::new(text).color(color::text_mid()));
        });
    };
    match status {
        Status::Idle => {
            if button(ui, "Check for updates", ButtonKind::Secondary, true).clicked() {
                *action = Some(AboutAction::Check);
            }
        }
        Status::Checking => busy(ui, "Checking for a new version…".to_owned()),
        Status::UpToDate => {
            ui.horizontal(|ui| {
                ui.label(RichText::new("Graft is up to date.").color(color::added()));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if button(ui, "Check again", ButtonKind::Secondary, true).clicked() {
                        *action = Some(AboutAction::Check);
                    }
                });
            });
        }
        Status::Available(release) => {
            ui.label(
                RichText::new(format!("Graft {} is available.", release.version))
                    .font(theme::semibold(theme::BODY_SIZE))
                    .color(color::text()),
            );
            ui.horizontal(|ui| {
                if button(ui, "Update and restart", ButtonKind::Primary, true).clicked() {
                    *action = Some(AboutAction::Install);
                }
                if button(ui, "What's new", ButtonKind::Secondary, true).clicked() {
                    *action = Some(AboutAction::Open(release.page.clone()));
                }
            });
        }
        Status::Installing(release) => busy(ui, format!("Downloading Graft {}…", release.version)),
        Status::Failed(error) => {
            error_label(ui, error);
            if button(ui, "Try again", ButtonKind::Secondary, true).clicked() {
                *action = Some(AboutAction::Check);
            }
        }
    }
}
