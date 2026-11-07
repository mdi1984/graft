//! Repositories inside WSL, opened through `\\wsl.localhost\<distro>\...`,
//! against real throwaway repositories in the default distro. Skipped where
//! no distro with git is installed (e.g. CI).

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use graft_core::watch::Watcher;
use graft_core::worktree::{self, Start};
use graft_core::wsl::Wsl;
use graft_core::{CancelToken, Console, Git, conflicts, history, ops, preview, repo, snapshot};

/// A temporary folder inside the distro, removed on drop.
struct Distro {
    name: String,
    wsl: Wsl,
    tmp: String,
}

/// Say that `test` did not run, past the test harness's output capture (so
/// it shows in a normal `cargo test` run), and let it pass.
fn skipped(test: &str) {
    let _ = writeln!(
        std::io::stderr(),
        "skipped: {test} needs a WSL distro with git, none found"
    );
}

impl Distro {
    /// The default distro, or `None` (and a visible skip) without one.
    fn start(test: &str) -> Option<Self> {
        let distro = Self::find();
        if distro.is_none() {
            skipped(test);
        }
        distro
    }

    fn find() -> Option<Self> {
        let out = Command::new("wsl.exe")
            .args([
                "--exec",
                "sh",
                "-c",
                "command -v git >/dev/null && echo $WSL_DISTRO_NAME",
            ])
            .output()
            .ok()?;
        let name = String::from_utf8_lossy(&out.stdout).trim().to_owned();
        if !out.status.success() || name.is_empty() {
            return None;
        }
        let mut distro = Self {
            wsl: Wsl::new(&name),
            name,
            tmp: String::new(),
        };
        distro.tmp = distro.sh("mktemp -d");
        Some(distro)
    }

