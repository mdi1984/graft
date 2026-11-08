//! The Settings dialog: theme, commit identity for the open repository,
//! editor, background fetch, Slash's protected branches, the files copied
//! into new worktrees, keyboard shortcuts and update checks.

use egui::{Align, ComboBox, Key, Layout, RichText, ScrollArea, Ui};
use graft_core::identity::Identity;

use super::widgets::dialog::{self, footer_row, main_buttons};
use super::widgets::{
    ButtonKind, button, error_label, field_label, hint, inline_spinner, section_label, text_field,
    title,
};
use crate::keys::{self, Command, Keymap, Scope};
use crate::settings::{Settings, ThemeChoice};
use crate::theme::{self, color};

const WIDTH: f32 = 520.0;
const FETCH_CHOICES: [u32; 6] = [0, 5, 10, 15, 30, 60];

pub struct SettingsForm {
    pub draft: Settings,
    /// The action whose new shortcut is being recorded.
    recording: Option<Command>,
    /// Why the last recorded shortcut was not taken.
    key_error: Option<String>,
    /// The identity as loaded, to tell whether it was edited.
    loaded: Option<Identity>,
    identity: Identity,
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
            recording: None,
            key_error: None,
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
    let shown = dialog::show(ctx, "settings-modal", WIDTH, false, |ui| {
        title(ui, "Settings");

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
                None => inline_spinner(ui),
                Some(Err(error)) => error_label(ui, error),
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

        section(ui, "Diffs");
        ui.checkbox(&mut form.draft.wrap_lines, "Wrap long lines");
        let switch = switches(&form.draft.keys, Command::WrapLines);
        hint(
            ui,
            &format!("Long lines continue on the next row instead of scrolling sideways.{switch}"),
        );
        ui.checkbox(&mut form.draft.whole_file, "Show whole files");
        let switch = switches(&form.draft.keys, Command::WholeFile);
        hint(
            ui,
            &format!(
                "Diffs show every line, not just the changes and a few lines around them.{switch}"
            ),
        );

        section(ui, "Review");
        ui.checkbox(&mut form.draft.review_folders, "Show files in folders");
        hint(
            ui,
            "Review lists the files under their folders instead of one flat list. The link \
             above Review's file list switches it too.",
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

        section(ui, "Keyboard");
        egui::CollapsingHeader::new(RichText::new("Shortcuts").color(color::text()))
            .id_salt("settings-shortcuts")
            .show(ui, |ui| shortcuts(ui, form));

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

        let (restore, answer) = footer_row(
            ui,
            |ui| button(ui, "Restore defaults", ButtonKind::Secondary, true).clicked(),
            |ui| main_buttons(ui, "Save", ButtonKind::Primary, true, false, None),
        );
        if restore {
            form.draft = Settings::default();
        } else if answer.submit {
            let identity = form
                .loaded
                .as_ref()
                .filter(|loaded| **loaded != form.identity)
                .map(|_| form.identity.clone());
            outcome = Outcome::Save(form.draft.clone(), identity);
        } else if answer.cancel {
            outcome = Outcome::Cancel;
        }
    });
    if matches!(outcome, Outcome::Open) && shown.cancel {
        outcome = Outcome::Cancel;
    }
    outcome
}

/// Every action with its shortcut; click one to record a new one.
fn shortcuts(ui: &mut Ui, form: &mut SettingsForm) {
    if let Some(command) = form.recording {
        let pressed = ui.input(keys::recorded);
        // Keys go to the recorder only: nothing else may act on them.
        ui.input_mut(|i| i.events.retain(|e| !matches!(e, egui::Event::Key { .. })));
        match pressed {
            Some(s) if s.modifiers.is_none() && s.logical_key == Key::Escape => {
                form.recording = None;
            }
            Some(shortcut) => {
                form.recording = None;
                form.key_error = form.draft.keys.refusal(command, shortcut);
                if form.key_error.is_none() {
                    form.draft.keys.set(command, Some(shortcut));
                }
            }
            None => {}
        }
    }
    hint(
        ui,
        "Click a shortcut, then press the new keys (Esc cancels). Shortcuts without Ctrl or Alt \
         don't fire while you type.",
    );
    if let Some(error) = &form.key_error {
        error_label(ui, error);
    }
    ScrollArea::vertical()
        .max_height(260.0)
        .auto_shrink([false, true])
        .show(ui, |ui| {
            for scope in [Scope::General, Scope::FileStatus, Scope::Review] {
                ui.add_space(4.0);
                field_label(ui, scope.label());
                for command in Command::ALL.into_iter().filter(|c| c.scope() == scope) {
                    shortcut_row(ui, form, command);
                }
            }
        });
}

fn shortcut_row(ui: &mut Ui, form: &mut SettingsForm, command: Command) {
    ui.horizontal(|ui| {
        ui.label(RichText::new(command.label()).color(color::text()));
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            let current = form.draft.keys.get(command);
            let changed = current != command.default_shortcut();
            if ui
                .add_enabled(changed, egui::Button::new("Reset").small())
                .on_hover_text(match command.default_shortcut() {
                    Some(s) => format!("Back to {}", keys::format(s)),
                    None => "Back to no shortcut".to_owned(),
                })
                .clicked()
            {
                // The default may be in use by now, too.
                let default = command.default_shortcut();
                form.key_error = default.and_then(|s| form.draft.keys.refusal(command, s));
                if form.key_error.is_none() {
                    form.draft.keys.set(command, default);
                }
            }
            if ui
                .add_enabled(current.is_some(), egui::Button::new("Clear").small())
                .on_hover_text("No shortcut for this")
                .clicked()
            {
                form.draft.keys.set(command, None);
                form.key_error = None;
            }
            let recording = form.recording == Some(command);
            let text = if recording {
                "Press keys…".to_owned()
            } else {
                current.map_or_else(|| "None".to_owned(), keys::format)
            };
            let text = RichText::new(text).font(theme::mono()).color(if recording {
                color::accent()
            } else {
                color::text()
            });
            if ui
                .add(egui::Button::new(text).min_size(egui::vec2(120.0, 0.0)))
                .on_hover_text("Click, then press the new keys")
                .clicked()
            {
                form.recording = if recording { None } else { Some(command) };
                form.key_error = None;
            }
        });
    });
}

fn section(ui: &mut Ui, title: &str) {
    ui.add_space(8.0);
    section_label(ui, title);
}

/// " Alt+Z switches it anywhere.", if `command` has a shortcut.
fn switches(keys: &Keymap, command: Command) -> String {
    keys.text(command)
        .map(|key| format!(" {key} switches it anywhere."))
        .unwrap_or_default()
}
