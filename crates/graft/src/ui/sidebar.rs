//! Left sidebar: a filter for its lists, WORKSPACE, the foldable
//! WORKTREES, BRANCHES, REMOTES and TAGS in the order the user put them in,
//! and STASHES at the bottom.

use egui::{
    CursorIcon, DragAndDrop, Id, Key, Rect, Response, ScrollArea, Sense, Stroke, Ui, pos2, vec2,
};
use graft_core::console::redact;
use graft_core::paths::same_path;
use graft_core::{Branch, RemoteBranch, RepoSnapshot, Upstream, Worktree, paths, remote};

use super::branch_menu::{self, Open, can_item, checkout_color};
use super::can::{self, Can};
use super::icons::Icon;
use super::widgets::{
    self, Badge, ICON_SIZE, clearable_field, elided, elided_path, paint_badges_right,
    paint_galley_left, paint_icon, paint_row_bg, section_heading,
};
use crate::project::filter::{self, ListFilter};
use crate::project::{
    Action, CheckedOut, Confirm, DialogKind, Project, RemoteOp, Tab, WorkspaceView, merge_dialog,
};
use crate::sidebar_order::{Section, SidebarOrder};
use crate::text::plural;
use crate::theme::{self, color};
use crate::ui::{Actions, AppAction};

const SECTION_GAP: f32 = 14.0;
const PAD_X: f32 = 8.0;
/// How far each folder level indents its rows.
const NEST: f32 = 14.0;

pub fn show(ui: &mut Ui, project: &Project, actions: &mut Actions) {
    let Some(repo) = &project.snapshot else {
        return;
    };
    let nav = &project.nav;
    let current = project.worktree();
    filter_row(ui, project, actions);
    ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 2.0;

            heading(ui, "WORKSPACE");
            let in_workspace = nav.tab == Tab::Workspace;
            let dirty = current.map_or(0, Worktree::dirty);
            let dirty_badge =
                (dirty > 0).then(|| Badge::new(ui, dirty.to_string(), widgets::dirty()));
            let status_selected = in_workspace && nav.view == WorkspaceView::FileStatus;
            if nav_row(
                ui,
                Icon::FileStatus,
                WorkspaceView::FileStatus.label(),
                status_selected,
                dirty_badge,
            )
            .clicked()
            {
                actions.push(Action::ShowFileStatus);
            }
            let history_selected = in_workspace && nav.view == WorkspaceView::History;
            let history = WorkspaceView::History.label();
            if nav_row(ui, Icon::History, history, history_selected, None).clicked() {
                actions.push(Action::ShowHistory);
            }
            ui.add_space(SECTION_GAP);

            let order = project.settings().sidebar_order;
            let mut placed = Vec::with_capacity(order.sections().len());
            for section in order.sections() {
                let top = ui.cursor().top();
                let heading = list_section(ui, project, repo, section, actions);
                placed.push(Placed {
                    top,
                    heading: heading.rect.center().y,
                    bottom: ui.cursor().top(),
                });
                if heading.drag_started_by(egui::PointerButton::Primary) {
                    DragAndDrop::set_payload(ui.ctx(), section);
                }
                heading.context_menu(|ui| order_menu(ui, order, section, actions));
            }
            drop_section(ui, order, &placed, actions);

            if footer_row(ui, "STASHES", &stash_count(project, repo)).clicked() {
                actions.push(Action::SetTab(Tab::Stashes));
            }
        });
}

/// The filter field above the lists, with a "×" inside that clears it.
/// Escape in the field clears it too. What is typed is saved per repository.
fn filter_row(ui: &mut Ui, project: &Project, actions: &mut Actions) {
    let mut text = project.memory().sidebar_filter.clone();
    let width = ui.available_width();
    let (field, clear) = clearable_field(ui, &mut text, "Filter", width);
    let field = field.on_hover_text(
        "Shows only the worktrees, branches, remotes, tags and stashes that contain this",
    );
    if field.changed() {
        actions.push(Action::SetSidebarFilter(text.clone()));
    }
    // Escape takes the focus away, so the field may just have lost it.
    let escape =
        (field.has_focus() || field.lost_focus()) && ui.input(|i| i.key_pressed(Key::Escape));
    if (escape || clear) && !text.is_empty() {
        actions.push(Action::SetSidebarFilter(String::new()));
    }
    ui.add_space(8.0);
}

/// The STASHES count: how many match the filter, once the list is loaded.
fn stash_count(project: &Project, repo: &RepoSnapshot) -> String {
    let filter = project.filter();
    match &project.stashes {
        Some(Ok(list)) if filter.is_on() => {
            filter::count(list.iter().filter(|s| filter.stash(s)).count(), list.len())
        }
        _ => repo.stash_count.to_string(),
    }
}

