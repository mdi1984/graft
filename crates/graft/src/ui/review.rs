//! The Review tab: what the selected worktree's branch changes against its
//! base branch (uncommitted work included), with "Viewed" checkboxes and a
//! progress bar, a side-by-side diff, local comments on lines and files (copied
//! for a coding agent in one go), Open in IDE and Merge.

use egui::{
    Align, Frame, Key, Layout, Margin, Panel, Rect, RichText, ScrollArea, Sense, TextEdit, Ui,
    pos2, vec2,
};
use graft_core::review::{Review, ReviewFile};
use graft_core::{Change, RepoSnapshot, Worktree};

use super::can;
use super::diff_view::{self, Marks};
use super::file_row;
use super::widgets::{
    Badge, ButtonKind, ahead, button, empty_state, paint_check, paint_galley_left, progress_bar,
    waiting,
};
use crate::keys::Command;
use crate::project::comments::{self, LineComment};
use crate::project::{Action, Confirm, Project};
use crate::text::plural;
use crate::theme::{self, color};
use crate::ui::Actions;

pub fn show(ui: &mut Ui, project: &mut Project, actions: &mut Actions) {
    // Shared handles: the screen reads them while `project` takes actions.
    let Some(snap) = project.snapshot.clone() else {
        return;
    };
    let Some(wt) = snap.worktree_at(&project.nav.worktree) else {
        return;
    };
    let Some(view) = &project.review else {
        waiting(ui);
        return;
    };
    let review = match &view.data {
        None => {
            waiting(ui);
            return;
        }
        Some(Err(e)) => {
            let base = project.review_state().and_then(|s| s.base.clone());
            Panel::top("review-header")
                .frame(header_frame())
                .show(ui, |ui| {
                    ui.horizontal(|ui| base_picker(ui, &snap, wt, base.as_deref(), actions));
                });
            empty_state(ui, "Nothing to review", e);
            return;
        }
        Some(Ok(review)) => review.clone(),
    };

    let viewed = review.files.iter().filter(|f| project.is_viewed(f)).count();
    Panel::top("review-header")
        .frame(header_frame())
        .show(ui, |ui| {
            header(ui, project, &snap, wt, &review, viewed, actions)
        });

    if review.files.is_empty() {
        empty_state(
            ui,
            "No changes",
            &format!(
                "{} has nothing that is not in {} yet.",
                wt.label(),
                review.base
            ),
        );
        return;
    }

    Panel::left("review-files")
        .resizable(true)
        .default_size(340.0)
        .size_range(220.0..=640.0)
        .frame(
            Frame::new()
                .fill(color::app_bg())
                .inner_margin(Margin::symmetric(8, 8)),
        )
        .show(ui, |ui| {
            comments_bar(ui, project, wt, &review, actions);
            file_list(ui, project, &review, actions)
        });

    let selected = project.review.as_ref().and_then(|v| v.selected.clone());
    let Some(file) = selected.and_then(|p| review.files.iter().find(|f| f.change.path == p)) else {
        empty_state(ui, "No file selected", "Pick a file on the left.");
        return;
    };
    let file = file.clone();
    file_header(ui, project, wt, &file, actions);
    let path = file.change.path.as_str();
    let diff = project.commit_diff.as_ref().and_then(|d| d.diff.as_ref());
    let (placed, outdated) = match (diff, project.review_state()) {
        (Some(diff), Some(state)) => {
            let (placed, outdated) =
                comments::place(diff, state.comments.iter().filter(|c| c.path == path));
            let placed: Vec<_> = placed.into_iter().map(|(at, c)| (at, c.id)).collect();
            let outdated: Vec<LineComment> = outdated.into_iter().cloned().collect();
            (placed, outdated)
        }
        _ => (Vec::new(), Vec::new()),
    };
    comment_panel(ui, project, path, &outdated, actions);
    // Side by side only helps when there is an old and a new version.
    let both_sides = !matches!(file.change.change, Change::Added | Change::Deleted);
    let split = both_sides && project.review.as_ref().is_none_or(|v| v.split);
    let state_comments = project
        .review_state()
        .map(|s| s.comments.as_slice())
        .unwrap_or_default();
    let marks = Marks {
        pick: project.review.as_ref().and_then(|v| v.pick),
        placed: placed
            .iter()
            .filter_map(|&(at, id)| Some((at, state_comments.iter().find(|c| c.id == id)?)))
            .collect(),
    };
    match &project.commit_diff {
        Some(diff) => diff_view::body(
            ui,
            diff,
            project.is_operating(),
            split,
            project.settings().wrap_lines,
            Some(&marks),
            actions,
        ),
        None => waiting(ui),
    }
}

