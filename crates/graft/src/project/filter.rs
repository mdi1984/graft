//! The sidebar filter: one text that narrows every list in the sidebar
//! (worktrees, branches, remotes, tags) and the stashes. It is saved per
//! repository, so it is still there after a restart.

use graft_core::paths::folder_name;
use graft_core::stash::Stash;
use graft_core::{Branch, Remote, RemoteBranch, Tag, Worktree};

/// A typed filter as lists apply it: a row matches when its text contains
/// the filter, in any case. A blank filter matches everything. Slash's
/// filter uses the same rule.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ListFilter {
    /// Trimmed and in lower case; empty when off.
    needle: String,
}

impl ListFilter {
    pub fn new(text: &str) -> Self {
        Self {
            needle: text.trim().to_lowercase(),
        }
    }

    /// Something is typed, so lists may hide rows.
    pub fn is_on(&self) -> bool {
        !self.needle.is_empty()
    }

    pub fn matches(&self, text: &str) -> bool {
        self.needle.is_empty() || text.to_lowercase().contains(&self.needle)
    }

    /// Its branch (or "detached at …") or its folder name.
    pub fn worktree(&self, wt: &Worktree) -> bool {
        self.matches(&wt.label()) || self.matches(&folder_name(&wt.path))
    }

    pub fn branch(&self, branch: &Branch) -> bool {
        self.matches(&branch.name)
    }

    /// The full name, with the remote: `origin/feature/x`.
    pub fn remote_branch(&self, branch: &RemoteBranch) -> bool {
        self.matches(&branch.name)
    }

    /// A remote shows when its name matches or one of its branches does.
    pub fn remote(&self, remote: &Remote, branches: &[RemoteBranch]) -> bool {
        self.matches(&remote.name)
            || branches
                .iter()
                .any(|b| b.remote == remote.name && self.remote_branch(b))
    }

    pub fn tag(&self, tag: &Tag) -> bool {
        self.matches(&tag.name)
    }

    /// Its message or the branch it was made on.
    pub fn stash(&self, stash: &Stash) -> bool {
        self.matches(&stash.message) || stash.branch.as_deref().is_some_and(|b| self.matches(b))
    }
}

/// A list's count: all of them, or "shown/all" while the filter hides some.
pub fn count(shown: usize, all: usize) -> String {
    if shown == all {
        all.to_string()
    } else {
        format!("{shown}/{all}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_any_part_in_any_case_and_blank_matches_all() {
        let filter = ListFilter::new("  Feat ");
        assert!(filter.is_on());
        assert!(filter.matches("my-FEATURE/x"));
        assert!(!filter.matches("main"));
        let off = ListFilter::new("   ");
        assert!(!off.is_on());
        assert!(off.matches("anything"));
    }

    #[test]
    fn stashes_match_by_message_or_branch() {
        let stash = |message: &str, branch: Option<&str>| Stash {
            index: 0,
            id: graft_core::null_oid(),
            message: message.to_owned(),
            branch: branch.map(str::to_owned),
            time: 0,
        };
        let filter = ListFilter::new("login");
        assert!(filter.stash(&stash("WIP login form", Some("main"))));
        assert!(filter.stash(&stash("WIP", Some("feature/login"))));
        assert!(!filter.stash(&stash("WIP", None)));
    }

    #[test]
    fn a_remote_shows_by_its_name_or_one_of_its_branches() {
        let remote = |name: &str| Remote {
            name: name.to_owned(),
            url: format!("https://example.com/{name}.git"),
            push_only: false,
        };
        let branch = |remote: &str, name: &str| RemoteBranch {
            name: format!("{remote}/{name}"),
            remote: remote.to_owned(),
            target: graft_core::null_oid(),
        };
        let branches = vec![branch("origin", "main"), branch("fork", "login")];
        let filter = ListFilter::new("login");
        assert!(!filter.remote(&remote("origin"), &branches));
        assert!(filter.remote(&remote("fork"), &branches));
        assert!(ListFilter::new("orig").remote(&remote("origin"), &branches));
    }

    #[test]
    fn counts_show_shown_of_all_only_when_some_are_hidden() {
        assert_eq!(count(4, 4), "4");
        assert_eq!(count(1, 4), "1/4");
        assert_eq!(count(0, 4), "0/4");
    }
}