/// One of the sections the user can move: its heading (to click, drag or
/// right-click) and, when open, its rows.
fn list_section(
    ui: &mut Ui,
    project: &Project,
    repo: &RepoSnapshot,
    section: Section,
    actions: &mut Actions,
) -> Response {
    let nav = &project.nav;
    let current = project.worktree();
    let filter = project.filter();
    match section {
        Section::Worktrees => {
            let new_worktree = can::new_worktree(project);
            let shown: Vec<&Worktree> = (repo.worktrees.iter())
                .filter(|wt| filter.worktree(wt))
                .collect();
            let count = filter::count(shown.len(), repo.worktrees.len());
            let heading = collapsible(ui, section, &count, true, Some(&new_worktree));
            if heading.plus_clicked {
                actions.push(Action::NewWorktree(None));
            }
            if heading.open {
                no_match(ui, shown.is_empty() && filter.is_on());
                for wt in shown {
                    let selected = same_path(&wt.path, &nav.worktree);
                    let response = worktree_row(ui, wt, selected);
                    if response.clicked() && !wt.prunable {
                        actions.push(Action::OpenWorktree(wt.path.clone()));
                    }
                    response
                        .context_menu(|ui| worktree_menu(ui, wt, &can::act(project, ""), actions));
                }
                ui.add_space(SECTION_GAP);
            }
            heading.response
        }
        Section::Branches => {
            let branches: Vec<&Branch> = (repo.branches.iter())
                .filter(|b| filter.branch(b))
                .collect();
            let count = filter::count(branches.len(), repo.branches.len());
            let heading = collapsible(ui, section, &count, true, None);
            if heading.open {
                let head = current.and_then(|wt| wt.branch.as_deref());
                let is_head = |b: &Branch| head == Some(b.name.as_str());
                grouped(
                    ui,
                    Id::unique("branch-folders"),
                    &branches,
                    &|b| b.name.as_str(),
                    &is_head,
                    Level::top(PAD_X - 4.0, filter.is_on()),
                    &mut |ui, branch, label, indent| {
                        let response = branch_row(ui, repo, branch, label, indent, head);
                        if response.double_clicked() {
                            open(Open::local(project, repo, &branch.name), actions);
                        } else if response.clicked() {
                            actions.push(Action::Reveal(branch.target));
                        }
                        response.context_menu(|ui| branch_menu(ui, project, branch, head, actions));
                    },
                );
                if repo.branches.is_empty() {
                    faint_row(ui, "No branches yet");
                } else {
                    no_match(ui, branches.is_empty() && filter.is_on());
                }
                ui.add_space(SECTION_GAP);
            }
            heading.response
        }
        Section::Remotes => {
            let shown = (repo.remotes.iter())
                .filter(|r| filter.remote(r, &repo.remote_branches))
                .count();
            let count = filter::count(shown, repo.remotes.len());
            let heading = collapsible(ui, section, &count, false, None);
            if heading.open {
                no_match(ui, shown == 0 && filter.is_on());
                remotes(ui, project, repo, &filter, actions);
                ui.add_space(SECTION_GAP);
            }
            heading.response
        }
        Section::Tags => {
            let tags: Vec<_> = repo.tags.iter().rev().filter(|t| filter.tag(t)).collect();
            let count = filter::count(tags.len(), repo.tags.len());
            let heading = collapsible(ui, section, &count, false, None);
            if heading.open {
                for tag in &tags {
                    let response =
                        simple_row(ui, Icon::Tag, &tag.name, color::tag(), color::text(), 20.0);
                    if response.clicked() {
                        actions.push(Action::Reveal(tag.target));
                    }
                    response.context_menu(|ui| {
                        tag_menu(ui, repo, &tag.name, &can::act(project, ""), actions)
                    });
                }
                if repo.tags.is_empty() {
                    faint_row(ui, "No tags");
                } else {
                    no_match(ui, tags.is_empty() && filter.is_on());
                }
                ui.add_space(SECTION_GAP);
            }
            heading.response
        }
    }
}

/// Where a section was drawn, in screen points.
struct Placed {
    top: f32,
    /// The middle of its heading.
    heading: f32,
    bottom: f32,
}

