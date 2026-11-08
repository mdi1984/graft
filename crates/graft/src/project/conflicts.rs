//! Merges and conflicts: merging and rebasing, the resolver for the selected
//! conflicted file, and Continue / Abort for whatever stopped part-way.

use std::path::PathBuf;
use std::sync::Arc;

use graft_core::conflicts::{self, ConflictFile, Resolution, Segment};
use graft_core::paths::same_path;
use graft_core::{Oid, Operation, review};

use super::ops::{At, say};
use super::{DialogKind, Project, Tab, WorkspaceView, readable};

/// The resolver for one conflicted file of the selected worktree.
pub struct ConflictView {
    pub path: String,
    /// Shared, so the screen can hold it without copying it every frame.
    pub file: Option<Result<Arc<ConflictFile>, String>>,
    /// One per conflict; `None` until the user picks.
    pub resolutions: Vec<Option<Resolution>>,
    /// Text of conflicts the user edits by hand (their resolution is Custom).
    pub edits: Vec<Option<String>>,
    ticket: u64,
}

impl ConflictView {
    pub fn resolved(&self) -> usize {
        self.resolutions.iter().filter(|r| r.is_some()).count()
    }

    /// The file as it would be saved, if every conflict has a resolution.
    pub fn result(&self) -> Option<Vec<u8>> {
        let Some(Ok(file)) = &self.file else {
            return None;
        };
        conflicts::assemble(&file.segments, &self.effective())
    }

    /// Resolutions with hand edits filled in.
    pub fn effective(&self) -> Vec<Option<Resolution>> {
        self.resolutions
            .iter()
            .zip(&self.edits)
            .map(|(r, edit)| match (r, edit) {
                (Some(Resolution::Custom(_)), Some(text)) => {
                    Some(Resolution::Custom(text.as_bytes().to_vec()))
                }
                (r, _) => r.clone(),
            })
            .collect()
    }
}

/// What the text of a resolution is, for the editor's starting point.
fn resolution_text(segment: &Segment, resolution: Option<&Resolution>) -> String {
    let Segment::Conflict { ours, theirs, .. } = segment else {
        return String::new();
    };
    let bytes = match resolution {
        Some(r) => r.apply(ours, theirs),
        None => [ours.as_slice(), theirs.as_slice()].concat(),
    };
    String::from_utf8_lossy(&bytes).into_owned()
}

/// The Merge dialog for the selected worktree's branch, with `rev` picked.
/// `None` on a detached HEAD.
pub fn merge_dialog(project: &Project, rev: Option<&str>) -> Option<DialogKind> {
    let snap = project.snapshot.as_ref()?;
    let into = project.worktree()?.branch.clone()?;
    let choices = snap.merge_choices(&into).map(str::to_owned).collect();
    Some(DialogKind::Merge {
        into,
        rev: rev.unwrap_or_default().to_owned(),
        choices,
        filter: String::new(),
        no_ff: false,
    })
}

/// The ending of a merge, rebase, cherry-pick, revert or continue (the job
/// says whether it stopped with conflicts): say `done`, or say that `doing`
/// ("Merging x") stopped and show the conflicts in File status of the
/// worktree it ran in. `name` names that worktree in the message (e.g. by
/// its branch) when it is not the selected one.
pub(super) fn after_conflicts(
    done: String,
    doing: String,
    name: Option<String>,
) -> impl FnOnce(&mut Project, PathBuf, anyhow::Result<bool>) + Send {
    let place = match &name {
        Some(name) => format!("File status of {name}"),
        None => "File status".to_owned(),
    };
    let stopped =
        format!("{doing} stopped with conflicts. Resolve them in {place}, then Continue.");
    move |p, ran_in, result| {
        let selected = same_path(&p.nav.worktree, &ran_in);
        match result {
            Ok(false) => {
                p.infos.push(done);
                // Show the new commit (and the worktree is usually clean now).
                if selected && p.nav.tab == Tab::Workspace {
                    p.nav.view = WorkspaceView::History;
                }
            }
            Ok(true) => {
                p.infos.push(stopped);
                p.nav.tab = Tab::Workspace;
                if !selected {
                    p.open_worktree(ran_in);
                }
                p.nav.view = WorkspaceView::FileStatus;
            }
            Err(e) => p.report(e),
        }
    }
}

impl Project {
    /// Whether `path` has conflicts in the selected worktree.
    pub fn is_conflicted(&self, path: &str) -> bool {
        self.worktree()
            .and_then(|wt| wt.status.as_ref())
            .is_some_and(|s| s.conflicted().any(|e| e.path == path))
    }

