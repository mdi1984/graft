//! File history: the commits that changed one file, each with its diff of
//! that file, or the whole file annotated with who last changed each line.

use graft_core::diff::Source;
use graft_core::file_log::{self, Blame, FileCommit};

use super::{Action, Event, FileKey, Project, WorkspaceView};

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
    generation: u64,
    /// The view to go back to.
    pub back: WorkspaceView,
}

impl Project {
    fn open_file_history(&mut self, path: String) {
        let back = match self.nav.view {
            WorkspaceView::FileHistory => self
                .file_history
                .as_ref()
                .map_or(WorkspaceView::History, |v| v.back),
            view => view,
        };
        self.nav.tab = super::Tab::Workspace;
        self.nav.view = WorkspaceView::FileHistory;
        self.commit_diff = None;
        self.file_history = Some(FileHistoryView {
            path: path.clone(),
            commits: None,
            selected: None,
            mode: FileHistoryMode::Changes,
            blame: None,
            generation: 0,
            back,
        });
        let git = self.wt_git();
        self.pool.spawn(&self.tx, move || {
            let result = file_log::log(&git, &path, LIMIT).map_err(|e| format!("{e:#}"));
            Event::FileLog(path, result)
        });
    }

    pub(super) fn on_file_log(&mut self, path: String, result: Result<Vec<FileCommit>, String>) {
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

    fn select_file_commit(&mut self, index: usize) {
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
        view.generation += 1;
        let generation = view.generation;
        let git = self.git.clone();
        self.pool.spawn(&self.tx, move || {
            let result =
                file_log::blame(&git, commit.id, &commit.path).map_err(|e| format!("{e:#}"));
            Event::Blame { generation, result }
        });
    }

    pub(super) fn on_blame(&mut self, generation: u64, result: Result<Blame, String>) {
        if let Some(view) = &mut self.file_history
            && view.generation == generation
        {
            view.blame = Some(result);
        }
    }

    pub(super) fn apply_file_history_action(&mut self, action: Action) {
        match action {
            Action::OpenFileHistory(path) => self.open_file_history(path),
            Action::CloseFileHistory => {
                if let Some(view) = self.file_history.take() {
                    self.nav.view = view.back;
                }
                self.commit_diff = None;
            }
            Action::SelectFileCommit(i) => self.select_file_commit(i),
            Action::SetFileHistoryMode(mode) => {
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
            _ => {}
        }
    }
}
