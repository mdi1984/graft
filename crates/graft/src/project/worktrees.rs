//! Worktrees: the New worktree form, creating, removing, locking and pruning
//! worktrees, and the file watcher that keeps every worktree's status current.

use std::path::{Path, PathBuf};

use graft_core::watch::Watcher;
use graft_core::worktree::{self, Start};
use graft_core::{RepoSnapshot, same_path};

use super::changes::Then;
use super::{Event, Project};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BranchMode {
    New,
    Existing,
}

/// The New worktree dialog's fields.
#[derive(Clone, Debug)]
pub struct NewWorktreeForm {
    pub mode: BranchMode,
    /// Name of the new branch.
    pub name: String,
    /// What the new branch starts from.
    pub base: String,
    /// An existing local branch, or a remote branch like `origin/feature`.
    pub existing: String,
    pub path: String,
    /// The user typed or picked a folder; stop deriving it from the branch.
    pub path_edited: bool,
    pub copy_ignored: bool,
    pub patterns: String,
    pub error: Option<String>,
    pub creating: bool,
    /// Focus the first field once, when the dialog opens.
    pub focus: bool,
}

impl NewWorktreeForm {
    /// The local branch the new worktree will have checked out.
    pub fn branch(&self, snap: &RepoSnapshot) -> String {
        match self.mode {
            BranchMode::New => self.name.trim().to_owned(),
            BranchMode::Existing => match remote_branch(snap, &self.existing) {
                Some(local) => local.to_owned(),
                None => self.existing.clone(),
            },
        }
    }

    /// Keep the folder following the branch name until the user edits it.
    pub fn sync_path(&mut self, snap: &RepoSnapshot) {
        if self.path_edited {
            return;
        }
        let branch = self.branch(snap);
        self.path = if branch.is_empty() {
            String::new()
        } else {
            worktree::default_path(&snap.root, &branch)
                .display()
                .to_string()
        };
    }

    fn start(&self, snap: &RepoSnapshot) -> Result<Start, String> {
        match self.mode {
            BranchMode::New => {
                let name = self.name.trim();
                if name.is_empty() {
                    return Err("Give the new branch a name.".to_owned());
                }
                if name.contains(char::is_whitespace) {
                    return Err("Branch names cannot contain spaces.".to_owned());
                }
                if snap.branch(name).is_some() {
                    return Err(format!(
                        "A branch named {name} already exists. Choose \"Existing branch\" to check it out."
                    ));
                }
                if self.base.trim().is_empty() {
                    return Err("Pick what the new branch starts from.".to_owned());
                }
                Ok(Start::NewBranch {
                    name: name.to_owned(),
                    base: self.base.trim().to_owned(),
                })
            }
            BranchMode::Existing => {
                if self.existing.is_empty() {
                    return Err("Pick a branch to check out.".to_owned());
                }
                if let Some(local) = remote_branch(snap, &self.existing) {
                    if snap.branch(local).is_some() {
                        return Ok(Start::Existing(local.to_owned()));
                    }
                    return Ok(Start::Tracking {
                        name: local.to_owned(),
                        remote: self.existing.clone(),
                    });
                }
                if let Some(wt) = snap.worktree_for_branch(&self.existing) {
                    return Err(format!(
                        "{} is already checked out in {}.",
                        self.existing,
                        wt.path.display()
                    ));
                }
                Ok(Start::Existing(self.existing.clone()))
            }
        }
    }
}

/// If `name` is a remote branch like `origin/feature`, its local name.
fn remote_branch<'a>(snap: &'a RepoSnapshot, name: &str) -> Option<&'a str> {
    snap.remote_branches
        .iter()
        .find(|b| b.name == name)
        .map(|b| b.branch_name())
}

/// Branches that can get a worktree of their own: local branches not checked
/// out anywhere, then remote branches without a local branch of that name.
pub fn checkout_choices(snap: &RepoSnapshot) -> Vec<String> {
    let local = snap
        .branches
        .iter()
        .filter(|b| snap.worktree_for_branch(&b.name).is_none())
        .map(|b| b.name.clone());
    let remote = snap
        .remote_branches
        .iter()
        .filter(|b| b.branch_name() != "HEAD" && snap.branch(b.branch_name()).is_none())
        .map(|b| b.name.clone());
    local.chain(remote).collect()
}

impl Project {
    /// Open the New worktree dialog, optionally for an existing branch.
    pub(super) fn open_worktree_form(&mut self, branch: Option<String>) {
        let Some(snap) = &self.snapshot else { return };
        let base = self
            .worktree()
            .and_then(|wt| wt.branch.clone())
            .or_else(|| snap.default_branch().map(str::to_owned))
            .unwrap_or_else(|| "HEAD".to_owned());
        let mut form = NewWorktreeForm {
            mode: if branch.is_some() {
                BranchMode::Existing
            } else {
                BranchMode::New
            },
            name: String::new(),
            base,
            existing: branch.unwrap_or_default(),
            path: String::new(),
            path_edited: false,
            copy_ignored: true,
            patterns: self.settings.copy_patterns.clone(),
            error: None,
            creating: false,
            focus: true,
        };
        form.sync_path(snap);
        self.new_worktree = Some(form);
    }

