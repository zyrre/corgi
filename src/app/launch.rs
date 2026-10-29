//! The dashboard's side of a launch: the background job that runs the
//! launch sequence of [`corgi_core::launch`], and the panel that follows it.

use std::time::Duration;

use crossterm::event::{KeyCode, KeyEvent};

pub(crate) use corgi_core::launch::*;

use crate::{
    job::{Job, Update},
    paths::dir_name,
    supervisor,
};

use super::{App, catalog::default_harness, overlay::Overlay, progress::JobReport};

/// How long the panel of a started agent stays up to say so, over its check
/// stamp, before it closes by itself.
const STARTED_HOLD: Duration = Duration::from_millis(900);

/// The new-agent popup stays visible while this state is present. Progress is
/// rendered there instead of in the dashboard footer, and the popup only goes
/// away after the worker confirms that the first prompt was accepted.
#[derive(Debug, Clone)]
pub(crate) struct NewAgentLaunch {
    pub(crate) name: String,
    /// What the launch does, in order, as the panel lists them.
    pub(crate) steps: Vec<String>,
    pub(crate) state: LaunchState,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum LaunchState {
    /// Running the step at index `step`, and the latest progress it
    /// reported, under the spinner on that step.
    Running { status: String, step: usize },
    /// The agent started, as `message` says, at `at` on the dashboard's
    /// motion clock. The panel says so, then closes by itself.
    Started { message: String, at: Duration },
    /// Why the launch failed. The popup stays open until Esc dismisses it.
    Failed(String),
}

impl LaunchState {
    /// The progress, the outcome or the error, whichever the popup shows.
    #[cfg(test)]
    pub(crate) fn text(&self) -> &str {
        match self {
            Self::Running { status, .. } => status,
            Self::Started { message, .. } => message,
            Self::Failed(error) => error,
        }
    }
}

impl App {
    /// The supervisor key: starts the supervisor of the selected agent's project,
    /// its repository's primary checkout, or focuses it when it already runs.
    pub(super) fn begin_supervisor(&mut self) {
        let Some(root) = self
            .selected_agent()
            .map(|agent| agent.project_root.clone())
            .filter(|root| !root.is_empty())
        else {
            self.set_status(
                "Select an agent to start the supervisor of its project",
                Some(Duration::from_secs(5)),
            );
            return;
        };
        if let Some(index) = self
            .agents
            .iter()
            .position(|agent| agent.supervisor && agent.project_root == root)
        {
            let focused = format!(
                "Focused {}'s supervisor, {}",
                dir_name(&root).unwrap_or(supervisor::UNNAMED_PROJECT),
                self.agents[index].info.display_name()
            );
            self.select(index);
            self.focus_selected_saying(focused);
            return;
        }
        let saved = supervisor::state_dir(&root)
            .ok()
            .and_then(|dir| supervisor::saved_launch(&dir));
        match dashboard_supervisor_plan(&self.agents, &root, saved, default_harness) {
            Ok(plan) => self.start_new_agent(plan),
            Err(error) => self.set_status(format!("{error:#}"), Some(Duration::from_secs(8))),
        }
    }

    /// Starts the launch of `plan` in the background, the new-agent panel
    /// following its progress.
    pub(super) fn start_new_agent(&mut self, plan: LaunchPlan) {
        let name = plan.name.clone();
        let client = self.client.clone();
        let dashboard_pane_id = self.dashboard_pane_id.clone();
        let restore_focus = self.restore_focus_after_launch;
        // The form closes here; what it asked for lives on in `plan`.
        self.overlay = Overlay::Launch(NewAgentLaunch {
            state: LaunchState::Running {
                status: format!("Starting {name}…"),
                step: 0,
            },
            steps: launch_steps(&plan),
            name,
        });
        self.launch_job = Job::spawn(move |mut progress| {
            run_launch(
                &client,
                &plan,
                dashboard_pane_id.as_deref(),
                restore_focus,
                &mut progress,
            )
        });
    }

