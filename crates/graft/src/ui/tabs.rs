use egui::Ui;
use graft_core::RepoSnapshot;

use super::widgets::{self, Badge};
use crate::app::{Nav, Tab};

pub fn show(ui: &mut Ui, repo: &RepoSnapshot, nav: &mut Nav) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        let stashes = (repo.stash_count > 0)
            .then(|| Badge::new(ui, repo.stash_count.to_string(), widgets::NEUTRAL));
        let tabs = [
            (Tab::Workspace, "Workspace", None),
            (Tab::Review, "Review", None),
            (Tab::Stashes, "Stashes", stashes),
        ];
        for (tab, label, badge) in tabs {
            if widgets::tab(ui, label, badge, nav.tab == tab).clicked() {
                nav.tab = tab;
            }
        }
    });
}
