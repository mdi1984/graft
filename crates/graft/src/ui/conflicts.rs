//! Conflicts: the banner while a merge, rebase, cherry-pick or revert is in
//! progress, and the resolver for a conflicted file — one card per conflict
//! (Ours | Base | Theirs → use ours / theirs / both / edit) next to a live
//! preview of the result.

use egui::text::{LayoutJob, TextFormat};
use egui::{Color32, Frame, Margin, Panel, RichText, ScrollArea, Stroke, TextEdit, Ui, vec2};
use graft_core::changes::count_lines;
use graft_core::conflicts::{
    self, ConflictFile, Piece, Resolution, Segment, SideNames, first_lines, last_lines,
};
use graft_core::{InProgress, Operation, Worktree};

use super::can::{self, Can};
use super::widgets::{ButtonKind, button, button_width, empty_state, section_heading_row, waiting};
use crate::project::{Action, Confirm, ConflictView, Project};
use crate::text::plural;
use crate::theme::{self, RADIUS, color};
use crate::ui::Actions;

/// Lines of clean context shown around each conflict.
const CONTEXT: usize = 3;

fn side_names(wt: &Worktree) -> SideNames {
    conflicts::side_names(
        wt.in_progress.as_ref().map(|p| p.kind),
        wt.branch.as_deref(),
    )
}

/// The strip across the top of the Workspace while something is in progress.
pub fn banner(ui: &mut Ui, project: &Project, wt: &Worktree, actions: &mut Actions) {
    let Some(InProgress { kind, title }) = &wt.in_progress else {
        return;
    };
    let left = wt.status.as_ref().map_or(0, |s| s.conflicted().count());
    let can = can::act(project, "");
    Panel::top("in-progress-banner")
        .frame(
            Frame::new()
                .fill(color::dirty_bg())
                .inner_margin(Margin::symmetric(14, 8))
                .stroke(Stroke::new(1.0, color::slash_border())),
        )
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(format!("{} in progress", kind.noun()))
                        .font(theme::semibold(theme::BODY_SIZE))
                        .color(color::dirty_fg()),
                );
                ui.label(RichText::new(title).color(color::text_mid()));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let next = if left > 0 {
                        can::no("Resolve every conflict first")
                    } else {
                        can::yes(match kind {
                            Operation::Rebase => "Commit this step and replay the next commit",
                            _ => "Commit the result",
                        })
                        .unless(can.blocked())
                    };
                    if button(ui, "Continue", ButtonKind::Primary, next.ok)
                        .on_hover_text(&next.hint)
                        .clicked()
                    {
                        actions.push(Action::ContinueOp(*kind));
                    }
                    if can
                        .why_not(button(ui, "Abort", ButtonKind::Secondary, can.ok))
                        .clicked()
                    {
                        actions.push(abort(*kind));
                    }
                    let text = match left {
                        0 => "All conflicts resolved".to_owned(),
                        n => format!("{} with conflicts", plural(n, "file", "files")),
                    };
                    ui.label(RichText::new(text).color(if left == 0 {
                        color::added()
                    } else {
                        color::removed()
                    }));
                });
            });
        });
}

/// Ask first: resolutions made so far are lost.
fn abort(kind: Operation) -> Action {
    let noun = kind.noun().to_lowercase();
    Action::Ask(Box::new(Confirm {
        title: format!("Abort the {noun}?"),
        body: "The worktree goes back to how it was before it started. Conflict resolutions \
               made so far are lost."
            .to_owned(),
        confirm_label: format!("Abort {noun}"),
        action: Action::AbortOp(kind),
    }))
}

/// The resolver for `project.conflict`, in place of the diff.
pub fn resolver(ui: &mut Ui, project: &mut Project, actions: &mut Actions) {
    let can = can::act(project, "");
    let names = project
        .worktree()
        .map_or_else(|| conflicts::side_names(None, None), side_names);
    let Some(view) = &mut project.conflict else {
        return;
    };
    let file = match &view.file {
        None => {
            waiting(ui);
            return;
        }
        Some(Err(e)) => {
            empty_state(ui, "Could not read the conflict", e);
            return;
        }
        Some(Ok(file)) => file.clone(),
    };

    header(ui, view, &file, &can, actions);

    if file.segments.is_empty() {
        whole_file(ui, &file, &names, &can, actions);
        return;
    }

    let width = ui.available_width();
    Panel::right("conflict-result")
        .resizable(true)
        .default_size(width * 0.38)
        .size_range(220.0..=(width * 0.6).max(240.0))
        .frame(
            Frame::new()
                .fill(color::sidebar_bg())
                .inner_margin(Margin::symmetric(12, 10)),
        )
        .show(ui, |ui| result_panel(ui, view, &file));

    ScrollArea::vertical()
        .id_salt(("conflict-cards", &file.path))
        .auto_shrink([false, false])
        .show(ui, |ui| {
            Frame::new()
                .inner_margin(Margin::symmetric(12, 10))
                .show(ui, |ui| cards(ui, view, &file, &names, &can, actions));
        });
}