    pub(super) fn poll_launch(&mut self) {
        for update in self.launch_job.poll() {
            let result = match update {
                Update::Progress(JobReport::Status(message)) => {
                    if let Some(NewAgentLaunch {
                        state: LaunchState::Running { status, .. },
                        ..
                    }) = self.overlay.new_agent_launch_mut()
                    {
                        *status = message;
                    }
                    continue;
                }
                Update::Progress(JobReport::Step(index)) => {
                    if let Some(NewAgentLaunch {
                        state: LaunchState::Running { step, .. },
                        ..
                    }) = self.overlay.new_agent_launch_mut()
                    {
                        *step = index;
                    }
                    continue;
                }
                Update::Progress(JobReport::Project(root)) => {
                    self.projects.remember(&root);
                    continue;
                }
                Update::Finished(result) => result,
                // Without an outcome the popup would spin and the dashboard
                // never refresh again.
                Update::Panicked => Err(anyhow::anyhow!(
                    "the launch stopped without reporting its outcome"
                )),
            };
            match result {
                Ok(message) => {
                    self.set_status(message.clone(), Some(Duration::from_secs(8)));
                    // An animated dashboard shows the start in its panel for
                    // a moment, over the success flash; without effects there
                    // is nothing to show, and the panel closes at once.
                    let at = self.motion.now();
                    match self.overlay.new_agent_launch_mut() {
                        Some(launch) if self.motion.animates() => {
                            launch.state = LaunchState::Started { message, at };
                        }
                        Some(_) => self.overlay = Overlay::None,
                        None => {}
                    }
                }
                Err(error) => {
                    let error = format!("{error:#}");
                    self.set_status(
                        format!("New agent failed: {error}"),
                        Some(Duration::from_secs(10)),
                    );
                    if let Some(launch) = self.overlay.new_agent_launch_mut() {
                        launch.state = LaunchState::Failed(error);
                    }
                    // The failed launch keeps its popup open so the user can
                    // read the error and dismiss it with Esc.
                }
            }
            self.request_refresh();
        }
    }

    /// Closes the panel of a started agent once it has shown the start for
    /// long enough.
    pub(super) fn close_started_launch(&mut self) {
        if let Some(NewAgentLaunch {
            state: LaunchState::Started { at, .. },
            ..
        }) = self.overlay.new_agent_launch()
            && self.motion.now().saturating_sub(*at) >= STARTED_HOLD
        {
            self.overlay = Overlay::None;
        }
    }

