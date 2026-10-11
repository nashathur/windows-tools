//! Serial queue for reads of the local timetable.

use crate::{data, worker};
use std::sync::{mpsc, OnceLock};

type Job = Box<dyn FnOnce() + Send + 'static>;

fn sender() -> &'static mpsc::Sender<Job> {
    static SENDER: OnceLock<mpsc::Sender<Job>> = OnceLock::new();
    SENDER.get_or_init(|| {
        let (tx, rx) = mpsc::channel::<Job>();
        std::thread::Builder::new().name("bus-timetable".into()).spawn(move || {
            while let Ok(job) = rx.recv() {
                job();
            }
        }).expect("start timetable queue");
        tx
    })
}

/// Adds one timetable read to the single worker queue.
pub(crate) fn submit(job: impl FnOnce() + Send + 'static) {
    if sender().send(Box::new(job)).is_err() {
        data::log(worker::data_dir(), "timetable queue thread is gone; dropping job");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// Verifies queued jobs run in order and never overlap.
    #[test]
    fn runs_jobs_one_at_a_time_in_order() {
        let (tx, rx) = mpsc::channel();
        let active = std::sync::Arc::new(AtomicUsize::new(0));
        let peak = std::sync::Arc::new(AtomicUsize::new(0));
        for n in 0..4 {
            let tx = tx.clone();
            let active = active.clone();
            let peak = peak.clone();
            submit(move || {
                let now = active.fetch_add(1, Ordering::SeqCst) + 1;
                peak.fetch_max(now, Ordering::SeqCst);
                tx.send((n, true)).unwrap();
                tx.send((n, false)).unwrap();
                active.fetch_sub(1, Ordering::SeqCst);
            });
        }
        drop(tx);
        let events: Vec<_> = rx.iter().collect();
        assert_eq!(events, vec![(0, true), (0, false), (1, true), (1, false), (2, true), (2, false), (3, true), (3, false)]);
        assert_eq!(peak.load(Ordering::SeqCst), 1);
    }
}
