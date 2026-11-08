//! Background jobs of one owner (the app or a project): each job's result
//! comes back as a closure over the owner, applied on the UI thread.

use crossbeam_channel::{Receiver, Sender};
use graft_core::{JobPool, Panicked, Waker};

/// What owns jobs: it says what to do when one panicked.
pub trait Owner: 'static {
    fn panicked(&mut self, message: String);
}

type Apply<S> = Box<dyn FnOnce(&mut S) + Send>;

/// A finished job's result, applied to the owner on the UI thread.
struct Done<S>(Apply<S>);

impl<S: Owner> From<Panicked> for Done<S> {
    fn from(panic: Panicked) -> Self {
        Done(Box::new(move |owner: &mut S| owner.panicked(panic.0)))
    }
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
    pub fn spawn<T: Send + 'static>(
        &self,
        job: impl FnOnce() -> T + Send + 'static,
        done: impl FnOnce(&mut S, T) + Send + 'static,
    ) {
        self.pool.spawn(&self.tx, move || {
            let result = job();
            Done(Box::new(move |owner: &mut S| done(owner, result)))
        });
    }

    /// Like [`Self::spawn`], on a thread of its own: for jobs that wait on
    /// the network or the user, so they cannot hold up the pool.
    pub fn spawn_long<T: Send + 'static>(
        &self,
        job: impl FnOnce(Waker) -> T + Send + 'static,
        done: impl FnOnce(&mut S, T) + Send + 'static,
    ) {
        self.pool.spawn_long(&self.tx, move |waker| {
            let result = job(waker);
            Done(Box::new(move |owner: &mut S| done(owner, result)))
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
        panics: Vec<String>,
    }

    impl Owner for Counter {
        fn panicked(&mut self, message: String) {
            self.panics.push(message);
        }
    }

    #[test]
    fn results_and_panics_come_back_to_the_owner() {
        let jobs = Jobs::<Counter>::new(JobPool::new(1, Arc::new(|| {})));
        jobs.spawn(|| 2, |c, n| c.sum += n);
        jobs.spawn_long(|_| 3, |c, n| c.sum += n);
        jobs.spawn(|| -> i32 { panic!("boom") }, |c, n| c.sum += n);
        jobs.poster()(Box::new(|c| c.sum += 10));
        let mut counter = Counter::default();
        let start = Instant::now();
        while counter.sum != 15 || counter.panics.is_empty() {
            assert!(
                start.elapsed() < Duration::from_secs(10),
                "jobs never ended"
            );
            match jobs.next_done() {
                Some(apply) => apply(&mut counter),
                None => std::thread::yield_now(),
            }
        }
        assert!(counter.panics[0].contains("boom"), "{:?}", counter.panics);
    }
}
