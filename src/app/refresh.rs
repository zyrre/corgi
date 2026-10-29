//! The dashboard's side of the periodic refresh of
//! [`corgi_core::refresh`]: when one is due, running it as a background
//! [`Job`], and putting its rows in place.
//!
//! What a refresh carries from one to the next, [`RefreshState`], goes with
//! the job and comes back with its result, so the UI thread and the job share
//! nothing that needs a lock, and applying a result is only putting its rows
//! in place. At most one refresh runs at a time: the next is due
//! [`REFRESH_INTERVAL`] after the last one finished, or at once when an
//! action asked for one, so a slow Herdr means fewer refreshes, never
//! overlapping ones.

use std::{
    sync::Arc,
    thread,
    time::{Duration, Instant},
};

pub(crate) use corgi_core::refresh::*;

#[cfg(test)]
use crate::herdr::SessionSnapshot;
use crate::{job::Job, model::Activity};

use super::{App, waker::SupervisorWaker};

/// How long quitting waits for a refresh in flight, so that a wake being
/// typed into a supervisor's box is not cut off.
pub(super) const QUIT_WAIT: Duration = Duration::from_secs(2);

impl App {
    /// Makes the next pass of the dashboard loop take a fresh snapshot: at
    /// once, or as soon as the refresh running now has finished, since that
    /// one may have started before what asked for this.
    pub(super) fn request_refresh(&mut self) {
        self.refresh_requested = true;
    }

    /// Whether the dashboard loop is due for a fresh snapshot: none is
    /// running, and one was asked for or the last finished long enough ago.
    pub(super) fn refresh_due(&self) -> bool {
        !self.refresh_job.is_running()
            && (self.refresh_requested || is_due(self.last_refresh, REFRESH_INTERVAL))
    }

    /// Whether a refresh is running in the background.
    #[cfg(test)]
    pub(super) fn refreshing(&self) -> bool {
        self.refresh_job.is_running()
    }

    fn refresh_inputs(&self) -> Inputs {
        Inputs {
            client: self.client.clone(),
            dashboard_pane_id: self.dashboard_pane_id.clone(),
            codex_task_titles: self.codex_task_titles.clone(),
            transcript_pane: self
                .selected_agent()
                .filter(|_| self.expanded)
                .map(|agent| agent.info.pane_id.clone()),
            notice_path: self.notice_path.clone(),
        }
    }

    /// The refresh state, to hand to a refresh. One a panicked refresh took
    /// with it is started again from nothing, as a new dashboard's.
    fn take_refresh_state(&mut self) -> RefreshState {
        self.refresh_state.take().unwrap_or_else(|| {
            RefreshState::new(self.inboxes.clone().map(SupervisorWaker::with_inboxes))
        })
    }

    /// The refresh state while no refresh has it, for the work done in place.
    pub(super) fn refresh_state(&mut self) -> &mut RefreshState {
        self.wait_for_refresh(None);
        if self.refresh_state.is_none() {
            let state = self.take_refresh_state();
            self.refresh_state = Some(state);
        }
        self.refresh_state.as_mut().expect("a refresh state")
    }

    /// Starts a refresh in the background when one is due. None starts while
    /// a launch runs, so the snapshot never catches a launch halfway.
    pub(super) fn start_refresh(&mut self) {
        if !self.refresh_due() || self.launch_job.is_running() {
            return;
        }
        self.refresh_requested = false;
        let inputs = self.refresh_inputs();
        let state = self.take_refresh_state();
        let log = self.refresh_log.clone();
        self.refresh_job = Job::spawn(move |_| run(&inputs, state, log.as_ref()));
    }

    /// Puts a finished background refresh in place, if one has finished.
    pub(super) fn poll_refresh(&mut self) {
        match self.refresh_job.outcome() {
            None => {}
            Some(Ok(refreshed)) => self.apply_refresh(refreshed),
            // Its state went with it; the next refresh starts as usual, from
            // a new one.
            Some(Err(_)) => {
                if let Some(log) = &self.refresh_log {
                    log.append("refresh panicked");
                }
                self.last_refresh = Some(Instant::now());
                self.set_status("The refresh failed; trying again", None);
            }
        }
    }

