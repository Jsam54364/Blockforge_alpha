//! Priority worker pool for chunk generation and meshing.
//!
//! * Lowest priority number runs first (use squared distance to the player), ties
//!   break FIFO, so the chunks you are standing next to are never stuck behind
//!   distant ones.
//! * Thread count is capped at 3 by default: on big.LITTLE phones, using every core
//!   throttles the device and starves the render thread.
//! * Dropping the pool discards queued jobs and joins the workers.
//!
//! Known gap: if a job closure panics, that worker thread dies and `in_flight`
//! never decrements. Wrap the closure in `catch_unwind` if jobs can panic.

use std::cmp::Ordering;
use std::collections::BinaryHeap;
use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};

struct Entry<J> {
    prio: u32,
    seq: u64,
    job: J,
}

impl<J> PartialEq for Entry<J> {
    fn eq(&self, o: &Self) -> bool {
        self.prio == o.prio && self.seq == o.seq
    }
}
impl<J> Eq for Entry<J> {}
impl<J> PartialOrd for Entry<J> {
    fn partial_cmp(&self, o: &Self) -> Option<Ordering> {
        Some(self.cmp(o))
    }
}
impl<J> Ord for Entry<J> {
    // BinaryHeap pops the maximum, so invert: smallest prio, then smallest seq, wins.
    fn cmp(&self, o: &Self) -> Ordering {
        o.prio.cmp(&self.prio).then(o.seq.cmp(&self.seq))
    }
}

struct Queue<J> {
    heap: BinaryHeap<Entry<J>>,
    seq: u64,
    shutdown: bool,
}

struct Shared<J> {
    queue: Mutex<Queue<J>>,
    cv: Condvar,
    in_flight: AtomicUsize,
}

pub struct JobPool<J: Send + 'static, R: Send + 'static> {
    shared: Arc<Shared<J>>,
    results: Receiver<R>,
    handles: Vec<JoinHandle<()>>,
}

/// 1..=3 workers, leaving at least one core for the render/main thread.
pub fn default_worker_threads() -> usize {
    let n = thread::available_parallelism().map(|n| n.get()).unwrap_or(2);
    n.saturating_sub(1).clamp(1, 3)
}

fn worker_loop<J, R, F>(shared: Arc<Shared<J>>, tx: Sender<R>, f: Arc<F>)
where
    J: Send + 'static,
    R: Send + 'static,
    F: Fn(J) -> R + Send + Sync + 'static,
{
    loop {
        let job = {
            let mut q = shared.queue.lock().unwrap();
            loop {
                if let Some(e) = q.heap.pop() {
                    break Some(e.job);
                }
                if q.shutdown {
                    break None;
                }
                q = shared.cv.wait(q).unwrap();
            }
        };
        match job {
            Some(j) => {
                let r = f(j);
                shared.in_flight.fetch_sub(1, AtomicOrdering::SeqCst);
                if tx.send(r).is_err() {
                    break;
                }
            }
            None => break,
        }
    }
}

impl<J: Send + 'static, R: Send + 'static> JobPool<J, R> {
    pub fn new<F>(threads: usize, f: F) -> Self
    where
        F: Fn(J) -> R + Send + Sync + 'static,
    {
        let shared = Arc::new(Shared {
            queue: Mutex::new(Queue { heap: BinaryHeap::new(), seq: 0, shutdown: false }),
            cv: Condvar::new(),
            in_flight: AtomicUsize::new(0),
        });
        let (tx, rx) = mpsc::channel::<R>();
        let f = Arc::new(f);
        let mut handles = Vec::new();
        for i in 0..threads.max(1) {
            let shared = Arc::clone(&shared);
            let tx = tx.clone();
            let f = Arc::clone(&f);
            let h = thread::Builder::new()
                .name(format!("job-worker-{i}"))
                .spawn(move || worker_loop(shared, tx, f))
                .expect("failed to spawn worker thread");
            handles.push(h);
        }
        drop(tx);
        JobPool { shared, results: rx, handles }
    }

    pub fn submit(&self, job: J, priority: u32) {
        self.shared.in_flight.fetch_add(1, AtomicOrdering::SeqCst);
        {
            let mut q = self.shared.queue.lock().unwrap();
            q.seq += 1;
            let seq = q.seq;
            q.heap.push(Entry { prio: priority, seq, job });
        }
        self.shared.cv.notify_one();
    }

    /// Collect every finished result without blocking.
    pub fn poll(&self) -> Vec<R> {
        self.results.try_iter().collect()
    }

    /// Jobs submitted whose results have not yet been produced (queued + running).
    pub fn in_flight(&self) -> usize {
        self.shared.in_flight.load(AtomicOrdering::SeqCst)
    }
}

impl<J: Send + 'static, R: Send + 'static> Drop for JobPool<J, R> {
    fn drop(&mut self) {
        {
            let mut q = self.shared.queue.lock().unwrap();
            q.shutdown = true;
            q.heap.clear();
        }
        self.shared.cv.notify_all();
        for h in self.handles.drain(..) {
            let _ = h.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    fn drain(pool: &JobPool<u32, u32>, want: usize) -> Vec<u32> {
        let start = Instant::now();
        let mut got = Vec::new();
        while got.len() < want && start.elapsed() < Duration::from_secs(10) {
            got.extend(pool.poll());
            thread::sleep(Duration::from_millis(1));
        }
        got
    }

    #[test]
    fn runs_all_jobs() {
        let pool = JobPool::new(3, |x: u32| x * 2);
        for i in 0..100 {
            pool.submit(i, i);
        }
        let mut got = drain(&pool, 100);
        got.sort_unstable();
        assert_eq!(got, (0..100).map(|i| i * 2).collect::<Vec<_>>());
        assert_eq!(pool.in_flight(), 0);
    }

    #[test]
    fn lowest_priority_number_runs_first() {
        // One worker, blocked on a gate while we queue jobs out of order.
        let gate = Arc::new((Mutex::new(false), Condvar::new()));
        let g2 = gate.clone();
        let pool = JobPool::new(1, move |x: u32| {
            if x == 999 {
                let (m, cv) = &*g2;
                let mut open = m.lock().unwrap();
                while !*open {
                    open = cv.wait(open).unwrap();
                }
            }
            x
        });
        pool.submit(999, 0); // occupies the worker
        thread::sleep(Duration::from_millis(50));
        pool.submit(30, 30);
        pool.submit(10, 10);
        pool.submit(20, 20);
        pool.submit(11, 10); // same priority as 10, submitted later
        {
            let (m, cv) = &*gate;
            *m.lock().unwrap() = true;
            cv.notify_all();
        }
        let got = drain(&pool, 5);
        assert_eq!(got, vec![999, 10, 11, 20, 30]);
    }

    #[test]
    fn drop_with_queued_jobs_does_not_hang() {
        let pool = JobPool::new(1, |x: u32| {
            thread::sleep(Duration::from_millis(5));
            x
        });
        for i in 0..1000 {
            pool.submit(i, i);
        }
        drop(pool);
    }

    #[test]
    fn worker_count_is_capped() {
        assert!((1..=3).contains(&default_worker_threads()));
    }
}