fn header_frame() -> Frame {
    Frame::new()
        .fill(color::bar_bg())
        .inner_margin(Margin::symmetric(14, 8))
}

fn header(
    ui: &mut Ui,
    project: &Project,
    snap: &RepoSnapshot,
    wt: &Worktree,
    review: &Review,
    viewed: usize,
    actions: &mut Actions,
) {
    let busy = project.is_operating();
    // Not enough room for everything on one line: the numbers and the
    // progress bar move to a second line instead of running under the buttons.
    let one_line = ui.available_width() > ONE_LINE_WIDTH;
    ui.horizontal(|ui| {
        let base = project.review_state().and_then(|s| s.base.clone());
        base_picker(
            ui,
            snap,
            wt,
            base.as_deref().or(Some(&review.base)),
            actions,
        );
        if one_line {
            stats(ui, review);
        }

        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            merge_button(ui, snap, wt, review, busy, actions);
            if button(ui, "Open in IDE", ButtonKind::Secondary, true)
                .on_hover_text(ide_tip(project, "this worktree"))
                .clicked()
            {
                actions.push(Action::OpenInIde(wt.path.clone()));
            }
            let (can, preview) = can::preview(snap, Some(wt));
            let label = match preview {
                Some(Action::UpdatePreview) => "Update preview",
                _ => "Preview in main folder",
            };
            if button(ui, label, ButtonKind::Secondary, can.ok && !busy)
                .on_hover_text(can.hint)
                .clicked()
                && let Some(action) = preview
            {
                actions.push(action);
            }
            if one_line {
                progress(ui, review, viewed);
            }
        });
    });
    if !one_line {
        ui.horizontal(|ui| {
            stats(ui, review);
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                progress(ui, review, viewed);
            });
        });
    }
}

/// What "Open in IDE" runs, from the settings.
fn ide_tip(project: &Project, what: &str) -> String {
    match project.settings().ide_command.trim() {
        "" => format!("Open {what} with its default program"),
        command => format!("Open {what} with {command}"),
    }
}

/// Header width below which it takes two lines.
const ONE_LINE_WIDTH: f32 = 1240.0;

/// "12 commits · 30 files  +120 −45".
fn stats(ui: &mut Ui, review: &Review) {
    let commits = plural(review.commits, "commit");
    let files = plural(review.files.len(), "file");
    ui.label(
        RichText::new(format!("{commits} · {files}"))
            .color(color::text_weak())
            .size(theme::SMALL_SIZE),
    );
    ui.label(
        RichText::new(format!("+{}", review.added()))
            .color(color::added())
            .size(theme::SMALL_SIZE),
    );
    ui.label(
        RichText::new(format!("−{}", review.removed()))
            .color(color::removed())
            .size(theme::SMALL_SIZE),
    );
}

/// "3 of 30 viewed" and a progress bar, laid out right to left.
fn progress(ui: &mut Ui, review: &Review, viewed: usize) {
    let total = review.files.len();
    ui.label(
        RichText::new(format!("{viewed} of {total} viewed"))
            .color(if viewed == total {
                color::added()
            } else {
                color::text_weak()
            })
            .size(theme::SMALL_SIZE),
    );
    ui.add_space(4.0);
    ui.vertical(|ui| {
        ui.add_space(13.0);
        progress_bar(ui, Some(viewed as f32 / total.max(1) as f32), 140.0);
    });
}

