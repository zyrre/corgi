//! The hc waker's passes against a fake core: what it types into which
//! terminal, when it waits, and how a handover goes.

use std::{
    collections::HashMap,
    path::Path,
    time::{Duration, Instant},
};

use super::*;
use crate::{
    hc::HcError,
    model::{AgentInfo, AgentSession, AgentState},
    test_support::ScratchDir,
};

const ROOT: &str = "/repos/weather";
const SUPERVISOR: &str = "weather-supervisor";

/// A core that answers from what the test set, and records what it was
/// asked to do.
#[derive(Default)]
struct FakeCore {
    ready: HashMap<String, Ready>,
    /// What each send returns, in order; `Sent` once they run out.
    deliveries: Vec<Delivery>,
    sent: Vec<(String, String)>,
    labels: Vec<(String, Vec<(String, String)>)>,
    replaced: Vec<Replacement>,
    looks: usize,
}

impl Core for FakeCore {
    fn ready(&mut self, supervisor: &Agent) -> Ready {
        self.looks += 1;
        self.ready
            .get(&supervisor.info.address)
            .copied()
            .unwrap_or(Ready::Yes)
    }

    fn send(&mut self, address: &str, text: &str) -> Delivery {
        self.sent.push((address.to_string(), text.to_string()));
        if self.deliveries.is_empty() {
            Delivery::Sent
        } else {
            self.deliveries.remove(0)
        }
    }

    fn report(&mut self, agent: &Agent) -> Capture {
        Capture::Found(Some(Captured {
            text: format!("{} finished the forecast.", agent.info.display_name()),
            transcript: true,
        }))
    }

    fn label(&mut self, address: &str, labels: &[(&str, &str)]) -> Result<()> {
        self.labels.push((
            address.to_string(),
            labels
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        ));
        Ok(())
    }

    fn replace(&mut self, replacement: Replacement) -> Job<Result<String>> {
        let name = replacement.name.clone();
        self.replaced.push(replacement);
        Job::spawn(move || Ok(format!("{name} handed over to a fresh session")))
    }
}

fn agent(name: &str, address: &str, supervisor: bool, state: AgentState, turn: u64) -> Agent {
    Agent {
        info: AgentInfo {
            address: address.into(),
            name: Some(name.into()),
            state,
            state_change_seq: turn,
            agent: Some("claude".into()),
            ..AgentInfo::default()
        },
        project_root: ROOT.into(),
        supervisor,
        ..Agent::default()
    }
}

fn supervisor(state: AgentState, turn: u64) -> Agent {
    agent(SUPERVISOR, "t1", true, state, turn)
}

/// A worker the supervisor spawned (it carries a request id).
fn worker(state: AgentState, turn: u64) -> Agent {
    let mut worker = agent("w-forecast", "t2", false, state, turn);
    worker
        .info
        .tokens
        .insert(crate::launch::CORGI_REQUEST_TOKEN.into(), "brief-1".into());
    worker
}

fn led(scratch: &ScratchDir) -> SupervisorWaker {
    let mut waker = SupervisorWaker::for_core_in(scratch);
    assert!(waker.lead());
    waker
}

const CORGI: &str = "/opt/corgi";

