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
    /// again then; zero means now. `None` while none can start by itself:
    /// auto-fetch is off, the repository is loading or has no remotes, or a
    /// fetch or another operation runs. Whatever ends those states (a job,
    /// a refresh) wakes the app anyway.
    pub fn next_wake(&self) -> Option<Duration> {
        let every = self.auto_fetch_every()?;
        let snap = self.snapshot.as_ref()?;
        if self.auto_fetching || self.is_operating() || snap.remotes.is_empty() {
            return None;
        }
        Some(every.saturating_sub(self.last_fetch.elapsed()))
    }

    /// Fetch when [`Self::next_wake`] says it is due, quietly: errors
    /// (offline) only show in the console.
    pub(super) fn maybe_auto_fetch(&mut self) {
        if self.next_wake() != Some(Duration::ZERO) {
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
            move |p, _, result| match result {
                Ok(()) => {
                    p.identity = Some(Ok(identity));
                    p.infos.push(done);
                }
                Err(e) => p.report(e),
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{git, open, wait};

    /// While no fetch can start (here: no remote yet), the app is not woken
    /// at all, rather than at once on every frame; once one can, it is due.
    #[test]
    fn no_wake_while_auto_fetch_cannot_run() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("r");
        std::fs::create_dir_all(&root).unwrap();
        git(
            dir.path(),
            &["init", "-q", "--bare", "-b", "main", "origin.git"],
        );
        git(&root, &["init", "-q", "-b", "main"]);
        git(&root, &["commit", "-q", "--allow-empty", "-m", "first"]);
        let mut project = open(&root);
        project.settings.auto_fetch_minutes = 10;
        project.last_fetch = Instant::now()
            .checked_sub(Duration::from_secs(11 * 60))
            .unwrap();
        assert_eq!(project.next_wake(), None, "no remote: nothing to wait for");
        project.poll();
        assert!(!project.auto_fetching);

        let origin = dir.path().join("origin.git");
        git(
            &root,
            &["remote", "add", "origin", origin.to_str().unwrap()],
        );
        project.refresh();
        wait(&mut project, "the remote never showed", |p| {
            p.snapshot.as_ref().is_some_and(|s| !s.remotes.is_empty())
        });
        // Due now: the next poll starts it, and its end sets the next time.
        project.poll();
        assert!(project.auto_fetching);
        assert_eq!(project.next_wake(), None, "a fetch runs");
        wait(&mut project, "the fetch never ended", |p| !p.auto_fetching);
        let next = project.next_wake().unwrap();
        assert!(next > Duration::from_secs(9 * 60), "{next:?}");
    }
}
