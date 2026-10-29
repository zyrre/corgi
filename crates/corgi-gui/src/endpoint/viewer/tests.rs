use super::*;
use crate::endpoint::protocol::{AgentStatus, ClientShellTab, ClientShellWorkspace};

/// A snapshot of `tabs` (each `w<n>:t1` in workspace `w<n>`), labelled as
/// `labels` says, with the connection on `at`.
fn snapshot(at: &str, tabs: &[(&str, &str)]) -> ClientShellSnapshot {
    let workspace = |tab: &str| tab.split(':').next().unwrap_or_default().to_string();
    ClientShellSnapshot {
        boot_id: "boot".into(),
        revision: 1,
        config_diagnostic: None,
        product_announcement: None,
        update_available: None,
        update_install_command: String::new(),
        server_keybindings_toml: None,
        latest_release_notes_available: false,
        integration_updates_available: false,
        worktree_directory: String::new(),
        release_notes: None,
        focused_workspace_id: Some(workspace(at)),
        focused_tab_id: Some(at.into()),
        focused_pane_id: None,
        tab_bar_right: Vec::new(),
        tab_bar_right_separator: String::new(),
        agent_view_label: None,
        agent_order: Vec::new(),
        workspaces: tabs
            .iter()
            .map(|(tab, label)| ClientShellWorkspace {
                workspace_id: workspace(tab),
                active_tab_id: (*tab).into(),
                new_workspace_cwd: String::new(),
                number: 1,
                label: (*label).into(),
                custom_label: true,
                branch: None,
                git_ahead_behind: None,
                tokens: Vec::new(),
                worktree: None,
                focused: false,
                agent_status: AgentStatus::Idle,
            })
            .collect(),
        tabs: tabs
            .iter()
            .map(|(tab, _)| ClientShellTab {
                tab_id: (*tab).into(),
                workspace_id: workspace(tab),
                number: 1,
                label: String::new(),
                custom_label: false,
                zoomed: false,
                focused: false,
                agent_status: AgentStatus::Idle,
            })
            .collect(),
        panes: Vec::new(),
        agents: Vec::new(),
        commands: Vec::new(),
    }
}

/// The user's tab, an agent's, another agent's and the park tab.
const TABS: &[(&str, &str)] = &[
    ("w1:t1", "my work"),
    ("w2:t1", "shop"),
    ("w3:t1", "billing"),
    ("w4:t1", PARK_LABEL),
];

fn ok() -> Result<Value, ApiError> {
    Ok(Value::Null)
}

fn refused(code: &str) -> Result<Value, ApiError> {
    Err(ApiError {
        code: code.into(),
        message: format!("{code} (test)"),
    })
}

/// Sends what the viewer wants, answering every step with `ok`, and
/// returns the steps in order.
fn run(viewer: &mut Viewer) -> Vec<Step> {
    let mut sent = Vec::new();
    loop {
        let steps = viewer.next();
        if steps.is_empty() {
            return sent;
        }
        let ids: Vec<String> = (0..steps.len())
            .map(|n| format!("r{}-{n}", sent.len()))
            .collect();
        for (id, step) in ids.iter().zip(&steps) {
            viewer.sent(id.clone(), step.clone());
        }
        for id in ids {
            assert!(viewer.answered(&id, &ok()));
        }
        sent.extend(steps);
    }
}

#[test]
fn a_new_connection_rests_on_the_park_tab_before_it_shows_anything() {
    // It said hello active on the user's tab, which it may have claimed if
    // no other client owned it: resting on the park tab gives it back.
    let mut viewer = Viewer::new(true);
    viewer.show(Some("w2:t1".into()));
    assert_eq!(
        viewer.next(),
        Vec::<Step>::new(),
        "nothing before a snapshot"
    );
    viewer.snapshot(&snapshot("w1:t1", TABS));
    assert_eq!(
        run(&mut viewer),
        vec![
            Step::Focus("w4:t1".into()),
            Step::Activate(false),
            Step::Activate(true),
            Step::Focus("w2:t1".into()),
        ]
    );
    assert_eq!(viewer.status(), Status::Showing("w2:t1".into()));
}