/// While a section heading is dragged, a line shows where it would land,
/// and letting go there moves it. Letting go outside the sidebar does nothing.
fn drop_section(ui: &Ui, order: SidebarOrder, placed: &[Placed], actions: &mut Actions) {
    let Some(dragged) = DragAndDrop::payload::<Section>(ui.ctx()) else {
        return;
    };
    ui.ctx().set_cursor_icon(CursorIcon::Grabbing);
    let (Some(pointer), Some(last)) = (ui.ctx().pointer_latest_pos(), placed.last()) else {
        return;
    };
    let area = ui.clip_rect();
    if !area.contains(pointer) {
        return;
    }
    // Above a heading's middle drops before its section. Headings, not
    // whole sections: an open section can be taller than the sidebar.
    let gap = placed.iter().filter(|p| p.heading < pointer.y).count();
    let moved = order.moved_to(*dragged, gap);
    if moved == order {
        // Right above or below itself: dropping there changes nothing.
        return;
    }
    // Kept on screen, also when that edge of the section is scrolled away.
    let y = placed.get(gap).map_or(last.bottom, |p| p.top) - 1.0;
    let y = y.clamp(area.top() + 1.0, area.bottom() - 1.0);
    ui.painter().hline(
        area.x_range().shrink(4.0),
        y,
        Stroke::new(2.0, color::accent()),
    );
    if ui.input(|i| i.pointer.any_released()) {
        DragAndDrop::clear_payload(ui.ctx());
        actions.app(AppAction::SidebarOrder(moved));
    }
}

/// A section heading's right-click menu: move it, or put all back.
fn order_menu(ui: &mut Ui, order: SidebarOrder, section: Section, actions: &mut Actions) {
    let mut item = |ui: &mut Ui, label: &str, can: bool, why_not: &str, moved: SidebarOrder| {
        if ui
            .add_enabled(can, egui::Button::new(label))
            .on_disabled_hover_text(why_not)
            .clicked()
        {
            actions.app(AppAction::SidebarOrder(moved));
            ui.close();
        }
    };
    let up = order.can_move_up(section);
    item(
        ui,
        "Move up",
        up,
        "Already at the top",
        order.moved_up(section),
    );
    let down = order.can_move_down(section);
    item(
        ui,
        "Move down",
        down,
        "Already at the bottom",
        order.moved_down(section),
    );
    ui.separator();
    let reset = !order.is_default();
    let why_not = "Already in the usual order";
    item(ui, "Reset order", reset, why_not, SidebarOrder::default());
}

/// Where [`grouped`] is in the folder tree.
#[derive(Clone, Copy)]
struct Level {
    /// Bytes of the name the folders above already show (`feature/`).
    skip: usize,
    /// Left edge of this level's rows.
    indent: f32,
    /// Folders show open whatever was saved: the filter is on, so they
    /// hold only matches.
    open_all: bool,
}

impl Level {
    fn top(indent: f32, open_all: bool) -> Self {
        Self {
            skip: 0,
            indent,
            open_all,
        }
    }
}

/// Rows nested by the `/` prefixes of their names: `feature/foo` and
/// `feature/bar` sit in a foldable `feature` folder as `foo` and `bar`.
/// `items` must be sorted by name, as git lists refs, so that each folder's
/// items are next to each other. Plain names come first, then the folders.
/// `marked` items light up a folded folder (the current branch).
#[allow(clippy::too_many_arguments)]
fn grouped<'a, T>(
    ui: &mut Ui,
    id: Id,
    items: &[&'a T],
    name: &dyn Fn(&T) -> &str,
    marked: &dyn Fn(&T) -> bool,
    level: Level,
    leaf: &mut dyn FnMut(&mut Ui, &'a T, &str, f32),
) {
    for &item in items {
        let rest = &name(item)[level.skip..];
        if !rest.contains('/') {
            leaf(ui, item, rest, level.indent);
        }
    }
    let mut i = 0;
    while i < items.len() {
        let full = name(items[i]);
        let Some((folder, _)) = full[level.skip..].split_once('/') else {
            i += 1;
            continue;
        };
        let prefix = &full[..level.skip + folder.len() + 1];
        let n = items[i..]
            .iter()
            .take_while(|item| name(item).starts_with(prefix))
            .count();
        let run = &items[i..i + n];
        let has_mark = run.iter().any(|item| marked(item));
        let count = n.to_string();
        if widgets::folder_row(
            ui,
            id.with(prefix),
            folder,
            theme::mono(),
            &count,
            level.indent,
            has_mark,
            level.open_all,
        ) {
            let inner = Level {
                skip: prefix.len(),
                indent: level.indent + NEST,
                ..level
            };
            grouped(ui, id, run, name, marked, inner, leaf);
        }
        i += n;
    }
}

fn row(ui: &mut Ui, height: f32) -> (Rect, Response) {
    ui.allocate_exact_size(vec2(ui.available_width(), height), Sense::click())
}

/// A section heading that does nothing (WORKSPACE).
fn heading(ui: &mut Ui, title: &str) {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 22.0), Sense::hover());
    section_heading(ui, ui.painter(), rect.left() + 6.0, rect.center().y, title);
}

