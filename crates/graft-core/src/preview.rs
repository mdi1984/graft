//! Preview in main: show another worktree's exact state (its commit plus its
//! uncommitted changes, untracked files included) in the main worktree, e.g.
//! to run the app from the usual folder. Main must be clean; it is checked
//! out detached at the worktree's commit and the worktree's changes are
//! applied on top. A marker file in main's git dir remembers what to go back
//! to, so "Restore main" works after a restart too.

use std::path::{Path, PathBuf};

use anyhow::{Result, bail};

use crate::cli::{Git, failure_message};
use crate::conflicts;
use crate::model::{Oid, RepoSnapshot, Worktree};
pub use crate::model::{Preview, Restore};
use crate::paths::same_path;
use crate::repo::git_dir;

const MARKER: &str = "graft-preview";

fn marker_path(main: &Path) -> Option<PathBuf> {
    git_dir(main).map(|d| d.join(MARKER))
}

/// What "Preview in main" does for a worktree.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    /// Show it in main.
    Start,
    /// Main already shows it: show its current state again.
    Update,
}

/// Why a worktree cannot be shown in main right now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Blocked {
    /// Main shows another worktree.
    OtherPreview,
    IsMain,
    MainInProgress,
    MainDirty,
}

impl Blocked {
    pub fn message(self) -> &'static str {
        match self {
            Blocked::OtherPreview => "The main folder already shows a preview. Restore it first.",
            Blocked::IsMain => "Select another worktree to show its state in the main folder",
            Blocked::MainInProgress => {
                "The main folder has a merge or rebase in progress. Finish or abort it first."
            }
            Blocked::MainDirty => {
                "The main folder has uncommitted changes. Commit or stash them first."
            }
        }
    }
}

impl std::fmt::Display for Blocked {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.message())
    }
}

impl std::error::Error for Blocked {}

/// The rules for showing a worktree in main, from what is known about both.
fn rules(
    preview: Option<&Path>,
    source: &Path,
    main: &Path,
    main_in_progress: bool,
    main_dirty: bool,
) -> Result<Step, Blocked> {
    match preview {
        Some(shown) if same_path(shown, source) => Ok(Step::Update),
        Some(_) => Err(Blocked::OtherPreview),
        None if same_path(main, source) => Err(Blocked::IsMain),
        None if main_in_progress => Err(Blocked::MainInProgress),
        None if main_dirty => Err(Blocked::MainDirty),
        None => Ok(Step::Start),
    }
}

/// What "Preview in main" would do for `wt`, judged from the snapshot.
pub fn check(snap: &RepoSnapshot, wt: &Worktree) -> Result<Step, Blocked> {
    let main = snap.main_worktree();
    rules(
        snap.preview.as_ref().map(|p| p.source.as_path()),
        &wt.path,
        main.map_or(snap.root.as_path(), |m| m.path.as_path()),
        main.is_some_and(|m| m.in_progress.is_some()),
        main.is_some_and(Worktree::is_dirty),
    )
}

/// The preview shown in the main worktree at `main`, if any.
pub fn active(main: &Path) -> Option<Preview> {
    let text = std::fs::read_to_string(marker_path(main)?).ok()?;
    let mut restore = None;
    let mut source = None;
    let mut head = None;
    let mut added = Vec::new();
    for line in text.lines() {
        let (key, value) = line.split_once(' ')?;
        match key {
            "restore-branch" => restore = Some(Restore::Branch(value.to_owned())),
            "restore-detached" => {
                restore = Some(Restore::Detached(Oid::from_hex(value.as_bytes()).ok()?));
            }
            "source" => source = Some(PathBuf::from(value)),
            "head" => head = Oid::from_hex(value.as_bytes()).ok(),
            // Only paths inside main: this file is in .git, which a
            // repository's tooling can write to.
            "added" if is_inside(value) => added.push(value.to_owned()),
            _ => {}
        }
    }
    Some(Preview {
        restore: restore?,
        source: source?,
        head: head?,
        added,
    })
}

/// A relative path made of plain names: no drive, root, `..` or `.`.
fn is_inside(path: &str) -> bool {
    !path.is_empty()
        && Path::new(path)
            .components()
            .all(|c| matches!(c, std::path::Component::Normal(_)))
}

fn write_marker(main: &Path, preview: &Preview) -> Result<()> {
    let Some(path) = marker_path(main) else {
        bail!("{} is not a git worktree", main.display());
    };
    let mut text = match &preview.restore {
        Restore::Branch(b) => format!("restore-branch {b}\n"),
        Restore::Detached(id) => format!("restore-detached {id}\n"),
    };
    text.push_str(&format!("source {}\n", preview.source.display()));
    text.push_str(&format!("head {}\n", preview.head));
    for a in &preview.added {
        text.push_str(&format!("added {a}\n"));
    }
    std::fs::write(path, text)?;
    Ok(())
}

