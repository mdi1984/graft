//! File status: the selected file's diff, line selection, staging and
//! discarding, the commit form, and fetch/pull/push.

use std::collections::BTreeSet;
use std::ops::Range;
use std::sync::{Arc, Mutex};

use graft_core::diff::{self, FileDiff, Source};
use graft_core::ops::{self, LineOp};
use graft_core::repo::Progress;
use graft_core::status::{EntryKind, StatusEntry};
use graft_core::{CancelToken, Git};
use graft_core::{refs, remote};

use super::{Action, Event, Project};

/// One file in one of the File status lists.
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

pub type Highlights = Vec<Vec<Option<Range<usize>>>>;

/// The diff shown next to the file lists.
pub struct DiffView {
    pub key: FileKey,
    pub diff: Option<FileDiff>,
    pub words: Highlights,
    pub error: Option<String>,
    pub loading: bool,
    /// Selected changed lines as (hunk, line).
    pub selection: BTreeSet<(usize, usize)>,
    pub anchor: Option<(usize, usize)>,
    generation: u64,
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
    pub fn message(&self) -> String {
        let summary = self.summary.trim();
        let body = self.body.trim();
        if body.is_empty() {
            summary.to_owned()
        } else {
            format!("{summary}\n\n{body}")
        }
    }
}

/// A git command that changes things, running in the background.
pub struct Operation {
    pub label: String,
    pub progress: Arc<Mutex<Progress>>,
    pub cancel: Option<CancelToken>,
}

/// A question to confirm before doing something destructive.
#[derive(Clone, Debug)]
pub struct Confirm {
    pub title: String,
    pub body: String,
    pub confirm_label: String,
    pub action: Action,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RemoteOp {
    Fetch,
    Pull,
    Push,
    /// Push over a diverged upstream, with lease.
    ForcePush,
    PushTag {
        remote: String,
        tag: String,
    },
    DeleteRemoteBranch {
        remote: String,
        branch: String,
    },
}

impl RemoteOp {
    fn label(&self) -> String {
        match self {
            Self::Fetch => "Fetching".to_owned(),
            Self::Pull => "Pulling".to_owned(),
            Self::Push => "Pushing".to_owned(),
            Self::ForcePush => "Force pushing".to_owned(),
            Self::PushTag { tag, .. } => format!("Pushing tag {tag}"),
            Self::DeleteRemoteBranch { remote, branch } => {
                format!("Deleting {remote}/{branch}")
            }
        }
    }
}

/// What happens when an operation finishes successfully.
pub(super) enum Then {
    Nothing,
    /// A plain push: if the remote rejects it, offer to force push.
    Pushed,
    /// Deleting this branch: if it is not merged, offer to force it.
    DeletedBranch(String),
    /// Submitted from the open dialog: close it on success, show errors in it.
    Dialog(String),
    /// Select this worktree as soon as a snapshot contains it.
    OpenWorktree(std::path::PathBuf),
    Committed {
        summary: String,
        push: bool,
    },
    Info(String),
    /// A merge, rebase, cherry-pick, revert or continue: the job's extra
    /// message means it stopped with conflicts.
    MayConflict(String),
}

impl Project {
    /// Git pointed at the selected worktree.
    pub fn wt_git(&self) -> Git {
        self.git.at(&self.nav.worktree)
    }

    pub fn is_operating(&self) -> bool {
        self.op.is_some()
    }

    pub fn select_file(&mut self, key: FileKey) {
        if self.diff_view.as_ref().is_some_and(|v| v.key == key) {
            return;
        }
        self.diff_view = Some(DiffView {
            key,
            diff: None,
            words: Vec::new(),
            error: None,
            loading: true,
            selection: BTreeSet::new(),
            anchor: None,
            generation: 0,
        });
        self.load_diff();
        self.sync_conflict();
    }

    /// Show one file of a commit or stash (read-only).
    pub fn select_commit_file(&mut self, key: FileKey) {
        if self.commit_diff.as_ref().is_some_and(|v| v.key == key) {
            return;
        }
        self.diff_generation += 1;
        let generation = self.diff_generation;
        self.commit_diff = Some(DiffView {
            key: key.clone(),
            diff: None,
            words: Vec::new(),
            error: None,
            loading: true,
            selection: BTreeSet::new(),
            anchor: None,
            generation,
        });
        let git = self.git.clone();
        self.pool.spawn(&self.tx, move || {
            let result = diff::load(&git, &key.source, &key.path)
                .map(|d| {
                    let words = diff::word_highlights(&d);
                    (d, words)
                })
                .map_err(|e| format!("{e:#}"));
            Event::CommitDiff { generation, result }
        });
    }

