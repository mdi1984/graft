//! The Review tab: what the selected worktree's branch changes against its
//! base branch (uncommitted work included), with "Viewed" checkboxes and a
//! progress bar, a side-by-side diff, local comments on lines and files (copied
//! for a coding agent in one go), Open in IDE and Merge.

use egui::{
    Align, Frame, Id, Key, Layout, Margin, Panel, Rect, RichText, ScrollArea, Sense, TextEdit, Ui,
    pos2, vec2,
};
use graft_core::review::{Review, ReviewFile};
use graft_core::{Change, RepoSnapshot, Worktree};

use super::AppAction;
use super::can;
use super::diff_view::{self, Marks};
use super::file_row::{self, Extras, file_row};
use super::file_tree::{self, Row};
use super::widgets::{
    ButtonKind, button, empty_state, fits, folder_row, paint_check, progress_bar, remember_width,
    section_with_link, waiting,
};
use crate::keys::{self, Command};
use crate::project::comments::{self, LineComment};
use crate::project::{Action, Confirm, DiffOwner, Project};
use crate::text::plural;
use crate::theme::{self, color};
use crate::ui::Actions;

pub fn show(ui: &mut Ui, project: &mut Project, actions: &mut Actions) {
    // A cheap clone (shared): `snap` and `wt` stay readable below while
    // `project` is lent out mutably, e.g. to the comment editor.
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
            comments_bar(ui, project, wt, actions);
            file_list(ui, project, &review, actions)
        });

    let selected = project.review.as_ref().and_then(|v| v.selected.clone());
    let Some(file) = selected.and_then(|p| review.files.iter().find(|f| f.change.path == p)) else {
        empty_state(ui, "No file selected", "Pick a file on the left.");
        return;
    };
    let file = file.clone();
    // Side by side only helps when there is an old and a new version; then
    // it can be toggled.
    let both_sides = !matches!(file.change.change, Change::Added | Change::Deleted);
    let split = both_sides.then(|| project.review.as_ref().is_none_or(|v| v.split));
    file_header(ui, project, wt, &file, split, actions);
    let path = file.change.path.as_str();
    let diff = project
        .diff(DiffOwner::Review)
        .and_then(|d| d.diff.as_ref());
    // Copies: the comment panel below may change the comments.
    let (placed, outdated): (Vec<_>, Vec<LineComment>) = match (diff, project.review_state()) {
        (Some(diff), Some(state)) => {
            let (placed, outdated) =
                comments::place(diff, state.comments.iter().filter(|c| c.path == path));
            (
                placed.into_iter().map(|(at, c)| (at, c.clone())).collect(),
                outdated.into_iter().cloned().collect(),
            )
        }
        _ => (Vec::new(), Vec::new()),
    };
    comment_panel(ui, project, path, &outdated, actions);
    let marks = Marks {
        pick: project.review.as_ref().and_then(|v| v.pick),
        placed: placed.iter().map(|(at, c)| (*at, c)).collect(),
    };
    match project.diff(DiffOwner::Review) {
        Some(diff) => diff_view::body(
            ui,
            diff,
            &can::act(project, ""),
            split == Some(true),
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
    let blocked = project.can_act().err();
    // Not enough room for everything on one line (as wide as it measured
    // last frame): the numbers and the progress bar move to a second line
    // instead of running under the buttons.
    let needed = Id::unique("review-header-width");
    let one_line = fits(ui, needed, ui.available_width());
    let mut width = 0.0;
    ui.horizontal(|ui| {
        let start = ui.cursor().left();
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
        width += ui.cursor().left() - start;

        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            merge_button(ui, snap, wt, review, blocked, actions);
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
            let can = can.unless(blocked);
            if button(ui, label, ButtonKind::Secondary, can.ok)
                .on_hover_text(can.hint)
                .clicked()
                && let Some(action) = preview
            {
                actions.push(action);
            }
            if one_line {
                progress(ui, review, viewed);
            }
            width += ui.min_rect().width();
        });
    });
    if !one_line {
        ui.horizontal(|ui| {
            let start = ui.cursor().left();
            stats(ui, review);
            width += ui.cursor().left() - start;
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                progress(ui, review, viewed);
                // On one line, the usual gap separates these from the buttons.
                width += ui.min_rect().width() + ui.spacing().item_spacing.x;
            });
        });
    }
    // Both layouts measure what one line needs, so the header goes back to
    // one line once that fits again (e.g. a shorter base branch).
    remember_width(ui, needed, width);
}

