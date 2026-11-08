//! Preview in main: show another worktree's exact state (its commit plus its
//! uncommitted changes, untracked files included) in the main worktree, e.g.
//! to run the app from the usual folder. Main must be clean; it is checked
//! out detached at the worktree's commit and the worktree's changes are
//! applied on top. A marker file in main's git dir remembers what to go back
//! to, so "Restore main" works after a restart too.

use std::path::{Path, PathBuf};

use anyhow::{Result, bail};

use crate::cli::Git;
use crate::model::{Oid, Preview, RepoSnapshot, Restore, Worktree};
use crate::paths::{display, same_path};
use crate::repo::{git_dir, in_progress};

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
    let mut shown = None;
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
            "shown" => shown = Oid::from_hex(value.as_bytes()).ok(),
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
        shown,
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
        bail!("{} is not a git worktree", display(main));
    };
    let mut text = match &preview.restore {
        Restore::Branch(b) => format!("restore-branch {b}\n"),
        Restore::Detached(id) => format!("restore-detached {id}\n"),
    };
    text.push_str(&format!("source {}\n", preview.source.display()));
    text.push_str(&format!("head {}\n", preview.head));
    if let Some(shown) = preview.shown {
        text.push_str(&format!("shown {shown}\n"));
    }
    for a in &preview.added {
        text.push_str(&format!("added {a}\n"));
    }
    std::fs::write(path, text)?;
    Ok(())
}

/// Run `then` on a copy of the worktree's index, so the worktree's own
/// staging is left alone.
fn with_index_copy<T>(worktree: &Git, then: impl FnOnce(&Git) -> Result<T>) -> Result<T> {
    // Unique per call: the clock alone repeats between threads on Windows.
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let name = format!(
        "graft-preview-{}-{}.index",
        std::process::id(),
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    );
    let git_dir = git_dir(worktree.cwd());
    // Git inside WSL reads the copy best from its own disk.
    let tmp = match &git_dir {
        Some(dir) if crate::wsl::Wsl::of(dir).is_some() => dir.join(name),
        _ => std::env::temp_dir().join(name),
    };
    let real = git_dir.map(|d| d.join("index"));
    let copied = real.is_some_and(|r| std::fs::copy(r, &tmp).is_ok());
    let git = worktree.with_index(&tmp);
    let result = (|| {
        if !copied {
            git.run(["read-tree", "HEAD"])?;
        }
        then(&git)
    })();
    let _ = std::fs::remove_file(&tmp);
    result
}

/// Run `then` with every file of the worktree (untracked ones included,
/// ignored ones not) added to a copy of its index.
fn with_everything_added<T>(worktree: &Git, then: impl FnOnce(&Git) -> Result<T>) -> Result<T> {
    with_index_copy(worktree, |git| {
        git.run(["add", "-A"])?;
        then(git)
    })
}

/// The tree of main's files that leaving the preview throws away: tracked
/// ones (reset) and the ones the preview `added` (deleted). Other new files
/// stay, so they are left out (and never read: one may be locked).
fn discarded_tree(main: &Git, added: &[String]) -> Result<Oid> {
    let mut paths = Vec::new();
    for path in added {
        if std::fs::symlink_metadata(main.cwd().join(path)).is_ok() {
            paths.extend_from_slice(path.as_bytes());
            paths.push(0);
        }
    }
    with_index_copy(main, |git| {
        // Any line endings: this only compares, nothing is committed.
        let add = ["-c", "core.safecrlf=false", "add"];
        git.run(add.iter().chain(&["-u"]))?;
        if !paths.is_empty() {
            let from_stdin = ["-f", "--pathspec-from-file=-", "--pathspec-file-nul"];
            git.run_input(add.iter().chain(&from_stdin), &paths)?;
        }
        let id = git.read(["write-tree"])?;
        Ok(Oid::from_hex(id.trim().as_bytes())?)
    })
}

