//! Slash: scan for finished branches and worktrees (after fetch --prune), let
//! the user pick, delete, and keep "Recently slashed" so a branch can be
//! brought back.

use std::sync::{Arc, Mutex};

use graft_core::repo::Progress;
use graft_core::slash::{self, Candidate, Outcome, Plan, Slashed};
use graft_core::{CancelToken, Oid};
use serde::{Deserialize, Serialize};

use super::changes::{Operation, Then};
use super::{Action, Event, Project};

/// A deleted branch in "Recently slashed". Saved by the app.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SlashedBranch {
    pub branch: String,
    /// Commit id (hex).
    pub target: String,
    /// Unix time of the deletion.
    pub at: i64,
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
    fn open_slash(&mut self) {
        if self.is_operating() {
            return;
        }
        self.slash = Some(SlashView {
            plan: None,
            checked: Vec::new(),
            running: false,
            outcome: None,
        });
        let progress = Arc::new(Mutex::new(Progress::default()));
        let cancel = CancelToken::default();
        self.op = Some(Operation {
            label: "Slash: fetching and scanning…".to_owned(),
            progress: progress.clone(),
            cancel: Some(cancel.clone()),
        });
        let git = self.git.clone();
        let protected = self.settings.protected.clone();
        self.pool.spawn_long(&self.tx, move |waker| {
            let report = |p: Progress| {
                if let Ok(mut slot) = progress.lock() {
                    *slot = p;
                }
                waker();
            };
            let result =
                slash::scan(&git, &protected, &cancel, report).map_err(|e| format!("{e:#}"));
            Event::SlashPlan(result)
        });
    }

    pub(super) fn on_slash_plan(&mut self, result: Result<Plan, String>) {
        self.op = None;
        self.refresh();
        let Some(view) = &mut self.slash else { return };
        if result.as_ref().is_err_and(|e| e == "Cancelled") {
            self.slash = None;
            return;
        }
        if let Ok(plan) = &result {
            view.checked = plan.candidates.iter().map(|c| c.suggested).collect();
        }
        view.plan = Some(result);
    }

    fn run_slash(&mut self) {
        let Some(view) = &mut self.slash else { return };
        let chosen = view.chosen();
        if chosen.is_empty() || view.running || self.op.is_some() {
            return;
        }
        view.running = true;
        let git = self.git.clone();
        self.op = Some(Operation {
            label: "Slashing…".to_owned(),
            progress: Arc::default(),
            cancel: None,
        });
        self.pool.spawn(&self.tx, move || {
            Event::SlashDone(slash::execute(&git, &chosen))
        });
    }

    pub(super) fn on_slash_done(&mut self, outcome: Outcome) {
        self.op = None;
        self.refresh();
        let now = jiff::Timestamp::now().as_second();
        for s in outcome.slashed.iter().rev() {
            self.slashed.insert(
                0,
                SlashedBranch {
                    branch: s.branch.clone(),
                    target: s.target.to_string(),
                    at: now,
                },
            );
        }
        self.slashed.truncate(KEEP_SLASHED);
        let branches = outcome.slashed.len();
        let mut parts = Vec::new();
        if branches > 0 {
            parts.push(match branches {
                1 => "1 branch".to_owned(),
                n => format!("{n} branches"),
            });
        }
        if outcome.worktrees_removed > 0 {
            parts.push(match outcome.worktrees_removed {
                1 => "1 worktree".to_owned(),
                n => format!("{n} worktrees"),
            });
        }
        if !parts.is_empty() {
            self.infos.push(format!("Slashed {}", parts.join(" and ")));
        }
        for e in &outcome.errors {
            self.notices.push(e.clone());
        }
        if outcome.errors.is_empty() {
            self.slash = None;
        } else if let Some(view) = &mut self.slash {
            view.running = false;
            view.outcome = Some(outcome);
        }
    }

    fn restore_slashed(&mut self, index: usize) {
        let Some(entry) = self.slashed.get(index).cloned() else {
            return;
        };
        let Ok(target) = Oid::from_hex(entry.target.as_bytes()) else {
            return;
        };
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
            target,
        };
        self.slashed.remove(index);
        let done = format!("Restored {}", entry.branch);
        self.run_op_at(
            self.git.clone(),
            "Restoring",
            Then::Info(done),
            move |git| slash::restore(&git, &slashed).map(|()| None),
        );
    }

    pub(super) fn apply_slash_action(&mut self, action: Action) {
        match action {
            Action::OpenSlash => self.open_slash(),
            Action::CloseSlash => {
                if !self.slash.as_ref().is_some_and(|s| s.running) {
                    if self.slash.as_ref().is_some_and(|s| s.plan.is_none())
                        && let Some(op) = &self.op
                        && let Some(cancel) = &op.cancel
                    {
                        cancel.cancel();
                    }
                    self.slash = None;
                }
            }
            Action::ToggleSlashItem(i) => {
                if let Some(view) = &mut self.slash
                    && let Some(on) = view.checked.get_mut(i)
                {
                    *on = !*on;
                }
            }
            Action::RunSlash => self.run_slash(),
            Action::RestoreSlashed(i) => self.restore_slashed(i),
            _ => {}
        }
    }
}
