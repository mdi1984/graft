//! The Settings window: the pages in a list on the left (Appearance,
//! General, Diffs & Review, Fetch, Slash & worktrees, Keyboard, and the open
//! repository's commit identity), the chosen page on the right, and a search
//! that shows the matching settings of every page.

use egui::{
    Align, Align2, ComboBox, CursorIcon, Key, Layout, PopupCloseBehavior, RichText, ScrollArea,
    Sense, Slider, Ui, vec2,
};
use graft_core::identity::Identity;

use super::widgets::dialog::{self, footer_row, main_buttons};
use super::widgets::{
    ButtonKind, button, error_label, field_label, hint, inline_spinner, paint_row_bg,
    section_label, text_field, title,
};
use crate::fonts::{Family, FontChoice, Kind, STYLES};
use crate::keys::{self, Command, Keymap, Scope};
use crate::settings::{Settings, ThemeChoice, ZOOM_MAX, ZOOM_MIN};
use crate::theme::{self, RADIUS, color};

const WIDTH: f32 = 780.0;
const NAV_WIDTH: f32 = 190.0;
const NAV_ROW: f32 = 30.0;
const BODY_HEIGHT: f32 = 480.0;
const FETCH_CHOICES: [u32; 6] = [0, 5, 10, 15, 30, 60];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Page {
    Appearance,
    General,
    Diffs,
    Remote,
    Keyboard,
    /// The open repository's commit identity.
    Identity,
}

impl Page {
    /// The pages about Graft, as the list shows them.
    const APP: [Page; 5] = [
        Page::Appearance,
        Page::General,
        Page::Diffs,
        Page::Remote,
        Page::Keyboard,
    ];

    fn label(self) -> &'static str {
        match self {
            Page::Appearance => "Appearance",
            Page::General => "General",
            Page::Diffs => "Diffs & Review",
            Page::Remote => "Fetch, Slash & worktrees",
            Page::Keyboard => "Keyboard",
            Page::Identity => "Commit identity",
        }
    }

    /// Put this page's settings back to the defaults.
    fn restore(self, draft: &mut Settings) {
        let d = Settings::default();
        match self {
            Page::Appearance => {
                draft.theme = d.theme;
                draft.ui_font = d.ui_font;
                draft.code_font = d.code_font;
                draft.zoom = d.zoom;
            }
            Page::General => {
                draft.ide_command = d.ide_command;
                draft.check_updates = d.check_updates;
            }
            Page::Diffs => {
                draft.wrap_lines = d.wrap_lines;
                draft.whole_file = d.whole_file;
                draft.review_folders = d.review_folders;
            }
            Page::Remote => {
                draft.auto_fetch_minutes = d.auto_fetch_minutes;
                draft.protected = d.protected;
                draft.copy_patterns = d.copy_patterns;
            }
            Page::Keyboard => draft.keys = d.keys,
            // Git keeps the identity, not Graft: there is no default.
            Page::Identity => {}
        }
    }
}

pub struct SettingsForm {
    pub draft: Settings,
    page: Page,
    search: String,
    /// The action whose new shortcut is being recorded.
    recording: Option<Command>,
    /// Why the last recorded shortcut was not taken.
    key_error: Option<String>,
    /// The identity as loaded, to tell whether it was edited.
    loaded: Option<Identity>,
    identity: Identity,
    /// Families from font files added while the window is open.
    added_fonts: Vec<Family>,
    /// Why the last font file could not be added, and for which font.
    font_error: Option<(Kind, String)>,
    /// What the font lists are narrowed to.
    font_filter: String,
    /// The zoom while its slider is dragged: it applies when let go, so the
    /// slider doesn't move under the mouse.
    zoom_dragged: Option<u32>,
}

pub enum Outcome {
    Open,
    Cancel,
    /// Save the settings, and the identity if it changed.
    Save(Box<Settings>, Option<Identity>),
    /// Close without saving and show About.
    About,
    /// Ask for a font file for this font.
    AddFontFile(Kind),
}