    pub(super) fn create_worktree(&mut self) {
        let (Some(form), Some(snap)) = (&mut self.new_worktree, &self.snapshot) else {
            return;
        };
        if form.creating {
            return;
        }
        let start = match form.start(snap) {
            Ok(start) => start,
            Err(e) => {
                form.error = Some(e);
                return;
            }
        };
        let path = PathBuf::from(form.path.trim());
        if form.path.trim().is_empty() || !path.is_absolute() {
            form.error = Some("Pick a full folder path for the worktree.".to_owned());
            return;
        }
        form.error = None;
        form.creating = true;
        let patterns = form.copy_ignored.then(|| form.patterns.clone());
        let main = self.git.clone();
        let target = path.clone();
        self.run_op_at(
            self.git.clone(),
            "Creating worktree",
            Then::OpenWorktree(path),
            move |git| {
                worktree::add(&git, &target, &start)?;
                let Some(patterns) = patterns else {
                    return Ok(None);
                };
                let copied = worktree::copy_ignored(&main, &target, &patterns)?;
                Ok(Some(match copied.len() {
                    0 => "Created the worktree. No ignored files matched to copy.".to_owned(),
                    n => format!(
                        "Created the worktree and copied {n} ignored file{}: {}",
                        if n == 1 { "" } else { "s" },
                        copied.join(", ")
                    ),
                }))
            },
        );
    }

    pub(super) fn browse_worktree_folder(&mut self) {
        let Some(form) = &self.new_worktree else {
            return;
        };
        let current = PathBuf::from(form.path.trim());
        let start = current
            .parent()
            .filter(|p| p.is_dir())
            .map(Path::to_path_buf)
            .or_else(|| self.root.parent().map(Path::to_path_buf));
        self.pool.spawn_long(&self.tx, move |_| {
            let mut dialog = rfd::FileDialog::new().set_title("Where should the worktree go?");
            if let Some(dir) = start {
                dialog = dialog.set_directory(dir);
            }
            Event::PickedFolder(dialog.pick_folder())
        });
    }

    /// A folder came back from the picker: the worktree goes in a new folder
    /// named after the branch inside it, unless it is empty.
    pub(super) fn on_picked_folder(&mut self, folder: Option<PathBuf>) {
        let (Some(form), Some(snap), Some(folder)) =
            (&mut self.new_worktree, &self.snapshot, folder)
        else {
            return;
        };
        let empty = folder
            .read_dir()
            .map(|mut d| d.next().is_none())
            .unwrap_or(false);
        let branch = form.branch(snap);
        let path = if empty || branch.is_empty() {
            folder
        } else {
            let name = worktree::default_path(Path::new("x"), &branch);
            folder.join(name.file_name().unwrap_or_default())
        };
        form.path = path.display().to_string();
        form.path_edited = true;
    }

    pub(super) fn remove_worktree(&mut self, path: PathBuf, force: bool) {
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        self.run_op_at(
            self.git.clone(),
            "Removing worktree",
            Then::Info(format!("Removed worktree {name}")),
            move |git| worktree::remove(&git, &path, force).map(|()| None),
        );
    }

    pub(super) fn lock_worktree(&mut self, path: PathBuf, locked: bool) {
        let label = if locked { "Locking" } else { "Unlocking" };
        self.run_op_at(self.git.clone(), label, Then::Nothing, move |git| {
            worktree::set_locked(&git, &path, locked).map(|()| None)
        });
    }

    pub(super) fn prune_worktrees(&mut self) {
        self.run_op_at(
            self.git.clone(),
            "Pruning worktrees",
            Then::Info("Removed missing worktrees".to_owned()),
            move |git| worktree::prune(&git).map(|()| None),
        );
    }

    /// Watch every worktree that exists on disk; rebuild when the set changes.
    pub(super) fn sync_watcher(&mut self) {
        let Some(snap) = &self.snapshot else { return };
        let roots: Vec<PathBuf> = snap
            .worktrees
            .iter()
            .filter(|wt| !wt.prunable)
            .map(|wt| wt.path.clone())
            .collect();
        let same = roots.len() == self.watched.len()
            && roots
                .iter()
                .zip(&self.watched)
                .all(|(a, b)| same_path(a, b));
        if same {
            return;
        }
        self.watched = roots.clone();
        self.watcher = None;
        let tx = self.tx.clone();
        let waker = self.pool.waker();
        match Watcher::new(roots, move || {
            let _ = tx.send(Event::Changed);
            waker();
        }) {
            Ok(watcher) => self.watcher = Some(watcher),
            Err(e) => tracing::warn!("file watcher unavailable: {e:#}"),
        }
    }
}

/// Open a terminal in `dir`: Windows Terminal if installed, else cmd.
pub fn open_terminal(dir: &Path) {
    let wt = std::process::Command::new("wt.exe")
        .arg("-d")
        .arg(dir)
        .spawn();
    if wt.is_err() {
        let _ = std::process::Command::new("cmd.exe")
            .args(["/c", "start", "", "/D"])
            .arg(dir)
            .arg("cmd.exe")
            .spawn();
    }
}

/// Open a folder or file in VS Code (`code` on the PATH); without it, open the
/// path with its default program.
pub fn open_in_ide(path: &Path, command: &str) {
    let path = path.to_path_buf();
    let command = command.trim().to_owned();
    std::thread::spawn(move || {
        let mut cmd = std::process::Command::new("cmd.exe");
        cmd.arg("/c").args(command.split_whitespace()).arg(&path);
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            cmd.creation_flags(CREATE_NO_WINDOW);
        }
        let ok = !command.is_empty() && cmd.status().is_ok_and(|s| s.success());
        if !ok {
            let _ = open::that_detached(&path);
        }
    });
}