fn header(ui: &mut Ui, view: &ConflictView, file: &ConflictFile, can: &Can, actions: &mut Actions) {
    Panel::top("conflict-header")
        .frame(
            Frame::new()
                .fill(color::bar_bg())
                .inner_margin(Margin::symmetric(12, 6)),
        )
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(&file.path)
                        .font(theme::semibold(theme::BODY_SIZE))
                        .color(color::text()),
                );
                let total = view.resolutions.len();
                if total > 0 {
                    let done = view.resolved();
                    ui.label(
                        RichText::new(format!("{done} of {total} conflicts resolved"))
                            .color(if done == total {
                                color::added()
                            } else {
                                color::text_weak()
                            })
                            .size(theme::SMALL_SIZE),
                    );
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if total > 0 {
                        let save = if view.result().is_some() {
                            can::yes("Write the result to the file and stage it")
                                .unless(can.blocked())
                        } else {
                            can::no("Pick a result for every conflict first")
                        };
                        if button(ui, "Save resolution", ButtonKind::Primary, save.ok)
                            .on_hover_text(&save.hint)
                            .clicked()
                        {
                            actions.push(Action::SaveResolution);
                        }
                    }
                    ui.menu_button("More", |ui| {
                        let ours = egui::Button::new("Take ours for the whole file");
                        if can.why_not(ui.add_enabled(can.ok, ours)).clicked() {
                            actions.push(Action::TakeSide {
                                path: file.path.clone(),
                                ours: true,
                            });
                            ui.close();
                        }
                        let theirs = egui::Button::new("Take theirs for the whole file");
                        if can.why_not(ui.add_enabled(can.ok, theirs)).clicked() {
                            actions.push(Action::TakeSide {
                                path: file.path.clone(),
                                ours: false,
                            });
                            ui.close();
                        }
                        ui.separator();
                        if ui.button("Open in editor").clicked() {
                            actions.push(Action::OpenPath(file.path.clone()));
                            ui.close();
                        }
                        let resolved = egui::Button::new("Mark as resolved");
                        if can
                            .why_not(ui.add_enabled(can.ok, resolved))
                            .on_hover_text("You fixed the file in an editor: stage it as it is")
                            .clicked()
                        {
                            actions.push(Action::MarkResolved(file.path.clone()));
                            ui.close();
                        }
                    });
                });
            });
        });
}

/// Files with no line-by-line view: binary, or deleted on one side.
fn whole_file(
    ui: &mut Ui,
    file: &ConflictFile,
    names: &SideNames,
    can: &Can,
    actions: &mut Actions,
) {
    let (ours_name, theirs_name) = (&names.ours.long, &names.theirs.long);
    let describe = |side: &Option<graft_core::conflicts::Side>| match side {
        None => "deleted the file",
        Some(_) if file.base.is_none() => "added the file",
        Some(_) => "changed the file",
    };
    let explanation = if file.binary {
        "This is a binary file. Pick the version to keep.".to_owned()
    } else {
        format!(
            "{ours_name} {}; {theirs_name} {}. Pick the version to keep.",
            describe(&file.ours),
            describe(&file.theirs)
        )
    };
    ui.add_space(24.0);
    ui.vertical_centered(|ui| {
        ui.label(
            RichText::new("Whole-file conflict")
                .font(theme::title())
                .color(color::text_mid()),
        );
        ui.add_space(4.0);
        ui.label(RichText::new(explanation).color(color::text_weak()));
        ui.add_space(12.0);
        ui.horizontal(|ui| {
            let label = |name: &str, side: &Option<_>| match side {
                None => format!("Delete it ({name})"),
                Some(_) => format!("Keep {name}"),
            };
            let (ours, theirs) = (
                label(ours_name, &file.ours),
                label(theirs_name, &file.theirs),
            );
            let w = button_width(ui, &ours, ButtonKind::Secondary)
                + ui.spacing().item_spacing.x
                + button_width(ui, &theirs, ButtonKind::Secondary);
            ui.add_space((ui.available_width() - w).max(0.0) / 2.0);
            if can
                .why_not(button(ui, &ours, ButtonKind::Secondary, can.ok))
                .clicked()
            {
                actions.push(Action::TakeSide {
                    path: file.path.clone(),
                    ours: true,
                });
            }
            if can
                .why_not(button(ui, &theirs, ButtonKind::Secondary, can.ok))
                .clicked()
            {
                actions.push(Action::TakeSide {
                    path: file.path.clone(),
                    ours: false,
                });
            }
        });
    });
}

