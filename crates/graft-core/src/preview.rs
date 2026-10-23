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
use crate::model::{Oid, same_path};

const MARKER: &str = "graft-preview";

/// What main goes back to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Restore {
    Branch(String),
    Detached(Oid),
}

/// A preview shown in the main worktree.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Preview {
    pub restore: Restore,
    /// The worktree being previewed.
    pub source: PathBuf,
    /// Its commit, now checked out (detached) in main.
    pub head: Oid,
    /// Files the preview created in main, removed again on restore.
    pub added: Vec<String>,
}

fn marker_path(main: &Path) -> Option<PathBuf> {
    conflicts::git_dir(main).map(|d| d.join(MARKER))
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
            "added" => added.push(value.to_owned()),
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
    let tmp = std::env::temp_dir().join(format!(
        "graft-preview-{}-{}.index",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos())
    ));
    let real = conflicts::git_dir(source.cwd()).map(|d| d.join("index"));
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
            let _ = restore_main(main);
            bail!("The worktree's changes could not be applied in main: {message}");
        }
    }
    Ok(preview)
}

/// Show the worktree at `source` in the main worktree (`main`).
pub fn start(main: &Git, source: &Git) -> Result<Preview> {
    if active(main.cwd()).is_some() {
        bail!("Main already shows a preview. Restore main first.");
    }
    if same_path(main.cwd(), source.cwd()) {
        bail!("That is the main worktree itself.");
    }
    if conflicts::in_progress(main.cwd()).is_some() {
        bail!("Main has a merge or rebase in progress. Finish or abort it first.");
    }
    if !main.read(["status", "--porcelain", "-z"])?.is_empty() {
        bail!("Main has uncommitted changes. Commit or stash them first.");
    }
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
        Restore::Branch(b) => main.run(["checkout", "-q", b])?,
        Restore::Detached(id) => main.run(["checkout", "-q", "--detach", &id.to_string()])?,
    };
    if let Some(path) = marker_path(main.cwd()) {
        std::fs::remove_file(path)?;
    }
    Ok(())
}
