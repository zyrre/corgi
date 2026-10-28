//! The wire format of `corgi serve`: JSON lines over stdio between Corgi and
//! a frontend that draws its own window, such as the desktop GUI. Corgi
//! writes [`ServerMessage`]s, one per line, and reads [`Request`]s, one per
//! line. `docs/serve-protocol.md` documents every message with examples;
//! these types are that document's source of truth, and a frontend in Rust
//! depends on them rather than writing its own.
//!
//! What the dashboard shows is carried as data, never as drawn text: rows,
//! project cards, plan usage, the transcript, open questions and the palette's
//! colors. The first line is a [`Hello`] with [`PROTOCOL_VERSION`], then a
//! [`Snapshot`] of the whole [`State`], then a [`Delta`] whenever a refresh
//! changed something.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::{model::AgentState, usage::PlanUsage, usage::Provider};

/// Raised for any change a frontend built against an older version would
/// misread: a field removed or renamed, or its meaning changed. New fields
/// and new commands do not raise it; a frontend ignores what it does not
/// know.
pub const PROTOCOL_VERSION: u32 = 1;

/// One line Corgi writes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerMessage {
    Hello(Hello),
    Snapshot(Box<Snapshot>),
    Delta(Delta),
    Progress(Progress),
    Reply(Reply),
}

/// The first line: what speaks and which commands it takes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Hello {
    pub protocol: u32,
    /// Corgi's own version, `0.2.3`.
    pub corgi: String,
    /// The Herdr socket this server reads.
    pub socket: String,
    pub pid: u32,
    pub commands: Vec<CommandInfo>,
}

/// A command the server takes, and whether it moves Herdr's focus.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommandInfo {
    pub name: String,
    /// The command calls Herdr's focus methods, which move what every
    /// attached Herdr client shows. A GUI never sends these.
    pub touches_focus: bool,
}

/// The whole state, sent after the hello and again on `resync`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Snapshot {
    /// The sequence number of this state; the next delta has `seq + 1`.
    pub seq: u64,
    /// The server's clock, Unix seconds, for countdowns over `ssh` to a
    /// machine whose clock differs.
    pub now: u64,
    pub state: State,
}

/// What changed since the state numbered `seq - 1`. A frontend that missed
/// one (its last `seq` is not `seq - 1`) sends `resync`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Delta {
    pub seq: u64,
    pub now: u64,
    pub changes: Vec<Change>,
}

/// Everything the dashboard shows.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct State {
    pub herdr: HerdrStatus,
    /// The refresh's status line: marks restored, a supervisor woken.
    pub status: Option<String>,
    /// One card per provider whose CLI is installed, as the header shows.
    pub usage: Vec<UsageCard>,
    /// The project groups, in the dashboard's order.
    pub projects: Vec<Project>,
    /// Every agent row, in the dashboard's order.
    pub agents: Vec<Agent>,
    /// The newest turns of the session `watch` named.
    pub transcript: Option<Transcript>,
    /// The agent `select` named, as `corgi edit` reads it.
    pub selection: Option<String>,
    pub palette: PaletteSet,
}

impl State {
    /// Applies a delta's changes in order, as a frontend keeps its copy of
    /// the state: rows and projects are replaced whole by key, or appended
    /// until an order change places them.
    pub fn apply(&mut self, changes: impl IntoIterator<Item = Change>) {
        for change in changes {
            match change {
                Change::Herdr(herdr) => self.herdr = herdr,
                Change::Status { status } => self.status = status,
                Change::Usage { usage } => self.usage = usage,
                Change::Agent(agent) => replace_keyed(&mut self.agents, *agent, |a| &a.pane_id),
                Change::AgentRemoved { pane_id } => {
                    self.agents.retain(|agent| agent.pane_id != pane_id)
                }
                Change::AgentOrder { pane_ids } => {
                    reorder(&mut self.agents, &pane_ids, |a| &a.pane_id)
                }
                Change::Project(project) => {
                    replace_keyed(&mut self.projects, *project, |p| &p.group)
                }
                Change::ProjectRemoved { group } => {
                    self.projects.retain(|project| project.group != group)
                }
                Change::ProjectOrder { groups } => {
                    reorder(&mut self.projects, &groups, |p| &p.group)
                }
                Change::Transcript { transcript } => self.transcript = transcript,
                Change::Selection { pane_id } => self.selection = pane_id,
                Change::Palette(palette) => self.palette = *palette,
            }
        }
    }
}

