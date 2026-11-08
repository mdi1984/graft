//! Whether a button can act now, and its hover text: what it does, or why
//! not. For buttons that show up in more than one place.

use graft_core::paths::{folder_name, same_path};
use graft_core::preview::{self, Step};
use graft_core::review::Review;
use graft_core::{RepoSnapshot, Worktree};

use crate::project::{Action, Project};

pub const FINISH_FIRST: &str = "Finish or abort what is in progress first";

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

/// Anything that starts an operation, with `hint` saying what it does:
/// not while the repository loads or another operation runs.
pub fn act(project: &Project, hint: impl Into<String>) -> Can {
    yes(hint).unless(project.can_act().err())
}

/// A new worktree (toolbar, the sidebar's "+", a branch's menu).
pub fn new_worktree(project: &Project) -> Can {
    act(project, "Check out a branch in a second folder")
}

/// Adding a remote (the sidebar's REMOTES).
pub fn add_remote(project: &Project) -> Can {
    act(project, "Add a remote to fetch from and push to")
}

/// Merging another branch into `wt`'s branch, or rebasing it.
pub fn merge(snap: Option<&RepoSnapshot>, wt: Option<&Worktree>) -> Can {
    let Some(wt) = wt else {
        return no("Select a worktree first");
    };
    let Some(branch) = wt.branch.as_deref() else {
        return no("Check out a branch to merge into");
    };
    if wt.in_progress.is_some() {
        return no(FINISH_FIRST);
    }
    if snap.is_some_and(|s| s.merge_choices(branch).next().is_some()) {
        yes(format!("Merge another branch into {branch}"))
    } else {
        no("There is no other branch to merge")
    }
}

/// Merging the reviewed branch of `wt` into the review's base, in the
/// worktree that has the base checked out. Returns that worktree, too.
pub fn merge_reviewed<'a>(
    snap: &'a RepoSnapshot,
    wt: &Worktree,
    review: &Review,
) -> (Can, Option<&'a Worktree>) {
    let base = review.base.as_str();
    let target = snap.worktree_for_branch(base);
    let can = match (&wt.branch, target) {
        (None, _) => no("Check out a branch to merge it"),
        _ if review.commits == 0 => {
            no("No commits to merge yet; uncommitted changes are not merged")
        }
        (Some(_), None) => no(format!(
            "{base} is not checked out in any worktree; check it out to merge into it"
        )),
        (Some(_), Some(t)) if same_path(&t.path, &wt.path) => {
            no("This worktree is the base itself")
        }
        (Some(_), Some(t)) if t.in_progress.is_some() => {
            no(format!("{base} has a merge or rebase in progress"))
        }
        (Some(branch), Some(t)) => yes(format!(
            "Run git merge {branch} in {}",
            folder_name(&t.path)
        )),
    };
    (can, target)
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