/// What "Open in IDE" runs, from the settings.
fn ide_tip(project: &Project, what: &str) -> String {
    match project.settings().ide_command.trim() {
        "" => format!("Open {what} with its default program"),
        command => format!("Open {what} with {command}"),
    }
}

/// "12 commits · 30 files  +120 −45".
fn stats(ui: &mut Ui, review: &Review) {
    let commits = plural(review.commits, "commit", "commits");
    let files = plural(review.files.len(), "file", "files");
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
    // Right-aligned like the rest of the row: `ui.vertical` would put the
    // bar at the far left of the space that is left.
    ui.with_layout(Layout::top_down(Align::Max), |ui| {
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
    // What could be merged into the branch is what it can be reviewed
    // against (a detached HEAD: every branch).
    let current = wt.branch.as_deref().unwrap_or_default();
    egui::ComboBox::from_id_salt("review-base")
        .selected_text(RichText::new(base.unwrap_or("pick a branch")).font(theme::mono()))
        .height(360.0)
        .show_ui(ui, |ui| {
            for name in snap.merge_choices(current) {
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
    blocked: Option<&str>,
    actions: &mut Actions,
) {
    let base = review.base.as_str();
    let (can, target) = can::merge_reviewed(snap, wt, review);
    let can = can.unless(blocked);
    let label = format!("Merge into {base}…");
    if button(ui, &label, ButtonKind::Primary, can.ok)
        .on_hover_text(&can.hint)
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
                head: review.head,
            },
        })));
    }
}

/// How far each folder level indents the tree.
const NEST: f32 = 14.0;

fn file_list(ui: &mut Ui, project: &Project, review: &Review, actions: &mut Actions) {
    let selected = project.review.as_ref().and_then(|v| v.selected.as_deref());
    let folders = project.settings().review_folders;
    let link = if folders {
        ("Show list", "List the files without their folders")
    } else {
        ("Show folders", "Show the files under their folders")
    };
    let heading = format!("FILES  {}", review.files.len());
    if section_with_link(ui, &heading, Some(link), &can::yes("")) {
        actions.app(AppAction::ToggleReviewFolders);
    }
    ScrollArea::vertical()
        .id_salt("review-file-list")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 1.0;
            if folders {
                file_tree(ui, project, review, selected, actions);
            } else {
                for file in &review.files {
                    review_file_row(ui, project, file, selected, None, actions);
                }
            }
        });
}

/// The files under their folders; a folded folder hides what is inside.
fn file_tree(
    ui: &mut Ui,
    project: &Project,
    review: &Review,
    selected: Option<&str>,
    actions: &mut Actions,
) {
    let paths: Vec<&str> = review
        .files
        .iter()
        .map(|f| f.change.path.as_str())
        .collect();
    let mut folded: Option<usize> = None;
    for row in file_tree::rows(&paths) {
        if folded.is_some_and(|depth| row.depth() > depth) {
            continue;
        }
        folded = None;
        match row {
            Row::Folder {
                path,
                name,
                depth,
                files,
            } => {
                let inside = &review.files[files.clone()];
                let viewed = inside.iter().filter(|f| project.is_viewed(f)).count();
                let marked = selected.is_some_and(|s| paths[files].contains(&s));
                let open = folder_row(
                    ui,
                    Id::unique(("review-folder", &project.root, path)),
                    name,
                    theme::body(),
                    &format!("{viewed}/{}", inside.len()),
                    18.0 + depth as f32 * NEST,
                    marked,
                );
                if !open {
                    folded = Some(depth);
                }
            }
            Row::File { index, depth } => {
                let file = &review.files[index];
                review_file_row(ui, project, file, selected, Some(depth), actions);
            }
        }
    }
}