/// Replaces the item of `items` with `item`'s key, or appends it.
fn replace_keyed<T>(items: &mut Vec<T>, item: T, key: impl Fn(&T) -> &String) {
    match items.iter_mut().find(|old| key(old) == key(&item)) {
        Some(old) => *old = item,
        None => items.push(item),
    }
}

/// Puts `items` in the order of `keys`; an item `keys` lacks goes last, in
/// its old order.
fn reorder<T>(items: &mut Vec<T>, keys: &[String], key: impl Fn(&T) -> &String) {
    let position = |item: &T| {
        keys.iter()
            .position(|wanted| wanted == key(item))
            .unwrap_or(keys.len())
    };
    items.sort_by_key(|item| position(item));
}

/// Whether Herdr answered the last refresh.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HerdrStatus {
    pub connected: bool,
    /// Why it did not.
    pub error: Option<String>,
}

/// One change in a [`Delta`]. Rows and projects are replaced whole.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "change", rename_all = "snake_case")]
pub enum Change {
    Herdr(HerdrStatus),
    Status {
        status: Option<String>,
    },
    Usage {
        usage: Vec<UsageCard>,
    },
    /// A new or changed row.
    Agent(Box<Agent>),
    AgentRemoved {
        pane_id: String,
    },
    /// The rows' order, whenever a row came, went or moved.
    AgentOrder {
        pane_ids: Vec<String>,
    },
    /// A new or changed project.
    Project(Box<Project>),
    ProjectRemoved {
        group: String,
    },
    ProjectOrder {
        groups: Vec<String>,
    },
    Transcript {
        transcript: Option<Transcript>,
    },
    Selection {
        pane_id: Option<String>,
    },
    Palette(Box<PaletteSet>),
}

/// One agent's row.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Agent {
    /// The row's key, and every command's handle on the agent.
    pub pane_id: String,
    pub workspace_id: String,
    pub tab_id: String,
    /// The name Herdr shows, else the harness.
    pub name: String,
    /// The name Corgi gave the agent, by which a supervisor knows it.
    pub herdr_name: Option<String>,
    /// The harness, as Herdr names it: `claude`, `codex`, …
    pub harness: String,
    pub state: AgentState,
    /// Herdr's counter at the agent's last state change.
    pub state_change_seq: u64,
    /// Tagged by its supervisor and still resting where it was tagged: the
    /// dashboard shows MERGE instead of its state.
    pub ready_to_merge: bool,
    /// Herdr's focus is on this pane.
    pub focused: bool,
    pub supervisor: bool,
    /// Runs in the home directory: a scratch session, in no project.
    pub scratch: bool,
    /// The key of the [`Project`] the row is in.
    pub project_group: String,
    pub project: String,
    pub project_root: String,
    /// The linked worktree the agent works in.
    pub worktree: Option<String>,
    /// That worktree's short name.
    pub checkout: Option<String>,
    pub task: String,
    pub model: Option<String>,
    pub effort: Option<String>,
    pub context_percent: Option<u8>,
    pub context_tokens: Option<u64>,
    pub cache: Option<Cache>,
    /// The session's newest record, Unix seconds.
    pub last_activity: Option<u64>,
    /// The newest thing said in the session.
    pub message: Activity,
    /// The tool call it runs or last ran.
    pub tool: Activity,
    /// A prompt can be typed to it (it is not blocked on a question).
    pub can_prompt: bool,
    /// It works in a linked worktree, which `merge_check` can merge.
    pub can_merge: bool,
}

