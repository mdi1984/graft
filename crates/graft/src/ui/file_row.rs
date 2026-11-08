//! A changed file in a list: its change letter, the name with the folder
//! behind it, and line counts or a note at the right. File status, Review,
//! commit details and stashes list files alike.

use egui::{Align2, Color32, Rect, Response, Sense, Ui, pos2, vec2};
use graft_core::Change;
use graft_core::changes::FileChange;

use super::widgets::{Badge, ahead, elided, paint_galley_left, paint_row_bg};
use crate::project::Action;
use crate::theme::{self, color};
use crate::ui::Actions;

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

/// What a file row shows besides the file.
#[derive(Default)]
pub struct Extras<'a> {
    /// Room left free before the change letter (Review's Viewed box).
    pub inset: f32,
    /// Shown in its color instead of the line counts (e.g. "untracked").
    pub note: Option<(&'a str, Color32)>,
    /// A count left of the line counts (Review's comments); none at 0.
    pub badge: usize,
    /// The file's name without its folder (a tree shows the folder above).
    pub name_only: bool,
}

/// A changed file of a commit, stash or review.
pub fn file_row(ui: &mut Ui, file: &FileChange, selected: bool, extras: Extras) -> Response {
    let (rect, response) = allocate(ui, selected, Sense::click());
    let y = rect.center().y;
    let (stats, stats_color) = match extras.note {
        Some((note, color)) => (note.to_owned(), color),
        None => (stats(file.added, file.removed), color::text_faint()),
    };
    let stats = ui
        .painter()
        .layout_no_wrap(stats, theme::small(), stats_color);
    let stats_x = rect.right() - 4.0 - stats.size().x;
    paint_galley_left(ui.painter(), stats_x, y, stats);
    let mut path_right = stats_x - 8.0;
    if extras.badge > 0 {
        let badge = Badge::new(ui, extras.badge.to_string(), ahead());
        path_right -= badge.width() + 6.0;
        badge.paint(ui.painter(), path_right + 6.0, y);
    }
    let shown = if extras.name_only {
        file.path.rsplit('/').next().unwrap_or(&file.path)
    } else {
        &file.path
    };
    paint_file(
        ui,
        rect.left() + 4.0 + extras.inset,
        y,
        file.change,
        shown,
        path_right,
    );
    response.on_hover_text(tip(&file.path, file.orig_path.as_deref()))
}

/// The context menu entries every file has.
pub fn menu_items(ui: &mut Ui, path: &str, actions: &mut Actions) {
    if ui.button("File history").clicked() {
        actions.push(Action::OpenFileHistory(path.to_owned()));
        ui.close();
    }
    if ui.button("Copy path").clicked() {
        ui.ctx().copy_text(path.to_owned());
        ui.close();
    }
}
