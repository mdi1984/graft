//! The Review tab: the selected worktree against the merge-base of its base
//! branch (uncommitted work included), "Viewed" marks keyed by content, and
//! local notes and line comments. Marks, notes, comments and the chosen base
//! are remembered per worktree.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::Arc;

use graft_core::diff::Source;
use graft_core::review::{self, Review, ReviewFile};
use graft_core::{conflicts, same_path};
use serde::{Deserialize, Serialize};

use super::changes::Then;
use super::comments::{self, LineComment, Pick};
use super::{Action, Event, FileKey, Project};

/// What Graft remembers about reviewing one worktree.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ReviewState {
    /// The branch picked to review against; `None` uses the default branch.
    pub base: Option<String>,
    /// [`ReviewFile::viewed_key`]s of files marked as viewed.
    pub viewed: BTreeSet<String>,
    /// Notes per file path.
    pub notes: BTreeMap<String, String>,
    /// Comments on lines of the diff.
    pub comments: Vec<LineComment>,
}

impl ReviewState {
    /// Line comments plus non-empty file notes.
    pub fn comment_count(&self) -> usize {
        let notes = self.notes.values().filter(|n| !n.trim().is_empty()).count();
        notes + self.comments.len()
    }
}

pub struct ReviewView {
    pub worktree: PathBuf,
    pub data: Option<Result<Arc<Review>, String>>,
    pub loading: bool,
    /// Path of the file whose diff is open.
    pub selected: Option<String>,
    /// Show diffs side by side (else unified).
    pub split: bool,
    /// Lines picked in the open file's diff, to comment on.
    pub pick: Option<Pick>,
    /// The comment being edited, if any.
    pub editing: Option<u64>,
    /// Text of the comment being written or edited.
    pub draft: String,
    /// Focus the comment box on the next frame.
    pub focus_draft: bool,
    generation: u64,
}

impl ReviewView {
    pub fn review(&self) -> Option<&Review> {
        self.data.as_ref().and_then(|d| d.as_deref().ok())
    }

    pub fn file(&self, path: &str) -> Option<&ReviewFile> {
        self.review()?.files.iter().find(|f| f.change.path == path)
    }
}

impl Project {
    /// The remembered review state of the selected worktree.
    pub fn review_state(&self) -> Option<&ReviewState> {
        self.reviews
            .iter()
            .find(|(path, _)| same_path(path, &self.nav.worktree))
            .map(|(_, state)| state)
    }

    pub fn review_state_mut(&mut self) -> &mut ReviewState {
        let key = self
            .reviews
            .keys()
            .find(|path| same_path(path, &self.nav.worktree))
            .cloned()
            .unwrap_or_else(|| self.nav.worktree.clone());
        self.reviews.entry(key).or_default()
    }

    pub fn is_viewed(&self, file: &ReviewFile) -> bool {
        self.review_state()
            .is_some_and(|s| s.viewed.contains(&file.viewed_key()))
    }

    /// (Re)load the review of the selected worktree.
    pub(super) fn load_review(&mut self) {
        let worktree = self.nav.worktree.clone();
        let same = self
            .review
            .as_ref()
            .is_some_and(|r| same_path(&r.worktree, &worktree));
        if !same {
            let split = self.review.as_ref().is_none_or(|r| r.split);
            self.review = Some(ReviewView {
                worktree: worktree.clone(),
                data: None,
                loading: false,
                selected: None,
                split,
                pick: None,
                editing: None,
                draft: String::new(),
                focus_draft: false,
                generation: 0,
            });
            self.commit_diff = None;
        }
        self.diff_generation += 1;
        let generation = self.diff_generation;
        let view = self.review.as_mut().expect("just set");
        view.loading = true;
        view.generation = generation;

        let base = self.review_state().and_then(|s| s.base.clone());
        let snap = self.snapshot.as_ref();
        let branches: Vec<String> = snap
            .map(|s| s.branches.iter().map(|b| b.name.clone()).collect())
            .unwrap_or_default();
        let current = self.worktree().and_then(|wt| wt.branch.clone());
        let git = self.wt_git();
        self.pool.spawn(&self.tx, move || {
            let result = (|| {
                let base = base
                    .or_else(|| review::default_base(&git, &branches, current.as_deref()))
                    .ok_or_else(|| {
                        "There is no main or master branch to review against. Pick a base \
                         branch above."
                            .to_owned()
                    })?;
                review::load(&git, &base).map_err(|e| format!("{e:#}"))
            })();
            Event::Review { generation, result }
        });
    }

    pub(super) fn on_review(&mut self, generation: u64, result: Result<Review, String>) {
        let Some(view) = &mut self.review else { return };
        if view.generation != generation {
            return;
        }
        view.loading = false;
        let old = view.selected.as_ref().and_then(|p| view.file(p)).cloned();
        view.data = Some(result.map(Arc::new));
        let Some(files) = view.review().map(|r| r.files.clone()) else {
            self.commit_diff = None;
            return;
        };
        // Keep the open file; else open the first one not viewed yet.
        let keep = old
            .as_ref()
            .and_then(|o| files.iter().find(|f| f.change.path == o.change.path))
            .cloned();
        let next = keep.or_else(|| {
            files
                .iter()
                .find(|f| !self.is_viewed(f))
                .or(files.first())
                .cloned()
        });
        match next {
            Some(file) => {
                // Reload the diff when the file changed since it was shown.
                let changed = old.as_ref() != Some(&file);
                self.open_review_file(file.change.path.clone(), changed);
            }
            None => {
                if let Some(view) = &mut self.review {
                    view.selected = None;
                }
                self.commit_diff = None;
            }
        }
    }

