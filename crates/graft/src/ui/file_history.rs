//! File history: the commits that changed one file on the left; on the
//! right the selected commit's diff of it, or the file with each line's last
//! change (blame).

use egui::{
    Align, Frame, Key, Layout, Margin, Modifiers, Panel, Rect, RichText, ScrollArea, Sense, Ui,
    pos2, vec2,
};
use graft_core::file_log::Blame;

use super::diff_view;
use super::widgets::{
    ButtonKind, button, elided, empty_state, paint_galley_left, paint_row_bg, spinner,
};
use crate::project::{Action, FileHistoryMode, FileHistoryView, Project};
use crate::theme::{self, color};

const ROW: f32 = 46.0;
const LINE: f32 = 18.0;
const GUTTER: f32 = 250.0;

pub fn show(ui: &mut Ui, project: &Project, actions: &mut Vec<Action>) {
    let Some(view) = &project.file_history else {
        return;
    };
    toolbar(ui, view, actions);
    let commits = match &view.commits {
        None => {
            let rect = ui.available_rect_before_wrap();
            spinner(ui, rect.center(), 9.0, color::text_weak());
            return;
        }
        Some(Err(e)) => {
            empty_state(ui, "Could not read the file's history", e);
            return;
        }
        Some(Ok(commits)) if commits.is_empty() => {
            empty_state(
                ui,
                "No history yet",
                "No commit contains this file. Commit it first.",
            );
            return;
        }
        Some(Ok(commits)) => commits,
    };

    Panel::left("file-history-commits")
        .resizable(true)
        .default_size(360.0)
        .size_range(240.0..=640.0)
        .frame(Frame::new().fill(color::sidebar_bg()))
        .show(ui, |ui| commit_list(ui, view, commits, actions));

    let busy = project.is_operating();
    let wrap = project.settings.wrap_lines;
    match view.mode {
        FileHistoryMode::Changes => match &project.commit_diff {
            Some(diff) => diff_view::body(ui, diff, busy, false, wrap, None, actions),
            None => {
                let rect = ui.available_rect_before_wrap();
                spinner(ui, rect.center(), 9.0, color::text_weak());
            }
        },
        FileHistoryMode::Blame => match &view.blame {
            None => {
                let rect = ui.available_rect_before_wrap();
                spinner(ui, rect.center(), 9.0, color::text_weak());
            }
            Some(Err(e)) => empty_state(ui, "Could not annotate the file", e),
            Some(Ok(blame)) if blame.binary => {
                empty_state(ui, "Binary file", "Blame works on text files only.");
            }
            Some(Ok(blame)) => blame_view(ui, view, commits, blame, actions),
        },
    }
}

fn toolbar(ui: &mut Ui, view: &FileHistoryView, actions: &mut Vec<Action>) {
    Panel::top("file-history-bar")
        .frame(
            Frame::new()
                .fill(color::app_bg())
                .inner_margin(Margin::symmetric(14, 8)),
        )
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                if button(ui, "← Back", ButtonKind::Secondary, true).clicked() {
                    actions.push(Action::CloseFileHistory);
                }
                ui.add_space(6.0);
                ui.label(
                    RichText::new(&view.path)
                        .font(theme::mono())
                        .color(color::text()),
                );
                if let Some(Ok(commits)) = &view.commits {
                    let n = commits.len();
                    ui.label(
                        RichText::new(if n == 1 {
                            "1 commit".to_owned()
                        } else {
                            format!("{n} commits")
                        })
                        .color(color::text_faint()),
                    );
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    for (mode, label) in [
                        (FileHistoryMode::Blame, "Blame"),
                        (FileHistoryMode::Changes, "Changes"),
                    ] {
                        if ui.selectable_label(view.mode == mode, label).clicked() {
                            actions.push(Action::SetFileHistoryMode(mode));
                        }
                    }
                });
            });
        });
}

