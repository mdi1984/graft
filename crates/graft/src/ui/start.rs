//! The start page ("New tab"): open a local repository, clone one, create one,
//! reopen a recent one, or open all repositories of a tab group.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use egui::{Align2, Rect, ScrollArea, Sense, Stroke, StrokeKind, TextureHandle, Ui, pos2, vec2};
use graft_core::CancelToken;
use graft_core::repo::{Progress, clone_folder_name, native};

use super::icons::{self, Icon};
use super::project_tabs::group_color;
use super::widgets::{
    ButtonKind, button, elided, elided_path, paint_galley_left, paint_icon_button, progress_bar,
    section_heading, text_field,
};
use crate::groups::{GroupId, TabGroup};
use crate::theme::{self, RADIUS, color};

const WIDTH: f32 = 600.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Home,
    Clone,
    Init,
}

/// A clone or init in progress.
pub struct Busy {
    pub label: String,
    pub progress: Arc<Mutex<Progress>>,
    pub cancel: Option<CancelToken>,
}

pub struct StartPage {
    pub mode: Mode,
    pub url: String,
    pub clone_parent: String,
    pub folder_name: String,
    /// The user typed a folder name, so stop deriving it from the URL.
    folder_name_edited: bool,
    pub init_path: String,
    pub busy: Option<Busy>,
    pub error: Option<String>,
    logo: Option<TextureHandle>,
}

/// A recently opened repository; `exists` is checked when the list changes,
/// not every frame.
pub struct Recent {
    pub path: PathBuf,
    pub exists: bool,
}

impl Recent {
    pub fn new(path: PathBuf) -> Self {
        let exists = path.is_dir();
        Self { path, exists }
    }
}

/// What the user asked for; the app carries it out.
pub enum StartAction {
    PickOpen,
    PickCloneParent,
    PickInitFolder,
    Clone { url: String, dest: PathBuf },
    Init(PathBuf),
    Cancel,
    OpenRecent(PathBuf),
    ForgetRecent(PathBuf),
    OpenGroup(GroupId),
    RenameGroup(GroupId),
    DeleteGroup(GroupId),
}

impl StartPage {
    pub fn new(clone_parent: PathBuf) -> Self {
        Self {
            mode: Mode::Home,
            url: String::new(),
            clone_parent: clone_parent.display().to_string(),
            folder_name: String::new(),
            folder_name_edited: false,
            init_path: String::new(),
            busy: None,
            error: None,
            logo: None,
        }
    }

    /// Forget the forms after a repository was opened.
    pub fn reset(&mut self) {
        self.mode = Mode::Home;
        self.url.clear();
        self.folder_name.clear();
        self.folder_name_edited = false;
        self.init_path.clear();
        self.busy = None;
        self.error = None;
    }

    pub fn clone_dest(&self) -> PathBuf {
        native(self.clone_parent.trim()).join(self.folder_name.trim())
    }