#[test]
fn a_stop_waits_for_a_resting_supervisor_with_an_empty_box_then_goes_once() {
    let scratch = ScratchDir::new("hc-waker");
    let mut waker = led(&scratch);
    let mut core = FakeCore::default();
    let corgi = Path::new(CORGI);
    use AgentState::{Done, Working};

    waker.tick(
        &mut core,
        &[supervisor(Done, 1), worker(Working, 0)],
        corgi,
        0,
    );
    assert!(core.sent.is_empty());

    // The worker is done while the user writes in the supervisor's box.
    core.ready.insert("t1".into(), Ready::Drafting);
    let said = waker.tick(&mut core, &[supervisor(Done, 1), worker(Done, 1)], corgi, 1);
    assert!(core.sent.is_empty());
    assert!(
        said.contains(&format!(
            "{SUPERVISOR}'s input box holds a draft: its inbox waits"
        )),
        "{said:?}"
    );
    assert_eq!(waker.undelivered_for(ROOT), 1);

    // Its shell in front, or a turn of its own: still nothing.
    core.ready.insert("t1".into(), Ready::Busy);
    waker.tick(&mut core, &[supervisor(Done, 1), worker(Done, 1)], corgi, 2);
    waker.tick(
        &mut core,
        &[supervisor(Working, 1), worker(Done, 1)],
        corgi,
        3,
    );
    assert!(core.sent.is_empty());

    core.ready.insert("t1".into(), Ready::Yes);
    let said = waker.tick(&mut core, &[supervisor(Done, 2), worker(Done, 1)], corgi, 4);
    assert_eq!(core.sent.len(), 1, "{:?}", core.sent);
    let (address, text) = &core.sent[0];
    assert_eq!(address, "t1");
    assert!(
        text.starts_with("[Corgi] w-forecast is done. Its report follows"),
        "{text}"
    );
    assert!(
        text.contains("> w-forecast finished the forecast."),
        "{text}"
    );
    assert!(said.iter().any(|line| line.contains("Delivered the inbox")));
    assert_eq!(waker.undelivered_for(ROOT), 0);

    waker.tick(&mut core, &[supervisor(Done, 2), worker(Done, 1)], corgi, 5);
    assert_eq!(core.sent.len(), 1, "sent once");
}

#[test]
fn a_blocked_worker_wakes_with_hcs_command_and_a_refused_send_is_tried_again() {
    let scratch = ScratchDir::new("hc-waker");
    let mut waker = led(&scratch);
    let mut core = FakeCore {
        deliveries: vec![Delivery::Refused],
        ..FakeCore::default()
    };
    let corgi = Path::new(CORGI);
    use AgentState::{Blocked, Done, Working};

    waker.tick(
        &mut core,
        &[supervisor(Done, 1), worker(Working, 0)],
        corgi,
        0,
    );
    waker.tick(
        &mut core,
        &[supervisor(Done, 1), worker(Blocked, 0)],
        corgi,
        1,
    );
    // hc refused before typing (say, the supervisor turned blocked): kept.
    assert_eq!(core.sent.len(), 1);
    assert_eq!(waker.undelivered_for(ROOT), 1);
    waker.tick(
        &mut core,
        &[supervisor(Done, 1), worker(Blocked, 0)],
        corgi,
        2,
    );
    assert_eq!(core.sent.len(), 2);
    assert_eq!(
        core.sent[1].1,
        "[Corgi] w-forecast is blocked. Run: hc read w-forecast --lines 120"
    );
    assert_eq!(waker.undelivered_for(ROOT), 0);
}

#[test]
fn a_supervisor_is_looked_at_once_per_pass() {
    let scratch = ScratchDir::new("hc-waker");
    let mut waker = led(&scratch);
    let mut core = FakeCore::default();
    core.ready.insert("t1".into(), Ready::Drafting);
    let corgi = Path::new(CORGI);
    use AgentState::{Done, Working};
    waker.tick(
        &mut core,
        &[supervisor(Done, 1), worker(Working, 0)],
        corgi,
        0,
    );
    assert_eq!(core.looks, 0, "nothing to type, nothing to look at");
    waker.tick(&mut core, &[supervisor(Done, 1), worker(Done, 1)], corgi, 1);
    assert_eq!(core.looks, 1);
}

fn full_supervisor(state: AgentState, turn: u64) -> Agent {
    let mut supervisor = supervisor(state, turn);
    supervisor.info.agent_session = Some(AgentSession {
        agent: "claude".into(),
        value: "s-1".into(),
    });
    supervisor.context_percent = Some(60);
    supervisor.context_tokens = Some(120_000);
    supervisor
}