#[test]
fn a_new_connection_on_the_park_tab_or_its_own_tab_goes_straight_on() {
    let mut viewer = Viewer::new(true);
    viewer.show(Some("w2:t1".into()));
    viewer.snapshot(&snapshot("w4:t1", TABS));
    assert_eq!(run(&mut viewer), vec![Step::Focus("w2:t1".into())]);
    let mut viewer = Viewer::new(true);
    viewer.show(Some("w2:t1".into()));
    viewer.snapshot(&snapshot("w2:t1", TABS));
    assert_eq!(run(&mut viewer), Vec::<Step>::new());
    assert_eq!(viewer.status(), Status::Showing("w2:t1".into()));
}

#[test]
fn it_parks_before_it_deactivates_and_activates_from_the_park_tab() {
    let mut viewer = Viewer::new(true);
    viewer.show(Some("w2:t1".into()));
    viewer.snapshot(&snapshot("w1:t1", TABS));
    run(&mut viewer);
    // Leaving: to the park tab first, then inactive.
    viewer.show(None);
    assert_eq!(
        run(&mut viewer),
        vec![Step::Focus("w4:t1".into()), Step::Activate(false)]
    );
    assert!(viewer.at_rest());
    assert_eq!(viewer.status(), Status::Parked);
    // Coming back: activation and the move together, from the park tab.
    viewer.show(Some("w3:t1".into()));
    assert_eq!(
        run(&mut viewer),
        vec![Step::Activate(true), Step::Focus("w3:t1".into())]
    );
    assert_eq!(viewer.status(), Status::Showing("w3:t1".into()));
}

#[test]
fn an_inactive_connection_elsewhere_rests_on_the_park_tab_before_activating() {
    let mut viewer = Viewer::new(false);
    viewer.show(Some("w2:t1".into()));
    viewer.snapshot(&snapshot("w1:t1", TABS));
    assert_eq!(
        run(&mut viewer),
        vec![
            Step::Activate(true),
            Step::Focus("w4:t1".into()),
            Step::Activate(false),
            Step::Activate(true),
            Step::Focus("w2:t1".into()),
        ]
    );
}

#[test]
fn a_park_tab_is_found_by_its_label_or_asked_for() {
    let mut viewer = Viewer::new(false);
    viewer.show(Some("w2:t1".into()));
    let without_park = &TABS[..3];
    viewer.snapshot(&snapshot("w1:t1", without_park));
    assert!(viewer.needs_park());
    assert_eq!(
        viewer.next(),
        Vec::<Step>::new(),
        "an inactive one waits for it"
    );
    viewer.set_park("w9:t1".into());
    assert!(!viewer.needs_park());
    // One found in a later snapshot does not replace the one it knows, but
    // a park tab that was closed is forgotten.
    viewer.snapshot(&snapshot("w1:t1", without_park));
    assert_eq!(viewer.park_tab(), None);
    viewer.snapshot(&snapshot("w1:t1", TABS));
    assert_eq!(viewer.park_tab(), Some("w4:t1"));
}

#[test]
fn nothing_more_is_sent_while_a_step_is_unanswered() {
    let mut viewer = Viewer::new(true);
    viewer.show(Some("w2:t1".into()));
    viewer.snapshot(&snapshot("w4:t1", TABS));
    let steps = viewer.next();
    viewer.sent("a".into(), steps[0].clone());
    assert_eq!(viewer.status(), Status::Opening);
    viewer.show(Some("w3:t1".into()));
    assert_eq!(viewer.next(), Vec::<Step>::new());
    assert!(!viewer.answered("someone else's", &ok()));
    assert!(viewer.answered("a", &ok()));
    assert_eq!(viewer.next(), vec![Step::Focus("w3:t1".into())]);
}

#[test]
fn a_move_it_did_not_ask_for_loses_the_terminal_and_is_not_followed() {
    let mut viewer = Viewer::new(true);
    viewer.show(Some("w2:t1".into()));
    viewer.snapshot(&snapshot("w1:t1", TABS));
    run(&mut viewer);
    // Someone's JSON focus call moves every client, this one too.
    viewer.snapshot(&snapshot("w1:t1", TABS));
    let Status::Gone { tab_id, why } = viewer.status() else {
        panic!("expected the terminal gone, not {:?}", viewer.status());
    };
    assert_eq!(tab_id, "w2:t1");
    assert!(why.contains("moved"), "{why}");
    // It parks from where it was put, and never goes back by itself.
    assert_eq!(
        run(&mut viewer),
        vec![Step::Focus("w4:t1".into()), Step::Activate(false)]
    );
    assert!(matches!(viewer.status(), Status::Gone { .. }));
    // Asked again, it opens the tab again.
    viewer.show(Some("w2:t1".into()));
    assert_eq!(
        run(&mut viewer),
        vec![Step::Activate(true), Step::Focus("w2:t1".into())]
    );
    assert_eq!(viewer.status(), Status::Showing("w2:t1".into()));
}