    pub fn show(
        &mut self,
        ui: &mut Ui,
        recent: &[Recent],
        groups: &[TabGroup],
    ) -> Option<StartAction> {
        let mut action = None;
        ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                let avail = ui.available_width();
                let width = WIDTH.min(avail - 32.0);
                let margin = ((avail - width) / 2.0).max(16.0);
                ui.add_space(48.0);
                ui.horizontal(|ui| {
                    ui.add_space(margin);
                    ui.vertical(|ui| {
                        ui.set_width(width);
                        ui.spacing_mut().item_spacing.y = 8.0;
                        self.header(ui);
                        ui.add_space(20.0);
                        self.cards(ui, width, &mut action);
                        ui.add_space(12.0);
                        match self.mode {
                            Mode::Home => {}
                            Mode::Clone => self.clone_form(ui, width, &mut action),
                            Mode::Init => self.init_form(ui, width, &mut action),
                        }
                        if let Some(busy) = &self.busy {
                            busy_panel(ui, busy, width, &mut action);
                        }
                        if let Some(error) = &self.error {
                            ui.add_space(4.0);
                            ui.label(egui::RichText::new(error).color(color::removed()));
                        }
                        ui.add_space(24.0);
                        if !groups.is_empty() {
                            group_list(ui, groups, width, &mut action);
                            ui.add_space(16.0);
                        }
                        recent_list(ui, recent, width, &mut action);
                        ui.add_space(16.0);
                        ui.label(
                            egui::RichText::new(
                                "Tip: drop a folder onto this window to open it. Ctrl+O opens, Ctrl+T shows this page.",
                            )
                            .color(color::text_faint())
                            .size(theme::SMALL_SIZE),
                        );
                    });
                });
            });
        action
    }

    fn header(&mut self, ui: &mut Ui) {
        let logo = self
            .logo
            .get_or_insert_with(|| {
                let icon = eframe::icon_data::from_png_bytes(include_bytes!(
                    "../../assets/icon/graft-256.png"
                ))
                .expect("bundled icon is a valid PNG");
                let image = egui::ColorImage::from_rgba_unmultiplied(
                    [icon.width as usize, icon.height as usize],
                    &icon.rgba,
                );
                ui.ctx()
                    .load_texture("graft-logo", image, egui::TextureOptions::LINEAR)
            })
            .clone();
        ui.horizontal(|ui| {
            ui.add(egui::Image::new(&logo).fit_to_exact_size(vec2(52.0, 52.0)));
            ui.add_space(8.0);
            ui.vertical(|ui| {
                ui.add_space(4.0);
                ui.label(
                    egui::RichText::new("Graft")
                        .font(theme::semibold(24.0))
                        .color(color::text()),
                );
                ui.label(
                    egui::RichText::new("Open a repository to get started.")
                        .color(color::text_weak()),
                );
            });
        });
    }

    fn cards(&mut self, ui: &mut Ui, width: f32, action: &mut Option<StartAction>) {
        let gap = 12.0;
        let card_width = (width - 2.0 * gap) / 3.0;
        let idle = self.busy.is_none();
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = gap;
            if card(
                ui,
                card_width,
                Icon::Folder,
                "Open local",
                "A folder with a git repository",
                false,
            )
            .clicked()
                && idle
            {
                self.error = None;
                *action = Some(StartAction::PickOpen);
            }
            if card(
                ui,
                card_width,
                Icon::Download,
                "Clone",
                "Copy a repository from a URL",
                self.mode == Mode::Clone,
            )
            .clicked()
                && idle
            {
                self.error = None;
                self.mode = Mode::Clone;
            }
            if card(
                ui,
                card_width,
                Icon::FolderPlus,
                "Create new",
                "Start an empty repository",
                self.mode == Mode::Init,
            )
            .clicked()
                && idle
            {
                self.error = None;
                self.mode = Mode::Init;
            }
        });
    }

    fn clone_form(&mut self, ui: &mut Ui, width: f32, action: &mut Option<StartAction>) {
        form_frame(ui, |ui| {
            let inner = width - 32.0;
            field_label(ui, "Repository URL");
            let url = text_field(
                ui,
                &mut self.url,
                "https://github.com/owner/repo.git",
                inner,
            );
            if url.changed() {
                self.error = None;
            }
            if url.changed() && !self.folder_name_edited {
                self.folder_name = if self.url.trim().is_empty() {
                    String::new()
                } else {
                    clone_folder_name(&self.url)
                };
            }
            ui.add_space(4.0);
            field_label(ui, "Clone into folder");
            ui.horizontal(|ui| {
                if text_field(ui, &mut self.clone_parent, r"C:\dev", inner - 96.0).changed() {
                    self.error = None;
                }
                if button(ui, "Browse…", ButtonKind::Secondary, self.busy.is_none()).clicked() {
                    *action = Some(StartAction::PickCloneParent);
                }
            });
            ui.add_space(4.0);
            field_label(ui, "Folder name");
            if text_field(ui, &mut self.folder_name, "repo", inner).changed() {
                self.folder_name_edited = true;
                self.error = None;
            }
            let ready = !self.url.trim().is_empty()
                && !self.clone_parent.trim().is_empty()
                && !self.folder_name.trim().is_empty()
                && self.busy.is_none();
            if ready {
                let (rect, _) = ui.allocate_exact_size(vec2(inner, 16.0), Sense::hover());
                let prefix = ui.painter().layout_no_wrap(
                    "Will clone to ".to_owned(),
                    theme::small(),
                    color::text_faint(),
                );
                let prefix_width = prefix.size().x;
                let y = rect.center().y;
                paint_galley_left(ui.painter(), rect.left(), y, prefix, color::text_faint());
                let dest = self.clone_dest().display().to_string();
                let dest = elided_path(
                    ui,
                    &dest,
                    theme::small(),
                    color::text_weak(),
                    inner - prefix_width,
                );
                paint_galley_left(
                    ui.painter(),
                    rect.left() + prefix_width,
                    y,
                    dest,
                    color::text_weak(),
                );
            }
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                let submit = url.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                if (button(ui, "Clone", ButtonKind::Primary, ready).clicked() || submit) && ready {
                    self.error = None;
                    *action = Some(StartAction::Clone {
                        url: self.url.trim().to_owned(),
                        dest: self.clone_dest(),
                    });
                }
                if button(ui, "Cancel", ButtonKind::Secondary, self.busy.is_none()).clicked() {
                    self.mode = Mode::Home;
                    self.error = None;
                }
            });
        });
    }

    fn init_form(&mut self, ui: &mut Ui, width: f32, action: &mut Option<StartAction>) {
        form_frame(ui, |ui| {
            let inner = width - 32.0;
            field_label(ui, "New repository folder (created if missing)");
            ui.horizontal(|ui| {
                text_field(ui, &mut self.init_path, r"C:\dev\my-project", inner - 96.0);
                if button(ui, "Browse…", ButtonKind::Secondary, self.busy.is_none()).clicked() {
                    *action = Some(StartAction::PickInitFolder);
                }
            });
            ui.add_space(6.0);
            let ready = !self.init_path.trim().is_empty() && self.busy.is_none();
            ui.horizontal(|ui| {
                if button(ui, "Create repository", ButtonKind::Primary, ready).clicked() {
                    self.error = None;
                    *action = Some(StartAction::Init(PathBuf::from(self.init_path.trim())));
                }
                if button(ui, "Cancel", ButtonKind::Secondary, self.busy.is_none()).clicked() {
                    self.mode = Mode::Home;
                    self.error = None;
                }
            });
        });
    }
}

