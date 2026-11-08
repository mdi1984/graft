//! Left sidebar: WORKSPACE and WORKTREES, the collapsible BRANCHES, REMOTES
//! and TAGS, and STASHES at the bottom.

use egui::{Id, Rect, Response, ScrollArea, Sense, Stroke, Ui, pos2, vec2};
use graft_core::console::redact;
use graft_core::paths::same_path;
use graft_core::{Branch, RemoteBranch, RepoSnapshot, Upstream, Worktree, paths, remote};

use super::branch_menu::{self, Open, can_item, checkout_color};
use super::can::{self, Can};
use super::icons::Icon;
use super::widgets::{
    self, Badge, ICON_SIZE, elided, elided_path, paint_badges_right, paint_galley_left, paint_icon,
    paint_row_bg, section_heading,
};
use crate::project::{
    Action, CheckedOut, Confirm, DialogKind, Project, RemoteOp, Tab, WorkspaceView, merge_dialog,
};
use crate::text::plural;
use crate::theme::{self, color};
use crate::ui::Actions;

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
    ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 2.0;

            heading(ui, "WORKSPACE", None);
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

            let new_worktree = can::new_worktree(project);
            if let Some(add) = heading(ui, "WORKTREES", Some(&new_worktree))
                && add.clicked()
            {
                actions.push(Action::NewWorktree(None));
            }
            for wt in &repo.worktrees {
                let selected = same_path(&wt.path, &nav.worktree);
                let response = worktree_row(ui, wt, selected);
                if response.clicked() && !wt.prunable {
                    actions.push(Action::OpenWorktree(wt.path.clone()));
                }
                response.context_menu(|ui| worktree_menu(ui, wt, &can::act(project, ""), actions));
            }
            ui.add_space(SECTION_GAP);

            if collapsible(ui, "BRANCHES", repo.branches.len(), true) {
                let head = current.and_then(|wt| wt.branch.as_deref());
                let branches: Vec<&Branch> = repo.branches.iter().collect();
                let is_head = |b: &Branch| head == Some(b.name.as_str());
                grouped(
                    ui,
                    Id::unique("branch-folders"),
                    &branches,
                    &|b| b.name.as_str(),
                    &is_head,
                    Level::top(PAD_X - 4.0),
                    &mut |ui, branch, label, indent| {
                        let response = branch_row(ui, repo, branch, label, indent, head);
                        if response.double_clicked() {
                            open(project, Open::local(project, repo, &branch.name), actions);
                        } else if response.clicked() {
                            actions.push(Action::Reveal(branch.target));
                        }
                        response.context_menu(|ui| branch_menu(ui, project, branch, head, actions));
                    },
                );
                if repo.branches.is_empty() {
                    faint_row(ui, "No branches yet");
                }
                ui.add_space(SECTION_GAP);
            }

            if collapsible(ui, "REMOTES", repo.remotes.len(), false) {
                remotes(ui, project, repo, actions);
                ui.add_space(SECTION_GAP);
            }

            if collapsible(ui, "TAGS", repo.tags.len(), false) {
                for tag in repo.tags.iter().rev() {
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
                }
                ui.add_space(SECTION_GAP);
            }

            if footer_row(ui, "STASHES", &repo.stash_count.to_string()).clicked() {
                actions.push(Action::SetTab(Tab::Stashes));
            }
        });
}

/// Where [`grouped`] is in the folder tree.
#[derive(Clone, Copy)]
struct Level {
    /// Bytes of the name the folders above already show (`feature/`).
    skip: usize,
    /// Left edge of this level's rows.
    indent: f32,
}

impl Level {
    fn top(indent: f32) -> Self {
        Self { skip: 0, indent }
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
        if folder_row(ui, id.with(prefix), folder, n, level.indent, has_mark) {
            let inner = Level {
                skip: prefix.len(),
                indent: level.indent + NEST,
            };
            grouped(ui, id, run, name, marked, inner, leaf);
        }
        i += n;
    }
}

