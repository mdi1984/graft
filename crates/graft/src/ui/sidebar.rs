//! Left sidebar: WORKSPACE, WORKTREES, BRANCHES, then collapsible REMOTES and
//! TAGS, and STASHES.

use egui::{Id, Rect, Response, ScrollArea, Sense, Ui, pos2, vec2};
use graft_core::{Branch, RemoteBranch, RepoSnapshot, Upstream, Worktree, same_path};

use super::icons::Icon;
use super::widgets::{
    self, Badge, ICON_SIZE, elided, elided_path, paint_badges_right, paint_galley_left, paint_icon,
    paint_row_bg, section_heading,
};
use crate::project::{Action, Confirm, DialogKind, Project, RemoteOp, Tab, WorkspaceView};
use crate::theme::{self, color};

const SECTION_GAP: f32 = 14.0;
const PAD_X: f32 = 8.0;

pub fn show(ui: &mut Ui, project: &Project, actions: &mut Vec<Action>) {
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
                (dirty > 0).then(|| Badge::new(ui, dirty.to_string(), widgets::DIRTY));
            let status_selected = in_workspace && nav.view == WorkspaceView::FileStatus;
            if nav_row(
                ui,
                Icon::FileStatus,
                "File status",
                status_selected,
                dirty_badge,
            )
            .clicked()
            {
                actions.push(Action::ShowFileStatus);
            }
            let history_selected = in_workspace && nav.view == WorkspaceView::History;
            if nav_row(ui, Icon::History, "History", history_selected, None).clicked() {
                actions.push(Action::ShowHistory);
            }
            ui.add_space(SECTION_GAP);

            if let Some(add) = heading(ui, "WORKTREES", Some(repo.worktrees.len()))
                && add.on_hover_text("New worktree").clicked()
                && !project.is_operating()
            {
                actions.push(Action::NewWorktree(None));
            }
            for wt in &repo.worktrees {
                let selected = same_path(&wt.path, &nav.worktree);
                let response = worktree_row(ui, wt, selected);
                if response.clicked() && !wt.prunable {
                    actions.push(Action::OpenWorktree(wt.path.clone()));
                }
                response.context_menu(|ui| worktree_menu(ui, wt, project.is_operating(), actions));
            }
            ui.add_space(SECTION_GAP);

            if collapsible(ui, "BRANCHES", repo.branches.len(), true) {
                let head = current.and_then(|wt| wt.branch.as_deref());
                for branch in &repo.branches {
                    let response = branch_row(ui, repo, branch, head);
                    if response.clicked() {
                        actions.push(Action::Reveal(branch.target));
                    }
                    response.context_menu(|ui| {
                        branch_menu(ui, repo, branch, head, project.is_operating(), actions)
                    });
                }
                if repo.branches.is_empty() {
                    faint_row(ui, "No branches yet");
                }
                ui.add_space(SECTION_GAP);
            }

            if collapsible(ui, "REMOTES", repo.remotes.len(), false) {
                remotes(ui, repo, project.is_operating(), actions);
                ui.add_space(SECTION_GAP);
            }

            if collapsible(ui, "TAGS", repo.tags.len(), false) {
                for tag in repo.tags.iter().rev() {
                    let response = simple_row(ui, Icon::Tag, &tag.name, color::TAG, 20.0);
                    if response.clicked() {
                        actions.push(Action::Reveal(tag.target));
                    }
                    response.context_menu(|ui| {
                        tag_menu(ui, repo, &tag.name, project.is_operating(), actions)
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

fn row(ui: &mut Ui, height: f32) -> (Rect, Response) {
    ui.allocate_exact_size(vec2(ui.available_width(), height), Sense::click())
}

/// Section heading with a count; returns the "+" button's response for WORKTREES.
fn heading(ui: &mut Ui, title: &str, count: Option<usize>) -> Option<Response> {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 22.0), Sense::hover());
    section_heading(ui, ui.painter(), rect.left() + 6.0, rect.center().y, title);
    count?;
    let button =
        Rect::from_center_size(pos2(rect.right() - 12.0, rect.center().y), vec2(22.0, 22.0));
    let response = ui.interact(button, ui.scope_id().with(title), Sense::click());
    widgets::paint_icon_button(ui, button, &response, Icon::Plus);
    Some(response)
}

/// A heading that folds its section; the open state survives restarts.
fn collapsible(ui: &mut Ui, title: &str, count: usize, default_open: bool) -> bool {
    let id = Id::unique(("sidebar-section", title));
    let mut open = ui.data_mut(|d| *d.get_persisted_mut_or(id, default_open));
    let (rect, response) = row(ui, 22.0);
    if response.hovered() {
        ui.painter().rect_filled(rect, 4, color::HOVER_ROW_BG);
    }
    let y = rect.center().y;
    let chevron = if open {
        Icon::ChevronDown
    } else {
        Icon::ChevronRight
    };
    let icon_rect = Rect::from_center_size(pos2(rect.left() + 10.0, y), vec2(10.0, 10.0));
    super::icons::paint(ui.painter(), icon_rect, chevron, color::TEXT_FAINT);
    section_heading(ui, ui.painter(), rect.left() + 20.0, y, title);
    let count = ui
        .painter()
        .layout_no_wrap(count.to_string(), theme::small(), color::TEXT_FAINT);
    paint_galley_left(
        ui.painter(),
        rect.right() - PAD_X - count.size().x,
        y,
        count,
        color::TEXT_FAINT,
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
    paint_icon(painter, pos2(rect.left() + PAD_X, y), icon, color::TEXT);
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
        color::TEXT,
        badges_x - 6.0 - text_x,
    );
    paint_galley_left(painter, text_x, y, text, color::TEXT);
    response
}

fn worktree_row(ui: &mut Ui, wt: &Worktree, selected: bool) -> Response {
    let (rect, response) = row(ui, 42.0);
    let painter = ui.painter();
    paint_row_bg(painter, rect, &response, selected);

    let top_y = rect.top() + 14.0;
    let bottom_y = rect.top() + 29.0;
    let icon_color = if wt.prunable {
        color::TEXT_FAINT
    } else if selected {
        color::ACCENT
    } else {
        color::TEXT_WEAK
    };
    paint_icon(
        painter,
        pos2(rect.left() + PAD_X, top_y),
        Icon::Folder,
        icon_color,
    );

    let mut badges = Vec::new();
    if wt.dirty() > 0 {
        badges.push(Badge::new(ui, wt.dirty().to_string(), widgets::DIRTY));
    }
    if wt.ahead() > 0 {
        badges.push(Badge::new(ui, format!("↑{}", wt.ahead()), widgets::AHEAD));
    }
    if wt.behind() > 0 {
        badges.push(Badge::new(
            ui,
            format!("↓{}", wt.behind()),
            widgets::NEUTRAL,
        ));
    }
    if wt.prunable {
        badges.push(Badge::new(ui, "missing", widgets::NEUTRAL));
    } else if wt.locked {
        badges.push(Badge::new(ui, "locked", widgets::NEUTRAL));
    }
    let badges_x = paint_badges_right(painter, rect.right() - PAD_X, top_y, badges);

    let text_x = rect.left() + PAD_X + ICON_SIZE + 8.0;
    let fg = if wt.prunable {
        color::TEXT_FAINT
    } else {
        color::TEXT
    };
    let branch = elided(ui, &wt.label(), theme::mono(), fg, badges_x - 6.0 - text_x);
    paint_galley_left(painter, text_x, top_y, branch, fg);
    let path = if wt.is_main {
        format!("{}  · main worktree", wt.path.display())
    } else {
        wt.path.display().to_string()
    };
    let path = elided_path(
        ui,
        &path,
        theme::small(),
        color::TEXT_WEAK,
        rect.right() - PAD_X - text_x,
    );
    paint_galley_left(painter, text_x, bottom_y, path, color::TEXT_WEAK);

    response.on_hover_text(wt.path.display().to_string())
}

fn worktree_menu(ui: &mut Ui, wt: &Worktree, busy: bool, actions: &mut Vec<Action>) {
    if wt.prunable {
        if ui
            .add_enabled(!busy, egui::Button::new("Prune missing worktrees"))
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
        ui.ctx().copy_text(wt.path.display().to_string());
        ui.close();
    }
    if wt.is_main {
        return;
    }
    ui.separator();
    let lock_label = if wt.locked { "Unlock" } else { "Lock" };
    if ui
        .add_enabled(!busy, egui::Button::new(lock_label))
        .on_hover_text("A locked worktree is never removed or pruned")
        .clicked()
    {
        actions.push(Action::LockWorktree {
            path: wt.path.clone(),
            locked: !wt.locked,
        });
        ui.close();
    }
    let remove = ui.add_enabled(!busy && !wt.locked, egui::Button::new("Remove worktree…"));
    let remove = if wt.locked {
        remove.on_disabled_hover_text("Unlock it first")
    } else {
        remove
    };
    if remove.clicked() {
        actions.push(Action::Ask(Box::new(remove_confirm(wt))));
        ui.close();
    }
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
            "The folder {} will be deleted, including {dirty} uncommitted change{}, which will be lost. {branch}",
            wt.path.display(),
            if dirty == 1 { "" } else { "s" }
        )
    } else {
        format!("The folder {} will be deleted. {branch}", wt.path.display())
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

fn menu_item(ui: &mut Ui, label: &str, enabled: bool, actions: &mut Vec<Action>, action: Action) {
    if ui.add_enabled(enabled, egui::Button::new(label)).clicked() {
        actions.push(action);
        ui.close();
    }
}

fn branch_menu(
    ui: &mut Ui,
    repo: &RepoSnapshot,
    branch: &Branch,
    head: Option<&str>,
    busy: bool,
    actions: &mut Vec<Action>,
) {
    let name = branch.name.as_str();
    let is_head = head == Some(name);
    let elsewhere = repo.worktree_for_branch(name).filter(|_| !is_head);
    match elsewhere {
        Some(wt) => menu_item(
            ui,
            "Open its worktree",
            true,
            actions,
            Action::OpenWorktree(wt.path.clone()),
        ),
        None if !is_head => {
            menu_item(
                ui,
                &format!("Check out {name}"),
                !busy,
                actions,
                Action::Checkout(name.to_owned()),
            );
            menu_item(
                ui,
                "New worktree for this branch…",
                !busy,
                actions,
                Action::NewWorktree(Some(name.to_owned())),
            );
        }
        None => {}
    }
    menu_item(
        ui,
        "New branch from here…",
        !busy,
        actions,
        Action::OpenDialog(DialogKind::NewBranch {
            name: String::new(),
            start: name.to_owned(),
            start_label: name.to_owned(),
            checkout: true,
        }),
    );
    menu_item(
        ui,
        "Rename…",
        !busy,
        actions,
        Action::OpenDialog(DialogKind::RenameBranch {
            old: name.to_owned(),
            name: name.to_owned(),
        }),
    );
    let deletable = !is_head && elsewhere.is_none();
    let delete = ui.add_enabled(!busy && deletable, egui::Button::new("Delete…"));
    let delete = if deletable {
        delete
    } else {
        delete.on_disabled_hover_text("It is checked out in a worktree")
    };
    if delete.clicked() {
        actions.push(Action::Ask(Box::new(Confirm {
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
        })));
        ui.close();
    }
    ui.separator();
    if ui.button("Copy name").clicked() {
        ui.ctx().copy_text(name.to_owned());
        ui.close();
    }
}

fn remote_branch_menu(
    ui: &mut Ui,
    repo: &RepoSnapshot,
    branch: &RemoteBranch,
    busy: bool,
    actions: &mut Vec<Action>,
) {
    let local = branch.branch_name();
    match repo.branch(local) {
        Some(_) if repo.worktree_for_branch(local).is_none() => menu_item(
            ui,
            &format!("Check out {local}"),
            !busy,
            actions,
            Action::Checkout(local.to_owned()),
        ),
        Some(_) => {}
        None => {
            menu_item(
                ui,
                &format!("Check out as {local}"),
                !busy,
                actions,
                Action::CheckoutRemote {
                    local: local.to_owned(),
                    remote: branch.name.clone(),
                },
            );
            menu_item(
                ui,
                "New worktree for this branch…",
                !busy,
                actions,
                Action::NewWorktree(Some(branch.name.clone())),
            );
        }
    }
    menu_item(
        ui,
        "New branch from here…",
        !busy,
        actions,
        Action::OpenDialog(DialogKind::NewBranch {
            name: String::new(),
            start: branch.name.clone(),
            start_label: branch.name.clone(),
            checkout: true,
        }),
    );
    menu_item(
        ui,
        &format!("Delete on {}…", branch.remote),
        !busy,
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

fn tag_menu(ui: &mut Ui, repo: &RepoSnapshot, tag: &str, busy: bool, actions: &mut Vec<Action>) {
    let remote = repo
        .remotes
        .iter()
        .find(|r| r.name == "origin")
        .or(repo.remotes.first());
    if let Some(remote) = remote {
        menu_item(
            ui,
            &format!("Push to {}", remote.name),
            !busy,
            actions,
            Action::Remote(RemoteOp::PushTag {
                remote: remote.name.clone(),
                tag: tag.to_owned(),
            }),
        );
    }
    menu_item(
        ui,
        "Delete…",
        !busy,
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

fn branch_row(ui: &mut Ui, repo: &RepoSnapshot, branch: &Branch, head: Option<&str>) -> Response {
    let (rect, response) = row(ui, 26.0);
    let painter = ui.painter();
    paint_row_bg(painter, rect, &response, false);
    let y = rect.center().y;

    let is_head = head == Some(branch.name.as_str());
    let elsewhere = !is_head && repo.worktree_for_branch(&branch.name).is_some();
    let gone = matches!(branch.upstream, Upstream::Gone { .. });
    let dot = if is_head {
        color::ACCENT
    } else if gone {
        color::TEXT_FAINT
    } else if elsewhere {
        color::BRANCH_DOT_AGENT
    } else {
        color::AHEAD_FG
    };
    painter.circle_filled(pos2(rect.left() + PAD_X + 3.0, y), 3.0, dot);

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
        .layout_no_wrap(meta, theme::small(), color::TEXT_WEAK);
    let meta_x = rect.right() - PAD_X - meta.size().x;
    paint_galley_left(painter, meta_x, y, meta, color::TEXT_WEAK);

    let text_x = rect.left() + PAD_X + 14.0;
    let fg = if is_head { color::ACCENT } else { color::TEXT };
    let name = elided(ui, &branch.name, theme::mono(), fg, meta_x - 6.0 - text_x);
    paint_galley_left(painter, text_x, y, name, fg);

    let tip = match &branch.upstream {
        Upstream::None => "Not pushed anywhere yet".to_owned(),
        Upstream::Gone { name } => format!("Upstream {name} was deleted"),
        Upstream::Tracking {
            name,
            ahead,
            behind,
        } => format!("Tracks {name} · {ahead} ahead, {behind} behind"),
    };
    let tip = if elsewhere {
        let wt = repo.worktree_for_branch(&branch.name).expect("checked");
        format!("{tip}\nChecked out in {}", wt.path.display())
    } else {
        tip
    };
    response.on_hover_text(tip)
}

fn remotes(ui: &mut Ui, repo: &RepoSnapshot, busy: bool, actions: &mut Vec<Action>) {
    if repo.remotes.is_empty() {
        faint_row(ui, "No remotes");
        return;
    }
    for remote in &repo.remotes {
        simple_row(ui, Icon::Cloud, &remote.name, color::TEXT_MID, 8.0)
            .on_hover_text(&remote.fetch_url);
        for branch in repo
            .remote_branches
            .iter()
            .filter(|b| b.remote == remote.name)
        {
            let response = simple_row(
                ui,
                Icon::ChevronRight,
                branch.branch_name(),
                color::TEXT_WEAK,
                26.0,
            );
            if response.clicked() {
                actions.push(Action::Reveal(branch.target));
            }
            response.context_menu(|ui| remote_branch_menu(ui, repo, branch, busy, actions));
        }
    }
}

/// An icon and a name, indented by `indent`.
fn simple_row(ui: &mut Ui, icon: Icon, text: &str, fg: egui::Color32, indent: f32) -> Response {
    let (rect, response) = row(ui, 24.0);
    paint_row_bg(ui.painter(), rect, &response, false);
    let y = rect.center().y;
    let icon_rect = Rect::from_center_size(pos2(rect.left() + indent + 6.0, y), vec2(12.0, 12.0));
    super::icons::paint(ui.painter(), icon_rect, icon, fg.gamma_multiply(0.8));
    let x = rect.left() + indent + 18.0;
    let name = elided(
        ui,
        text,
        theme::mono(),
        color::TEXT,
        rect.right() - PAD_X - x,
    );
    paint_galley_left(ui.painter(), x, y, name, color::TEXT);
    response
}

fn faint_row(ui: &mut Ui, text: &str) {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 22.0), Sense::hover());
    let galley = ui
        .painter()
        .layout_no_wrap(text.to_owned(), theme::small(), color::TEXT_FAINT);
    paint_galley_left(
        ui.painter(),
        rect.left() + 20.0,
        rect.center().y,
        galley,
        color::TEXT_FAINT,
    );
}

fn footer_row(ui: &mut Ui, title: &str, value: &str) -> Response {
    let (rect, response) = row(ui, 24.0);
    let painter = ui.painter();
    paint_row_bg(painter, rect, &response, false);
    let y = rect.center().y;
    section_heading(ui, painter, rect.left() + PAD_X, y, title);
    let value = ui
        .painter()
        .layout_no_wrap(value.to_owned(), theme::small(), color::TEXT_WEAK);
    let x = rect.right() - PAD_X - value.size().x;
    paint_galley_left(painter, x, y, value, color::TEXT_WEAK);
    response
}
