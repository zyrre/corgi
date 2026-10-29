//! The window's copy of Corgi's state, kept from `corgi serve`'s snapshot
//! and deltas, and the requests sent to it.

use std::{
    collections::{HashMap, HashSet},
    time::{Duration, Instant},
};

use corgi_core::protocol::{
    Agent, Command, Hello, Progress, Project, Reply, Request, ServerMessage, State,
};

/// Where the connection to `corgi serve` stands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Connection {
    /// Started, waiting for the hello and snapshot.
    Connecting,
    Connected,
    /// It ended; the next attempt is due at `retry_at`.
    Lost {
        why: String,
        retry_at: Instant,
    },
}

/// What a message asks of the window.
#[derive(Debug, PartialEq)]
pub enum Received {
    /// The state changed: draw again.
    State,
    /// A delta out of sequence: ask for the whole state again.
    Resync,
    Reply(Reply),
    Progress(Progress),
}

/// The last state `corgi serve` sent, kept while the connection is lost so
/// the window can show it dimmed.
#[derive(Debug)]
pub struct Store {
    pub state: State,
    /// The sequence number of `state`, `None` before the first snapshot.
    pub seq: Option<u64>,
    pub hello: Option<Hello>,
    pub connection: Connection,
    /// The server's clock at the last message, and when it came.
    server_now: u64,
    received_at: Instant,
}

impl Default for Store {
    fn default() -> Self {
        Self {
            state: State::default(),
            seq: None,
            hello: None,
            connection: Connection::Connecting,
            server_now: 0,
            received_at: Instant::now(),
        }
    }
}

impl Store {
    pub fn receive(&mut self, message: ServerMessage) -> Option<Received> {
        match message {
            ServerMessage::Hello(hello) => {
                self.hello = Some(hello);
                None
            }
            ServerMessage::Snapshot(snapshot) => {
                self.connection = Connection::Connected;
                self.state = snapshot.state;
                self.seq = Some(snapshot.seq);
                self.tick(snapshot.now);
                Some(Received::State)
            }
            ServerMessage::Delta(delta) => {
                if self.seq.map(|seq| seq + 1) != Some(delta.seq) {
                    return Some(Received::Resync);
                }
                self.state.apply(delta.changes);
                self.seq = Some(delta.seq);
                self.tick(delta.now);
                Some(Received::State)
            }
            ServerMessage::Progress(progress) => Some(Received::Progress(progress)),
            ServerMessage::Reply(reply) => Some(Received::Reply(reply)),
        }
    }

    fn tick(&mut self, now: u64) {
        self.server_now = now;
        self.received_at = Instant::now();
    }

    /// The connection ended: keep the state, dimmed, and wait `delay`.
    pub fn lost(&mut self, why: String, delay: Duration) {
        self.connection = Connection::Lost {
            why,
            retry_at: Instant::now() + delay,
        };
        // The next connection starts from its own snapshot.
        self.seq = None;
        self.hello = None;
    }

    pub fn connecting(&mut self) {
        self.connection = Connection::Connecting;
    }

    pub fn connected(&self) -> bool {
        self.connection == Connection::Connected
    }

    /// The server's clock now, for countdowns: its last reading plus the
    /// time since, so a host whose clock differs still counts right.
    pub fn now(&self) -> u64 {
        self.server_now + self.received_at.elapsed().as_secs()
    }

    pub fn agent(&self, pane_id: &str) -> Option<&Agent> {
        self.state
            .agents
            .iter()
            .find(|agent| agent.pane_id == pane_id)
    }

    pub fn project(&self, group: &str) -> Option<&Project> {
        self.state
            .projects
            .iter()
            .find(|project| project.group == group)
    }

    /// The project `pane_id`'s row is in.
    pub fn project_of(&self, pane_id: &str) -> Option<&Project> {
        self.project(&self.agent(pane_id)?.project_group)
    }

    /// Whether `command` may be sent: never one that moves Herdr's focus,
    /// which would move every attached Herdr client. `focus` and `edit` are
    /// refused by name, and any other command the hello marks.
    pub fn allows(&self, command: &Command) -> bool {
        let name = command_name(command);
        if matches!(command, Command::Focus { .. } | Command::Edit { .. }) {
            return false;
        }
        !self.hello.as_ref().is_some_and(|hello| {
            hello
                .commands
                .iter()
                .any(|info| info.name == name && info.touches_focus)
        })
    }
}