    pub(super) fn on_commit_diff(
        &mut self,
        generation: u64,
        result: Result<(FileDiff, Highlights), String>,
    ) {
        let Some(view) = &mut self.commit_diff else {
            return;
        };
        if view.generation != generation {
            return;
        }
        view.loading = false;
        match result {
            Ok((diff, words)) => {
                view.diff = Some(diff);
                view.words = words;
            }
            Err(e) => view.error = Some(e),
        }
    }

    /// (Re)load the selected file's diff; the selection survives if the diff
    /// did not change.
    pub(super) fn load_diff(&mut self) {
        self.diff_generation += 1;
        let generation = self.diff_generation;
        let git = self.wt_git();
        let Some(view) = &mut self.diff_view else {
            return;
        };
        view.generation = generation;
        view.loading = true;
        let key = view.key.clone();
        self.pool.spawn(&self.tx, move || {
            let result = diff::load(&git, &key.source, &key.path)
                .map(|d| {
                    let words = diff::word_highlights(&d);
                    (d, words)
                })
                .map_err(|e| format!("{e:#}"));
            Event::Diff { generation, result }
        });
    }

    pub(super) fn on_diff(
        &mut self,
        generation: u64,
        result: Result<(FileDiff, Highlights), String>,
    ) {
        let Some(view) = &mut self.diff_view else {
            return;
        };
        if view.generation != generation {
            return;
        }
        view.loading = false;
        match result {
            Ok((diff, words)) => {
                if view.diff.as_ref() != Some(&diff) {
                    view.selection.clear();
                    view.anchor = None;
                }
                view.diff = Some(diff);
                view.words = words;
                view.error = None;
            }
            Err(e) => {
                view.diff = None;
                view.error = Some(e);
            }
        }
    }

    /// After a status refresh: keep the selected file if it is still in its
    /// list, follow it to the other list if it moved, or pick the first file.
    pub(super) fn sync_file_selection(&mut self) {
        let Some(status) = self.worktree().and_then(|wt| wt.status.clone()) else {
            self.diff_view = None;
            return;
        };
        let keys: Vec<FileKey> = status
            .staged()
            .map(|e| FileKey::of(e, true))
            .chain(status.unstaged().map(|e| FileKey::of(e, false)))
            .chain(status.conflicted().map(|e| FileKey::of(e, false)))
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
            Some(key) if Some(&key) == current.as_ref() => self.load_diff(),
            Some(key) => {
                self.diff_view = None;
                self.select_file(key);
            }
            None => self.diff_view = None,
        }
        self.sync_conflict();
    }

    pub(super) fn apply_change(&mut self, action: Action) {
        match action {
            Action::SelectFile(key) => self.select_file(key),
            Action::ClickLine { hunk, line, shift } => self.click_line(hunk, line, shift),
            Action::DragLine { hunk, line } => self.drag_line(hunk, line),
            Action::ClearLines => {
                if let Some(v) = &mut self.diff_view {
                    v.selection.clear();
                    v.anchor = None;
                }
            }
            Action::HunkOp(hunk, op) => self.lines_op(op, Some(hunk)),
            Action::LinesOp(op) => self.lines_op(op, None),
            Action::StageFiles(paths) => self.run_op("Staging", Then::Nothing, move |git| {
                ops::stage_paths(&git, &paths)
            }),
            Action::UnstageFiles(paths) => self.run_op("Unstaging", Then::Nothing, move |git| {
                ops::unstage_paths(&git, &paths)
            }),
            Action::Discard { tracked, untracked } => {
                self.run_op("Discarding", Then::Nothing, move |git| {
                    ops::discard_paths(&git, &tracked, &untracked)
                })
            }
            Action::Commit => self.commit(),
            Action::ToggleAmend => self.toggle_amend(),
            Action::Remote(op) => self.remote(op),
            Action::CancelOperation => {
                if let Some(cancel) = self.op.as_ref().and_then(|o| o.cancel.as_ref()) {
                    cancel.cancel();
                }
            }
            Action::Ask(confirm) => self.confirm = Some(*confirm),
            Action::ConfirmYes => {
                if let Some(confirm) = self.confirm.take() {
                    self.apply(confirm.action);
                }
            }
            Action::ConfirmNo => self.confirm = None,
            Action::OpenPath(path) => {
                let full = self.nav.worktree.join(&path);
                if let Err(e) = open::that_detached(&full) {
                    self.notices
                        .push(format!("Could not open {}: {e}", full.display()));
                }
            }
            Action::RevealPath(path) => {
                let full =
                    graft_core::repo::native(&self.nav.worktree.join(&path).to_string_lossy());
                let mut arg = std::ffi::OsString::from("/select,");
                arg.push(&full);
                if let Err(e) = std::process::Command::new("explorer").arg(arg).spawn() {
                    self.notices.push(format!("Could not open Explorer: {e}"));
                }
            }
            _ => {}
        }
    }

