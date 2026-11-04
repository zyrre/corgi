//! Find orphaned sessions in the dashboard and on the command line: the
//! read-only `corgi orphans`, and the `o` key's dialog, which lists
//! [`corgi_core::orphans::detect`]'s candidates and applies the choice made
//! for each with [`corgi_core::orphans::apply`]. Both run as a background
//! job, so keys and frames never wait on Herdr's process lookups.

use std::time::Duration;

use anyhow::Result;
use corgi_core::{
    herdr::HerdrClient,
    job::{Job, Update},
    orphans::{Candidate, Choices, Options, apply, detect, report},
};
use crossterm::event::{KeyCode, KeyEvent};

use super::{App, overlay::Overlay};

pub const ORPHANS_USAGE: &str = "\
Usage: corgi orphans [--json]

Lists the agent sessions Corgi gets wrong on this Herdr session, and leftover
workspaces, each with its evidence and the choices the dashboard's o key
offers for it: agents Herdr lists but Corgi does not show, agent CLIs Herdr
does not detect, supervisors that lost their mark, workers whose supervisor
lost track of them, agents in no repository, and empty duplicate project
workspaces. Candidates left alone in the dialog are not listed until their
evidence changes. It only reads: nothing in Herdr, Git or Corgi's state
changes. With --json, prints the candidates as a JSON array.";

/// `corgi orphans`: the candidates on the Herdr session of
/// `HERDR_SOCKET_PATH`, as text or JSON. Reads only.
pub fn orphans_command(args: &[String]) -> Result<()> {
    let json = match args {
        [] => false,
        [flag] if flag == "--json" => true,
        _ => anyhow::bail!("{ORPHANS_USAGE}"),
    };
    let client = HerdrClient::from_env()?;
    let candidates = detect(&client, &Options::new(None))?;
    if json {
        println!("{}", serde_json::to_string_pretty(&candidates)?);
    } else {
        print!("{}", report(&candidates));
    }
    Ok(())
}

/// What the dialog's background job brings back.
#[derive(Debug)]
pub(crate) enum Found {
    /// A fresh list.
    Candidates(Vec<Candidate>),
    /// What one choice did, by the candidate's key.
    Applied(String, String),
}

/// Where the dialog is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Stage {
    /// Looking, for the first list.
    Looking,
    /// The list, taking choices.
    List,
    /// A choice being applied to the candidate of this key.
    Applying(String),
}

/// The "Orphaned sessions" dialog: where it is, and the candidates with
/// the choice made for each.
#[derive(Debug, Clone)]
pub(crate) struct OrphansDialog {
    pub(crate) stage: Stage,
    pub(crate) choices: Choices,
}

impl OrphansDialog {
    pub(crate) fn looking() -> Self {
        Self {
            stage: Stage::Looking,
            choices: Choices::default(),
        }
    }

    /// The dialog showing `candidates`, each on its suggestion.
    #[cfg(test)]
    pub(crate) fn with(candidates: Vec<Candidate>) -> Self {
        Self {
            stage: Stage::List,
            choices: Choices::new(candidates),
        }
    }
}

impl App {
    /// The `o` key: opens the dialog and looks for orphans in the
    /// background. Only now does Corgi look; nothing counts them meanwhile.
    pub(super) fn begin_orphans(&mut self) {
        if self.orphans_job.is_running() {
            return;
        }
        self.overlay = Overlay::Orphans(OrphansDialog::looking());
        let (client, options) = (self.client.clone(), self.orphan_options());
        self.orphans_job = Job::spawn(move |_| detect(&client, &options).map(Found::Candidates));
    }

    fn orphan_options(&self) -> Options {
        Options::in_state(
            self.dashboard_pane_id.clone(),
            self.inboxes.clone().unwrap_or_default(),
        )
    }

    /// Puts the job's list or outcome in the dialog, if it is still open.
    pub(super) fn poll_orphans(&mut self) {
        for update in self.orphans_job.poll() {
            let result = match update {
                Update::Progress(()) => continue,
                Update::Finished(result) => result,
                Update::Panicked => Err(anyhow::anyhow!("Looking for orphans stopped")),
            };
            let Overlay::Orphans(dialog) = &mut self.overlay else {
                continue;
            };
            match result {
                Ok(Found::Candidates(candidates)) => {
                    dialog.choices.show(candidates);
                    dialog.stage = Stage::List;
                }
                Ok(Found::Applied(key, status)) => {
                    dialog.choices.applied(&key, status.clone());
                    dialog.stage = Stage::List;
                    self.motion.succeeded();
                    self.set_status(status, Some(Duration::from_secs(6)));
                    self.request_refresh();
                }
                Err(error) => {
                    dialog.choices.error = Some(format!("{error:#}"));
                    dialog.stage = Stage::List;
                }
            }
        }
    }

