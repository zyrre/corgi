//! The dashboard's `P`: Pause all, after a confirmation that names what
//! stops, or Resume all while agents are paused
//! ([`corgi_core::pause`]). The pause is kept in Corgi's state, so it
//! outlasts the dashboard, and the same key brings the agents back.

use std::time::Duration;

use corgi_core::{job::Job, pause, progress::Silent};
use crossterm::event::{KeyCode, KeyEvent};

use super::{App, Overlay};

impl App {
    /// `P`: Resume all while any agent is paused, else the confirmation.
    pub(super) fn pause_or_resume_all(&mut self) {
        if self.pause_job.is_running() {
            self.status = "Pause all or Resume all is already running".into();
            return;
        }
        if pause::paused_rows(&self.agents) > 0 {
            return self.resume_all();
        }
        let plan = pause::plan(&self.agents);
        if plan.targets.is_empty() {
            self.status = "No agent to pause".into();
            return;
        }
        self.overlay = Overlay::Pause(plan.form());
    }

    pub(super) fn handle_pause_key(&mut self, key: KeyEvent) -> bool {
        match key.code {
            KeyCode::Esc => self.cancel_editor(),
            KeyCode::Enter => self.pause_all(),
            _ => {}
        }
        false
    }

    fn pause_all(&mut self) {
        let Overlay::Pause(form) = std::mem::take(&mut self.overlay) else {
            return;
        };
        let client = self.client.clone();
        let agents = self.agents.clone();
        self.set_status(format!("Pausing {} agents…", form.agents), None);
        self.pause_job = Job::spawn(move |_| {
            pause::pause_all(&client, &agents, &mut Silent)
                .map(|outcome| outcome.message("Paused"))
                .map_err(|error| format!("Pause all failed: {error:#}"))
        });
    }

    fn resume_all(&mut self) {
        let client = self.client.clone();
        self.set_status("Resuming the paused agents…", None);
        self.pause_job = Job::spawn(move |_| {
            pause::resume_all(&client, &mut Silent)
                .map(|outcome| outcome.message("Resumed"))
                .map_err(|error| format!("Resume all failed: {error:#}"))
        });
    }

    pub(super) fn poll_pause(&mut self) {
        let Some(outcome) = self.pause_job.outcome() else {
            return;
        };
        let status = match outcome {
            Ok(Ok(status)) => {
                self.motion.succeeded();
                status
            }
            Ok(Err(status)) => status,
            Err(_) => "Pause all stopped unexpectedly".into(),
        };
        self.set_status(status, Some(Duration::from_secs(8)));
        self.request_refresh();
    }
}

#[cfg(test)]
mod tests {
    use crossterm::event::KeyModifiers;

    use super::*;
    use crate::{
        model::{AgentInfo, AgentState, DashboardAgent},
        test_support::{buffer_text, test_app, test_terminal},
    };

    fn row(pane: &str, state: AgentState, paused: bool) -> DashboardAgent {
        DashboardAgent {
            info: AgentInfo {
                pane_id: pane.into(),
                name: Some(format!("agent-{pane}")),
                agent: Some("claude".into()),
                state,
                agent_session: Some(corgi_core::model::AgentSession {
                    value: "11111111-2222-4333-8444-555555555555".into(),
                    ..Default::default()
                }),
                ..AgentInfo::default()
            },
            project_group: "weather".into(),
            exited: paused,
            paused,
            ..DashboardAgent::default()
        }
    }

    fn press(app: &mut App, key: char) {
        app.handle_key(KeyEvent::new(KeyCode::Char(key), KeyModifiers::SHIFT));
    }

    #[test]
    fn shift_p_asks_first_and_esc_stops_nothing() {
        let mut app = test_app();
        app.agents = vec![
            row("w1:p1", AgentState::Working, false),
            row("w2:p1", AgentState::Idle, false),
        ];
        press(&mut app, 'P');
        let Overlay::Pause(form) = &app.overlay else {
            panic!("no confirmation")
        };
        assert_eq!((form.agents, form.working), (2, 1));
        let mut terminal = test_terminal(100, 30);
        terminal
            .draw(|frame| crate::ui::draw(frame, &mut app))
            .unwrap();
        let screen = buffer_text(terminal.backend().buffer());
        assert!(screen.contains("Pause all agents"), "{screen}");
        assert!(screen.contains("weather: 2 workers"), "{screen}");
        assert!(screen.contains("interrupted with Esc first"), "{screen}");
        app.handle_key(KeyEvent::from(KeyCode::Esc));
        assert!(matches!(app.overlay, Overlay::None));
        assert!(!app.pause_job.is_running() && !app.pause_job.is_finished());
    }

    #[test]
    fn with_agents_paused_the_footer_and_the_key_offer_resume_all() {
        let mut app = test_app();
        app.agents = vec![row("w1:p1", AgentState::Idle, false)];
        let mut terminal = test_terminal(220, 30);
        terminal
            .draw(|frame| crate::ui::draw(frame, &mut app))
            .unwrap();
        assert!(buffer_text(terminal.backend().buffer()).contains("P pause all"));
        app.agents.push(row("w2:p1", AgentState::Unknown, true));
        terminal
            .draw(|frame| crate::ui::draw(frame, &mut app))
            .unwrap();
        assert!(buffer_text(terminal.backend().buffer()).contains("P resume all"));
        press(&mut app, 'P');
        assert!(
            matches!(app.overlay, Overlay::None),
            "no confirmation to resume"
        );
        assert!(app.pause_job.is_running() || app.pause_job.is_finished());
    }
}
