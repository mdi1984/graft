//! Slash: scan for finished branches and worktrees (after fetch --prune), let
//! the user pick, delete, and keep "Recently slashed" so a branch can be
//! brought back.

use std::path::Path;

use graft_core::paths::folder_name;
use graft_core::slash::{self, Candidate, Outcome, Plan, Slashed};
use graft_core::{Oid, is_cancelled};
use serde::{Deserialize, Serialize};

use super::ops::At;
use super::{Project, readable};
use crate::text::plural;

/// A deleted branch in "Recently slashed". Saved by the app.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SlashedBranch {
    pub branch: String,
    #[serde(with = "hex")]
    pub target: Oid,
    /// Unix time of the deletion.
    pub at: i64,
}

/// Commit ids saved as hex text.
mod hex {
    use graft_core::{Oid, null_oid};
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(id: &Oid, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(id)
    }

    /// Text that is not a commit id reads as the null id, rather than
    /// failing everything else Graft saved.
    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Oid, D::Error> {
        let text = String::deserialize(d)?;
        Ok(Oid::from_hex(text.as_bytes()).unwrap_or_else(|_| null_oid()))
    }
}

/// Branches kept in "Recently slashed".
const KEEP_SLASHED: usize = 30;

pub struct SlashView {
    /// `None` while fetching and scanning.
    pub plan: Option<Result<Plan, String>>,
    /// Per candidate: selected for deletion. A row the filter hides keeps
    /// its check, but Slash leaves it alone while hidden.
    pub checked: Vec<bool>,
    /// Shows only the rows whose branch or worktree folder name contains
    /// this text, in any case. Empty: every row.
    pub filter: String,
    pub running: bool,
    /// What the last run did, shown until the dialog closes.
    pub outcome: Option<Outcome>,
}

impl SlashView {
    /// Whether a row for `branch` and `worktree` shows under the filter:
    /// the branch name or the worktree's folder name (not its whole path)
    /// contains it.
    pub fn shows(&self, branch: Option<&str>, worktree: Option<&Path>) -> bool {
        let needle = self.filter.trim().to_lowercase();
        let has = |text: &str| text.to_lowercase().contains(&needle);
        needle.is_empty()
            || branch.is_some_and(has)
            || worktree.is_some_and(|path| has(&folder_name(path)))
    }

    /// The candidates the filter shows, with their index.
    pub fn shown(&self) -> Vec<(usize, &Candidate)> {
        let Some(Ok(plan)) = &self.plan else {
            return Vec::new();
        };
        plan.candidates
            .iter()
            .enumerate()
            .filter(|(_, c)| self.shows(c.branch.as_deref(), c.worktree.as_deref()))
            .collect()
    }

    /// What Slash acts on: the indexes of the candidates that are checked
    /// and shown.
    pub fn chosen_indexes(&self) -> Vec<usize> {
        self.shown()
            .into_iter()
            .map(|(i, _)| i)
            .filter(|i| self.is_checked(*i))
            .collect()
    }

    /// Whether every shown candidate is checked (and at least one shows):
    /// then the link unchecks them.
    pub fn all_shown_checked(&self) -> bool {
        let shown = self.shown();
        !shown.is_empty() && shown.iter().all(|(i, _)| self.is_checked(*i))
    }

    /// What the list says instead of rows, given whether any row of any
    /// list (candidates, Kept, Recently slashed) shows: a filter that
    /// matches nothing says so before "Nothing to clean up".
    pub fn empty_note(&self, any_shown: bool) -> Option<String> {
        let filter = self.filter.trim();
        if !filter.is_empty() && !any_shown {
            return Some(format!("Nothing matches '{filter}'."));
        }
        let Some(Ok(plan)) = &self.plan else {
            return None;
        };
        plan.candidates
            .is_empty()
            .then(|| "Nothing to clean up.".to_owned())
    }

    pub fn is_checked(&self, index: usize) -> bool {
        self.checked.get(index).copied().unwrap_or(false)
    }
}

impl Project {
    pub(super) fn open_slash(&mut self) {
        let protected = self.settings.protected.clone();
        let started = self.run_long(
            At::Main,
            "Slash: fetching and scanning",
            move |git, cancel, report| slash::scan(&git, &protected, cancel, report),
            |p, _, result| p.on_slash_plan(result),
        );
        if started {
            self.slash = Some(SlashView {
                plan: None,
                checked: Vec::new(),
                filter: String::new(),
                running: false,
                outcome: None,
            });
        }
    }