fn cards(
    ui: &mut Ui,
    view: &mut ConflictView,
    file: &ConflictFile,
    names: &SideNames,
    can: &Can,
    actions: &mut Actions,
) {
    let (ours_name, theirs_name) = (&names.ours.long, &names.theirs.long);
    let total = view.resolutions.len();
    let mut index = 0;
    // Line in the "ours" version where each conflict starts.
    let mut line = 1;
    for (n, segment) in file.segments.iter().enumerate() {
        let Segment::Conflict { ours, base, theirs } = segment else {
            if let Segment::Common(text) = segment {
                line += count_lines(text);
            }
            continue;
        };
        let before = match n.checked_sub(1).map(|i| &file.segments[i]) {
            Some(Segment::Common(text)) => last_lines(text, CONTEXT),
            _ => String::new(),
        };
        let after = match file.segments.get(n + 1) {
            Some(Segment::Common(text)) => first_lines(text, CONTEXT),
            _ => String::new(),
        };
        let i = index;
        index += 1;
        let resolution = view.resolutions[i].clone();
        Frame::new()
            .fill(color::sidebar_bg())
            .stroke(Stroke::new(
                1.0,
                if resolution.is_some() {
                    color::accent().gamma_multiply(0.45)
                } else {
                    color::border()
                },
            ))
            .corner_radius(RADIUS)
            .inner_margin(Margin::same(10))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(format!("Conflict {} of {total}", i + 1))
                            .font(theme::semibold(theme::BODY_SIZE))
                            .color(color::text()),
                    );
                    ui.label(
                        RichText::new(format!("line {line}"))
                            .color(color::text_faint())
                            .size(theme::SMALL_SIZE),
                    );
                    if let Some(r) = &resolution {
                        ui.label(
                            RichText::new(format!("✓ {}", describe(r, names)))
                                .color(color::added())
                                .size(theme::SMALL_SIZE),
                        );
                    }
                });
                if !before.is_empty() {
                    code(ui, &before, color::text_faint());
                }
                let columns = if base.is_empty() { 2 } else { 3 };
                // Side by side when each column has room for a line of code,
                // stacked otherwise.
                if ui.available_width() < columns as f32 * 300.0 {
                    side(ui, ours_name, ours, color::ours());
                    if columns == 3 {
                        side(ui, "Base", base, color::text_faint());
                    }
                    side(ui, theirs_name, theirs, color::theirs());
                } else {
                    ui.columns(columns, |cols| {
                        side(&mut cols[0], ours_name, ours, color::ours());
                        if columns == 3 {
                            side(&mut cols[1], "Base", base, color::text_faint());
                        }
                        side(&mut cols[columns - 1], theirs_name, theirs, color::theirs());
                    });
                }
                if !after.is_empty() {
                    code(ui, &after, color::text_faint());
                }
                ui.add_space(4.0);
                ui.horizontal_wrapped(|ui| {
                    let mut pick = |ui: &mut Ui, label: &str, r: Resolution| {
                        let selected =
                            resolution.as_ref() == Some(&r) && !matches!(r, Resolution::Custom(_));
                        let button = egui::Button::selectable(selected, label);
                        if can.why_not(ui.add_enabled(can.ok, button)).clicked() {
                            actions.push(Action::ResolveConflict {
                                index: i,
                                resolution: if selected { None } else { Some(r) },
                            });
                        }
                    };
                    let (ours, theirs) = (&names.ours.short, &names.theirs.short);
                    pick(ui, &format!("Use {ours}"), Resolution::Ours);
                    pick(ui, &format!("Use {theirs}"), Resolution::Theirs);
                    pick(
                        ui,
                        &format!("Both ({ours} first)"),
                        Resolution::OursThenTheirs,
                    );
                    pick(
                        ui,
                        &format!("Both ({theirs} first)"),
                        Resolution::TheirsThenOurs,
                    );
                    let editing = view.edits[i].is_some();
                    let edit = egui::Button::selectable(editing, "Edit…");
                    if can.why_not(ui.add_enabled(can.ok, edit)).clicked() {
                        if editing {
                            actions.push(Action::ResolveConflict {
                                index: i,
                                resolution: None,
                            });
                        } else {
                            actions.push(Action::EditConflict(i));
                        }
                    }
                });
                if let Some(text) = &mut view.edits[i] {
                    ui.add_space(4.0);
                    let rows = count_lines(text.as_bytes()).max(2);
                    ui.add(
                        TextEdit::multiline(text)
                            .font(theme::mono())
                            .desired_width(f32::INFINITY)
                            .desired_rows(rows),
                    );
                }
            });
        ui.add_space(10.0);
        line += count_lines(ours);
    }
}

