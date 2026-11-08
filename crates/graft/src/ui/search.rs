//! The toolbar search: type to find worktrees, branches, tags,
//! commits and changed files in every open project; Enter or a click jumps
//! there (switching to that project's tab).

use egui::{
    Align, Area, Frame, Id, Key, Layout, Margin, Modifiers, Order, Rect, RichText, ScrollArea,
    Sense, Stroke, TextEdit, Ui, vec2,
};

use std::path::PathBuf;
use std::sync::Arc;

use graft_core::history::History;
use graft_core::{RepoSnapshot, WorktreeStatus};

use super::icons::{self, Icon};
use crate::keys::Command;
use crate::project::{Action, FileKey, Project};
use crate::theme::{self, RADIUS, color};
use crate::ui::{Actions, AppAction};

/// A search result, and what choosing it does.
struct Hit {
    kind: Kind,
    title: String,
    detail: String,
    /// The other project's tab index and name, for a hit outside the
    /// active project.
    project: Option<(usize, String)>,
    /// What choosing it does, in its project.
    actions: Vec<Action>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Worktree,
    Branch,
    Remote,
    Tag,
    Changed,
    Commit,
}

impl Kind {
    fn label(self) -> &'static str {
        match self {
            Kind::Worktree => "Worktree",
            Kind::Branch => "Branch",
            Kind::Remote => "Remote",
            Kind::Tag => "Tag",
            Kind::Changed => "Changed",
            Kind::Commit => "Commit",
        }
    }

    /// Ref names show in the code font; commit summaries and file paths
    /// read better in the text font.
    fn font(self) -> egui::FontId {
        match self {
            Kind::Commit | Kind::Changed => theme::body(),
            _ => theme::mono(),
        }
    }
}

const LIMIT: usize = 8;
const HEADER_HEIGHT: f32 = 24.0;
const ROW_HEIGHT: f32 = 30.0;
/// The popup scrolls beyond this.
const MAX_POPUP_HEIGHT: f32 = 420.0;

/// What a list of hits was computed from: while it stays the same, the
/// popup reuses the hits instead of searching every project each frame.
#[derive(Clone, PartialEq)]
struct SearchKey {
    text: String,
    active: usize,
    projects: Vec<(u64, PathBuf)>,
}

fn cached_hits(ui: &Ui, id: Id, projects: &[Project], active: usize, text: &str) -> Arc<Vec<Hit>> {
    let key = SearchKey {
        text: text.to_owned(),
        active,
        projects: projects
            .iter()
            .map(|p| {
                let (generation, worktree) = p.search_stamp();
                (generation, worktree.to_path_buf())
            })
            .collect(),
    };
    let cached: Option<(SearchKey, Arc<Vec<Hit>>)> = ui.data(|d| d.get_temp(id));
    if let Some((old, results)) = cached
        && old == key
    {
        return results;
    }
    let results = Arc::new(hits(projects, active, text));
    ui.data_mut(|d| d.insert_temp(id, (key, results.clone())));
    results
}

/// Hits in every open project, grouped as [`grouped`] says.
fn hits(projects: &[Project], active: usize, text: &str) -> Vec<Hit> {
    let found = projects.iter().map(|project| {
        let hits = match project.snapshot.as_deref() {
            Some(snap) => project_hits(
                snap,
                project.worktree().and_then(|wt| wt.status.as_ref()),
                project.history.as_ref(),
                text,
            ),
            None => Vec::new(),
        };
        (project.name.clone(), hits)
    });
    grouped(found.collect(), active)
}

/// Each project's hits (by tab, with its name) in one list: the active
/// project's first, then one group per other project in tab order. Hits
/// elsewhere are marked with their project, which choosing them switches to.
fn grouped(per_project: Vec<(String, Vec<Hit>)>, active: usize) -> Vec<Hit> {
    let mut per_project: Vec<Option<(String, Vec<Hit>)>> =
        per_project.into_iter().map(Some).collect();
    let order = std::iter::once(active).chain((0..per_project.len()).filter(|&i| i != active));
    order
        .filter_map(|i| Some((i, per_project.get_mut(i)?.take()?)))
        .flat_map(|(i, (name, hits))| {
            hits.into_iter().map(move |mut hit| {
                if i != active {
                    hit.project = Some((i, name.clone()));
                }
                hit
            })
        })
        .collect()
}

