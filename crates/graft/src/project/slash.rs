//! Slash: scan for finished branches and worktrees (after fetch --prune), let
//! the user pick, delete, and keep "Recently slashed" so a branch can be
//! brought back.

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
    /// Per candidate: selected for deletion.
    pub checked: Vec<bool>,
    pub running: bool,
    /// What the last run did, shown until the dialog closes.
    pub outcome: Option<Outcome>,
}

impl SlashView {
    pub fn chosen(&self) -> Vec<Candidate> {
        let Some(Ok(plan)) = &self.plan else {
            return Vec::new();
        };
        plan.candidates
            .iter()
            .zip(&self.checked)
            .filter(|(_, on)| **on)
            .map(|(c, _)| c.clone())
            .collect()
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

    pub(super) fn run_slash(&mut self) {
        let Some(view) = &self.slash else { return };
        let chosen = view.chosen();
        if chosen.is_empty() || view.running {
            return;
        }
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