/// What [`collapsible`] drew and the user did.
struct Heading {
    /// The heading row: a click folds, a drag moves, a right-click has a menu.
    response: Response,
    /// The section shows its rows.
    open: bool,
    /// The "+" button was clicked while it could act.
    plus_clicked: bool,
}

/// A heading that folds its section; the open state survives restarts. It
/// has a "+" button when `plus` says what it adds (or why it can't now).
fn collapsible(
    ui: &mut Ui,
    section: Section,
    count: &str,
    default_open: bool,
    plus: Option<&Can>,
) -> Heading {
    let title = section.title();
    let id = Id::unique(("sidebar-section", title));
    let mut open = ui.data_mut(|d| *d.get_persisted_mut_or(id, default_open));
    let size = vec2(ui.available_width(), 22.0);
    let (rect, response) = ui.allocate_exact_size(size, Sense::click_and_drag());
    if response.hovered() || response.dragged() {
        ui.painter().rect_filled(rect, 4, color::hover_row_bg());
    }
    let y = rect.center().y;
    let chevron = if open {
        Icon::ChevronDown
    } else {
        Icon::ChevronRight
    };
    let icon_rect = Rect::from_center_size(pos2(rect.left() + 10.0, y), vec2(10.0, 10.0));
    super::icons::paint(ui.painter(), icon_rect, chevron, color::text_faint());
    section_heading(ui, ui.painter(), rect.left() + 20.0, y, title);
    let mut count_right = rect.right() - PAD_X;
    let mut plus_clicked = false;
    if let Some(can) = plus {
        let button = Rect::from_center_size(pos2(rect.right() - 12.0, y), vec2(22.0, 22.0));
        // It takes clicks even when it can't act, so they don't fold the section.
        let add = ui.interact(button, id.with("plus"), Sense::click());
        widgets::paint_icon_button_when(ui, button, &add, Icon::Plus, can.ok);
        plus_clicked = add.on_hover_text(&can.hint).clicked() && can.ok;
        count_right = button.left() - 2.0;
    }
    let count = ui
        .painter()
        .layout_no_wrap(count.to_owned(), theme::small(), color::text_faint());
    paint_galley_left(ui.painter(), count_right - count.size().x, y, count);
    if response.clicked() {
        open = !open;
        ui.data_mut(|d| d.insert_persisted(id, open));
    }
    Heading {
        response,
        open,
        plus_clicked,
    }
}

fn nav_row(ui: &mut Ui, icon: Icon, label: &str, selected: bool, badge: Option<Badge>) -> Response {
    let (rect, response) = row(ui, 28.0);
    let painter = ui.painter();
    paint_row_bg(painter, rect, &response, selected);
    let y = rect.center().y;
    paint_icon(painter, pos2(rect.left() + PAD_X, y), icon, color::text());
    let text_x = rect.left() + PAD_X + ICON_SIZE + 8.0;
    let badges_x = paint_badges_right(
        painter,
        rect.right() - PAD_X,
        y,
        badge.into_iter().collect(),
    );
    let text = elided(
        ui,
        label,
        theme::body(),
        color::text(),
        badges_x - 6.0 - text_x,
    );
    paint_galley_left(painter, text_x, y, text);
    response
}