/// A line of the result popup, top to bottom.
#[derive(Debug, PartialEq)]
enum Line<'a> {
    /// A project's name above its hits; `first` gets no line above it.
    Header {
        name: &'a str,
        first: bool,
    },
    /// Under the active project's header when only other projects matched.
    NoMatches,
    Hit(usize),
}

impl Line<'_> {
    fn height(&self) -> f32 {
        match self {
            Line::Header { .. } | Line::NoMatches => HEADER_HEIGHT,
            Line::Hit(_) => ROW_HEIGHT,
        }
    }
}

/// The popup's lines for `results`; the active project is `active_name`.
/// A header goes where a new project's hits start; the active project gets
/// one only when other projects have hits too.
fn popup_lines<'a>(results: &'a [Hit], active_name: &'a str) -> Vec<Line<'a>> {
    let elsewhere = results.iter().any(|hit| hit.project.is_some());
    // The active project found nothing, but others did: say so on top.
    let none_here = results.first().is_some_and(|hit| hit.project.is_some());
    let mut lines = Vec::new();
    if none_here {
        lines.push(Line::Header {
            name: active_name,
            first: true,
        });
        lines.push(Line::NoMatches);
    }
    for i in 0..results.len() {
        if let Some(name) = header(results, i, elsewhere, active_name) {
            lines.push(Line::Header {
                name,
                first: i == 0 && !none_here,
            });
        }
        lines.push(Line::Hit(i));
    }
    lines
}

/// The group header to draw above `results[i]`: the project name where a
/// new project's hits start. The active project gets one only when other
/// projects have hits too (`elsewhere`).
fn header<'a>(
    results: &'a [Hit],
    i: usize,
    elsewhere: bool,
    active_name: &'a str,
) -> Option<&'a str> {
    let tab = |hit: &Hit| hit.project.as_ref().map(|(tab, _)| *tab);
    let starts = i == 0 || tab(&results[i - 1]) != tab(&results[i]);
    match &results[i].project {
        _ if !starts => None,
        None => elsewhere.then_some(active_name),
        Some((_, name)) => Some(name),
    }
}

/// Every word of `query` occurs in one of `fields`.
fn matches(query: &[String], fields: &[&str]) -> bool {
    query.iter().all(|word| {
        fields
            .iter()
            .any(|f| f.to_lowercase().contains(word.as_str()))
    })
}

/// The hits in one project: its refs and worktrees, the changed files of
/// the selected worktree (`status`), and the loaded commits.
fn project_hits(
    snap: &RepoSnapshot,
    status: Option<&WorktreeStatus>,
    history: Option<&History>,
    text: &str,
) -> Vec<Hit> {
    let query: Vec<String> = text.split_whitespace().map(str::to_lowercase).collect();
    let mut out = Vec::new();
    if query.is_empty() {
        return out;
    }

    for wt in snap
        .worktrees
        .iter()
        .filter(|wt| !wt.prunable)
        .filter(|wt| matches(&query, &[&wt.label(), &wt.path.to_string_lossy()]))
        .take(LIMIT)
    {
        out.push(Hit {
            kind: Kind::Worktree,
            title: wt.label(),
            detail: graft_core::paths::display(&wt.path),
            project: None,
            actions: vec![Action::OpenWorktree(wt.path.clone())],
        });
    }
    for b in snap
        .branches
        .iter()
        .filter(|b| matches(&query, &[&b.name]))
        .take(LIMIT)
    {
        out.push(Hit {
            kind: Kind::Branch,
            title: b.name.clone(),
            detail: graft_core::short(&b.target),
            project: None,
            actions: vec![Action::Reveal(b.target)],
        });
    }
    for b in snap
        .remote_branches
        .iter()
        .filter(|b| matches(&query, &[&b.name]))
        .take(LIMIT)
    {
        out.push(Hit {
            kind: Kind::Remote,
            title: b.name.clone(),
            detail: graft_core::short(&b.target),
            project: None,
            actions: vec![Action::Reveal(b.target)],
        });
    }
    for t in snap
        .tags
        .iter()
        .filter(|t| matches(&query, &[&t.name]))
        .take(LIMIT)
    {
        out.push(Hit {
            kind: Kind::Tag,
            title: t.name.clone(),
            detail: graft_core::short(&t.target),
            project: None,
            actions: vec![Action::Reveal(t.target)],
        });
    }

    if let Some(status) = status {
        for entry in status
            .entries
            .iter()
            .filter(|e| matches(&query, &[&e.path]))
            .take(LIMIT)
        {
            let staged = entry.is_staged() && !entry.is_unstaged();
            out.push(Hit {
                kind: Kind::Changed,
                title: entry.path.clone(),
                detail: if staged { "staged" } else { "uncommitted" }.to_owned(),
                project: None,
                actions: vec![
                    Action::ShowFileStatus,
                    Action::SelectFile(FileKey::of(entry, staged)),
                ],
            });
        }
    }

    if let Some(history) = history {
        let hex = text.trim().to_lowercase();
        let by_id = hex.len() >= 4 && hex.chars().all(|c| c.is_ascii_hexdigit());
        for c in history
            .commits
            .iter()
            .filter(|c| !c.is_worktree())
            .filter(|c| {
                (by_id && c.id.to_string().starts_with(&hex))
                    || matches(&query, &[&c.summary, &c.author])
            })
            .take(LIMIT * 2)
        {
            out.push(Hit {
                kind: Kind::Commit,
                title: c.summary.clone(),
                detail: format!("{} · {}", graft_core::short(&c.id), c.author),
                project: None,
                actions: vec![Action::Reveal(c.id)],
            });
        }
    }
    out
}

