//! Preview in main: show a worktree's state in the main worktree, update it,
//! and restore main.

use std::path::{Path, PathBuf};

use graft_core::preview::{self, Changed};

use super::ops::{At, say};
use super::{Action, Confirm, Project};

/// Leaving what main shows now: both throw away main's files as they are.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LeavePreview {
    /// Show the previewed worktree's current state instead.
    Update,
    /// Go back to what main had before the preview.
    Restore,
}

impl Project {
    /// The name the UI uses for the worktree at `path`.
    fn worktree_label(&self, path: &Path) -> String {
        self.snapshot
            .as_ref()
            .and_then(|s| s.worktree_at(path))
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

    /// Update the preview or restore main. Changes made in main since the
    /// preview showed are only thrown away once the user agreed
    /// (`confirmed`): otherwise finding them asks first.
    pub(super) fn leave_preview(&mut self, leave: LeavePreview, confirmed: bool) {
        let (label, done, confirm_label) = match leave {
            LeavePreview::Update => ("Updating the preview", "Preview updated", "Update preview"),
            LeavePreview::Restore => ("Restoring main", "Main restored", "Restore main folder"),
        };
        self.run_op(
            At::Main,
            label,
            move |git| {
                if !confirmed && let Some(changed) = preview::main_changed(&git)? {
                    return Ok(Some(changed));
                }
                match leave {
                    LeavePreview::Update => preview::update(&git).map(|_| ()),
                    LeavePreview::Restore => preview::restore_main(&git),
                }?;
                Ok(None)
            },
            move |p, _, result| match result {
                Ok(None) => p.infos.push(done.to_owned()),
                Ok(Some(changed)) => p.ask(Confirm {
                    title: "Discard changes made in the main folder?".to_owned(),
                    body: format!(
                        "{} Changes made since to tracked files and to files the preview \
                         added are discarded; other new files stay.",
                        match changed {
                            Changed::Head =>
                                "The main folder is on another commit than the preview \
                                 checked out (e.g. a commit was made there): commits made \
                                 in main since are left behind, on no branch.",
                            Changed::Files =>
                                "Files in the main folder changed since the preview, or \
                                 Graft could not read them all to check.",
                        }
                    ),
                    confirm_label: confirm_label.to_owned(),
                    action: Action::LeavePreviewAnyway(leave),
                }),
                Err(e) => p.report(e),
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{git, open, tip, wait};

    /// Main with a worktree `agent` shown in it.
    fn previewing() -> (tempfile::TempDir, std::path::PathBuf, Project) {
        let dir = tempfile::tempdir().unwrap();
        let main = dir.path().join("main");
        std::fs::create_dir_all(&main).unwrap();
        git(&main, &["init", "-q", "-b", "main"]);
        std::fs::write(main.join("a.txt"), "a\n").unwrap();
        git(&main, &["add", "a.txt"]);
        git(&main, &["commit", "-q", "-m", "first"]);
        let agent = dir.path().join("agent");
        let agent_arg = agent.to_str().unwrap();
        git(&main, &["worktree", "add", "-q", "-b", "agent", agent_arg]);
        std::fs::write(agent.join("a.txt"), "agent\n").unwrap();
        let mut project = open(&main);
        let agent = project.snapshot.as_ref().unwrap().worktrees[1].path.clone();
        project.apply(Action::PreviewInMain(agent));
        wait(&mut project, "the preview never showed", |p| {
            p.snapshot.as_ref().is_some_and(|s| s.preview.is_some())
        });
        (dir, main, project)
    }

    /// Main as the preview left it is restored at once.
    #[test]
    fn restoring_an_unchanged_main_asks_nothing() {
        let (_dir, main, mut project) = previewing();
        project.apply(Action::RestoreMain);
        wait(&mut project, "main was never restored", |p| {
            p.snapshot.as_ref().is_some_and(|s| s.preview.is_none())
        });
        assert!(project.confirm.is_none());
        let restored = std::fs::read_to_string(main.join("a.txt")).unwrap();
        assert_eq!(restored.trim_end(), "a");
    }

    /// Committing in main while it shows a preview is refused: Restore main
    /// would leave the commit behind.
    #[test]
    fn a_commit_in_main_is_refused_during_a_preview() {
        let (_dir, main, mut project) = previewing();
        let before = tip(&main, "HEAD");
        git(&main, &["add", "a.txt"]);
        project.refresh();
        wait(&mut project, "the staged file never showed", |p| {
            p.worktree()
                .and_then(|wt| wt.status.as_ref())
                .is_some_and(|s| s.staged().count() == 1)
        });
        project.commit_form.summary = "made in main".to_owned();
        project.apply(Action::Commit);
        assert!(!project.is_operating());
        assert_eq!(project.notices.len(), 1, "{:?}", project.notices);
        assert!(project.notices[0].contains("Restore main first"));
        assert_eq!(tip(&main, "HEAD"), before);
    }

    /// Renaming the branch Restore main goes back to is refused during a
    /// preview (Restore would find no branch); other branches rename.
    #[test]
    fn main_branch_keeps_its_name_during_a_preview() {
        let (_dir, main, mut project) = previewing();
        let snap = project.snapshot.clone().unwrap();
        assert!(snap.rename_blocked("main").is_some());
        assert!(snap.rename_blocked("agent").is_none());
        project.apply(Action::OpenDialog(
            crate::project::DialogKind::RenameBranch {
                old: "main".to_owned(),
                name: "renamed".to_owned(),
            },
        ));
        project.apply(Action::SubmitDialog);
        assert!(!project.is_operating());
        let error = project.dialog.as_ref().and_then(|d| d.error.as_deref());
        assert_eq!(
            error,
            Some("The main folder shows a preview. Restore main first.")
        );
        assert!(!tip(&main, "refs/heads/main").is_empty());
    }

    /// A commit made in main during the preview by other means (git itself)
    /// is not left behind without asking.
    #[test]
    fn a_moved_head_asks_before_restoring() {
        let (_dir, main, mut project) = previewing();
        git(&main, &["commit", "-q", "-am", "made in main"]);
        project.apply(Action::RestoreMain);
        wait(&mut project, "nothing was asked", |p| {
            p.confirm.is_some() && !p.is_operating()
        });
        let (confirm, _) = project.confirm.as_ref().unwrap();
        assert!(confirm.body.contains("left behind"), "{}", confirm.body);
        project.apply(Action::ConfirmNo);
        let preview = project.snapshot.as_ref().unwrap().preview.as_ref();
        assert!(preview.is_some());
    }

    /// An edit made in main during the preview is kept until the user
    /// agrees to lose it.
    #[test]
    fn a_changed_main_asks_before_it_is_discarded() {
        let (_dir, main, mut project) = previewing();
        std::fs::write(main.join("a.txt"), "edited in main\n").unwrap();
        let edited = || {
            std::fs::read_to_string(main.join("a.txt"))
                .unwrap()
                .trim_end()
                .to_owned()
        };
        for (leave, answer) in [
            (Action::UpdatePreview, Action::ConfirmNo),
            (Action::RestoreMain, Action::ConfirmYes),
        ] {
            project.apply(leave);
            wait(&mut project, "nothing was asked", |p| {
                p.confirm.is_some() && !p.is_operating()
            });
            let (confirm, _) = project.confirm.as_ref().unwrap();
            assert!(confirm.body.contains("discarded"), "{}", confirm.body);
            assert_eq!(edited(), "edited in main");
            project.apply(answer);
        }
        wait(&mut project, "main was never restored", |p| {
            p.snapshot.as_ref().is_some_and(|s| s.preview.is_none())
        });
        assert_eq!(edited(), "a");
    }
}
