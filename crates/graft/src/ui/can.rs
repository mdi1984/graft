//! Whether a button can act now, and its hover text: what it does, or why
//! not. For buttons that show up in more than one place.

use std::path::Path;

use egui::Response;
use graft_core::paths::{folder_name, same_path};
use graft_core::preview::{self, Step};
use graft_core::review::Review;
use graft_core::{RepoSnapshot, Worktree};

use crate::project::{Action, Project};

pub const FINISH_FIRST: &str = "Finish or abort what is in progress first";

#[derive(Clone)]
pub struct Can {
    pub ok: bool,
    pub hint: String,
}

impl Can {
    /// `response` of a control drawn as enabled when `self.ok`: when it is
    /// off, hovering says why.
    pub fn why_not(&self, response: Response) -> Response {
        if self.ok {
            response
        } else if response.enabled() {
            // Graft's own buttons stay enabled to egui and only ignore clicks.
            response.on_hover_text(&self.hint)
        } else {
            response.on_disabled_hover_text(&self.hint)
        }
    }

    /// Why not, when it cannot act.
    pub fn blocked(&self) -> Option<&str> {
        (!self.ok).then_some(self.hint.as_str())
    }

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

/// Anything that moves HEAD in the worktree at `path` (commit, check out,
/// reset, cherry-pick, …), see [`Project::head_blocked`].
pub fn moves_head(project: &Project, path: &Path, hint: impl Into<String>) -> Can {
    act(project, hint).unless(project.head_blocked(path))
}

/// A checkout, which always happens in the main folder, see
/// [`Project::checkout_blocked`].
pub fn checkout(project: &Project) -> Can {
    act(project, "").unless(project.checkout_blocked())
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
    if let Some(reason) = snap.and_then(|s| s.head_blocked(&wt.path)) {
        return no(reason);
    }
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
        (Some(_), Some(t)) if let Some(reason) = snap.head_blocked(&t.path) => no(reason),
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

#[cfg(test)]
mod tests {
    use std::process::Command;

    use super::*;
    use crate::testing::{git, open, tip, wait};

    /// A repo on `main` with a branch `other` and a linked worktree `agent`.
    fn repo(dir: &Path) -> std::path::PathBuf {
        let main = dir.join("main");
        std::fs::create_dir_all(&main).unwrap();
        git(&main, &["init", "-q", "-b", "main"]);
        std::fs::write(main.join("a.txt"), "a\n").unwrap();
        git(&main, &["add", "a.txt"]);
        git(&main, &["commit", "-q", "-m", "first"]);
        git(&main, &["switch", "-q", "-c", "other"]);
        std::fs::write(main.join("a.txt"), "other\n").unwrap();
        git(&main, &["commit", "-q", "-am", "other"]);
        git(&main, &["switch", "-q", "main"]);
        let agent = dir.join("agent");
        git(
            &main,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                "agent",
                agent.to_str().unwrap(),
            ],
        );
        main
    }

    /// Mid-merge, a checkout looks disabled and says why; asked anyway
    /// (a double-click), it says the same and does nothing.
    #[test]
    fn no_checkout_while_main_merges() {
        let dir = tempfile::tempdir().unwrap();
        let main = repo(dir.path());
        std::fs::write(main.join("a.txt"), "main\n").unwrap();
        git(&main, &["commit", "-q", "-am", "main"]);
        let merge = Command::new("git")
            .args(["merge", "other"])
            .current_dir(&main)
            .output()
            .unwrap();
        assert!(!merge.status.success(), "the merge must stop");
        let mut project = open(&main);

        let can = checkout(&project);
        assert!(!can.ok);
        assert!(can.hint.contains("Finish or abort"), "{}", can.hint);
        project.apply(Action::Checkout("other".to_owned()));
        assert!(!project.is_operating());
        assert_eq!(project.notices, [can.hint]);
    }

    /// During a preview a checkout is refused even when the project was
    /// opened as a folder spelled differently from git's main worktree.
    #[test]
    fn no_checkout_during_a_preview_whatever_the_spelling() {
        let dir = tempfile::tempdir().unwrap();
        let main = repo(dir.path());
        let mut project = open(&main.join("."));
        let agent = project.snapshot.as_ref().unwrap().worktrees[1].path.clone();
        project.apply(Action::PreviewInMain(agent));
        wait(&mut project, "the preview never showed", |p| {
            p.snapshot.as_ref().is_some_and(|s| s.preview.is_some())
        });
        assert!(!same_path(&project.root, &project.main_path()));
        let before = tip(&main, "HEAD");

        let can = checkout(&project);
        assert!(!can.ok);
        assert!(can.hint.contains("Restore main first"), "{}", can.hint);
        project.apply(Action::Checkout("other".to_owned()));
        assert!(!project.is_operating());
        assert_eq!(project.notices.len(), 1, "{:?}", project.notices);
        assert!(project.notices[0].contains("Restore main first"));
        assert_eq!(tip(&main, "HEAD"), before);
    }
}
