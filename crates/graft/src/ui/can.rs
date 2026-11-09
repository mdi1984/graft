//! Whether a button can act now, and its hover text: what it does, or why
//! not. For buttons that show up in more than one place.

use std::path::Path;

use egui::Response;
use graft_core::paths::{folder_name, same_path};
use graft_core::preview::{self, Step};
use graft_core::review::Review;
use graft_core::{RepoSnapshot, Upstream, Worktree, remote};

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

/// Pulling the local branch `branch` (the toolbar for the selected
/// worktree's, a branch's menu for any): where it is checked out, a merge
/// or rebase as configured; where it isn't, only a fast-forward.
pub fn pull(snap: &RepoSnapshot, branch: &str, selected: &Path) -> Can {
    if snap.remotes.is_empty() {
        return no("No remote to pull from. Add one under Remotes in the sidebar.");
    }
    let Some(local) = snap.branch(branch) else {
        return no("Check out a branch to pull from");
    };
    let upstream = match remote::pull_check(branch, &local.upstream) {
        Ok(name) => name,
        Err(why) => return no(why),
    };
    match snap.worktree_for_branch(branch) {
        Some(wt) if wt.prunable => no(format!(
            "{branch} is checked out in a worktree whose folder is missing. \
             Prune missing worktrees first."
        )),
        Some(wt) if let Some(reason) = snap.head_blocked(&wt.path) => no(reason),
        Some(wt) if wt.in_progress.is_some() => no(FINISH_FIRST),
        Some(wt) if same_path(&wt.path, selected) => yes(format!("Pull {upstream} into {branch}")),
        Some(wt) => yes(format!(
            "Pull {upstream} into {branch}, in its worktree {}",
            folder_name(&wt.path)
        )),
        None => match local.upstream {
            Upstream::Tracking { ahead, .. } if ahead > 0 => no(format!(
                "{branch} has commits that {upstream} lacks, so it can only be pulled \
                 where it is checked out (a merge or rebase)"
            )),
            _ => yes(format!(
                "Move {branch} ahead to {upstream}'s newest commit (it is not checked \
                 out, so only a fast-forward)"
            )),
        },
    }
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
    use super::*;
    use crate::testing::{git, merge_stops, open, tip, wait};

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
        merge_stops(&main, "other");
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
    /// Pull in a branch's menu: where it can run, and why not.
    #[test]
    fn pull_rules() {
        let dir = tempfile::tempdir().unwrap();
        let main = repo(dir.path());
        git(
            dir.path(),
            &["init", "-q", "--bare", "-b", "main", "origin.git"],
        );
        let url = dir.path().join("origin.git");
        git(&main, &["remote", "add", "origin", url.to_str().unwrap()]);
        for branch in ["main", "other", "agent"] {
            git(&main, &["push", "-q", "-u", "origin", branch]);
        }
        // `other` gets a commit origin lacks; `gone` loses its folder.
        git(&main, &["branch", "-q", "-f", "behind", "main"]);
        git(&main, &["push", "-q", "-u", "origin", "behind"]);
        let gone = dir.path().join("gone");
        git(
            &main,
            &["worktree", "add", "-q", gone.to_str().unwrap(), "behind"],
        );
        std::fs::remove_dir_all(&gone).unwrap();
        git(&main, &["switch", "-q", "other"]);
        std::fs::write(
            main.join("o.txt"),
            "o
",
        )
        .unwrap();
        git(&main, &["add", "o.txt"]);
        git(&main, &["commit", "-q", "-m", "more"]);
        git(&main, &["switch", "-q", "main"]);
        git(&main, &["branch", "-q", "local-only"]);
        let project = open(&main);
        let snap = project.snapshot.clone().unwrap();
        let here = project.nav.worktree.clone();
        let hint = |branch: &str| {
            let can = pull(&snap, branch, &here);
            (can.ok, can.hint)
        };

        assert_eq!(
            hint("main"),
            (true, "Pull origin/main into main".to_owned())
        );
        let (ok, text) = hint("agent");
        assert!(ok && text.ends_with("in its worktree agent"), "{text}");
        let (ok, text) = hint("other");
        assert!(
            !ok && text.contains("commits that origin/other lacks"),
            "{text}"
        );
        let (ok, text) = hint("behind");
        assert!(!ok && text.contains("folder is missing"), "{text}");
        let (ok, text) = hint("local-only");
        assert!(!ok && text.contains("no upstream"), "{text}");
        git(&main, &["worktree", "prune"]);
        let project = open(&main);
        let snap = project.snapshot.clone().unwrap();
        let can = pull(&snap, "behind", &here);
        assert!(can.ok && can.hint.contains("fast-forward"), "{}", can.hint);
    }
}
