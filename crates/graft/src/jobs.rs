//! Background jobs of one owner (the app or a project): each job's result
//! comes back as a closure over the owner, applied on the UI thread.
//!
//! A job that panics (a bug) still ends: the owner announces the panic, and
//! the job's own handler gets a failed result, so whatever waited for the
//! job (a running operation, a loading flag, a busy dialog) ends like on any
//! failure.

use crossbeam_channel::{Receiver, Sender};
use graft_core::{JobPool, Panicked, Waker};

/// What owns jobs: it announces a job's panic.
pub trait Owner: 'static {
    fn panicked(&mut self, message: String);
}

/// What a job returns; it can say the job failed by panicking.
pub trait JobResult: Send + 'static {
    fn panicked(panic: Panicked) -> Self;
}

impl<T: Send + 'static> JobResult for anyhow::Result<T> {
    fn panicked(panic: Panicked) -> Self {
        Err(panic.into())
    }
}

impl<T: Send + 'static> JobResult for Result<T, String> {
    fn panicked(panic: Panicked) -> Self {
        Err(panic.0)
    }
}

impl<T: Send + 'static> JobResult for std::io::Result<T> {
    fn panicked(panic: Panicked) -> Self {
        Err(std::io::Error::other(panic.0))
    }
}

/// `None` for a panic: for jobs whose handler has nothing to say about a
/// failure.
impl<T: Send + 'static> JobResult for Option<T> {
    fn panicked(_: Panicked) -> Self {
        None
    }
}

type Apply<S> = Box<dyn FnOnce(&mut S) + Send>;

/// A finished job's result, applied to the owner on the UI thread.
struct Done<S>(Apply<S>);

impl<S: Owner> From<Panicked> for Done<S> {
    fn from(panic: Panicked) -> Self {
        Done(Box::new(move |owner: &mut S| owner.panicked(panic.0)))
    }
}

/// Hand `result` to `done`; a panic is announced by the owner first.
fn finish<S: Owner, T: JobResult>(
    result: Result<T, Panicked>,
    done: impl FnOnce(&mut S, T) + Send + 'static,
) -> Done<S> {
    Done(Box::new(move |owner: &mut S| match result {
        Ok(result) => done(owner, result),
        Err(panic) => {
            owner.panicked(panic.0.clone());
            done(owner, T::panicked(panic));
        }
    }))
}

pub struct Jobs<S> {
    pool: JobPool,
    tx: Sender<Done<S>>,
    rx: Receiver<Done<S>>,
}

impl<S: Owner> Jobs<S> {
    pub fn new(pool: JobPool) -> Self {
        let (tx, rx) = crossbeam_channel::unbounded();
        Self { pool, tx, rx }
    }

    pub fn pool(&self) -> &JobPool {
        &self.pool
    }

    pub fn waker(&self) -> Waker {
        self.pool.waker()
    }

    /// Run `job` on the pool; `done` gets its result on the UI thread.
    pub fn spawn<T: JobResult>(
        &self,
        job: impl FnOnce() -> T + Send + 'static,
        done: impl FnOnce(&mut S, T) + Send + 'static,
    ) {
        self.pool
            .spawn(&self.tx, move || finish(graft_core::catch(job), done));
    }

    /// Like [`Self::spawn`], on a thread of its own: for jobs that wait on
    /// the network or the user, so they cannot hold up the pool.
    pub fn spawn_long<T: JobResult>(
        &self,
        job: impl FnOnce(Waker) -> T + Send + 'static,
        done: impl FnOnce(&mut S, T) + Send + 'static,
    ) {
        self.pool.spawn_long(&self.tx, move |waker| {
            finish(graft_core::catch(|| job(waker)), done)
        });
    }

    /// Something outside the pool (e.g. a file watcher) can hand the owner
    /// `apply` with this, from any thread.
    pub fn poster(&self) -> impl Fn(Apply<S>) + Send + Sync + 'static {
        let tx = self.tx.clone();
        let waker = self.pool.waker();
        move |apply| {
            let _ = tx.send(Done(apply));
            waker();
        }
    }

    /// The next finished job's result, if one is waiting.
    pub fn next_done(&self) -> Option<Apply<S>> {
        self.rx.try_recv().ok().map(|Done(apply)| apply)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    use super::*;

    #[derive(Default)]
    struct Counter {
        sum: i32,
        failed: Vec<String>,
        panics: Vec<String>,
    }

    impl Owner for Counter {
        fn panicked(&mut self, message: String) {
            self.panics.push(message);
        }
    }

    fn add(c: &mut Counter, n: anyhow::Result<i32>) {
        match n {
            Ok(n) => c.sum += n,
            Err(e) => c.failed.push(format!("{e:#}")),
        }
    }

    /// Apply results until `until` holds.
    fn drain(jobs: &Jobs<Counter>, counter: &mut Counter, until: impl Fn(&Counter) -> bool) {
        let start = Instant::now();
        while !until(counter) {
            assert!(
                start.elapsed() < Duration::from_secs(10),
                "jobs never ended"
            );
            match jobs.next_done() {
                Some(apply) => apply(counter),
                None => std::thread::yield_now(),
            }
        }
    }

    #[test]
    fn results_come_back_to_the_owner() {
        let jobs = Jobs::<Counter>::new(JobPool::new(1, Arc::new(|| {})));
        jobs.spawn(|| Ok(2), add);
        jobs.spawn_long(|_| Ok(3), add);
        jobs.poster()(Box::new(|c| c.sum += 10));
        let mut counter = Counter::default();
        drain(&jobs, &mut counter, |c| c.sum == 15);
        assert!(counter.failed.is_empty() && counter.panics.is_empty());
    }

    #[test]
    fn a_panicking_job_ends_through_its_own_handler() {
        let jobs = Jobs::<Counter>::new(JobPool::new(1, Arc::new(|| {})));
        jobs.spawn(|| -> anyhow::Result<i32> { panic!("boom") }, add);
        jobs.spawn_long(|_| -> anyhow::Result<i32> { panic!("long boom") }, add);
        // Another job's handler is not touched by those panics.
        jobs.spawn(|| Ok(2), add);
        let mut counter = Counter::default();
        drain(&jobs, &mut counter, |c| c.failed.len() == 2 && c.sum == 2);
        assert_eq!(counter.panics.len(), 2, "each panic is announced once");
        assert!(counter.failed.iter().any(|m| m.contains("long boom")));
        assert!(counter.failed.iter().any(|m| m.ends_with(": boom")));
    }
}