    fn click_line(&mut self, hunk: usize, line: usize, shift: bool) {
        let Some(view) = &mut self.diff_view else {
            return;
        };
        match (shift, view.anchor) {
            (true, Some(anchor)) => {
                let changed = changed_lines(view);
                let (a, b) = (anchor.min((hunk, line)), anchor.max((hunk, line)));
                for key in changed.into_iter().filter(|k| *k >= a && *k <= b) {
                    view.selection.insert(key);
                }
            }
            _ => {
                if !view.selection.remove(&(hunk, line)) {
                    view.selection.insert((hunk, line));
                }
                view.anchor = Some((hunk, line));
            }
        }
    }

    /// Dragging over lines selects everything from where the drag started.
    fn drag_line(&mut self, hunk: usize, line: usize) {
        let Some(view) = &mut self.diff_view else {
            return;
        };
        let Some(anchor) = view.anchor else {
            return;
        };
        let (a, b) = (anchor.min((hunk, line)), anchor.max((hunk, line)));
        for key in changed_lines(view)
            .into_iter()
            .filter(|k| *k >= a && *k <= b)
        {
            view.selection.insert(key);
        }
    }

    /// Stage, unstage or discard a hunk (`Some(index)`) or the selected lines.
    fn lines_op(&mut self, op: LineOp, hunk: Option<usize>) {
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
        if let Some(v) = &mut self.diff_view {
            v.selection.clear();
            v.anchor = None;
        }
        self.run_op(label, Then::Nothing, move |git| {
            ops::apply_lines(&git, &diff, &source, op, |h, l| match hunk {
                Some(only) => h == only,
                None => selection.contains(&(h, l)),
            })
        });
    }

    fn commit(&mut self) {
        let message = self.commit_form.message();
        if message.is_empty() || self.is_operating() {
            return;
        }
        let amend = self.commit_form.amend;
        let push = self.commit_form.push_after;
        let summary = self.commit_form.summary.trim().to_owned();
        self.run_op(
            if amend { "Amending" } else { "Committing" },
            Then::Committed { summary, push },
            move |git| ops::commit(&git, &message, amend).map(|_| ()),
        );
    }

    fn toggle_amend(&mut self) {
        let form = &mut self.commit_form;
        form.amend = !form.amend;
        if form.amend {
            form.before_amend = Some((form.summary.clone(), form.body.clone()));
            if form.summary.trim().is_empty() {
                let git = self.wt_git();
                self.pool.spawn(&self.tx, move || {
                    Event::LastMessage(ops::last_commit_message(&git).ok().flatten())
                });
            }
        } else if let Some((summary, body)) = form.before_amend.take() {
            form.summary = summary;
            form.body = body;
        }
    }

    pub(super) fn on_last_message(&mut self, message: Option<String>) {
        let form = &mut self.commit_form;
        let Some(message) = message else {
            form.amend = false;
            self.notices
                .push("There is no commit to amend yet.".to_owned());
            return;
        };
        if !form.amend || !form.summary.trim().is_empty() {
            return;
        }
        let (summary, body) = message.split_once('\n').unwrap_or((&message, ""));
        form.summary = summary.trim().to_owned();
        form.body = body.trim().to_owned();
    }

