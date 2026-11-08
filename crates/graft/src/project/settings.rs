//! Per-project parts of the settings: the commit identity, and fetching in
//! the background.

use std::time::{Duration, Instant};

use graft_core::identity::{self, Identity};
use graft_core::{CancelToken, remote};

use super::ops::At;
use super::{Project, readable};

impl Project {
    fn auto_fetch_every(&self) -> Option<Duration> {
        let minutes = self.settings.auto_fetch_minutes;
        (minutes > 0).then(|| Duration::from_secs(u64::from(minutes) * 60))
    }

    /// How long until the next background fetch is due, for the app to look
    /// again then.
    pub fn next_wake(&self) -> Option<Duration> {
        let every = self.auto_fetch_every()?;
        Some(every.saturating_sub(self.last_fetch.elapsed()))
    }

    /// Fetch when the interval from the settings has passed since the last
    /// fetch, quietly: errors (offline) only show in the console.
    pub(super) fn maybe_auto_fetch(&mut self) {
        let Some(every) = self.auto_fetch_every() else {
            return;
        };
        if self.auto_fetching || self.is_operating() || self.last_fetch.elapsed() < every {
            return;
        }
        let Some(snap) = &self.snapshot else { return };
        if snap.remotes.is_empty() {
            return;
        }
        self.auto_fetching = true;
        let git = self.git.clone();
        self.jobs.spawn_long(
            move |_| remote::fetch(&git, false, &CancelToken::default(), |_| {}),
            |p, result| {
                p.auto_fetching = false;
                p.last_fetch = Instant::now();
                // Remote branches may have moved; WSL repositories send no
                // file events to notice that by.
                if result.is_ok() {
                    p.refresh();
                }
            },
        );
    }

    pub(super) fn load_identity(&mut self) {
        self.identity = None;
        let git = self.git.clone();
        self.jobs.spawn(
            move || identity::load(&git),
            |p, result| p.identity = Some(readable(result)),
        );
    }

    pub(super) fn save_identity(&mut self, identity: Identity) {
        let done = if identity.local {
            format!("Saved your identity for {}", self.name)
        } else {
            "Saved your identity for all repositories".to_owned()
        };
        let saved = identity.clone();
        self.run_op(
            At::Main,
            "Saving identity",
            move |git| identity::save(&git, &saved),
            move |p, result| match result {
                Ok(()) => {
                    p.identity = Some(Ok(identity));
                    p.infos.push(done);
                }
                Err(e) => p.report(e),
            },
        );
    }
}