/// `command`'s name on the wire.
pub fn command_name(command: &Command) -> String {
    serde_json::to_value(command)
        .ok()
        .and_then(|value| value.get("command")?.as_str().map(String::from))
        .unwrap_or_default()
}

/// Why a request was sent, so its reply and progress reach the right place.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Purpose {
    /// Nothing waits on it (refresh, watch, resync).
    Quiet,
    Prompt,
    Answer,
    /// Closes the question `question` (`Q3`) of `project_root`.
    CloseQuestion {
        project_root: String,
        question: String,
    },
    NewAgentForm,
    Spawn,
    StartSupervisor,
    MergeCheck,
    Merge,
    MergeEnd,
    CloseCheck,
    Close,
    Tag,
}

/// The requests sent and not yet answered.
#[derive(Debug, Default)]
pub struct Outbox {
    next_id: u64,
    pending: HashMap<u64, Purpose>,
}

impl Outbox {
    /// A request for `command`, numbered, remembered under `purpose`; `None`
    /// for a command the store refuses.
    pub fn request(
        &mut self,
        store: &Store,
        command: Command,
        purpose: Purpose,
    ) -> Option<Request> {
        if !store.allows(&command) {
            return None;
        }
        self.next_id += 1;
        self.pending.insert(self.next_id, purpose);
        Some(Request {
            id: self.next_id,
            command,
        })
    }

    /// The purpose of the request `id`, still waiting for its reply.
    pub fn purpose(&self, id: u64) -> Option<&Purpose> {
        self.pending.get(&id)
    }

    /// Takes the purpose of the request `reply` answers.
    pub fn answered(&mut self, reply: &Reply) -> Option<Purpose> {
        self.pending.remove(&reply.id?)
    }

    /// Forgets a request that could not be written.
    pub fn unsent(&mut self, id: u64) {
        self.pending.remove(&id);
    }

    /// The connection ended: every request still waiting is answered by
    /// nobody. Returns their purposes.
    pub fn lost(&mut self) -> Vec<Purpose> {
        self.pending.drain().map(|(_, purpose)| purpose).collect()
    }
}

/// The questions the window asked `corgi serve` to close, by project root
/// and handle, shown greyed until the ledger drops them. Kept only here: a
/// state without one forgets it, and so does a lost connection, so a
/// question the next snapshot still lists shows as open again.
#[derive(Debug, Default)]
pub struct Closing(HashSet<(String, String)>);

impl Closing {
    pub fn start(&mut self, project_root: &str, question: &str) {
        self.0.insert((project_root.into(), question.into()));
    }

    pub fn contains(&self, project_root: &str, question: &str) -> bool {
        self.0
            .contains(&(project_root.to_string(), question.to_string()))
    }

    /// What `reply` to a request sent for `purpose` does: a refused close
    /// leaves its question open.
    pub fn replied(&mut self, purpose: &Purpose, reply: &Reply) {
        if let Purpose::CloseQuestion {
            project_root,
            question,
        } = purpose
            && !reply.ok
        {
            self.0.remove(&(project_root.clone(), question.clone()));
        }
    }

    /// Forgets each question `state` no longer lists.
    pub fn prune(&mut self, state: &State) {
        self.0.retain(|(root, handle)| {
            state.projects.iter().any(|project| {
                project.root == *root && project.questions.iter().any(|open| open.id == *handle)
            })
        });
    }

    pub fn clear(&mut self) {
        self.0.clear();
    }
}

#[cfg(test)]
mod tests {
    use corgi_core::protocol::{Change, CommandInfo, Delta, Snapshot};

    use super::*;

    fn agent(pane_id: &str) -> Agent {
        Agent {
            pane_id: pane_id.into(),
            ..Agent::default()
        }
    }

    fn snapshot(seq: u64, agents: &[&str]) -> ServerMessage {
        ServerMessage::Snapshot(Box::new(Snapshot {
            seq,
            now: 1000,
            state: State {
                agents: agents.iter().map(|id| agent(id)).collect(),
                ..State::default()
            },
        }))
    }

    fn delta(seq: u64, changes: Vec<Change>) -> ServerMessage {
        ServerMessage::Delta(Delta {
            seq,
            now: 1005,
            changes,
        })
    }

