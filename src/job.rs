//! Work handed to a background thread and collected by polling, so the
//! caller never waits on it: the daemon replaces a supervisor that hands
//! over in one, between its passes.

use std::{
    fmt,
    thread::{self, JoinHandle},
};

/// One background job. It is running from [`Job::spawn`] until
/// [`Job::outcome`] delivers how it ended, so a worker that panics is
/// reported like one that returns and never leaves its poller waiting.
pub struct Job<T> {
    worker: Option<JoinHandle<T>>,
}

/// The worker of a job panicked before returning anything.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Panicked;

impl<T: Send + 'static> Job<T> {
    /// Runs `work` on a new thread.
    pub fn spawn(work: impl FnOnce() -> T + Send + 'static) -> Self {
        Self {
            worker: Some(thread::spawn(work)),
        }
    }
}

impl<T> Job<T> {
    /// Whether the worker still runs: it has neither returned nor panicked.
    pub fn is_working(&self) -> bool {
        self.worker
            .as_ref()
            .is_some_and(|worker| !worker.is_finished())
    }

    /// How the worker ended, once it has; `None` while it runs, and after
    /// its end was delivered.
    pub fn outcome(&mut self) -> Option<Result<T, Panicked>> {
        if !self.worker.as_ref()?.is_finished() {
            return None;
        }
        Some(self.worker.take()?.join().map_err(|_| Panicked))
    }
}

impl<T> fmt::Debug for Job<T> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Job")
            .field("working", &self.is_working())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use std::{
        sync::mpsc,
        time::{Duration, Instant},
    };

    use super::*;

    /// Polls `job` until it ends.
    fn outcome_of<T>(job: &mut Job<T>) -> Result<T, Panicked> {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(outcome) = job.outcome() {
                return outcome;
            }
            assert!(Instant::now() < deadline, "the job never ended");
            thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn a_panicking_worker_is_reported_rather_than_left_running() {
        let mut job: Job<u8> = Job::spawn(|| panic!("worker failed"));
        assert_eq!(outcome_of(&mut job), Err(Panicked));
        assert!(!job.is_working());
    }

    #[test]
    fn a_job_works_from_spawn_until_its_worker_ends_and_delivers_its_end_once() {
        let (release, released) = mpsc::channel::<()>();
        let mut job: Job<bool> = Job::spawn(move || released.recv().is_err());
        assert!(job.is_working());
        assert!(job.outcome().is_none(), "still waiting to be released");

        drop(release);
        assert_eq!(outcome_of(&mut job), Ok(true));
        assert!(!job.is_working());
        assert!(
            job.outcome().is_none(),
            "an ended job delivers nothing more"
        );
    }
}