/// The search box's width: what the toolbar leaves, within these.
const MIN_WIDTH: f32 = 160.0;
pub const MAX_WIDTH: f32 = 300.0;

pub fn show(
    ui: &mut Ui,
    projects: &[Project],
    active: usize,
    text: &mut String,
    actions: &mut Actions,
) {
    let id = Id::unique("toolbar-search");
    let selected_id = id.with("selected");
    let popup_id = id.with("popup-rect");
    let keys = &projects[active].settings().keys;
    if keys.pressed(ui.ctx(), Command::Search) {
        ui.memory_mut(|m| m.request_focus(id));
        // Select the old query so typing replaces it.
        let mut state = egui::text_edit::TextEditState::load(ui.ctx(), id).unwrap_or_default();
        let all = egui::text::CCursorRange::two(
            egui::text::CCursor::new(0),
            egui::text::CCursor::new(text.chars().count()),
        );
        state.cursor.set_char_range(Some(all));
        state.store(ui.ctx(), id);
    }
    let focused = ui.memory(|m| m.has_focus(id));
    // egui drops the focus as soon as Escape is pressed, so remember it.
    let focused_id = id.with("was-focused");
    let was_focused: bool = ui.data(|d| d.get_temp(focused_id)).unwrap_or(false);
    ui.data_mut(|d| d.insert_temp(focused_id, focused));
    if was_focused && !focused && ui.input(|i| i.key_pressed(Key::Escape)) {
        text.clear();
    }
    let border = if focused {
        color::accent()
    } else {
        color::control_border()
    };
    let width = ui.available_width().clamp(MIN_WIDTH, MAX_WIDTH);

    let before = text.clone();
    let frame = Frame::new()
        .fill(color::app_bg())
        .stroke(Stroke::new(1.0, border))
        .corner_radius(RADIUS)
        .inner_margin(Margin::symmetric(8, 0))
        .show(ui, |ui| {
            ui.set_width(width - 16.0);
            ui.set_height(28.0);
            ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                let (rect, _) = ui.allocate_exact_size(vec2(14.0, 14.0), Sense::hover());
                icons::paint(ui.painter(), rect, Icon::Search, color::text_weak());
                ui.add(
                    TextEdit::singleline(text)
                        .id(id)
                        .frame(Frame::NONE)
                        .margin(Margin::ZERO)
                        .min_size(vec2(0.0, 28.0))
                        .align(egui::Align2::LEFT_CENTER)
                        .hint_text(keys.tip("Commits, branches, files…", Command::Search))
                        .desired_width(f32::INFINITY),
                );
            });
        });
    if *text != before {
        ui.data_mut(|d| d.insert_temp(selected_id, 0usize));
        // The result list sizes itself from the previous frame: draw again.
        ui.ctx().request_repaint();
    }

    // Keep the list open while the pointer is on it, so a click lands.
    let last_popup: Option<Rect> = ui.data(|d| d.get_temp(popup_id));
    let over_popup = last_popup
        .zip(ui.ctx().pointer_hover_pos())
        .is_some_and(|(r, p)| r.contains(p));
    if text.trim().is_empty() || !(focused || over_popup) {
        ui.data_mut(|d| d.remove::<Rect>(popup_id));
        return;
    }

    let results = cached_hits(ui, id.with("hits"), projects, active, text);
    let mut selected: usize = ui.data(|d| d.get_temp(selected_id)).unwrap_or(0);
    // Scroll to the selection only when it moved, so the wheel still scrolls.
    let mut scroll = *text != before;
    if focused {
        ui.input_mut(|i| {
            if i.consume_key(Modifiers::NONE, Key::ArrowDown) {
                selected += 1;
                scroll = true;
            }
            if i.consume_key(Modifiers::NONE, Key::ArrowUp) {
                selected = selected.saturating_sub(1);
                scroll = true;
            }
        });
    }
    selected = selected.min(results.len().saturating_sub(1));
    let enter = focused && ui.input(|i| i.key_pressed(Key::Enter));
    let mut chosen = enter.then_some(selected);

    let anchor = frame.response.rect;
    let popup_width = 460.0f32.max(anchor.width());
    let active_name = format!("{} · current", projects[active].name);
    let lines = popup_lines(&results, &active_name);
    let area = Area::new(id.with("popup"))
        .order(Order::Foreground)
        .fixed_pos(egui::pos2(
            anchor.right() - popup_width,
            anchor.bottom() + 4.0,
        ))
        .show(ui.ctx(), |ui| {
            Frame::new()
                .fill(color::sidebar_bg())
                .stroke(Stroke::new(1.0, color::border()))
                .corner_radius(RADIUS)
                .inner_margin(Margin::same(6))
                .show(ui, |ui| {
                    ui.set_width(popup_width - 12.0);
                    if results.is_empty() {
                        ui.label(RichText::new("No matches").color(color::text_faint()));
                        return;
                    }
                    // The popup keeps last frame's size; ask for this list's height.
                    let spacing = ui.spacing().item_spacing.y;
                    let height: f32 = lines.iter().map(|line| line.height() + spacing).sum();
                    ScrollArea::vertical()
                        .max_height(MAX_POPUP_HEIGHT)
                        .min_scrolled_height(height.min(MAX_POPUP_HEIGHT))
                        .show(ui, |ui| {
                            // Headers right above the selected row scroll in with it.
                            let mut above: Option<Rect> = None;
                            for line in &lines {
                                let i = match *line {
                                    Line::Header { name, first } => {
                                        let rect = group_header(ui, name, first);
                                        above = Some(above.map_or(rect, |a| a.union(rect)));
                                        continue;
                                    }
                                    Line::NoMatches => {
                                        let rect = no_matches(ui);
                                        above = Some(above.map_or(rect, |a| a.union(rect)));
                                        continue;
                                    }
                                    Line::Hit(i) => i,
                                };
                                let response = row(ui, &results[i], i == selected);
                                if i == selected && scroll {
                                    let rect =
                                        above.map_or(response.rect, |a| a.union(response.rect));
                                    ui.scroll_to_rect(rect, None);
                                }
                                above = None;
                                if response.clicked() {
                                    chosen = Some(i);
                                }
                                if response.hovered()
                                    && ui.input(|i| i.pointer.delta() != vec2(0.0, 0.0))
                                {
                                    selected = i;
                                }
                            }
                        });
                });
        });
    ui.data_mut(|d| {
        d.insert_temp(popup_id, area.response.rect);
        d.insert_temp(selected_id, selected);
    });

    if let Some(i) = chosen
        && let Some(hit) = results.get(i)
    {
        match &hit.project {
            Some((project, _)) => actions.app(AppAction::InProject {
                project: *project,
                actions: hit.actions.clone(),
            }),
            None => actions.extend(hit.actions.iter().cloned()),
        }
        text.clear();
        ui.memory_mut(|m| m.surrender_focus(id));
        ui.data_mut(|d| d.remove::<Rect>(popup_id));
    }
}

