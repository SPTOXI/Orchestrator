//! A small persistent thread pool (phase M2).
//!
//! Generating one token runs a few hundred matrix-vector products; starting
//! threads for each would cost more than the work. The workers start once
//! and wait; [`Pool::run`] hands them the tasks of one product and returns
//! when all are done, the calling thread working too.

use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;

type Job = dyn Fn(usize) + Sync;

struct State {
    /// The current job; only valid while `run` is waiting for it.
    job: Option<*const Job>,
    generation: u64,
    tasks: usize,
    next: usize,
    finished: usize,
    panicked: bool,
    shutdown: bool,
}

// SAFETY: the job pointer is only dereferenced while `run` keeps the job
// alive, and the job is `Sync`.
unsafe impl Send for State {}

struct Shared {
    state: Mutex<State>,
    work: Condvar,
    done: Condvar,
}

pub struct Pool {
    shared: Arc<Shared>,
    workers: Vec<JoinHandle<()>>,
}

impl Pool {
    /// `threads` counts the caller: 1 means no extra threads.
    pub fn new(threads: usize) -> Self {
        let shared = Arc::new(Shared {
            state: Mutex::new(State {
                job: None,
                generation: 0,
                tasks: 0,
                next: 0,
                finished: 0,
                panicked: false,
                shutdown: false,
            }),
            work: Condvar::new(),
            done: Condvar::new(),
        });
        let workers = (1..threads.max(1))
            .map(|i| {
                let shared = Arc::clone(&shared);
                std::thread::Builder::new()
                    .name(format!("nativa-{i}"))
                    .spawn(move || worker(&shared))
                    .expect("could not start an engine thread")
            })
            .collect();
        Self { shared, workers }
    }

    /// Threads that work on a job, the caller included.
    pub fn threads(&self) -> usize {
        self.workers.len() + 1
    }

    /// Runs `f(0..tasks)` across the pool and waits for all of it.
    pub fn run(&self, tasks: usize, f: &(dyn Fn(usize) + Sync)) {
        if tasks == 0 {
            return;
        }
        if self.workers.is_empty() || tasks == 1 {
            (0..tasks).for_each(f);
            return;
        }
        // SAFETY: the lifetime is erased only for the duration of this call;
        // we do not return before every task has finished.
        let job: *const Job =
            unsafe { std::mem::transmute::<*const (dyn Fn(usize) + Sync + '_), *const Job>(f) };
        {
            let mut s = self.shared.state.lock().unwrap();
            s.job = Some(job);
            s.generation += 1;
            s.tasks = tasks;
            s.next = 0;
            s.finished = 0;
            s.panicked = false;
        }
        self.shared.work.notify_all();
        work_on(&self.shared);
        let mut s = self.shared.state.lock().unwrap();
        while s.finished < s.tasks {
            s = self.shared.done.wait(s).unwrap();
        }
        s.job = None;
        let panicked = s.panicked;
        drop(s);
        if panicked {
            panic!("an engine thread panicked");
        }
    }
}

impl Drop for Pool {
    fn drop(&mut self) {
        self.shared.state.lock().unwrap().shutdown = true;
        self.shared.work.notify_all();
        for w in self.workers.drain(..) {
            let _ = w.join();
        }
    }
}

/// Takes tasks of the current job until there are none left.
fn work_on(shared: &Shared) {
    loop {
        let (job, task) = {
            let mut s = shared.state.lock().unwrap();
            match s.job {
                Some(job) if s.next < s.tasks => {
                    s.next += 1;
                    (job, s.next - 1)
                }
                _ => return,
            }
        };
        // SAFETY: `run` keeps the job alive until `finished == tasks`.
        let ok = catch_unwind(AssertUnwindSafe(|| unsafe { (*job)(task) })).is_ok();
        let mut s = shared.state.lock().unwrap();
        s.finished += 1;
        s.panicked |= !ok;
        if s.finished == s.tasks {
            shared.done.notify_all();
        }
    }
}

fn worker(shared: &Shared) {
    let mut seen = 0;
    loop {
        {
            let mut s = shared.state.lock().unwrap();
            while s.generation == seen && !s.shutdown {
                s = shared.work.wait(s).unwrap();
            }
            if s.shutdown {
                return;
            }
            seen = s.generation;
        }
        work_on(shared);
    }
}

/// A raw pointer that may cross threads; users write disjoint ranges.
#[derive(Clone, Copy)]
pub(crate) struct SendPtr<T>(pub *mut T);
unsafe impl<T> Send for SendPtr<T> {}
unsafe impl<T> Sync for SendPtr<T> {}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn runs_every_task_once() {
        let pool = Pool::new(4);
        for tasks in [1, 3, 4, 17, 100] {
            let hits: Vec<AtomicUsize> = (0..tasks).map(|_| AtomicUsize::new(0)).collect();
            pool.run(tasks, &|i| {
                hits[i].fetch_add(1, Ordering::Relaxed);
            });
            assert!(hits.iter().all(|h| h.load(Ordering::Relaxed) == 1));
        }
    }

    #[test]
    fn many_jobs_in_a_row() {
        let pool = Pool::new(3);
        let total = AtomicUsize::new(0);
        for _ in 0..2000 {
            pool.run(6, &|i| {
                total.fetch_add(i, Ordering::Relaxed);
            });
        }
        assert_eq!(total.load(Ordering::Relaxed), 2000 * 15);
    }

    #[test]
    fn a_panic_reaches_the_caller_and_the_pool_survives() {
        let pool = Pool::new(2);
        let r = catch_unwind(AssertUnwindSafe(|| {
            pool.run(4, &|i| {
                if i == 2 {
                    panic!("boom");
                }
            })
        }));
        assert!(r.is_err());
        let n = AtomicUsize::new(0);
        pool.run(4, &|_| {
            n.fetch_add(1, Ordering::Relaxed);
        });
        assert_eq!(n.load(Ordering::Relaxed), 4);
    }
}
