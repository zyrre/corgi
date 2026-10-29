use std::{
    cell::RefCell,
    rc::Rc,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use serde_json::{Value, json};

use super::*;
use crate::{
    inbox::{Item, Kind},
    supervisor::{SUPERVISOR_TOKEN, marker},
    test_support::{FakeHerdr, Prompts, ScratchDir},
};

#[test]
fn herdrs_variables_name_the_event_and_its_pane() {
    let startup = Event::parse(Some("startup"), None);
    assert_eq!(startup.kind, EventKind::Startup);
    assert_eq!(startup.describe(), "startup");

    let changed = Event::parse(
        Some("pane.agent_status_changed"),
        Some(
            r#"{"event":"pane_agent_status_changed","data":{"type":"pane_agent_status_changed",
            "pane_id":"w1:p2","workspace_id":"w1","agent_status":"done","agent":"claude"}}"#,
        ),
    );
    assert_eq!(
        changed,
        Event {
            kind: EventKind::AgentStatusChanged,
            pane_id: Some("w1:p2".into()),
            workspace_id: Some("w1".into()),
            agent: Some("claude".into()),
            status: Some(AgentState::Done),
            released: false,
        }
    );
    assert_eq!(
        changed.describe(),
        "pane.agent_status_changed w1:p2 claude done"
    );

    // An agent leaving its pane, and the name taken from the JSON alone.
    let released = Event::parse(
        None,
        Some(
            r#"{"event":"pane_agent_detected","data":{"pane_id":"w1:p3","workspace_id":"w1",
            "agent":null,"released":true,"final_status":"idle"}}"#,
        ),
    );
    assert_eq!(released.kind, EventKind::AgentDetected);
    assert_eq!(released.agent, None);
    assert_eq!(released.status, Some(AgentState::Idle));
    assert!(released.released);

    for (name, kind) in [
        ("pane.exited", EventKind::PaneExited),
        ("pane.closed", EventKind::PaneClosed),
        ("workspace.closed", EventKind::WorkspaceClosed),
        ("pane.agent_detected", EventKind::AgentDetected),
    ] {
        assert_eq!(Event::parse(Some(name), None).kind, kind);
        assert_eq!(kind.name(), name);
    }
}