/// "No matches", where the active project's hits would be.
fn no_matches(ui: &mut Ui) -> Rect {
    let (rect, _) =
        ui.allocate_exact_size(vec2(ui.available_width(), HEADER_HEIGHT), Sense::hover());
    let none =
        ui.painter()
            .layout_no_wrap("No matches".to_owned(), theme::small(), color::text_faint());
    super::widgets::paint_galley_left(ui.painter(), rect.left() + 8.0, rect.center().y, none);
    rect
}

/// A project's name above its hits; a line separates it from the group above.
fn group_header(ui: &mut Ui, name: &str, first: bool) -> Rect {
    let (rect, _) =
        ui.allocate_exact_size(vec2(ui.available_width(), HEADER_HEIGHT), Sense::hover());
    let painter = ui.painter();
    if !first {
        painter.hline(
            rect.x_range(),
            rect.top(),
            Stroke::new(1.0, color::border()),
        );
    }
    super::widgets::section_heading(ui, painter, rect.left() + 8.0, rect.center().y + 2.0, name);
    rect
}

fn row(ui: &mut Ui, hit: &Hit, selected: bool) -> egui::Response {
    let (rect, response) =
        ui.allocate_exact_size(vec2(ui.available_width(), ROW_HEIGHT), Sense::click());
    let painter = ui.painter();
    if selected {
        painter.rect_filled(rect, RADIUS, color::selected_row_bg());
    } else if response.hovered() {
        painter.rect_filled(rect, RADIUS, color::hover_row_bg());
    }
    let y = rect.center().y;
    let kind = painter.layout_no_wrap(
        hit.kind.label().to_owned(),
        theme::small(),
        color::text_faint(),
    );
    painter.galley(
        egui::pos2(rect.left() + 8.0, y - kind.size().y / 2.0),
        kind,
        color::text_faint(),
    );
    let detail_width = 150.0;
    let title = super::widgets::elided(
        ui,
        &hit.title,
        hit.kind.font(),
        color::text(),
        rect.width() - 80.0 - detail_width - 16.0,
    );
    super::widgets::paint_galley_left(painter, rect.left() + 80.0, y, title);
    let detail = super::widgets::elided_path(
        ui,
        &hit.detail,
        theme::small(),
        color::text_weak(),
        detail_width,
    );
    let x = rect.right() - 8.0 - detail.size().x;
    super::widgets::paint_galley_left(painter, x, y, detail);
    response
}