/// The worktree's uncommitted changes (untracked files included) as a
/// binary patch against its HEAD, and the files that patch creates.
fn uncommitted_patch(source: &Git) -> Result<(Vec<u8>, Vec<String>)> {
    with_everything_added(source, |git| {
        let patch = git
            .run([
                "diff",
                "--cached",
                "--binary",
                "--no-color",
                "--no-ext-diff",
                "--no-textconv",
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
    })
}

/// Check out `source`'s commit in main and apply its uncommitted changes.
fn show(main: &Git, source: &Git, restore: Restore) -> Result<Preview> {
    let Some(head) = source.rev("HEAD")? else {
        bail!("{} has no commits yet.", display(source.cwd()));
    };
    let (patch, mut added) = uncommitted_patch(source)?;
    main.run(["checkout", "-q", "--detach", &head.to_string()])?;
    // Only files the preview creates are its to remove later: main may
    // already have one by that name (an ignored local config, say).
    added.retain(|p| std::fs::symlink_metadata(main.cwd().join(p)).is_err());
    let mut preview = Preview {
        restore,
        source: source.cwd().to_path_buf(),
        head,
        added,
        shown: None,
    };
    write_marker(main.cwd(), &preview)?;
    if !patch.is_empty()
        && let Err(error) = main.run_input(["apply", "--whitespace=nowarn", "-"], &patch)
    {
        if let Err(rollback) = go_back(main, &preview) {
            bail!(
                "The worktree's changes could not be applied in main: {error:#}\n\
                 Going back failed too, so main still shows the worktree's commit: \
                 {rollback:#}. Use Restore main to try again."
            );
        }
        bail!("The worktree's changes could not be applied in main: {error:#}");
    }
    // Unknown, leaving the preview asks first, as if main had changed.
    preview.shown = discarded_tree(main, &preview.added).ok();
    write_marker(main.cwd(), &preview)?;
    Ok(preview)
}

/// What changed in main since the preview showed, which leaving it loses.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Changed {
    /// The files leaving the preview throws away (see [`discarded_tree`]),
    /// or whether they changed is unknown (a marker from an older Graft, a
    /// file git cannot read).
    Files,
    /// HEAD moved (e.g. a commit made there): going back leaves those
    /// commits behind.
    Head,
}

/// What changed in main since the preview showed, if anything.
pub fn main_changed(main: &Git) -> Result<Option<Changed>> {
    let Some(preview) = active(main.cwd()) else {
        bail!("Main shows no preview.");
    };
    if main.rev("HEAD")? != Some(preview.head) {
        return Ok(Some(Changed::Head));
    }
    let same = preview
        .shown
        .is_some_and(|shown| discarded_tree(main, &preview.added).ok() == Some(shown));
    Ok((!same).then_some(Changed::Files))
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
        in_progress(main.cwd()).is_some(),
        dirty,
    )?;
    let restore = match main.current_branch()? {
        Some(branch) => Restore::Branch(branch),
        None => match main.rev("HEAD")? {
            Some(id) => Restore::Detached(id),
            None => bail!("The main folder has no commits yet."),
        },
    };
    show(main, source, restore)
}

/// Throw away what the preview put into main (and anything changed there
/// since, see [`main_changed`]), without going back yet.
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

/// Show the worktree's current state again (after it changed). Discards
/// changes made in main since the preview showed, see [`main_changed`].
pub fn update(main: &Git) -> Result<Preview> {
    let Some(preview) = active(main.cwd()) else {
        bail!("Main shows no preview.");
    };
    clear(main, &preview)?;
    let source = main.at(&preview.source);
    show(main, &source, preview.restore)
}

/// Put main back on what it had before the preview. Discards changes made
/// in main since the preview showed, see [`main_changed`].
pub fn restore_main(main: &Git) -> Result<()> {
    let Some(preview) = active(main.cwd()) else {
        bail!("Main shows no preview.");
    };
    go_back(main, &preview)
}

/// Undo `preview`: clear it, check out what main had and forget the marker.
fn go_back(main: &Git, preview: &Preview) -> Result<()> {
    clear(main, preview)?;
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
