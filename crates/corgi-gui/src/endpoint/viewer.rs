//! What the window's terminal does with its endpoint connection: which tab
//! it shows, and the steps that get it there without disturbing anyone.
//!
//! Herdr's rules, from spike 3 and Herdr 0.9.0's source, which this follows:
//!
//! - An active connection claims the size of the tab it is on, the moment
//!   it is activated and whenever it moves; `tab.focus` is refused while the
//!   connection is inactive, so it cannot move first and activate after.
//!   Hence the **park tab**: one empty tab of Corgi's own. The connection is
//!   moved there before it is deactivated, so the next activation claims only
//!   the park tab's size, and is then sent `surface.set` and `tab.focus`
//!   together.
//! - A new connection starts on Herdr's focused tab, the user's. Activated,
//!   it would claim that tab's size at once; said hello already active, it
//!   claims it only when no client owns it (a tab the user's window reached
//!   through someone's JSON focus call and has not typed in since), and then
//!   keeps it until it is deactivated. So a new connection, opened active,
//!   first rests on the park tab, moving there and deactivating, which hands
//!   any tab it claimed back within milliseconds, and only then is activated
//!   there and moved to its tab. An inactive connection found anywhere but
//!   the park tab does the same.
//! - Deactivating releases every tab the connection sized, so Herdr's own
//!   window takes back the sizes of the tabs it shows.
//! - Herdr moves a connection when someone calls its JSON focus methods,
//!   creates something with focus, or closes the tab it is on. Any change of
//!   the tab it is on that the window did not ask for means the terminal is
//!   gone: the viewer parks and says so, and never follows.

use serde_json::Value;

use super::connection::{ApiError, SURFACE_SET};
use super::protocol::ClientShellSnapshot;

/// The label of the workspace whose one tab is the park tab.
pub const PARK_LABEL: &str = "corgi-gui (parked terminal)";

/// A request the viewer wants sent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    /// `client_shell.surface.set`: whether the connection gets surfaces and
    /// claims sizes.
    Activate(bool),
    /// `tab.focus` on this connection alone.
    Focus(String),
}

impl Step {
    /// The endpoint method and its parameters.
    pub fn request(&self) -> (&'static str, Value) {
        match self {
            Self::Activate(active) => (SURFACE_SET, serde_json::json!({ "active": active })),
            Self::Focus(tab_id) => ("tab.focus", serde_json::json!({ "tab_id": tab_id })),
        }
    }
}

/// What the terminal shows, for the window to draw.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    /// No snapshot yet.
    Connecting,
    /// On its way to the tab it was asked for.
    Opening,
    /// Active on the tab, which its surfaces show.
    Showing(String),
    /// Parked and inactive, or on its way there.
    Parked,
    /// The tab was closed or moved away from under the terminal, which then
    /// parked.
    Gone { tab_id: String, why: String },
}

#[derive(Debug, Default)]
pub struct Viewer {
    ready: bool,
    /// The tab the connection is on, by its latest snapshot or answer.
    location: Option<String>,
    /// The tab the window last moved it to (or found it on).
    expected: Option<String>,
    /// Whether it was last activated.
    active: bool,
    /// A new active connection that has not rested on the park tab yet, so
    /// may still hold the tab it started on.
    fresh: bool,
    park: Option<String>,
    /// The tab to show; `None` parks.
    want: Option<String>,
    lost: Option<(String, String)>,
    in_flight: Vec<(String, Step)>,
}

impl Viewer {
    /// A viewer for a connection that said hello as `active` or not.
    pub fn new(active: bool) -> Self {
        Self {
            active,
            fresh: active,
            ..Self::default()
        }
    }

    /// Show `tab_id`'s tab, or park with `None`. Asking for a tab again
    /// after it was lost tries it again.
    pub fn show(&mut self, tab_id: Option<String>) {
        if tab_id.is_some() {
            self.lost = None;
        }
        self.want = tab_id;
    }

    pub fn park_tab(&self) -> Option<&str> {
        self.park.as_deref()
    }

    /// The window made a park tab (or found one).
    pub fn set_park(&mut self, tab_id: String) {
        self.park = Some(tab_id);
    }

    /// A park tab is needed and none is known.
    pub fn needs_park(&self) -> bool {
        self.ready && self.park.is_none() && (self.want.is_some() || self.active)
    }

    pub fn snapshot(&mut self, snapshot: &ClientShellSnapshot) {
        let first = !self.ready;
        self.ready = true;
        self.location = snapshot.focused_tab_id.clone();
        if first {
            self.expected = self.location.clone();
        }
        // A park tab that was closed is forgotten; one found by its label is
        // taken.
        if let Some(park) = &self.park
            && !snapshot.tabs.iter().any(|tab| &tab.tab_id == park)
        {
            self.park = None;
        }
        if self.park.is_none() {
            self.park = snapshot
                .workspaces
                .iter()
                .find(|workspace| workspace.label == PARK_LABEL)
                .map(|workspace| workspace.active_tab_id.clone());
        }
        self.check_lost(Some(snapshot));
    }

