//! Stashes: loading the list and a stash's files, applying, popping and
//! dropping. Creating a stash and branching from one go through dialogs.

use std::path::PathBuf;

use graft_core::stash::{self, Stash};
use graft_core::{Oid, short};

use super::ops::{At, say};
use super::{Project, readable};

impl Project {
    pub(super) fn load_stashes(&mut self) {
        let git = self.git.clone();
        self.jobs.spawn(
            move || stash::list(&git),
            |p, result| p.on_stashes(readable(result)),
        );
    }

    fn on_stashes(&mut self, result: Result<Vec<Stash>, String>) {
        if let Ok(list) = &result {
            let still_there = self
                .selected_stash
                .is_some_and(|id| list.iter().any(|s| s.id == id));
            if !still_there {
                self.selected_stash = None;
                self.stash_files = None;
                self.stash_diff = None;
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
        self.stash_diff = None;
        let git = self.git.clone();
        self.jobs.spawn(
            move || stash::files(&git, id),
            move |p, result| {
                if p.selected_stash == Some(id) {
                    p.stash_files = Some(readable(result));
                }
            },
        );
    }

    pub(super) fn apply_stash(&mut self, id: Oid, worktree: PathBuf, pop: bool) {
        let done = format!(
            "{} {} in {}",
            if pop { "Popped" } else { "Applied" },
            short(&id),
            graft_core::paths::folder_name(&worktree)
        );
        let label = if pop {
            "Popping stash"
        } else {
            "Applying stash"
        };
        self.run_op(
            At::Path(worktree),
            label,
            move |git| stash::apply(&git, id, pop),
            say(done),
        );
    }

    pub(super) fn drop_stash(&mut self, id: Oid) {
        self.run_op(
            At::Main,
            "Dropping stash",
            move |git| stash::drop(&git, id),
            say(format!("Dropped stash {}", short(&id))),
        );
    }
}
