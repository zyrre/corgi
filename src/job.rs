//! Work the dashboard hands to a background thread: the thread, the messages
//! it sends as it goes, and the value it ends with, collected by polling from
//! the UI loop so the UI thread never waits on it.

use std::{
    fmt,
    sync::mpsc::{self, Receiver, Sender},
    thread::{self, JoinHandle},
};

/// One background job, or none: a default job is idle. A job is running from
/// [`Job::spawn`] until the poll that delivers how it ended, so a worker that
/// panics is reported like one that returns and never leaves its poller
/// waiting.
pub struct Job<T, M = ()> {
    running: Option<Running<T, M>>,
}

struct Running<T, M> {
    messages: Receiver<M>,
    worker: JoinHandle<T>,
}

/// Where a job's worker sends its messages.
pub struct Reporter<M>(Sender<M>);

impl<M> Reporter<M> {
    /// Passes `message` to the job's next poll. A job dropped by its poller
    /// no longer listens, and its worker carries on regardless.
    pub fn send(&self, message: M) {
        let _ = self.0.send(message);
    }
}

/// One thing a poll found, in the order the worker produced it.
#[derive(Debug)]
pub enum Update<T, M> {
    /// A message the worker sent while it ran.
    Progress(M),
    /// The value the worker returned.
    Finished(T),
    /// The worker panicked before returning anything.
    Panicked,
}

/// The worker of a job panicked before returning anything.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Panicked;

impl<T: Send + 'static, M: Send + 'static> Job<T, M> {
    /// Runs `work` on a new thread, giving it the reporter for its messages.
    pub fn spawn(work: impl FnOnce(Reporter<M>) -> T + Send + 'static) -> Self {
        let (tx, messages) = mpsc::channel();
        let worker = thread::spawn(move || work(Reporter(tx)));
        Self {
            running: Some(Running { messages, worker }),
        }
    }
}

impl<T, M> Job<T, M> {
    /// Whether the job has been started and its end not yet polled.
    pub fn is_running(&self) -> bool {
        self.running.is_some()
    }

    /// Everything the worker has produced since the last poll, ending with
    /// how it ended once it has. Once that is delivered, the job is idle.
    pub fn poll(&mut self) -> Vec<Update<T, M>> {
        let Some(running) = &self.running else {
            return Vec::new();
        };
        if !running.worker.is_finished() {
            return running.messages.try_iter().map(Update::Progress).collect();
        }
        let Running { messages, worker } = self.running.take().expect("a running job");
        // Joined before the messages are read, so every message the worker
        // sent is in the channel and comes before its outcome.
        let outcome = match worker.join() {
            Ok(value) => Update::Finished(value),
            Err(_) => Update::Panicked,
        };
        let mut updates: Vec<_> = messages.try_iter().map(Update::Progress).collect();
        updates.push(outcome);
        updates
    }

    /// How the worker ended, once it has, for a job whose messages do not
    /// matter.
    pub fn outcome(&mut self) -> Option<Result<T, Panicked>> {
        self.poll().into_iter().find_map(|update| match update {
            Update::Progress(_) => None,
            Update::Finished(value) => Some(Ok(value)),
            Update::Panicked => Some(Err(Panicked)),
        })
    }
}

impl<T, M> Default for Job<T, M> {
    fn default() -> Self {
        Self { running: None }
    }
}

impl<T, M> fmt::Debug for Job<T, M> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Job")
            .field("running", &self.is_running())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::*;

    /// Polls `job` until it ends, collecting everything it produced.
    fn poll_to_end<T, M>(job: &mut Job<T, M>) -> Vec<Update<T, M>> {
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut updates = Vec::new();
        while job.is_running() {
            assert!(Instant::now() < deadline, "the job never ended");
            updates.extend(job.poll());
            thread::sleep(Duration::from_millis(5));
        }
        updates
    }

    #[test]
    fn a_job_delivers_its_messages_then_its_value() {
        let mut job = Job::spawn(|progress| {
            progress.send("one");
            progress.send("two");
            42
        });
        let updates = poll_to_end(&mut job);
        assert!(
            matches!(
                updates.as_slice(),
                [
                    Update::Progress("one"),
                    Update::Progress("two"),
                    Update::Finished(42)
                ]
            ),
            "{updates:?}"
        );
    }

    #[test]
    fn a_panicking_worker_is_reported_rather_than_left_running() {
        let mut job: Job<u8, &str> = Job::spawn(|progress| {
            progress.send("before");
            panic!("worker failed");
        });
        let updates = poll_to_end(&mut job);
        assert!(
            matches!(
                updates.as_slice(),
                [Update::Progress("before"), Update::Panicked]
            ),
            "{updates:?}"
        );
    }

    #[test]
    fn a_job_runs_from_spawn_until_its_end_is_polled() {
        let mut idle: Job<()> = Job::default();
        assert!(!idle.is_running());
        assert!(idle.poll().is_empty());

        let (release, released) = mpsc::channel::<()>();
        let mut job: Job<bool> = Job::spawn(move |_| released.recv().is_err());
        assert!(job.is_running());
        assert!(job.outcome().is_none());
        assert!(job.is_running(), "still waiting to be released");

        drop(release);
        let deadline = Instant::now() + Duration::from_secs(5);
        let outcome = loop {
            if let Some(outcome) = job.outcome() {
                break outcome;
            }
            assert!(Instant::now() < deadline, "the job never ended");
            thread::sleep(Duration::from_millis(5));
        };
        assert_eq!(outcome, Ok(true));
        assert!(!job.is_running());
        assert!(job.poll().is_empty(), "an ended job delivers nothing more");
    }
}