    /// The answer to a request this viewer sent; false when `id` was not one.
    pub fn answered(&mut self, id: &str, result: &Result<Value, ApiError>) -> bool {
        let Some(index) = self.in_flight.iter().position(|(sent, _)| sent == id) else {
            return false;
        };
        let (_, step) = self.in_flight.remove(index);
        match (step, result) {
            (Step::Activate(active), Ok(_)) => {
                self.active = active;
                if !active {
                    self.fresh = false;
                }
            }
            (Step::Focus(tab_id), Ok(_)) => {
                self.location = Some(tab_id.clone());
                self.expected = Some(tab_id);
            }
            // The park tab went away. The deactivation queued behind the
            // move still runs, where the connection is; a new park tab is
            // made before anything shows again.
            (Step::Focus(tab_id), Err(_)) if Some(&tab_id) == self.park.as_ref() => {
                self.park = None;
            }
            (Step::Focus(tab_id), Err(error)) => {
                self.lose(
                    tab_id,
                    format!("Herdr could not show it: {}", error.message),
                );
            }
            (Step::Activate(_), Err(error)) => {
                let tab_id = self.want.clone().unwrap_or_default();
                self.lose(
                    tab_id,
                    format!("Herdr refused the terminal: {}", error.message),
                );
            }
        }
        self.check_lost(None);
        true
    }

    fn lose(&mut self, tab_id: String, why: String) {
        self.lost = Some((tab_id, why));
        self.want = None;
    }

    /// A tab the window did not ask for: the terminal is gone.
    fn check_lost(&mut self, snapshot: Option<&ClientShellSnapshot>) {
        if !self.active
            || self
                .in_flight
                .iter()
                .any(|(_, step)| matches!(step, Step::Focus(_)))
            || self.location == self.expected
        {
            return;
        }
        let Some(tab_id) = self.want.clone() else {
            return;
        };
        if self.expected.as_ref() != Some(&tab_id) {
            return;
        }
        let closed = snapshot.is_some_and(|s| !s.tabs.iter().any(|tab| tab.tab_id == tab_id));
        let why = if closed {
            "its tab was closed".to_string()
        } else {
            "something else moved this window's view of Herdr away from it".to_string()
        };
        self.lose(tab_id, why);
        // From here the window's moves start from where Herdr put it.
        self.expected = self.location.clone();
    }

    /// The requests to send now, in order; send each and tell [`Self::sent`]
    /// its id. Nothing while a step is still unanswered.
    pub fn next(&mut self) -> Vec<Step> {
        if !self.ready || !self.in_flight.is_empty() {
            return Vec::new();
        }
        let at_park = self.park.is_some() && self.location == self.park;
        match &self.want {
            None if !self.active => Vec::new(),
            None => match &self.park {
                Some(park) if !at_park => vec![Step::Focus(park.clone()), Step::Activate(false)],
                _ => vec![Step::Activate(false)],
            },
            // Started on the tab it is to show: whatever it claimed there is
            // wanted.
            Some(tab_id) if self.active && self.fresh && self.location.as_ref() == Some(tab_id) => {
                self.fresh = false;
                Vec::new()
            }
            Some(_) if self.active && self.fresh => match self.park.clone() {
                None => Vec::new(),
                // Started on the park tab: it claimed nothing of anyone's.
                Some(_) if at_park => {
                    self.fresh = false;
                    self.next()
                }
                Some(park) => vec![Step::Focus(park), Step::Activate(false)],
            },
            Some(tab_id) if self.active => {
                if self.location.as_ref() != Some(tab_id) {
                    vec![Step::Focus(tab_id.clone())]
                } else {
                    Vec::new()
                }
            }
            Some(tab_id) => match self.park.clone() {
                None => Vec::new(),
                Some(_) if at_park => vec![Step::Activate(true), Step::Focus(tab_id.clone())],
                // Inactive somewhere else, as when the park tab was closed
                // under it: activating there would claim that tab, so it
                // goes to the park tab and rests first.
                Some(park) => vec![
                    Step::Activate(true),
                    Step::Focus(park),
                    Step::Activate(false),
                ],
            },
        }
    }

    pub fn sent(&mut self, id: String, step: Step) {
        self.in_flight.push((id, step));
    }

    /// The connection ended: every request with it.
    pub fn reset(&mut self) {
        self.reset_active(false);
    }

    /// A new connection, which said hello `active` or not.
    pub fn reset_active(&mut self, active: bool) {
        *self = Self {
            want: self.want.clone(),
            park: self.park.clone(),
            lost: self.lost.clone(),
            active,
            fresh: active,
            ..Self::default()
        };
    }

    pub fn status(&self) -> Status {
        if let Some((tab_id, why)) = &self.lost {
            return Status::Gone {
                tab_id: tab_id.clone(),
                why: why.clone(),
            };
        }
        if !self.ready {
            return Status::Connecting;
        }
        match &self.want {
            None => Status::Parked,
            Some(tab_id)
                if self.active
                    && self.location.as_ref() == Some(tab_id)
                    && self.in_flight.is_empty() =>
            {
                Status::Showing(tab_id.clone())
            }
            Some(_) => Status::Opening,
        }
    }

    /// Parked and inactive, with nothing on the way: safe to close.
    pub fn at_rest(&self) -> bool {
        !self.active && self.in_flight.is_empty()
    }
}

#[cfg(test)]
mod tests;
