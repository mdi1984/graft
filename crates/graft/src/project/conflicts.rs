//! Merges and conflicts: the Merge dialog's job, the resolver for the selected
//! conflicted file, and Continue / Abort for whatever stopped part-way.

use graft_core::conflicts::{self, ConflictFile, Resolution, Segment};

use super::changes::Then;
use super::{Action, Confirm, DialogKind, Event, Project, WorkspaceView};

/// The resolver for one conflicted file of the selected worktree.
pub struct ConflictView {
    pub path: String,
    pub file: Option<Result<ConflictFile, String>>,
    /// One per conflict; `None` until the user picks.
    pub resolutions: Vec<Option<Resolution>>,
    /// Text of conflicts the user edits by hand (their resolution is Custom).
    pub edits: Vec<Option<String>>,
    generation: u64,
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
        Some(r) => conflicts::assemble(
            std::slice::from_ref(segment),
            std::slice::from_ref(&Some(r.clone())),
        )
        .unwrap_or_default(),
        None => [ours.as_slice(), theirs.as_slice()].concat(),
    };
    String::from_utf8_lossy(&bytes).into_owned()
}

/// The Merge dialog for the selected worktree's branch, with `rev` picked.
/// `None` on a detached HEAD.
pub fn merge_dialog(project: &Project, rev: Option<&str>) -> Option<DialogKind> {
    let snap = project.snapshot.as_ref()?;
    let into = project.worktree()?.branch.clone()?;
    let choices: Vec<String> = snap
        .branches
        .iter()
        .map(|b| b.name.clone())
        .filter(|b| *b != into)
        .chain(snap.remote_branches.iter().map(|b| b.name.clone()))
        .collect();
    Some(DialogKind::Merge {
        into,
        rev: rev.unwrap_or_default().to_owned(),
        choices,
        filter: String::new(),
        no_ff: false,
    })
}

impl Project {
    /// Paths with conflicts in the selected worktree.
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
        self.diff_generation += 1;
        let generation = self.diff_generation;
        self.conflict = Some(ConflictView {
            path: path.clone(),
            file: None,
            resolutions: Vec::new(),
            edits: Vec::new(),
            generation,
        });
        let git = self.wt_git();
        self.pool.spawn(&self.tx, move || Event::Conflict {
            generation,
            result: conflicts::load(&git, &path).map_err(|e| format!("{e:#}")),
        });
    }

    pub(super) fn on_conflict(&mut self, generation: u64, result: Result<ConflictFile, String>) {
        let Some(view) = &mut self.conflict else {
            return;
        };
        if view.generation != generation {
            return;
        }
        if let Ok(file) = &result {
            view.resolutions = vec![None; file.conflicts()];
            view.edits = vec![None; file.conflicts()];
        }
        view.file = Some(result);
    }

    fn merge(&mut self, rev: String, no_ff: bool, into: String) {
        let done = format!("Merged {rev} into {into}");
        let stopped = format!(
            "Merging {rev} stopped with conflicts. Resolve them in File status, then Continue."
        );
        self.run_op_at(
            self.wt_git(),
            "Merging",
            Then::MayConflict(done),
            move |git| Ok(conflicts::merge(&git, &rev, no_ff)?.then_some(stopped)),
        );
    }

    pub(super) fn apply_conflict_action(&mut self, action: Action) {
        match action {
            Action::Merge { rev, no_ff, into } => self.merge(rev, no_ff, into),
            Action::Rebase(onto) => {
                let done = format!("Rebased onto {onto}");
                let stopped = format!(
                    "Rebasing onto {onto} stopped with conflicts. Resolve them in File status, then Continue."
                );
                self.run_op_at(
                    self.wt_git(),
                    "Rebasing",
                    Then::MayConflict(done),
                    move |git| Ok(conflicts::rebase(&git, &onto)?.then_some(stopped)),
                );
            }
            Action::ResolveConflict { index, resolution } => {
                let Some(view) = &mut self.conflict else {
                    return;
                };
                if let Some(slot) = view.resolutions.get_mut(index) {
                    *slot = resolution;
                    view.edits[index] = None;
                }
            }
            Action::EditConflict(index) => {
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
            Action::SaveResolution => {
                let Some(view) = &self.conflict else { return };
                let (Some(content), Some(Ok(file))) = (view.result(), &view.file) else {
                    return;
                };
                let file = file.clone();
                let done = format!("Resolved {}", file.path);
                self.run_op("Saving resolution", Then::Info(done), move |git| {
                    conflicts::resolve_with(&git, &file, &content)
                });
            }
            Action::TakeSide { path, ours } => {
                let side = if ours { "ours" } else { "theirs" };
                let done = format!("Resolved {path} with {side}");
                self.run_op("Resolving", Then::Info(done), move |git| {
                    let file = conflicts::load(&git, &path)?;
                    conflicts::take_side(&git, &file, ours)
                });
            }
            Action::MarkResolved(path) => {
                let done = format!("Marked {path} as resolved");
                self.run_op("Resolving", Then::Info(done), move |git| {
                    conflicts::mark_resolved(&git, &path)
                });
            }
            Action::ContinueOp(kind) => {
                let done = format!("{} finished", kind.noun());
                let stopped =
                    "The next commit has conflicts too. Resolve them, then Continue.".to_owned();
                self.run_op_at(
                    self.wt_git(),
                    &format!("Continuing {}", kind.noun().to_lowercase()),
                    Then::MayConflict(done),
                    move |git| Ok(conflicts::continue_op(&git, kind)?.then_some(stopped)),
                );
            }
            Action::AbortOp { kind, confirmed } => {
                if !confirmed {
                    self.confirm = Some(Confirm {
                        title: format!("Abort the {}?", kind.noun().to_lowercase()),
                        body: "The worktree goes back to how it was before it started. \
                               Conflict resolutions made so far are lost."
                            .to_owned(),
                        confirm_label: format!("Abort {}", kind.noun().to_lowercase()),
                        action: Action::AbortOp {
                            kind,
                            confirmed: true,
                        },
                    });
                    return;
                }
                let done = format!("{} aborted", kind.noun());
                self.run_op("Aborting", Then::Info(done), move |git| {
                    conflicts::abort(&git, kind)
                });
            }
            _ => {}
        }
    }

    /// A merge, rebase, cherry-pick or revert finished or stopped.
    pub(super) fn on_may_conflict(&mut self, done: String, stopped: Option<String>) {
        if self.dialog.as_ref().is_some_and(|d| d.submitting) {
            self.dialog = None;
        }
        let elsewhere = self.pending_conflicts.take();
        match stopped {
            Some(text) => {
                self.infos.push(text);
                if let Some(path) = elsewhere {
                    self.nav.tab = super::Tab::Workspace;
                    self.open_worktree(path);
                }
                self.nav.tab = super::Tab::Workspace;
                self.nav.view = WorkspaceView::FileStatus;
            }
            None => {
                self.infos.push(done);
                // Show the new commit (and the worktree is usually clean now).
                if self.nav.tab == super::Tab::Workspace {
                    self.nav.view = WorkspaceView::History;
                }
            }
        }
    }
}