    fn remote(&mut self, op: RemoteOp) {
        if self.is_operating() {
            return;
        }
        let progress = Arc::new(Mutex::new(Progress::default()));
        let cancel = CancelToken::default();
        self.op = Some(Operation {
            label: format!("{}…", op.label()),
            progress: progress.clone(),
            cancel: Some(cancel.clone()),
        });
        let git = self.wt_git();
        let then = match &op {
            RemoteOp::Fetch => Then::Nothing,
            RemoteOp::Pull => Then::Info("Pulled".to_owned()),
            RemoteOp::Push => Then::Pushed,
            RemoteOp::ForcePush => Then::Info("Force pushed".to_owned()),
            RemoteOp::PushTag { remote, tag } => {
                Then::Info(format!("Pushed tag {tag} to {remote}"))
            }
            RemoteOp::DeleteRemoteBranch { remote, branch } => {
                Then::Info(format!("Deleted {remote}/{branch}"))
            }
        };
        self.pool.spawn_long(&self.tx, move |waker| {
            let report = |p: Progress| {
                if let Ok(mut slot) = progress.lock() {
                    *slot = p;
                }
                waker();
            };
            let result = match op {
                RemoteOp::Fetch => remote::fetch(&git, &cancel, report),
                RemoteOp::Pull => remote::pull(&git, &cancel, report),
                RemoteOp::Push => remote::push(&git, &cancel, report),
                RemoteOp::ForcePush => remote::force_push(&git, &cancel, report),
                RemoteOp::PushTag { remote, tag } => {
                    refs::push_tag(&git, &remote, &tag, &cancel, report)
                }
                RemoteOp::DeleteRemoteBranch { remote, branch } => {
                    refs::delete_remote_branch(&git, &remote, &branch, &cancel, report)
                }
            };
            Event::OpDone {
                then,
                result: result.map(|()| None).map_err(|e| format!("{e:#}")),
            }
        });
    }

    /// Run a quick git operation on the selected worktree, then refresh.
    pub(super) fn run_op(
        &mut self,
        label: &str,
        then: Then,
        job: impl FnOnce(Git) -> anyhow::Result<()> + Send + 'static,
    ) {
        let git = self.wt_git();
        self.run_op_at(git, label, then, move |git| job(git).map(|()| None));
    }

    /// Run a quick git operation with `git`, then refresh. The job may return
    /// an extra message for the user.
    pub(super) fn run_op_at(
        &mut self,
        git: Git,
        label: &str,
        then: Then,
        job: impl FnOnce(Git) -> anyhow::Result<Option<String>> + Send + 'static,
    ) {
        if self.is_operating() {
            return;
        }
        self.op = Some(Operation {
            label: format!("{label}…"),
            progress: Arc::default(),
            cancel: None,
        });
        self.pool.spawn(&self.tx, move || Event::OpDone {
            then,
            result: job(git).map_err(|e| format!("{e:#}")),
        });
    }

    pub(super) fn on_op_done(&mut self, then: Then, result: Result<Option<String>, String>) {
        self.op = None;
        self.refresh();
        match result {
            Err(e) if e == "Cancelled" => {}
            Err(e) => self.on_op_error(then, e),
            Ok(extra) => match then {
                Then::Nothing | Then::DeletedBranch(_) => self.infos.extend(extra),
                Then::Pushed => self.infos.push("Pushed".to_owned()),
                Then::Dialog(text) => {
                    self.dialog = None;
                    self.infos.push(text);
                }
                Then::MayConflict(done) => self.on_may_conflict(done, extra),
                Then::Info(text) => self.infos.push(match extra {
                    Some(extra) => format!("{text}. {extra}"),
                    None => text,
                }),
                Then::OpenWorktree(path) => {
                    self.new_worktree = None;
                    self.infos.extend(extra);
                    self.pending_open = Some(path);
                }
                Then::Committed { summary, push } => {
                    self.commit_form = CommitForm {
                        push_after: self.commit_form.push_after,
                        ..CommitForm::default()
                    };
                    self.infos.push(format!("Committed: {summary}"));
                    if push {
                        self.remote(RemoteOp::Push);
                    }
                }
            },
        }
    }
}

fn changed_lines(view: &DiffView) -> Vec<(usize, usize)> {
    let Some(diff) = &view.diff else {
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