/// The prompt cache of a session.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Cache {
    /// Unix seconds when it lapses, or, `estimated`, when its documented
    /// minimum retention ends.
    pub expires_at: u64,
    pub tokens: u64,
    pub estimated: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Activity {
    pub kind: ActivityKind,
    pub text: String,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActivityKind {
    Command,
    Tool,
    Prompt,
    Message,
    Thinking,
    Question,
    #[default]
    Ready,
}

/// A project heading, or a card when its supervisor runs.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Project {
    /// The heading, and the project's key.
    pub group: String,
    /// The repository, empty for the scratch sessions.
    pub root: String,
    /// Led by a running supervisor: drawn as a card.
    pub card: bool,
    pub scratch: bool,
    /// The supervisor's pane.
    pub supervisor: Option<String>,
    /// Its rows, in order, the supervisor first.
    pub agents: Vec<String>,
    /// What a collapsed card's footer sums up, workers only.
    pub workers: WorkerCounts,
    /// Each blocked worker and what it waits on.
    pub blocked: Vec<Blocked>,
    /// The numbered questions the supervisor's ledger has open.
    pub questions: Vec<Question>,
    /// How many inbox items are held: Herdr never confirmed a delivery of
    /// them, so nothing more is typed into the supervisor until they are
    /// read (`corgi inbox <root> --take`).
    #[serde(default)]
    pub held: usize,
}

/// A card's workers by state; those tagged ready to merge count only as
/// `merge`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkerCounts {
    pub total: usize,
    pub blocked: usize,
    pub merge: usize,
    pub working: usize,
    pub done: usize,
    pub idle: usize,
    pub unknown: usize,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Blocked {
    pub pane_id: String,
    pub task: String,
    /// The question on its screen, or `waiting for your input`.
    pub waiting_on: String,
}

/// A question the supervisor asked the user, as its ledger lists it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Question {
    /// `Q2`, the handle the user answers by; empty when the ledger gave no
    /// number.
    pub id: String,
    pub title: String,
    /// The worker it is about.
    pub agent: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Transcript {
    pub pane_id: String,
    /// Newest first, the agent's reasoning left out.
    pub entries: Vec<Activity>,
}

/// One provider's card in the header.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UsageCard {
    pub provider: Provider,
    /// `CODEX`, `CLAUDE`.
    pub title: String,
    /// The last good reading.
    pub usage: Option<PlanUsage>,
    /// When it was read, Unix seconds.
    pub fetched_at: Option<u64>,
    /// Why the last reading failed.
    pub error: Option<String>,
}

/// The colors to paint with: the active Omarchy theme when there is one,
/// both entries the same, else Catppuccin Latte and Mocha, so a GUI follows
/// the system's light or dark scheme.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaletteSet {
    /// `omarchy`, `catppuccin`, or `forced` (`CORGI_THEME`).
    pub source: String,
    pub light: ThemeColors,
    pub dark: ThemeColors,
}

/// A theme's colors, each `#rrggbb`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThemeColors {
    pub foreground: String,
    pub background: String,
    /// The sixteen ANSI colors.
    pub ansi: Vec<String>,
    /// Each role of `palette::Roles` by its field name (`accent`, `muted`,
    /// `merge`, …).
    pub roles: BTreeMap<String, String>,
    /// How full a context window is, green to red.
    pub context_ramp: Vec<String>,
}

/// Progress of a long command, before its reply.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Progress {
    pub id: u64,
    /// The steps it takes, sent once at the start.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub steps: Option<Vec<String>>,
    /// The step now running, counted from 0.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub step: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
}

