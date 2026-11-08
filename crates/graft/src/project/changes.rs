//! File status: the selected file's diff, line selection, staging and
//! discarding, and the commit form. Also loads the read-only diffs: each
//! screen that shows one (History, Stashes, Review, File history) holds its
//! own, see [`DiffOwner`].

use std::collections::BTreeSet;
use std::ops::Range;

use graft_core::diff::{self, FileDiff, Gap, Source};
use graft_core::ops::{self, LineOp};
use graft_core::paths::same_path;
use graft_core::{EntryKind, FileList, Git, StatusEntry};

use super::comments::{Pick, Side};
use super::ops::{At, quietly};
use super::{Action, Confirm, Project, RemoteOp, readable};

/// Which file a diff shows, and from where: a File status list, a commit
/// or stash, or the review's base.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileKey {
    pub source: Source,
    pub path: String,
}

impl FileKey {
    /// The key for a status entry in the staged or the unstaged list.
    pub fn of(entry: &StatusEntry, staged: bool) -> Self {
        let source = if staged {
            Source::Staged
        } else if entry.kind == EntryKind::Untracked {
            Source::Untracked
        } else {
            Source::Unstaged
        };
        Self {
            source,
            path: entry.path.clone(),
        }
    }

    pub fn is_staged(&self) -> bool {
        self.source == Source::Staged
    }
}

/// Worked out with the diff, off the UI thread: the changed words and the
/// syntax colors of each (hunk, line).
#[derive(Default)]
pub struct Highlights {
    pub words: Vec<Vec<Option<Range<usize>>>>,
    pub syntax: graft_core::syntax::Syntax,
}

/// Diffs up to this many lines get their syntax colors with the diff; bigger
/// ones show at once and get colored by a second job (~20 µs a line).
const INLINE_SYNTAX_LINES: usize = 1_500;

fn line_count(diff: &FileDiff) -> usize {
    diff.hunks.iter().map(|h| h.lines.len()).sum()
}

/// A diff as loaded in the background, ready to show.
struct Loaded {
    diff: FileDiff,
    gaps: Option<Vec<Gap>>,
    highlights: Highlights,
}

/// Line ranges on one side of a diff.
pub type SideLines = Vec<(Side, Range<u32>)>;

fn has(lines: &[(Side, Range<u32>)], old: u32, new: u32) -> bool {
    lines.iter().any(|(side, r)| match side {
        Side::Old => r.contains(&old),
        Side::New => r.contains(&new),
    })
}

/// Load a file's diff with the unchanged lines asked for (all of them with
/// `whole`) and the `kept` ones (review comments), and work out its
/// highlights.
fn load_shown(
    git: &Git,
    key: &FileKey,
    whole: bool,
    revealed: &[(Side, Range<u32>)],
    kept: &[(Side, Range<u32>)],
) -> anyhow::Result<Loaded> {
    let reveal = |old, new| whole || has(revealed, old, new);
    let keep = |old, new| has(kept, old, new);
    let (diff, gaps) = diff::load_expanded(git, &key.source, &key.path, reveal, keep)?;
    let highlights = Highlights::of(&diff);
    Ok(Loaded {
        diff,
        gaps,
        highlights,
    })
}

/// Lines of `from` at their place in `to`; `None` when `to` changed more
/// than which unchanged lines show.
fn follow(
    from: &FileDiff,
    to: &FileDiff,
    at: impl IntoIterator<Item = (usize, usize)>,
) -> Option<Vec<(usize, usize)>> {
    if !diff::same_changes(from, to) {
        return None;
    }
    at.into_iter()
        .map(|at| diff::relocate(from, to, at))
        .collect()
}

impl Highlights {
    pub fn of(diff: &FileDiff) -> Self {
        let small = line_count(diff) <= INLINE_SYNTAX_LINES;
        Self {
            words: diff::word_highlights(diff),
            syntax: if small {
                graft_core::syntax::highlight(diff)
            } else {
                Vec::new()
            },
        }
    }
}

/// The screen a diff belongs to. Each holds its own diff, so a result that
/// arrives late lands where it was asked for, or nowhere.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiffOwner {
    /// File status: the selected worktree's file, with staging.
    Status,
    /// History: a file of the selected commit.
    Commit,
    /// Stashes: a file of the selected stash.
    Stash,
    /// Review: the open file of the review.
    Review,
    /// File history: the file in the selected commit.
    FileHistory,
}

impl DiffOwner {
    const ALL: [DiffOwner; 5] = [
        DiffOwner::Status,
        DiffOwner::Commit,
        DiffOwner::Stash,
        DiffOwner::Review,
        DiffOwner::FileHistory,
    ];

