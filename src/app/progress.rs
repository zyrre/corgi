//! Where a launch or merge reports how far it has got: the dashboard's panel
//! through its background job, the terminal for `corgi spawn`, or nowhere.

use crate::{job::Reporter, projects::ProjectMemory};

/// Takes the status lines of a launch or merge as it runs.
pub(super) trait Progress {
    /// The step now running, as the status line shows it.
    fn report(&mut self, message: String);

    /// The work moved on to its `index`th step, counted from 0, as the
    /// dashboard's step list numbers them. Only the dashboard shows steps.
    fn step(&mut self, _index: usize) {}

    /// The directory a launch's project resolved to, so it is remembered for
    /// the project list. A merge reports none.
    fn project(&mut self, _root: &str) {}
}

/// What a dashboard launch or merge sends back from its background job. A
/// merge sends no project.
pub(super) enum JobReport {
    Status(String),
    Step(usize),
    Project(String),
}

impl Progress for Reporter<JobReport> {
    fn report(&mut self, message: String) {
        self.send(JobReport::Status(message));
    }

    fn step(&mut self, index: usize) {
        self.send(JobReport::Step(index));
    }

    fn project(&mut self, root: &str) {
        self.send(JobReport::Project(root.to_string()));
    }
}

/// Progress on stderr, for a command-line launch whose stdout is its result.
pub(super) struct Stderr<'a> {
    pub(super) projects: &'a mut ProjectMemory,
}

impl Progress for Stderr<'_> {
    fn report(&mut self, message: String) {
        eprintln!("{message}");
    }

    fn project(&mut self, root: &str) {
        self.projects.remember(root);
    }
}

/// Progress nobody watches, such as a Steward's replacement.
pub(super) struct Silent;

impl Progress for Silent {
    fn report(&mut self, _message: String) {}
}