#[test]
fn a_closed_tab_says_so() {
    let mut viewer = Viewer::new(true);
    viewer.show(Some("w2:t1".into()));
    viewer.snapshot(&snapshot("w1:t1", TABS));
    run(&mut viewer);
    // Herdr closed the tab and put the connection elsewhere.
    let without_shop: Vec<_> = TABS
        .iter()
        .copied()
        .filter(|(t, _)| *t != "w2:t1")
        .collect();
    viewer.snapshot(&snapshot("w4:t1", &without_shop));
    let Status::Gone { why, .. } = viewer.status() else {
        panic!("expected the terminal gone");
    };
    assert_eq!(why, "its tab was closed");
    // Already on the park tab: it only deactivates.
    assert_eq!(run(&mut viewer), vec![Step::Activate(false)]);
}

#[test]
fn a_move_in_flight_is_not_a_loss() {
    let mut viewer = Viewer::new(true);
    viewer.show(Some("w2:t1".into()));
    viewer.snapshot(&snapshot("w4:t1", TABS));
    let steps = viewer.next();
    viewer.sent("a".into(), steps[0].clone());
    // The snapshot of the move comes before its answer.
    viewer.snapshot(&snapshot("w2:t1", TABS));
    assert_eq!(viewer.status(), Status::Opening);
    viewer.answered("a", &ok());
    assert_eq!(viewer.status(), Status::Showing("w2:t1".into()));
}

#[test]
fn herdr_refusing_the_tab_loses_it_and_a_missing_park_tab_is_made_again() {
    let mut viewer = Viewer::new(true);
    viewer.show(Some("w2:t1".into()));
    viewer.snapshot(&snapshot("w4:t1", TABS));
    let steps = viewer.next();
    viewer.sent("a".into(), steps[0].clone());
    viewer.answered("a", &refused("tab_not_found"));
    let Status::Gone { why, .. } = viewer.status() else {
        panic!("expected the terminal gone");
    };
    assert!(why.contains("tab_not_found"), "{why}");

    // Parking to a park tab that went away deactivates where it is and asks
    // for a new park tab.
    let mut viewer = Viewer::new(true);
    viewer.show(Some("w2:t1".into()));
    viewer.snapshot(&snapshot("w1:t1", TABS));
    run(&mut viewer);
    viewer.show(None);
    let steps = viewer.next();
    assert_eq!(
        steps,
        vec![Step::Focus("w4:t1".into()), Step::Activate(false)]
    );
    viewer.sent("f".into(), steps[0].clone());
    viewer.sent("d".into(), steps[1].clone());
    viewer.answered("f", &refused("tab_not_found"));
    viewer.answered("d", &ok());
    assert!(viewer.at_rest());
    assert_eq!(viewer.park_tab(), None);
}

#[test]
fn a_new_connection_keeps_what_was_wanted_and_forgets_its_requests() {
    let mut viewer = Viewer::new(true);
    viewer.show(Some("w2:t1".into()));
    viewer.snapshot(&snapshot("w1:t1", TABS));
    let steps = viewer.next();
    viewer.sent("a".into(), steps[0].clone());
    viewer.reset_active(true);
    assert_eq!(viewer.status(), Status::Connecting);
    assert!(
        !viewer.answered("a", &ok()),
        "the old request is gone with its connection"
    );
    viewer.snapshot(&snapshot("w4:t1", TABS));
    assert_eq!(viewer.park_tab(), Some("w4:t1"));
    assert_eq!(run(&mut viewer), vec![Step::Focus("w2:t1".into())]);
}

#[test]
fn steps_are_the_endpoint_methods_herdr_takes() {
    assert_eq!(
        Step::Activate(true).request(),
        (
            "client_shell.surface.set",
            serde_json::json!({"active": true})
        )
    );
    assert_eq!(
        Step::Focus("w2:t1".into()).request(),
        ("tab.focus", serde_json::json!({"tab_id": "w2:t1"}))
    );
}