    /// A diff with a close button (and Esc): a commit's or stash's file.
    pub fn closes(self) -> bool {
        matches!(self, DiffOwner::Commit | DiffOwner::Stash)
    }
}

/// What a diff can change in the worktree. Only File status stages; a
/// commit's, stash's or review's diff changes nothing (it is read-only),
/// even where the review shows an untracked file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Staging {
    /// Staged changes: unstage them.
    Staged,
    /// Unstaged or untracked changes: stage or discard them.
    Unstaged,
}

impl Staging {
    fn of(owner: DiffOwner, source: &Source) -> Option<Self> {
        match (owner, source) {
            (DiffOwner::Status, Source::Staged) => Some(Staging::Staged),
            (DiffOwner::Status, Source::Unstaged | Source::Untracked) => Some(Staging::Unstaged),
            _ => None,
        }
    }
}

/// One file's diff, as one screen shows it.
pub struct DiffView {
    pub owner: DiffOwner,
    pub key: FileKey,
    pub diff: Option<FileDiff>,
    pub highlights: Highlights,
    pub error: Option<String>,
    /// Selected changed lines as (hunk, line).
    pub selection: BTreeSet<(usize, usize)>,
    pub anchor: Option<(usize, usize)>,
    /// What staging, discarding and line selection do; `None` when there
    /// are none (read-only).
    pub staging: Option<Staging>,
    /// Unchanged lines still hidden; `None` when they cannot be shown.
    pub gaps: Option<Vec<Gap>>,
    /// Unchanged lines the user asked to see, by the line numbers of the
    /// side that stays put (the working tree's while staging).
    pub revealed: SideLines,
    ticket: u64,
}

impl DiffView {
    fn new(key: FileKey, owner: DiffOwner) -> Self {
        Self {
            staging: Staging::of(owner, &key.source),
            owner,
            key,
            diff: None,
            highlights: Highlights::default(),
            error: None,
            selection: BTreeSet::new(),
            anchor: None,
            gaps: None,
            revealed: Vec::new(),
            ticket: 0,
        }
    }

    fn clear_selection(&mut self) {
        self.selection.clear();
        self.anchor = None;
    }

    fn changed_lines(&self) -> Vec<(usize, usize)> {
        let Some(diff) = &self.diff else {
            return Vec::new();
        };
        diff.hunks
            .iter()
            .enumerate()
            .flat_map(|(h, hunk)| {
                hunk.lines
                    .iter()
                    .enumerate()
                    .filter(|(_, l)| l.is_change())
                    .map(move |(l, _)| (h, l))
            })
            .collect()
    }

    /// Select every changed line from `a` to `b`.
    fn select_range(&mut self, a: (usize, usize), b: (usize, usize)) {
        let (lo, hi) = (a.min(b), a.max(b));
        for at in self.changed_lines() {
            if lo <= at && at <= hi {
                self.selection.insert(at);
            }
        }
    }
}

#[derive(Default)]
pub struct CommitForm {
    pub summary: String,
    pub body: String,
    pub amend: bool,
    pub push_after: bool,
    /// What the user had typed before switching to amend.
    before_amend: Option<(String, String)>,
}

impl CommitForm {
    /// Empty, for the next commit: what was typed goes, "Push after commit"
    /// (a preference, not part of the message) stays.
    pub fn cleared(&self) -> Self {
        Self {
            push_after: self.push_after,
            ..Self::default()
        }
    }

    /// Can it be committed, with `staged` files staged? Amending needs no
    /// staged files.
    pub fn ready(&self, staged: usize) -> bool {
        !self.summary.trim().is_empty() && (staged > 0 || self.amend)
    }

    pub fn message(&self) -> String {
        let summary = self.summary.trim();
        let body = self.body.trim();
        if body.is_empty() {
            summary.to_owned()
        } else {
            format!("{summary}\n\n{body}")
        }
    }

    /// Something typed that a reset would lose.
    pub fn is_drafted(&self) -> bool {
        !self.summary.trim().is_empty() || !self.body.trim().is_empty()
    }
}

impl Action {
    /// Ask before discarding a whole file (File status, the diff's
    /// Discard…); confirmed, [`Project::discard`] runs it.
    pub fn discard_file(source: &Source, path: String) -> Action {
        let untracked = *source == Source::Untracked;
        let body = if untracked {
            format!("{path} is not tracked by git. It will be moved to the Recycle Bin.")
        } else {
            format!("All unstaged changes to {path} will be lost. Staged changes stay.")
        };
        let (tracked, untracked_paths) = if untracked {
            (Vec::new(), vec![path])
        } else {
            (vec![path], Vec::new())
        };
        Action::Ask(Box::new(Confirm {
            title: "Discard changes?".to_owned(),
            body,
            confirm_label: "Discard".to_owned(),
            action: Action::Discard {
                tracked,
                untracked: untracked_paths,
            },
        }))
    }
}

