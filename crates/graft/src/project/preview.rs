//! Preview in main: show a worktree's state in the main worktree, update it,
//! and restore main.

use std::path::{Path, PathBuf};

use graft_core::preview;

use super::Project;
use super::ops::{At, say};

impl Project {
    /// The name the UI uses for the worktree at `path`.
    fn worktree_label(&self, path: &Path) -> String {
        self.snapshot
            .as_ref()
            .and_then(|s| {
                s.worktrees
                    .iter()
                    .find(|wt| graft_core::same_path(&wt.path, path))
            })
            .map(|wt| wt.label())
            .unwrap_or_else(|| graft_core::paths::folder_name(path))
    }

    pub(super) fn preview_in_main(&mut self, worktree: PathBuf) {
        let done = format!(
            "The main folder now shows {}",
            self.worktree_label(&worktree)
        );
        self.run_op(
            At::Main,
            "Preview in main folder",
            move |git| preview::start(&git, &git.at(&worktree)).map(|_| ()),
            say(done),
        );
    }

    pub(super) fn update_preview(&mut self) {
        self.run_op(
            At::Main,
            "Updating the preview",
            |git| preview::update(&git).map(|_| ()),
            say("Preview updated".to_owned()),
        );
    }

    pub(super) fn restore_main(&mut self) {
        self.run_op(
            At::Main,
            "Restoring main",
            |git| preview::restore_main(&git),
            say("Main restored".to_owned()),
        );
    }
}
