//! Conflicts: the banner while a merge, rebase, cherry-pick or revert is in
//! progress, and the resolver for a conflicted file — one card per conflict
//! (Ours | Base | Theirs → use ours / theirs / both / edit) next to a live
//! preview of the result.

use egui::text::{LayoutJob, TextFormat};
use egui::{Color32, Frame, Margin, Panel, RichText, ScrollArea, Stroke, TextEdit, Ui, vec2};
use graft_core::Worktree;
use graft_core::conflicts::{ConflictFile, InProgress, Kind, Resolution, Segment};

use super::widgets::{ButtonKind, button, empty_state, spinner};
use crate::project::{Action, ConflictView, Project};
use crate::theme::{self, RADIUS, color};

const OURS: Color32 = Color32::from_rgb(0x8c, 0xc2, 0xff);
const THEIRS: Color32 = Color32::from_rgb(0xd6, 0xa3, 0xff);
/// Lines of clean context shown around each conflict.
const CONTEXT: usize = 3;

/// Names for the two sides. In a rebase, "ours" is the branch being rebased
/// onto and "theirs" is your commit being replayed.
fn side_names(wt: &Worktree) -> (String, String) {
    let branch = wt.branch.clone().unwrap_or_else(|| "HEAD".to_owned());
    match wt.in_progress.as_ref().map(|p| p.kind) {
        Some(Kind::Rebase) => ("Upstream".to_owned(), "Your commit".to_owned()),
        Some(Kind::CherryPick) => (format!("Current ({branch})"), "Picked commit".to_owned()),
        Some(Kind::Revert) => (format!("Current ({branch})"), "Reverted commit".to_owned()),
        _ => (format!("Current ({branch})"), "Incoming".to_owned()),
    }
}

/// The strip across the top of the Workspace while something is in progress.
pub fn banner(ui: &mut Ui, project: &Project, wt: &Worktree, actions: &mut Vec<Action>) {
    let Some(InProgress { kind, title }) = &wt.in_progress else {
        return;
    };
    let left = wt.status.as_ref().map_or(0, |s| s.conflicted().count());
    let busy = project.is_operating();
    Panel::top("in-progress-banner")
        .frame(
            Frame::new()
                .fill(color::DIRTY_BG)
                .inner_margin(Margin::symmetric(14, 8))
                .stroke(Stroke::new(1.0, color::SLASH_BORDER)),
        )
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(format!("{} in progress", kind.noun()))
                        .font(theme::semibold(theme::BODY_SIZE))
                        .color(color::DIRTY_FG),
                );
                ui.label(RichText::new(title).color(color::TEXT_MID));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let can_continue = left == 0 && !busy;
                    let r = button(ui, "Continue", ButtonKind::Primary, can_continue);
                    let r = if left > 0 {
                        r.on_hover_text("Resolve every conflict first")
                    } else {
                        r.on_hover_text(match kind {
                            Kind::Rebase => "Commit this step and replay the next commit",
                            _ => "Commit the result",
                        })
                    };
                    if r.clicked() {
                        actions.push(Action::ContinueOp(*kind));
                    }
                    if button(ui, "Abort", ButtonKind::Secondary, !busy).clicked() {
                        actions.push(Action::AbortOp {
                            kind: *kind,
                            confirmed: false,
                        });
                    }
                    let text = match left {
                        0 => "All conflicts resolved".to_owned(),
                        1 => "1 file with conflicts".to_owned(),
                        n => format!("{n} files with conflicts"),
                    };
                    ui.label(RichText::new(text).color(if left == 0 {
                        color::ADDED
                    } else {
                        color::REMOVED
                    }));
                });
            });
        });
}

/// The resolver for `project.conflict`, in place of the diff.
pub fn resolver(ui: &mut Ui, project: &mut Project, actions: &mut Vec<Action>) {
    let busy = project.is_operating();
    let (ours_name, theirs_name) = project
        .worktree()
        .map(side_names)
        .unwrap_or_else(|| ("Ours".to_owned(), "Theirs".to_owned()));
    let Some(view) = &mut project.conflict else {
        return;
    };
    let file = match &view.file {
        None => {
            let rect = ui.available_rect_before_wrap();
            spinner(ui, rect.center(), 9.0, color::TEXT_WEAK);
            return;
        }
        Some(Err(e)) => {
            empty_state(ui, "Could not read the conflict", e);
            return;
        }
        Some(Ok(file)) => file.clone(),
    };

    header(ui, view, &file, busy, actions);

    if file.segments.is_empty() {
        whole_file(ui, &file, &ours_name, &theirs_name, busy, actions);
        return;
    }

    let width = ui.available_width();
    Panel::right("conflict-result")
        .resizable(true)
        .default_size(width * 0.38)
        .size_range(220.0..=(width * 0.6).max(240.0))
        .frame(
            Frame::new()
                .fill(color::SIDEBAR_BG)
                .inner_margin(Margin::symmetric(12, 10)),
        )
        .show(ui, |ui| result_panel(ui, view, &file));

    ScrollArea::vertical()
        .id_salt(("conflict-cards", &file.path))
        .auto_shrink([false, false])
        .show(ui, |ui| {
            Frame::new()
                .inner_margin(Margin::symmetric(12, 10))
                .show(ui, |ui| {
                    cards(ui, view, &file, &ours_name, &theirs_name, busy, actions)
                });
        });
}