/// The answer to one request. Every request gets exactly one.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Reply {
    /// The request's `id`; `None` when the line could not be read far
    /// enough to find one.
    pub id: Option<u64>,
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// The new-agent form's row a refused `spawn` points at: `task`,
    /// `harness` or `project`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub field: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<ReplyResult>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ReplyResult {
    /// What was done, as the dashboard's status line says it.
    Done {
        message: String,
    },
    NewAgentForm(Box<NewAgentForm>),
    /// `start_supervisor`: started, or found already running in `pane_id`.
    Supervisor {
        pane_id: Option<String>,
        started: bool,
        message: String,
    },
    MergeForm(Box<MergeForm>),
    MergeOutcome(MergeOutcome),
    CloseForm(CloseForm),
    /// `close` found a dirty worktree; `close` with `force` discards it.
    CloseDirty {
        message: String,
    },
    Digest {
        text: String,
    },
}

/// The new-agent form, preset as the dashboard's `n` presets it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct NewAgentForm {
    pub project: String,
    pub projects: Vec<Choice>,
    /// Where a project typed by name alone is created.
    pub new_project_parent: Option<String>,
    pub harness: String,
    pub harnesses: Vec<Choice>,
    /// Empty: the harness's own default.
    pub model: String,
    pub models: Vec<Choice>,
    pub effort: String,
    pub efforts: Vec<Choice>,
    /// The harness takes an effort level; without, leave the row out.
    pub supports_effort: bool,
    pub checkout: String,
    pub checkouts: Vec<Choice>,
    /// What the harness's own configuration picks for an empty model and
    /// effort, for the "Harness default" rows.
    pub default_model: Option<String>,
    pub default_effort: Option<String>,
    /// The project has no agent yet, so `spawn` starts its supervisor.
    pub starts_supervisor: bool,
}

/// One entry of a selector list.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Choice {
    pub value: String,
    pub label: String,
    pub detail: String,
    pub badge: String,
}

/// A merge, ready to confirm with `merge`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MergeForm {
    pub pane_id: String,
    pub agent: String,
    pub task: String,
    pub project_root: String,
    pub worktree: String,
    pub source_branch: String,
    pub target_branch: String,
    /// Short hash and subject, newest first.
    pub commits: Vec<String>,
    /// The target branch's upstream, fetched and checked first.
    pub upstream: Option<String>,
    pub steps: Vec<String>,
}

/// How a merge ended. A merge that did not succeed is still an `ok` reply.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum MergeOutcome {
    Succeeded {
        commit: String,
        message: String,
    },
    /// Merged locally; only the push failed. `supervisor` can take it
    /// (`merge_handoff`).
    PushFailed {
        commit: String,
        error: String,
        supervisor: Option<String>,
    },
    /// Stopped on conflicts; the primary checkout is mid-merge.
    /// `merge_handoff` aborts it and hands it to `supervisor`.
    Conflicted {
        files: Vec<String>,
        supervisor: Option<String>,
    },
    /// The upstream has these commits; `merge` with `pull` takes them first.
    Behind {
        commits: Vec<String>,
    },
    /// Fetching the upstream failed; `merge` with `skip` merges anyway.
    FetchFailed {
        error: String,
    },
    Failed {
        error: String,
    },
}

/// What `close` takes down, for the confirmation.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CloseForm {
    pub pane_id: String,
    pub label: String,
    /// `worktree` (removes `checkout` from disk), `tab` or `workspace`.
    pub target: String,
    pub checkout: Option<String>,
    /// The confirm button's words, as the dashboard's dialog has them.
    pub confirm: String,
}

/// One line a frontend writes: an `id` it chooses, echoed in the reply and
/// any progress, and the command.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Request {
    pub id: u64,
    #[serde(flatten)]
    pub command: Command,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "command", rename_all = "snake_case")]
