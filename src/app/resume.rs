//! The dashboard's `R`: starts an agent that dropped to its shell again in
//! its pane, with its session and the options Corgi launched it with
//! ([`crate::restart::resume`]). Only a row for such an agent offers
//! it; Corgi never resumes one by itself.

use std::time::Duration;

use crate::{job::Job, restart};

use super::App;

/// What the status line says on a row that cannot be resumed.
pub(super) const NOT_EXITED: &str = "R resumes an agent that exited to its shell; this one runs";

impl App {
    pub(super) fn resume_selected(&mut self) {
        let Some(agent) = self.selected_agent() else {
            self.status = "No agent selected".into();
            return;
        };
        if !agent.exited {
            self.status = NOT_EXITED.into();
            return;
        }
        if self.resume_job.is_running() {
            self.status = "A resume is already starting".into();
            return;
        }
        let client = self.client.clone();
        let pane_id = agent.info.pane_id.clone();
        let name = agent.info.display_name().to_string();
        self.set_status(format!("Resuming {name}…"), None);
        self.resume_job = Job::spawn(move |_| {
            restart::resume(&client, &pane_id)
                .map(|_| format!("Resumed {name} in its pane with its session and options"))
                .map_err(|error| format!("Resume failed: {error:#}"))
        });
    }

    pub(super) fn poll_resume(&mut self) {
        let Some(outcome) = self.resume_job.outcome() else {
            return;
        };
        let status = match outcome {
            Ok(Ok(status)) => {
                self.motion.succeeded();
                status
            }
            Ok(Err(status)) => status,
            Err(_) => "Resume stopped unexpectedly".into(),
        };
        self.set_status(status, Some(Duration::from_secs(6)));
        self.request_refresh();
    }

    /// The selected row, refused with a status line when its agent has
    /// exited: such a row takes no prompt and no focus.
    pub(super) fn exited_refusal(&mut self) -> bool {
        let Some(agent) = self.selected_agent().filter(|agent| agent.exited) else {
            return false;
        };
        self.status = format!(
            "{} has exited to its shell; R resumes it",
            agent.info.display_name()
        );
        true
    }
}

#[cfg(test)]
mod tests {
    use crossterm::event::{KeyCode, KeyModifiers};

    use super::*;
    use crate::{
        model::{AgentInfo, DashboardAgent},
        test_support::{buffer_text, test_app, test_terminal},
    };

    fn row(exited: bool, missing: &[&str]) -> DashboardAgent {
        DashboardAgent {
            info: AgentInfo {
                agent: Some("claude".into()),
                name: Some("fix-cache".into()),
                pane_id: "w1:p1".into(),
                workspace_id: "w1".into(),
                tab_id: "w1:t1".into(),
                ..AgentInfo::default()
            },
            project_group: "repo".into(),
            project: "repo".into(),
            task: "Fix the cache".into(),
            exited,
            missing_options: missing.iter().map(|item| item.to_string()).collect(),
            ..DashboardAgent::default()
        }
    }

    #[test]
    fn r_resumes_only_an_agent_that_exited() {
        let mut app = test_app();
        app.agents = vec![row(false, &[])];
        super::super::test_helpers::press(&mut app, KeyCode::Char('R'), KeyModifiers::SHIFT);
        assert_eq!(app.status, NOT_EXITED);
        assert!(!app.resume_job.is_running() && !app.resume_job.is_finished());

        // A scratch record keeps the real state directory out of it.
        let scratch = crate::test_support::ScratchDir::new("tui-resume");
        app.client =
            crate::herdr::HerdrClient::from_socket_path(crate::test_support::UNUSED_SOCKET)
                .with_marker_record(scratch.join("markers.json"));
        app.agents = vec![row(true, &[])];
        super::super::test_helpers::press(&mut app, KeyCode::Char('R'), KeyModifiers::SHIFT);
        assert_eq!(app.status, "Resuming fix-cache…");
        // The test client has no record of the pane, so the job refuses.
        super::super::test_helpers::poll_until(&mut app, App::poll_resume, |app| {
            app.status.starts_with("Resume failed")
        });
    }

    #[test]
    fn an_exited_row_takes_no_prompt_or_focus_and_says_r() {
        let mut app = test_app();
        app.agents = vec![row(true, &[])];
        for code in [KeyCode::Char('p'), KeyCode::Char('f'), KeyCode::Enter] {
            app.status.clear();
            super::super::test_helpers::press(&mut app, code, KeyModifiers::NONE);
            assert_eq!(
                app.status,
                "fix-cache has exited to its shell; R resumes it"
            );
            assert!(matches!(app.overlay, super::super::Overlay::None));
        }
    }

    #[test]
    fn the_rows_show_exited_and_a_missing_role() {
        let mut app = test_app();
        app.agents = vec![row(true, &[]), row(false, &["role", "memory"])];
        app.agents[1].info.pane_id = "w1:p2".into();
        app.agents[1].info.name = Some("supervisor-repo".into());
        app.selected = 0;
        let mut terminal = test_terminal(140, 30);
        terminal
            .draw(|frame| crate::ui::draw(frame, &mut app))
            .unwrap();
        let screen = buffer_text(terminal.backend().buffer());
        assert!(screen.contains(" EXITED "), "{screen}");
        assert!(screen.contains("⚠ no role"), "{screen}");
        assert!(screen.contains("R resume"), "{screen}");

        app.selected = 1;
        app.status.clear();
        terminal
            .draw(|frame| crate::ui::draw(frame, &mut app))
            .unwrap();
        let screen = buffer_text(terminal.backend().buffer());
        assert!(
            screen.contains("Running without its supervisor role and memory access"),
            "{screen}"
        );
        assert!(!screen.contains("R resume"), "{screen}");
    }
}