/// "Reviewing feature against [main ▾]".
fn base_picker(
    ui: &mut Ui,
    snap: &RepoSnapshot,
    wt: &Worktree,
    base: Option<&str>,
    actions: &mut Actions,
) {
    ui.label(RichText::new("Reviewing").color(color::text_weak()));
    ui.label(
        RichText::new(wt.label())
            .font(theme::mono())
            .color(color::dirty_fg()),
    );
    ui.label(RichText::new("against").color(color::text_weak()));
    let current = wt.branch.as_deref();
    egui::ComboBox::from_id_salt("review-base")
        .selected_text(RichText::new(base.unwrap_or("pick a branch")).font(theme::mono()))
        .height(360.0)
        .show_ui(ui, |ui| {
            let local = snap
                .branches
                .iter()
                .map(|b| b.name.as_str())
                .filter(|b| Some(*b) != current);
            let remote = snap.remote_branches.iter().map(|b| b.name.as_str());
            for name in local.chain(remote) {
                if ui
                    .selectable_label(base == Some(name), RichText::new(name).font(theme::mono()))
                    .clicked()
                {
                    actions.push(Action::SetReviewBase(name.to_owned()));
                }
            }
        });
}

/// Merge the reviewed branch into the base, where the base is checked out.
fn merge_button(
    ui: &mut Ui,
    snap: &RepoSnapshot,
    wt: &Worktree,
    review: &Review,
    busy: bool,
    actions: &mut Actions,
) {
    let base = review.base.as_str();
    let (can, target) = can::merge_reviewed(snap, wt, review);
    let label = format!("Merge into {base}…");
    if button(ui, &label, ButtonKind::Primary, can.ok && !busy)
        .on_hover_text(&can.hint)
        .on_disabled_hover_text(&can.hint)
        .clicked()
        && let (Some(branch), Some(t)) = (&wt.branch, target)
    {
        let mut body = format!(
            "Runs git merge {branch} in the worktree {} (where {base} is checked out).",
            graft_core::paths::folder_name(&t.path)
        );
        if wt.is_dirty() {
            body.push_str(&format!(
                "\n\n{} has {} uncommitted changes. They are not part of the merge; commit \
                 them first if they should be.",
                wt.label(),
                wt.dirty()
            ));
        }
        actions.push(Action::Ask(Box::new(Confirm {
            title: format!("Merge {branch} into {base}?"),
            body,
            confirm_label: "Merge".to_owned(),
            action: Action::MergeReviewed {
                worktree: t.path.clone(),
                branch: branch.clone(),
                into: base.to_owned(),
            },
        })));
    }
}

fn file_list(ui: &mut Ui, project: &Project, review: &Review, actions: &mut Actions) {
    let selected = project.review.as_ref().and_then(|v| v.selected.as_deref());
    ui.label(
        RichText::new(format!("FILES  {}", review.files.len()))
            .font(theme::semibold(theme::SMALL_SIZE))
            .color(color::text_faint()),
    );
    ui.add_space(4.0);
    ScrollArea::vertical()
        .id_salt("review-file-list")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 1.0;
            for file in &review.files {
                let path = &file.change.path;
                let viewed = project.is_viewed(file);
                let is_selected = selected == Some(path.as_str());
                let (rect, response) = file_row::allocate(ui, is_selected, Sense::click());
                let y = rect.center().y;
                let check = Rect::from_center_size(pos2(rect.left() + 12.0, y), vec2(14.0, 14.0));
                let check_response = ui.interact(
                    check.expand(4.0),
                    ui.scope_id().with(("viewed", path)),
                    Sense::click(),
                );
                paint_check(ui.painter(), check, viewed, check_response.hovered());

                let note = if file.untracked {
                    Some("untracked")
                } else if file.uncommitted {
                    Some("uncommitted")
                } else {
                    None
                };
                let (stats, stats_color) = match note {
                    Some(note) => (note.to_owned(), color::dirty_fg()),
                    None => (
                        file_row::stats(file.change.added, file.change.removed),
                        color::text_faint(),
                    ),
                };
                let stats = ui
                    .painter()
                    .layout_no_wrap(stats, theme::small(), stats_color);
                let stats_x = rect.right() - 6.0 - stats.size().x;
                paint_galley_left(ui.painter(), stats_x, y, stats);
                // Comments on this file: a badge left of the stats.
                let count = project
                    .review_state()
                    .map_or(0, |s| s.file_comment_count(path));
                let mut path_right = stats_x - 8.0;
                if count > 0 {
                    let badge = Badge::new(ui, count.to_string(), ahead());
                    path_right -= badge.width() + 6.0;
                    badge.paint(ui.painter(), path_right + 6.0, y);
                }
                file_row::paint_file(
                    ui,
                    rect.left() + 26.0,
                    y,
                    file.change.change,
                    path,
                    path_right,
                );
                if viewed {
                    // Dim viewed files so what is left stands out.
                    ui.painter().rect_filled(
                        Rect::from_min_max(pos2(rect.left() + 24.0, rect.top()), rect.max),
                        4,
                        color::app_bg().gamma_multiply(0.55),
                    );
                }

                if check_response.clicked() {
                    actions.push(Action::ToggleViewed(path.clone()));
                } else if response.clicked() {
                    actions.push(Action::SelectReviewFile(path.clone()));
                }
                response.on_hover_text(file_row::tip(path, file.change.orig_path.as_deref()));
            }
        });
}

