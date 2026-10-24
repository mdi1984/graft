//! The Settings dialog: theme, commit identity for the open repository,
//! editor, background fetch, Slash's protected branches, the files copied
//! into new worktrees and update checks.

use egui::{Align, ComboBox, Id, Key, Layout, RichText, Ui};
use graft_core::identity::Identity;

use super::widgets::{ButtonKind, button, spinner, text_field};
use crate::settings::{Settings, ThemeChoice};
use crate::theme::{self, color};

const WIDTH: f32 = 520.0;
const FETCH_CHOICES: [u32; 6] = [0, 5, 10, 15, 30, 60];

pub struct SettingsForm {
    pub draft: Settings,
    /// The identity as loaded, to tell whether it was edited.
    loaded: Option<Identity>,
    pub identity: Identity,
}

pub enum Outcome {
    Open,
    Cancel,
    /// Save the settings, and the identity if it changed.
    Save(Settings, Option<Identity>),
    /// Close without saving and show About.
    About,
}

impl SettingsForm {
    pub fn new(settings: &Settings) -> Self {
        Self {
            draft: settings.clone(),
            loaded: None,
            identity: Identity::default(),
        }
    }
}

/// `repo` is the open repository's name and its identity (`None` while
/// loading); `None` on the start page.
pub fn show(
    ctx: &egui::Context,
    form: &mut SettingsForm,
    repo: Option<(&str, Option<&Result<Identity, String>>)>,
) -> Outcome {
    if form.loaded.is_none()
        && let Some((_, Some(Ok(identity)))) = repo
    {
        form.loaded = Some(identity.clone());
        form.identity = identity.clone();
    }
    let mut outcome = Outcome::Open;
    let response = super::widgets::modal(ctx, Id::unique("settings-modal")).show(ctx, |ui| {
        ui.set_width(WIDTH);
        ui.spacing_mut().item_spacing.y = 6.0;
        ui.label(
            RichText::new("Settings")
                .font(theme::semibold(15.0))
                .color(color::text()),
        );

        section(ui, "Appearance");
        ui.horizontal(|ui| {
            for choice in ThemeChoice::ALL {
                ui.radio_value(&mut form.draft.theme, choice, choice.label());
                ui.add_space(8.0);
            }
        });

        if let Some((name, identity)) = repo {
            section(ui, "Commit identity");
            hint(
                ui,
                "Git records this name and email as the author of your commits.",
            );
            match identity {
                None => {
                    let (rect, _) =
                        ui.allocate_exact_size(egui::vec2(14.0, 14.0), egui::Sense::hover());
                    spinner(ui, rect.center(), 5.0, color::text_weak());
                }
                Some(Err(error)) => {
                    ui.label(RichText::new(error).color(color::removed()));
                }
                Some(Ok(_)) => {
                    let half = (WIDTH - 8.0) / 2.0;
                    ui.horizontal(|ui| {
                        ui.vertical(|ui| {
                            field_label(ui, "Name");
                            text_field(ui, &mut form.identity.name, "Ada Lovelace", half);
                        });
                        ui.vertical(|ui| {
                            field_label(ui, "Email");
                            text_field(ui, &mut form.identity.email, "ada@example.com", half);
                        });
                    });
                    ui.horizontal(|ui| {
                        ui.radio_value(&mut form.identity.local, false, "All repositories");
                        ui.radio_value(&mut form.identity.local, true, format!("Only {name}"));
                    });
                }
            }
        }

        section(ui, "Editor");
        field_label(ui, "Command that opens a folder or file");
        text_field(ui, &mut form.draft.ide_command, "code", WIDTH);
        hint(
            ui,
            "For example code, cursor, idea64 or rider64. If it does not start, Graft opens the \
             default program.",
        );

        section(ui, "Fetching");
        ui.horizontal(|ui| {
            ui.label(
                RichText::new("Fetch open repositories in the background").color(color::text()),
            );
            let label = |m: u32| match m {
                0 => "Never".to_owned(),
                m => format!("Every {m} minutes"),
            };
            ComboBox::from_id_salt("auto-fetch")
                .selected_text(label(form.draft.auto_fetch_minutes))
                .show_ui(ui, |ui| {
                    for m in FETCH_CHOICES {
                        ui.selectable_value(&mut form.draft.auto_fetch_minutes, m, label(m));
                    }
                });
        });
        hint(
            ui,
            "Keeps the ahead/behind counts and remote branches current.",
        );

        section(ui, "Slash");
        field_label(ui, "Branches Slash never deletes");
        text_field(
            ui,
            &mut form.draft.protected,
            "main, master, release/*",
            WIDTH,
        );
        hint(
            ui,
            "Comma-separated; * matches anything. The default branch is always kept.",
        );

        section(ui, "New worktrees");
        field_label(ui, "Ignored files to copy into new worktrees");
        text_field(ui, &mut form.draft.copy_patterns, ".env, .env.*", WIDTH);

        section(ui, "Updates");
        ui.horizontal(|ui| {
            ui.checkbox(
                &mut form.draft.check_updates,
                "Check for a new version when Graft starts",
            );
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if button(ui, "About Graft", ButtonKind::Secondary, true).clicked() {
                    outcome = Outcome::About;
                }
            });
        });

        ui.add_space(10.0);
        ui.horizontal(|ui| {
            if button(ui, "Restore defaults", ButtonKind::Secondary, true).clicked() {
                form.draft = Settings::default();
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if button(ui, "Save", ButtonKind::Primary, true).clicked() {
                    let identity = form
                        .loaded
                        .as_ref()
                        .filter(|loaded| **loaded != form.identity)
                        .map(|_| form.identity.clone());
                    outcome = Outcome::Save(form.draft.clone(), identity);
                }
                if button(ui, "Cancel", ButtonKind::Secondary, true).clicked() {
                    outcome = Outcome::Cancel;
                }
            });
        });
    });
    if matches!(outcome, Outcome::Open)
        && (response.should_close() || ctx.input(|i| i.key_pressed(Key::Escape)))
    {
        outcome = Outcome::Cancel;
    }
    outcome
}

fn section(ui: &mut Ui, title: &str) {
    ui.add_space(8.0);
    ui.label(
        RichText::new(title.to_uppercase())
            .color(color::text_faint())
            .size(theme::SMALL_SIZE),
    );
}

fn field_label(ui: &mut Ui, text: &str) {
    ui.label(
        RichText::new(text)
            .color(color::text_mid())
            .size(theme::SMALL_SIZE),
    );
}

fn hint(ui: &mut Ui, text: &str) {
    ui.add(
        egui::Label::new(
            RichText::new(text)
                .color(color::text_faint())
                .size(theme::SMALL_SIZE),
        )
        .wrap(),
    );
}