pub enum Command {
    /// Send the whole state again.
    Resync,
    /// Refresh now rather than at the next interval.
    Refresh,
    /// The agent the frontend has selected, for `corgi edit`.
    Select {
        pane_id: Option<String>,
    },
    /// The session whose transcript `State::transcript` carries.
    Watch {
        pane_id: Option<String>,
    },
    Prompt {
        pane_id: String,
        text: String,
    },
    /// Answers a numbered question by prompting the project's supervisor
    /// with `Q<n>: <text>`.
    Answer {
        project_root: String,
        question: String,
        text: String,
    },
    /// The new-agent form preset for `pane_id`'s project, else `project`,
    /// else the project used last, on `harness` and `model` when given.
    NewAgentForm {
        #[serde(default)]
        pane_id: Option<String>,
        #[serde(default)]
        project: Option<String>,
        #[serde(default)]
        harness: Option<String>,
        #[serde(default)]
        model: Option<String>,
    },
    /// Starts an agent from the filled-in form: a worker, or the project's
    /// supervisor when it has no agent yet.
    Spawn(SpawnRequest),
    /// Starts the project's supervisor, or names its pane when it runs.
    StartSupervisor {
        project_root: String,
    },
    MergeCheck {
        pane_id: String,
    },
    Merge {
        pane_id: String,
        #[serde(default)]
        upstream: UpstreamChoice,
    },
    /// Hands a conflicted merge or a failed push to the supervisor.
    MergeHandoff {
        pane_id: String,
    },
    /// Forgets the merge, leaving a conflict to resolve by hand.
    MergeDismiss {
        pane_id: String,
    },
    CloseCheck {
        pane_id: String,
    },
    Close {
        pane_id: String,
        #[serde(default)]
        force: bool,
    },
    Tag {
        pane_id: String,
        merge: bool,
    },
    /// Brings the agent's pane into view in Herdr: touches focus.
    Focus {
        pane_id: String,
    },
    /// Opens the agent's checkout in the editor in a new tab and goes to
    /// it: touches focus.
    Edit {
        pane_id: String,
    },
    Digest {
        project_root: String,
        #[serde(default)]
        lookup: DigestQuery,
    },
}

/// The new agent, as the form's rows hold it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SpawnRequest {
    pub project: String,
    /// Chosen from the project list as a new directory to create.
    pub new_project: bool,
    pub harness: String,
    pub model: String,
    pub effort: String,
    pub task: String,
    /// `worktree` or `directory`; empty is `worktree`.
    pub checkout: String,
}

/// What a merge does about the target branch's upstream.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UpstreamChoice {
    /// Fetch it and stop if it has commits the target lacks.
    #[default]
    Check,
    /// Bring those commits in first.
    Pull,
    /// Merge without it, after its fetch failed.
    Skip,
}

/// A lookup in the supervisor's memory, as `corgi digest` makes them.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DigestQuery {
    /// The whole digest.
    #[default]
    Summary,
    Search {
        words: String,
    },
    Decision {
        words: String,
    },
    Show {
        handle: String,
    },
    Ledger {
        id: String,
    },
    Brief {
        id: String,
    },
}