fn file_header(
    ui: &mut Ui,
    project: &Project,
    wt: &Worktree,
    file: &ReviewFile,
    actions: &mut Actions,
) {
    let viewed = project.is_viewed(file);
    let split = project.review.as_ref().is_none_or(|v| v.split);
    let path = file.change.path.clone();
    Panel::top("review-file-header")
        .frame(
            Frame::new()
                .fill(color::bar_bg())
                .inner_margin(Margin::symmetric(12, 6)),
        )
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                let (rect, _) = ui.allocate_exact_size(vec2(18.0, 30.0), Sense::hover());
                file_row::paint_change_letter(ui, rect.left(), rect.center().y, file.change.change);
                ui.label(
                    RichText::new(&path)
                        .font(theme::semibold(theme::BODY_SIZE))
                        .color(color::text()),
                );
                if let Some(orig) = &file.change.orig_path {
                    ui.label(
                        RichText::new(format!("from {orig}"))
                            .color(color::text_faint())
                            .size(theme::SMALL_SIZE),
                    );
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let label = if viewed { "✓ Viewed" } else { "Mark viewed" };
                    let kind = if viewed {
                        ButtonKind::Secondary
                    } else {
                        ButtonKind::Primary
                    };
                    if button(ui, label, kind, true)
                        .on_hover_text(
                            project
                                .settings()
                                .keys
                                .tip("Marks it and opens the next file", Command::MarkViewed),
                        )
                        .clicked()
                    {
                        actions.push(Action::ToggleViewed(path.clone()));
                    }
                    if file.change.change != Change::Deleted
                        && button(ui, "Open in IDE", ButtonKind::Secondary, true)
                            .on_hover_text(ide_tip(project, "this file"))
                            .clicked()
                    {
                        actions.push(Action::OpenInIde(wt.path.join(&path)));
                    }
                    let both_sides = !matches!(file.change.change, Change::Added | Change::Deleted);
                    let toggle = if split { "Unified" } else { "Side by side" };
                    if both_sides && button(ui, toggle, ButtonKind::Secondary, true).clicked() {
                        actions.push(Action::ToggleReviewSplit);
                    }
                    if let Some(view) = &project.commit_diff {
                        diff_view::whole_file_button(ui, view, project.settings(), actions);
                    }
                });
            });
        });
    // Marks the open file as viewed (not while typing a comment).
    if project
        .settings()
        .keys
        .pressed(ui.ctx(), Command::MarkViewed)
    {
        actions.push(Action::ToggleViewed(path));
    }
}