    fn on_slash_plan(&mut self, result: anyhow::Result<Plan>) {
        let Some(view) = &mut self.slash else { return };
        if result.as_ref().is_err_and(is_cancelled) {
            self.slash = None;
            return;
        }
        let result = readable(result);
        if let Ok(plan) = &result {
            view.checked = plan.candidates.iter().map(|c| c.suggested).collect();
        }
        view.plan = Some(result);
    }

    pub(super) fn close_slash(&mut self) {
        let Some(view) = &self.slash else { return };
        if view.running {
            return;
        }
        if view.plan.is_none() {
            self.cancel_op();
        }
        self.slash = None;
    }

    pub(super) fn toggle_slash_item(&mut self, i: usize) {
        if let Some(view) = &mut self.slash
            && let Some(on) = view.checked.get_mut(i)
        {
            *on = !*on;
        }
    }

    pub(super) fn set_slash_filter(&mut self, filter: String) {
        if let Some(view) = &mut self.slash
            && !view.running
        {
            view.filter = filter;
        }
    }

    /// Check (or uncheck) every shown candidate; hidden ones keep theirs.
    pub(super) fn check_shown_slash(&mut self, on: bool) {
        let Some(view) = &mut self.slash else { return };
        if view.running {
            return;
        }
        let shown: Vec<usize> = view.shown().into_iter().map(|(i, _)| i).collect();
        for i in shown {
            if let Some(check) = view.checked.get_mut(i) {
                *check = on;
            }
        }
    }

    /// Slash the rows at `seen`, the checked and shown rows when the user
    /// pressed Slash; if they changed since (the filter, a check), nothing
    /// runs, so Slash never acts on rows the user did not see.
    pub(super) fn run_slash(&mut self, seen: Vec<usize>) {
        let Some(view) = &self.slash else { return };
        if seen.is_empty() || view.running {
            return;
        }
        if view.chosen_indexes() != seen {
            self.infos
                .push("The Slash list changed; check it and press Slash again.".to_owned());
            return;
        }
        let Some(Ok(plan)) = &view.plan else { return };
        let chosen: Vec<_> = seen.iter().map(|&i| plan.candidates[i].clone()).collect();
        let started = self.run_long(
            At::Main,
            "Slashing",
            move |git, cancel, report| Some(slash::execute(&git, &chosen, cancel, report)),
            |p, _, outcome| p.on_slash_done(outcome),
        );
        if started && let Some(view) = &mut self.slash {
            view.running = true;
        }
    }

    /// `None`: Slash panicked, which was announced.
    fn on_slash_done(&mut self, outcome: Option<Outcome>) {
        let Some(outcome) = outcome else {
            if let Some(view) = &mut self.slash {
                view.running = false;
            }
            return;
        };
        let now = crate::time::now();
        let slashed = &mut self.memory.slashed;
        for s in outcome.slashed.iter().rev() {
            slashed.insert(
                0,
                SlashedBranch {
                    branch: s.branch.clone(),
                    target: s.target,
                    at: now,
                },
            );
        }
        slashed.truncate(KEEP_SLASHED);
        let branches = outcome.slashed.len();
        let mut parts = Vec::new();
        if branches > 0 {
            parts.push(plural(branches, "branch", "branches"));
        }
        if outcome.worktrees_removed > 0 {
            parts.push(plural(outcome.worktrees_removed, "worktree", "worktrees"));
        }
        if !parts.is_empty() {
            let stopped = if outcome.cancelled {
                ", then stopped"
            } else {
                ""
            };
            self.infos
                .push(format!("Slashed {}{stopped}", parts.join(" and ")));
        } else if outcome.cancelled {
            self.infos
                .push("Slash stopped; nothing was deleted".to_owned());
        }
        self.notices.extend(outcome.errors.iter().cloned());
        if outcome.errors.is_empty() {
            self.slash = None;
        } else if let Some(view) = &mut self.slash {
            view.running = false;
            view.outcome = Some(outcome);
        }
    }

