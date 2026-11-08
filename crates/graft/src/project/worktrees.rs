//! Worktrees: the New worktree form, creating, removing, locking and pruning
//! worktrees, and the file watcher that keeps every worktree's status current.

use std::path::{Path, PathBuf};

use graft_core::RepoSnapshot;
use graft_core::paths::folder_name;
use graft_core::paths::same_path;
use graft_core::refs::{RefKind, check_ref_name};
use graft_core::watch::Watcher;
use graft_core::worktree::{self, Start};

use super::Project;
use super::ops::{At, quietly, say};
use crate::launch;
use crate::text::plural;

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
                check_ref_name(RefKind::Branch, name)?;
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

    pub(super) fn close_worktree_form(&mut self) {
        if !self.new_worktree.as_ref().is_some_and(|f| f.creating) {
            self.new_worktree = None;
        }
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
        let patterns = form.copy_ignored.then(|| form.patterns.clone());
        let main = self.git.clone();
        let target = path.clone();
        let job = move |git: graft_core::Git| -> anyhow::Result<Option<String>> {
            worktree::add(&git, &target, &start)?;
            let Some(patterns) = patterns else {
                return Ok(None);
            };
            let copied = worktree::copy_ignored(&main, &target, &patterns)?;
            Ok(Some(match copied.len() {
                0 => "Created the worktree. No ignored files matched to copy.".to_owned(),
                n => format!(
                    "Created the worktree and copied {}: {}",
                    plural(n, "ignored file", "ignored files"),
                    copied.join(", ")
                ),
            }))
        };
        let done = move |p: &mut Project, result: anyhow::Result<Option<String>>| match result {
            Ok(extra) => {
                p.new_worktree = None;
                p.infos.extend(extra);
                p.pending_open = Some(path);
            }
            Err(e) => match &mut p.new_worktree {
                Some(form) => {
                    form.creating = false;
                    form.error = Some(format!("{e:#}"));
                }
                None => p.report(e),
            },
        };
        if self.run_op(At::Main, "Creating worktree", job, done)
            && let Some(form) = &mut self.new_worktree
        {
            form.error = None;
            form.creating = true;
        }
    }

    pub(super) fn browse_worktree_folder(&mut self) {
        let Some(form) = &self.new_worktree else {
            return;
        };
        let typed = PathBuf::from(form.path.trim())
            .parent()
            .map(Path::to_path_buf);
        let fallback = self.root.parent().map(Path::to_path_buf);
        self.jobs.spawn_long(
            move |_| {
                let start = typed.filter(|p| p.is_dir()).or(fallback);
                let folder = launch::pick_folder("Where should the worktree go?", start)?;
                let empty = folder
                    .read_dir()
                    .map(|mut d| d.next().is_none())
                    .unwrap_or(false);
                Some((folder, empty))
            },
            |p, picked| {
                if let Some((folder, empty)) = picked {
                    p.on_picked_folder(folder, empty);
                }
            },
        );
    }

    /// A folder came back from the picker: the worktree goes in a new folder
    /// named after the branch inside it, unless it is `empty`.
    fn on_picked_folder(&mut self, folder: PathBuf, empty: bool) {
        let (Some(form), Some(snap)) = (&mut self.new_worktree, &self.snapshot) else {
            return;
        };
        let branch = form.branch(snap);
        let path = if empty || branch.is_empty() {
            folder
        } else {
            folder.join(worktree::folder_for(&branch))
        };
        form.path = path.display().to_string();
        form.path_edited = true;
    }

    pub(super) fn remove_worktree(&mut self, path: PathBuf, force: bool) {
        let done = format!("Removed worktree {}", folder_name(&path));
        self.run_op(
            At::Main,
            "Removing worktree",
            move |git| worktree::remove(&git, &path, force),
            say(done),
        );
    }

    pub(super) fn lock_worktree(&mut self, path: PathBuf, locked: bool) {
        let label = if locked { "Locking" } else { "Unlocking" };
        self.run_op(
            At::Main,
            label,
            move |git| worktree::set_locked(&git, &path, locked),
            quietly,
        );
    }

    pub(super) fn prune_worktrees(&mut self) {
        self.run_op(
            At::Main,
            "Pruning worktrees",
            move |git| worktree::prune(&git),
            say("Removed missing worktrees".to_owned()),
        );
    }

    /// Watch every worktree that exists on disk; rebuild when the set
    /// changes. Setting up the watches reads the disk, so it runs as a job.
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
        // The old watcher stops on the job's thread too.
        let old = self.watcher.take();
        let post = self.jobs.poster();
        let changed = move || {
            post(Box::new(|p: &mut Project| {
                if !p.is_operating() {
                    p.refresh();
                }
            }))
        };
        let asked = roots.clone();
        self.jobs.spawn(
            move || {
                drop(old);
                Watcher::new(roots, changed)
            },
            move |p, result| {
                // A newer set of worktrees is being watched by now.
                if p.watched != asked {
                    return;
                }
                match result {
                    Ok(watcher) => p.watcher = Some(watcher),
                    Err(e) => p.report(e.context(
                        "Graft cannot watch this repository's files, so changes made outside \
                         Graft show when you come back to its window",
                    )),
                }
            },
        );
    }
}