impl SettingsForm {
    pub fn new(settings: &Settings) -> Self {
        Self {
            draft: settings.clone(),
            page: Page::Appearance,
            search: String::new(),
            recording: None,
            key_error: None,
            loaded: None,
            identity: Identity::default(),
            added_fonts: Vec::new(),
            font_error: None,
            font_filter: String::new(),
            zoom_dragged: None,
        }
    }

    fn font_mut(&mut self, kind: Kind) -> &mut FontChoice {
        match kind {
            Kind::Ui => &mut self.draft.ui_font,
            Kind::Code => &mut self.draft.code_font,
        }
    }

    /// A font file the user picked for `kind`: its families, or why it
    /// can't be used. The first fitting family becomes the font.
    pub fn font_file_added(&mut self, kind: Kind, picked: Result<Vec<Family>, String>) {
        let families = match picked {
            Ok(families) => families,
            Err(error) => {
                self.font_error = Some((kind, error));
                return;
            }
        };
        let fitting: Vec<Family> = families
            .into_iter()
            .filter(|f| kind == Kind::Ui || f.monospace)
            .collect();
        let Some(first) = fitting.first() else {
            self.font_error = Some((
                kind,
                "Its letters are not all equally wide, so it can't be the code font.".to_owned(),
            ));
            return;
        };
        self.font_error = None;
        *self.font_mut(kind) = first.choice();
        for family in fitting {
            if !self.added_fonts.contains(&family) {
                self.added_fonts.push(family);
            }
        }
    }
}

/// What the sections need besides the form.
struct View<'a> {
    /// The open repository's name and identity (`None` while loading).
    repo: Option<(&'a str, Option<&'a Result<Identity, String>>)>,
    /// The fonts on this PC; `None` while Graft looks for them.
    installed: Option<&'a [Family]>,
    /// The search, in lowercase words.
    terms: Vec<String>,
    /// Width of the page's content.
    width: f32,
    outcome: Outcome,
}

/// One group of settings on a page, which the search can find.
struct Section {
    page: Page,
    title: &'static str,
    /// More words the search finds it by.
    words: &'static str,
    show: fn(&mut Ui, &mut SettingsForm, &mut View),
}

const SECTIONS: [Section; 13] = [
    Section {
        page: Page::Appearance,
        title: "Theme",
        words: "dark light windows mode colors",
        show: theme_section,
    },
    Section {
        page: Page::Appearance,
        title: "Font style",
        words: "fonts inter segoe consolas cascadia jetbrains look",
        show: style_section,
    },
    Section {
        page: Page::Appearance,
        title: "Fonts",
        words: "font interface code monospace typeface file inter jetbrains",
        show: fonts_section,
    },
    Section {
        page: Page::Appearance,
        title: "Zoom",
        words: "scale size bigger smaller text dpi",
        show: zoom_section,
    },
    Section {
        page: Page::General,
        title: "Editor",
        words: "ide open code cursor rider command",
        show: editor_section,
    },
    Section {
        page: Page::General,
        title: "Updates",
        words: "version new about check",
        show: updates_section,
    },
    Section {
        page: Page::Diffs,
        title: "Diffs",
        words: "wrap long lines whole files context",
        show: diffs_section,
    },
    Section {
        page: Page::Diffs,
        title: "Review",
        words: "folders tree files list",
        show: review_section,
    },
    Section {
        page: Page::Remote,
        title: "Fetching",
        words: "fetch background remote minutes ahead behind",
        show: fetch_section,
    },
    Section {
        page: Page::Remote,
        title: "Slash",
        words: "protected branches delete cleanup",
        show: slash_section,
    },
    Section {
        page: Page::Remote,
        title: "New worktrees",
        words: "copy ignored files env worktree",
        show: worktrees_section,
    },
    Section {
        page: Page::Keyboard,
        title: "Shortcuts",
        words: "keys keyboard hotkeys",
        show: shortcuts_section,
    },
    Section {
        page: Page::Identity,
        title: "Commit identity",
        words: "name email author user",
        show: identity_section,
    },
];

