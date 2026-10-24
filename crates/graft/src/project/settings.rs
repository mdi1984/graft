//! Per-project parts of the settings: the commit identity, and fetching in
//! the background.

use std::time::{Duration, Instant};

use graft_core::identity::{self, Identity};
use graft_core::{CancelToken, remote};

use super::changes::Then;
use super::{Action, Event, Project};

impl Project {
    /// Fetch when the interval from the settings has passed since the last
    /// fetch, quietly: errors (offline) only show in the console.
    pub(super) fn maybe_auto_fetch(&mut self) {
        let minutes = self.settings.auto_fetch_minutes;
        if minutes == 0
            || self.auto_fetching
            || self.is_operating()
            || self.last_fetch.elapsed() < Duration::from_secs(u64::from(minutes) * 60)
        {
            return;
        }
        let Some(snap) = &self.snapshot else { return };
        if snap.remotes.is_empty() {
            return;
        }
        self.auto_fetching = true;
        let git = self.git.clone();
        // A fetch can take a while; it must not hold up the pool that loads
        // diffs and status.
        self.pool.spawn_long(&self.tx, move |_| {
            let result =
                remote::fetch(&git, &CancelToken::default(), |_| {}).map_err(|e| format!("{e:#}"));
            Event::AutoFetched(result)
        });
    }

    /// No refresh here: if the fetch moved any remote branch, the file
    /// watcher sees it in .git/refs and refreshes once.
    pub(super) fn on_auto_fetched(&mut self, _result: Result<(), String>) {
        self.auto_fetching = false;
        self.last_fetch = Instant::now();
    }

    pub(super) fn apply_settings_action(&mut self, action: Action) {
        match action {
            Action::LoadIdentity => {
                self.identity = None;
                let git = self.git.clone();
                self.pool.spawn(&self.tx, move || {
                    Event::Identity(identity::load(&git).map_err(|e| format!("{e:#}")))
                });
            }
            Action::SaveIdentity(identity) => {
                let done = if identity.local {
                    format!("Saved your identity for {}", self.name)
                } else {
                    "Saved your identity for all repositories".to_owned()
                };
                self.identity = Some(Ok(identity.clone()));
                self.run_op_at(
                    self.git.clone(),
                    "Saving identity",
                    Then::Info(done),
                    move |git| identity::save(&git, &identity).map(|()| None),
                );
            }
            _ => {}
        }
    }

    pub(super) fn on_identity(&mut self, result: Result<Identity, String>) {
        self.identity = Some(result);
    }
}
