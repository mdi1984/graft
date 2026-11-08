//! Whether a button can act now, and its hover text: what it does, or why
//! not. For buttons that show up in more than one place.

use graft_core::preview::{self, Step};
use graft_core::{RepoSnapshot, Worktree};

use crate::project::Action;

pub struct Can {
    pub ok: bool,
    pub hint: String,
}

impl Can {
    /// Not while `blocked` says why nothing can start.
    pub fn unless(self, blocked: Option<&str>) -> Self {
        match blocked {
            Some(why) if self.ok => no(why),
            _ => self,
        }
    }
}

pub fn yes(hint: impl Into<String>) -> Can {
    Can {
        ok: true,
        hint: hint.into(),
    }
}

pub fn no(hint: impl Into<String>) -> Can {
    Can {
        ok: false,
        hint: hint.into(),
    }
}

/// "Preview in main folder" for `wt`, and what it does: a first preview, or
/// showing the previewed worktree's current state again.
pub fn preview(snap: &RepoSnapshot, wt: Option<&Worktree>) -> (Can, Option<Action>) {
    let Some(wt) = wt else {
        return (no("Select a worktree first"), None);
    };
    match preview::check(snap, wt) {
        Ok(Step::Update) => (
            yes("The main folder shows this worktree: show its current state again"),
            Some(Action::UpdatePreview),
        ),
        Ok(Step::Start) => (
            yes(
                "Check out this worktree's commit plus its uncommitted changes in the main folder, e.g. to run it from there",
            ),
            Some(Action::PreviewInMain(wt.path.clone())),
        ),
        Err(blocked) => (no(blocked.message()), None),
    }
}