impl Section {
    fn matches(&self, terms: &[String]) -> bool {
        let mut text = format!("{} {} {}", self.page.label(), self.title, self.words);
        if self.page == Page::Keyboard {
            for command in Command::ALL {
                text.push(' ');
                text.push_str(command.label());
            }
        }
        let text = text.to_lowercase();
        terms.iter().all(|term| text.contains(term.as_str()))
    }
}

/// `repo` is the open repository's name and its identity (`None` while
/// loading); `None` on the start page. `installed` is the fonts on this PC,
/// `None` while Graft looks for them.
pub fn show(
    ctx: &egui::Context,
    form: &mut SettingsForm,
    repo: Option<(&str, Option<&Result<Identity, String>>)>,
    installed: Option<&[Family]>,
) -> Outcome {
    if form.loaded.is_none()
        && let Some((_, Some(Ok(identity)))) = repo
    {
        form.loaded = Some(identity.clone());
        form.identity = identity.clone();
    }
    if repo.is_none() && form.page == Page::Identity {
        form.page = Page::Appearance;
    }
    // At a high zoom the window must still fit.
    let screen = ctx.content_rect();
    let width = WIDTH.min(screen.width() - 64.0).max(480.0);
    let body_height = BODY_HEIGHT.min(screen.height() - 180.0).max(160.0);
    let mut view = View {
        repo,
        installed,
        terms: Vec::new(),
        width: 0.0,
        outcome: Outcome::Open,
    };
    let shown = dialog::show(ctx, "settings-modal", width, false, |ui| {
        // A fixed height: pages of any length keep the window still.
        let row = vec2(ui.available_width(), body_height);
        ui.allocate_ui_with_layout(row, Layout::left_to_right(Align::Min), |ui| {
            ui.set_height(body_height);
            ui.vertical(|ui| nav(ui, form, &view));
            ui.add_space(12.0);
            let (line, _) = ui.allocate_exact_size(vec2(1.0, body_height), Sense::hover());
            ui.painter().rect_filled(line, 0.0, color::border());
            ui.add_space(16.0);
            let body_width = ui.available_width();
            view.width = body_width - 16.0;
            view.terms = form
                .search
                .split_whitespace()
                .map(str::to_lowercase)
                .collect();
            ui.vertical(|ui| {
                ui.set_width(body_width);
                ui.set_height(body_height);
                let scroll = if view.terms.is_empty() {
                    form.page.label()
                } else {
                    "search"
                };
                ScrollArea::vertical()
                    .id_salt(("settings-body", scroll))
                    .auto_shrink([false, false])
                    .max_height(body_height)
                    .show(ui, |ui| body(ui, form, &mut view));
            });
        });

        let searching = !form.search.trim().is_empty();
        let (restore, answer) = footer_row(
            ui,
            |ui| {
                let can = !searching && form.page != Page::Identity;
                let response = button(ui, "Restore defaults", ButtonKind::Secondary, can);
                if can {
                    response.on_hover_text(format!(
                        "Put the settings on {} back to how Graft comes",
                        form.page.label()
                    ))
                } else if searching {
                    response.on_disabled_hover_text("Open a page first: this resets one page")
                } else {
                    response.on_disabled_hover_text("Git keeps your identity, not Graft")
                }
                .clicked()
            },
            |ui| main_buttons(ui, "Save", ButtonKind::Primary, true, false, None),
        );
        if restore {
            form.page.restore(&mut form.draft);
        } else if answer.submit {
            let identity = form
                .loaded
                .as_ref()
                .filter(|loaded| **loaded != form.identity)
                .map(|_| form.identity.clone());
            view.outcome = Outcome::Save(Box::new(form.draft.clone()), identity);
        } else if answer.cancel {
            view.outcome = Outcome::Cancel;
        }
    });
    if matches!(view.outcome, Outcome::Open) && shown.cancel {
        view.outcome = Outcome::Cancel;
    }
    view.outcome
}