impl Project {
    /// The diff `owner` shows, if any.
    pub fn diff(&self, owner: DiffOwner) -> Option<&DiffView> {
        match owner {
            DiffOwner::Status => self.diff_view.as_ref(),
            DiffOwner::Commit => self.commit_diff.as_ref(),
            DiffOwner::Stash => self.stash_diff.as_ref(),
            DiffOwner::Review => self.review.as_ref()?.diff.as_ref(),
            DiffOwner::FileHistory => self.file_history.as_ref()?.diff.as_ref(),
        }
    }

    /// Where `owner` keeps its diff; `None` while its screen is not open.
    fn diff_slot(&mut self, owner: DiffOwner) -> Option<&mut Option<DiffView>> {
        match owner {
            DiffOwner::Status => Some(&mut self.diff_view),
            DiffOwner::Commit => Some(&mut self.commit_diff),
            DiffOwner::Stash => Some(&mut self.stash_diff),
            DiffOwner::Review => Some(&mut self.review.as_mut()?.diff),
            DiffOwner::FileHistory => Some(&mut self.file_history.as_mut()?.diff),
        }
    }

    fn diff_mut(&mut self, owner: DiffOwner) -> Option<&mut DiffView> {
        self.diff_slot(owner)?.as_mut()
    }

    /// Close the diff `owner` shows.
    pub(super) fn close_diff(&mut self, owner: DiffOwner) {
        if let Some(slot) = self.diff_slot(owner) {
            *slot = None;
        }
    }

    pub(super) fn select_file(&mut self, key: FileKey) {
        self.open_diff(DiffOwner::Status, key);
        self.sync_conflict();
    }

    /// Show `key` in `owner`'s diff, unless it shows it already.
    pub(super) fn open_diff(&mut self, owner: DiffOwner, key: FileKey) {
        let Some(slot) = self.diff_slot(owner) else {
            return;
        };
        if slot.as_ref().is_some_and(|v| v.key == key) {
            return;
        }
        *slot = Some(DiffView::new(key, owner));
        self.load_view(owner);
    }

    /// (Re)load `owner`'s diff; what is selected or picked in it survives
    /// if only the unchanged lines shown change.
    pub(super) fn load_view(&mut self, owner: DiffOwner) {
        let Some(key) = self.diff(owner).map(|v| v.key.clone()) else {
            return;
        };
        let ticket = self.ticket();
        let whole = self.settings.whole_file;
        // Commits are the same everywhere; the rest reads the worktree.
        let git = match key.source {
            Source::Commit(_) => self.git.clone(),
            _ => self.wt_git(),
        };
        let kept = self.comment_lines(&key);
        let view = self.diff_mut(owner).expect("checked above");
        view.ticket = ticket;
        let revealed = view.revealed.clone();
        self.jobs.spawn(
            move || load_shown(&git, &key, whole, &revealed, &kept),
            move |p, result| p.on_view_loaded(owner, ticket, result),
        );
    }

    /// A loaded diff, if `owner` still shows the load asked for (the ticket
    /// of a closed or replaced diff matches nothing).
    fn on_view_loaded(&mut self, owner: DiffOwner, ticket: u64, result: anyhow::Result<Loaded>) {
        let picked = self.picked_in(owner);
        let Some(view) = self.diff_mut(owner).filter(|v| v.ticket == ticket) else {
            return;
        };
        let Loaded {
            diff,
            gaps,
            highlights,
        } = match readable(result) {
            Ok(loaded) => loaded,
            Err(e) => {
                view.diff = None;
                view.error = Some(e);
                return;
            }
        };
        let same = view.diff.as_ref() == Some(&diff);
        let mut pick = None;
        if !same {
            // Selected and picked lines stay when only more lines show; when
            // the changes differ, they would point elsewhere.
            let old = view.diff.as_ref();
            let moved = old.and_then(|old| {
                let selection = follow(old, &diff, view.selection.iter().copied())?;
                let anchor = match view.anchor {
                    Some(a) => Some(follow(old, &diff, [a])?[0]),
                    None => None,
                };
                Some((selection, anchor))
            });
            let (selection, anchor) = moved.unwrap_or_default();
            view.selection = selection.into_iter().collect();
            view.anchor = anchor;
            pick = picked.map(|p| {
                old.and_then(|old| follow(old, &diff, [p.anchor, p.cursor]))
                    .map(|at| Pick {
                        anchor: at[0],
                        cursor: at[1],
                        ..p
                    })
            });
        }
        // A big diff's colors come later; keep the old ones meanwhile.
        let old = std::mem::replace(&mut view.highlights, highlights);
        if same && view.highlights.syntax.is_empty() {
            view.highlights.syntax = old.syntax;
        }
        view.diff = Some(diff);
        view.gaps = gaps;
        view.error = None;
        if let (Some(pick), Some(review)) = (pick, &mut self.review) {
            review.pick = pick;
        }
        self.color_later(owner, ticket);
    }