/// One file with its Viewed box; `depth` in a tree, `None` in the flat list.
fn review_file_row(
    ui: &mut Ui,
    project: &Project,
    file: &ReviewFile,
    selected: Option<&str>,
    depth: Option<usize>,
    actions: &mut Actions,
) {
    let path = &file.change.path;
    let viewed = project.is_viewed(file);
    let note = if file.untracked {
        Some("untracked")
    } else if file.uncommitted {
        Some("uncommitted")
    } else {
        None
    };
    let extras = Extras {
        inset: 22.0 + depth.unwrap_or(0) as f32 * NEST,
        note: note.map(|note| (note, color::dirty_fg())),
        // Comments on this file.
        badge: project
            .review_state()
            .map_or(0, |s| s.file_comment_count(path)),
        name_only: depth.is_some(),
    };
    let response = file_row(ui, &file.change, selected == Some(path.as_str()), extras);
    let rect = response.rect;
    let y = rect.center().y;
    let check = Rect::from_center_size(pos2(rect.left() + 12.0, y), vec2(14.0, 14.0));
    let check_response = ui.interact(
        check.expand(4.0),
        ui.scope_id().with(("viewed", path)),
        Sense::click(),
    );
    paint_check(ui.painter(), check, viewed, check_response.hovered());
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
}

/// The open file's name and buttons; `split` is whether it shows side by
/// side, `None` when it cannot.
fn file_header(
    ui: &mut Ui,
    project: &Project,
    wt: &Worktree,
    file: &ReviewFile,
    split: Option<bool>,
    actions: &mut Actions,
) {
    let viewed = project.is_viewed(file);
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
                    if let Some(split) = split {
                        let toggle = if split { "Unified" } else { "Side by side" };
                        if button(ui, toggle, ButtonKind::Secondary, true).clicked() {
                            actions.push(Action::ToggleReviewSplit);
                        }
                    }
                    if let Some(view) = project.diff(DiffOwner::Review) {
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
fn comments_bar(ui: &mut Ui, project: &Project, wt: &Worktree, actions: &mut Actions) {
    let Some(state) = project.review_state() else {
        return;
    };
    let count = state.comment_count();
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
                    n => plural(n, "comment", "comments"),
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
                        actions.push(Action::CopyReviewComments);
                    }
                });
            });
        });
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
    let diff = project
        .diff(DiffOwner::Review)
        .and_then(|d| d.diff.as_ref());
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
    if target.is_some() && view.draft.trim().is_empty() && keys::plain_key(ui.ctx(), Key::Escape) {
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
    project: &Project,
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
                let mut note = project
                    .review_state()
                    .and_then(|s| s.notes.get(path))
                    .cloned()
                    .unwrap_or_default();
                let edit = TextEdit::multiline(&mut note)
                    .hint_text("What to fix, ask or remember about this file…")
                    .desired_rows(3)
                    .desired_width(f32::INFINITY);
                if ui.add(edit).changed() {
                    actions.push(Action::SetFileNote {
                        path: path.to_owned(),
                        text: note,
                    });
                }
            }
        });
    ui.data_mut(|d| d.insert_temp(id, open));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{git, open};

    /// The header measures what one line needs alike on one line and on
    /// two: it neither flickers between them nor stays on two lines once
    /// one fits again.
    #[test]
    fn both_header_layouts_measure_one_line_alike() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("app");
        std::fs::create_dir(&root).unwrap();
        git(&root, &["init", "-q", "-b", "main"]);
        std::fs::write(root.join("a.txt"), "a\n").unwrap();
        git(&root, &["add", "a.txt"]);
        git(&root, &["commit", "-q", "-m", "first"]);
        let project = open(&root);
        let snap = project.snapshot.clone().unwrap();
        let wt = project.worktree().unwrap().clone();
        let review = Review {
            base: "main".to_owned(),
            merge_base: wt.head.unwrap(),
            head: wt.head.unwrap(),
            commits: 12,
            files: Vec::new(),
        };

        let ctx = egui::Context::default();
        crate::fonts::install(&ctx);
        theme::apply(&ctx, egui::ThemePreference::Dark);
        let needed = Id::unique("review-header-width");
        let measure = |width: f32| {
            let input = egui::RawInput {
                screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(width, 400.0))),
                ..Default::default()
            };
            let output = ctx.run_ui(input, |ui| {
                let mut actions = Actions::default();
                header(ui, &project, &snap, &wt, &review, 0, &mut actions);
            });
            output.drop_without_applying_deltas();
            ctx.data(|d| d.get_temp::<f32>(needed)).unwrap()
        };
        let one_line = measure(4000.0);
        let two_lines = measure(one_line - 1.0);
        assert!(
            (one_line - two_lines).abs() < 0.01,
            "{one_line} vs {two_lines}"
        );
        assert_eq!(measure(one_line), one_line, "back on one line");
    }
}