/// The list of pages, with the search above it.
fn nav(ui: &mut Ui, form: &mut SettingsForm, view: &View) {
    ui.set_width(NAV_WIDTH);
    ui.spacing_mut().item_spacing.y = 2.0;
    title(ui, "Settings");
    ui.add_space(6.0);
    text_field(ui, &mut form.search, "Search settings", NAV_WIDTH);
    let terms: Vec<String> = form
        .search
        .split_whitespace()
        .map(str::to_lowercase)
        .collect();
    let searching = !terms.is_empty();
    let has_hits =
        |page: Page| !searching || SECTIONS.iter().any(|s| s.page == page && s.matches(&terms));
    let mut item = |ui: &mut Ui, page: Page| {
        let selected = !searching && form.page == page;
        if nav_item(ui, page.label(), selected, has_hits(page)).clicked() {
            form.page = page;
            form.search.clear();
        }
    };
    ui.add_space(10.0);
    section_label(ui, "Graft");
    for page in Page::APP {
        item(ui, page);
    }
    if view.repo.is_some() {
        ui.add_space(10.0);
        section_label(ui, "This repository");
        item(ui, Page::Identity);
    }
}

fn nav_item(ui: &mut Ui, label: &str, selected: bool, has_hits: bool) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(vec2(NAV_WIDTH, NAV_ROW), Sense::click());
    paint_row_bg(ui.painter(), rect, &response, selected);
    let text = if !has_hits {
        color::text_faint()
    } else if selected || response.hovered() {
        color::text()
    } else {
        color::text_mid()
    };
    ui.painter().text(
        rect.left_center() + vec2(10.0, 0.0),
        Align2::LEFT_CENTER,
        label,
        theme::body(),
        text,
    );
    response.on_hover_cursor(CursorIcon::PointingHand)
}

/// The chosen page, or every section the search finds.
fn body(ui: &mut Ui, form: &mut SettingsForm, view: &mut View) {
    let has_repo = view.repo.is_some();
    let available = move |s: &&Section| s.page != Page::Identity || has_repo;
    if view.terms.is_empty() {
        let heading = match (form.page, view.repo) {
            (Page::Identity, Some((name, _))) => format!("Commit identity · {name}"),
            (page, _) => page.label().to_owned(),
        };
        title(ui, &heading);
        for section in SECTIONS.iter().filter(available) {
            if section.page == form.page {
                section_title(ui, section.title);
                (section.show)(ui, form, view);
            }
        }
        return;
    }
    let hits: Vec<&Section> = SECTIONS
        .iter()
        .filter(available)
        .filter(|s| s.matches(&view.terms))
        .collect();
    if hits.is_empty() {
        hint(ui, &format!("No setting matches “{}”.", form.search.trim()));
        return;
    }
    for section in hits {
        section_title(ui, &format!("{} › {}", section.page.label(), section.title));
        (section.show)(ui, form, view);
        ui.add_space(6.0);
    }
}

fn section_title(ui: &mut Ui, title: &str) {
    ui.add_space(12.0);
    section_label(ui, title);
}

fn theme_section(ui: &mut Ui, form: &mut SettingsForm, _: &mut View) {
    ui.horizontal(|ui| {
        for choice in ThemeChoice::ALL {
            ui.radio_value(&mut form.draft.theme, choice, choice.label());
            ui.add_space(8.0);
        }
    });
}

