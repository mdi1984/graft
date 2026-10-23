//! Repository state shared between the git engine and the UI.
//!
//! The UI only ever reads these types; the git engine (later steps) produces
//! them on background threads and sends fresh snapshots to the UI.

pub mod demo;
pub mod model;

pub use model::{Branch, RepoSnapshot, Upstream, Worktree};