    /// Waits for a refresh running in the background and puts it in place,
    /// for at most `limit`, or for as long as it takes without one.
    pub(super) fn wait_for_refresh(&mut self, limit: Option<Duration>) {
        let started = Instant::now();
        while self.refresh_job.is_running() {
            if limit.is_some_and(|limit| started.elapsed() >= limit) {
                return;
            }
            self.poll_refresh();
            if self.refresh_job.is_running() {
                thread::sleep(Duration::from_millis(5));
            }
        }
    }

    /// Refreshes in place, on this thread, as the command-line paths and the
    /// Omarchy bar do, which draw no dashboard and have nothing to keep
    /// responsive.
    pub(crate) fn refresh(&mut self) {
        self.wait_for_refresh(None);
        self.refresh_requested = false;
        let inputs = self.refresh_inputs();
        let state = self.take_refresh_state();
        let refreshed = run(&inputs, state, None);
        self.apply_refresh(refreshed);
    }

    /// Takes a refresh's state back and puts its rows in place.
    fn apply_refresh(&mut self, refreshed: Refreshed) {
        let Refreshed {
            state,
            view,
            notice,
            again,
        } = refreshed;
        self.refresh_state = Some(state);
        // A hook found the wake lock taken by this refresh and left the work
        // to it: the next pass goes now rather than after the interval.
        if again {
            self.request_refresh();
        }
        if notice.is_some() {
            self.pending_notice = notice;
            self.show_pending_notice();
        }
        self.last_refresh = Some(Instant::now());
        match view {
            Ok(mut view) => {
                self.connected = true;
                let status = view.status.take();
                self.install_view(view);
                if let Some((status, hold)) = status {
                    self.set_status(status, Some(hold));
                }
                let status_held = self
                    .status_hold_until
                    .is_some_and(|deadline| Instant::now() < deadline);
                if !status_held && !self.status.starts_with("Starting ") {
                    self.status_hold_until = None;
                    self.status = format!(
                        "{} agent{}",
                        self.agents.len(),
                        if self.agents.len() == 1 { "" } else { "s" }
                    );
                }
            }
            Err(error) => {
                self.connected = false;
                self.status = format!("Herdr unavailable: {error}");
            }
        }
    }

    /// Puts the rows of `snapshot` in place, as a refresh would, without
    /// the marks, the wakes or the snapshot's round trip.
    #[cfg(test)]
    pub(super) fn install_snapshot(&mut self, snapshot: SessionSnapshot) {
        let inputs = self.refresh_inputs();
        let view = self.refresh_state().view(&inputs, snapshot);
        self.install_view(view);
    }

    /// Puts one refresh's rows in place. Nothing here reads a file or asks
    /// Herdr: it runs on the UI thread.
    fn install_view(&mut self, view: View) {
        // The selection stays on its row of the list, which is a whole card
        // for a collapsed project, rather than on its index into the agents.
        let row = self.selected_stop();
        self.agents = view.agents;
        self.select_stop(row);
        self.forget_gone_sessions();
        self.refresh_codex_task_titles();
        self.note_open_projects(&view.workspaces);
        self.workspaces = view.workspaces;
        self.clamp_selection(view.transcript);
    }

    /// Keeps the selection on the list after agents went away, and the
    /// expanded transcript on the session now selected: the one the refresh
    /// read, when it read the selected session's, or else the one already
    /// shown, when that is the selected session's. Otherwise the transcript
    /// is left empty for the refresh asked for here to read.
    fn clamp_selection(&mut self, transcript: Option<(String, Arc<[Activity]>)>) {
        if self.agents.is_empty() {
            self.selected = 0;
        } else {
            self.selected = self.selected.min(self.agents.len() - 1);
        }
        if !self.expanded {
            return;
        }
        let Some(selected) = self
            .selected_agent()
            .map(|agent| agent.info.pane_id.clone())
        else {
            // The session being read is gone; there is nothing to zoom in
            // on, so fall back to the list rather than an empty panel.
            self.expanded = false;
            self.transcript = Default::default();
            self.transcript_pane = None;
            self.transcript_scroll = 0;
            return;
        };
        match transcript {
            Some((pane, entries)) if pane == selected => {
                self.transcript = entries;
                self.transcript_pane = Some(pane);
            }
            _ if self.transcript_pane.as_ref() == Some(&selected) => {}
            _ => {
                self.transcript = Default::default();
                self.transcript_pane = None;
                self.request_refresh();
            }
        }
    }
}

#[cfg(test)]
mod tests;