fn form_frame(ui: &mut Ui, add: impl FnOnce(&mut Ui)) {
    egui::Frame::new()
        .fill(color::sidebar_bg())
        .stroke(Stroke::new(1.0, color::border()))
        .corner_radius(8)
        .inner_margin(egui::Margin::same(16))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = 6.0;
            add(ui);
        });
}

fn field_label(ui: &mut Ui, text: &str) {
    ui.label(
        egui::RichText::new(text)
            .color(color::text_weak())
            .size(theme::SMALL_SIZE),
    );
}

fn card(
    ui: &mut Ui,
    width: f32,
    icon: Icon,
    title: &str,
    detail: &str,
    active: bool,
) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(vec2(width, 92.0), Sense::click());
    let painter = ui.painter();
    let (fill, border) = if active {
        (
            color::selected_row_bg(),
            color::accent().gamma_multiply(0.6),
        )
    } else if response.hovered() {
        (color::control_hover_bg(), color::control_border())
    } else {
        (color::control_bg(), color::control_border())
    };
    painter.rect(rect, 8, fill, Stroke::new(1.0, border), StrokeKind::Inside);
    let icon_rect = Rect::from_min_size(rect.min + vec2(14.0, 14.0), vec2(20.0, 20.0));
    icons::paint(painter, icon_rect, icon, color::accent());
    painter.text(
        pos2(rect.left() + 14.0, rect.top() + 52.0),
        Align2::LEFT_CENTER,
        title,
        theme::semibold(theme::BODY_SIZE),
        color::text(),
    );
    let detail = elided(
        ui,
        detail,
        theme::small(),
        color::text_weak(),
        rect.width() - 28.0,
    );
    paint_galley_left(
        painter,
        rect.left() + 14.0,
        rect.top() + 72.0,
        detail,
        color::text_weak(),
    );
    response
}

fn busy_panel(ui: &mut Ui, busy: &Busy, width: f32, action: &mut Option<StartAction>) {
    let progress = busy.progress.lock().map(|p| p.clone()).unwrap_or_default();
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(&busy.label).color(color::text()));
        if !progress.phase.is_empty() {
            let pct = progress
                .fraction
                .map(|f| format!(" {:.0}%", f * 100.0))
                .unwrap_or_default();
            ui.label(
                egui::RichText::new(format!("· {}{pct}", progress.phase)).color(color::text_weak()),
            );
        }
    });
    ui.horizontal(|ui| {
        progress_bar(ui, progress.fraction, width - 96.0);
        // Progress arrives from git in the background; look again soon.
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_millis(100));
        if busy.cancel.is_some() && button(ui, "Cancel", ButtonKind::Secondary, true).clicked() {
            *action = Some(StartAction::Cancel);
        }
    });
}

