use std::collections::VecDeque;
use std::sync::{Condvar, Mutex, OnceLock};

type Job = Box<dyn FnOnce() + Send + 'static>;

const MAX_WORKERS: usize = 4;
const MAX_QUEUED_JOBS: usize = 64;

struct QueueState {
    jobs: VecDeque<Job>,
    shutdown: bool,
}

struct Shared {
    state: Mutex<QueueState>,
    ready: Condvar,
}

struct WorkerPool {
    shared: std::sync::Arc<Shared>,
    workers: Vec<std::thread::JoinHandle<()>>,
}

impl WorkerPool {
    fn new(worker_count: usize, queue_capacity: usize) -> Self {
        let worker_count = worker_count.max(1);
        let shared = std::sync::Arc::new(Shared {
            state: Mutex::new(QueueState {
                jobs: VecDeque::with_capacity(queue_capacity),
                shutdown: false,
            }),
            ready: Condvar::new(),
        });
        let workers = (0..worker_count)
            .map(|index| {
                let shared = std::sync::Arc::clone(&shared);
                std::thread::Builder::new()
                    .name(format!("logotomy-worker-{index}"))
                    .spawn(move || loop {
                        let job = {
                            let mut state = shared.state.lock().unwrap();
                            while state.jobs.is_empty() && !state.shutdown {
                                state = shared.ready.wait(state).unwrap();
                            }
                            if state.shutdown {
                                return;
                            }
                            state.jobs.pop_front()
                        };
                        if let Some(job) = job {
                            job();
                        }
                    })
                    .expect("failed to start GUI worker")
            })
            .collect();
        Self { shared, workers }
    }

    fn submit(&self, job: Job, queue_capacity: usize) {
        let mut state = self.shared.state.lock().unwrap();
        if state.jobs.len() >= queue_capacity {
            // Cancel flags make superseded work cheap once running. Dropping
            // the oldest still-queued closure also releases its document Arc
            // before bursts can retain an unbounded amount of memory.
            state.jobs.pop_front();
        }
        state.jobs.push_back(job);
        self.shared.ready.notify_one();
    }
}

impl Drop for WorkerPool {
    fn drop(&mut self) {
        {
            let mut state = self.shared.state.lock().unwrap();
            state.shutdown = true;
            state.jobs.clear();
        }
        self.shared.ready.notify_all();
        for worker in self.workers.drain(..) {
            let _ = worker.join();
        }
    }
}

fn global() -> &'static WorkerPool {
    static POOL: OnceLock<WorkerPool> = OnceLock::new();
    POOL.get_or_init(|| {
        let available = std::thread::available_parallelism()
            .map(usize::from)
            .unwrap_or(2);
        WorkerPool::new(available.clamp(1, MAX_WORKERS), MAX_QUEUED_JOBS)
    })
}

pub(crate) fn spawn(job: impl FnOnce() + Send + 'static) {
    global().submit(Box::new(job), MAX_QUEUED_JOBS);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::time::Duration;

    #[test]
    fn worker_count_bounds_concurrent_jobs() {
        let pool = WorkerPool::new(2, 16);
        let active = Arc::new(AtomicUsize::new(0));
        let maximum = Arc::new(AtomicUsize::new(0));
        let (done_tx, done_rx) = crossbeam_channel::bounded(8);
        for _ in 0..8 {
            let active = Arc::clone(&active);
            let maximum = Arc::clone(&maximum);
            let done_tx = done_tx.clone();
            pool.submit(
                Box::new(move || {
                    let now = active.fetch_add(1, Ordering::SeqCst) + 1;
                    maximum.fetch_max(now, Ordering::SeqCst);
                    std::thread::sleep(Duration::from_millis(10));
                    active.fetch_sub(1, Ordering::SeqCst);
                    done_tx.send(()).unwrap();
                }),
                16,
            );
        }
        for _ in 0..8 {
            done_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        }
        assert!(maximum.load(Ordering::SeqCst) <= 2);
    }
}