/// A branch folder that folds its branches; open until folded, and it stays
/// the way it was left. Highlighted while folded over the current branch.
fn folder_row(ui: &mut Ui, id: Id, name: &str, count: usize, indent: f32, marked: bool) -> bool {
    let mut open = ui.data_mut(|d| *d.get_persisted_mut_or(id, true));
    let (rect, response) = row(ui, 24.0);
    let painter = ui.painter();
    paint_row_bg(painter, rect, &response, marked && !open);
    let y = rect.center().y;
    let chevron = if open {
        Icon::ChevronDown
    } else {
        Icon::ChevronRight
    };
    let chevron_rect =
        Rect::from_center_size(pos2(rect.left() + indent + 7.0, y), vec2(10.0, 10.0));
    super::icons::paint(painter, chevron_rect, chevron, color::text_faint());
    let folder_rect =
        Rect::from_center_size(pos2(rect.left() + indent + 21.0, y), vec2(12.0, 12.0));
    super::icons::paint(painter, folder_rect, Icon::Folder, color::text_weak());

    let count = painter.layout_no_wrap(count.to_string(), theme::small(), color::text_faint());
    let count_x = rect.right() - PAD_X - count.size().x;
    paint_galley_left(painter, count_x, y, count);
    let x = rect.left() + indent + 32.0;
    let label = elided(
        ui,
        name,
        theme::mono(),
        color::text_mid(),
        count_x - 6.0 - x,
    );
    paint_galley_left(ui.painter(), x, y, label);
    if response.clicked() {
        open = !open;
        ui.data_mut(|d| d.insert_persisted(id, open));
    }
    open
}

fn row(ui: &mut Ui, height: f32) -> (Rect, Response) {
    ui.allocate_exact_size(vec2(ui.available_width(), height), Sense::click())
}

/// A section heading, with a "+" button when `plus` says what it adds (or
/// why it can't now); the button's response takes clicks only when it can.
fn heading(ui: &mut Ui, title: &str, plus: Option<&Can>) -> Option<Response> {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 22.0), Sense::hover());
    section_heading(ui, ui.painter(), rect.left() + 6.0, rect.center().y, title);
    let can = plus?;
    let button =
        Rect::from_center_size(pos2(rect.right() - 12.0, rect.center().y), vec2(22.0, 22.0));
    let sense = if can.ok {
        Sense::click()
    } else {
        Sense::hover()
    };
    let response = ui.interact(button, ui.scope_id().with(title), sense);
    widgets::paint_icon_button_when(ui, button, &response, Icon::Plus, can.ok);
    Some(response.on_hover_text(&can.hint))
}

/// A heading that folds its section; the open state survives restarts.
fn collapsible(ui: &mut Ui, title: &str, count: usize, default_open: bool) -> bool {
    let id = Id::unique(("sidebar-section", title));
    let mut open = ui.data_mut(|d| *d.get_persisted_mut_or(id, default_open));
    let (rect, response) = row(ui, 22.0);
    if response.hovered() {
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
    let count = ui
        .painter()
        .layout_no_wrap(count.to_string(), theme::small(), color::text_faint());
    paint_galley_left(
        ui.painter(),
        rect.right() - PAD_X - count.size().x,
        y,
        count,
    );
    if response.clicked() {
        open = !open;
        ui.data_mut(|d| d.insert_persisted(id, open));
    }
    open
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
    can_item(
        ui,
        "New branch from here…",
        &act,
        actions,
        Action::OpenDialog(DialogKind::new_branch(name, name)),
    );
    can_item(
        ui,
        "Rename…",
        &act,
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
    if let Some(remote) = remote::default_remote(repo.remotes.iter().map(|r| r.name.as_str())) {
        can_item(
            ui,
            &format!("Push to {remote}"),
            can,
            actions,
            Action::Remote(RemoteOp::PushTag {
                remote: remote.to_owned(),
                tag: tag.to_owned(),
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

/// Double-click on a branch: what its menu offers first, if it can start.
fn open(project: &Project, open: Option<Open>, actions: &mut Actions) {
    if let Some(open) = open.filter(|o| o.can(project).ok) {
        actions.push(open.action());
    }
}

fn remotes(ui: &mut Ui, project: &Project, repo: &RepoSnapshot, actions: &mut Actions) {
    let can_add = can::add_remote(project);
    // Only shown when it can't: why.
    let can_change = can::act(project, "");
    let add = Action::OpenDialog(DialogKind::add_remote(repo.remotes.is_empty()));
    for remote in &repo.remotes {
        simple_row(
            ui,
            Icon::Cloud,
            &remote.name,
            color::text_mid(),
            color::text(),
            8.0,
        )
        .on_hover_text(graft_core::console::redact(&remote.fetch_url))
        .context_menu(|ui| {
            can_item(
                ui,
                "Edit…",
                &can_change,
                actions,
                Action::OpenDialog(DialogKind::edit_remote(&remote.name, &remote.fetch_url)),
            );
            if ui.button("Copy URL").clicked() {
                ui.ctx().copy_text(remote.fetch_url.clone());
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
                        redact(&remote.fetch_url)
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
            .filter(|b| b.remote == remote.name)
            .collect();
        grouped(
            ui,
            Id::unique(("remote-folders", &remote.name)),
            &branches,
            &|b| b.branch_name(),
            &|_| false,
            Level::top(22.0),
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
        open(project, Open::remote(project, repo, branch), actions);
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