#[test]
fn a_full_supervisor_is_asked_for_its_note_when_its_cache_is_about_to_go_cold_then_replaced() {
    let scratch = ScratchDir::new("hc-waker");
    let mut waker = led(&scratch);
    let mut core = FakeCore::default();
    let corgi = Path::new(CORGI);
    use AgentState::{Done, Working};
    let idle = crate::supervisor::handover_idle_secs(&crate::harness::Harness::Claude).unwrap();

    // The rest is timed from the first pass that sees it.
    waker.tick(&mut core, &[full_supervisor(Done, 3)], corgi, 1_000);
    assert!(core.sent.is_empty());
    waker.tick(
        &mut core,
        &[full_supervisor(Done, 3)],
        corgi,
        1_000 + idle - 1,
    );
    assert!(core.sent.is_empty(), "the cache is still warm");
    let asked_at = 1_000 + idle;
    let said = waker.tick(&mut core, &[full_supervisor(Done, 3)], corgi, asked_at);
    assert_eq!(core.sent.len(), 1, "{said:?}");
    let (address, request) = &core.sent[0];
    assert_eq!(address, "t1");
    assert!(
        request.contains("role handover /repos/weather") && request.contains("handover.md"),
        "{request}"
    );
    assert!(core.labels.iter().any(|(address, labels)| {
        address == "t1" && labels.contains(&(HANDOVER_LABEL.into(), format!("{asked_at} s-1")))
    }));
    assert!(waker.handing_over_any());

    // It writes its note in a turn, and rests.
    waker.tick(
        &mut core,
        &[full_supervisor(Working, 3)],
        corgi,
        asked_at + 5,
    );
    let dir = waker.inbox_dir(ROOT).unwrap();
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join(crate::supervisor::HANDOVER_NOTE), "# Handover\n").unwrap();
    waker.tick(&mut core, &[full_supervisor(Done, 4)], corgi, asked_at + 30);
    assert_eq!(
        core.replaced,
        [Replacement {
            root: ROOT.into(),
            name: SUPERVISOR.into(),
            address: "t1".into(),
            harness: crate::harness::Harness::Claude,
        }]
    );
    // The replacement ends in the background; the next pass records it.
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut said = Vec::new();
    while Instant::now() < deadline && !waker.replacements.is_empty() {
        std::thread::sleep(Duration::from_millis(20));
        said = waker.tick(&mut core, &[full_supervisor(Done, 4)], corgi, asked_at + 40);
    }
    assert!(
        said.contains(&format!("{SUPERVISOR} handed over to a fresh session")),
        "{said:?}"
    );
    assert!(!waker.handing_over_any());
}

#[test]
fn a_supervisor_with_a_draft_is_not_asked_for_its_note() {
    let scratch = ScratchDir::new("hc-waker");
    let mut waker = led(&scratch);
    let mut core = FakeCore::default();
    core.ready.insert("t1".into(), Ready::Drafting);
    let corgi = Path::new(CORGI);
    let idle = crate::supervisor::handover_idle_secs(&crate::harness::Harness::Claude).unwrap();
    waker.tick(&mut core, &[full_supervisor(AgentState::Done, 3)], corgi, 0);
    waker.tick(
        &mut core,
        &[full_supervisor(AgentState::Done, 3)],
        corgi,
        idle + 1,
    );
    assert!(core.sent.is_empty());
    assert!(!waker.handing_over_any());
}

#[test]
fn one_waker_leads_a_core_and_its_records_are_named_after_the_core() {
    let scratch = ScratchDir::new("hc-waker");
    let _first = led(&scratch);
    let mut second = SupervisorWaker::for_core_in(&scratch);
    assert!(!second.lead());

    let waker = SupervisorWaker::for_core(Path::new("/run/hc/x.sock"));
    let record = waker.record_file().unwrap();
    assert!(
        record.ends_with("wake/_run_hc_x_sock.seen.json"),
        "{}",
        record.display()
    );
}

#[test]
fn a_send_hc_refused_before_typing_is_tried_again_and_any_other_failure_is_unknown() {
    let refused = |kind: &str| -> Result<()> {
        Err(HcError {
            kind: kind.into(),
            ..HcError::default()
        }
        .into())
    };
    assert_eq!(delivery_of(&Ok(())), Delivery::Sent);
    for kind in ["blocked", "not_found", "finished", "unavailable"] {
        assert_eq!(delivery_of(&refused(kind)), Delivery::Refused, "{kind}");
    }
    assert_eq!(delivery_of(&refused("io")), Delivery::Unknown);
    assert_eq!(
        delivery_of::<()>(&Err(anyhow::anyhow!("hc send failed (signal 9)"))),
        Delivery::Unknown
    );
}

