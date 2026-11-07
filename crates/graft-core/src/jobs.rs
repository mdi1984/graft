//! A small fixed pool of worker threads for git work, so the UI thread never
//! blocks on git or the disk, and the token that cancels a running job.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use crossbeam_channel::{Receiver, Sender};

type Task = Box<dyn FnOnce() + Send + 'static>;

/// Called after a job delivers a result, so the UI wakes up and repaints.
pub type Waker = Arc<dyn Fn() + Send + Sync>;

/// Shared flag for cancelling a running job.
#[derive(Clone, Default)]
pub struct CancelToken(Arc<AtomicBool>);

impl CancelToken {
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Relaxed);
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }

    /// Fail with [`Cancelled`] once the job was cancelled.
    pub fn check(&self) -> anyhow::Result<()> {
        if self.is_cancelled() {
            return Err(Cancelled.into());
        }
        Ok(())
    }
}

/// The error of a job the user cancelled: nothing went wrong, so there is
/// nothing to report.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cancelled;

impl std::fmt::Display for Cancelled {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Cancelled")
    }
}

impl std::error::Error for Cancelled {}

/// Whether `error` means the job was cancelled rather than failed.
pub fn is_cancelled(error: &anyhow::Error) -> bool {
    error.downcast_ref::<Cancelled>().is_some()
}

/// A job's error on its way to the UI thread.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum JobError {
    /// The user cancelled it: nothing to report.
    Cancelled,
    /// It failed; the message is for showing.
    Failed(String),
}

impl From<anyhow::Error> for JobError {
    fn from(error: anyhow::Error) -> Self {
        if is_cancelled(&error) {
            JobError::Cancelled
        } else {
            JobError::Failed(format!("{error:#}"))
        }
    }
}

/// A job panicked (a bug). Its reply carries this instead, so whoever waits
/// for the result is not left waiting forever.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Panicked(pub String);

impl Panicked {
    fn from_payload(payload: &(dyn std::any::Any + Send)) -> Self {
        let what = payload
            .downcast_ref::<&str>()
            .map(|s| (*s).to_owned())
            .or_else(|| payload.downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "unknown error".to_owned());
        Self(format!("Something went wrong inside Graft: {what}"))
    }
}

/// Run `job`; a panic becomes a [`Panicked`] reply.
fn run_caught<T: From<Panicked>>(job: impl FnOnce() -> T) -> T {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(job))
        .unwrap_or_else(|payload| T::from(Panicked::from_payload(payload.as_ref())))
}

#[derive(Clone)]
pub struct JobPool {
    tx: Sender<Task>,
    waker: Waker,
}

impl JobPool {
    pub fn new(threads: usize, waker: Waker) -> Self {
        let (tx, rx) = crossbeam_channel::unbounded::<Task>();
        for i in 0..threads.max(1) {
            let rx: Receiver<Task> = rx.clone();
            std::thread::Builder::new()
                .name(format!("graft-job-{i}"))
                .spawn(move || {
                    for task in rx {
                        task();
                    }
                })
                .expect("spawn worker thread");
        }
        Self { tx, waker }
    }

    /// Run `job` on a worker and deliver its result through `reply`.
    pub fn spawn<T, F>(&self, reply: &Sender<T>, job: F)
    where
        T: From<Panicked> + Send + 'static,
        F: FnOnce() -> T + Send + 'static,
    {
        let reply = reply.clone();
        let waker = self.waker.clone();
        let _ = self.tx.send(Box::new(move || {
            let _ = reply.send(run_caught(job));
            waker();
        }));
    }

    /// Run a long job on its own thread (clone, fetch, push) so it cannot
    /// starve the pool. `job` gets a waker to report progress.
    pub fn spawn_long<T, F>(&self, reply: &Sender<T>, job: F)
    where
        T: From<Panicked> + Send + 'static,
        F: FnOnce(Waker) -> T + Send + 'static,
    {
        let reply = reply.clone();
        let waker = self.waker.clone();
        std::thread::spawn(move || {
            let _ = reply.send(run_caught(|| job(waker.clone())));
            waker();
        });
    }

    pub fn waker(&self) -> Waker {
        self.waker.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, PartialEq)]
    enum Reply {
        Done(u32),
        Panicked(String),
    }

    impl From<Panicked> for Reply {
        fn from(p: Panicked) -> Self {
            Reply::Panicked(p.0)
        }
    }

    #[test]
    fn a_panicking_job_still_replies() {
        let pool = JobPool::new(1, Arc::new(|| {}));
        let (tx, rx) = crossbeam_channel::unbounded();
        pool.spawn(&tx, || -> Reply { panic!("boom") });
        pool.spawn(&tx, || Reply::Done(1));
        pool.spawn_long(&tx, |_| -> Reply { panic!("long boom") });
        let mut replies: Vec<Reply> = (0..3)
            .map(|_| rx.recv_timeout(std::time::Duration::from_secs(10)).unwrap())
            .collect();
        replies.sort_by_key(|r| format!("{r:?}"));
        assert_eq!(replies[0], Reply::Done(1));
        assert!(matches!(&replies[1], Reply::Panicked(m) if m.contains("boom")));
        assert!(matches!(&replies[2], Reply::Panicked(m) if m.contains("long boom")));
    }

    #[test]
    fn cancellation_is_told_apart_from_failure() {
        let token = CancelToken::default();
        assert!(token.check().is_ok());
        token.cancel();
        let error = token.check().unwrap_err().context("while fetching");
        assert!(is_cancelled(&error));
        assert!(!is_cancelled(&anyhow::anyhow!("Cancelled")));
    }
}