    /// The review's picked lines, for the review's diff of its open file.
    fn picked_in(&self, owner: DiffOwner) -> Option<Pick> {
        if owner != DiffOwner::Review {
            return None;
        }
        let review = self.review.as_ref()?;
        let view = review.diff.as_ref()?;
        let reviewed = review.selected.as_deref() == Some(view.key.path.as_str());
        review.pick.filter(|_| reviewed)
    }

    /// Reload every open diff (the "Whole file" setting changed).
    pub(super) fn reload_diffs(&mut self) {
        for owner in DiffOwner::ALL {
            self.load_view(owner);
        }
    }

    /// Syntax colors for a diff too big to color with the diff itself.
    fn color_later(&self, owner: DiffOwner, ticket: u64) {
        let Some(view) = self.diff(owner) else { return };
        let Some(diff) = &view.diff else { return };
        if !view.highlights.syntax.is_empty() || line_count(diff) <= INLINE_SYNTAX_LINES {
            return;
        }
        let diff = diff.clone();
        self.jobs.spawn(
            move || Some(graft_core::syntax::highlight(&diff)),
            move |p, syntax| {
                let Some(syntax) = syntax else { return };
                if let Some(view) = p.diff_mut(owner).filter(|v| v.ticket == ticket) {
                    view.highlights.syntax = syntax;
                }
            },
        );
    }

    /// Show more unchanged lines, `old` and `new` being their numbers on
    /// each side, in `owner`'s diff if it still shows `key`.
    pub(super) fn show_lines(
        &mut self,
        owner: DiffOwner,
        key: &FileKey,
        old: Range<u32>,
        new: Range<u32>,
    ) {
        let Some(view) = self.diff_mut(owner).filter(|v| v.key == *key) else {
            return;
        };
        // Staging a hunk above moves the index's numbers, not the working
        // tree's.
        view.revealed.push(match view.key.source {
            Source::Unstaged => (Side::New, new),
            _ => (Side::Old, old),
        });
        self.load_view(owner);
    }

    /// After a status refresh: keep the selected file if it is still in its
    /// list, follow it to the other list if it moved, or pick the first file.
    pub(super) fn sync_file_selection(&mut self) {
        let Some(status) = self.worktree().and_then(|wt| wt.status.clone()) else {
            self.diff_view = None;
            return;
        };
        let keys: Vec<FileKey> = status
            .listed()
            .map(|(list, e)| FileKey::of(e, list == FileList::Staged))
            .collect();
        let current = self.diff_view.as_ref().map(|v| v.key.clone());
        let next = match &current {
            Some(key) if keys.contains(key) => Some(key.clone()),
            Some(key) => keys
                .iter()
                .find(|k| k.path == key.path)
                .or_else(|| keys.first())
                .cloned(),
            None => keys.first().cloned(),
        };
        match next {
            Some(key) if Some(&key) == current.as_ref() => self.load_view(DiffOwner::Status),
            Some(key) => self.select_file(key),
            None => self.diff_view = None,
        }
        self.sync_conflict();
    }

    pub(super) fn click_line(&mut self, hunk: usize, line: usize, shift: bool) {
        let Some(view) = &mut self.diff_view else {
            return;
        };
        match (shift, view.anchor) {
            (true, Some(anchor)) => view.select_range(anchor, (hunk, line)),
            _ => {
                if !view.selection.remove(&(hunk, line)) {
                    view.selection.insert((hunk, line));
                }
                view.anchor = Some((hunk, line));
            }
        }
    }

    /// Dragging over lines selects everything from where the drag started.
    pub(super) fn drag_line(&mut self, hunk: usize, line: usize) {
        if let Some(view) = &mut self.diff_view
            && let Some(anchor) = view.anchor
        {
            view.select_range(anchor, (hunk, line));
        }
    }

    pub(super) fn clear_lines(&mut self) {
        if let Some(view) = &mut self.diff_view {
            view.clear_selection();
        }
    }