    fn open_review_file(&mut self, path: String, reload: bool) {
        let Some(view) = &mut self.review else { return };
        let Some(review) = view.review() else { return };
        let Some(file) = view.file(&path) else { return };
        let source = if file.untracked {
            Source::Untracked
        } else {
            Source::Base(review.merge_base)
        };
        if view.selected.as_ref() != Some(&path) {
            // Picked lines belong to the old file's diff.
            view.pick = None;
            if view.editing.take().is_some() {
                view.draft.clear();
            }
        }
        view.selected = Some(path.clone());
        let key = FileKey { source, path };
        if reload || self.commit_diff.as_ref().is_some_and(|d| d.key != key) {
            self.commit_diff = None;
        }
        self.select_commit_file(key);
    }

    /// Mark or unmark a file as viewed; marking the open file moves on to the
    /// next file not viewed yet.
    fn toggle_viewed(&mut self, path: String) {
        let Some(file) = self.review.as_ref().and_then(|v| v.file(&path)).cloned() else {
            return;
        };
        let key = file.viewed_key();
        let state = self.review_state_mut();
        let now_viewed = !state.viewed.remove(&key);
        if now_viewed {
            state.viewed.insert(key);
        }
        let Some(view) = &self.review else { return };
        if !now_viewed || view.selected.as_deref() != Some(path.as_str()) {
            return;
        }
        let Some(review) = view.review() else { return };
        let files = &review.files;
        let at = files
            .iter()
            .position(|f| f.change.path == path)
            .unwrap_or(0);
        let next = files[at + 1..]
            .iter()
            .chain(&files[..at])
            .find(|f| !self.is_viewed(f))
            .map(|f| f.change.path.clone());
        if let Some(next) = next {
            self.open_review_file(next, false);
        }
    }

    /// Save the draft: as a new comment on the picked lines, or into the
    /// comment being edited.
    fn save_comment(&mut self) {
        let Some(view) = &self.review else { return };
        let text = view.draft.trim().to_owned();
        if text.is_empty() {
            return;
        }
        if let Some(id) = view.editing {
            if let Some(c) = self
                .review_state_mut()
                .comments
                .iter_mut()
                .find(|c| c.id == id)
            {
                c.text = text;
            }
        } else {
            let (Some(pick), Some(path), Some(diff)) = (
                view.pick,
                view.selected.clone(),
                self.commit_diff.as_ref().and_then(|d| d.diff.as_ref()),
            ) else {
                return;
            };
            let Some((side, start, end)) = comments::range(diff, &pick) else {
                return;
            };
            let Some(excerpt) = comments::excerpt(diff, side, start, end) else {
                return;
            };
            let state = self.review_state_mut();
            let id = state.comments.iter().map(|c| c.id).max().unwrap_or(0) + 1;
            state.comments.push(LineComment {
                id,
                path,
                side,
                start,
                end,
                excerpt,
                text,
            });
        }
        if let Some(view) = &mut self.review {
            view.pick = None;
            view.editing = None;
            view.draft.clear();
        }
    }

    pub(super) fn apply_review_action(&mut self, action: Action) {
        match action {
            Action::SelectReviewFile(path) => self.open_review_file(path, false),
            Action::ToggleViewed(path) => self.toggle_viewed(path),
            Action::SetReviewBase(base) => {
                self.review_state_mut().base = Some(base);
                self.load_review();
            }
            Action::ToggleReviewSplit => {
                if let Some(view) = &mut self.review {
                    view.split = !view.split;
                }
            }
            Action::PickLine { at, side, extend } => {
                let Some(view) = &mut self.review else { return };
                match &mut view.pick {
                    Some(pick) if extend && pick.side == side => pick.cursor = at,
                    _ => {
                        if view.editing.take().is_some() {
                            view.draft.clear();
                        }
                        view.pick = Some(Pick {
                            anchor: at,
                            cursor: at,
                            side,
                        });
                        view.focus_draft = true;
                    }
                }
            }
            Action::SaveComment => self.save_comment(),
            Action::CancelComment => {
                if let Some(view) = &mut self.review {
                    view.pick = None;
                    view.editing = None;
                    view.draft.clear();
                }
            }
            Action::EditComment(id) => {
                let Some(text) = self
                    .review_state()
                    .and_then(|s| s.comments.iter().find(|c| c.id == id))
                    .map(|c| c.text.clone())
                else {
                    return;
                };
                if let Some(view) = &mut self.review {
                    view.pick = None;
                    view.editing = Some(id);
                    view.draft = text;
                    view.focus_draft = true;
                }
            }
            Action::DeleteComment(id) => {
                self.review_state_mut().comments.retain(|c| c.id != id);
                if let Some(view) = &mut self.review
                    && view.editing == Some(id)
                {
                    view.editing = None;
                    view.draft.clear();
                }
            }
            Action::ClearReviewComments => {
                let state = self.review_state_mut();
                state.comments.clear();
                state.notes.clear();
                if let Some(view) = &mut self.review {
                    view.pick = None;
                    view.editing = None;
                    view.draft.clear();
                }
            }
            Action::MergeReviewed {
                worktree,
                branch,
                into,
            } => {
                let done = format!("Merged {branch} into {into}");
                let stopped = format!(
                    "Merging {branch} into {into} stopped with conflicts. Resolve them in \
                     File status of {into}, then Continue."
                );
                let target = worktree.clone();
                self.run_op_at(
                    self.git.at(&worktree),
                    "Merging",
                    Then::MayConflict(done),
                    move |git| Ok(conflicts::merge(&git, &branch, false)?.then_some(stopped)),
                );
                // Conflicts are resolved where the merge happened.
                self.pending_conflicts = Some(target);
            }
            _ => {}
        }
    }
}