/// A second worker the supervisor spawned, at `address`.
fn other_worker(name: &str, address: &str, state: AgentState, turn: u64) -> Agent {
    let mut worker = agent(name, address, false, state, turn);
    worker
        .info
        .tokens
        .insert(crate::launch::CORGI_REQUEST_TOKEN.into(), "brief-2".into());
    worker
}

/// Marks `name`, at `address`, as `corgi pause` does, in `phase`.
fn mark(path: &Path, name: &str, address: &str, phase: crate::hc::pause::Phase) {
    crate::hc::pause::update(path, |record| {
        record.agents.retain(|paused| paused.name != name);
        record.agents.push(crate::hc::pause::PausedAgent {
            address: address.into(),
            name: name.into(),
            project: ROOT.into(),
            supervisor: false,
            harness: "claude".into(),
            session: Some("s-1".into()),
            resume: "claude --resume s-1".into(),
            cwd: "/w".into(),
            labels: Default::default(),
            phase,
            at: 100,
        });
    })
    .unwrap();
}

#[test]
fn while_agents_are_paused_no_supervisor_is_typed_into_and_wakes_wait_for_resume() {
    use crate::hc::pause::Phase;
    let scratch = ScratchDir::new("hc-waker-pause");
    let path = scratch.join("pause.json");
    let mut waker = led(&scratch);
    let mut core = FakeCore::default();
    let corgi = Path::new(CORGI);
    use AgentState::{Done, Idle, Working};

    waker.tick(
        &mut core,
        &[
            supervisor(Done, 1),
            worker(Working, 0),
            other_worker("w-tides", "t3", Working, 0),
        ],
        corgi,
        100,
    );
    // Pause stops w-forecast (Esc, then its exit ends its terminal);
    // w-tides finishes its turn meanwhile, and the supervisor rests.
    mark(&path, "w-forecast", "t2", Phase::Stopping);
    waker.tick(
        &mut core,
        &[
            supervisor(Done, 1),
            worker(Idle, 1),
            other_worker("w-tides", "t3", Done, 1),
        ],
        corgi,
        101,
    );
    mark(&path, "w-forecast", "t2", Phase::Paused);
    let said = waker.tick(
        &mut core,
        &[supervisor(Done, 1), other_worker("w-tides", "t3", Done, 1)],
        corgi,
        102,
    );
    assert!(core.sent.is_empty(), "{:?}", core.sent);
    assert_eq!(
        core.looks, 0,
        "no supervisor is even looked at to be typed into"
    );
    assert!(
        said.iter()
            .any(|line| line.starts_with("Agents are paused"))
    );
    assert!(waker.pause_in_force());
    // w-tides's wake waits in the inbox; the paused worker added none.
    let pending = waker.inboxes.pending_for(ROOT);
    assert_eq!(pending.len(), 1, "{pending:?}");

    // Resume starts w-forecast again in a new terminal: its return wakes no
    // one, its mark goes, and the waiting wake goes out.
    mark(&path, "w-forecast", "t9", Phase::Resuming);
    let said = waker.tick(
        &mut core,
        &[
            supervisor(Done, 1),
            other_worker("w-tides", "t3", Done, 1),
            agent("w-forecast", "t9", false, Idle, 0),
        ],
        corgi,
        103,
    );
    assert!(
        said.contains(&"w-forecast is back after Resume".to_string()),
        "{said:?}"
    );
    assert!(!path.exists());
    assert!(!waker.pause_in_force());
    assert_eq!(core.sent.len(), 1, "{:?}", core.sent);
    let text = &core.sent[0].1;
    assert!(text.contains("w-tides is done"), "{text}");
    assert!(!text.contains("w-forecast"), "{text}");
}