#[cfg(test)]
mod tests {
    use graft_core::history::{Commit, History};
    use graft_core::{
        Branch, Change, EntryKind, Oid, RepoSnapshot, StatusEntry, Upstream, Worktree,
        WorktreeStatus,
    };
    use std::path::Path;

    use super::*;
    use crate::testing::{git, open};

    /// The name the popup gives the active project in these tests; no
    /// project is called that.
    const ACTIVE: &str = "<active>";

    fn oid(n: u8) -> Oid {
        Oid::from_hex(format!("{n:02x}").repeat(20).as_bytes()).unwrap()
    }

    /// A repository called `name` with its main worktree on `main`, and the
    /// extra local `branches`.
    fn snap(name: &str, branches: &[&str]) -> RepoSnapshot {
        let root = PathBuf::from(format!(r"C:\repos\{name}"));
        RepoSnapshot {
            name: name.to_owned(),
            root: root.clone(),
            worktrees: vec![Worktree {
                path: root,
                branch: Some("main".to_owned()),
                head: Some(oid(1)),
                is_main: true,
                locked: false,
                prunable: false,
                status: None,
                in_progress: None,
            }],
            branches: std::iter::once("main")
                .chain(branches.iter().copied())
                .map(|name| Branch {
                    name: name.to_owned(),
                    target: oid(1),
                    upstream: Upstream::None,
                })
                .collect(),
            ..Default::default()
        }
    }

    /// Search `text` from the tab `active` of projects made of just a
    /// snapshot each.
    fn search(snaps: &[RepoSnapshot], active: usize, text: &str) -> Vec<Hit> {
        let found = snaps
            .iter()
            .map(|s| (s.name.clone(), project_hits(s, None, None, text)))
            .collect();
        grouped(found, active)
    }