    /// Keys while a launch runs, which take none; after it failed, when Esc
    /// dismisses the error; or once it started, when any key closes the
    /// panel early.
    pub(super) fn handle_launch_key(&mut self, key: KeyEvent) -> bool {
        match self.overlay.new_agent_launch().map(|launch| &launch.state) {
            Some(LaunchState::Failed(_)) if key.code == KeyCode::Esc => self.cancel_editor(),
            Some(LaunchState::Started { .. }) => self.overlay = Overlay::None,
            _ => {}
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use std::{fs, sync::mpsc};

    use anyhow::anyhow;
    use crossterm::event::{KeyCode, KeyModifiers};
    use serde_json::json;

    use crate::{
        app::test_helpers::{poll_until, press},
        herdr::HerdrClient,
        model::{AgentInfo, DashboardAgent},
        test_support::{answer, fake_herdr, test_app},
    };

    use super::*;

    #[test]
    fn the_supervisor_key_starts_the_selected_projects_supervisor_or_focuses_the_running_one() {
        // A name no developer's own supervisor state directory has.
        let root = "/repos/corgi-key-test";
        let worker = DashboardAgent {
            info: AgentInfo {
                name: Some("corgi-key-test".into()),
                pane_id: "w1:p1".into(),
                ..AgentInfo::default()
            },
            project_root: root.into(),
            ..DashboardAgent::default()
        };

        let mut app = test_app();
        press(&mut app, KeyCode::Char('s'), KeyModifiers::NONE);
        assert!(matches!(app.overlay, Overlay::None));
        assert!(app.status.contains("Select an agent"), "{}", app.status);

        // Only a worker runs: the key starts the launch at once, no form.
        app.agents = vec![worker.clone()];
        press(&mut app, KeyCode::Char('s'), KeyModifiers::NONE);
        assert!(
            app.overlay
                .new_agent_launch()
                .is_some_and(|launch| launch.name == "supervisor-corgi-key-test"),
            "{:?}",
            app.overlay.new_agent_launch()
        );
        poll_until(&mut app, App::poll_launch, |app| {
            !app.launch_job.is_running()
        });

        // With the supervisor running, the key selects and focuses it, walking
        // to its workspace and tab first as Enter does, and starts nothing.
        let (socket_path, server) = fake_herdr("corgi-key-focus", |listener| {
            let mut requests = Vec::new();
            for (method, reply) in [
                ("workspace.focus", "workspace_info"),
                ("tab.focus", "tab_info"),
                ("agent.focus", "agent_info"),
            ] {
                answer(&listener, |request| {
                    assert_eq!(request["method"], method);
                    requests.push(request["params"].clone());
                    json!({ "result": {
                        "type": reply,
                        "agent": {
                            "pane_id": "w2:p1", "workspace_id": "w2", "tab_id": "w2:t3"
                        }
                    }})
                });
            }
            requests
        });
        let mut app = test_app();
        app.client = HerdrClient::from_socket_path(&socket_path);
        app.agents = vec![
            DashboardAgent {
                info: AgentInfo {
                    name: Some("supervisor-corgi-key-test".into()),
                    pane_id: "w2:p1".into(),
                    workspace_id: "w2".into(),
                    tab_id: "w2:t3".into(),
                    ..AgentInfo::default()
                },
                project_root: root.into(),
                supervisor: true,
                ..DashboardAgent::default()
            },
            worker,
        ];
        app.selected = 1;
        press(&mut app, KeyCode::Char('s'), KeyModifiers::NONE);
        let requests = server.join().expect("fake server panicked");
        fs::remove_file(socket_path).expect("remove fake socket");
        assert_eq!(
            requests,
            [
                json!({ "workspace_id": "w2" }),
                json!({ "tab_id": "w2:t3" }),
                json!({ "target": "w2:p1" }),
            ]
        );
        assert!(matches!(app.overlay, Overlay::None));
        assert!(!app.launch_job.is_running());
        assert_eq!(app.selected, 0);
        assert_eq!(
            app.status,
            "Focused corgi-key-test's supervisor, supervisor-corgi-key-test"
        );
    }

    #[test]
    fn new_agent_popup_stays_open_until_launch_finishes() {
        let mut app = test_app();
        app.overlay = Overlay::Launch(NewAgentLaunch {
            name: "corgi-worker".into(),
            state: LaunchState::Running {
                status: "Creating workspace for corgi-worker…".into(),
                step: 0,
            },
            steps: Vec::new(),
        });
        let (step, steps) = mpsc::channel::<String>();
        app.launch_job = Job::spawn(move |progress| {
            for message in steps {
                progress.send(JobReport::Status(message));
            }
            Ok("corgi-worker started".into())
        });

        step.send("Sending first prompt…".into())
            .expect("send progress");
        poll_until(&mut app, App::poll_launch, |app| {
            app.overlay
                .new_agent_launch()
                .is_some_and(|launch| launch.state.text() == "Sending first prompt…")
        });
        assert!(matches!(app.overlay, Overlay::Launch(_)));

        drop(step);
        poll_until(&mut app, App::poll_launch, |app| {
            !app.launch_job.is_running()
        });
        assert!(matches!(app.overlay, Overlay::None));
        assert!(app.overlay.new_agent_launch().is_none());
    }

    #[test]
    fn an_animated_dashboard_shows_a_started_agent_for_a_moment_then_closes() {
        let mut app = test_app();
        app.motion = crate::motion::Motion::manual();
        app.motion.set_time(Duration::from_secs(10));
        app.overlay = Overlay::Launch(NewAgentLaunch {
            name: "corgi-worker".into(),
            steps: vec!["Create a worktree".into(), "Start claude".into()],
            state: LaunchState::Running {
                status: "Starting corgi-worker…".into(),
                step: 0,
            },
        });
        app.launch_job = Job::spawn(|progress| {
            progress.send(JobReport::Step(1));
            Ok("corgi-worker started in worktree calm-river".into())
        });
        poll_until(&mut app, App::poll_launch, |app| {
            !app.launch_job.is_running()
        });
        assert!(matches!(
            app.overlay.new_agent_launch().map(|launch| &launch.state),
            Some(LaunchState::Started { message, .. }) if message.contains("calm-river")
        ));
        assert_eq!(app.overlay.outcome(), Some(crate::motion::Outcome::Success));

        // It stays up over the whole stamp, then closes by itself.
        assert!(crate::motion::STAMP_LENGTH < STARTED_HOLD);
        app.motion
            .set_time(Duration::from_secs(10) + STARTED_HOLD / 2);
        app.close_started_launch();
        assert!(matches!(app.overlay, Overlay::Launch(_)));
        app.motion.set_time(Duration::from_secs(10) + STARTED_HOLD);
        app.close_started_launch();
        assert!(matches!(app.overlay, Overlay::None));

        // A key closes it sooner.
        app.overlay = Overlay::Launch(NewAgentLaunch {
            name: "corgi-worker".into(),
            steps: Vec::new(),
            state: LaunchState::Started {
                message: "started".into(),
                at: Duration::from_secs(10),
            },
        });
        press(&mut app, KeyCode::Char('j'), KeyModifiers::NONE);
        assert!(matches!(app.overlay, Overlay::None));
    }

    #[test]
    fn a_failed_new_agent_keeps_its_popup_open_for_dismissal() {
        let mut app = test_app();
        app.overlay = Overlay::Launch(NewAgentLaunch {
            name: "corgi-worker".into(),
            state: LaunchState::Running {
                status: "Starting corgi-worker…".into(),
                step: 0,
            },
            steps: Vec::new(),
        });
        app.launch_job = Job::spawn(|_| Err(anyhow!("approval required")));

        poll_until(&mut app, App::poll_launch, |app| {
            !app.launch_job.is_running()
        });
        assert!(matches!(app.overlay, Overlay::Launch(_)));
        assert!(
            app.overlay
                .new_agent_launch()
                .is_some_and(|launch| matches!(launch.state, LaunchState::Failed(_)))
        );

        press(&mut app, KeyCode::Esc, KeyModifiers::NONE);
        assert!(matches!(app.overlay, Overlay::None));
        assert!(app.overlay.new_agent_launch().is_none());
    }
}