#[test]
fn an_event_that_cannot_be_read_still_triggers_a_pass() {
    // Malformed JSON keeps the name; the pass reads Herdr itself.
    let broken = Event::parse(Some("pane.exited"), Some("{not json"));
    assert_eq!(broken.kind, EventKind::PaneExited);
    assert_eq!(broken.pane_id, None);
    // An unknown status is no status.
    let odd = Event::parse(
        Some("pane.agent_status_changed"),
        Some(r#"{"data":{"pane_id":"w1:p1","agent_status":"sleeping"}}"#),
    );
    assert_eq!(odd.status, None);
    assert_eq!(odd.pane_id.as_deref(), Some("w1:p1"));
    // Run by hand, and an event a later Herdr may add.
    assert_eq!(Event::parse(None, None).kind, EventKind::Manual);
    assert_eq!(Event::parse(Some(" "), Some("")).kind, EventKind::Manual);
    assert_eq!(
        Event::parse(Some("tab.created"), None).kind,
        EventKind::Other("tab.created".into())
    );
}

/// A reconciler run from a script: who holds the lock, and what each pass
/// does to the flag, shared with the test.
#[derive(Default)]
struct Scripted {
    dirty: Rc<RefCell<bool>>,
    taken: Rc<RefCell<bool>>,
    holding: bool,
    /// What each pass does, in order; a pass past the end does nothing.
    passes: Vec<fn(&Scripted)>,
    ran: usize,
    /// Called at release, as a hook that marks dirty just then.
    on_release: Option<fn(&Scripted)>,
    /// Until when a replacement a pass started runs.
    replacing_until: RefCell<Option<Instant>>,
    cannot_mark: bool,
    log: Rc<RefCell<Vec<String>>>,
}

impl Reconciler for Scripted {
    fn mark_dirty(&mut self) -> bool {
        if self.cannot_mark {
            return false;
        }
        *self.dirty.borrow_mut() = true;
        true
    }

    fn is_dirty(&self) -> bool {
        *self.dirty.borrow()
    }

    fn hold(&mut self) -> bool {
        if *self.taken.borrow() {
            return false;
        }
        *self.taken.borrow_mut() = true;
        self.holding = true;
        true
    }

    fn release(&mut self) {
        *self.taken.borrow_mut() = false;
        self.holding = false;
        self.log.borrow_mut().push("release".into());
        if let Some(on_release) = self.on_release.take() {
            on_release(self);
        }
    }

    fn pass(&mut self) -> bool {
        assert!(self.holding, "a pass without the lock");
        // As the waker's begin does.
        *self.dirty.borrow_mut() = false;
        let index = self.ran;
        self.ran += 1;
        self.log.borrow_mut().push(format!("pass {index}"));
        if let Some(step) = self.passes.get(index).copied() {
            step(self);
        }
        *self.dirty.borrow()
    }

    fn replacing(&self) -> bool {
        self.replacing_until
            .borrow()
            .is_some_and(|until| Instant::now() < until)
    }
}

const QUICK: Limits = Limits {
    rounds: Duration::from_secs(5),
    replacement: Duration::from_secs(5),
    hard: Duration::from_secs(30),
};

fn mark(scripted: &Scripted) {
    *scripted.dirty.borrow_mut() = true;
}

#[test]
fn a_hook_that_finds_the_lock_taken_marks_the_state_dirty_and_leaves() {
    let mut hook = Scripted::default();
    *hook.taken.borrow_mut() = true;
    assert_eq!(reconcile(&mut hook, QUICK, |_| {}), Outcome::Deferred);
    assert!(hook.is_dirty(), "for the holder to go round again");
    assert_eq!(hook.ran, 0);

    // Without a flag to leave, nobody would go round for it.
    let mut stateless = Scripted {
        cannot_mark: true,
        ..Scripted::default()
    };
    *stateless.taken.borrow_mut() = true;
    assert_eq!(reconcile(&mut stateless, QUICK, |_| {}), Outcome::NoState);
}

#[test]
fn the_holder_goes_round_again_for_a_hook_that_came_during_its_pass() {
    let mut hook = Scripted {
        passes: vec![mark, mark, |_| {}],
        ..Scripted::default()
    };
    assert_eq!(
        reconcile(&mut hook, QUICK, |_| {}),
        Outcome::Done { passes: 3 }
    );
    assert!(!hook.is_dirty());
    assert_eq!(
        *hook.log.borrow(),
        ["pass 0", "pass 1", "pass 2", "release"]
    );
}

#[test]
fn a_flag_set_after_the_holders_last_look_is_not_lost() {
    // Another hook marks the state dirty after the last pass ended and
    // before the lock is given up: it failed to take the lock, so the
    // holder must look once more after releasing.
    let mut hook = Scripted {
        on_release: Some(mark),
        ..Scripted::default()
    };
    assert_eq!(
        reconcile(&mut hook, QUICK, |_| {}),
        Outcome::Done { passes: 2 }
    );
    assert_eq!(
        *hook.log.borrow(),
        ["pass 0", "release", "pass 1", "release"]
    );
}

#[test]
fn no_pass_starts_once_the_time_for_rounds_is_up() {
    let limits = Limits {
        rounds: Duration::from_millis(60),
        ..QUICK
    };
    let slow: fn(&Scripted) = |scripted| {
        thread::sleep(Duration::from_millis(40));
        mark(scripted);
    };
    let mut hook = Scripted {
        passes: vec![slow; 10],
        ..Scripted::default()
    };
    let started = Instant::now();
    assert_eq!(
        reconcile(&mut hook, limits, |_| {}),
        Outcome::OutOfTime { passes: 2 }
    );
    assert!(started.elapsed() < Duration::from_millis(200));
    // Left dirty for the next hook or dashboard.
    assert!(hook.is_dirty());
}

#[test]
fn a_replacement_is_waited_for_without_the_lock_and_then_recorded() {
    let taken = Rc::new(RefCell::new(false));
    let extended = RefCell::new(Vec::new());
    // The first pass starts a replacement that runs for a while.
    let start: fn(&Scripted) = |scripted| {
        *scripted.replacing_until.borrow_mut() = Some(Instant::now() + Duration::from_millis(80));
    };
    let mut hook = Scripted {
        taken: taken.clone(),
        passes: vec![start],
        ..Scripted::default()
    };
    // The rounds are over before it ends, yet a pass is owed to record it.
    let limits = Limits {
        rounds: Duration::ZERO,
        ..QUICK
    };
    let started = Instant::now();
    let outcome = reconcile(&mut hook, limits, |at| extended.borrow_mut().push(at));
    assert_eq!(outcome, Outcome::Done { passes: 2 });
    assert!(started.elapsed() >= Duration::from_millis(80));
    assert_eq!(
        *hook.log.borrow(),
        ["pass 0", "release", "pass 1", "release"]
    );
    // The watchdog's deadline moved past the wait.
    assert_eq!(extended.borrow().len(), 1);
    assert!(extended.borrow()[0] >= started + QUICK.replacement);
    assert!(!*taken.borrow(), "given up at the end");
}

#[test]
fn the_watchdog_ends_a_hook_at_its_limit_unless_moved_or_stopped() {
    let fired = Arc::new(AtomicBool::new(false));
    let started = Instant::now();
    let watchdog = {
        let fired = fired.clone();
        Watchdog::start(Duration::from_millis(80), move || {
            fired.store(true, Ordering::Relaxed)
        })
    };
    watchdog.extend_to(started + Duration::from_millis(200));
    // An earlier deadline does not shorten it.
    watchdog.extend_to(started + Duration::from_millis(10));
    thread::sleep(Duration::from_millis(140));
    assert!(!fired.load(Ordering::Relaxed));
    while !fired.load(Ordering::Relaxed) {
        assert!(started.elapsed() < Duration::from_secs(2), "never fired");
        thread::sleep(Duration::from_millis(5));
    }
    assert!(started.elapsed() >= Duration::from_millis(200));

    let stopped = Arc::new(AtomicBool::new(false));
    let watchdog = {
        let stopped = stopped.clone();
        Watchdog::start(Duration::from_millis(30), move || {
            stopped.store(true, Ordering::Relaxed)
        })
    };
    watchdog.stop();
    thread::sleep(Duration::from_millis(100));
    assert!(!stopped.load(Ordering::Relaxed));
}

const ROOT: &str = "/repos/hook-weather";

fn agent(pane: u8, name: &str, state: &str, change: u64, tokens: Value) -> Value {
    json!({
        "pane_id": format!("w1:p{pane}"),
        "workspace_id": "w1",
        "tab_id": "w1:t1",
        "agent": "claude",
        "agent_status": state,
        "state_change_seq": change,
        "name": name,
        "cwd": ROOT,
        "tokens": tokens,
        "agent_session": { "value": format!("{name}-session") }
    })
}

/// The weather project's idle supervisor, and a worker in `state` after
/// `change` state changes.
fn weather(state: &str, change: u64) -> Value {
    let mut supervisor = json!({});
    supervisor[SUPERVISOR_TOKEN] = json!(marker(
        "supervisor-weather",
        Some("supervisor-weather-session")
    ));
    json!({
        "workspaces": [{"workspace_id": "w1", "label": "weather"}],
        "agents": [
            agent(1, "supervisor-weather", "idle", 1, supervisor),
            agent(2, "w-forecast", state, change, json!({})),
        ]
    })
}

/// One hook run on `herdr`, its files under `base`.
fn hook(herdr: &FakeHerdr, base: &std::path::Path) -> (Outcome, HookPasses) {
    let client = HerdrClient::from_socket_path(&herdr.socket);
    let mut passes = HookPasses::with_waker(client, SupervisorWaker::in_dir(base));
    let outcome = reconcile(&mut passes, QUICK, |_| {});
    (outcome, passes)
}

#[test]
fn a_worker_going_idle_wakes_its_supervisor_once_with_no_dashboard() {
    let scratch = ScratchDir::new("hook-wake");
    let herdr = FakeHerdr::start("hook-wake", weather("working", 2));
    // The first hook sees the worker at work.
    let (outcome, _) = hook(&herdr, &scratch);
    assert_eq!(outcome, Outcome::Done { passes: 1 });
    assert!(herdr.prompts().is_empty());

    // It stops; the hook for that, and every one after it, finds it once.
    herdr.set_snapshot(weather("idle", 3));
    for _ in 0..3 {
        hook(&herdr, &scratch);
    }
    let prompts = herdr.prompts();
    assert_eq!(prompts.len(), 1, "{prompts:?}");
    assert!(
        prompts[0].starts_with("supervisor-weather: [Corgi] w-forecast is idle."),
        "{prompts:?}"
    );
    // The lock and the flag are left for the next one.
    assert!(!scratch.join("dirty").exists());
}

#[test]
fn a_dropped_event_is_caught_by_the_next_hook_whatever_it_is_for() {
    let scratch = ScratchDir::new("hook-dropped");
    let herdr = FakeHerdr::start("hook-dropped", weather("working", 2));
    hook(&herdr, &scratch);
    // The worker's stop, and its event, never reach a hook. Another event
    // (a pane exiting elsewhere) does.
    herdr.set_snapshot(weather("done", 3));
    hook(&herdr, &scratch);
    assert_eq!(herdr.prompts().len(), 1, "{:?}", herdr.prompts());
}

#[test]
fn a_hook_leaves_the_pass_to_a_dashboard_holding_the_lock_and_it_goes_round() {
    let scratch = ScratchDir::new("hook-dashboard");
    let herdr = FakeHerdr::start("hook-dashboard", weather("working", 2));
    let client = HerdrClient::from_socket_path(&herdr.socket);
    let mut dashboard = SupervisorWaker::in_dir(&scratch);
    hook(&herdr, &scratch);

    // A dashboard's pass is under way when the worker stops.
    assert!(dashboard.begin(&client));
    herdr.set_snapshot(weather("idle", 3));
    let (outcome, _) = hook(&herdr, &scratch);
    assert_eq!(outcome, Outcome::Deferred);
    assert!(herdr.prompts().is_empty());
    // Its pass ends and finds the flag: it is due again at once.
    assert!(dashboard.end());

    let mut state = RefreshState::new(Some(dashboard));
    let inputs = Inputs::new(client);
    let refreshed = refresh::run(&inputs, std::mem::take(&mut state), None);
    assert!(!refreshed.again);
    assert_eq!(herdr.prompts().len(), 1);
    // The next hook finds it delivered.
    let (outcome, _) = hook(&herdr, &scratch);
    assert_eq!(outcome, Outcome::Done { passes: 1 });
    assert_eq!(herdr.prompts().len(), 1);
}

#[test]
fn a_hook_and_a_dashboard_taking_turns_deliver_an_inbox_once() {
    let scratch = ScratchDir::new("hook-turns");
    let herdr = FakeHerdr::start("hook-turns", weather("idle", 1));
    let client = HerdrClient::from_socket_path(&herdr.socket);
    let dashboard = SupervisorWaker::in_dir(&scratch);
    dashboard
        .notify(Item {
            project: ROOT.into(),
            source: "test".into(),
            kind: Kind::Note,
            key: "forecast".into(),
            text: "The forecast is ready".into(),
            ..Item::default()
        })
        .expect("notify");
    let inputs = Inputs::new(client);
    let mut state = RefreshState::new(Some(dashboard));
    for turn in 0..4 {
        if turn % 2 == 0 {
            hook(&herdr, &scratch);
        } else {
            state = refresh::run(&inputs, state, None).state;
        }
    }
    let prompts = herdr.prompts();
    assert_eq!(prompts.len(), 1, "{prompts:?}");
    assert!(prompts[0].contains("The forecast is ready"), "{prompts:?}");
}

#[test]
fn the_hooks_log_line_says_what_woke_it_and_how_it_ended() {
    let scratch = ScratchDir::new("hook-log");
    let herdr = FakeHerdr::start("hook-log", weather("working", 2));
    let (outcome, passes) = hook(&herdr, &scratch);
    let event = Event::parse(Some("startup"), None);
    let line = log_line(&event, &outcome, &passes, Duration::from_millis(12));
    assert_eq!(line, "corgi hook startup: 1 pass in 12 ms");
    let line = log_line(
        &event,
        &Outcome::Deferred,
        &passes,
        Duration::from_millis(2),
    );
    assert_eq!(
        line,
        "corgi hook startup: deferred to the pass holding the wake lock in 2 ms"
    );
}

/// A client of `herdr` that keeps Corgi's marks and recovery journal under
/// `base`, as the real one does under the state directory, so deliveries
/// are journalled.
fn journalled(herdr: &FakeHerdr, base: &std::path::Path) -> HerdrClient {
    HerdrClient::from_socket_path(&herdr.socket)
        .with_marker_record(base.join("markers").join("herdr.json"))
}

/// One hook run with `client`, its files under `base`.
fn hook_with(client: &HerdrClient, base: &std::path::Path) -> HookPasses {
    let mut passes = HookPasses::with_waker(client.clone(), SupervisorWaker::in_dir(base));
    reconcile(&mut passes, QUICK, |_| {});
    passes
}

#[test]
fn a_prompt_herdr_refused_before_typing_goes_out_once_the_supervisor_takes_it() {
    let scratch = ScratchDir::new("hook-refused");
    let herdr = FakeHerdr::start("hook-refused", weather("working", 2));
    let client = journalled(&herdr, &scratch);
    let inboxes = SupervisorWaker::in_dir(&scratch).inboxes();
    hook_with(&client, &scratch);
    // The worker stops while the supervisor's pane runs something else:
    // Herdr refuses the prompt and types nothing.
    herdr.set_prompts(Prompts::Refuse("agent_not_ready".into()));
    herdr.set_snapshot(weather("idle", 3));
    hook_with(&client, &scratch);
    hook_with(&client, &scratch);
    assert_eq!(
        herdr.requests("agent.prompt").len(),
        2,
        "tried at each pass"
    );
    assert!(inboxes.held(&client, ROOT).is_empty(), "nothing is held");

    // The supervisor takes prompts again: the next pass delivers, once.
    herdr.set_prompts(Prompts::Accept);
    let passes = hook_with(&client, &scratch);
    assert!(passes.statuses.is_empty(), "{:?}", passes.statuses);
    hook_with(&client, &scratch);
    assert_eq!(herdr.requests("agent.prompt").len(), 3);
    assert!(inboxes.pending_for(ROOT).is_empty());
}

#[test]
fn a_prompt_whose_outcome_is_unknown_is_held_and_shown_until_read() {
    let scratch = ScratchDir::new("hook-unknown");
    let herdr = FakeHerdr::start("hook-unknown", weather("working", 2));
    let client = journalled(&herdr, &scratch);
    let inboxes = SupervisorWaker::in_dir(&scratch).inboxes();
    hook_with(&client, &scratch);
    // Herdr hangs up without saying whether it typed the wake in.
    herdr.set_prompts(Prompts::HangUp);
    herdr.set_snapshot(weather("idle", 3));
    hook_with(&client, &scratch);
    let held = inboxes.held(&client, ROOT);
    assert_eq!(held.len(), 1, "{held:?}");

    // It is never typed in again, nor is a later stop of the project.
    herdr.set_prompts(Prompts::Accept);
    herdr.set_snapshot(weather("working", 4));
    hook_with(&client, &scratch);
    herdr.set_snapshot(weather("idle", 5));
    let passes = hook_with(&client, &scratch);
    assert_eq!(herdr.requests("agent.prompt").len(), 1);
    assert!(
        passes
            .statuses
            .iter()
            .any(|status| status.contains("quarantined")),
        "{:?}",
        passes.statuses
    );
    assert_eq!(inboxes.held(&client, ROOT), held);

    // Read with `corgi inbox --take`, the held item is done with, and the
    // later stop goes out alone.
    let dir = scratch.join("hook-weather");
    crate::inbox::mark_delivered(&dir, &held, "corgi inbox --take").expect("take");
    assert!(inboxes.held(&client, ROOT).is_empty());
    hook_with(&client, &scratch);
    let prompts = herdr.prompts();
    assert_eq!(prompts.len(), 2, "{prompts:?}");
    assert!(prompts[1].contains("w-forecast is idle"), "{prompts:?}");
    assert!(inboxes.pending_for(ROOT).is_empty());
}

#[test]
fn only_herdrs_refusals_before_typing_are_retried() {
    for (code, retried) in [
        ("agent_not_found", true),
        ("agent_target_ambiguous", true),
        ("agent_blocked", true),
        ("agent_not_ready", true),
        // Failed while writing, or timed out: it may be in the box.
        ("agent_prompt_failed", false),
        ("timeout", false),
    ] {
        let scratch = ScratchDir::new("hook-codes");
        let herdr = FakeHerdr::start("hook-codes", weather("working", 2));
        let client = journalled(&herdr, &scratch);
        hook_with(&client, &scratch);
        herdr.set_prompts(Prompts::Refuse(code.into()));
        herdr.set_snapshot(weather("idle", 3));
        hook_with(&client, &scratch);
        let held = SupervisorWaker::in_dir(&scratch)
            .inboxes()
            .held(&client, ROOT);
        assert_eq!(held.is_empty(), retried, "{code}");
    }
}