    /// Run a shell script inside the distro, in the temporary folder.
    fn sh(&self, script: &str) -> String {
        let out = Command::new("wsl.exe")
            .args(["-d", &self.name, "--exec", "sh", "-c"])
            .arg(format!("cd {} 2>/dev/null; {script}", self.tmp))
            .output()
            .expect("wsl.exe runs");
        assert!(
            out.status.success(),
            "{script} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim().to_owned()
    }

    /// The Windows path of `rel` inside the temporary folder.
    fn path(&self, rel: &str) -> PathBuf {
        self.wsl.to_windows(&format!("{}/{rel}", self.tmp))
    }
}

impl Drop for Distro {
    fn drop(&mut self) {
        if self.tmp.starts_with("/tmp/") {
            let _ = Command::new("wsl.exe")
                .args(["-d", &self.name, "--exec", "rm", "-rf", &self.tmp])
                .status();
        }
    }
}

/// `app` with a commit (including an executable script) and a linked
/// worktree `app.worktrees/feature`.
fn repo_with_worktree(d: &Distro) {
    d.sh("git init -q -b main app && cd app \
         && git config user.name Ada && git config user.email ada@example.com \
         && printf 'one\\n' > readme.md && printf '#!/bin/sh\\n' > run.sh && chmod +x run.sh \
         && git add . && git commit -q -m init \
         && git worktree add -q -b feature ../app.worktrees/feature");
}

#[test]
fn open_status_worktrees_and_history() {
    let Some(d) = Distro::start("open_status_worktrees_and_history") else {
        return;
    };
    repo_with_worktree(&d);
    let app = d.path("app");
    let feature = d.path("app.worktrees/feature");
    let console = Console::default();

    // Opening either worktree finds the main one, as Windows paths.
    let loc = repo::locate(&app, &console).unwrap();
    assert_eq!(loc.root, app);
    let loc = repo::locate(&feature, &console).unwrap();
    assert_eq!((loc.root, loc.opened), (app.clone(), feature.clone()));

    // Both worktrees listed with Windows paths; the executable script is
    // not a change (Windows git would report it as modified).
    let git = Git::new(&app, console.clone());
    let snap = snapshot::load(&git).unwrap();
    let paths: Vec<&Path> = snap.worktrees.iter().map(|w| w.path.as_path()).collect();
    assert_eq!(paths, [app.as_path(), feature.as_path()]);
    assert!(snap.worktrees[0].is_main && !snap.worktrees[1].is_main);
    for wt in &snap.worktrees {
        assert_eq!(wt.status.as_ref().unwrap().entries, [], "{:?}", wt.path);
    }

    // Files written from Windows show up; staging and committing work,
    // including literal paths and the message passed on stdin.
    std::fs::write(app.join("x[ab].txt"), "x\n").unwrap();
    std::fs::write(app.join("xa.txt"), "x\n").unwrap();
    ops::stage_paths(&git, &["x[ab].txt".to_owned()]).unwrap();
    let staged: Vec<String> = graft_core::status::load(&git)
        .unwrap()
        .entries
        .into_iter()
        .filter(|e| e.kind == graft_core::status::EntryKind::Tracked)
        .map(|e| e.path)
        .collect();
    assert_eq!(staged, ["x[ab].txt"]);
    ops::commit(&git, "Brackets äöü\n\nbody", false).unwrap();
    assert_eq!(d.sh("cd app && git log -1 --format=%s"), "Brackets äöü");

    // History through gix, also when opened at the linked worktree.
    let tips = [snap.worktrees[1].head.unwrap()];
    let commits = history::walk(&feature, &tips, 100, &CancelToken::default()).unwrap();
    assert_eq!(commits.list.len(), 1);
}

#[test]
fn worktrees_clone_and_conflicts() {
    let Some(d) = Distro::start("worktrees_clone_and_conflicts") else {
        return;
    };
    repo_with_worktree(&d);
    let app = d.path("app");
    let feature = d.path("app.worktrees/feature");
    let console = Console::default();
    let git = Git::new(&app, console.clone());

    // Add, lock and remove a worktree by its Windows path.
    let path = worktree::default_path(&app, "fix/one");
    let start = Start::NewBranch {
        name: "fix/one".into(),
        base: "main".into(),
    };
    worktree::add(&git, &path, &start).unwrap();
    assert!(path.join("readme.md").exists());
    let snap = snapshot::load(&git).unwrap();
    assert!(snap.worktrees.iter().any(|w| w.path == path));
    let locked = || {
        let snap = snapshot::load(&git).unwrap();
        snap.worktrees
            .iter()
            .find(|w| w.path == path)
            .unwrap()
            .locked
    };
    worktree::set_locked(&git, &path, true).unwrap();
    assert!(locked());
    worktree::set_locked(&git, &path, false).unwrap();
    assert!(!locked());
    worktree::remove(&git, &path, false).unwrap();
    assert!(!path.exists());

    // A folder Linux git cannot reach is refused, not created as a file
    // name with backslashes inside the repository.
    let share = Path::new(r"\\server\share\wt");
    let err = worktree::add(&git, share, &Start::Existing("main".into())).unwrap_err();
    assert!(format!("{err:#}").contains("is not inside"), "{err:#}");
    assert_eq!(d.sh("cd app && git status --porcelain"), "");

    // A merge stopped in the linked worktree is seen through its `.git` file,
    // which holds a Linux path.
    d.sh(
        "cd app && printf 'main\\n' > readme.md && git commit -qam main \
         && cd ../app.worktrees/feature && printf 'feature\\n' > readme.md \
         && git commit -qam feature && (git merge -q main >/dev/null 2>&1 || true)",
    );
    assert!(conflicts::in_progress(&feature).is_some());
    let wt = Git::new(&feature, console.clone());
    let file = conflicts::load(&wt, "readme.md").unwrap();
    conflicts::take_side(&wt, &file, true).unwrap();
    let stopped_again = conflicts::continue_op(&wt, conflicts::Kind::Merge).unwrap();
    assert!(!stopped_again);
    assert!(conflicts::in_progress(&feature).is_none());

    // Clone into a WSL folder.
    let dest = d.path("copy");
    let url = format!("{}/app", d.tmp);
    let loc = repo::clone(&url, &dest, &console, &CancelToken::default(), |_| {}).unwrap();
    assert_eq!(loc.root, dest);
    assert!(dest.join("readme.md").exists());
}

#[test]
fn preview_in_main() {
    let Some(d) = Distro::start("preview_in_main") else {
        return;
    };
    repo_with_worktree(&d);
    let app = d.path("app");
    let feature = d.path("app.worktrees/feature");
    let console = Console::default();
    std::fs::write(feature.join("new.txt"), "agent work\n").unwrap();

    // The worktree's uncommitted changes are built with a copy of its index.
    let main = Git::new(&app, console.clone());
    preview::start(&main, &Git::new(&feature, console)).unwrap();
    assert_eq!(
        std::fs::read_to_string(app.join("new.txt")).unwrap(),
        "agent work\n"
    );
    preview::restore_main(&main).unwrap();
    assert!(!app.join("new.txt").exists());
    assert_eq!(d.sh("cd app && git status --porcelain"), "");
}

#[test]
fn distros_and_home_folders() {
    let Some(d) = Distro::start("distros_and_home_folders") else {
        return;
    };
    assert!(graft_core::wsl::distros().contains(&d.name));
    let wsl = &d.wsl;
    let home = wsl.home().expect("home folder");
    assert_eq!(home, d.sh("echo $HOME"));
    // `~/x` typed on the start page is the folder git sees as `$HOME/x`.
    let typed = wsl.resolve("~/x", Some(&home)).unwrap();
    assert_eq!(wsl.to_linux(&typed).unwrap(), format!("{home}/x"));
}

#[test]
fn changes_inside_wsl_are_noticed() {
    let Some(d) = Distro::start("changes_inside_wsl_are_noticed") else {
        return;
    };
    repo_with_worktree(&d);
    let changes = Arc::new(AtomicUsize::new(0));
    let seen = changes.clone();
    let _watcher = Watcher::new(
        vec![d.path("app"), d.path("app.worktrees/feature")],
        move || {
            seen.fetch_add(1, Ordering::SeqCst);
        },
    )
    .unwrap();
    let wait = |what: &str| {
        let started = Instant::now();
        while changes.swap(0, Ordering::SeqCst) == 0 {
            assert!(
                started.elapsed() < Duration::from_secs(15),
                "missed: {what}"
            );
            std::thread::sleep(Duration::from_millis(200));
        }
    };
    // The first check always refreshes once (it has nothing to compare
    // with yet); every change after it must be noticed on its own.
    wait("first check");
    d.sh("printf 'x\\n' > app.worktrees/feature/new.txt");
    wait("new file in the linked worktree");
    d.sh("cd app && git branch other");
    wait("new branch");
    // Files that are already changed: every further edit counts too.
    d.sh("printf 'two\\n' >> app/readme.md");
    wait("first edit of a tracked file");
    d.sh("printf 'three\\n' >> app/readme.md");
    wait("second edit of the same file");
    d.sh("printf 'y\\n' >> app.worktrees/feature/new.txt");
    wait("second edit of an untracked file");
    // Nothing changed: no refresh.
    std::thread::sleep(Duration::from_secs(5));
    assert_eq!(changes.load(Ordering::SeqCst), 0);
}