fn worktree_row(ui: &mut Ui, wt: &Worktree, selected: bool) -> Response {
    let (rect, response) = row(ui, 42.0);
    let painter = ui.painter();
    paint_row_bg(painter, rect, &response, selected);

    let top_y = rect.top() + 14.0;
    let bottom_y = rect.top() + 29.0;
    // Green is always the main folder, orange a linked worktree (like the
    // branch dots below); the row background shows which one is selected.
    let icon_color = if wt.prunable {
        color::text_faint()
    } else {
        checkout_color(CheckedOut::of(Some(wt)))
    };
    paint_icon(
        painter,
        pos2(rect.left() + PAD_X, top_y),
        Icon::Folder,
        icon_color,
    );

    let mut badges = Vec::new();
    if wt.is_dirty() {
        badges.push(Badge::new(ui, wt.dirty().to_string(), widgets::dirty()));
    }
    if wt.ahead() > 0 {
        badges.push(Badge::new(ui, format!("↑{}", wt.ahead()), widgets::ahead()));
    }
    if wt.behind() > 0 {
        badges.push(Badge::new(
            ui,
            format!("↓{}", wt.behind()),
            widgets::neutral(),
        ));
    }
    if wt.prunable {
        badges.push(Badge::new(ui, "missing", widgets::neutral()));
    } else if wt.locked {
        badges.push(Badge::new(ui, "locked", widgets::neutral()));
    }
    let badges_x = paint_badges_right(painter, rect.right() - PAD_X, top_y, badges);

    let text_x = rect.left() + PAD_X + ICON_SIZE + 8.0;
    let fg = if wt.prunable {
        color::text_faint()
    } else {
        color::text()
    };
    let branch = elided(ui, &wt.label(), theme::mono(), fg, badges_x - 6.0 - text_x);
    paint_galley_left(painter, text_x, top_y, branch);
    let path = if wt.is_main {
        format!("{}  · main folder", graft_core::paths::display(&wt.path))
    } else {
        graft_core::paths::display(&wt.path)
    };
    let path = elided_path(
        ui,
        &path,
        theme::small(),
        color::text_weak(),
        rect.right() - PAD_X - text_x,
    );
    paint_galley_left(painter, text_x, bottom_y, path);

    response.on_hover_text(graft_core::paths::display(&wt.path))
}

fn worktree_menu(ui: &mut Ui, wt: &Worktree, can: &Can, actions: &mut Actions) {
    if wt.prunable {
        let prune = egui::Button::new("Prune missing worktrees");
        if can
            .why_not(ui.add_enabled(can.ok, prune))
            .on_hover_text("The folder is gone; forget it (git worktree prune)")
            .clicked()
        {
            actions.push(Action::PruneWorktrees);
            ui.close();
        }
        return;
    }
    if ui.button("Show in Explorer").clicked() {
        actions.push(Action::OpenFolder(wt.path.clone()));
        ui.close();
    }
    if ui.button("Open terminal here").clicked() {
        actions.push(Action::OpenTerminal(wt.path.clone()));
        ui.close();
    }
    if ui.button("Copy path").clicked() {
        ui.ctx().copy_text(paths::display(&wt.path));
        ui.close();
    }
    if wt.is_main {
        return;
    }
    ui.separator();
    let lock_label = if wt.locked { "Unlock" } else { "Lock" };
    if can
        .why_not(ui.add_enabled(can.ok, egui::Button::new(lock_label)))
        .on_hover_text("A locked worktree is never removed or pruned")
        .clicked()
    {
        actions.push(Action::LockWorktree {
            path: wt.path.clone(),
            locked: !wt.locked,
        });
        ui.close();
    }
    let remove = if wt.locked {
        &can::no("Unlock it first")
    } else {
        can
    };
    let ask = Action::Ask(Box::new(remove_confirm(wt)));
    can_item(ui, "Remove worktree…", remove, actions, ask);
}

fn remove_confirm(wt: &Worktree) -> Confirm {
    let dirty = wt.dirty();
    let branch = match &wt.branch {
        Some(b) => format!("The branch {b} is kept."),
        None => {
            "Its detached HEAD is kept in the history only if a branch points at it.".to_owned()
        }
    };
    let body = if dirty > 0 {
        format!(
            "The folder {} will be deleted, including {}, which will be lost. {branch}",
            paths::display(&wt.path),
            plural(dirty, "uncommitted change", "uncommitted changes")
        )
    } else {
        format!(
            "The folder {} will be deleted. {branch}",
            paths::display(&wt.path)
        )
    };
    Confirm {
        title: format!("Remove worktree {}?", wt.label()),
        body,
        confirm_label: if dirty > 0 {
            "Remove and lose changes".to_owned()
        } else {
            "Remove worktree".to_owned()
        },
        action: Action::RemoveWorktree {
            path: wt.path.clone(),
            force: dirty > 0,
        },
    }
}

