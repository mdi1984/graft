//! App-wide preferences, saved with the rest of Graft's state.

use graft_core::slash::PROTECTED;
use graft_core::worktree::DEFAULT_COPY_PATTERNS;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Command that opens a folder or file in the editor, e.g. `code`.
    pub ide_command: String,
    /// Fetch every open repository this often; 0 = never.
    pub auto_fetch_minutes: u32,
    /// Branch names Slash never proposes (comma-separated, `*` matches anything).
    pub protected: String,
    /// Ignored files copied into new worktrees.
    pub copy_patterns: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            ide_command: "code".to_owned(),
            auto_fetch_minutes: 10,
            protected: PROTECTED.to_owned(),
            copy_patterns: DEFAULT_COPY_PATTERNS.to_owned(),
        }
    }
}
