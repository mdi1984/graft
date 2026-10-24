//! Graft's git engine. Everything here runs on background threads; the UI
//! only ever sees the plain data types it produces.
//!
//! Reads that must be fast on big repositories (history walk) use gix.
//! Everything else goes through `git.exe` with machine-readable output, so the
//! results match git exactly and hooks, config and credentials keep working.

pub mod cli;
pub mod conflicts;
pub mod console;
pub mod details;
pub mod diff;
pub mod history;
pub mod jobs;
pub mod model;
pub mod ops;
pub mod refs;
pub mod remote;
pub mod repo;
pub mod snapshot;
pub mod stash;
pub mod status;
pub mod watch;
pub mod worktree;

pub use cli::{CancelToken, Git};
pub use console::Console;
pub use model::{
    Branch, Oid, Remote, RemoteBranch, RepoSnapshot, Tag, Upstream, Worktree, null_oid, same_path,
    short,
};