fn branch_menu(
    ui: &mut Ui,
    project: &Project,
    branch: &Branch,
    head: Option<&str>,
    actions: &mut Actions,
) {
    let Some(repo) = &project.snapshot else {
        return;
    };
    let act = can::act(project, "");
    let name = branch.name.as_str();
    let is_head = head == Some(name);
    branch_menu::local_items(ui, project, repo, name, false, actions);
    if !is_head {
        merge_items(ui, project, name, actions);
    }
    let pull = can::pull(repo, name, &project.nav.worktree).unless(project.can_act().err());
    can_item(
        ui,
        "Pull",
        &pull,
        actions,
        Action::Remote(RemoteOp::PullBranch {
            branch: name.to_owned(),
        }),
    );
    can_item(
        ui,
        "New branch from here…",
        &act,
        actions,
        Action::OpenDialog(DialogKind::new_branch(name, name)),
    );
    let rename = act.clone().unless(repo.rename_blocked(name));
    can_item(
        ui,
        "Rename…",
        &rename,
        actions,
        Action::OpenDialog(DialogKind::RenameBranch {
            old: name.to_owned(),
            name: name.to_owned(),
        }),
    );
    let delete = if repo.worktree_for_branch(name).is_some() {
        &can::no("It is checked out in a worktree")
    } else {
        &act
    };
    can_item(
        ui,
        "Delete…",
        delete,
        actions,
        Action::Ask(Box::new(Confirm {
            title: format!("Delete branch {name}?"),
            body: match &branch.upstream {
                Upstream::Tracking { name: up, .. } => {
                    format!("The local branch goes away; {up} on the remote stays.")
                }
                _ => "The local branch goes away. If its commits are not merged anywhere, \
                      Graft asks again."
                    .to_owned(),
            },
            confirm_label: "Delete branch".to_owned(),
            action: Action::DeleteBranch {
                name: name.to_owned(),
                force: false,
            },
        })),
    );
    ui.separator();
    if ui.button("Copy name").clicked() {
        ui.ctx().copy_text(name.to_owned());
        ui.close();
    }
}

/// "Merge X into current…" and "Rebase current onto X…", for the selected
/// worktree's branch.
fn merge_items(ui: &mut Ui, project: &Project, name: &str, actions: &mut Actions) {
    let Some(wt) = project.worktree() else { return };
    let Some(current) = wt.branch.as_deref() else {
        return;
    };
    let can = can::merge(project.snapshot.as_deref(), Some(wt)).unless(project.can_act().err());
    if let Some(kind) = merge_dialog(project, Some(name)) {
        can_item(
            ui,
            &format!("Merge {name} into {current}…"),
            &can,
            actions,
            Action::OpenDialog(kind),
        );
    }
    can_item(
        ui,
        &format!("Rebase {current} onto {name}…"),
        &can,
        actions,
        Action::Ask(Box::new(Confirm {
            title: format!("Rebase {current} onto {name}?"),
            body: format!(
                "Replays the commits of {current} on top of {name}, one by one. \
                 This rewrites them: if {current} was pushed already, you will have \
                 to force push. If a commit conflicts, the rebase stops so you can \
                 resolve it."
            ),
            confirm_label: "Rebase".to_owned(),
            action: Action::Rebase(name.to_owned()),
        })),
    );
    ui.separator();
}

fn remote_branch_menu(
    ui: &mut Ui,
    project: &Project,
    repo: &RepoSnapshot,
    branch: &RemoteBranch,
    actions: &mut Actions,
) {
    let act = can::act(project, "");
    merge_items(ui, project, &branch.name, actions);
    let local = branch.branch_name();
    // With a local branch of its name, that is what opens.
    branch_menu::remote_items(ui, project, repo, branch, actions);
    can_item(
        ui,
        "New branch from here…",
        &act,
        actions,
        Action::OpenDialog(DialogKind::new_branch(&branch.name, &branch.name)),
    );
    can_item(
        ui,
        &format!("Delete on {}…", branch.remote),
        &act,
        actions,
        Action::Ask(Box::new(Confirm {
            title: format!("Delete {} on the remote?", branch.name),
            body: format!(
                "The branch {local} is deleted on {} for everyone. Local branches are not touched.",
                branch.remote
            ),
            confirm_label: "Delete on remote".to_owned(),
            action: Action::Remote(RemoteOp::DeleteRemoteBranch {
                remote: branch.remote.clone(),
                branch: local.to_owned(),
            }),
        })),
    );
    ui.separator();
    if ui.button("Copy name").clicked() {
        ui.ctx().copy_text(branch.name.clone());
        ui.close();
    }
}

fn tag_menu(ui: &mut Ui, repo: &RepoSnapshot, tag: &str, can: &Can, actions: &mut Actions) {
    if let Some(remote) = remote::tag_remote(repo) {
        can_item(
            ui,
            &format!("Push to {remote}"),
            can,
            actions,
            Action::Remote(RemoteOp::PushTag {
                remote: remote.to_owned(),
                tag: tag.to_owned(),
                created: false,
            }),
        );
    }
    can_item(
        ui,
        "Delete…",
        can,
        actions,
        Action::Ask(Box::new(Confirm {
            title: format!("Delete tag {tag}?"),
            body: "Deletes the local tag. A copy already pushed to a remote stays there."
                .to_owned(),
            confirm_label: "Delete tag".to_owned(),
            action: Action::DeleteTag(tag.to_owned()),
        })),
    );
    ui.separator();
    if ui.button("Copy name").clicked() {
        ui.ctx().copy_text(tag.to_owned());
        ui.close();
    }
}