/// "5 comments  [Copy for agent] [Clear…]" above the file list.
fn comments_bar(
    ui: &mut Ui,
    project: &mut Project,
    wt: &Worktree,
    review: &Review,
    actions: &mut Actions,
) {
    let Some(state) = project.review_state() else {
        return;
    };
    let count = state.comment_count();
    let mut copied = false;
    Panel::bottom("review-comments-bar")
        .frame(Frame::new().inner_margin(Margin {
            left: 0,
            right: 0,
            top: 8,
            bottom: 0,
        }))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                let label = match count {
                    0 => "No comments yet".to_owned(),
                    n => plural(n, "comment"),
                };
                ui.label(
                    RichText::new(label)
                        .color(color::text_weak())
                        .size(theme::SMALL_SIZE),
                );
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if button(ui, "Clear…", ButtonKind::Secondary, count > 0)
                        .on_hover_text("Remove all comments of this review")
                        .clicked()
                    {
                        actions.push(Action::Ask(Box::new(Confirm {
                            title: "Clear all comments?".to_owned(),
                            body: format!(
                                "Removes the {count} comments on {}, e.g. once your agent has \
                                 addressed them.",
                                wt.label()
                            ),
                            confirm_label: "Clear".to_owned(),
                            action: Action::ClearReviewComments,
                        })));
                    }
                    if button(ui, "Copy for agent", ButtonKind::Primary, count > 0)
                        .on_hover_text(
                            "Copy all comments with file, lines and code as Markdown, to paste \
                             into your coding agent",
                        )
                        .clicked()
                    {
                        let title = format!("{} against {}", wt.label(), review.base);
                        let text = comments::export(&title, &state.notes, &state.comments);
                        ui.ctx().copy_text(text);
                        copied = true;
                    }
                });
            });
        });
    if copied {
        project.infos.push(format!(
            "Copied {} for your agent",
            plural(count, "comment")
        ));
    }
}

/// Below the diff: the box for a comment on the picked lines (or the one
/// being edited); else the note on the whole file and outdated comments.
fn comment_panel(
    ui: &mut Ui,
    project: &mut Project,
    path: &str,
    outdated: &[LineComment],
    actions: &mut Actions,
) {
    let Some(view) = &project.review else { return };
    let diff = project.commit_diff.as_ref().and_then(|d| d.diff.as_ref());
    let target = match (view.editing, view.pick, diff) {
        (Some(id), _, _) => project
            .review_state()
            .and_then(|s| s.comments.iter().find(|c| c.id == id))
            .map(|c| format!("Edit comment on {}", c.lines_label().to_lowercase())),
        (None, Some(pick), Some(diff)) => comments::range(diff, &pick).map(|(side, a, b)| {
            format!(
                "Comment on {}",
                comments::lines_label(side, a, b).to_lowercase()
            )
        }),
        _ => None,
    };
    // Esc drops the pick; a typed comment is kept for the next pick.
    if target.is_some()
        && !ui.ctx().egui_wants_keyboard_input()
        && ui.input(|i| i.key_pressed(Key::Escape))
        && view.draft.trim().is_empty()
    {
        actions.push(Action::CancelComment);
    }
    match target {
        Some(title) => comment_editor(ui, project, &title, actions),
        None => file_note(ui, project, path, outdated, actions),
    }
}

