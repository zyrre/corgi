//! The prompt typed for an existing agent: opening it on the selected
//! agent, the keys it takes, and sending it.

use std::time::Duration;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::{App, overlay::Overlay};

/// A prompt being typed for one agent.
#[derive(Debug, Clone)]
pub(crate) struct PromptForm {
    /// The pane of the agent the prompt goes to.
    pub(crate) target: String,
    /// The agent's name, as the dialog and the status line show it.
    pub(crate) label: String,
    pub(crate) text: String,
}

impl App {
    pub(super) fn begin_prompt(&mut self) {
        let Some(agent) = self.selected_agent() else {
            self.status = "No agent selected".into();
            return;
        };
        if !agent.info.state.can_prompt() {
            self.status = "Blocked agents must be handled in their own pane".into();
            return;
        }
        self.overlay = Overlay::Prompt(PromptForm {
            target: agent.info.pane_id.clone(),
            label: agent.info.display_name().to_string(),
            text: String::new(),
        });
    }

    fn submit_prompt(&mut self) {
        let Some(form) = self.overlay.take_prompt_form() else {
            return;
        };
        if form.text.trim().is_empty() {
            self.status = "Prompt cannot be empty".into();
            return;
        }
        let status = match self.client.prompt_agent(&form.target, form.text.trim()) {
            Ok(_) => {
                self.motion.succeeded();
                format!("Prompt sent to {}", form.label)
            }
            Err(error) => format!("Prompt failed: {error:#}"),
        };
        self.set_status(status, Some(Duration::from_secs(5)));
        self.request_refresh();
    }

    pub(super) fn handle_prompt_key(&mut self, key: KeyEvent) -> bool {
        match key.code {
            KeyCode::Esc => self.cancel_editor(),
            KeyCode::Enter => self.submit_prompt(),
            KeyCode::Backspace => {
                if let Some(form) = self.overlay.prompt_form_mut() {
                    form.text.pop();
                }
            }
            KeyCode::Char(character) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                if let Some(form) = self.overlay.prompt_form_mut() {
                    form.text.push(character);
                }
            }
            _ => {}
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use crossterm::event::{KeyCode, KeyModifiers};
    use serde_json::json;

    use super::*;
    use crate::{
        app::{AppInputs, test_helpers::press},
        herdr::HerdrClient,
        motion::Motion,
        test_support::{answer, fake_herdr},
    };

    fn prompt(text: &str) -> Overlay {
        Overlay::Prompt(PromptForm {
            target: "w1:p1".into(),
            label: "worker".into(),
            text: text.into(),
        })
    }

    #[test]
    fn only_a_prompt_that_was_sent_stamps_its_check() {
        let (socket, herdr) = fake_herdr("prompt-stamp", |listener| {
            answer(&listener, |_| {
                json!({"result": {"type": "agent_prompted", "agent": {
                    "agent": "claude", "agent_status": "working", "pane_id": "w1:p1",
                    "workspace_id": "w1", "tab_id": "w1:t1"
                }}})
            });
            answer(
                &listener,
                |_| json!({"error": {"code": "agent_blocked", "message": "approval required"}}),
            );
        });
        let mut app = App::with_inputs(
            HerdrClient::from_socket_path(&socket),
            false,
            AppInputs::default(),
        );
        app.motion = Motion::manual();
        app.motion.record_frame(
            ratatui::layout::Rect::new(10, 5, 40, 6),
            ratatui::style::Color::Cyan,
        );

        // Cancelled, nothing plays.
        app.overlay = prompt("never sent");
        press(&mut app, KeyCode::Esc, KeyModifiers::NONE);
        assert_eq!(app.motion.stamp_time(), None);

        // Sent, the check plays where the prompt was.
        app.overlay = prompt("Run the tests");
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE);
        assert!(app.status.starts_with("Prompt sent"), "{}", app.status);
        assert!(app.motion.stamp().is_some());

        // Refused, nothing plays.
        app.motion.set_time(std::time::Duration::from_secs(5));
        app.overlay = prompt("Run the tests");
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE);
        assert!(app.status.starts_with("Prompt failed"), "{}", app.status);
        assert_eq!(app.motion.stamp_time(), None);
        herdr.join().expect("fake Herdr");
    }
}