    /// The group headers the popup draws.
    fn headers(results: &[Hit]) -> Vec<&str> {
        popup_lines(results, ACTIVE)
            .into_iter()
            .filter_map(|line| match line {
                Line::Header { name, .. } => Some(name),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn groups_hits_by_project_active_first() {
        let snaps = [snap("a", &[]), snap("b", &[]), snap("c", &[])];
        // "main" matches each project's main worktree and main branch.
        // Searching from the second tab: its hits first, then tab 1, tab 3.
        let results = search(&snaps, 1, "main");
        let found: Vec<(Kind, Option<usize>)> = results
            .iter()
            .map(|h| (h.kind, h.project.as_ref().map(|(tab, _)| *tab)))
            .collect();
        assert_eq!(
            found,
            [
                (Kind::Worktree, None),
                (Kind::Branch, None),
                (Kind::Worktree, Some(0)),
                (Kind::Branch, Some(0)),
                (Kind::Worktree, Some(2)),
                (Kind::Branch, Some(2)),
            ]
        );
        assert_eq!(results[2].project, Some((0, "a".to_owned())));
        assert!(matches!(results[1].actions[..], [Action::Reveal(_)]));
        // One header per project, where its hits start.
        assert_eq!(headers(&results), [ACTIVE, "a", "c"]);
    }

    #[test]
    fn no_header_when_only_the_active_project_matches() {
        let results = search(&[snap("a", &["only-in-a"]), snap("b", &[])], 0, "only-in-a");
        assert_eq!(results.len(), 1);
        assert!(results[0].project.is_none());
        assert_eq!(popup_lines(&results, ACTIVE), [Line::Hit(0)]);
    }

    #[test]
    fn other_projects_only_when_the_active_one_has_no_match() {
        let results = search(&[snap("a", &[]), snap("b", &["only-in-b"])], 0, "only-in-b");
        // "No matches" under the active project on top, then b's hit.
        assert_eq!(
            popup_lines(&results, ACTIVE),
            [
                Line::Header {
                    name: ACTIVE,
                    first: true
                },
                Line::NoMatches,
                Line::Header {
                    name: "b",
                    first: false
                },
                Line::Hit(0),
            ]
        );
    }

    #[test]
    fn projects_with_the_same_name_get_their_own_groups() {
        let snaps = [
            snap("here", &["topic"]),
            snap("app", &["topic"]),
            snap("app", &["topic"]),
        ];
        let results = search(&snaps, 0, "topic");
        assert_eq!(headers(&results), [ACTIVE, "app", "app"]);
    }

    #[test]
    fn finds_changed_files_and_commits() {
        let snap = snap("a", &[]);
        let status = WorktreeStatus {
            ahead: 0,
            behind: 0,
            entries: vec![StatusEntry {
                path: "src/topic.rs".to_owned(),
                orig_path: None,
                index: Change::Modified,
                worktree: Change::None,
                kind: EntryKind::Tracked,
            }],
        };
        let commit = Commit {
            id: oid(0xab),
            parents: Default::default(),
            author: "Ada".to_owned(),
            time: 0,
            commit_time: 0,
            summary: "Fix the topic".to_owned(),
        };
        let history = History::build(&[commit], None);
        let hits = project_hits(&snap, Some(&status), Some(&history), "topic");
        let found: Vec<(Kind, &str)> = hits.iter().map(|h| (h.kind, h.detail.as_str())).collect();
        assert_eq!(
            found,
            [(Kind::Changed, "staged"), (Kind::Commit, "abababa · Ada")]
        );
        // A commit id prefix finds it, too.
        let by_id = project_hits(&snap, None, Some(&history), "abab");
        assert_eq!(by_id.len(), 1);
    }

    /// A repo with one commit and an extra branch `branch`, opened as a tab.
    fn open_with(dir: &Path, branch: &str) -> Project {
        git(dir, &["init", "-q", "-b", "main"]);
        std::fs::write(dir.join("a.txt"), "a\n").unwrap();
        git(dir, &["add", "a.txt"]);
        git(dir, &["commit", "-q", "-m", "first"]);
        git(dir, &["branch", branch]);
        open(dir)
    }

    /// The whole path once, with real repositories opened as tabs.
    #[test]
    fn finds_branches_in_open_projects() {
        let (a, b) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let projects = [
            open_with(a.path(), "feature-here"),
            open_with(b.path(), "feature-there"),
        ];
        let found: Vec<(String, Option<usize>)> = hits(&projects, 1, "feature")
            .into_iter()
            .filter(|h| h.kind == Kind::Branch)
            .map(|h| (h.title, h.project.map(|(tab, _)| tab)))
            .collect();
        assert_eq!(
            found,
            [
                ("feature-there".to_owned(), None),
                ("feature-here".to_owned(), Some(0))
            ]
        );
    }
}
