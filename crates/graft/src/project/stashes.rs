//! Stashes: loading the list and a stash's files, applying, popping and
//! dropping. Creating a stash and branching from one go through dialogs.

use std::path::PathBuf;

use graft_core::stash::{self, Stash};
use graft_core::{Oid, short};

use super::changes::Then;
use super::{Event, Project};

impl Project {
    pub(super) fn load_stashes(&mut self) {
        let git = self.git.clone();
        self.pool.spawn(&self.tx, move || {
            Event::Stashes(stash::list(&git).map_err(|e| format!("{e:#}")))
        });
    }

    pub(super) fn on_stashes(&mut self, result: Result<Vec<Stash>, String>) {
        if let Ok(list) = &result {
            let still_there = self
                .selected_stash
                .is_some_and(|id| list.iter().any(|s| s.id == id));
            if !still_there {
                self.selected_stash = None;
                self.stash_files = None;
                self.commit_diff = None;
                if let Some(first) = list.first() {
                    self.select_stash(first.id);
                }
            }
        }
        self.stashes = Some(result);
    }

    pub(super) fn select_stash(&mut self, id: Oid) {
        if self.selected_stash == Some(id) {
            return;
        }
        self.selected_stash = Some(id);
        self.stash_files = None;
        self.commit_diff = None;
        let git = self.git.clone();
        self.pool.spawn(&self.tx, move || {
            Event::StashFiles(id, stash::files(&git, id).map_err(|e| format!("{e:#}")))
        });
    }

    pub(super) fn apply_stash(&mut self, id: Oid, worktree: PathBuf, pop: bool) {
        let name = worktree
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let done = format!(
            "{} {} in {name}",
            if pop { "Popped" } else { "Applied" },
            short(&id)
        );
        let label = if pop {
            "Popping stash"
        } else {
            "Applying stash"
        };
        self.run_op_at(
            self.git.at(&worktree),
            label,
            Then::Info(done),
            move |git| stash::apply(&git, id, pop).map(|()| None),
        );
    }

    pub(super) fn drop_stash(&mut self, id: Oid) {
        self.run_op_at(
            self.git.clone(),
            "Dropping stash",
            Then::Info(format!("Dropped stash {}", short(&id))),
            move |git| stash::drop(&git, id).map(|()| None),
        );
    }
}
