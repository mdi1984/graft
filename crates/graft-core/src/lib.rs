//! Graft's git engine. Everything here runs on background threads; the UI
//! only ever sees the plain data types it produces.
//!
//! Reads that must be fast on big repositories (history walk) use gix.
//! Everything else goes through `git.exe` with machine-readable output, so the
//! results match git exactly and hooks, config and credentials keep working.

pub mod changes;
mod cli;
pub mod conflicts;
pub mod console;
pub mod details;
pub mod diff;
pub mod file_log;
pub mod history;
pub mod identity;
mod jobs;
mod model;
pub mod ops;
pub mod paths;
pub mod preview;
pub mod refs;
pub mod refusal;
pub mod remote;
pub mod repo;
pub mod review;
pub mod slash;
pub mod snapshot;
pub mod stash;
pub mod status;
pub mod syntax;
pub mod watch;
pub mod worktree;
pub mod wsl;

// The basic types, each reachable at one path: here.
pub use cli::{Git, Output, Progress};
pub use jobs::{CancelToken, JobPool, Panicked, Waker, catch, is_cancelled, is_panic};
pub use model::{
    Branch, Change, EntryKind, FileList, InProgress, Oid, Operation, Preview, Remote, RemoteBranch,
    RepoSnapshot, Restore, StatusEntry, Tag, Upstream, Worktree, WorktreeStatus, null_oid, short,
};
