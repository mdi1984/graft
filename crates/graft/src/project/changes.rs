//! File status: the selected file's diff, line selection, staging and
//! discarding, and the commit form. Also loads the read-only diffs: each
//! screen that shows one (History, Stashes, Review, File history) holds its
//! own, see [`DiffOwner`].

use std::collections::BTreeSet;
use std::ops::Range;
use std::path::{Path, PathBuf};

use graft_core::changes::FileChange;
use graft_core::diff::{self, FileDiff, Gap, Source};
use graft_core::ops::{self, LineOp};
use graft_core::paths::same_path;
use graft_core::{EntryKind, FileList, Git, StatusEntry};

use super::comments::{Pick, Side};
use super::ops::{At, quietly};
use super::{Action, Confirm, Project, RemoteOp, readable};
use crate::text::plural;

/// Which file a diff shows, and from where: a File status list, a commit
/// or stash, or the review's base.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileKey {
    pub source: Source,
    pub path: String,
    /// The path before a rename, for a diff that shows it as one (see
    /// [`diff::load`]).
    pub orig_path: Option<String>,
}

impl FileKey {
    /// The key for a file of a commit, stash or review.
    pub fn changed(source: Source, file: &FileChange) -> Self {
        Self {
            source,
            path: file.path.clone(),
            orig_path: file.orig_path.clone(),
        }
    }

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
            orig_path: None,
        }
    }

    pub fn is_staged(&self) -> bool {
        self.source == Source::Staged
    }
}