    pub(super) fn handle_orphans_key(&mut self, key: KeyEvent) -> bool {
        let Overlay::Orphans(dialog) = &mut self.overlay else {
            return false;
        };
        if let Stage::Applying(_) = dialog.stage {
            // The choice is going through; it reports back either way.
            return false;
        }
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => self.overlay = Overlay::None,
            _ if dialog.stage == Stage::Looking => {}
            KeyCode::Char('j') | KeyCode::Down => dialog.choices.select_by(1),
            KeyCode::Char('k') | KeyCode::Up => dialog.choices.select_by(-1),
            KeyCode::Right | KeyCode::Tab => dialog.choices.cycle(1),
            KeyCode::Left | KeyCode::BackTab => dialog.choices.cycle(-1),
            KeyCode::Char(letter @ ('s' | 'w' | 'x' | 'l')) => {
                dialog.choices.choose_key(letter);
            }
            KeyCode::Enter => {
                let Some((candidate, action)) = dialog.choices.choice() else {
                    return false;
                };
                let (key, fingerprint, action) = (
                    candidate.key.clone(),
                    candidate.fingerprint.clone(),
                    action.clone(),
                );
                dialog.stage = Stage::Applying(key.clone());
                let (client, options) = (self.client.clone(), self.orphan_options());
                self.orphans_job = Job::spawn(move |_| {
                    apply(&client, &options, &key, &fingerprint, &action)
                        .map(|status| Found::Applied(key, status))
                });
            }
            _ => {}
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use corgi_core::orphans::{Action, Cause, Confidence};
    use crossterm::event::KeyModifiers;

    use super::*;
    use crate::test_support::test_app;

    pub(crate) fn candidate(key: &str, actions: Vec<Action>, suggested: usize) -> Candidate {
        Candidate {
            key: key.into(),
            cause: Cause::Worker,
            confidence: Confidence::Certain,
            pane_id: Some("wQ:p1".into()),
            workspace_id: "wQ".into(),
            workspace: "pay".into(),
            harness: "claude".into(),
            name: "restored-w1".into(),
            cwd: "/repos/webshop-pay".into(),
            summary: "Worker its supervisor lost track of".into(),
            evidence: vec!["Ledger entry r-pay (w-pay, dispatched) names it".into()],
            fix: None,
            actions,
            suggested,
            fingerprint: format!("worker {key}"),
        }
    }

    fn worker() -> Action {
        Action::Worker {
            project: "webshop".into(),
            project_root: "/repos/webshop".into(),
            request_id: "r-pay".into(),
            agent: "w-pay".into(),
        }
    }

    fn press(app: &mut App, code: KeyCode) {
        app.handle_key(KeyEvent::new(code, KeyModifiers::NONE));
    }

    #[test]
    fn each_row_starts_on_its_suggestion_and_keys_change_only_the_selected_one() {
        let mut app = test_app();
        let close = Action::Close {
            target: "pane".into(),
        };
        app.overlay = Overlay::Orphans(OrphansDialog::with(vec![
            candidate("pane:a", vec![worker(), Action::Leave, close.clone()], 0),
            candidate("pane:b", vec![Action::Leave, close.clone()], 0),
        ]));
        let chosen = |app: &App| match &app.overlay {
            Overlay::Orphans(dialog) => (
                dialog.choices.selected,
                dialog.choices.choice().map(|(_, a)| a.clone()),
            ),
            _ => panic!("the dialog closed"),
        };
        assert_eq!(chosen(&app), (0, Some(worker())));
        press(&mut app, KeyCode::Char('x'));
        assert_eq!(chosen(&app), (0, Some(close.clone())));
        press(&mut app, KeyCode::Right);
        assert_eq!(chosen(&app), (0, Some(worker())));
        press(&mut app, KeyCode::Char('j'));
        assert_eq!(chosen(&app), (1, Some(Action::Leave)));
        // A key the row does not offer changes nothing.
        press(&mut app, KeyCode::Char('w'));
        assert_eq!(chosen(&app), (1, Some(Action::Leave)));
        press(&mut app, KeyCode::Left);
        assert_eq!(chosen(&app), (1, Some(close)));
        press(&mut app, KeyCode::Esc);
        assert!(matches!(app.overlay, Overlay::None));
    }

    #[test]
    fn an_applied_choice_takes_its_row_away_and_says_what_it_did() {
        let mut app = test_app();
        let mut dialog = OrphansDialog::with(vec![
            candidate("pane:a", vec![worker(), Action::Leave], 0),
            candidate("pane:b", vec![Action::Leave], 0),
        ]);
        dialog.stage = Stage::Applying("pane:a".into());
        app.overlay = Overlay::Orphans(dialog);
        // Keys wait while it applies.
        press(&mut app, KeyCode::Esc);
        assert!(matches!(app.overlay, Overlay::Orphans(_)));
        app.orphans_job = Job::spawn(|_| {
            Ok(Found::Applied(
                "pane:a".into(),
                "restored-w1 is webshop's worker r-pay again".into(),
            ))
        });
        while app.orphans_job.is_running() {
            app.poll_orphans();
        }
        let Overlay::Orphans(dialog) = &app.overlay else {
            panic!("the dialog closed");
        };
        assert_eq!(dialog.stage, Stage::List);
        assert_eq!(dialog.choices.candidates.len(), 1);
        assert_eq!(dialog.choices.candidates[0].key, "pane:b");
        assert_eq!(
            dialog.choices.done,
            ["restored-w1 is webshop's worker r-pay again"]
        );
        assert_eq!(app.status, "restored-w1 is webshop's worker r-pay again");
    }
}