/// The font styles: both fonts with one click.
fn style_section(ui: &mut Ui, form: &mut SettingsForm, view: &mut View) {
    let gap = 8.0;
    let card_width = (view.width - gap * (STYLES.len() - 1) as f32) / STYLES.len() as f32;
    let current = form.draft.fonts();
    ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing.x = gap;
        for style in &STYLES {
            // The built-in style needs no search.
            let fonts = match view.installed {
                Some(installed) => style.fonts(installed),
                None => style
                    .fonts(&[])
                    .map_err(|_| "Looking for the fonts on this PC…".to_owned()),
            };
            let selected = style.is(&current);
            let note = ui.painter().layout(
                style.note.to_owned(),
                theme::small(),
                color::text_weak(),
                card_width - 20.0,
            );
            let height = 38.0 + note.size().y;
            let (rect, response) = ui.allocate_exact_size(vec2(card_width, height), Sense::click());
            let enabled = fonts.is_ok();
            let border = if selected {
                color::accent()
            } else if enabled && response.hovered() {
                color::control_hover_border()
            } else {
                color::control_border()
            };
            let painter = ui.painter();
            painter.rect(
                rect,
                RADIUS,
                color::control_bg(),
                egui::Stroke::new(if selected { 2.0 } else { 1.0 }, border),
                egui::StrokeKind::Inside,
            );
            let name_color = if enabled {
                color::text()
            } else {
                color::text_faint()
            };
            painter.text(
                rect.left_top() + vec2(10.0, 10.0),
                Align2::LEFT_TOP,
                style.name,
                theme::semibold(theme::BODY_SIZE),
                name_color,
            );
            painter.galley(rect.left_top() + vec2(10.0, 30.0), note, color::text_weak());
            match fonts {
                Ok(fonts) => {
                    if response.on_hover_cursor(CursorIcon::PointingHand).clicked() {
                        form.draft.set_fonts(fonts);
                    }
                }
                Err(reason) => {
                    response.on_hover_text(reason);
                }
            }
        }
    });
    let custom = !STYLES.iter().any(|s| s.is(&current));
    if custom {
        hint(ui, "You picked your own fonts below.");
    }
}

fn fonts_section(ui: &mut Ui, form: &mut SettingsForm, view: &mut View) {
    font_row(
        ui,
        form,
        view,
        Kind::Ui,
        "Interface font",
        "everything but code",
    );
    ui.add_space(4.0);
    font_row(
        ui,
        form,
        view,
        Kind::Code,
        "Code font",
        "diffs, commit hashes, the console",
    );
    hint(
        ui,
        "The code font list shows only fonts whose letters are all equally wide. Characters \
         a font lacks come from Graft's built-in fonts.",
    );
}

fn font_row(
    ui: &mut Ui,
    form: &mut SettingsForm,
    view: &mut View,
    kind: Kind,
    label: &str,
    what: &str,
) {
    field_label(ui, &format!("{label} · {what}"));
    let current = match kind {
        Kind::Ui => form.draft.ui_font.clone(),
        Kind::Code => form.draft.code_font.clone(),
    };
    let mut picked: Option<FontChoice> = None;
    let list_width = view.width.min(380.0);
    ComboBox::from_id_salt(("font", label))
        .width(list_width)
        .height(340.0)
        .close_behavior(PopupCloseBehavior::CloseOnClickOutside)
        .selected_text(current.name(kind))
        .show_ui(ui, |ui| {
            ui.set_min_width(list_width - 12.0);
            text_field(ui, &mut form.font_filter, "Find a font", list_width - 16.0);
            let filter = form.font_filter.trim().to_lowercase();
            let shown = |name: &str| filter.is_empty() || name.to_lowercase().contains(&filter);
            let fits = |f: &&Family| (kind == Kind::Ui || f.monospace) && shown(&f.name);

            let mut option = |ui: &mut Ui, choice: FontChoice| {
                let selected = choice.same_family(&current);
                if ui
                    .selectable_label(selected, choice.name(kind).to_owned())
                    .clicked()
                {
                    picked = Some(choice);
                }
            };
            if shown(kind.built_in()) {
                list_label(ui, "Built into Graft");
                option(ui, FontChoice::default());
            }
            let added: Vec<&Family> = form.added_fonts.iter().filter(fits).collect();
            if !added.is_empty() {
                list_label(ui, "Added");
                for family in added {
                    option(ui, family.choice());
                }
            }
            list_label(ui, "On this PC");
            match view.installed {
                None => {
                    ui.horizontal(|ui| {
                        inline_spinner(ui);
                        hint(ui, "Looking for fonts…");
                    });
                }
                Some(installed) => {
                    let mut any = false;
                    for family in installed.iter().filter(fits) {
                        any = true;
                        option(ui, family.choice());
                    }
                    if !any {
                        hint(ui, "No font matches.");
                    }
                }
            }
            ui.separator();
            if ui.button("Add a font file…").clicked() {
                view.outcome = Outcome::AddFontFile(kind);
                ui.close();
            }
            if picked.is_some() {
                ui.close();
            }
        });
    if let Some(choice) = picked {
        *form.font_mut(kind) = choice;
        form.font_error = None;
    }
    if let Some((error_kind, error)) = &form.font_error
        && *error_kind == kind
    {
        error_label(ui, error);
    }
}

