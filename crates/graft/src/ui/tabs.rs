//! The view tabs under the toolbar: Workspace, Review, Stashes.

use egui::Ui;

use super::widgets::{self, Badge};
use crate::project::{Action, Project, Tab};
use crate::ui::Actions;

pub fn show(ui: &mut Ui, project: &Project, actions: &mut Actions) {
    let stash_count = project.snapshot.as_ref().map_or(0, |s| s.stash_count);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        let stashes =
            (stash_count > 0).then(|| Badge::new(ui, stash_count.to_string(), widgets::neutral()));
        let tabs = [
            (Tab::Workspace, None),
            (Tab::Review, None),
            (Tab::Stashes, stashes),
        ];
        for (tab, badge) in tabs {
            if widgets::tab(ui, tab.label(), badge, project.nav.tab == tab).clicked() {
                actions.push(Action::SetTab(tab));
            }
        }
    });
}