/// A local branch, shown as `label` (its name without the folders above).
/// Green: checked out in the main folder. Orange: checked out in a linked
/// worktree. Highlighted: the selected worktree's branch.
fn branch_row(
    ui: &mut Ui,
    repo: &RepoSnapshot,
    branch: &Branch,
    label: &str,
    indent: f32,
    head: Option<&str>,
) -> Response {
    let (rect, response) = row(ui, 26.0);
    let painter = ui.painter();
    let is_head = head == Some(branch.name.as_str());
    paint_row_bg(painter, rect, &response, is_head);
    let y = rect.center().y;

    let checked_out = repo.worktree_for_branch(&branch.name);
    let in_main = checked_out.is_some_and(|wt| wt.is_main);
    let gone = matches!(branch.upstream, Upstream::Gone { .. });
    let dot = match CheckedOut::of(checked_out) {
        CheckedOut::Nowhere if gone => color::text_faint(),
        place => checkout_color(place),
    };
    painter.circle_filled(pos2(rect.left() + indent + 7.0, y), 3.0, dot);

    let meta = match &branch.upstream {
        Upstream::None => "local".to_owned(),
        Upstream::Gone { .. } => "gone".to_owned(),
        Upstream::Tracking { ahead, behind, .. } => match (ahead, behind) {
            (0, 0) => String::new(),
            (a, 0) => format!("↑{a}"),
            (0, b) => format!("↓{b}"),
            (a, b) => format!("↑{a} ↓{b}"),
        },
    };
    let meta = ui
        .painter()
        .layout_no_wrap(meta, theme::small(), color::text_weak());
    let meta_x = rect.right() - PAD_X - meta.size().x;
    paint_galley_left(painter, meta_x, y, meta);

    let text_x = rect.left() + indent + 18.0;
    let fg = if in_main {
        color::accent()
    } else {
        color::text()
    };
    let name = elided(ui, label, theme::mono(), fg, meta_x - 6.0 - text_x);
    paint_galley_left(painter, text_x, y, name);

    let tip = match &branch.upstream {
        Upstream::None => "Not pushed anywhere yet".to_owned(),
        Upstream::Gone { name } => format!("Upstream {name} was deleted"),
        Upstream::Tracking {
            name,
            ahead,
            behind,
        } => format!("Tracks {name} · {ahead} ahead, {behind} behind"),
    };
    let tip = if label.len() < branch.name.len() {
        format!("{}\n{tip}", branch.name)
    } else {
        tip
    };
    let tip = match checked_out {
        Some(wt) if wt.is_main => format!("{tip}\nChecked out in your main folder"),
        Some(wt) => format!(
            "{tip}\nChecked out in the worktree {}",
            paths::display(&wt.path)
        ),
        None => format!("{tip}\nDouble-click to check it out in the main folder"),
    };
    response.on_hover_text(tip)
}

/// Double-click on a branch: what its menu offers first. When that cannot
/// start, the project says why.
fn open(open: Option<Open>, actions: &mut Actions) {
    if let Some(open) = open {
        actions.push(open.action());
    }
}

