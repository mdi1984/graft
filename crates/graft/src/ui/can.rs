//! Whether a button can act now, and its hover text: what it does, or why
//! not. For buttons that show up in more than one place.

use graft_core::{RepoSnapshot, Worktree, same_path};

use crate::project::Action;

pub struct Can {
    pub ok: bool,
    pub hint: String,
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
    match (snap.preview.as_ref(), wt) {
        (Some(p), Some(wt)) if same_path(&p.source, &wt.path) => (
            yes("The main folder shows this worktree: show its current state again"),
            Some(Action::UpdatePreview),
        ),
        (Some(_), _) => (
            no("The main folder already shows a preview. Restore it first."),
            None,
        ),
        (None, Some(wt)) if wt.is_main => (
            no("Select another worktree to show its state in the main folder"),
            None,
        ),
        (None, Some(_)) if snap.main_worktree().is_some_and(|m| m.dirty() > 0) => (
            no("The main folder has uncommitted changes. Commit or stash them first."),
            None,
        ),
        (None, Some(wt)) => (
            yes(
                "Check out this worktree's commit plus its uncommitted changes in the main \
                 folder, e.g. to run it from there",
            ),
            Some(Action::PreviewInMain(wt.path.clone())),
        ),
        (None, None) => (no("Select a worktree first"), None),
    }
}
