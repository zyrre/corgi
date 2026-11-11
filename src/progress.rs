//! Where a launch or merge reports how far it has got: stderr for `corgi
//! spawn`, or nowhere.

/// Takes the status lines of a launch or merge as it runs.
pub trait Progress {
    /// The step now running.
    fn report(&mut self, message: String);
}

/// Progress nobody watches, such as a supervisor's replacement.
pub struct Silent;

impl Progress for Silent {
    fn report(&mut self, _message: String) {}
}

/// Keeps the status lines a launch step reports, for a test to read.
#[cfg(test)]
#[derive(Default)]
pub struct Recorded(pub Vec<String>);

#[cfg(test)]
impl Progress for Recorded {
    fn report(&mut self, message: String) {
        self.0.push(message);
    }
}