fn remotes(
    ui: &mut Ui,
    project: &Project,
    repo: &RepoSnapshot,
    filter: &ListFilter,
    actions: &mut Actions,
) {
    let can_add = can::add_remote(project);
    // Only shown when it can't: why.
    let can_change = can::act(project, "");
    let add = Action::OpenDialog(DialogKind::add_remote(repo.remotes.is_empty()));
    for remote in &repo.remotes {
        if !filter.remote(remote, &repo.remote_branches) {
            continue;
        }
        simple_row(
            ui,
            Icon::Cloud,
            &remote.name,
            color::text_mid(),
            color::text(),
            8.0,
        )
        .on_hover_text(graft_core::console::redact(&remote.url))
        .context_menu(|ui| {
            can_item(
                ui,
                "Edit…",
                &can_change,
                actions,
                Action::OpenDialog(DialogKind::edit_remote(remote)),
            );
            if ui.button("Copy URL").clicked() {
                ui.ctx().copy_text(remote.url.clone());
                ui.close();
            }
            can_item(
                ui,
                "Remove…",
                &can_change,
                actions,
                Action::Ask(Box::new(Confirm {
                    title: format!("Remove remote {}?", remote.name),
                    body: format!(
                        "Graft forgets {} and its remote branches. Nothing changes on the \
                             server, and your local branches keep their commits; branches that \
                             tracked it lose their upstream.",
                        redact(&remote.url)
                    ),
                    confirm_label: "Remove remote".to_owned(),
                    action: Action::RemoveRemote(remote.name.clone()),
                })),
            );
            ui.separator();
            can_item(ui, "Add remote…", &can_add, actions, add.clone());
        });
        let branches: Vec<&RemoteBranch> = repo
            .remote_branches
            .iter()
            .filter(|b| b.remote == remote.name && filter.remote_branch(b))
            .collect();
        grouped(
            ui,
            Id::unique(("remote-folders", &remote.name)),
            &branches,
            &|b| b.branch_name(),
            &|_| false,
            Level::top(22.0, filter.is_on()),
            &mut |ui, branch, label, indent| {
                remote_branch_row(ui, project, repo, branch, label, indent, actions)
            },
        );
    }
    let (icon, text) = if can_add.ok {
        (color::text_weak(), color::text())
    } else {
        (color::text_faint(), color::text_faint())
    };
    let response = simple_row(ui, Icon::Plus, "Add remote…", icon, text, 8.0);
    if response.on_hover_text(&can_add.hint).clicked() && can_add.ok {
        actions.push(add);
    }
}

fn remote_branch_row(
    ui: &mut Ui,
    project: &Project,
    repo: &RepoSnapshot,
    branch: &RemoteBranch,
    label: &str,
    indent: f32,
    actions: &mut Actions,
) {
    let (rect, response) = row(ui, 24.0);
    paint_row_bg(ui.painter(), rect, &response, false);
    let y = rect.center().y;
    let dot = pos2(rect.left() + indent + 7.0, y);
    ui.painter()
        .circle_stroke(dot, 2.5, Stroke::new(1.0, color::text_weak()));
    let x = rect.left() + indent + 18.0;
    let name = elided(
        ui,
        label,
        theme::mono(),
        color::text(),
        rect.right() - PAD_X - x,
    );
    paint_galley_left(ui.painter(), x, y, name);
    let response = if label.len() < branch.branch_name().len() {
        response.on_hover_text(&branch.name)
    } else {
        response
    };
    if response.double_clicked() {
        open(Open::remote(project, repo, branch), actions);
    } else if response.clicked() {
        actions.push(Action::Reveal(branch.target));
    }
    response.context_menu(|ui| remote_branch_menu(ui, project, repo, branch, actions));
}

/// An icon in `icon_color` and a name, indented by `indent`.
fn simple_row(
    ui: &mut Ui,
    icon: Icon,
    text: &str,
    icon_color: egui::Color32,
    text_color: egui::Color32,
    indent: f32,
) -> Response {
    let (rect, response) = row(ui, 24.0);
    paint_row_bg(ui.painter(), rect, &response, false);
    let y = rect.center().y;
    let icon_rect = Rect::from_center_size(pos2(rect.left() + indent + 6.0, y), vec2(12.0, 12.0));
    super::icons::paint(
        ui.painter(),
        icon_rect,
        icon,
        icon_color.gamma_multiply(0.8),
    );
    let x = rect.left() + indent + 18.0;
    let name = elided(
        ui,
        text,
        theme::mono(),
        text_color,
        rect.right() - PAD_X - x,
    );
    paint_galley_left(ui.painter(), x, y, name);
    response
}

/// Says so when the filter hides every row of an open section.
fn no_match(ui: &mut Ui, nothing_shown: bool) {
    if nothing_shown {
        faint_row(ui, "Nothing matches the filter");
    }
}

fn faint_row(ui: &mut Ui, text: &str) {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 22.0), Sense::hover());
    let galley = ui
        .painter()
        .layout_no_wrap(text.to_owned(), theme::small(), color::text_faint());
    paint_galley_left(ui.painter(), rect.left() + 20.0, rect.center().y, galley);
}

fn footer_row(ui: &mut Ui, title: &str, value: &str) -> Response {
    let (rect, response) = row(ui, 24.0);
    let painter = ui.painter();
    paint_row_bg(painter, rect, &response, false);
    let y = rect.center().y;
    section_heading(ui, painter, rect.left() + PAD_X, y, title);
    let value = ui
        .painter()
        .layout_no_wrap(value.to_owned(), theme::small(), color::text_weak());
    let x = rect.right() - PAD_X - value.size().x;
    paint_galley_left(painter, x, y, value);
    response
}
