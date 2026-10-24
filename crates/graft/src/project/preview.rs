//! Preview in main: show a worktree's state in the main worktree, update it,
//! and restore main.

use std::path::PathBuf;

use graft_core::preview;

use super::changes::Then;
use super::{Action, Project};

impl Project {
    /// The name the UI uses for the worktree at `path`.
    fn worktree_label(&self, path: &std::path::Path) -> String {
        self.snapshot
            .as_ref()
            .and_then(|s| {
                s.worktrees
                    .iter()
                    .find(|wt| graft_core::same_path(&wt.path, path))
            })
            .map(|wt| wt.label())
            .unwrap_or_else(|| {
                path.file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default()
            })
    }

    fn preview_in_main(&mut self, worktree: PathBuf) {
        let done = format!(
            "The main folder now shows {}",
            self.worktree_label(&worktree)
        );
        self.run_op_at(
            self.git.clone(),
            "Preview in main folder",
            Then::Info(done),
            move |git| {
                let source = git.at(&worktree);
                preview::start(&git, &source).map(|_| None)
            },
        );
    }

    pub(super) fn apply_preview_action(&mut self, action: Action) {
        match action {
            Action::PreviewInMain(worktree) => self.preview_in_main(worktree),
            Action::UpdatePreview => self.run_op_at(
                self.git.clone(),
                "Updating the preview",
                Then::Info("Preview updated".to_owned()),
                |git| preview::update(&git).map(|_| None),
            ),
            Action::RestoreMain => self.run_op_at(
                self.git.clone(),
                "Restoring main",
                Then::Info("Main restored".to_owned()),
                |git| preview::restore_main(&git).map(|()| None),
            ),
            _ => {}
        }
    }
}