fn commit_list(
    ui: &mut Ui,
    view: &FileHistoryView,
    commits: &[graft_core::file_log::FileCommit],
    actions: &mut Vec<Action>,
) {
    // Arrow keys move through the list unless something else has the keyboard.
    if !ui.ctx().egui_wants_keyboard_input()
        && let Some(selected) = view.selected
    {
        let (up, down) = ui.input_mut(|i| {
            (
                i.consume_key(Modifiers::NONE, Key::ArrowUp),
                i.consume_key(Modifiers::NONE, Key::ArrowDown),
            )
        });
        if up && selected > 0 {
            actions.push(Action::SelectFileCommit(selected - 1));
        } else if down && selected + 1 < commits.len() {
            actions.push(Action::SelectFileCommit(selected + 1));
        }
    }
    // Scroll to the selection when it changed (e.g. by the arrow keys).
    let seen_id = ui.scope_id().with("seen-selection");
    let scroll = ui.data(|d| d.get_temp::<Option<usize>>(seen_id)) != Some(view.selected);
    ui.data_mut(|d| d.insert_temp(seen_id, view.selected));
    let now = crate::time::now();
    ScrollArea::vertical()
        .auto_shrink([false, false])
        .show_rows(ui, ROW, commits.len(), |ui, range| {
            for i in range {
                let commit = &commits[i];
                let (rect, response) =
                    ui.allocate_exact_size(vec2(ui.available_width(), ROW), Sense::click());
                let selected = view.selected == Some(i);
                paint_row_bg(
                    ui.painter(),
                    rect.shrink2(vec2(6.0, 2.0)),
                    &response,
                    selected,
                );
                let x = rect.left() + 14.0;
                let width = rect.width() - 28.0;
                let summary = elided(ui, &commit.summary, theme::body(), color::text(), width);
                paint_galley_left(ui.painter(), x, rect.top() + 15.0, summary, color::text());
                let mut meta = format!(
                    "{} · {} · {}",
                    commit.author,
                    crate::time::relative(commit.time, now),
                    graft_core::short(&commit.id)
                );
                if commit.path != view.path {
                    meta.push_str(&format!(" · as {}", commit.path));
                }
                let meta = elided(ui, &meta, theme::small(), color::text_weak(), width);
                paint_galley_left(ui.painter(), x, rect.top() + 33.0, meta, color::text_weak());
                if response.clicked() {
                    actions.push(Action::SelectFileCommit(i));
                }
                if selected && scroll {
                    response.scroll_to_me(None);
                }
            }
        });
}

fn blame_view(
    ui: &mut Ui,
    view: &FileHistoryView,
    commits: &[graft_core::file_log::FileCommit],
    blame: &Blame,
    actions: &mut Vec<Action>,
) {
    let now = crate::time::now();
    let digits = blame.lines.len().max(1).to_string().len() as f32;
    let number_width = digits * 8.0 + 12.0;
    let selected_id = view.selected.and_then(|i| commits.get(i)).map(|c| c.id);
    let char_width = ui
        .painter()
        .layout_no_wrap("0".to_owned(), theme::mono(), color::text())
        .size()
        .x;
    let longest = blame
        .lines
        .iter()
        .map(|l| l.text.chars().count() + l.text.matches('\t').count() * 3)
        .max()
        .unwrap_or(0);
    let content_width = GUTTER + number_width + 24.0 + longest as f32 * char_width;
    ScrollArea::both().auto_shrink([false, false]).show_rows(
        ui,
        LINE,
        blame.lines.len(),
        |ui, range| {
            for i in range {
                let line = &blame.lines[i];
                let first = i == 0 || blame.lines[i - 1].commit != line.commit;
                let width = ui.available_width().max(content_width);
                let (rect, _) = ui.allocate_exact_size(vec2(width, LINE), Sense::hover());
                let gutter = Rect::from_min_size(rect.min, vec2(GUTTER, LINE));
                let painter = ui.painter();
                let this_commit = Some(line.commit) == selected_id;
                painter.rect_filled(
                    gutter,
                    0,
                    if this_commit {
                        color::selected_row_bg()
                    } else {
                        color::sidebar_bg()
                    },
                );
                if first && i > 0 {
                    painter.hline(rect.x_range(), rect.top(), (1.0, color::border()));
                }
                let info = blame.commits.get(&line.commit);
                if first && let Some(info) = info {
                    let text = format!(
                        "{}  {}  {}",
                        graft_core::short(&line.commit),
                        info.author,
                        crate::time::relative(info.time, now)
                    );
                    let galley =
                        elided(ui, &text, theme::small(), color::text_weak(), GUTTER - 16.0);
                    paint_galley_left(
                        painter,
                        gutter.left() + 8.0,
                        rect.center().y,
                        galley,
                        color::text_weak(),
                    );
                }
                let gutter_response = ui.interact(
                    gutter,
                    ui.scope_id().with(("blame-gutter", i)),
                    Sense::click(),
                );
                if let Some(info) = info {
                    let gutter_response = gutter_response.on_hover_text(format!(
                        "{}\n{} · {}",
                        info.summary,
                        info.author,
                        graft_core::short(&line.commit)
                    ));
                    if gutter_response.clicked() {
                        match commits.iter().position(|c| c.id == line.commit) {
                            Some(index) => actions.push(Action::SelectFileCommit(index)),
                            None => actions.push(Action::Reveal(line.commit)),
                        }
                    }
                }
                let number = ui.painter().layout_no_wrap(
                    (i + 1).to_string(),
                    theme::mono(),
                    color::text_faint(),
                );
                let nx = gutter.right() + number_width - 6.0 - number.size().x;
                ui.painter().galley(
                    pos2(nx, rect.center().y - number.size().y / 2.0),
                    number,
                    color::text_faint(),
                );
                let text = ui.painter().layout_no_wrap(
                    line.text.replace('\t', "    "),
                    theme::mono(),
                    color::text(),
                );
                ui.painter().galley(
                    pos2(
                        gutter.right() + number_width + 6.0,
                        rect.center().y - text.size().y / 2.0,
                    ),
                    text,
                    color::text(),
                );
            }
        },
    );
}
