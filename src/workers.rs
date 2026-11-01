//! Shared workers for day-file reads from the UI and MCP.
use std::collections::BTreeMap;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::{Arc, Mutex, OnceLock, mpsc};

type Job = Box<dyn FnOnce() + Send>;

struct Pool(mpsc::SyncSender<Job>);
impl Pool {
    fn new(threads: usize) -> Result<Self, String> {
        let threads = threads.max(1);
        let (send, receive) = mpsc::sync_channel::<Job>(threads * 2);
        let receive = Arc::new(Mutex::new(receive));
        for i in 0..threads {
            let receive = Arc::clone(&receive);
            std::thread::Builder::new()
                .name(format!("history-{i}"))
                .spawn(move || {
                    loop {
                        let job = receive.lock().unwrap().recv();
                        let Ok(job) = job else { break };
                        job();
                    }
                })
                .map_err(|e| format!("Could not start a history worker: {e}"))?;
        }
        Ok(Self(send))
    }

    fn map<T: Send + 'static, R: Send + 'static>(
        &self,
        inputs: Vec<T>,
        work: impl Fn(T) -> R + Send + Sync + 'static,
    ) -> Result<Vec<R>, String> {
        let work = Arc::new(work);
        let (send, receive) = mpsc::channel();
        for (i, input) in inputs.into_iter().enumerate() {
            let send = send.clone();
            let work = Arc::clone(&work);
            self.0
                .send(Box::new(move || {
                    let result = catch_unwind(AssertUnwindSafe(|| work(input)))
                        .map_err(|_| "A history worker panicked".to_string());
                    let _ = send.send((i, result));
                }))
                .map_err(|_| "History workers closed".to_string())?;
        }
        drop(send);
        // Merging out of day order would change each hop's latest reply and jitter.
        receive
            .into_iter()
            .map(|(i, result)| result.map(|value| (i, value)))
            .collect::<Result<BTreeMap<_, _>, _>>()
            .map(|results| results.into_values().collect())
    }
}

pub(super) fn history_map<T: Send + 'static, R: Send + 'static>(
    inputs: Vec<T>,
    work: impl Fn(T) -> R + Send + Sync + 'static,
) -> Result<Vec<R>, String> {
    if inputs.is_empty() {
        return Ok(Vec::new());
    }
    static POOL: OnceLock<Result<Pool, String>> = OnceLock::new();
    POOL.get_or_init(|| {
        // Each worker can hold a whole day file. Leave a core for the UI and probes,
        // and cap parallel reads even on machines with many cores.
        let threads = std::thread::available_parallelism()
            .map_or(2, |n| n.get().saturating_sub(1))
            .clamp(1, 4);
        Pool::new(threads)
    })
    .as_ref()
    .map_err(Clone::clone)?
    .map(inputs, work)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Barrier;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn concurrent_requests_share_the_limit_and_keep_their_result_order() {
        let pool = Pool::new(2).unwrap();
        let barrier = Arc::new(Barrier::new(2));
        let active = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));
        std::thread::scope(|scope| {
            let requests: Vec<_> = (0..2)
                .map(|request| {
                    let pool = &pool;
                    let (barrier, active, peak) =
                        (Arc::clone(&barrier), Arc::clone(&active), Arc::clone(&peak));
                    scope.spawn(move || {
                        let inputs: Vec<_> = (request * 10..request * 10 + 8).collect();
                        let results = pool
                            .map(inputs.clone(), move |i| {
                                let running = active.fetch_add(1, Ordering::SeqCst) + 1;
                                peak.fetch_max(running, Ordering::SeqCst);
                                barrier.wait();
                                active.fetch_sub(1, Ordering::SeqCst);
                                i * i
                            })
                            .unwrap();
                        assert_eq!(results, inputs.iter().map(|i| i * i).collect::<Vec<_>>());
                    })
                })
                .collect();
            for request in requests {
                request.join().unwrap();
            }
        });
        assert_eq!(peak.load(Ordering::SeqCst), 2);
        assert_eq!(active.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn a_panicking_job_reports_failure_and_the_worker_can_be_reused() {
        let pool = Pool::new(1).unwrap();
        let result = pool.map(vec![0, 1, 2], |i| {
            assert_ne!(i, 1, "bad history job");
            i
        });
        assert!(matches!(result, Err(e) if e == "A history worker panicked"));
        assert_eq!(pool.map(vec![3], |i| i * i).unwrap(), vec![9]);
    }
}