impl Command {
    /// Every command's name, and whether it touches Herdr's focus.
    pub fn catalog() -> Vec<CommandInfo> {
        const COMMANDS: &[(&str, bool)] = &[
            ("resync", false),
            ("refresh", false),
            ("select", false),
            ("watch", false),
            ("prompt", false),
            ("answer", false),
            ("new_agent_form", false),
            ("spawn", false),
            ("start_supervisor", false),
            ("merge_check", false),
            ("merge", false),
            ("merge_handoff", false),
            ("merge_dismiss", false),
            ("close_check", false),
            ("close", false),
            ("tag", false),
            ("focus", true),
            ("edit", true),
            ("digest", false),
        ];
        COMMANDS
            .iter()
            .map(|(name, touches_focus)| CommandInfo {
                name: (*name).to_string(),
                touches_focus: *touches_focus,
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn a_request_is_one_flat_object_with_its_command_by_name() {
        let line = r#"{"id":7,"command":"prompt","pane_id":"w1:p1","text":"go on"}"#;
        let request: Request = serde_json::from_str(line).expect("request");
        assert_eq!(
            request,
            Request {
                id: 7,
                command: Command::Prompt {
                    pane_id: "w1:p1".into(),
                    text: "go on".into()
                }
            }
        );
        assert_eq!(
            serde_json::to_value(&request).unwrap(),
            serde_json::from_str::<serde_json::Value>(line).unwrap()
        );
        let spawn: Request =
            serde_json::from_str(r#"{"id":1,"command":"spawn","project":"/r","task":"t"}"#)
                .expect("spawn with defaults");
        let Command::Spawn(spawn) = spawn.command else {
            panic!("not a spawn")
        };
        assert_eq!(
            (spawn.project.as_str(), spawn.checkout.as_str()),
            ("/r", "")
        );
        let digest: Request =
            serde_json::from_str(r#"{"id":2,"command":"digest","project_root":"/r"}"#).unwrap();
        assert_eq!(
            digest.command,
            Command::Digest {
                project_root: "/r".into(),
                lookup: DigestQuery::Summary
            }
        );
    }

    #[test]
    fn applying_changes_replaces_rows_by_key_and_follows_the_order() {
        let agent = |pane_id: &str, task: &str| Agent {
            pane_id: pane_id.into(),
            task: task.into(),
            ..Agent::default()
        };
        let mut state = State {
            agents: vec![agent("a", "one"), agent("b", "two")],
            ..State::default()
        };
        state.apply([
            Change::Agent(Box::new(agent("b", "changed"))),
            Change::Agent(Box::new(agent("c", "new"))),
            Change::AgentRemoved {
                pane_id: "a".into(),
            },
            Change::AgentOrder {
                pane_ids: vec!["c".into(), "b".into()],
            },
            Change::Status {
                status: Some("woke".into()),
            },
        ]);
        let tasks: Vec<_> = state
            .agents
            .iter()
            .map(|agent| (agent.pane_id.as_str(), agent.task.as_str()))
            .collect();
        assert_eq!(tasks, [("c", "new"), ("b", "changed")]);
        assert_eq!(state.status.as_deref(), Some("woke"));
    }

    #[test]
    fn every_command_is_in_the_catalog_and_only_focus_and_edit_touch_focus() {
        let catalog = Command::catalog();
        let touching: Vec<&str> = catalog
            .iter()
            .filter(|info| info.touches_focus)
            .map(|info| info.name.as_str())
            .collect();
        assert_eq!(touching, ["focus", "edit"]);
        // Each command's tag is a name in the catalog.
        let commands = [
            json!({"command": "resync"}),
            json!({"command": "refresh"}),
            json!({"command": "select", "pane_id": null}),
            json!({"command": "watch", "pane_id": "p"}),
            json!({"command": "prompt", "pane_id": "p", "text": "t"}),
            json!({"command": "answer", "project_root": "/r", "question": "Q1", "text": "A"}),
            json!({"command": "new_agent_form"}),
            json!({"command": "spawn"}),
            json!({"command": "start_supervisor", "project_root": "/r"}),
            json!({"command": "merge_check", "pane_id": "p"}),
            json!({"command": "merge", "pane_id": "p"}),
            json!({"command": "merge_handoff", "pane_id": "p"}),
            json!({"command": "merge_dismiss", "pane_id": "p"}),
            json!({"command": "close_check", "pane_id": "p"}),
            json!({"command": "close", "pane_id": "p"}),
            json!({"command": "tag", "pane_id": "p", "merge": true}),
            json!({"command": "focus", "pane_id": "p"}),
            json!({"command": "edit", "pane_id": "p"}),
            json!({"command": "digest", "project_root": "/r"}),
        ];
        assert_eq!(commands.len(), catalog.len());
        for (command, info) in commands.into_iter().zip(&catalog) {
            assert_eq!(command["command"], info.name.as_str());
            let mut request = command.clone();
            request["id"] = json!(1);
            serde_json::from_value::<Request>(request).expect("parses");
        }
    }
}