fn comment_editor(ui: &mut Ui, project: &mut Project, title: &str, actions: &mut Actions) {
    let keys = project.settings().keys.clone();
    let Some(view) = &mut project.review else {
        return;
    };
    let editing = view.editing;
    Panel::bottom("review-notes")
        .frame(
            Frame::new()
                .fill(color::sidebar_bg())
                .inner_margin(Margin::symmetric(12, 8)),
        )
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(title)
                        .font(theme::semibold(theme::SMALL_SIZE + 1.0))
                        .color(color::text()),
                );
                let save = keys
                    .text(Command::SaveComment)
                    .map(|key| format!(" · {key} saves"))
                    .unwrap_or_default();
                ui.label(
                    RichText::new(format!(
                        "Shift-click or drag to pick more lines{save} · only on this computer"
                    ))
                    .color(color::text_faint())
                    .size(theme::SMALL_SIZE),
                );
            });
            // The save shortcut is taken before the text box turns Enter into a newline.
            let id = ui.make_persistent_id("review-comment-draft");
            let save_keys =
                ui.memory(|m| m.has_focus(id)) && keys.pressed(ui.ctx(), Command::SaveComment);
            let response = ui.add(
                TextEdit::multiline(&mut view.draft)
                    .id(id)
                    .hint_text("What should change here, or what is unclear?")
                    .desired_rows(3)
                    .desired_width(f32::INFINITY),
            );
            // Focus once the click that picked the lines is over; else that
            // click's release takes the focus away again.
            let pointer_idle = ui.input(|i| !i.pointer.any_down() && !i.pointer.any_released());
            if view.focus_draft && pointer_idle {
                view.focus_draft = false;
                response.request_focus();
            }
            let has_text = !view.draft.trim().is_empty();
            ui.add_space(2.0);
            ui.horizontal(|ui| {
                let save = if editing.is_some() {
                    "Save"
                } else {
                    "Add comment"
                };
                if button(ui, save, ButtonKind::Primary, has_text).clicked()
                    || (save_keys && has_text)
                {
                    actions.push(Action::SaveComment);
                }
                if button(ui, "Cancel", ButtonKind::Secondary, true).clicked() {
                    actions.push(Action::CancelComment);
                }
                if let Some(id) = editing {
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if button(ui, "Delete comment", ButtonKind::Secondary, true).clicked() {
                            actions.push(Action::DeleteComment(id));
                        }
                    });
                }
            });
        });
}

/// A local note on the whole open file, and comments whose lines changed
/// since (never committed or pushed).
fn file_note(
    ui: &mut Ui,
    project: &mut Project,
    path: &str,
    outdated: &[LineComment],
    actions: &mut Actions,
) {
    let has_note = project
        .review_state()
        .and_then(|s| s.file_note(path))
        .is_some();
    let id = ui.make_persistent_id(("review-note-open", path));
    let mut open = ui.data(|d| d.get_temp::<bool>(id)).unwrap_or(has_note);
    Panel::bottom("review-notes")
        .frame(
            Frame::new()
                .fill(color::sidebar_bg())
                .inner_margin(Margin::symmetric(12, 6)),
        )
        .show(ui, |ui| {
            for c in outdated {
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(format!("{} · outdated", c.lines_label()))
                            .color(color::dirty_fg())
                            .size(theme::SMALL_SIZE),
                    )
                    .on_hover_text("These lines changed since the comment. It is still copied.");
                    let text = c.text.lines().next().unwrap_or_default();
                    if ui
                        .add(
                            egui::Label::new(RichText::new(text).color(color::text_mid()))
                                .truncate()
                                .sense(Sense::click()),
                        )
                        .on_hover_text("Edit or delete")
                        .clicked()
                    {
                        actions.push(Action::EditComment(c.id));
                    }
                });
            }
            ui.horizontal(|ui| {
                let title = match (open, has_note) {
                    (true, _) => "Hide file comment",
                    (false, true) => "Show file comment",
                    (false, false) => "+ Comment on the whole file",
                };
                if ui
                    .add(
                        egui::Label::new(
                            RichText::new(title)
                                .color(color::text_mid())
                                .size(theme::SMALL_SIZE),
                        )
                        .sense(Sense::click()),
                    )
                    .clicked()
                {
                    open = !open;
                }
                ui.label(
                    RichText::new("Click a line in the diff to comment on it, drag for several.")
                        .color(color::text_faint())
                        .size(theme::SMALL_SIZE),
                );
            });
            if open {
                let note = project
                    .review_state_mut()
                    .notes
                    .entry(path.to_owned())
                    .or_default();
                ui.add(
                    TextEdit::multiline(note)
                        .hint_text("What to fix, ask or remember about this file…")
                        .desired_rows(3)
                        .desired_width(f32::INFINITY),
                );
            }
        });
    ui.data_mut(|d| d.insert_temp(id, open));
    // Drop empty notes so they do not pile up.
    if let Some(n) = project.review_state().and_then(|s| s.notes.get(path))
        && n.is_empty()
    {
        project.review_state_mut().notes.remove(path);
    }
}
