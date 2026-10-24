//! Watching worktrees so the UI refreshes by itself when files or refs change.
//!
//! Events are debounced, and changes that cannot affect what Graft shows are
//! dropped: git's object store and logs, lock files, and anything gitignored
//! (build output churning in `target/` must not trigger a refresh every tick).

use std::collections::HashSet;
use std::io::Write;
use std::path::{Component, Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use anyhow::Result;
use notify_debouncer_full::notify::{self, RecommendedWatcher, RecursiveMode};
use notify_debouncer_full::{DebounceEventResult, Debouncer, NoCache, new_debouncer_opt};

use crate::cli;

const DEBOUNCE: Duration = Duration::from_millis(300);
/// More changed paths than this in one batch: refresh without asking git.
const MAX_CHECK: usize = 2_000;

pub struct Watcher {
    _debouncer: Debouncer<RecommendedWatcher, NoCache>,
    roots: Vec<PathBuf>,
}

impl Watcher {
    /// Watch the given worktree folders recursively (the main worktree's
    /// folder includes `.git`). `on_change` runs on a background thread.
    pub fn new(roots: Vec<PathBuf>, on_change: impl Fn() + Send + 'static) -> Result<Self> {
        let mut filter = Filter {
            roots: roots.clone(),
            ignored: HashSet::new(),
        };
        // No file id cache: building it walks every file under the roots
        // (node_modules included), which took 38 s on a big repo. It only
        // serves rename tracking, which Graft does not need.
        let handler = move |result: DebounceEventResult| {
            let Ok(events) = result else {
                // Overflowed or failed: refresh to be safe.
                on_change();
                return;
            };
            let paths: Vec<&Path> = events
                .iter()
                .flat_map(|e| e.paths.iter().map(PathBuf::as_path))
                .collect();
            if filter.relevant(&paths) {
                on_change();
            }
        };
        let mut debouncer = new_debouncer_opt::<_, RecommendedWatcher, _>(
            DEBOUNCE,
            None,
            handler,
            NoCache::new(),
            notify::Config::default(),
        )?;
        for root in &roots {
            // A worktree inside another one (e.g. `.claude/worktrees/x`) is
            // already covered by the outer recursive watch.
            let nested = roots.iter().any(|r| r != root && root.starts_with(r));
            if root.is_dir() && !nested {
                debouncer.watch(root, RecursiveMode::Recursive)?;
            }
        }
        Ok(Self {
            _debouncer: debouncer,
            roots,
        })
    }

    pub fn roots(&self) -> &[PathBuf] {
        &self.roots
    }
}

struct Filter {
    roots: Vec<PathBuf>,
    /// Folders (root index, relative path) known to be gitignored.
    ignored: HashSet<(usize, PathBuf)>,
}

impl Filter {
    fn relevant(&mut self, paths: &[&Path]) -> bool {
        // Worktree files to ask git about, with the root they are in.
        let mut unknown: Vec<(usize, PathBuf)> = Vec::new();
        for path in paths {
            let Some((root, rel)) = self.locate(path) else {
                continue;
            };
            if let Some(inside) = inside_git_dir(&rel) {
                if git_file_matters(&inside) {
                    return true;
                }
                continue;
            }
            if rel.file_name().is_some_and(|n| n == ".gitignore") {
                self.ignored.clear();
                return true;
            }
            if rel
                .ancestors()
                .any(|a| self.ignored.contains(&(root, a.to_path_buf())))
            {
                continue;
            }
            unknown.push((root, rel));
        }
        if unknown.is_empty() {
            return false;
        }
        if unknown.len() > MAX_CHECK {
            return true;
        }
        let mut relevant = false;
        for root in 0..self.roots.len() {
            let files: Vec<&PathBuf> = unknown
                .iter()
                .filter(|(r, _)| *r == root)
                .map(|(_, p)| p)
                .collect();
            if files.is_empty() {
                continue;
            }
            // Ask about each file and its folders, so an ignored folder is
            // remembered and its churn never reaches git again.
            let mut queries: Vec<PathBuf> = Vec::new();
            for file in &files {
                for a in file.ancestors().filter(|a| !a.as_os_str().is_empty()) {
                    if !queries.iter().any(|q| q == a) {
                        queries.push(a.to_path_buf());
                    }
                }
            }
            let Some(ignored) = check_ignore(&self.roots[root], &queries) else {
                return true;
            };
            for file in &files {
                // The outermost ignored folder covers the most future churn.
                match file.ancestors().filter(|a| ignored.contains(*a)).last() {
                    Some(dir) if dir != file.as_path() => {
                        self.ignored.insert((root, dir.to_path_buf()));
                    }
                    Some(_) => {}
                    None => relevant = true,
                }
            }
        }
        relevant
    }

    /// The most specific watched root `path` is in, and the path relative to it.
    fn locate(&self, path: &Path) -> Option<(usize, PathBuf)> {
        self.roots
            .iter()
            .enumerate()
            .filter_map(|(i, root)| path.strip_prefix(root).ok().map(|rel| (i, rel)))
            .min_by_key(|(_, rel)| rel.components().count())
            .map(|(i, rel)| (i, rel.to_path_buf()))
    }
}

/// The part of `rel` after a `.git` folder, if it is inside one.
fn inside_git_dir(rel: &Path) -> Option<PathBuf> {
    let mut components = rel.components();
    for c in components.by_ref() {
        if c == Component::Normal(".git".as_ref()) {
            return Some(components.as_path().to_path_buf());
        }
    }
    None
}

/// Whether a change inside `.git` can change what Graft shows.
fn git_file_matters(inside: &Path) -> bool {
    let text = inside.to_string_lossy().replace('\\', "/");
    if text.ends_with(".lock") {
        return false;
    }
    // A linked worktree's own HEAD and index live in .git/worktrees/<name>/.
    let text = match text.strip_prefix("worktrees/") {
        Some(rest) => rest.split_once('/').map_or("", |(_, r)| r).to_owned(),
        None => text,
    };
    matches!(
        text.as_str(),
        "HEAD" | "index" | "packed-refs" | "MERGE_HEAD" | "CHERRY_PICK_HEAD" | "REVERT_HEAD"
    ) || text.starts_with("refs/")
        || text.starts_with("rebase-merge")
        || text.starts_with("rebase-apply")
}

/// Which of `paths` (relative to `root`) git ignores, or `None` if git could
/// not tell.
fn check_ignore(root: &Path, paths: &[PathBuf]) -> Option<HashSet<PathBuf>> {
    let mut child = cli::command(root)
        .args(["check-ignore", "-z", "--stdin"])
        // check-ignore refuses literal pathspecs; it reads plain paths anyway.
        .env_remove("GIT_LITERAL_PATHSPECS")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let mut input = Vec::new();
    for p in paths {
        input.extend_from_slice(p.to_string_lossy().replace('\\', "/").as_bytes());
        input.push(0);
    }
    let mut stdin = child.stdin.take()?;
    std::thread::spawn(move || {
        let _ = stdin.write_all(&input);
    });
    let out = child.wait_with_output().ok()?;
    // 0: some ignored, 1: none ignored, anything else: an error.
    if !matches!(out.status.code(), Some(0 | 1)) {
        return None;
    }
    Some(
        out.stdout
            .split(|b| *b == 0)
            .filter(|s| !s.is_empty())
            .map(|s| String::from_utf8_lossy(s).split('/').collect::<PathBuf>())
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn git_dir_filter() {
        let m = |s: &str| git_file_matters(Path::new(s));
        assert!(m("HEAD") && m("index") && m("refs/heads/main") && m("packed-refs"));
        assert!(m("worktrees/agent/HEAD") && m("worktrees/agent/index"));
        assert!(!m("objects/ab/cdef") && !m("index.lock") && !m("logs/HEAD"));
        assert!(!m("FETCH_HEAD") && !m("worktrees/agent/logs/HEAD"));
        assert_eq!(
            inside_git_dir(Path::new(".git/refs/heads/x")),
            Some(PathBuf::from("refs/heads/x"))
        );
        assert_eq!(inside_git_dir(Path::new("src/main.rs")), None);
    }

    #[test]
    fn ignored_folders_are_learned() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_path_buf();
        let git = |args: &[&str]| {
            std::process::Command::new("git")
                .args(args)
                .current_dir(&root)
                .output()
                .unwrap()
        };
        git(&["init", "-q"]);
        std::fs::write(root.join(".gitignore"), "target/\n").unwrap();
        std::fs::create_dir_all(root.join("target/debug")).unwrap();
        std::fs::write(root.join("target/debug/x.o"), "x").unwrap();
        std::fs::write(root.join("main.rs"), "x").unwrap();

        let mut f = Filter {
            roots: vec![root.clone()],
            ignored: HashSet::new(),
        };
        let build = root.join("target").join("debug").join("x.o");
        assert!(!f.relevant(&[build.as_path()]));
        assert!(f.ignored.contains(&(0, PathBuf::from("target"))));
        assert!(f.relevant(&[root.join("main.rs").as_path()]));
        assert!(!f.relevant(&[root.join(".git").join("objects").join("ab").as_path()]));
        assert!(f.relevant(&[root.join(".git").join("HEAD").as_path()]));
    }

    #[test]
    fn nested_worktree_changes_are_seen() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("app");
        std::fs::create_dir(&root).unwrap();
        let git = |args: &[&str]| {
            let out = std::process::Command::new("git")
                .args(args)
                .current_dir(&root)
                .output()
                .unwrap();
            assert!(out.status.success(), "git {args:?} failed");
        };
        git(&["init", "-q", "-b", "main"]);
        std::fs::write(root.join(".gitignore"), ".claude/\n").unwrap();
        git(&["add", "."]);
        git(&[
            "-c",
            "user.name=A",
            "-c",
            "user.email=a@x",
            "commit",
            "-qm",
            "a",
        ]);
        git(&[
            "worktree",
            "add",
            "-q",
            "-b",
            "agent",
            ".claude/worktrees/agent",
        ]);
        let nested = root.join(".claude").join("worktrees").join("agent");

        let (tx, rx) = crossbeam_channel::unbounded();
        let _watcher = Watcher::new(vec![root.clone(), nested.clone()], move || {
            let _ = tx.send(());
        })
        .unwrap();
        std::thread::sleep(Duration::from_millis(200));
        std::fs::write(nested.join("new.txt"), "x").unwrap();
        assert!(rx.recv_timeout(Duration::from_secs(10)).is_ok());
    }
}
