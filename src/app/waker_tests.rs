//! The tests of the supervisor waker (`corgi_core::waker`) that run through
//! the dashboard's refresh.

use std::path::Path;

use crate::{
    app::launch::CORGI_REQUEST_TOKEN,
    model::{AgentInfo, AgentState, DashboardAgent},
};

use crate::app::waker::*;
use crate::test_support::ScratchDir;

/// One refresh of the dashboard that wakes supervisors, with `report` as
/// every resting agent's report and the supervisor's box holding a draft
/// when `drafting`. Returns what was typed into the supervisor's box.
fn wake_refresh(
    waker: &mut SupervisorWaker,
    agents: &[DashboardAgent],
    report: Option<&str>,
    drafting: bool,
) -> Vec<String> {
    let mut sent = Vec::new();
    assert_eq!(
        waker.observe(agents, Path::new("/opt/corgi"), |_| report.map(said)),
        None
    );
    waker.deliver(
        agents,
        |_| drafting,
        |supervisor, text| {
            sent.push(format!("{supervisor}: {text}"));
            true
        },
    );
    sent
}

/// `text` as a report from a transcript.
fn said(text: &str) -> Captured {
    Captured {
        text: text.into(),
        transcript: true,
    }
}

#[test]
fn a_worker_herdr_gave_no_session_is_listed_and_wakes_its_supervisor() {
    use crate::{
        app::{cli::fleet_rows, rows::STATUS_LINE_SESSION_TOKEN},
        herdr::{PaneInfo, SessionSnapshot},
        model::AgentSession,
        supervisor::{SUPERVISOR_TOKEN, marker},
        test_support::test_app,
    };
    use AgentState::{Done, Working};

    let scratch = ScratchDir::new("waker-no-session");
    let agent = |name: &str, state: AgentState, tokens: &[(&str, &str)]| AgentInfo {
        pane_id: format!("{name}:p1"),
        name: Some(name.into()),
        agent: Some("codex".into()),
        cwd: Some("/repos/weather".into()),
        state,
        tokens: tokens
            .iter()
            .map(|(key, value)| (key.to_string(), value.to_string()))
            .collect(),
        ..AgentInfo::default()
    };
    let snapshot = |state: AgentState| SessionSnapshot {
        panes: vec![PaneInfo {
            pane_id: "corgi:p1".into(),
            label: Some("Corgi".into()),
            ..PaneInfo::default()
        }],
        agents: vec![
            AgentInfo {
                agent_session: Some(AgentSession {
                    value: "supervisor-session".into(),
                    ..AgentSession::default()
                }),
                ..agent(
                    "supervisor-weather",
                    AgentState::Idle,
                    &[(SUPERVISOR_TOKEN, &marker("", Some("supervisor-session")))],
                )
            },
            agent(
                "w-request",
                state,
                &[(CORGI_REQUEST_TOKEN, "20261008-w-request")],
            ),
            agent(
                "w-bridge",
                state,
                &[(STATUS_LINE_SESSION_TOKEN, "f44d6b29-bridge")],
            ),
            // Herdr's detector taking a terminal for an agent.
            agent("detected", state, &[]),
            agent("blank", state, &[(STATUS_LINE_SESSION_TOKEN, " ")]),
            // Corgi's own dashboard, whatever marks its pane carries.
            AgentInfo {
                pane_id: "corgi:p1".into(),
                ..agent(
                    "dashboard",
                    state,
                    &[(CORGI_REQUEST_TOKEN, "20261008-dashboard")],
                )
            },
        ],
        ..SessionSnapshot::default()
    };
    let mut app = test_app();
    let mut waker = SupervisorWaker::in_dir(&scratch);

    app.install_snapshot(snapshot(Working));
    let names: Vec<_> = app
        .agents
        .iter()
        .map(|agent| agent.info.display_name())
        .collect();
    assert_eq!(names.len(), 3, "{names:?}");
    for name in ["supervisor-weather", "w-request", "w-bridge"] {
        assert!(names.contains(&name), "{name} missing from {names:?}");
    }
    let fleet = fleet_rows(&app, "/repos/weather");
    for name in ["w-request", "w-bridge"] {
        assert!(
            fleet
                .iter()
                .any(|row| row.starts_with(&format!("{name}\tworker\t"))),
            "{name} missing from {fleet:?}"
        );
    }
    assert!(wake_refresh(&mut waker, &app.agents, None, false).is_empty());

    app.install_snapshot(snapshot(Done));
    assert_eq!(
        wake_refresh(&mut waker, &app.agents, None, false),
        [
            "supervisor-weather: [Corgi] w-bridge is done. Run: /opt/corgi report w-bridge\n\
             [Corgi] w-request is done. Run: /opt/corgi report w-request"
        ]
    );
}