    /// Bring a slashed branch back; it leaves "Recently slashed" once it is
    /// back.
    pub(super) fn restore_slashed(&mut self, index: usize) {
        let Some(entry) = self.memory.slashed.get(index).cloned() else {
            return;
        };
        if entry.target.is_null() {
            return;
        }
        if self
            .snapshot
            .as_ref()
            .is_some_and(|s| s.branch(&entry.branch).is_some())
        {
            self.notices.push(format!(
                "A branch named {} exists again. Rename or delete it first.",
                entry.branch
            ));
            return;
        }
        let slashed = Slashed {
            branch: entry.branch.clone(),
            target: entry.target,
        };
        self.run_op(
            At::Main,
            "Restoring",
            move |git| slash::restore(&git, &slashed),
            move |p, _, result| match result {
                Ok(()) => {
                    if let Some(i) = p.memory.slashed.iter().position(|s| *s == entry) {
                        p.memory.slashed.remove(i);
                    }
                    p.infos.push(format!("Restored {}", entry.branch));
                }
                Err(e) => p.report(e),
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::project::Action;
    use crate::testing::{git, open, tip, wait};

    /// The branches of the shown candidates, and of those Slash would act on.
    fn rows(project: &Project) -> (Vec<String>, Vec<String>) {
        let view = project.slash.as_ref().unwrap();
        let name = |c: &Candidate| c.branch.clone().unwrap();
        let shown = view.shown().into_iter().map(|(_, c)| name(c)).collect();
        let Some(Ok(plan)) = &view.plan else {
            panic!("no plan")
        };
        let chosen = view
            .chosen_indexes()
            .into_iter()
            .map(|i| name(&plan.candidates[i]))
            .collect();
        (shown, chosen)
    }

    fn index_of(project: &Project, branch: &str) -> usize {
        let view = project.slash.as_ref().unwrap();
        let Some(Ok(plan)) = &view.plan else {
            panic!("no plan")
        };
        plan.candidates
            .iter()
            .position(|c| c.branch.as_deref() == Some(branch))
            .unwrap()
    }

    /// The filter decides what Slash acts on: checked rows it hides keep
    /// their check but are left alone, and "check all shown" touches only
    /// the shown rows. It matches branch names and worktree folder names.
    #[test]
    fn slash_acts_only_on_the_checked_rows_the_filter_shows() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("app");
        std::fs::create_dir(&root).unwrap();
        git(&root, &["init", "-q", "-b", "main"]);
        std::fs::write(
            root.join("a.txt"),
            "a
",
        )
        .unwrap();
        git(&root, &["add", "a.txt"]);
        git(&root, &["commit", "-q", "-m", "first"]);
        // Merged branches with a commit of their own each.
        for branch in ["alpha-fix", "alpha-docs", "beta"] {
            git(&root, &["checkout", "-q", "-b", branch]);
            std::fs::write(
                root.join(format!("{branch}.txt")),
                "x
",
            )
            .unwrap();
            git(&root, &["add", "."]);
            git(&root, &["commit", "-q", "-m", branch]);
            git(&root, &["checkout", "-q", "main"]);
            git(&root, &["merge", "-q", "--ff-only", branch]);
        }
        // A merged branch in a worktree whose folder is named differently.
        let folder = tmp.path().join("omega-tree");
        let folder_arg = folder.to_str().unwrap();
        git(&root, &["worktree", "add", "-q", "-b", "zeta", folder_arg]);
        std::fs::write(
            folder.join("z.txt"),
            "z
",
        )
        .unwrap();
        git(&folder, &["add", "z.txt"]);
        git(&folder, &["commit", "-q", "-m", "zeta"]);
        git(&root, &["merge", "-q", "--ff-only", "zeta"]);

        let mut project = open(&root);
        project.apply(Action::OpenSlash);
        wait(&mut project, "the scan never ended", |p| {
            p.slash.as_ref().is_some_and(|v| v.plan.is_some())
        });
        let all = ["alpha-docs", "alpha-fix", "beta", "zeta"];
        let (shown, chosen) = rows(&project);
        assert_eq!(shown, all, "every row shows when the dialog opens");
        assert_eq!(chosen, all, "all are suggested");

        // Any case, any part of the name.
        project.apply(Action::SetSlashFilter("ALPHA".to_owned()));
        assert_eq!(rows(&project).0, ["alpha-docs", "alpha-fix"]);
        let docs = index_of(&project, "alpha-docs");
        project.apply(Action::ToggleSlashItem(docs));
        assert_eq!(rows(&project).1, ["alpha-fix"], "the count follows");

        // The worktree's folder name matches, its full path does not.
        project.apply(Action::SetSlashFilter("omega".to_owned()));
        assert_eq!(rows(&project), (vec!["zeta".into()], vec!["zeta".into()]));
        let parent = tmp.path().file_name().unwrap().to_str().unwrap();
        project.apply(Action::SetSlashFilter(parent.to_owned()));
        assert!(rows(&project).0.is_empty(), "matched the path");

        // Unchecking the shown rows leaves the hidden ones as they were.
        project.apply(Action::SetSlashFilter("omega".to_owned()));
        assert!(project.slash.as_ref().unwrap().all_shown_checked());
        project.apply(Action::CheckShownSlash(false));
        assert!(rows(&project).1.is_empty());
        project.apply(Action::SetSlashFilter(String::new()));
        assert_eq!(rows(&project).1, ["alpha-fix", "beta"]);
        project.apply(Action::SetSlashFilter("omega".to_owned()));
        project.apply(Action::CheckShownSlash(true));

        // Clearing the filter brings the hidden checks back.
        project.apply(Action::SetSlashFilter(String::new()));
        assert_eq!(rows(&project).1, ["alpha-fix", "beta", "zeta"]);

        // Slash pressed on what the user saw, then the filter changed in
        // the same frame (Backspace): nothing runs.
        project.apply(Action::SetSlashFilter("alpha".to_owned()));
        let seen = project.slash.as_ref().unwrap().chosen_indexes();
        project.apply(Action::SetSlashFilter(String::new()));
        project.apply(Action::RunSlash(seen.clone()));
        assert!(
            !project.slash.as_ref().unwrap().running,
            "ran on a changed list"
        );
        for branch in all {
            let tip = tip(&root, &format!("refs/heads/{branch}"));
            assert!(!tip.is_empty(), "{branch} was slashed");
        }

        // Slash under a filter: beta and zeta are checked but hidden.
        project.apply(Action::SetSlashFilter("alpha".to_owned()));
        project.apply(Action::RunSlash(seen));
        wait(&mut project, "Slash never ended", |p| p.slash.is_none());
        assert!(project.notices.is_empty(), "{:?}", project.notices);
        let exists = |branch: &str| !tip(&root, &format!("refs/heads/{branch}")).is_empty();
        assert!(!exists("alpha-fix"), "shown and checked: slashed");
        assert!(exists("alpha-docs"), "shown, not checked");
        assert!(exists("beta"), "checked, hidden");
        assert!(exists("zeta"), "checked, hidden");
        assert!(folder.exists(), "the hidden row's worktree stays");
        let slashed: Vec<_> = project.memory.slashed.iter().map(|s| &s.branch).collect();
        assert_eq!(slashed, ["alpha-fix"]);
    }

    /// With nothing to slash but Kept or Recently slashed entries, a filter
    /// that matches none of them says so, not "Nothing to clean up".
    #[test]
    fn a_filter_matching_nothing_says_so_without_candidates() {
        let mut view = SlashView {
            plan: Some(Ok(graft_core::slash::Plan {
                kept: vec![("main".into(), "current branch".into())],
                ..Default::default()
            })),
            checked: Vec::new(),
            filter: String::new(),
            running: false,
            outcome: None,
        };
        assert_eq!(
            view.empty_note(true).as_deref(),
            Some("Nothing to clean up.")
        );
        view.filter = "xyz".into();
        assert_eq!(
            view.empty_note(false).as_deref(),
            Some("Nothing matches 'xyz'.")
        );
    }

    /// Saved as before, when the id was a hex string.
    #[test]
    fn slashed_ids_are_saved_as_hex() {
        let hex = "0123456789abcdef0123456789abcdef01234567";
        let saved = format!(r#"{{"branch":"done","target":"{hex}","at":7}}"#);
        let entry: SlashedBranch = serde_json::from_str(&saved).unwrap();
        assert_eq!(entry.target.to_string(), hex);
        assert_eq!(serde_json::to_string(&entry).unwrap(), saved);
        let broken: SlashedBranch =
            serde_json::from_str(r#"{"branch":"x","target":"nope","at":1}"#).unwrap();
        assert!(broken.target.is_null());
    }
}
