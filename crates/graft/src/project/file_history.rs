//! File history: the commits that changed one file, each with its diff of
//! that file, or the whole file annotated with who last changed each line.

use graft_core::diff::Source;
use graft_core::file_log::{self, Blame, FileCommit};

use super::{FileKey, Project, WorkspaceView, readable};

/// Commits listed at most.
const LIMIT: usize = 2000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileHistoryMode {
    /// The selected commit's diff of the file.
    Changes,
    /// The file as of the selected commit, line by line with its last change.
    Blame,
}

pub struct FileHistoryView {
    /// The file's path in the worktree.
    pub path: String,
    /// `None` while loading.
    pub commits: Option<Result<Vec<FileCommit>, String>>,
    pub selected: Option<usize>,
    pub mode: FileHistoryMode,
    /// Blame for the selected commit; `None` while loading.
    pub blame: Option<Result<Blame, String>>,
    ticket: u64,
    /// The view to go back to.
    pub back: WorkspaceView,
}

impl Project {
    pub(super) fn open_file_history(&mut self, path: String) {
        let back = match self.nav.view {
            WorkspaceView::FileHistory => self
                .file_history
                .as_ref()
                .map_or(WorkspaceView::History, |v| v.back),
            view => view,
        };
        self.show_view(WorkspaceView::FileHistory);
        self.read_only_diff = None;
        self.file_history = Some(FileHistoryView {
            path: path.clone(),
            commits: None,
            selected: None,
            mode: FileHistoryMode::Changes,
            blame: None,
            ticket: 0,
            back,
        });
        let git = self.wt_git();
        let asked = path.clone();
        self.jobs.spawn(
            move || file_log::log(&git, &asked, LIMIT),
            move |p, result| p.on_file_log(path, readable(result)),
        );
    }

    fn on_file_log(&mut self, path: String, result: Result<Vec<FileCommit>, String>) {
        let Some(view) = &mut self.file_history else {
            return;
        };
        if view.path != path {
            return;
        }
        let first = result.as_ref().is_ok_and(|c| !c.is_empty());
        view.commits = Some(result);
        if first {
            self.select_file_commit(0);
        }
    }

    pub(super) fn select_file_commit(&mut self, index: usize) {
        let Some(view) = &mut self.file_history else {
            return;
        };
        let Some(Ok(commits)) = &view.commits else {
            return;
        };
        let Some(commit) = commits.get(index).cloned() else {
            return;
        };
        view.selected = Some(index);
        view.blame = None;
        let mode = view.mode;
        self.select_commit_file(FileKey {
            source: Source::Commit(commit.id),
            path: commit.path.clone(),
        });
        if mode == FileHistoryMode::Blame {
            self.load_blame();
        }
    }

    fn load_blame(&mut self) {
        let ticket = self.ticket();
        let Some(view) = &mut self.file_history else {
            return;
        };
        let Some(commit) = view
            .selected
            .and_then(|i| view.commits.as_ref()?.as_ref().ok()?.get(i))
            .cloned()
        else {
            return;
        };
        view.blame = None;
        view.ticket = ticket;
        let git = self.git.clone();
        self.jobs.spawn(
            move || file_log::blame(&git, commit.id, &commit.path),
            move |p, result: anyhow::Result<Blame>| {
                if let Some(view) = p.file_history.as_mut().filter(|v| v.ticket == ticket) {
                    view.blame = Some(readable(result));
                }
            },
        );
    }

    pub(super) fn close_file_history(&mut self) {
        if let Some(view) = self.file_history.take() {
            self.nav.view = view.back;
        }
        self.read_only_diff = None;
    }

    pub(super) fn set_file_history_mode(&mut self, mode: FileHistoryMode) {
        let Some(view) = &mut self.file_history else {
            return;
        };
        if view.mode == mode {
            return;
        }
        view.mode = mode;
        if mode == FileHistoryMode::Blame && view.blame.is_none() {
            self.load_blame();
        }
    }
}
