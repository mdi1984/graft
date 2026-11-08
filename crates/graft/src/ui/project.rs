//! One open project on screen: its dialogs, the toolbar, the tabs, the
//! sidebar and the main area.

use egui::{CentralPanel, Frame, Margin, Panel, Ui};

use super::Actions;
use crate::project::Project;
use crate::theme::color;

/// Show `projects[index]`; the toolbar's search looks in every project.
pub fn show(ui: &mut Ui, projects: &mut [Project], index: usize, search: &mut String) -> Actions {
    let mut actions = Actions::default();
    let ctx = ui.ctx().clone();
    let project = &mut projects[index];
    if let Some(confirm) = project.confirm() {
        super::confirm::show(&ctx, confirm, &mut actions);
    }
    if let (Some(form), Some(snap)) = (&mut project.new_worktree, &project.snapshot) {
        super::new_worktree::show(&ctx, form, snap, &mut actions);
    }
    if let Some(dialog) = &mut project.dialog {
        super::dialog::show(&ctx, dialog, &mut actions);
    }
    if let Some(view) = &project.slash {
        let progress = project.op_progress().filter(|p| !p.line.is_empty());
        let slashed = &project.memory().slashed;
        super::slash::show(&ctx, view, slashed, progress, &mut actions);
    }
    let bar = |margin| Frame::new().fill(color::bar_bg()).inner_margin(margin);

    Panel::top("toolbar")
        .frame(bar(Margin::symmetric(12, 8)))
        .show(ui, |ui| {
            super::toolbar::show(ui, projects, index, &mut actions, search)
        });
    let project = &projects[index];
    Panel::top("tabs")
        .frame(bar(Margin::symmetric(12, 0)))
        .show(ui, |ui| super::tabs::show(ui, project, &mut actions));
    if project.snapshot.is_some() {
        Panel::left("sidebar")
            .resizable(true)
            .default_size(260.0)
            .size_range(200.0..=440.0)
            .frame(
                Frame::new()
                    .fill(color::sidebar_bg())
                    .inner_margin(Margin::symmetric(8, 12)),
            )
            .show(ui, |ui| super::sidebar::show(ui, project, &mut actions));
    }
    let project = &mut projects[index];
    CentralPanel::no_frame()
        .frame(Frame::new().fill(color::app_bg()))
        .show(ui, |ui| super::main_area::show(ui, project, &mut actions));
    actions
}