/// A small heading inside a font list.
fn list_label(ui: &mut Ui, text: &str) {
    ui.add_space(4.0);
    section_label(ui, text);
}

fn zoom_section(ui: &mut Ui, form: &mut SettingsForm, _: &mut View) {
    ui.horizontal(|ui| {
        ui.spacing_mut().slider_width = 260.0;
        let mut zoom = form.zoom_dragged.unwrap_or(form.draft.zoom);
        let response = ui.add(
            Slider::new(&mut zoom, ZOOM_MIN..=ZOOM_MAX)
                .step_by(5.0)
                .suffix("%"),
        );
        if response.dragged() {
            form.zoom_dragged = Some(zoom);
        } else {
            form.zoom_dragged = None;
            form.draft.zoom = zoom;
        }
        if button(ui, "Reset", ButtonKind::Secondary, form.draft.zoom != 100)
            .on_hover_text("Back to 100%")
            .clicked()
        {
            form.draft.zoom = 100;
        }
    });
    let keys = &form.draft.keys;
    let shortcuts: Vec<String> = [Command::ZoomIn, Command::ZoomOut, Command::ZoomReset]
        .into_iter()
        .filter_map(|c| keys.text(c))
        .collect();
    let anywhere = match shortcuts.as_slice() {
        [] => String::new(),
        [one] => format!(" {one} changes it anywhere."),
        [rest @ .., last] => format!(" {} and {last} change it anywhere.", rest.join(", ")),
    };
    hint(
        ui,
        &format!(
            "Makes everything bigger or smaller: text, icons and spacing. It comes on top of \
             the Windows display scale.{anywhere}"
        ),
    );
}

fn editor_section(ui: &mut Ui, form: &mut SettingsForm, view: &mut View) {
    field_label(ui, "Command that opens a folder or file");
    text_field(ui, &mut form.draft.ide_command, "code", view.width);
    hint(
        ui,
        "For example code, cursor, idea64 or rider64. If it does not start, Graft opens the \
         default program.",
    );
}

fn updates_section(ui: &mut Ui, form: &mut SettingsForm, view: &mut View) {
    ui.horizontal(|ui| {
        ui.checkbox(
            &mut form.draft.check_updates,
            "Check for a new version when Graft starts",
        );
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if button(ui, "About Graft", ButtonKind::Secondary, true).clicked() {
                view.outcome = Outcome::About;
            }
        });
    });
}

fn diffs_section(ui: &mut Ui, form: &mut SettingsForm, _: &mut View) {
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
}

fn review_section(ui: &mut Ui, form: &mut SettingsForm, _: &mut View) {
    ui.checkbox(&mut form.draft.review_folders, "Show files in folders");
    hint(
        ui,
        "Review lists the files under their folders instead of one flat list. The link \
         above Review's file list switches it too.",
    );
}