fn header(
    ui: &mut Ui,
    view: &ConflictView,
    file: &ConflictFile,
    busy: bool,
    actions: &mut Vec<Action>,
) {
    Panel::top("conflict-header")
        .frame(
            Frame::new()
                .fill(color::BAR_BG)
                .inner_margin(Margin::symmetric(12, 6)),
        )
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(&file.path)
                        .font(theme::semibold(theme::BODY_SIZE))
                        .color(color::TEXT),
                );
                let total = view.resolutions.len();
                if total > 0 {
                    let done = view.resolved();
                    ui.label(
                        RichText::new(format!("{done} of {total} conflicts resolved"))
                            .color(if done == total {
                                color::ADDED
                            } else {
                                color::TEXT_WEAK
                            })
                            .size(theme::SMALL_SIZE),
                    );
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if total > 0 {
                        let ready = view.result().is_some();
                        if button(ui, "Save resolution", ButtonKind::Primary, ready && !busy)
                            .on_hover_text("Write the result to the file and stage it")
                            .clicked()
                        {
                            actions.push(Action::SaveResolution);
                        }
                    }
                    ui.menu_button("More", |ui| {
                        if ui
                            .add_enabled(!busy, egui::Button::new("Take ours for the whole file"))
                            .clicked()
                        {
                            actions.push(Action::TakeSide {
                                path: file.path.clone(),
                                ours: true,
                            });
                            ui.close();
                        }
                        if ui
                            .add_enabled(!busy, egui::Button::new("Take theirs for the whole file"))
                            .clicked()
                        {
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
                        if ui
                            .add_enabled(!busy, egui::Button::new("Mark as resolved"))
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
    ours_name: &str,
    theirs_name: &str,
    busy: bool,
    actions: &mut Vec<Action>,
) {
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
                .font(theme::semibold(15.0))
                .color(color::TEXT_MID),
        );
        ui.add_space(4.0);
        ui.label(RichText::new(explanation).color(color::TEXT_WEAK));
        ui.add_space(12.0);
        ui.horizontal(|ui| {
            let label = |name: &str, side: &Option<_>| match side {
                None => format!("Delete it ({name})"),
                Some(_) => format!("Keep {name}"),
            };
            let w = 2.0 * 180.0 + 8.0;
            ui.add_space((ui.available_width() - w).max(0.0) / 2.0);
            if button(
                ui,
                &label(ours_name, &file.ours),
                ButtonKind::Secondary,
                !busy,
            )
            .clicked()
            {
                actions.push(Action::TakeSide {
                    path: file.path.clone(),
                    ours: true,
                });
            }
            if button(
                ui,
                &label(theirs_name, &file.theirs),
                ButtonKind::Secondary,
                !busy,
            )
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
    ours_name: &str,
    theirs_name: &str,
    busy: bool,
    actions: &mut Vec<Action>,
) {
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
            .fill(color::SIDEBAR_BG)
            .stroke(Stroke::new(
                1.0,
                if resolution.is_some() {
                    color::ACCENT.gamma_multiply(0.45)
                } else {
                    color::BORDER
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
                            .color(color::TEXT),
                    );
                    ui.label(
                        RichText::new(format!("line {line}"))
                            .color(color::TEXT_FAINT)
                            .size(theme::SMALL_SIZE),
                    );
                    if let Some(r) = &resolution {
                        ui.label(
                            RichText::new(format!("✓ {}", describe(r, ours_name, theirs_name)))
                                .color(color::ADDED)
                                .size(theme::SMALL_SIZE),
                        );
                    }
                });
                if !before.is_empty() {
                    code(ui, &before, color::TEXT_FAINT);
                }
                let columns = if base.is_empty() { 2 } else { 3 };
                // Side by side when each column has room for a line of code,
                // stacked otherwise.
                if ui.available_width() < columns as f32 * 300.0 {
                    side(ui, ours_name, ours, OURS);
                    if columns == 3 {
                        side(ui, "Base", base, color::TEXT_FAINT);
                    }
                    side(ui, theirs_name, theirs, THEIRS);
                } else {
                    ui.columns(columns, |cols| {
                        side(&mut cols[0], ours_name, ours, OURS);
                        if columns == 3 {
                            side(&mut cols[1], "Base", base, color::TEXT_FAINT);
                        }
                        side(&mut cols[columns - 1], theirs_name, theirs, THEIRS);
                    });
                }
                if !after.is_empty() {
                    code(ui, &after, color::TEXT_FAINT);
                }
                ui.add_space(4.0);
                ui.horizontal_wrapped(|ui| {
                    let mut pick = |ui: &mut Ui, label: &str, r: Resolution| {
                        let selected =
                            resolution.as_ref() == Some(&r) && !matches!(r, Resolution::Custom(_));
                        if ui
                            .add_enabled(!busy, egui::Button::selectable(selected, label))
                            .clicked()
                        {
                            actions.push(Action::ResolveConflict {
                                index: i,
                                resolution: if selected { None } else { Some(r) },
                            });
                        }
                    };
                    pick(
                        ui,
                        &format!("Use {}", short_name(ours_name)),
                        Resolution::Ours,
                    );
                    pick(
                        ui,
                        &format!("Use {}", short_name(theirs_name)),
                        Resolution::Theirs,
                    );
                    let both = |first: &str| format!("Both ({} first)", short_name(first));
                    pick(ui, &both(ours_name), Resolution::OursThenTheirs);
                    pick(ui, &both(theirs_name), Resolution::TheirsThenOurs);
                    let editing = view.edits[i].is_some();
                    if ui
                        .add_enabled(!busy, egui::Button::selectable(editing, "Edit…"))
                        .clicked()
                    {
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

/// "Current (main)" → "current"; "Incoming" → "incoming".
fn short_name(name: &str) -> String {
    name.split(" (").next().unwrap_or(name).to_lowercase()
}

fn describe(r: &Resolution, ours: &str, theirs: &str) -> String {
    match r {
        Resolution::Ours => format!("using {}", short_name(ours)),
        Resolution::Theirs => format!("using {}", short_name(theirs)),
        Resolution::OursThenTheirs => format!("both, {} first", short_name(ours)),
        Resolution::TheirsThenOurs => format!("both, {} first", short_name(theirs)),
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
                        .color(color::TEXT_FAINT)
                        .italics()
                        .font(theme::mono()),
                );
            } else {
                code(ui, &String::from_utf8_lossy(text), color::TEXT);
            }
        });
}

/// Lines of code, unwrapped; wide lines scroll sideways.
fn code(ui: &mut Ui, text: &str, fg: Color32) {
    let text = text.strip_suffix('\n').unwrap_or(text);
    let text = text.replace('\t', "    ");
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
    ui.label(
        RichText::new("RESULT")
            .font(theme::semibold(theme::SMALL_SIZE))
            .color(color::TEXT_FAINT),
    );
    ui.add_space(4.0);
    let resolutions = view.effective();
    let mut job = LayoutJob::default();
    let plain = TextFormat {
        font_id: theme::mono(),
        color: color::TEXT_WEAK,
        ..Default::default()
    };
    let mut index = 0;
    for segment in &file.segments {
        match segment {
            Segment::Common(text) => {
                job.append(&String::from_utf8_lossy(text), 0.0, plain.clone());
            }
            Segment::Conflict { .. } => {
                let resolution = resolutions.get(index).cloned().flatten();
                index += 1;
                match resolution {
                    Some(r) => {
                        let bytes = graft_core::conflicts::assemble(
                            std::slice::from_ref(segment),
                            &[Some(r)],
                        )
                        .unwrap_or_default();
                        job.append(
                            &String::from_utf8_lossy(&bytes),
                            0.0,
                            TextFormat {
                                color: color::TEXT,
                                background: color::ACCENT.gamma_multiply(0.12),
                                ..plain.clone()
                            },
                        );
                    }
                    None => job.append(
                        &format!("⟨ conflict {index} not resolved yet ⟩\n"),
                        0.0,
                        TextFormat {
                            color: color::REMOVED,
                            background: color::REMOVED.gamma_multiply(0.12),
                            ..plain.clone()
                        },
                    ),
                }
            }
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

fn count_lines(text: &[u8]) -> usize {
    let n = text.iter().filter(|b| **b == b'\n').count();
    if text.last().is_some_and(|b| *b != b'\n') {
        n + 1
    } else {
        n
    }
}

fn last_lines(text: &[u8], n: usize) -> String {
    let text = String::from_utf8_lossy(text);
    let lines: Vec<&str> = text.lines().collect();
    lines[lines.len().saturating_sub(n)..].join("\n")
}

fn first_lines(text: &[u8], n: usize) -> String {
    let text = String::from_utf8_lossy(text);
    text.lines().take(n).collect::<Vec<_>>().join("\n")
}
