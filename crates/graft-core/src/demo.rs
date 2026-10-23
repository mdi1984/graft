//! Sample data used until the git engine can open a real repository.

use crate::model::{Branch, RepoSnapshot, Upstream, Worktree};

pub fn snapshot() -> RepoSnapshot {
    let wt = |branch: &str, path: &str, is_main, dirty, ahead| Worktree {
        branch: Some(branch.to_owned()),
        path: path.into(),
        is_main,
        dirty,
        ahead,
        behind: 0,
    };
    let branch = |name: &str, upstream, is_head| Branch {
        name: name.to_owned(),
        upstream,
        is_head,
    };

    RepoSnapshot {
        name: "graft".to_owned(),
        path: r"C:\dev\graft".into(),
        worktrees: vec![
            wt("main", r"C:\dev\graft", true, 4, 1),
            wt(
                "agent/review-pane",
                r"C:\dev\graft.wt\review-pane",
                false,
                4,
                0,
            ),
            wt("agent/slash-dialog", r"C:\dev\graft.wt\slash", false, 0, 2),
        ],
        branches: vec![
            branch(
                "main",
                Upstream::Tracking {
                    ahead: 1,
                    behind: 0,
                },
                true,
            ),
            branch("agent/review-pane", Upstream::None, false),
            branch(
                "agent/slash-dialog",
                Upstream::Tracking {
                    ahead: 2,
                    behind: 0,
                },
                false,
            ),
            branch("feature/slash-dialog", Upstream::Gone, false),
            branch("fix/crlf-staging", Upstream::Gone, false),
        ],
        remotes: vec!["origin".to_owned()],
        stash_count: 3,
        tag_count: 3,
    }
}