/// The worktree's uncommitted changes (untracked files included) as a
/// binary patch against its HEAD, and the files that patch creates. Uses a
/// copy of the index, so the worktree's own staging is left alone.
fn uncommitted_patch(source: &Git) -> Result<(Vec<u8>, Vec<String>)> {
    let name = format!(
        "graft-preview-{}-{}.index",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos())
    );
    let git_dir = git_dir(source.cwd());
    // Git inside WSL reads the copy best from its own disk.
    let tmp = match &git_dir {
        Some(dir) if crate::wsl::Wsl::of(dir).is_some() => dir.join(name),
        _ => std::env::temp_dir().join(name),
    };
    let real = git_dir.map(|d| d.join("index"));
    let copied = real.is_some_and(|r| std::fs::copy(r, &tmp).is_ok());
    let git = source.with_index(&tmp);
    let result = (|| {
        if !copied {
            git.run(["read-tree", "HEAD"])?;
        }
        git.run(["add", "-A"])?;
        let patch = git
            .run([
                "diff",
                "--cached",
                "--binary",
                "--no-color",
                "--no-ext-diff",
                "HEAD",
            ])?
            .stdout;
        let added = git
            .read([
                "diff",
                "--cached",
                "--name-only",
                "-z",
                "--no-renames",
                "--diff-filter=A",
                "HEAD",
            ])?
            .split('\0')
            .filter(|p| !p.is_empty())
            .map(str::to_owned)
            .collect();
        Ok((patch, added))
    })();
    let _ = std::fs::remove_file(&tmp);
    result
}

/// Check out `source`'s commit in main and apply its uncommitted changes.
fn show(main: &Git, source: &Git, restore: Restore) -> Result<Preview> {
    let head = source.read(["rev-parse", "--verify", "HEAD"])?;
    let head = Oid::from_hex(head.trim().as_bytes())?;
    let (patch, added) = uncommitted_patch(source)?;
    main.run(["checkout", "-q", "--detach", &head.to_string()])?;
    let preview = Preview {
        restore,
        source: source.cwd().to_path_buf(),
        head,
        added,
    };
    write_marker(main.cwd(), &preview)?;
    if !patch.is_empty() {
        let out = main.run_with_input(["apply", "--whitespace=nowarn", "-"], Some(&patch))?;
        if !out.success() {
            let message = failure_message(&out);
            if let Err(rollback) = restore_main(main) {
                bail!(
                    "The worktree's changes could not be applied in main: {message}\n\
                     Going back failed too, so main still shows the worktree's commit: \
                     {rollback:#}. Use Restore main to try again."
                );
            }
            bail!("The worktree's changes could not be applied in main: {message}");
        }
    }
    Ok(preview)
}

/// Show the worktree at `source` in the main worktree (`main`).
pub fn start(main: &Git, source: &Git) -> Result<Preview> {
    // Any preview blocks a new one here, even of the same worktree: that is
    // `update`.
    if active(main.cwd()).is_some() {
        bail!(Blocked::OtherPreview);
    }
    let dirty = !main.read(["status", "--porcelain", "-z"])?.is_empty();
    rules(
        None,
        source.cwd(),
        main.cwd(),
        conflicts::in_progress(main.cwd()).is_some(),
        dirty,
    )?;
    let out = main.run_expecting(["symbolic-ref", "-q", "--short", "HEAD"], None, |o| {
        o.status <= 1
    })?;
    let branch = out.stdout_str().trim().to_owned();
    let restore = if branch.is_empty() {
        let id = main.read(["rev-parse", "--verify", "HEAD"])?;
        Restore::Detached(Oid::from_hex(id.trim().as_bytes())?)
    } else {
        Restore::Branch(branch)
    };
    show(main, source, restore)
}

/// Throw away what the preview put into main (and anything changed there
/// since), without going back yet.
fn clear(main: &Git, preview: &Preview) -> Result<()> {
    main.run(["reset", "--hard", "-q"])?;
    for path in &preview.added {
        let file = main.cwd().join(path);
        if file.is_file() {
            let _ = std::fs::remove_file(&file);
        }
        // Folders the preview created and left empty.
        let mut dir = file.parent();
        while let Some(d) = dir {
            if same_path(d, main.cwd()) || std::fs::remove_dir(d).is_err() {
                break;
            }
            dir = d.parent();
        }
    }
    Ok(())
}

/// Show the worktree's current state again (after it changed).
pub fn update(main: &Git) -> Result<Preview> {
    let Some(preview) = active(main.cwd()) else {
        bail!("Main shows no preview.");
    };
    clear(main, &preview)?;
    let source = main.at(&preview.source);
    show(main, &source, preview.restore)
}

/// Put main back on what it had before the preview.
pub fn restore_main(main: &Git) -> Result<()> {
    let Some(preview) = active(main.cwd()) else {
        bail!("Main shows no preview.");
    };
    clear(main, &preview)?;
    match &preview.restore {
        // `switch`: git before 2.44 (e.g. Ubuntu 24.04's, in WSL) reads
        // `checkout --end-of-options` as a file name.
        Restore::Branch(b) => main.run(["switch", "-q", "--end-of-options", b])?,
        Restore::Detached(id) => main.run(["checkout", "-q", "--detach", &id.to_string()])?,
    };
    if let Some(path) = marker_path(main.cwd()) {
        std::fs::remove_file(path)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::is_inside;

    #[test]
    fn marker_paths_stay_inside_main() {
        assert!(is_inside("src/new.rs"));
        assert!(is_inside("a"));
        for bad in [
            "",
            "../x",
            "src/../../x",
            r"C:\Windows\x",
            r"\\server\share\x",
            "/etc/x",
            "./x",
        ] {
            assert!(!is_inside(bad), "{bad}");
        }
    }
}
