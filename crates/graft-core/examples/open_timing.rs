//! Time the steps of opening a repository: `cargo run --release -p graft-core --example open_timing -- <repo>`.
use std::time::Instant;

use graft_core::console::Console;
use graft_core::history::{self, History};
use graft_core::{CancelToken, Git, snapshot};

fn main() {
    let root = std::path::PathBuf::from(std::env::args().nth(1).expect("repo path"));
    let git = Git::new(&root, Console::default());
    let t = Instant::now();
    let snap = snapshot::load(&git).unwrap();
    println!("snapshot: {:?}", t.elapsed());
    let tips = snap.tips();
    let t = Instant::now();
    let commits = history::walk(&root, &tips, 100_000, &CancelToken::default()).unwrap();
    println!("walk: {:?} ({} commits)", t.elapsed(), commits.len());
    let t = Instant::now();
    let h = History::build(&commits, None);
    println!("build: {:?} ({} rows)", t.elapsed(), h.commits.len());
    let roots: Vec<_> = snap.worktrees.iter().map(|w| w.path.clone()).collect();
    let t = Instant::now();
    let w = graft_core::watch::Watcher::new(roots, || {}).unwrap();
    println!("watcher: {:?}", t.elapsed());
    drop(w);
}
