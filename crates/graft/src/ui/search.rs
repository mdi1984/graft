//! The toolbar search (Ctrl+K): type to find worktrees, branches, tags,
//! commits and changed files in every open project; Enter or a click jumps
//! there (switching to that project's tab).

use egui::{
    Align, Area, Frame, Id, Key, Layout, Margin, Modifiers, Order, Rect, RichText, ScrollArea,
    Sense, Stroke, TextEdit, Ui, vec2,
};

use std::path::PathBuf;
use std::sync::Arc;

use super::icons::{self, Icon};
use crate::keys::Command;
use crate::project::{Action, FileKey, Project};
use crate::theme::{self, RADIUS, color};

/// One search result.
struct Hit {
    kind: &'static str,
    title: String,
    detail: String,
    /// The other project's tab index and name, for a hit outside the
    /// active project.
    project: Option<(usize, String)>,
    actions: Vec<Action>,
}

const LIMIT: usize = 8;
const HEADER_HEIGHT: f32 = 24.0;

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

/// Hits in every open project: the active project's first, then one group
/// per other project in tab order. A hit elsewhere switches to its project
/// when chosen.
fn hits(projects: &[Project], active: usize, text: &str) -> Vec<Hit> {
    let order = std::iter::once(active).chain((0..projects.len()).filter(|&i| i != active));
    order
        .filter_map(|i| Some((i, projects.get(i)?)))
        .flat_map(|(i, project)| {
            project_hits(project, text).into_iter().map(move |mut hit| {
                if i != active {
                    hit.project = Some((i, project.name.clone()));
                    hit.actions = vec![Action::InProject {
                        project: i,
                        actions: hit.actions,
                    }];
                }
                hit
            })
        })
        .collect()
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

fn project_hits(project: &Project, text: &str) -> Vec<Hit> {
    let query: Vec<String> = text.split_whitespace().map(str::to_lowercase).collect();
    let mut out = Vec::new();
    if query.is_empty() {
        return out;
    }
    let Some(snap) = &project.snapshot else {
        return out;
    };

    for wt in snap
        .worktrees
        .iter()
        .filter(|wt| !wt.prunable)
        .filter(|wt| matches(&query, &[&wt.label(), &wt.path.to_string_lossy()]))
        .take(LIMIT)
    {
        out.push(Hit {
            kind: "Worktree",
            title: wt.label(),
            detail: wt.path.display().to_string(),
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
            kind: "Branch",
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
            kind: "Remote",
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
            kind: "Tag",
            title: t.name.clone(),
            detail: graft_core::short(&t.target),
            project: None,
            actions: vec![Action::Reveal(t.target)],
        });
    }

    if let Some(status) = project.worktree().and_then(|wt| wt.status.as_ref()) {
        for entry in status
            .entries
            .iter()
            .filter(|e| matches(&query, &[&e.path]))
            .take(LIMIT)
        {
            let staged = entry.is_staged() && !entry.is_unstaged();
            out.push(Hit {
                kind: "Changed",
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

    if let Some(history) = &project.history {
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
                kind: "Commit",
                title: c.summary.clone(),
                detail: format!("{} · {}", graft_core::short(&c.id), c.author),
                project: None,
                actions: vec![Action::Reveal(c.id)],
            });
        }
    }
    out
}

pub fn show(
    ui: &mut Ui,
    projects: &[Project],
    active: usize,
    text: &mut String,
    actions: &mut Vec<Action>,
) {
    let id = Id::unique("toolbar-search");
    let selected_id = id.with("selected");
    let popup_id = id.with("popup-rect");
    let keys = &projects[active].settings.keys;
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
    let width = ui.available_width().clamp(160.0, 300.0);

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
    // The active project found nothing, but others did: say so on top.
    let none_here = results.first().is_some_and(|hit| hit.project.is_some());
    let elsewhere = results.iter().any(|hit| hit.project.is_some());
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
                    let headers = (0..results.len())
                        .filter(|&i| header(&results, i, elsewhere, &active_name).is_some())
                        .count()
                        + usize::from(none_here) * 2;
                    let height = (results.len() as f32 * (30.0 + spacing)
                        + headers as f32 * (HEADER_HEIGHT + spacing))
                        .min(420.0);
                    ScrollArea::vertical()
                        .max_height(420.0)
                        .min_scrolled_height(height)
                        .show(ui, |ui| {
                            // Headers right above the selected row scroll in with it.
                            let mut above: Option<Rect> = None;
                            if none_here {
                                let top = group_header(ui, &active_name, true);
                                let (rect, _) = ui.allocate_exact_size(
                                    vec2(ui.available_width(), HEADER_HEIGHT),
                                    Sense::hover(),
                                );
                                let none = ui.painter().layout_no_wrap(
                                    "No matches".to_owned(),
                                    theme::small(),
                                    color::text_faint(),
                                );
                                super::widgets::paint_galley_left(
                                    ui.painter(),
                                    rect.left() + 8.0,
                                    rect.center().y,
                                    none,
                                    color::text_faint(),
                                );
                                above = Some(top.union(rect));
                            }
                            for (i, hit) in results.iter().enumerate() {
                                if let Some(name) = header(&results, i, elsewhere, &active_name) {
                                    let rect = group_header(ui, name, i == 0 && !none_here);
                                    above = Some(above.map_or(rect, |a| a.union(rect)));
                                }
                                let response = row(ui, hit, i == selected);
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
        actions.extend(hit.actions.iter().cloned());
        text.clear();
        ui.memory_mut(|m| m.surrender_focus(id));
        ui.data_mut(|d| d.remove::<Rect>(popup_id));
    }
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
    let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width(), 30.0), Sense::click());
    let painter = ui.painter();
    if selected {
        painter.rect_filled(rect, RADIUS, color::selected_row_bg());
    } else if response.hovered() {
        painter.rect_filled(rect, RADIUS, color::hover_row_bg());
    }
    let y = rect.center().y;
    let kind = painter.layout_no_wrap(hit.kind.to_owned(), theme::small(), color::text_faint());
    painter.galley(
        egui::pos2(rect.left() + 8.0, y - kind.size().y / 2.0),
        kind,
        color::text_faint(),
    );
    let detail_width = 150.0;
    let font = if matches!(hit.kind, "Commit" | "Changed") {
        theme::body()
    } else {
        theme::mono()
    };
    let title = super::widgets::elided(
        ui,
        &hit.title,
        font,
        color::text(),
        rect.width() - 80.0 - detail_width - 16.0,
    );
    super::widgets::paint_galley_left(painter, rect.left() + 80.0, y, title, color::text());
    let detail = super::widgets::elided_path(
        ui,
        &hit.detail,
        theme::small(),
        color::text_weak(),
        detail_width,
    );
    let x = rect.right() - 8.0 - detail.size().x;
    super::widgets::paint_galley_left(painter, x, y, detail, color::text_weak());
    response
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::process::Command as Cmd;
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    use graft_core::Console;
    use graft_core::jobs::JobPool;

    use super::*;

    fn git(dir: &Path, args: &[&str]) {
        let status = Cmd::new("git")
            .args([
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "-c",
                "commit.gpgsign=false",
            ])
            .args(args)
            .current_dir(dir)
            .status()
            .unwrap();
        assert!(status.success(), "git {args:?}");
    }

    /// A repo with one commit and an extra branch `branch`.
    fn repo(dir: &Path, branch: &str) {
        git(dir, &["init", "-q", "-b", "main"]);
        std::fs::write(dir.join("a.txt"), "a\n").unwrap();
        git(dir, &["add", "a.txt"]);
        git(dir, &["commit", "-q", "-m", "first"]);
        git(dir, &["branch", branch]);
    }

    fn open(root: &Path, pool: &JobPool) -> Project {
        let mut project = Project::new(
            root.to_path_buf(),
            root.to_path_buf(),
            Console::default(),
            pool.clone(),
        );
        let start = Instant::now();
        while project.snapshot.is_none() {
            assert!(
                start.elapsed() < Duration::from_secs(20),
                "snapshot never came"
            );
            std::thread::sleep(Duration::from_millis(20));
            project.poll();
        }
        project
    }

    #[test]
    fn finds_branches_in_other_projects() {
        let (a, b) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        repo(a.path(), "feature-here");
        repo(b.path(), "feature-there");
        let pool = JobPool::new(2, Arc::new(|| {}));
        let projects = [open(a.path(), &pool), open(b.path(), &pool)];

        // Searching from the second tab: its own hit comes first.
        let found: Vec<Hit> = hits(&projects, 1, "feature")
            .into_iter()
            .filter(|h| h.kind == "Branch")
            .collect();
        assert_eq!(found.len(), 2);
        assert_eq!(found[0].title, "feature-there");
        assert!(found[0].project.is_none());
        assert!(matches!(found[0].actions[..], [Action::Reveal(_)]));

        assert_eq!(found[1].title, "feature-here");
        assert_eq!(found[1].project, Some((0, projects[0].name.clone())));
        match &found[1].actions[..] {
            [
                Action::InProject {
                    project: 0,
                    actions,
                },
            ] => {
                assert!(matches!(actions[..], [Action::Reveal(_)]));
            }
            other => panic!("unexpected actions {other:?}"),
        }
    }

    #[test]
    fn groups_hits_by_project_active_first() {
        let (a, b, c) = (
            tempfile::tempdir().unwrap(),
            tempfile::tempdir().unwrap(),
            tempfile::tempdir().unwrap(),
        );
        repo(a.path(), "topic");
        repo(b.path(), "topic");
        repo(c.path(), "topic");
        let pool = JobPool::new(2, Arc::new(|| {}));
        let projects = [
            open(a.path(), &pool),
            open(b.path(), &pool),
            open(c.path(), &pool),
        ];
        let (na, nc) = (projects[0].name.as_str(), projects[2].name.as_str());

        // "main" matches each project's main worktree and main branch.
        // Searching from the second tab: its hits first, then tab 1, tab 3.
        let results = hits(&projects, 1, "main");
        let found: Vec<(&str, Option<usize>)> = results
            .iter()
            .filter(|h| matches!(h.kind, "Worktree" | "Branch"))
            .map(|h| (h.kind, h.project.as_ref().map(|(tab, _)| *tab)))
            .collect();
        assert_eq!(
            found,
            [
                ("Worktree", None),
                ("Branch", None),
                ("Worktree", Some(0)),
                ("Branch", Some(0)),
                ("Worktree", Some(2)),
                ("Branch", Some(2)),
            ]
        );

        // One header per project, where its hits start.
        assert_eq!(headers(&results), ["here", na, nc]);
    }

    #[test]
    fn no_header_when_only_the_active_project_matches() {
        let (a, b) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        repo(a.path(), "only-in-a");
        repo(b.path(), "topic");
        let pool = JobPool::new(2, Arc::new(|| {}));
        let projects = [open(a.path(), &pool), open(b.path(), &pool)];

        let results = hits(&projects, 0, "only-in-a");
        assert!(!results.is_empty());
        assert!(results.iter().all(|h| h.project.is_none()));
        assert!(headers(&results).is_empty());
    }

    #[test]
    fn other_projects_only_when_the_active_one_has_no_match() {
        let (a, b) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        repo(a.path(), "topic");
        repo(b.path(), "only-in-b");
        let pool = JobPool::new(2, Arc::new(|| {}));
        let projects = [open(a.path(), &pool), open(b.path(), &pool)];

        // The popup puts "No matches" under the active project on top.
        let results = hits(&projects, 0, "only-in-b");
        assert!(results[0].project.is_some());
        assert_eq!(headers(&results), [projects[1].name.as_str()]);
    }

    #[test]
    fn projects_with_the_same_folder_name_get_their_own_groups() {
        let dir = tempfile::tempdir().unwrap();
        let roots = [
            dir.path().join("here"),
            dir.path().join("x").join("app"),
            dir.path().join("y").join("app"),
        ];
        for root in &roots {
            std::fs::create_dir_all(root).unwrap();
            repo(root, "topic");
        }
        let pool = JobPool::new(2, Arc::new(|| {}));
        let projects: Vec<Project> = roots.iter().map(|r| open(r, &pool)).collect();

        let results = hits(&projects, 0, "topic");
        assert_eq!(headers(&results), ["here", "app", "app"]);
    }

    /// The group headers the popup draws, the active project's as "here".
    fn headers(results: &[Hit]) -> Vec<&str> {
        let elsewhere = results.iter().any(|h| h.project.is_some());
        (0..results.len())
            .filter_map(|i| header(results, i, elsewhere, "here"))
            .collect()
    }
}
