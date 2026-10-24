//! The About dialog: version, license, and updates.

use egui::{Align, Id, Key, Layout, RichText, Sense, Ui, vec2};

use super::widgets::{ButtonKind, button, spinner};
use super::window_frame::paint_app_icon;
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
    let response = super::widgets::modal(ctx, Id::unique("about-modal")).show(ctx, |ui| {
        ui.set_width(WIDTH);
        ui.spacing_mut().item_spacing.y = 6.0;
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

        ui.add_space(12.0);
        ui.horizontal(|ui| {
            if button(ui, "GitHub", ButtonKind::Secondary, true).clicked() {
                action = Some(AboutAction::Open(update::REPO_URL.to_owned()));
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if button(ui, "Close", ButtonKind::Secondary, true).clicked() {
                    action = Some(AboutAction::Close);
                }
            });
        });
    });
    if action.is_none() && (response.should_close() || ctx.input(|i| i.key_pressed(Key::Escape))) {
        action = Some(AboutAction::Close);
    }
    action
}

fn updates(ui: &mut Ui, status: &Status, action: &mut Option<AboutAction>) {
    let busy = |ui: &mut Ui, text: String| {
        ui.horizontal(|ui| {
            let (rect, _) = ui.allocate_exact_size(vec2(14.0, 14.0), Sense::hover());
            spinner(ui, rect.center(), 5.0, color::text_weak());
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
            ui.add(egui::Label::new(RichText::new(error).color(color::removed())).wrap());
            if button(ui, "Try again", ButtonKind::Secondary, true).clicked() {
                *action = Some(AboutAction::Check);
            }
        }
    }
}