    #[test]
    fn deltas_in_sequence_apply_and_a_gap_asks_for_the_whole_state() {
        let mut store = Store::default();
        assert_eq!(store.connection, Connection::Connecting);
        // A delta before any snapshot cannot apply.
        assert_eq!(store.receive(delta(1, vec![])), Some(Received::Resync));
        assert_eq!(store.receive(snapshot(4, &["a"])), Some(Received::State));
        assert!(store.connected());
        let added = Change::Agent(Box::new(agent("b")));
        assert_eq!(store.receive(delta(5, vec![added])), Some(Received::State));
        assert_eq!(store.state.agents.len(), 2);
        assert_eq!(store.now(), 1005);
        assert_eq!(store.receive(delta(7, vec![])), Some(Received::Resync));
        assert_eq!(store.seq, Some(5));
    }

    #[test]
    fn a_lost_connection_keeps_the_state_and_starts_the_next_from_a_snapshot() {
        let mut store = Store::default();
        store.receive(snapshot(0, &["a"]));
        store.lost("ssh: connection closed".into(), Duration::from_secs(2));
        assert!(!store.connected());
        assert_eq!(
            store.state.agents.len(),
            1,
            "the last state stays to be dimmed"
        );
        assert_eq!(store.receive(delta(1, vec![])), Some(Received::Resync));
        store.connecting();
        assert_eq!(
            store.receive(snapshot(0, &["a", "b"])),
            Some(Received::State)
        );
        assert!(store.connected());
    }

    #[test]
    fn commands_that_move_herdr_s_focus_are_never_sent() {
        let mut store = Store::default();
        let mut outbox = Outbox::default();
        let focus = Command::Focus {
            pane_id: "w1:p1".into(),
        };
        let edit = Command::Edit {
            pane_id: "w1:p1".into(),
        };
        assert!(outbox.request(&store, focus, Purpose::Quiet).is_none());
        assert!(outbox.request(&store, edit, Purpose::Quiet).is_none());
        // A command the hello marks is refused too, whatever its name.
        store.hello = Some(Hello {
            protocol: 1,
            corgi: "0".into(),
            socket: String::new(),
            pid: 1,
            commands: vec![CommandInfo {
                name: "tag".into(),
                touches_focus: true,
            }],
        });
        let tag = Command::Tag {
            pane_id: "p".into(),
            merge: true,
        };
        assert!(outbox.request(&store, tag, Purpose::Tag).is_none());
        let prompt = Command::Prompt {
            pane_id: "p".into(),
            text: "go".into(),
        };
        let request = outbox
            .request(&store, prompt, Purpose::Prompt)
            .expect("prompt is allowed");
        assert_eq!(request.id, 1);
        assert_eq!(command_name(&request.command), "prompt");
    }

    #[test]
    fn replies_find_their_purpose_once_and_a_lost_connection_drops_the_rest() {
        let store = Store::default();
        let mut outbox = Outbox::default();
        let merge = outbox
            .request(
                &store,
                Command::MergeCheck {
                    pane_id: "p".into(),
                },
                Purpose::MergeCheck,
            )
            .unwrap();
        outbox.request(&store, Command::Refresh, Purpose::Quiet);
        let reply = Reply {
            id: Some(merge.id),
            ok: true,
            error: None,
            field: None,
            result: None,
        };
        assert_eq!(outbox.answered(&reply), Some(Purpose::MergeCheck));
        assert_eq!(outbox.answered(&reply), None);
        assert_eq!(outbox.lost(), [Purpose::Quiet]);
    }

    #[test]
    fn a_closing_question_lasts_until_the_state_drops_it_or_serve_refuses() {
        let question = |id: &str| corgi_core::protocol::Question {
            id: id.into(),
            ..Default::default()
        };
        let mut state = State {
            projects: vec![Project {
                root: "/r".into(),
                questions: vec![question("Q4"), question("Q5")],
                ..Project::default()
            }],
            ..State::default()
        };
        let mut closing = Closing::default();
        closing.start("/r", "Q4");
        closing.start("/r", "Q5");
        closing.prune(&state);
        assert!(closing.contains("/r", "Q4") && closing.contains("/r", "Q5"));
        assert!(!closing.contains("/other", "Q4"));

        state.projects[0].questions.remove(0);
        closing.prune(&state);
        assert!(!closing.contains("/r", "Q4"));

        let purpose = Purpose::CloseQuestion {
            project_root: "/r".into(),
            question: "Q5".into(),
        };
        let reply = |ok| Reply {
            id: Some(1),
            ok,
            error: (!ok).then(|| "Q5 is not open for /r".into()),
            field: None,
            result: None,
        };
        closing.replied(&purpose, &reply(true));
        assert!(closing.contains("/r", "Q5"));
        closing.replied(&purpose, &reply(false));
        assert!(!closing.contains("/r", "Q5"));
    }
}