fn recent_list(ui: &mut Ui, recent: &[Recent], width: f32, action: &mut Option<StartAction>) {
    let (rect, _) = ui.allocate_exact_size(vec2(width, 20.0), Sense::hover());
    section_heading(ui, ui.painter(), rect.left(), rect.center().y, "RECENT");
    if recent.is_empty() {
        ui.label(
            egui::RichText::new("Repositories you open show up here.").color(color::text_faint()),
        );
        return;
    }
    for Recent { path, exists } in recent {
        let exists = *exists;
        let (rect, response) = ui.allocate_exact_size(vec2(width, 44.0), Sense::click());
        let close =
            Rect::from_center_size(pos2(rect.right() - 18.0, rect.center().y), vec2(22.0, 22.0));
        let close_response =
            ui.interact(close, ui.scope_id().with(("forget", path)), Sense::click());
        let hovered = response.hovered() || close_response.hovered();
        let painter = ui.painter();
        if hovered {
            painter.rect_filled(rect, RADIUS, color::hover_row_bg());
        }
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.display().to_string());
        let fg = if exists {
            color::text()
        } else {
            color::text_faint()
        };
        let name = elided(
            ui,
            &name,
            theme::semibold(theme::BODY_SIZE),
            fg,
            width - 60.0,
        );
        paint_galley_left(painter, rect.left() + 10.0, rect.top() + 14.0, name, fg);
        let detail = if exists {
            path.display().to_string()
        } else {
            format!("{} (missing)", path.display())
        };
        let detail = elided_path(
            ui,
            &detail,
            theme::small(),
            color::text_weak(),
            width - 60.0,
        );
        paint_galley_left(
            painter,
            rect.left() + 10.0,
            rect.top() + 31.0,
            detail,
            color::text_weak(),
        );
        if hovered {
            paint_icon_button(ui, close, &close_response, Icon::Close);
        }
        if close_response.on_hover_text("Remove from recent").clicked() {
            *action = Some(StartAction::ForgetRecent(path.clone()));
        } else if response.clicked() && exists {
            *action = Some(StartAction::OpenRecent(path.clone()));
        }
    }
}

/// Tab groups: click one to open all of its repositories.
fn group_list(ui: &mut Ui, groups: &[TabGroup], width: f32, action: &mut Option<StartAction>) {
    let (rect, _) = ui.allocate_exact_size(vec2(width, 20.0), Sense::hover());
    section_heading(ui, ui.painter(), rect.left(), rect.center().y, "GROUPS");
    for group in groups {
        let (rect, response) = ui.allocate_exact_size(vec2(width, 44.0), Sense::click());
        let response = response.on_hover_text("Open all of its repositories");
        let painter = ui.painter();
        if response.hovered() {
            painter.rect_filled(rect, RADIUS, color::hover_row_bg());
        }
        let fg = group_color(group.color);
        painter.circle_filled(pos2(rect.left() + 14.0, rect.top() + 14.0), 4.0, fg);
        let count = match group.repos.len() {
            0 => "no repositories yet".to_owned(),
            1 => "1 repository".to_owned(),
            n => format!("{n} repositories"),
        };
        let name = elided(
            ui,
            &group.name,
            theme::semibold(theme::BODY_SIZE),
            color::text(),
            width - 200.0,
        );
        let name_right = rect.left() + 26.0 + name.size().x;
        paint_galley_left(
            painter,
            rect.left() + 26.0,
            rect.top() + 14.0,
            name,
            color::text(),
        );
        let count = elided(ui, &count, theme::small(), color::text_weak(), 160.0);
        paint_galley_left(
            painter,
            name_right + 8.0,
            rect.top() + 14.0,
            count,
            color::text_weak(),
        );
        let names: Vec<String> = group
            .repos
            .iter()
            .map(|p| {
                p.file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| p.display().to_string())
            })
            .collect();
        let names = elided(
            ui,
            &names.join(" · "),
            theme::small(),
            color::text_weak(),
            width - 120.0,
        );
        paint_galley_left(
            painter,
            rect.left() + 26.0,
            rect.top() + 31.0,
            names,
            color::text_weak(),
        );
        if response.hovered() {
            painter.text(
                pos2(rect.right() - 12.0, rect.center().y),
                Align2::RIGHT_CENTER,
                "Open all",
                theme::semibold(theme::BODY_SIZE),
                color::accent(),
            );
        }
        if response.clicked() {
            *action = Some(StartAction::OpenGroup(group.id));
        }
        response.context_menu(|ui| {
            if ui.button("Open all").clicked() {
                *action = Some(StartAction::OpenGroup(group.id));
                ui.close();
            }
            if ui.button("Rename…").clicked() {
                *action = Some(StartAction::RenameGroup(group.id));
                ui.close();
            }
            ui.separator();
            if ui.button("Delete group").clicked() {
                *action = Some(StartAction::DeleteGroup(group.id));
                ui.close();
            }
        });
    }
}
