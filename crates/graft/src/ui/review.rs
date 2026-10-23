//! The Review tab: what the selected worktree's branch changes against its
//! base branch (uncommitted work included), with "Viewed" checkboxes and a
//! progress bar, a side-by-side diff, local notes per file, Open in IDE and
//! Merge.

use egui::{
    Align, Frame, Key, Layout, Margin, Panel, Rect, RichText, ScrollArea, Sense, Stroke,
    StrokeKind, TextEdit, Ui, pos2, vec2,
};
use graft_core::review::{Review, ReviewFile};
use graft_core::status::Change;
use graft_core::{RepoSnapshot, Worktree, same_path};

use super::diff_view;
use super::history::{paint_change_letter, paint_path};
use super::widgets::{ButtonKind, button, empty_state, progress_bar, spinner};
use crate::project::{Action, Confirm, Project};
use crate::theme::{self, color};

pub fn show(ui: &mut Ui, project: &mut Project, actions: &mut Vec<Action>) {
    let (Some(snap), Some(wt)) = (project.snapshot.clone(), project.worktree().cloned()) else {
        return;
    };
    let Some(view) = &project.review else {
        spinner(
            ui,
            ui.available_rect_before_wrap().center(),
            9.0,
            color::TEXT_WEAK,
        );
        return;
    };
    let review = match &view.data {
        None => {
            spinner(
                ui,
                ui.available_rect_before_wrap().center(),
                9.0,
                color::TEXT_WEAK,
            );
            return;
        }
        Some(Err(e)) => {
            let base = project.review_state().and_then(|s| s.base.clone());
            Panel::top("review-header")
                .frame(header_frame())
                .show(ui, |ui| {
                    ui.horizontal(|ui| base_picker(ui, &snap, &wt, base.as_deref(), actions));
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
            header(ui, project, &snap, &wt, &review, viewed, actions)
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

    let uncommitted: Vec<&str> = wt
        .status
        .as_ref()
        .map(|s| s.entries.iter().map(|e| e.path.as_str()).collect())
        .unwrap_or_default();
    Panel::left("review-files")
        .resizable(true)
        .default_size(340.0)
        .size_range(220.0..=640.0)
        .frame(
            Frame::new()
                .fill(color::APP_BG)
                .inner_margin(Margin::symmetric(8, 8)),
        )
        .show(ui, |ui| {
            file_list(ui, project, &review, &uncommitted, actions)
        });

    let selected = project.review.as_ref().and_then(|v| v.selected.clone());
    let Some(file) = selected.and_then(|p| review.files.iter().find(|f| f.change.path == p)) else {
        empty_state(ui, "No file selected", "Pick a file on the left.");
        return;
    };
    let file = file.clone();
    file_header(ui, project, &wt, &file, actions);
    notes(ui, project, &file.change.path);
    // Side by side only helps when there is an old and a new version.
    let both_sides = !matches!(file.change.change, Change::Added | Change::Deleted);
    let split = both_sides && project.review.as_ref().is_none_or(|v| v.split);
    match &project.commit_diff {
        Some(diff) => diff_view::body(ui, diff, project.is_operating(), split, actions),
        None => spinner(
            ui,
            ui.available_rect_before_wrap().center(),
            9.0,
            color::TEXT_WEAK,
        ),
    }
}

fn header_frame() -> Frame {
    Frame::new()
        .fill(color::BAR_BG)
        .inner_margin(Margin::symmetric(14, 8))
}

fn header(
    ui: &mut Ui,
    project: &Project,
    snap: &RepoSnapshot,
    wt: &Worktree,
    review: &Review,
    viewed: usize,
    actions: &mut Vec<Action>,
) {
    let busy = project.is_operating();
    ui.horizontal(|ui| {
        let base = project.review_state().and_then(|s| s.base.clone());
        base_picker(
            ui,
            snap,
            wt,
            base.as_deref().or(Some(&review.base)),
            actions,
        );
        let commits = match review.commits {
            1 => "1 commit".to_owned(),
            n => format!("{n} commits"),
        };
        let files = match review.files.len() {
            1 => "1 file".to_owned(),
            n => format!("{n} files"),
        };
        ui.label(
            RichText::new(format!("{commits} · {files}"))
                .color(color::TEXT_WEAK)
                .size(theme::SMALL_SIZE),
        );
        ui.label(
            RichText::new(format!("+{}", review.added()))
                .color(color::ADDED)
                .size(theme::SMALL_SIZE),
        );
        ui.label(
            RichText::new(format!("−{}", review.removed()))
                .color(color::REMOVED)
                .size(theme::SMALL_SIZE),
        );

        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            merge_button(ui, snap, wt, review, busy, actions);
            if button(ui, "Open in IDE", ButtonKind::Secondary, true)
                .on_hover_text("Open this worktree in VS Code")
                .clicked()
            {
                actions.push(Action::OpenInIde(wt.path.clone()));
            }
            let total = review.files.len();
            ui.label(
                RichText::new(format!("{viewed} of {total} viewed"))
                    .color(if viewed == total {
                        color::ADDED
                    } else {
                        color::TEXT_WEAK
                    })
                    .size(theme::SMALL_SIZE),
            );
            ui.add_space(4.0);
            ui.vertical(|ui| {
                ui.add_space(13.0);
                progress_bar(ui, Some(viewed as f32 / total.max(1) as f32), 140.0);
            });
        });
    });
}

/// "Reviewing feature against [main ▾]".
fn base_picker(
    ui: &mut Ui,
    snap: &RepoSnapshot,
    wt: &Worktree,
    base: Option<&str>,
    actions: &mut Vec<Action>,
) {
    ui.label(RichText::new("Reviewing").color(color::TEXT_WEAK));
    ui.label(
        RichText::new(wt.label())
            .font(theme::mono())
            .color(color::DIRTY_FG),
    );
    ui.label(RichText::new("against").color(color::TEXT_WEAK));
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
    actions: &mut Vec<Action>,
) {
    let base = review.base.as_str();
    let target = snap.worktree_for_branch(base);
    let (enabled, hint) = match (&wt.branch, target) {
        (None, _) => (false, "Check out a branch to merge it".to_owned()),
        _ if review.commits == 0 => (
            false,
            "No commits to merge yet; uncommitted changes are not merged".to_owned(),
        ),
        (Some(_), None) => (
            false,
            format!("{base} is not checked out in any worktree; check it out to merge into it"),
        ),
        (Some(_), Some(t)) if same_path(&t.path, &wt.path) => {
            (false, "This worktree is the base itself".to_owned())
        }
        (Some(_), Some(t)) if t.in_progress.is_some() => {
            (false, format!("{base} has a merge or rebase in progress"))
        }
        (Some(branch), Some(t)) => (
            !busy,
            format!("Run git merge {branch} in {}", folder(&t.path)),
        ),
    };
    let label = format!("Merge into {base}…");
    if button(ui, &label, ButtonKind::Primary, enabled)
        .on_hover_text(&hint)
        .on_disabled_hover_text(&hint)
        .clicked()
        && let (Some(branch), Some(t)) = (&wt.branch, target)
    {
        let mut body = format!(
            "Runs git merge {branch} in the worktree {} (where {base} is checked out).",
            folder(&t.path)
        );
        if wt.dirty() > 0 {
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

fn file_list(
    ui: &mut Ui,
    project: &Project,
    review: &Review,
    uncommitted: &[&str],
    actions: &mut Vec<Action>,
) {
    let selected = project.review.as_ref().and_then(|v| v.selected.as_deref());
    ui.label(
        RichText::new(format!("FILES  {}", review.files.len()))
            .font(theme::semibold(theme::SMALL_SIZE))
            .color(color::TEXT_FAINT),
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
                let (rect, response) =
                    ui.allocate_exact_size(vec2(ui.available_width(), 26.0), Sense::click());
                let painter = ui.painter();
                if is_selected {
                    painter.rect_filled(rect, 4, color::SELECTED_ROW_BG);
                } else if response.hovered() {
                    painter.rect_filled(rect, 4, color::HOVER_ROW_BG);
                }
                let y = rect.center().y;
                let check = Rect::from_center_size(pos2(rect.left() + 12.0, y), vec2(14.0, 14.0));
                let check_response = ui.interact(
                    check.expand(4.0),
                    ui.scope_id().with(("viewed", path)),
                    Sense::click(),
                );
                paint_check(ui, check, viewed, check_response.hovered());
                paint_change_letter(ui, rect.left() + 26.0, y, file.change.change);

                let note = if file.untracked {
                    Some("untracked")
                } else if uncommitted.contains(&path.as_str()) {
                    Some("uncommitted")
                } else {
                    None
                };
                let stats = match (note, file.change.added, file.change.removed) {
                    (Some(n), _, _) => n.to_owned(),
                    (None, Some(a), Some(r)) => format!("+{a} −{r}"),
                    _ => "binary".to_owned(),
                };
                let stats_galley = ui.painter().layout_no_wrap(
                    stats,
                    theme::small(),
                    if note.is_some() {
                        color::DIRTY_FG
                    } else {
                        color::TEXT_FAINT
                    },
                );
                let stats_x = rect.right() - 6.0 - stats_galley.size().x;
                let path_x = rect.left() + 48.0;
                paint_path(ui, path_x, y, path, stats_x - path_x - 8.0);
                ui.painter().galley(
                    pos2(stats_x, y - stats_galley.size().y / 2.0),
                    stats_galley,
                    color::TEXT_FAINT,
                );
                if viewed {
                    // Dim viewed files so what is left stands out.
                    ui.painter().rect_filled(
                        Rect::from_min_max(pos2(rect.left() + 24.0, rect.top()), rect.max),
                        4,
                        color::APP_BG.gamma_multiply(0.55),
                    );
                }

                if check_response.clicked() {
                    actions.push(Action::ToggleViewed(path.clone()));
                } else if response.clicked() {
                    actions.push(Action::SelectReviewFile(path.clone()));
                }
                let tip = match &file.change.orig_path {
                    Some(orig) => format!("{orig} → {path}"),
                    None => path.clone(),
                };
                response.on_hover_text(tip);
            }
        });
}

fn paint_check(ui: &Ui, rect: Rect, checked: bool, hovered: bool) {
    let painter = ui.painter();
    if checked {
        painter.rect_filled(rect, 3, color::ACCENT);
        let y = rect.center().y;
        painter.line(
            vec![
                pos2(rect.left() + 3.0, y),
                pos2(rect.left() + 6.0, y + 3.0),
                pos2(rect.right() - 3.0, y - 3.0),
            ],
            Stroke::new(1.6, color::ON_ACCENT),
        );
    } else {
        let stroke = if hovered {
            color::TEXT_WEAK
        } else {
            color::CONTROL_BORDER
        };
        painter.rect_stroke(rect, 3, Stroke::new(1.0, stroke), StrokeKind::Inside);
    }
}

fn file_header(
    ui: &mut Ui,
    project: &Project,
    wt: &Worktree,
    file: &ReviewFile,
    actions: &mut Vec<Action>,
) {
    let viewed = project.is_viewed(file);
    let split = project.review.as_ref().is_none_or(|v| v.split);
    let path = file.change.path.clone();
    Panel::top("review-file-header")
        .frame(
            Frame::new()
                .fill(color::BAR_BG)
                .inner_margin(Margin::symmetric(12, 6)),
        )
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                let (rect, _) = ui.allocate_exact_size(vec2(18.0, 30.0), Sense::hover());
                paint_change_letter(ui, rect.left(), rect.center().y, file.change.change);
                ui.label(
                    RichText::new(&path)
                        .font(theme::semibold(theme::BODY_SIZE))
                        .color(color::TEXT),
                );
                if let Some(orig) = &file.change.orig_path {
                    ui.label(
                        RichText::new(format!("from {orig}"))
                            .color(color::TEXT_FAINT)
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
                        .on_hover_text("V · marks it and opens the next file")
                        .clicked()
                    {
                        actions.push(Action::ToggleViewed(path.clone()));
                    }
                    if file.change.change != Change::Deleted
                        && button(ui, "Open in IDE", ButtonKind::Secondary, true).clicked()
                    {
                        actions.push(Action::OpenInIde(wt.path.join(&path)));
                    }
                    let both_sides = !matches!(file.change.change, Change::Added | Change::Deleted);
                    let toggle = if split { "Unified" } else { "Side by side" };
                    if both_sides && button(ui, toggle, ButtonKind::Secondary, true).clicked() {
                        actions.push(Action::ToggleReviewSplit);
                    }
                });
            });
        });
    // V marks the open file as viewed (unless typing in the notes).
    if !ui.ctx().egui_wants_keyboard_input() && ui.input(|i| i.key_pressed(Key::V)) {
        actions.push(Action::ToggleViewed(path));
    }
}

/// A local note for the open file (never committed or pushed).
fn notes(ui: &mut Ui, project: &mut Project, path: &str) {
    let has_note = project
        .review_state()
        .and_then(|s| s.notes.get(path))
        .is_some_and(|n| !n.trim().is_empty());
    let id = ui.make_persistent_id(("review-note-open", path));
    let mut open = ui.data(|d| d.get_temp::<bool>(id)).unwrap_or(has_note);
    Panel::bottom("review-notes")
        .frame(
            Frame::new()
                .fill(color::SIDEBAR_BG)
                .inner_margin(Margin::symmetric(12, 6)),
        )
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                let title = match (open, has_note) {
                    (true, _) => "Hide note",
                    (false, true) => "Show note",
                    (false, false) => "+ Add a note",
                };
                if ui
                    .add(
                        egui::Label::new(
                            RichText::new(title)
                                .color(color::TEXT_MID)
                                .size(theme::SMALL_SIZE),
                        )
                        .sense(Sense::click()),
                    )
                    .clicked()
                {
                    open = !open;
                }
                ui.label(
                    RichText::new("Only on this computer; never committed.")
                        .color(color::TEXT_FAINT)
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

/// The last part of a path, e.g. "demo-repo".
fn folder(path: &std::path::Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}
