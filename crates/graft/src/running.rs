//! Something long running in the background that the user watches: its
//! label, the latest progress git reported, and how to cancel it.

use std::sync::{Arc, Mutex};

use graft_core::{CancelToken, Progress, Waker};

pub struct Running {
    pub label: String,
    progress: Arc<Mutex<Progress>>,
    cancel: Option<CancelToken>,
}

impl Running {
    /// A short job: no progress, no cancel.
    pub fn new(label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            progress: Arc::default(),
            cancel: None,
        }
    }

    /// A job that reports progress and can be cancelled through the token.
    pub fn cancellable(label: impl Into<String>) -> (Self, CancelToken) {
        let cancel = CancelToken::default();
        let running = Self {
            cancel: Some(cancel.clone()),
            ..Self::new(label)
        };
        (running, cancel)
    }

    /// For the job: keep `p` as the latest progress and wake the UI.
    pub fn reporter(&self, waker: Waker) -> impl Fn(Progress) + Send + 'static {
        let slot = self.progress.clone();
        move |p| {
            if let Ok(mut latest) = slot.lock() {
                *latest = p;
            }
            waker();
        }
    }

    pub fn progress(&self) -> Progress {
        self.progress.lock().map(|p| p.clone()).unwrap_or_default()
    }

    pub fn can_cancel(&self) -> bool {
        self.cancel.is_some()
    }

    pub fn cancel(&self) {
        if let Some(cancel) = &self.cancel {
            cancel.cancel();
        }
    }

    /// What to wait for before closing what runs this, e.g.
    /// `Wait for "Pushing" to finish (or cancel it)`.
    pub fn wait_text(&self) -> String {
        let what = self.label.trim_end_matches('…');
        let cancel = if self.can_cancel() {
            " (or cancel it)"
        } else {
            ""
        };
        format!("Wait for \"{what}\" to finish{cancel}")
    }

    /// "Pushing… · Writing objects 42%"
    pub fn status(&self) -> String {
        let progress = self.progress();
        let mut text = self.label.clone();
        if !progress.phase.is_empty() {
            text.push_str(" · ");
            text.push_str(&progress.phase);
        }
        if let Some(fraction) = progress.fraction {
            text.push_str(&format!(" {:.0}%", fraction * 100.0));
        }
        text
    }
}
