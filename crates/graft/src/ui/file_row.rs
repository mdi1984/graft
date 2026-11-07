//! A changed file in a list: its change letter, the name with the folder
//! behind it, and line counts or a note at the right. File status, Review,
//! commit details and stashes list files alike.

use egui::{Align2, Color32, Rect, Response, Sense, Ui, pos2, vec2};
use graft_core::details::FileChange;
use graft_core::status::Change;

use super::widgets::{elided, paint_galley_left, paint_row_bg};
use crate::project::Action;
use crate::theme::{self, color};

pub const FILE_ROW_HEIGHT: f32 = 26.0;
/// From the change letter to the path.
const PATH_GAP: f32 = 24.0;

pub fn change_color(change: Change) -> Color32 {
    match change {
        Change::Added => color::added(),
        Change::Deleted | Change::Unmerged => color::removed(),
        Change::Renamed | Change::Copied => color::renamed(),
        Change::TypeChanged => color::text_weak(),
        Change::Modified | Change::None => color::dirty_fg(),
    }
}

/// The change as a colored letter in a box, its left edge at `x`.
pub fn paint_change_letter(ui: &Ui, x: f32, y: f32, change: Change) {
    let c = change_color(change);
    let rect = Rect::from_min_size(pos2(x, y - 8.0), vec2(16.0, 16.0));
    ui.painter().rect_filled(rect, 3, c.gamma_multiply(0.16));
    ui.painter().text(
        rect.center(),
        Align2::CENTER_CENTER,
        change.letter(),
        theme::semibold(theme::SMALL_SIZE),
        c,
    );
}

/// A file path as "name" in front and the folder faint behind it.
pub fn paint_path(ui: &Ui, x: f32, y: f32, path: &str, max_width: f32) {
    let (dir, name) = match path.rsplit_once('/') {
        Some((dir, name)) => (dir, name),
        None => ("", path),
    };
    let name = elided(ui, name, theme::body(), color::text(), max_width);
    let name_width = name.size().x;
    paint_galley_left(ui.painter(), x, y, name);
    let room = max_width - name_width - 8.0;
    if !dir.is_empty() && room > 30.0 {
        let dir = elided(ui, dir, theme::small(), color::text_faint(), room);
        paint_galley_left(ui.painter(), x + name_width + 8.0, y, dir);
    }
}

/// The change letter at `x` and the path after it, up to `right`.
pub fn paint_file(ui: &Ui, x: f32, y: f32, change: Change, path: &str, right: f32) {
    paint_change_letter(ui, x, y, change);
    let path_x = x + PATH_GAP;
    paint_path(ui, path_x, y, path, right - path_x);
}

/// A full-width row for one file, with the hover or selected background.
pub fn allocate(ui: &mut Ui, selected: bool, sense: Sense) -> (Rect, Response) {
    let (rect, response) =
        ui.allocate_exact_size(vec2(ui.available_width(), FILE_ROW_HEIGHT), sense);
    paint_row_bg(ui.painter(), rect, &response, selected);
    (rect, response)
}

/// The hover text of a file row: the path, and where a rename came from.
pub fn tip(path: &str, orig: Option<&str>) -> String {
    match orig {
        Some(orig) => format!("{orig} → {path}"),
        None => path.to_owned(),
    }
}

/// "+3 −1", or "binary" for a file without line counts.
pub fn stats(added: Option<usize>, removed: Option<usize>) -> String {
    match (added, removed) {
        (Some(a), Some(r)) => format!("+{a} −{r}"),
        _ => "binary".to_owned(),
    }
}

/// A changed file of a commit or stash; `note` replaces the line counts.
pub fn file_row(ui: &mut Ui, file: &FileChange, selected: bool, note: Option<&str>) -> Response {
    let (rect, response) = allocate(ui, selected, Sense::click());
    let y = rect.center().y;
    let stats = match note {
        Some(note) => note.to_owned(),
        None => stats(file.added, file.removed),
    };
    let stats = ui
        .painter()
        .layout_no_wrap(stats, theme::small(), color::text_faint());
    let stats_x = rect.right() - 4.0 - stats.size().x;
    paint_galley_left(ui.painter(), stats_x, y, stats);
    paint_file(
        ui,
        rect.left() + 4.0,
        y,
        file.change,
        &file.path,
        stats_x - 8.0,
    );
    response.on_hover_text(tip(&file.path, file.orig_path.as_deref()))
}

/// The context menu entries every file has.
pub fn menu_items(ui: &mut Ui, path: &str, actions: &mut Vec<Action>) {
    if ui.button("File history").clicked() {
        actions.push(Action::OpenFileHistory(path.to_owned()));
        ui.close();
    }
    if ui.button("Copy path").clicked() {
        ui.ctx().copy_text(path.to_owned());
        ui.close();
    }
}