fn fetch_section(ui: &mut Ui, form: &mut SettingsForm, _: &mut View) {
    ui.horizontal(|ui| {
        ui.label(RichText::new("Fetch open repositories in the background").color(color::text()));
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
}

fn slash_section(ui: &mut Ui, form: &mut SettingsForm, view: &mut View) {
    field_label(ui, "Branches Slash never deletes");
    text_field(
        ui,
        &mut form.draft.protected,
        "main, master, release/*",
        view.width,
    );
    hint(
        ui,
        "Comma-separated; * matches anything. The default branch is always kept.",
    );
}

fn worktrees_section(ui: &mut Ui, form: &mut SettingsForm, view: &mut View) {
    field_label(ui, "Ignored files to copy into new worktrees");
    text_field(
        ui,
        &mut form.draft.copy_patterns,
        ".env, .env.*",
        view.width,
    );
}

fn identity_section(ui: &mut Ui, form: &mut SettingsForm, view: &mut View) {
    let Some((name, identity)) = view.repo else {
        return;
    };
    hint(
        ui,
        "Git records this name and email as the author of your commits.",
    );
    match identity {
        None => inline_spinner(ui),
        Some(Err(error)) => error_label(ui, error),
        Some(Ok(_)) => {
            let half = (view.width - 8.0) / 2.0;
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

/// Every action with its shortcut; click one to record a new one. While
/// searching, only the actions the search names (or all, if it names none).
fn shortcuts_section(ui: &mut Ui, form: &mut SettingsForm, view: &mut View) {
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
    let named: Vec<Command> = Command::ALL
        .into_iter()
        .filter(|c| {
            let label = c.label().to_lowercase();
            !view.terms.is_empty() && view.terms.iter().all(|t| label.contains(t.as_str()))
        })
        .collect();
    for scope in [Scope::General, Scope::FileStatus, Scope::Review] {
        let commands: Vec<Command> = Command::ALL
            .into_iter()
            .filter(|c| c.scope() == scope && (named.is_empty() || named.contains(c)))
            .collect();
        if commands.is_empty() {
            continue;
        }
        ui.add_space(4.0);
        field_label(ui, scope.label());
        for command in commands {
            shortcut_row(ui, form, command);
        }
    }
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

/// " Alt+Z switches it anywhere.", if `command` has a shortcut.
fn switches(keys: &Keymap, command: Command) -> String {
    keys.text(command)
        .map(|key| format!(" {key} switches it anywhere."))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn terms(search: &str) -> Vec<String> {
        search.split_whitespace().map(str::to_lowercase).collect()
    }

    fn found(search: &str) -> Vec<&'static str> {
        SECTIONS
            .iter()
            .filter(|s| s.matches(&terms(search)))
            .map(|s| s.title)
            .collect()
    }

    #[test]
    fn search_finds_sections_by_any_of_their_words() {
        // The zoom setting, and its shortcuts.
        assert_eq!(found("zoom"), ["Zoom", "Shortcuts"]);
        assert_eq!(found("FONT"), ["Font style", "Fonts"]);
        assert_eq!(found("email"), ["Commit identity"]);
        // Every word must match.
        assert_eq!(found("protected branches"), ["Slash"]);
        assert!(found("protected zoom").is_empty());
        // Keyboard is found by the actions it lists.
        assert_eq!(found("stage all"), ["Shortcuts"]);
    }

    #[test]
    fn restore_resets_only_its_own_page() {
        let mut draft = Settings {
            zoom: 150,
            wrap_lines: true,
            ide_command: "rider64".into(),
            ..Settings::default()
        };
        Page::Appearance.restore(&mut draft);
        assert_eq!(draft.zoom, 100);
        assert!(draft.wrap_lines);
        assert_eq!(draft.ide_command, "rider64");
        Page::Diffs.restore(&mut draft);
        Page::General.restore(&mut draft);
        assert_eq!(draft, Settings::default());
    }

    #[test]
    fn a_proportional_font_file_is_refused_as_code_font() {
        let mut form = SettingsForm::new(&Settings::default());
        let inter = crate::fonts::inspect(
            &std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("assets/fonts/Inter-Regular.ttf"),
        )
        .unwrap();
        form.font_file_added(Kind::Code, Ok(inter.clone()));
        assert!(form.draft.code_font.is_built_in());
        assert!(matches!(form.font_error, Some((Kind::Code, _))));

        form.font_file_added(Kind::Ui, Ok(inter));
        assert_eq!(form.draft.ui_font.family, "Inter");
        assert!(!form.draft.ui_font.is_built_in());
        assert!(form.font_error.is_none());
        assert_eq!(form.added_fonts.len(), 1);
    }
}
