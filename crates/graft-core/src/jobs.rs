//! A small fixed pool of worker threads for git work, so the UI thread never
//! blocks on git or the disk.

use std::sync::Arc;

use crossbeam_channel::{Receiver, Sender};

type Task = Box<dyn FnOnce() + Send + 'static>;

/// Called after a job delivers a result, so the UI wakes up and repaints.
pub type Waker = Arc<dyn Fn() + Send + Sync>;

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
                        // A panicking job must not take the worker down with it.
                        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(task));
                    }
                })
                .expect("spawn worker thread");
        }
        Self { tx, waker }
    }

    /// Run `job` on a worker and deliver its result through `reply`.
    pub fn spawn<T, F>(&self, reply: &Sender<T>, job: F)
    where
        T: Send + 'static,
        F: FnOnce() -> T + Send + 'static,
    {
        let reply = reply.clone();
        let waker = self.waker.clone();
        let _ = self.tx.send(Box::new(move || {
            let result = job();
            let _ = reply.send(result);
            waker();
        }));
    }

    /// Run a long job on its own thread (clone, fetch, push) so it cannot
    /// starve the pool. `job` gets a waker to report progress.
    pub fn spawn_long<T, F>(&self, reply: &Sender<T>, job: F)
    where
        T: Send + 'static,
        F: FnOnce(Waker) -> T + Send + 'static,
    {
        let reply = reply.clone();
        let waker = self.waker.clone();
        std::thread::spawn(move || {
            let result = job(waker.clone());
            let _ = reply.send(result);
            waker();
        });
    }

    pub fn waker(&self) -> Waker {
        self.waker.clone()
    }
}
