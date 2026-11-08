//! The Review tab: the selected worktree against the merge-base of its base
//! branch (uncommitted work included), "Viewed" marks keyed by content, and
//! local notes and line comments. Marks, notes, comments and the chosen base
//! are remembered per worktree.

use std::collections::{BTreeMap, BTreeSet};
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use graft_core::diff::Source;
use graft_core::paths::same_path;
use graft_core::review::{self, Review, ReviewFile};
use serde::{Deserialize, Serialize};

use super::changes::SideLines;
use super::comments::{self, LineComment, Pick, Side};
use super::{DiffOwner, DiffView, FileKey, Project, readable};
use crate::text::plural;

/// What Graft remembers about reviewing one worktree.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
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
    /// Nothing to remember.
    pub fn is_empty(&self) -> bool {
        let Self {
            base,
            viewed,
            notes,
            comments,
        } = self;
        base.is_none() && viewed.is_empty() && notes.is_empty() && comments.is_empty()
    }

    /// The file notes that say something, trimmed, by path. A blank note
    /// counts as none.
    pub fn written_notes(&self) -> impl Iterator<Item = (&str, &str)> {
        self.notes
            .iter()
            .filter_map(|(path, note)| Some((path.as_str(), written(note)?)))
    }

    /// The note on the file at `path`, unless it is blank.
    pub fn file_note(&self, path: &str) -> Option<&str> {
        written(self.notes.get(path)?)
    }

    /// Line comments plus file notes.
    pub fn comment_count(&self) -> usize {
        self.written_notes().count() + self.comments.len()
    }

    /// Like [`Self::comment_count`], for the file at `path`.
    pub fn file_comment_count(&self, path: &str) -> usize {
        let note = self.file_note(path).is_some();
        self.comments.iter().filter(|c| c.path == path).count() + usize::from(note)
    }

    /// Drop the blank file notes.
    pub fn forget_blank_notes(&mut self) {
        self.notes.retain(|_, note| written(note).is_some());
    }
}

/// `note` trimmed, unless it is blank.
fn written(note: &str) -> Option<&str> {
    Some(note.trim()).filter(|n| !n.is_empty())
}

pub struct ReviewView {
    pub worktree: PathBuf,
    pub data: Option<Result<Arc<Review>, String>>,
    /// Path of the file whose diff is open.
    pub selected: Option<String>,
    /// The open file's diff.
    pub diff: Option<DiffView>,
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
    ticket: u64,
}

impl ReviewView {
    pub fn review(&self) -> Option<&Review> {
        self.data.as_ref().and_then(|d| d.as_deref().ok())
    }

    pub fn file(&self, path: &str) -> Option<&ReviewFile> {
        self.review()?.files.iter().find(|f| f.change.path == path)
    }

    /// Drop the pick and the comment being written.
    fn clear_draft(&mut self) {
        self.pick = None;
        self.editing = None;
        self.draft.clear();
    }
}

impl Project {
    /// The remembered review state of the selected worktree.
    pub fn review_state(&self) -> Option<&ReviewState> {
        self.review_state_of(&self.nav.worktree)
    }

    fn review_state_of(&self, worktree: &Path) -> Option<&ReviewState> {
        self.memory
            .reviews
            .iter()
            .find(|(path, _)| same_path(path, worktree))
            .map(|(_, state)| state)
    }