fn describe(r: &Resolution, names: &SideNames) -> String {
    let (ours, theirs) = (&names.ours.short, &names.theirs.short);
    match r {
        Resolution::Ours => format!("using {ours}"),
        Resolution::Theirs => format!("using {theirs}"),
        Resolution::OursThenTheirs => format!("both, {ours} first"),
        Resolution::TheirsThenOurs => format!("both, {theirs} first"),
        Resolution::Custom(_) => "edited".to_owned(),
    }
}

/// One side of a conflict: a label and its lines.
fn side(ui: &mut Ui, name: &str, text: &[u8], accent: Color32) {
    ui.label(
        RichText::new(name)
            .color(accent)
            .size(theme::SMALL_SIZE)
            .strong(),
    );
    Frame::new()
        .fill(accent.gamma_multiply(0.07))
        .stroke(Stroke::new(1.0, accent.gamma_multiply(0.3)))
        .corner_radius(4)
        .inner_margin(Margin::symmetric(6, 4))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            if text.is_empty() {
                ui.label(
                    RichText::new("(nothing)")
                        .color(color::text_faint())
                        .italics()
                        .font(theme::mono()),
                );
            } else {
                code(ui, &String::from_utf8_lossy(text), color::text());
            }
        });
}

/// Lines of code, unwrapped; wide lines scroll sideways.
fn code(ui: &mut Ui, text: &str, fg: Color32) {
    let text = text.strip_suffix('\n').unwrap_or(text);
    let text = text.replace('\t', graft_core::diff::TAB);
    ScrollArea::horizontal()
        .id_salt(ui.next_auto_id())
        .auto_shrink([false, true])
        .show(ui, |ui| {
            // Not justified: `columns` would stretch the words of the text.
            ui.with_layout(egui::Layout::top_down(egui::Align::Min), |ui| {
                ui.add(
                    egui::Label::new(RichText::new(text).font(theme::mono()).color(fg)).extend(),
                );
            });
        });
}

/// The whole file as it will be saved; unresolved conflicts stand out.
fn result_panel(ui: &mut Ui, view: &ConflictView, file: &ConflictFile) {
    section_heading_row(ui, "RESULT");
    ui.add_space(4.0);
    let resolutions = view.effective();
    let mut job = LayoutJob::default();
    let plain = TextFormat {
        font_id: theme::mono(),
        color: color::text_weak(),
        ..Default::default()
    };
    for piece in conflicts::pieces(&file.segments, &resolutions) {
        match piece {
            Piece::Common(text) => {
                job.append(&String::from_utf8_lossy(text), 0.0, plain.clone());
            }
            Piece::Resolved(text) => job.append(
                &String::from_utf8_lossy(&text),
                0.0,
                TextFormat {
                    color: color::text(),
                    background: color::accent().gamma_multiply(0.12),
                    ..plain.clone()
                },
            ),
            Piece::Unresolved(n) => job.append(
                &format!("⟨ conflict {} not resolved yet ⟩\n", n + 1),
                0.0,
                TextFormat {
                    color: color::removed(),
                    background: color::removed().gamma_multiply(0.12),
                    ..plain.clone()
                },
            ),
        }
    }
    ScrollArea::both()
        .id_salt(("conflict-result-scroll", &file.path))
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
            ui.add(egui::Label::new(job).extend());
        });
}