    /// Stage, unstage or discard a hunk (`Some(index)`) or the selected lines.
    pub(super) fn lines_op(&mut self, op: LineOp, hunk: Option<usize>) {
        let Some(view) = &self.diff_view else {
            return;
        };
        let Some(diff) = view.diff.clone() else {
            return;
        };
        let source = view.key.source.clone();
        let selection = view.selection.clone();
        let label = match op {
            LineOp::Stage => "Staging",
            LineOp::Unstage => "Unstaging",
            LineOp::Discard => "Discarding",
        };
        let job = move |git: Git| {
            ops::apply_lines(&git, &diff, &source, op, |h, l| match hunk {
                Some(only) => h == only,
                None => selection.contains(&(h, l)),
            })
        };
        if self.run_op(At::Selected, label, job, quietly)
            && let Some(view) = &mut self.diff_view
        {
            view.clear_selection();
        }
    }

    pub(super) fn stage_files(&mut self, paths: Vec<String>) {
        let job = move |git: Git| ops::stage_paths(&git, &paths);
        self.run_op(At::Selected, "Staging", job, quietly);
    }

    pub(super) fn unstage_files(&mut self, paths: Vec<String>) {
        let job = move |git: Git| ops::unstage_paths(&git, &paths);
        self.run_op(At::Selected, "Unstaging", job, quietly);
    }

    /// Discard whole files: what [`Action::discard_file`] asks for.
    pub(super) fn discard(&mut self, tracked: Vec<String>, untracked: Vec<String>) {
        let job = move |git: Git| ops::discard_paths(&git, &tracked, &untracked);
        self.run_op(At::Selected, "Discarding", job, quietly);
    }

    pub(super) fn commit(&mut self) {
        let staged = self
            .worktree()
            .and_then(|wt| wt.status.as_ref())
            .map_or(0, |s| s.staged().count());
        if !self.commit_form.ready(staged) {
            return;
        }
        let message = self.commit_form.message();
        let amend = self.commit_form.amend;
        let push = self.commit_form.push_after;
        let summary = self.commit_form.summary.trim().to_owned();
        // The follow-ups belong to this worktree even if another is
        // selected by the time the commit ends.
        self.run_op(
            At::Selected,
            if amend { "Amending" } else { "Committing" },
            move |git| ops::commit(&git, &message, amend).map(|_| ()),
            move |p, worktree, result| match result {
                Ok(()) => {
                    if same_path(&p.nav.worktree, &worktree) {
                        p.commit_form = p.commit_form.cleared();
                    }
                    p.infos.push(format!("Committed: {summary}"));
                    if push {
                        p.remote_in(worktree, RemoteOp::Push);
                    }
                }
                Err(e) => p.report(e),
            },
        );
    }

    pub(super) fn toggle_amend(&mut self) {
        let form = &mut self.commit_form;
        form.amend = !form.amend;
        if form.amend {
            form.before_amend = Some((form.summary.clone(), form.body.clone()));
            if form.summary.trim().is_empty() {
                let git = self.wt_git();
                let worktree = self.nav.worktree.clone();
                self.jobs.spawn(
                    move || ops::last_commit_message(&git),
                    move |p, message| {
                        // Another worktree's form has nothing to amend.
                        if same_path(&p.nav.worktree, &worktree) {
                            p.on_last_message(message);
                        }
                    },
                );
            }
        } else if let Some((summary, body)) = form.before_amend.take() {
            form.summary = summary;
            form.body = body;
        }
    }

    fn on_last_message(&mut self, message: anyhow::Result<Option<String>>) {
        let form = &mut self.commit_form;
        let message = match message {
            Ok(Some(message)) => message,
            Ok(None) => {
                form.amend = false;
                self.notices
                    .push("There is no commit to amend yet.".to_owned());
                return;
            }
            Err(e) => {
                self.report(e.context("Could not read the last commit message"));
                return;
            }
        };
        if !form.amend || !form.summary.trim().is_empty() {
            return;
        }
        let (summary, body) = message.split_once('\n').unwrap_or((&message, ""));
        form.summary = summary.trim().to_owned();
        form.body = body.trim().to_owned();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_file_status_stages() {
        assert_eq!(
            Staging::of(DiffOwner::Status, &Source::Untracked),
            Some(Staging::Unstaged)
        );
        assert_eq!(
            Staging::of(DiffOwner::Status, &Source::Staged),
            Some(Staging::Staged)
        );
        // Review shows untracked files too; they stay read-only there.
        assert_eq!(Staging::of(DiffOwner::Review, &Source::Untracked), None);
        assert_eq!(Staging::of(DiffOwner::Stash, &Source::Staged), None);
    }
}