    fn review_state_mut(&mut self) -> &mut ReviewState {
        let reviews = &mut self.memory.reviews;
        let key = reviews
            .keys()
            .find(|path| same_path(path, &self.nav.worktree))
            .cloned()
            .unwrap_or_else(|| self.nav.worktree.clone());
        reviews.entry(key).or_default()
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
                worktree,
                data: None,
                selected: None,
                diff: None,
                split,
                pick: None,
                editing: None,
                draft: String::new(),
                focus_draft: false,
                ticket: 0,
            });
        }
        let ticket = self.ticket();
        self.review.as_mut().expect("just set").ticket = ticket;

        let current = self.worktree().and_then(|wt| wt.branch.as_deref());
        let base = self
            .review_state()
            .and_then(|s| s.base.clone())
            .or_else(|| review::default_base(self.snapshot.as_ref()?, current));
        let git = self.wt_git();
        self.jobs.spawn(
            move || {
                let base = base.ok_or_else(|| {
                    anyhow::anyhow!(
                        "There is no main or master branch to review against. Pick a base \
                         branch above."
                    )
                })?;
                review::load(&git, &base)
            },
            move |p, result| p.on_review(ticket, readable(result)),
        );
    }

    fn on_review(&mut self, ticket: u64, result: Result<Review, String>) {
        let Some(view) = self.review.as_mut().filter(|v| v.ticket == ticket) else {
            return;
        };
        let old = view.selected.as_ref().and_then(|p| view.file(p)).cloned();
        view.data = Some(result.map(Arc::new));
        let Some(files) = view.review().map(|r| r.files.clone()) else {
            view.diff = None;
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
                    view.diff = None;
                }
            }
        }
    }

    pub(super) fn open_review_file(&mut self, path: String, reload: bool) {
        let Some(view) = &mut self.review else { return };
        let Some(review) = view.review() else { return };
        let Some(file) = view.file(&path) else { return };
        let source = if file.untracked {
            Source::Untracked
        } else {
            Source::Base(review.merge_base)
        };
        let key = FileKey::changed(source, &file.change);
        if view.selected.as_ref() != Some(&path) {
            // Picked lines belong to the old file's diff.
            view.pick = None;
            if view.editing.take().is_some() {
                view.draft.clear();
            }
        }
        view.selected = Some(path);
        if reload && view.diff.as_ref().is_some_and(|d| d.key == key) {
            // Keeps the lines shown and the unchanged lines opened up.
            self.load_view(DiffOwner::Review);
            return;
        }
        self.open_diff(DiffOwner::Review, key);
    }

    /// The lines of the review comments on the file `key` shows in the
    /// review of `worktree`, so they never hide in a gap.
    pub(super) fn comment_lines(&self, key: &FileKey, worktree: &Path) -> SideLines {
        if !matches!(key.source, Source::Base(_)) {
            return Vec::new();
        }
        self.review_state_of(worktree)
            .map(|state| {
                state
                    .comments
                    .iter()
                    .filter(|c| c.path == key.path)
                    .map(|c| (c.side, c.start..c.end + 1))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// A pick across hidden lines shows them: a comment is about all of its
    /// lines, and they will show with it from then on.
    fn open_gaps_in_pick(&mut self) {
        let Some(pick) = self.review.as_ref().and_then(|v| v.pick) else {
            return;
        };
        let Some(view) = self.review.as_mut().and_then(|r| r.diff.as_mut()) else {
            return;
        };
        let (lo, hi) = (
            pick.anchor.0.min(pick.cursor.0),
            pick.anchor.0.max(pick.cursor.0),
        );
        let inside: Vec<Range<u32>> = view
            .gaps
            .iter()
            .flatten()
            .filter(|g| g.before.is_some_and(|h| lo < h && h <= hi))
            .map(|g| g.lines.clone())
            .collect();
        if inside.is_empty() {
            return;
        }
        view.revealed
            .extend(inside.into_iter().map(|lines| (Side::Old, lines)));
        self.load_view(DiffOwner::Review);
    }

    /// Mark or unmark a file as viewed; marking the open file moves on to the
    /// next file not viewed yet.
    pub(super) fn toggle_viewed(&mut self, path: String) {
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

    pub(super) fn set_review_base(&mut self, base: String) {
        self.review_state_mut().base = Some(base);
        self.load_review();
    }

    pub(super) fn toggle_review_split(&mut self) {
        if let Some(view) = &mut self.review {
            view.split = !view.split;
        }
    }

    pub(super) fn pick_line(&mut self, at: (usize, usize), side: Option<Side>, extend: bool) {
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
        self.open_gaps_in_pick();
    }

    /// Save the draft: as a new comment on the picked lines, or into the
    /// comment being edited.
    pub(super) fn save_comment(&mut self) {
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
                view.diff.as_ref().and_then(|d| d.diff.as_ref()),
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
        self.cancel_comment();
    }

    pub(super) fn cancel_comment(&mut self) {
        if let Some(view) = &mut self.review {
            view.clear_draft();
        }
    }

    pub(super) fn edit_comment(&mut self, id: u64) {
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

    pub(super) fn delete_comment(&mut self, id: u64) {
        self.review_state_mut().comments.retain(|c| c.id != id);
        if let Some(view) = &mut self.review
            && view.editing == Some(id)
        {
            view.editing = None;
            view.draft.clear();
        }
    }

    pub(super) fn set_file_note(&mut self, path: String, text: String) {
        let notes = &mut self.review_state_mut().notes;
        if text.is_empty() {
            notes.remove(&path);
        } else {
            notes.insert(path, text);
        }
    }

    pub(super) fn copy_review_comments(&mut self) {
        let (Some(wt), Some(review), Some(state)) = (
            self.worktree(),
            self.review.as_ref().and_then(ReviewView::review),
            self.review_state(),
        ) else {
            return;
        };
        let count = state.comment_count();
        if count == 0 {
            return;
        }
        let title = format!("{} against {}", wt.label(), review.base);
        self.clipboard = Some(comments::export(
            &title,
            state.written_notes(),
            &state.comments,
        ));
        self.infos.push(format!(
            "Copied {} for your agent",
            plural(count, "comment", "comments")
        ));
    }

    pub(super) fn clear_review_comments(&mut self) {
        let state = self.review_state_mut();
        state.comments.clear();
        state.notes.clear();
        self.cancel_comment();
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;
    use crate::project::{Action, Tab};
    use crate::testing::{git, open, wait};

    /// A repo on `feature`, one commit ahead of `main` (a.txt changed).
    fn feature_repo() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_path_buf();
        git(&root, &["init", "-q", "-b", "main"]);
        std::fs::write(
            root.join("a.txt"),
            "a
",
        )
        .unwrap();
        git(&root, &["add", "a.txt"]);
        git(&root, &["commit", "-q", "-m", "first"]);
        git(&root, &["switch", "-q", "-c", "feature"]);
        std::fs::write(
            root.join("a.txt"),
            "feature
",
        )
        .unwrap();
        git(&root, &["commit", "-q", "-am", "feature"]);
        (dir, root)
    }

    fn commit_file(root: &Path, name: &str) {
        std::fs::write(
            root.join(name),
            "new
",
        )
        .unwrap();
        git(root, &["add", name]);
        git(root, &["commit", "-q", "-m", name]);
    }

    /// The review's diff of a.txt, loaded.
    fn review_shows_a(p: &Project) -> bool {
        p.diff(DiffOwner::Review)
            .is_some_and(|d| d.key.path == "a.txt" && d.diff.is_some())
    }

    /// HEAD moving while Review is open selects the new HEAD in History;
    /// the review's diff stays (it was wiped to a spinner).
    #[test]
    fn a_head_move_keeps_the_review_diff() {
        let (_dir, root) = feature_repo();
        let mut project = open(&root);
        wait(&mut project, "no commit selected", |p| p.selected.is_some());
        let before = project.selected;
        project.apply(Action::SetTab(Tab::Review));
        wait(&mut project, "the review diff never loaded", review_shows_a);
        commit_file(&root, "b.txt");
        project.refresh();
        wait(&mut project, "the new HEAD was never selected", |p| {
            assert!(review_shows_a(p), "the review diff went away");
            p.selected.is_some() && p.selected != before
        });
    }

    /// A review that finishes after the user left Review lands in the
    /// review, not in the commit diff History shows meanwhile.
    #[test]
    fn a_late_review_leaves_the_commit_diff_alone() {
        let (_dir, root) = feature_repo();
        let mut project = open(&root);
        wait(&mut project, "no commit selected", |p| p.selected.is_some());
        project.apply(Action::SetTab(Tab::Review));
        project.apply(Action::ShowHistory);
        let key = FileKey {
            source: Source::Commit(project.selected.unwrap()),
            path: "a.txt".to_owned(),
            orig_path: None,
        };
        project.apply(Action::SelectCommitFile(key.clone()));
        wait(&mut project, "the review never loaded", |p| {
            review_shows_a(p) && p.commit_diff.as_ref().is_some_and(|d| d.diff.is_some())
        });
        assert_eq!(project.commit_diff.as_ref().map(|d| &d.key), Some(&key));
        assert_eq!(project.nav.tab, Tab::Workspace);
    }

    /// Going to another view closes File history, its diff with it.
    #[test]
    fn leaving_file_history_closes_it() {
        let (_dir, root) = feature_repo();
        let mut project = open(&root);
        project.apply(Action::OpenFileHistory("a.txt".to_owned()));
        wait(&mut project, "the file's diff never loaded", |p| {
            p.diff(DiffOwner::FileHistory)
                .is_some_and(|d| d.diff.is_some())
        });
        project.apply(Action::ShowHistory);
        assert!(project.file_history.is_none());
        assert!(project.diff(DiffOwner::FileHistory).is_none());
    }

    /// A review with only line comments is worth remembering (they were
    /// lost on restart when only marks and notes counted).
    #[test]
    fn line_comments_alone_are_remembered() {
        let mut state = ReviewState::default();
        assert!(state.is_empty());
        state.comments.push(LineComment {
            id: 1,
            path: "a.rs".into(),
            side: Side::New,
            start: 1,
            end: 1,
            excerpt: "+a\n".into(),
            text: "why?".into(),
        });
        assert!(!state.is_empty());
        let memory = super::super::RepoMemory {
            reviews: [
                (PathBuf::from("wt"), state),
                (PathBuf::from("empty"), ReviewState::default()),
            ]
            .into(),
            slashed: Vec::new(),
        }
        .pruned();
        assert_eq!(memory.reviews.len(), 1);
    }

    /// Turning "Whole file" on while another worktree is selected reloads
    /// the review's diff from the reviewed worktree, not the selected one.
    #[test]
    fn whole_file_toggled_elsewhere_keeps_the_reviewed_worktrees_file() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("main");
        std::fs::create_dir_all(&root).unwrap();
        let lines: String = (1..=20).map(|n| format!("l{n}\n")).collect();
        let reviewed = lines.replace("l1\n", "feature\n");
        git(&root, &["init", "-q", "-b", "main"]);
        std::fs::write(root.join("a.txt"), &lines).unwrap();
        git(&root, &["add", "a.txt"]);
        git(&root, &["commit", "-q", "-m", "first"]);
        git(&root, &["switch", "-q", "-c", "feature"]);
        std::fs::write(root.join("a.txt"), &reviewed).unwrap();
        git(&root, &["commit", "-q", "-am", "feature"]);
        let other = dir.path().join("other");
        let other_arg = other.to_str().unwrap();
        git(
            &root,
            &["worktree", "add", "-q", "-b", "other", other_arg, "main"],
        );
        std::fs::write(other.join("a.txt"), "bbb from other\n").unwrap();

        let mut project = open(&root);
        let snap = project.snapshot.clone().unwrap();
        let main = snap.main_worktree().unwrap().path.clone();
        let other = snap.worktrees.iter().find(|w| !w.is_main).unwrap();
        let other = other.path.clone();
        project.apply(Action::SetTab(Tab::Review));
        wait(&mut project, "the review diff never loaded", review_shows_a);
        let hidden = |p: &Project| {
            p.diff(DiffOwner::Review)
                .and_then(|d| d.gaps.as_ref())
                .is_some_and(|g| !g.is_empty())
        };
        assert!(hidden(&project), "unchanged lines are hidden at first");

        project.apply(Action::ShowHistory);
        project.apply(Action::OpenWorktree(other));
        let mut settings = project.settings().clone();
        settings.whole_file = true;
        project.set_settings(&settings);
        wait(&mut project, "the whole file never showed", |p| {
            review_shows_a(p) && !hidden(p)
        });
        let shown = |p: &Project| -> String {
            let diff = p.diff(DiffOwner::Review).unwrap().diff.as_ref().unwrap();
            let lines = diff.hunks.iter().flat_map(|h| &h.lines);
            lines.map(|l| l.text() + "\n").collect()
        };
        assert_eq!(shown(&project), format!("l1\n{reviewed}"));

        project.apply(Action::OpenWorktree(main));
        project.apply(Action::SetTab(Tab::Review));
        wait(&mut project, "the review never loaded again", |p| {
            p.review.as_ref().is_some_and(|r| r.review().is_some())
        });
        assert!(!shown(&project).contains("bbb"), "{}", shown(&project));
    }
}