    /// Open the resolver when the selected file is conflicted; close it when
    /// it no longer is.
    pub(super) fn sync_conflict(&mut self) {
        let path = self
            .diff_view
            .as_ref()
            .map(|v| v.key.path.clone())
            .filter(|p| self.is_conflicted(p));
        match path {
            None => self.conflict = None,
            Some(p) if self.conflict.as_ref().is_some_and(|c| c.path == p) => {}
            Some(p) => self.load_conflict(p),
        }
    }

    fn load_conflict(&mut self, path: String) {
        let ticket = self.ticket();
        self.conflict = Some(ConflictView {
            path: path.clone(),
            file: None,
            resolutions: Vec::new(),
            edits: Vec::new(),
            ticket,
        });
        let git = self.wt_git();
        self.jobs.spawn(
            move || conflicts::load(&git, &path),
            move |p, result| {
                let Some(view) = p.conflict.as_mut().filter(|v| v.ticket == ticket) else {
                    return;
                };
                if let Ok(file) = &result {
                    view.resolutions = vec![None; file.conflicts()];
                    view.edits = vec![None; file.conflicts()];
                }
                view.file = Some(readable(result).map(Arc::new));
            },
        );
    }

    /// Merge a reviewed branch in the worktree that has its base checked
    /// out, unless it moved on since `head`; conflicts are resolved there.
    pub(super) fn merge_reviewed(
        &mut self,
        worktree: PathBuf,
        branch: String,
        into: String,
        head: Oid,
    ) {
        let done = format!("Merged {branch} into {into}");
        let doing = format!("Merging {branch} into {into}");
        self.run_op(
            At::Path(worktree),
            "Merging",
            move |git| review::merge(&git, &branch, head),
            after_conflicts(done, doing, Some(into)),
        );
    }

    pub(super) fn rebase(&mut self, onto: String) {
        let done = format!("Rebased onto {onto}");
        let doing = format!("Rebasing onto {onto}");
        self.run_op(
            At::Selected,
            "Rebasing",
            move |git| conflicts::rebase(&git, &onto),
            after_conflicts(done, doing, None),
        );
    }

    pub(super) fn resolve_conflict(&mut self, index: usize, resolution: Option<Resolution>) {
        let Some(view) = &mut self.conflict else {
            return;
        };
        if let Some(slot) = view.resolutions.get_mut(index) {
            *slot = resolution;
            view.edits[index] = None;
        }
    }

    pub(super) fn edit_conflict(&mut self, index: usize) {
        let Some(view) = &mut self.conflict else {
            return;
        };
        let Some(Ok(file)) = &view.file else { return };
        let Some(segment) = file
            .segments
            .iter()
            .filter(|s| matches!(s, Segment::Conflict { .. }))
            .nth(index)
        else {
            return;
        };
        let text = resolution_text(segment, view.resolutions[index].as_ref());
        view.edits[index] = Some(text);
        view.resolutions[index] = Some(Resolution::Custom(Vec::new()));
    }

    pub(super) fn save_resolution(&mut self) {
        let Some(view) = &self.conflict else { return };
        let (Some(content), Some(Ok(file))) = (view.result(), &view.file) else {
            return;
        };
        let file = file.clone();
        let done = format!("Resolved {}", file.path);
        self.run_op(
            At::Selected,
            "Saving resolution",
            move |git| conflicts::resolve_with(&git, &file, &content),
            say(done),
        );
    }

    pub(super) fn take_side(&mut self, path: String, ours: bool) {
        let side = if ours { "ours" } else { "theirs" };
        let done = format!("Resolved {path} with {side}");
        self.run_op(
            At::Selected,
            "Resolving",
            move |git| {
                let file = conflicts::load(&git, &path)?;
                conflicts::take_side(&git, &file, ours)
            },
            say(done),
        );
    }

    pub(super) fn mark_resolved(&mut self, path: String) {
        let done = format!("Marked {path} as resolved");
        self.run_op(
            At::Selected,
            "Resolving",
            move |git| conflicts::mark_resolved(&git, &path),
            say(done),
        );
    }

    pub(super) fn continue_op(&mut self, kind: Operation) {
        let done = format!("{} finished", kind.noun());
        let noun = kind.noun().to_lowercase();
        let doing = format!("Continuing the {noun}");
        self.run_op(
            At::Selected,
            &format!("Continuing {noun}"),
            move |git| conflicts::continue_op(&git, kind),
            after_conflicts(done, doing, None),
        );
    }

    pub(super) fn abort_op(&mut self, kind: Operation) {
        self.run_op(
            At::Selected,
            "Aborting",
            move |git| conflicts::abort(&git, kind),
            say(format!("{} aborted", kind.noun())),
        );
    }
}