/// Files of one File status list selected together with Ctrl or Shift,
/// in list order. The shown file is one of them. Stage, Unstage and
/// Discard (from the keyboard or a row's menu) act on all of them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FilePicks {
    pub list: FileList,
    pub keys: Vec<FileKey>,
    /// Where Shift+click starts its range: the last file Ctrl+clicked.
    anchor: FileKey,
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
    let (diff, gaps) = diff::load_expanded(
        git,
        &key.source,
        &key.path,
        key.orig_path.as_deref(),
        reveal,
        keep,
    )?;
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
    /// The worktree it shows: the one selected when it opened, or the
    /// review's. Its file is read from there, whatever is selected later.
    pub worktree: PathBuf,
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
    fn new(key: FileKey, owner: DiffOwner, worktree: PathBuf) -> Self {
        Self {
            staging: Staging::of(owner, &key.source),
            owner,
            key,
            worktree,
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

/// Lines of File status's diff to stage, unstage or discard, with the diff
/// they were picked from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LineChange {
    op: LineOp,
    key: FileKey,
    diff: FileDiff,
    /// One hunk, or else the selected lines.
    hunk: Option<usize>,
    selection: BTreeSet<(usize, usize)>,
}

impl DiffView {
    /// `op` on hunk `hunk`, or on the selected lines with `None`.
    pub fn change_lines(&self, op: LineOp, hunk: Option<usize>) -> Option<Action> {
        Some(Action::ChangeLines(Box::new(LineChange {
            op,
            key: self.key.clone(),
            diff: self.diff.clone()?,
            hunk,
            selection: self.selection.clone(),
        })))
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
    /// Ask before discarding a whole file (the diff's Discard…) in
    /// `worktree`, see [`Project::ask_discard`].
    pub fn discard_file(worktree: &Path, source: &Source, path: String) -> Action {
        Action::discard_files(
            worktree,
            vec![FileKey {
                source: source.clone(),
                path,
                orig_path: None,
            }],
        )
    }

    /// Moving `paths` of `list` to the other list: unstaged files are
    /// staged, staged ones unstaged. Conflicts move by being resolved
    /// instead.
    pub fn toggle_files(list: FileList, paths: Vec<String>) -> Option<Action> {
        match list {
            FileList::Unstaged => Some(Action::StageFiles(paths)),
            FileList::Staged => Some(Action::UnstageFiles(paths)),
            FileList::Conflicts => None,
        }
    }

    /// Ask before discarding whole files of the unstaged list (File
    /// status) in `worktree`, see [`Project::ask_discard`].
    pub fn discard_files(worktree: &Path, files: Vec<FileKey>) -> Action {
        Action::AskDiscard {
            worktree: worktree.to_path_buf(),
            files,
        }
    }
}

impl Confirm {
    /// Enter answers yes. Only for discarding: the user asked for that
    /// question a moment ago with a click or a key. Other questions can
    /// come up while they type (a job's follow-up), so a stray Enter must
    /// not answer them.
    pub fn enter_confirms(&self) -> bool {
        match &self.action {
            Action::Discard { .. } => true,
            Action::ChangeLines(change) => change.op == LineOp::Discard,
            _ => false,
        }
    }
}

/// At most this many file names in the question before discarding many.
const NAMED_FILES: usize = 6;

/// The question before discarding whole files; a new one goes to the
/// Recycle Bin if the worktree has one, else it is deleted for good.
fn discard_question(recycle_bin: bool, files: &[FileKey]) -> Confirm {
    let (untracked, tracked): (Vec<&FileKey>, Vec<&FileKey>) = files
        .iter()
        .partition(|key| key.source == Source::Untracked);
    let paths = |keys: &[&FileKey]| keys.iter().map(|k| k.path.clone()).collect::<Vec<_>>();
    let no_bin = "Windows has no Recycle Bin inside WSL or on a network share or drive.";
    let (title, body, confirm_label) = match files {
        [one] if one.source != Source::Untracked => (
            "Discard changes?".to_owned(),
            format!(
                "All unstaged changes to {} will be lost. Staged changes stay.",
                one.path
            ),
            "Discard",
        ),
        [one] if recycle_bin => (
            "Discard changes?".to_owned(),
            format!(
                "{} is not tracked by git. It will be moved to the Recycle Bin.",
                one.path
            ),
            "Discard",
        ),
        [one] => (
            "Delete the file for good?".to_owned(),
            format!(
                "{} is not tracked by git. It will be deleted permanently: {no_bin}",
                one.path
            ),
            "Delete permanently",
        ),
        _ => {
            let mut names: Vec<&str> = files
                .iter()
                .take(NAMED_FILES)
                .map(|k| k.path.as_str())
                .collect();
            let more = files.len().saturating_sub(NAMED_FILES);
            let more = format!("… and {more} more");
            if files.len() > NAMED_FILES {
                names.push(&more);
            }
            let mut lines = vec![names.join("\n"), String::new()];
            if !tracked.is_empty() {
                lines.push(format!(
                    "All unstaged changes to {} will be lost. Staged changes stay.",
                    plural(tracked.len(), "tracked file", "tracked files")
                ));
            }
            if !untracked.is_empty() {
                let new = plural(untracked.len(), "new file", "new files");
                lines.push(if recycle_bin {
                    format!("{new} not tracked by git will be moved to the Recycle Bin.")
                } else {
                    format!("{new} not tracked by git will be deleted permanently: {no_bin}")
                });
            }
            let label = match (recycle_bin || untracked.is_empty(), tracked.is_empty()) {
                (true, _) => "Discard",
                (false, true) => "Delete permanently",
                (false, false) => "Discard and delete",
            };
            (
                format!("Discard changes to {} files?", files.len()),
                lines.join("\n"),
                label,
            )
        }
    };
    Confirm {
        title,
        body,
        confirm_label: confirm_label.to_owned(),
        action: Action::Discard {
            tracked: paths(&tracked),
            untracked: paths(&untracked),
        },
    }
}

impl Project {
    /// Ask before discarding `path` in `worktree`; confirmed,
    /// [`Project::discard`] runs it. Whether a new file goes to the Recycle
    /// Bin is the snapshot's answer (see [`graft_core::Worktree::recycle_bin`]):
    /// finding out takes the disk, which the UI thread never touches.
    pub(super) fn ask_discard(&mut self, worktree: &Path, files: &[FileKey]) {
        if files.is_empty() {
            return;
        }
        let recycle_bin = self
            .snapshot
            .as_ref()
            .and_then(|s| s.worktree_at(worktree))
            .is_some_and(|wt| wt.recycle_bin);
        self.ask(discard_question(recycle_bin, files));
    }

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
        self.file_picks = None;
        self.open_diff(DiffOwner::Status, key);
        self.sync_conflict();
    }

    /// The rows of `list` in File status, top to bottom.
    fn list_keys(&self, list: FileList) -> Vec<FileKey> {
        let Some(status) = self.worktree().and_then(|wt| wt.status.as_ref()) else {
            return Vec::new();
        };
        status
            .in_list(list)
            .map(|e| FileKey::of(e, list == FileList::Staged))
            .collect()
    }

    /// The files of `list` selected now: the picked ones, or the shown
    /// file if it is in `list`.
    fn selected_in(&self, list: FileList, rows: &[FileKey]) -> (Vec<FileKey>, Option<FileKey>) {
        if let Some(picks) = self.file_picks.as_ref().filter(|p| p.list == list) {
            return (picks.keys.clone(), Some(picks.anchor.clone()));
        }
        let shown = self.diff_view.as_ref().map(|v| &v.key);
        match shown.filter(|key| rows.contains(key)) {
            Some(key) => (vec![key.clone()], Some(key.clone())),
            None => (Vec::new(), None),
        }
    }

    /// Ctrl+click (`range` false) or Shift+click (`range` true) on `key`
    /// of `list`. Picks stay in one list, and not in Conflicts: there, or
    /// in another list than the picked files, it selects `key` alone.
    pub(super) fn pick_file(&mut self, list: FileList, key: FileKey, range: bool) {
        if list == FileList::Conflicts {
            return self.select_file(key);
        }
        let rows = self.list_keys(list);
        let (mut keys, anchor) = self.selected_in(list, &rows);
        let Some(at) = rows.iter().position(|k| *k == key) else {
            return;
        };
        if keys.is_empty() {
            return self.select_file(key);
        }
        let mut anchor = anchor.unwrap_or_else(|| key.clone());
        let mut shown = key.clone();
        if range {
            let from = rows.iter().position(|k| *k == anchor).unwrap_or(at);
            keys = rows[from.min(at)..=from.max(at)].to_vec();
        } else if let Some(i) = keys.iter().position(|k| *k == key) {
            if keys.len() == 1 {
                return;
            }
            keys.remove(i);
            // The next Shift+click starts here, picked or not.
            anchor = key.clone();
            // The shown file stays, unless it was the one taken out.
            let current = self.diff_view.as_ref().map(|v| &v.key);
            shown = match current.filter(|k| keys.contains(k)) {
                Some(current) => current.clone(),
                None => keys.last().cloned().expect("more than one"),
            };
        } else {
            keys.push(key.clone());
            keys.sort_by_key(|k| rows.iter().position(|r| r == k));
            anchor = key.clone();
        }
        self.open_diff(DiffOwner::Status, shown);
        self.sync_conflict();
        self.file_picks = (keys.len() > 1).then_some(FilePicks { list, keys, anchor });
    }

    /// Whether `key` of `list` shows as selected besides the shown file.
    pub fn is_picked(&self, list: FileList, key: &FileKey) -> bool {
        self.file_picks
            .as_ref()
            .is_some_and(|p| p.list == list && p.keys.contains(key))
    }

    /// The files a command on `key` of `list` acts on: every picked file
    /// if `key` is one of them, else `key` alone.
    pub fn file_targets(&self, list: FileList, key: &FileKey) -> Vec<FileKey> {
        match &self.file_picks {
            Some(p) if p.list == list && p.keys.contains(key) => p.keys.clone(),
            _ => vec![key.clone()],
        }
    }

    /// What Stage (in Unstaged) or Unstage (in Staged) does for `key` of
    /// `list`, from its menu or the keyboard: all picked files if `key` is
    /// one of them. Conflicts move by being resolved instead.
    pub fn toggle_files(&self, list: FileList, key: &FileKey) -> Option<Action> {
        let paths = self
            .file_targets(list, key)
            .into_iter()
            .map(|k| k.path)
            .collect();
        Action::toggle_files(list, paths)
    }

    /// What Discard does for `key` of `list`, from its menu or the
    /// keyboard: ask about all picked files if `key` is one of them. Only
    /// unstaged changes are discarded.
    pub fn discard_files(&self, list: FileList, key: &FileKey) -> Option<Action> {
        (list == FileList::Unstaged)
            .then(|| Action::discard_files(&self.nav.worktree, self.file_targets(list, key)))
    }

    /// After a status refresh: picked files that left their list are
    /// unpicked; one left over is just the shown file.
    fn sync_file_picks(&mut self) {
        let Some(picks) = &self.file_picks else {
            return;
        };
        let rows = self.list_keys(picks.list);
        let shown = self.diff_view.as_ref().map(|v| &v.key);
        let keys: Vec<FileKey> = picks
            .keys
            .iter()
            .filter(|k| rows.contains(k))
            .cloned()
            .collect();
        let anchor = Some(&picks.anchor)
            .filter(|a| keys.contains(a))
            .or(shown)
            .cloned();
        self.file_picks = match anchor {
            Some(anchor) if keys.len() > 1 && shown.is_some_and(|s| keys.contains(s)) => {
                Some(FilePicks {
                    list: picks.list,
                    keys,
                    anchor,
                })
            }
            _ => None,
        };
    }

    /// Show `key` in `owner`'s diff, unless it shows it already. The diff
    /// shows the review's worktree on Review, else the selected one.
    pub(super) fn open_diff(&mut self, owner: DiffOwner, key: FileKey) {
        let review = self.review.as_ref().filter(|_| owner == DiffOwner::Review);
        let worktree = review.map_or(&self.nav.worktree, |r| &r.worktree).clone();
        let Some(slot) = self.diff_slot(owner) else {
            return;
        };
        if slot
            .as_ref()
            .is_some_and(|v| v.key == key && same_path(&v.worktree, &worktree))
        {
            return;
        }
        *slot = Some(DiffView::new(key, owner, worktree));
        self.load_view(owner);
    }

    /// (Re)load `owner`'s diff; what is selected or picked in it survives
    /// if only the unchanged lines shown change.
    pub(super) fn load_view(&mut self, owner: DiffOwner) {
        let Some((key, worktree)) = self
            .diff(owner)
            .map(|v| (v.key.clone(), v.worktree.clone()))
        else {
            return;
        };
        let ticket = self.ticket();
        let whole = self.settings.whole_file;
        // Commits are the same everywhere; the rest reads the diff's worktree.
        let git = match key.source {
            Source::Commit(_) => self.git.clone(),
            _ => self.git.at(&worktree),
        };
        let kept = self.comment_lines(&key, &worktree);
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
            self.file_picks = None;
            return;
        };
        let keys: Vec<FileKey> = status
            .listed()
            .map(|(list, e)| FileKey::of(e, list == FileList::Staged))
            .collect();
        let current = self.diff_view.as_ref().map(|v| v.key.clone());
        // The shown file left its list (staged with its button, say):
        // show another picked file and keep the picks.
        let picked = self
            .file_picks
            .as_ref()
            .and_then(|p| p.keys.iter().find(|k| keys.contains(k)))
            .cloned();
        let next = match &current {
            Some(key) if keys.contains(key) => Some(key.clone()),
            Some(key) => picked.clone().or_else(|| {
                keys.iter()
                    .find(|k| k.path == key.path)
                    .or_else(|| keys.first())
                    .cloned()
            }),
            None => keys.first().cloned(),
        };
        match next {
            Some(key) if Some(&key) == current.as_ref() => self.load_view(DiffOwner::Status),
            Some(key) if Some(&key) == picked.as_ref() => self.open_diff(DiffOwner::Status, key),
            Some(key) => self.select_file(key),
            None => self.diff_view = None,
        }
        self.sync_file_picks();
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

    /// Stage, unstage or discard lines, if File status still shows the diff
    /// they were picked from: a refresh or another file while a Discard
    /// dialog was open must not discard something else.
    pub(super) fn change_lines(&mut self, change: LineChange) {
        let LineChange {
            op,
            key,
            diff,
            hunk,
            selection,
        } = change;
        let (label, done) = match op {
            LineOp::Stage => ("Staging", "staged"),
            LineOp::Unstage => ("Unstaging", "unstaged"),
            LineOp::Discard => ("Discarding", "discarded"),
        };
        let unchanged = self
            .diff_view
            .as_ref()
            .is_some_and(|v| v.key == key && v.diff.as_ref() == Some(&diff));
        if !unchanged {
            self.notices.push(format!(
                "{} changed in the meantime, so nothing was {done}. Check the diff and try again.",
                key.path
            ));
            return;
        }
        let job = move |git: Git| {
            ops::apply_lines(&git, &diff, &key.source, op, |h, l| match hunk {
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
        self.run_head_op(
            At::Selected,
            if amend { "Amending" } else { "Committing" },
            move |git| ops::commit(&git, &message, amend).map(|_| ()),
            move |p, worktree, result| match result {
                Ok(()) => {
                    p.commits_made += 1;
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

    /// Ask the AI command for a message for what is staged in the selected
    /// worktree. The answer fills the commit box only if that worktree is
    /// still selected and nothing was typed meanwhile.
    pub(super) fn write_commit_message(&mut self) {
        if let Err(why) = self.can_write_message() {
            self.notices.push(why);
            return;
        }
        let command = self.settings.ai_command.clone();
        let git = self.wt_git();
        let worktree = self.nav.worktree.clone();
        let typed = (
            self.commit_form.summary.clone(),
            self.commit_form.body.clone(),
        );
        let commits_made = self.commits_made;
        self.writing_message = Some(worktree.clone());
        // It can take minutes: on a thread of its own, not the pool's.
        self.jobs.spawn_long(
            move |_| {
                let diff = ops::staged_diff(&git)?;
                let recent = ops::recent_summaries(&git, crate::ai_message::STYLE_EXAMPLES)?;
                crate::ai_message::write(&command, git.cwd(), &diff, &recent)
            },
            move |p, result| {
                p.writing_message = None;
                let message = match result {
                    Ok(message) => message,
                    Err(e) => {
                        p.report(e.context("Could not write a commit message"));
                        return;
                    }
                };
                let form = &mut p.commit_form;
                if !same_path(&p.nav.worktree, &worktree) {
                    p.notices.push(
                        "The commit message was for another worktree, so it was not used."
                            .to_owned(),
                    );
                } else if p.commits_made != commits_made {
                    p.notices.push(
                        "You committed meanwhile, so the written message was not used.".to_owned(),
                    );
                } else if (form.summary.clone(), form.body.clone()) != typed {
                    p.notices.push(
                        "You changed the commit message meanwhile, so Graft kept yours.".to_owned(),
                    );
                } else {
                    form.summary = message.summary;
                    form.body = message.body;
                }
            },
        );
    }

    /// Whether Write with AI can start now, and if not, why.
    pub fn can_write_message(&self) -> Result<(), String> {
        let staged = self
            .worktree()
            .and_then(|wt| wt.status.as_ref())
            .map_or(0, |s| s.staged().count());
        if !self.settings.ai_messages {
            Err("Turn it on in Settings › General first".to_owned())
        } else if self.writing_message.is_some() {
            Err("Already writing one".to_owned())
        } else if self.settings.ai_command.trim().is_empty() {
            Err("Set the command in Settings › General first".to_owned())
        } else if staged == 0 {
            Err("Stage the changes to describe first".to_owned())
        } else {
            Ok(())
        }
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
    use crate::testing::{git, open, wait};

    /// Write with AI fills the commit box with what the command printed,
    /// only with something staged, and never over what the user typed
    /// meanwhile. `git rev-parse --show-toplevel` stands in for the AI
    /// command, so the answer shows the folder it ran in.
    #[test]
    fn write_with_ai_fills_the_commit_box_from_the_command() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("app");
        std::fs::create_dir(&root).unwrap();
        git(&root, &["init", "-q", "-b", "main"]);
        // Graft's own commit runs without the test identity.
        git(&root, &["config", "user.name", "Test"]);
        git(&root, &["config", "user.email", "test@example.com"]);
        std::fs::write(root.join("a.txt"), "a\n").unwrap();
        let mut project = open(&root);
        let settings = crate::settings::Settings {
            ai_messages: true,
            ai_command: "git rev-parse --show-toplevel".to_owned(),
            ..Default::default()
        };
        project.set_settings(&settings);

        // Nothing staged: it says so and starts nothing.
        project.apply(Action::WriteCommitMessage);
        assert!(project.writing_message.is_none());
        assert_eq!(
            project.notices.pop().as_deref(),
            Some("Stage the changes to describe first")
        );

        git(&root, &["add", "a.txt"]);
        project.refresh();
        wait(&mut project, "a.txt never showed staged", |p| {
            p.can_write_message().is_ok()
        });
        project.apply(Action::WriteCommitMessage);
        assert!(project.writing_message.is_some());
        assert!(project.can_write_message().is_err(), "one at a time");
        wait(&mut project, "no message came", |p| {
            p.writing_message.is_none()
        });
        // It ran in the worktree's folder.
        let top = std::fs::canonicalize(&project.commit_form.summary).unwrap();
        assert_eq!(top, std::fs::canonicalize(&root).unwrap());

        // A commit made while it ran: its message is not put in. The command
        // says "started" once Graft has read the staged changes and answers
        // only once the test creates "go", after the commit ended.
        let script = tmp.path().join("slow.cmd");
        std::fs::write(
            &script,
            "@echo off\r\necho.> \"%~dp0started\"\r\n:wait\r\nif exist \"%~dp0go\" goto done\r\n\
             ping -n 2 127.0.0.1 >nul\r\ngoto wait\r\n:done\r\necho Late message\r\n",
        )
        .unwrap();
        let slow = crate::settings::Settings {
            ai_command: format!("\"{}\"", script.display()),
            ..settings.clone()
        };
        project.set_settings(&slow);
        project.commit_form = CommitForm::default();
        project.apply(Action::WriteCommitMessage);
        let started = tmp.path().join("started");
        wait(&mut project, "the command never started", |_| {
            started.exists()
        });
        project.commit_form.summary = "By hand".to_owned();
        project.apply(Action::Commit);
        wait(&mut project, "the commit never ended", |p| {
            !p.is_operating()
        });
        assert!(project.notices.is_empty(), "{:?}", project.notices);
        assert!(project.writing_message.is_some());
        std::fs::write(tmp.path().join("go"), "").unwrap();
        wait(&mut project, "no message came", |p| {
            p.writing_message.is_none()
        });
        assert_eq!(project.commit_form.summary, "");
        assert!(
            project
                .notices
                .iter()
                .any(|n| n.contains("committed meanwhile")),
            "{:?}",
            project.notices
        );
        project.set_settings(&settings);
        std::fs::write(root.join("a.txt"), "a2\n").unwrap();
        git(&root, &["add", "a.txt"]);
        project.refresh();
        wait(&mut project, "a.txt never showed staged", |p| {
            p.can_write_message().is_ok()
        });

        // Typed while it ran: the typed message stays.
        project.commit_form = CommitForm::default();
        project.apply(Action::WriteCommitMessage);
        project.commit_form.summary = "Mine".to_owned();
        wait(&mut project, "no message came", |p| {
            p.writing_message.is_none()
        });
        assert_eq!(project.commit_form.summary, "Mine");
        assert!(project.notices.iter().any(|n| n.contains("kept yours")));

        // Another worktree picked while it ran: its box stays as it was.
        let other = tmp.path().join("other");
        git(&root, &["commit", "-q", "-m", "first"]);
        git(
            &root,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                "other",
                other.to_str().unwrap(),
            ],
        );
        std::fs::write(root.join("b.txt"), "b\n").unwrap();
        git(&root, &["add", "b.txt"]);
        project.refresh();
        wait(&mut project, "b.txt never showed staged", |p| {
            p.snapshot.as_ref().is_some_and(|s| s.worktrees.len() == 2)
                && p.can_write_message().is_ok()
        });
        project.commit_form = CommitForm::default();
        project.notices.clear();
        project.apply(Action::WriteCommitMessage);
        let other = project
            .snapshot
            .as_ref()
            .unwrap()
            .worktrees
            .iter()
            .find(|wt| !wt.is_main)
            .unwrap()
            .path
            .clone();
        project.apply(Action::OpenWorktree(other));
        wait(&mut project, "no message came", |p| {
            p.writing_message.is_none()
        });
        assert_eq!(project.commit_form.summary, "");
        assert!(
            project
                .notices
                .iter()
                .any(|n| n.contains("another worktree"))
        );
    }

    /// A failing command becomes a notice that says why; with the feature
    /// off, nothing runs.
    #[test]
    fn write_with_ai_says_why_it_failed_or_cannot_run() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_path_buf();
        git(&root, &["init", "-q", "-b", "main"]);
        std::fs::write(root.join("a.txt"), "a\n").unwrap();
        git(&root, &["add", "a.txt"]);
        let mut project = open(&root);
        wait(&mut project, "a.txt never showed staged", |p| {
            p.worktree()
                .and_then(|wt| wt.status.as_ref())
                .is_some_and(|s| s.staged().count() == 1)
        });

        project.apply(Action::WriteCommitMessage);
        assert!(project.writing_message.is_none(), "off by default");
        assert!(project.notices.pop().unwrap().contains("Turn it on"));

        let settings = crate::settings::Settings {
            ai_messages: true,
            ai_command: "git no-such-command".to_owned(),
            ..Default::default()
        };
        project.set_settings(&settings);
        project.apply(Action::WriteCommitMessage);
        wait(&mut project, "the command never ended", |p| {
            p.writing_message.is_none()
        });
        let notice = project.notices.join("\n");
        assert!(
            notice.contains("Could not write a commit message"),
            "{notice}"
        );
        assert!(notice.contains("failed"), "{notice}");
        assert!(notice.contains("not a git command"), "{notice}");
        assert_eq!(project.commit_form.summary, "");
    }

    /// Discarding a new file asks to move it to the Recycle Bin, or, where
    /// Windows has none, says it is deleted for good.
    #[test]
    fn discarding_a_new_file_says_where_it_goes() {
        let file = FileKey {
            source: Source::Untracked,
            path: "new.txt".into(),
            orig_path: None,
        };
        let ask = |recycle_bin| discard_question(recycle_bin, std::slice::from_ref(&file));
        let local = ask(true);
        assert!(local.body.contains("Recycle Bin"), "{}", local.body);
        assert!(!local.body.contains("permanently"), "{}", local.body);
        let remote = ask(false);
        assert!(
            remote.body.contains("deleted permanently"),
            "{}",
            remote.body
        );
        assert_eq!(remote.confirm_label, "Delete permanently");
    }

    fn unstaged(path: &str) -> FileKey {
        FileKey {
            source: Source::Unstaged,
            path: path.to_owned(),
            orig_path: None,
        }
    }

    fn untracked(path: &str) -> FileKey {
        FileKey {
            source: Source::Untracked,
            ..unstaged(path)
        }
    }

    fn paths_of(keys: &[FileKey]) -> Vec<String> {
        keys.iter().map(|k| k.path.clone()).collect()
    }

    /// Discarding many files asks once: it names them (the first few),
    /// says what happens to tracked and to new files, and discards each
    /// the right way.
    #[test]
    fn discarding_many_files_asks_once() {
        let mut files: Vec<FileKey> = (1..=7).map(|n| unstaged(&format!("f{n}.txt"))).collect();
        files.push(untracked("new.txt"));
        let local = discard_question(true, &files);
        assert_eq!(local.title, "Discard changes to 8 files?");
        assert!(local.body.contains("f1.txt\nf2.txt"), "{}", local.body);
        assert!(!local.body.contains("f7.txt"), "{}", local.body);
        assert!(local.body.contains("… and 2 more"), "{}", local.body);
        assert!(local.body.contains("7 tracked files"), "{}", local.body);
        assert!(local.body.contains("1 new file "), "{}", local.body);
        assert!(local.body.contains("Recycle Bin"), "{}", local.body);
        assert_eq!(local.confirm_label, "Discard");
        let Action::Discard {
            tracked,
            untracked: new,
        } = &local.action
        else {
            panic!("{:?}", local.action);
        };
        assert_eq!(tracked.len(), 7);
        assert_eq!(new, &["new.txt".to_owned()]);

        let remote = discard_question(false, &files);
        assert!(
            remote.body.contains("deleted permanently"),
            "{}",
            remote.body
        );
        assert_eq!(remote.confirm_label, "Discard and delete");
        let new_only = discard_question(false, &[untracked("a"), untracked("b")]);
        assert_eq!(new_only.confirm_label, "Delete permanently");
        assert!(
            !new_only.body.contains("tracked files"),
            "{}",
            new_only.body
        );
    }

    /// Enter answers the question before discarding, and no other: other
    /// questions can come up while the user types.
    #[test]
    fn enter_confirms_only_discarding() {
        assert!(discard_question(true, &[unstaged("a.txt")]).enter_confirms());
        assert!(discard_question(false, &[untracked("a"), unstaged("b")]).enter_confirms());
        let other = Confirm {
            title: "Force push?".to_owned(),
            body: String::new(),
            confirm_label: "Force push".to_owned(),
            action: Action::Refresh,
        };
        assert!(!other.enter_confirms());
    }

    /// Ctrl+click and Shift+click select more files of one list; Stage
    /// and Discard on one of them act on all; a file of another list, or
    /// a plain click, selects one file again.
    #[test]
    fn ctrl_and_shift_click_select_files_of_one_list() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        git(root, &["init", "-q", "-b", "main"]);
        for name in ["a", "b", "c", "d", "s"] {
            std::fs::write(root.join(format!("{name}.txt")), "1\n").unwrap();
        }
        git(root, &["add", "."]);
        git(root, &["commit", "-q", "-m", "first"]);
        for name in ["a", "b", "c", "d", "s"] {
            std::fs::write(root.join(format!("{name}.txt")), "2\n").unwrap();
        }
        git(root, &["add", "s.txt"]);
        let mut project = open(root);
        let pick = |p: &mut Project, path: &str, range| {
            p.apply(Action::PickFile {
                list: FileList::Unstaged,
                key: unstaged(path),
                range,
            })
        };
        let picked = |p: &Project| {
            p.file_picks
                .as_ref()
                .map(|picks| paths_of(&picks.keys))
                .unwrap_or_default()
        };
        let shown = |p: &Project| p.diff_view.as_ref().unwrap().key.path.clone();
        let targets = |p: &Project, path: &str| {
            paths_of(&p.file_targets(FileList::Unstaged, &unstaged(path)))
        };

        project.apply(Action::SelectFile(unstaged("a.txt")));
        pick(&mut project, "c.txt", false);
        assert_eq!(picked(&project), ["a.txt", "c.txt"]);
        assert_eq!(shown(&project), "c.txt");
        assert!(project.is_picked(FileList::Unstaged, &unstaged("a.txt")));
        assert!(!project.is_picked(FileList::Unstaged, &unstaged("b.txt")));
        assert!(!project.is_picked(FileList::Staged, &unstaged("a.txt")));
        assert_eq!(targets(&project, "a.txt"), ["a.txt", "c.txt"]);
        assert_eq!(targets(&project, "d.txt"), ["d.txt"]);
        // Shift+click: from the last Ctrl+clicked file.
        pick(&mut project, "a.txt", true);
        assert_eq!(picked(&project), ["a.txt", "b.txt", "c.txt"]);
        assert_eq!(shown(&project), "a.txt");
        // Ctrl+click takes the shown file out and shows another.
        pick(&mut project, "a.txt", false);
        assert_eq!(picked(&project), ["b.txt", "c.txt"]);
        assert!(["b.txt", "c.txt"].contains(&shown(&project).as_str()));

        // Staging one file unpicks it: one is left, just the shown file.
        git(root, &["add", "b.txt"]);
        let snap = graft_core::snapshot::load(&project.git).unwrap();
        project.take_snapshot(snap);
        assert!(project.file_picks.is_none(), "{:?}", project.file_picks);

        // A plain click selects one file again.
        project.apply(Action::SelectFile(unstaged("c.txt")));
        pick(&mut project, "d.txt", false);
        assert_eq!(picked(&project), ["c.txt", "d.txt"]);
        project.apply(Action::SelectFile(unstaged("a.txt")));
        assert!(project.file_picks.is_none());

        // A file of another list selects that one alone.
        pick(&mut project, "d.txt", false);
        assert_eq!(picked(&project), ["a.txt", "d.txt"]);
        project.apply(Action::PickFile {
            list: FileList::Staged,
            key: FileKey {
                source: Source::Staged,
                ..unstaged("s.txt")
            },
            range: false,
        });
        assert!(project.file_picks.is_none());
        assert_eq!(shown(&project), "s.txt");

        // Discard on picked files discards them all, once confirmed.
        project.apply(Action::SelectFile(unstaged("c.txt")));
        pick(&mut project, "d.txt", false);
        let files = project.file_targets(FileList::Unstaged, &unstaged("d.txt"));
        let worktree = project.nav.worktree.clone();
        project.apply(Action::discard_files(&worktree, files));
        let title = &project.confirm().unwrap().title;
        assert_eq!(title, "Discard changes to 2 files?");
        project.apply(Action::ConfirmYes);
        wait(&mut project, "discarding never ended", |p| {
            !p.is_operating()
        });
        for name in ["c", "d"] {
            let text = std::fs::read_to_string(root.join(format!("{name}.txt"))).unwrap();
            assert_eq!(text.trim_end(), "1", "{name}.txt discarded");
        }
        let a = std::fs::read_to_string(root.join("a.txt")).unwrap();
        assert_eq!(a.trim_end(), "2", "a.txt was not picked");
    }

    /// Picked files keep their places: Ctrl+click on a picked file that is
    /// not shown keeps the shown one, and the next Shift+click starts at
    /// the Ctrl+clicked file. Staging the shown file with its button
    /// leaves the other picks picked, one of them shown. Stage and
    /// Discard from the keyboard or a menu act on every picked file.
    #[test]
    fn picks_keep_the_shown_file_the_anchor_and_survive_a_refresh() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        git(root, &["init", "-q", "-b", "main"]);
        for name in ["a", "b", "c", "d"] {
            std::fs::write(root.join(format!("{name}.txt")), "1\n").unwrap();
        }
        git(root, &["add", "."]);
        git(root, &["commit", "-q", "-m", "first"]);
        for name in ["a", "b", "c", "d"] {
            std::fs::write(root.join(format!("{name}.txt")), "2\n").unwrap();
        }
        let mut project = open(root);
        let pick = |p: &mut Project, path: &str, range| {
            p.apply(Action::PickFile {
                list: FileList::Unstaged,
                key: unstaged(path),
                range,
            })
        };
        let picked = |p: &Project| {
            p.file_picks
                .as_ref()
                .map(|picks| paths_of(&picks.keys))
                .unwrap_or_default()
        };
        let shown = |p: &Project| p.diff_view.as_ref().unwrap().key.path.clone();

        // a to d picked, a shown.
        project.apply(Action::SelectFile(unstaged("d.txt")));
        pick(&mut project, "a.txt", true);
        assert_eq!(picked(&project), ["a.txt", "b.txt", "c.txt", "d.txt"]);
        assert_eq!(shown(&project), "a.txt");
        pick(&mut project, "b.txt", false);
        assert_eq!(picked(&project), ["a.txt", "c.txt", "d.txt"]);
        assert_eq!(shown(&project), "a.txt", "the shown file stays");
        pick(&mut project, "c.txt", true);
        assert_eq!(picked(&project), ["b.txt", "c.txt"], "from b");

        // Stage, Unstage and Discard act on the picks.
        let b = unstaged("b.txt");
        let Some(Action::StageFiles(paths)) = project.toggle_files(FileList::Unstaged, &b) else {
            panic!("stages");
        };
        assert_eq!(paths, ["b.txt", "c.txt"]);
        let Some(Action::AskDiscard { files, .. }) = project.discard_files(FileList::Unstaged, &b)
        else {
            panic!("asks before discarding");
        };
        assert_eq!(paths_of(&files), ["b.txt", "c.txt"]);
        let staged = FileKey {
            source: Source::Staged,
            ..b.clone()
        };
        assert!(project.discard_files(FileList::Staged, &staged).is_none());
        let d = unstaged("d.txt");
        let Some(Action::StageFiles(paths)) = project.toggle_files(FileList::Unstaged, &d) else {
            panic!("stages");
        };
        assert_eq!(paths, ["d.txt"], "d is not picked");

        // Stage the shown file alone: the other picks stay, one shown.
        pick(&mut project, "d.txt", false);
        assert_eq!(picked(&project), ["b.txt", "c.txt", "d.txt"]);
        assert_eq!(shown(&project), "d.txt");
        git(root, &["add", "d.txt"]);
        let snap = graft_core::snapshot::load(&project.git).unwrap();
        project.take_snapshot(snap);
        assert_eq!(picked(&project), ["b.txt", "c.txt"]);
        assert!(["b.txt", "c.txt"].contains(&shown(&project).as_str()));

        // Conflicts never pick more than one file.
        project.apply(Action::PickFile {
            list: FileList::Conflicts,
            key: unstaged("a.txt"),
            range: false,
        });
        assert!(project.file_picks.is_none());
    }

    /// Discard… asks with the snapshot's answer for the worktree: a local
    /// folder has a Recycle Bin.
    #[test]
    fn discard_asks_with_the_worktrees_recycle_bin() {
        let dir = tempfile::tempdir().unwrap();
        git(dir.path(), &["init", "-q", "-b", "main"]);
        std::fs::write(dir.path().join("new.txt"), "new\n").unwrap();
        let mut project = open(dir.path());
        assert!(project.worktree().unwrap().recycle_bin);
        let worktree = project.nav.worktree.clone();
        project.apply(Action::discard_file(
            &worktree,
            &Source::Untracked,
            "new.txt".to_owned(),
        ));
        let (confirm, _) = project.confirm.as_ref().unwrap();
        assert!(confirm.body.contains("Recycle Bin"), "{}", confirm.body);
    }

    fn porcelain(dir: &std::path::Path) -> String {
        let out = std::process::Command::new("git")
            .args(["status", "--porcelain"])
            .current_dir(dir)
            .output()
            .unwrap();
        String::from_utf8(out.stdout).unwrap()
    }

    fn numbered(changed: &[usize]) -> String {
        (1..=20)
            .map(|n| match changed.contains(&n) {
                true => format!("changed {n}\n"),
                false => format!("line {n}\n"),
            })
            .collect()
    }

    #[test]
    fn a_hunk_is_only_discarded_while_its_diff_is_shown() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let file = root.join("f.txt");
        git(root, &["init", "-q", "-b", "main"]);
        git(root, &["config", "core.autocrlf", "false"]);
        std::fs::write(&file, numbered(&[])).unwrap();
        git(root, &["add", "f.txt"]);
        git(root, &["commit", "-q", "-m", "first"]);
        std::fs::write(&file, numbered(&[2, 18])).unwrap();
        let mut project = open(root);
        let key = FileKey {
            source: Source::Unstaged,
            path: "f.txt".to_owned(),
            orig_path: None,
        };
        project.apply(Action::SelectFile(key));
        let hunks = |p: &Project| p.diff_view.as_ref()?.diff.as_ref().map(|d| d.hunks.len());
        wait(&mut project, "the diff never came", |p| hunks(p) == Some(2));
        let stale = project.diff_view.as_ref().unwrap();
        // Enter answers the question before discarding a hunk, not before
        // staging one (which asks nothing anyway).
        let question = |action| Confirm {
            title: String::new(),
            body: String::new(),
            confirm_label: String::new(),
            action,
        };
        let stage = stale.change_lines(LineOp::Stage, Some(1)).unwrap();
        assert!(!question(stage).enter_confirms());
        let stale = stale.change_lines(LineOp::Discard, Some(1)).unwrap();
        assert!(question(stale.clone()).enter_confirms());

        // The file changes while the dialog is open: the hunk asked about
        // is now a different one.
        std::fs::write(&file, numbered(&[2, 10, 18])).unwrap();
        project.load_view(DiffOwner::Status);
        wait(&mut project, "the diff never reloaded", |p| {
            hunks(p) == Some(3)
        });
        project.apply(stale);
        assert!(!project.is_operating());
        assert_eq!(project.notices.len(), 1, "{:?}", project.notices);
        assert!(project.notices[0].contains("nothing was discarded"));
        assert_eq!(
            std::fs::read_to_string(&file).unwrap(),
            numbered(&[2, 10, 18])
        );

        // Asked again, it goes.
        let view = project.diff_view.as_ref().unwrap();
        let fresh = view.change_lines(LineOp::Discard, Some(2)).unwrap();
        project.apply(fresh);
        wait(&mut project, "discarding never ended", |p| {
            !p.is_operating()
        });
        assert_eq!(project.notices.len(), 1, "{:?}", project.notices);
        assert_eq!(std::fs::read_to_string(&file).unwrap(), numbered(&[2, 10]));
    }

    #[test]
    fn unstaging_a_rename_puts_back_both_sides() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        git(root, &["init", "-q", "-b", "main"]);
        std::fs::write(root.join("d.txt"), "d\n").unwrap();
        git(root, &["add", "d.txt"]);
        git(root, &["commit", "-q", "-m", "first"]);
        git(root, &["mv", "d.txt", "e.txt"]);
        let mut project = open(root);
        wait(&mut project, "the rename never showed", |p| {
            p.worktree()
                .and_then(|wt| wt.status.as_ref())
                .is_some_and(|s| s.staged().any(|e| e.index == graft_core::Change::Renamed))
        });

        project.unstage_files(vec!["e.txt".to_owned()]);
        wait(&mut project, "unstaging never ended", |p| !p.is_operating());
        assert!(project.notices.is_empty(), "{:?}", project.notices);
        assert_eq!(porcelain(root), " D d.txt\n?? e.txt\n");
    }

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
