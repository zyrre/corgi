//! The Steward: a project's long-lived coordinating agent.
//!
//! Corgi launches it as Claude Code or Codex in the root tab of the project's
//! workspace, with the role below in its instructions rather than its
//! conversation, so compaction does not lose it. Its memory is a directory of
//! plain files under Corgi's state directory, outside the repository. The
//! dashboard wakes it when one of its project's agents stops, and hands it
//! over to a fresh session once its context is half full, or once it has
//! sat idle nearly as long as its prompt cache lives.

use std::{
    env, fs,
    path::{Path, PathBuf},
    time::UNIX_EPOCH,
};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::{
    harness::Harness,
    model::{AgentInfo, AgentState},
    paths::{corgi_state_dir, dir_name},
    time::utc_stamp,
};

/// The role, with `{{corgi}}` and `{{state}}` standing for the Corgi binary
/// and the project's state directory.
const ROLE: &str = include_str!("../steward/ROLE.md");

/// Where one project's Steward keeps its decisions, briefs, and ledger.
pub fn state_dir(project_root: &str) -> Result<PathBuf> {
    Ok(corgi_state_dir()
        .context("neither XDG_STATE_HOME nor HOME is set")?
        .join("steward")
        .join(dir_name(project_root).unwrap_or(UNNAMED_PROJECT)))
}

/// What a project is called, in its state directory, its workspace label and
/// the Steward's name, when its root has no directory name of its own.
pub const UNNAMED_PROJECT: &str = "project";

/// A project's state directory, ready for a Steward to start in: the files
/// it reads exist, and its role names the commands of the Corgi binary that
/// launches it.
pub struct Prepared {
    pub state_dir: PathBuf,
    pub role_file: PathBuf,
    /// The role as written to `role_file`, for a harness that takes it inline.
    pub role: String,
}

/// Corgi's Herdr plugin ID, under which its installed build is found.
pub const PLUGIN_ID: &str = "io.github.zyrre.corgi";

/// Chooses a development build for Stewards' commands on purpose, instead of
/// the installed plugin.
const BIN_OVERRIDE_ENV: &str = "CORGI_STEWARD_BIN";

/// The Corgi binary a Steward's commands call: `CORGI_STEWARD_BIN` when it is
/// set, else the release build of the installed plugin (what the dashboard
/// runs), else the running binary. A worktree build therefore launches
/// Stewards that still use the installed Corgi unless told otherwise.
pub fn corgi_bin(installed_plugin_root: Option<&Path>) -> Result<PathBuf> {
    if let Some(path) = env::var_os(BIN_OVERRIDE_ENV).filter(|path| !path.is_empty()) {
        return Ok(PathBuf::from(path));
    }
    if let Some(bin) = installed_plugin_root
        .map(|root| root.join("target").join("release").join("corgi"))
        .filter(|bin| bin.is_file())
    {
        return Ok(bin);
    }
    env::current_exe().context("find the Corgi binary")
}

/// Creates the state directory and its files where missing, and writes the
/// role for this launch, naming `corgi` as the binary its commands call.
/// Existing decisions, briefs, and ledger are kept.
pub fn prepare(project_root: &str, corgi: &Path) -> Result<Prepared> {
    let state_dir = state_dir(project_root)?;
    for dir in ["briefs", HANDOVERS_DIR] {
        fs::create_dir_all(state_dir.join(dir))
            .with_context(|| format!("create {}", state_dir.display()))?;
    }
    let decisions = state_dir.join("decisions.md");
    if !decisions.exists() {
        fs::write(
            &decisions,
            format!(
                "# {} decisions\n",
                dir_name(project_root).unwrap_or(UNNAMED_PROJECT)
            ),
        )
        .with_context(|| format!("create {}", decisions.display()))?;
    }
    let ledger = state_dir.join("ledger.jsonl");
    if !ledger.exists() {
        fs::write(&ledger, "").with_context(|| format!("create {}", ledger.display()))?;
    }
    let role_file = state_dir.join("ROLE.md");
    let role = role(corgi, &state_dir);
    fs::write(&role_file, &role).with_context(|| format!("write {}", role_file.display()))?;
    Ok(Prepared {
        state_dir,
        role_file,
        role,
    })
}

/// How a Steward was launched, kept in its state directory so the Steward
/// that takes over from it starts the same way.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Launch {
    pub kind: String,
    #[serde(default)]
    pub model: String,
    #[serde(default)]
    pub effort: String,
    #[serde(default)]
    pub extra_args: Vec<String>,
}

const LAUNCH_FILE: &str = "launch.json";

/// Records how the Steward in `state_dir` was launched.
pub fn save_launch(state_dir: &Path, launch: &Launch) -> Result<()> {
    let file = state_dir.join(LAUNCH_FILE);
    fs::write(&file, serde_json::to_vec_pretty(launch)?)
        .with_context(|| format!("write {}", file.display()))
}

/// How the Steward in `state_dir` was last launched, if Corgi recorded it.
pub fn saved_launch(state_dir: &Path) -> Option<Launch> {
    fs::read(state_dir.join(LAUNCH_FILE))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Launch>(&bytes).ok())
}

/// How the Steward in `state_dir` was launched, if it was on `harness`. A
/// Steward launched before Corgi recorded this, or since moved to another
/// harness, starts on `harness` with its defaults.
pub fn launch_of(state_dir: &Path, harness: &Harness) -> Launch {
    saved_launch(state_dir)
        .filter(|launch| launch.kind == harness.kind())
        .unwrap_or_else(|| Launch {
            kind: harness.kind().to_string(),
            ..Launch::default()
        })
}

fn role(corgi: &Path, state_dir: &Path) -> String {
    ROLE.replace("{{corgi}}", &corgi.to_string_lossy())
        .replace("{{state}}", &state_dir.to_string_lossy())
}

/// The Steward's first prompt. With a task from the user, the Steward skips
/// its greeting and takes that task up; without one it greets with the state
/// of the project.
pub fn first_prompt(project_root: &str, state_dir: &Path, task: &str) -> String {
    let project = dir_name(project_root).unwrap_or(UNNAMED_PROJECT);
    let state = state_dir.display();
    let task = task.trim();
    if task.is_empty() {
        format!(
            "Start your Steward session for {project}. Project directory: {project_root}. \
             State directory: {state}. Follow the start-of-session steps in your role."
        )
    } else {
        format!(
            "Start your Steward session for {project}. Project directory: {project_root}. \
             State directory: {state}. Do the start-of-session reading, but skip the \
             greeting: the user started this project's session with the request below, \
             so take it up with them.\n\n{task}"
        )
    }
}

/// The first prompt of a Steward that takes over from a previous session:
/// the usual start-of-session reading, then the note that session left, which
/// it archives as `archive`. A short word to the user replaces the greeting.
pub fn takeover_prompt(project_root: &str, state_dir: &Path, archive: &Path) -> String {
    let project = dir_name(project_root).unwrap_or(UNNAMED_PROJECT);
    let state = state_dir.display();
    format!(
        "Start your Steward session for {project}. Project directory: {project_root}. \
         State directory: {state}. You take over from the previous Steward session. \
         Do the start-of-session reading, then read \
         {} and move it to {}, as your role's section on handing over says. Skip the \
         greeting: tell the user in at most three lines that you took over, and what \
         is open.",
        state_dir.join(HANDOVER_NOTE).display(),
        archive.display()
    )
}

/// How a wake message starts, so the Steward and the user can tell it from
/// a message the user typed.
const WAKE_PREFIX: &str = "[corgi]";

/// Context usage, in percent of the window, from which a Steward is handed
/// over to a fresh session.
const HANDOVER_PERCENT: u8 = 50;

/// Lowers the handover threshold for a debug run, such as a live test that
/// cannot fill half a context window. It is not a setting.
const HANDOVER_PERCENT_ENV: &str = "CORGI_DEBUG_HANDOVER_PERCENT";

/// The handover threshold: `CORGI_DEBUG_HANDOVER_PERCENT` when it holds a
/// percentage, else [`HANDOVER_PERCENT`].
pub fn handover_percent() -> u8 {
    percent_or_default(env::var(HANDOVER_PERCENT_ENV).ok().as_deref())
}

fn percent_or_default(value: Option<&str>) -> u8 {
    value
        .and_then(|value| value.trim().parse().ok())
        .filter(|percent| (1..=100).contains(percent))
        .unwrap_or(HANDOVER_PERCENT)
}

/// How long a Claude Code Steward rests before it is handed over: ten
/// minutes short of its one-hour prompt cache, so the request still finds
/// the cache warm.
const CLAUDE_IDLE_SECS: u64 = 50 * 60;

/// The same for Codex, whose cache holds for about thirty minutes.
const CODEX_IDLE_SECS: u64 = 25 * 60;

/// Shortens the idle time, in seconds, for a debug run on any harness. It is
/// not a setting.
const HANDOVER_IDLE_ENV: &str = "CORGI_DEBUG_HANDOVER_IDLE_SECS";

/// How long a Steward on `harness` rests before it is handed over:
/// `CORGI_DEBUG_HANDOVER_IDLE_SECS` when it holds a positive number of
/// seconds, else the harness's own. `None` for a harness whose context Corgi
/// cannot read.
pub fn handover_idle_secs(harness: &Harness) -> Option<u64> {
    idle_secs_or_default(harness, env::var(HANDOVER_IDLE_ENV).ok().as_deref())
}

fn idle_secs_or_default(harness: &Harness, value: Option<&str>) -> Option<u64> {
    let default = match harness {
        Harness::Claude => CLAUDE_IDLE_SECS,
        Harness::Codex => CODEX_IDLE_SECS,
        _ => return None,
    };
    Some(
        value
            .and_then(|value| value.trim().parse().ok())
            .filter(|secs| *secs > 0)
            .unwrap_or(default),
    )
}

/// How many times its baseline a resting Steward's context must be before
/// idleness hands it over. Below that a fresh session saves too little.
const IDLE_BASELINE_FACTOR: u64 = 2;

/// The note a Steward writes for the session that takes over from it, in
/// its state directory.
const HANDOVER_NOTE: &str = "handover.md";

/// Where the Steward that took over keeps the notes it read.
const HANDOVERS_DIR: &str = "handovers";

/// The pane token recording that a Steward session was asked for its
/// handover note, as `<unix seconds> <session>`, so a dashboard that takes
/// over waking does not ask the same session again.
pub const HANDOVER_TOKEN: &str = "corgi_handover";

/// The pane token recording a Steward session's baseline, the context it
/// had when its first turn ended, as `<tokens> <session>`, so a dashboard
/// that restarts or takes over waking measures growth from the same point.
pub const BASELINE_TOKEN: &str = "corgi_baseline";

/// The value of a `<value> <session>` pane token, if it is `session`'s.
fn session_token<T: std::str::FromStr>(token: Option<&str>, session: &str) -> Option<T> {
    token
        .and_then(|token| token.split_once(' '))
        .filter(|(_, owner)| *owner == session)
        .and_then(|(value, _)| value.parse().ok())
}

/// The pane token that marks a project's Steward: `session:<native session>`
/// once known, otherwise the launch name. Name values from older Corgi
/// versions remain valid until they can be upgraded to a session marker.
pub const CORGI_STEWARD_TOKEN: &str = "corgi_steward";

/// Native identity belongs to the live agent. A status-line `session` token
/// can survive pane reuse, so it must never establish Steward ownership.
pub fn native_session(info: &AgentInfo) -> Option<&str> {
    info.agent_session
        .as_ref()
        .map(|session| session.value.as_str())
        .filter(|session| !session.is_empty())
}

pub fn marker(name: &str, session: Option<&str>) -> String {
    match session.filter(|session| !session.is_empty()) {
        Some(session) => format!("session:{session}"),
        None => name.to_string(),
    }
}

/// Whether `info` is a project's Steward: the agent Corgi launched as one and
/// marked, wherever its pane has since moved. Sitting in the project's root
/// tab does not make an agent the Steward.
pub fn is_steward(info: &AgentInfo) -> bool {
    let Some(marker) = info.tokens.get(CORGI_STEWARD_TOKEN) else {
        return false;
    };
    if let Some(session) = marker.strip_prefix("session:") {
        return native_session(info) == Some(session);
    }
    info.name
        .as_deref()
        .is_some_and(|name| !name.is_empty() && marker == name)
}

/// The line that asks a Steward for its handover note, saying why.
pub fn handover_request(state_dir: &Path, trigger: Trigger) -> String {
    let note = state_dir.join(HANDOVER_NOTE);
    let why = match trigger {
        Trigger::Update => {
            return format!(
                "{WAKE_PREFIX} You worked after writing your handover note; bring {} up to \
                 date with what happened since you wrote it, then end your turn.",
                note.display()
            );
        }
        Trigger::Full(percent) => format!("Your context is past {percent}%"),
        Trigger::Idle(minutes) => format!(
            "You have been idle for {minutes} minutes and your prompt cache is about \
             to expire"
        ),
    };
    format!(
        "{WAKE_PREFIX} {why}, so a fresh Steward session takes over from you. Write {} \
         as your role's section on handing over says, then end your turn.",
        note.display()
    )
}

/// Whether `state_dir` holds a handover note written at or after `since`
/// (Unix seconds).
pub fn note_written_since(state_dir: &Path, since: u64) -> bool {
    fs::metadata(state_dir.join(HANDOVER_NOTE)).is_ok_and(|meta| {
        meta.len() > 0
            && meta
                .modified()
                .ok()
                .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
                .is_some_and(|age| age.as_secs() >= since)
    })
}

/// Where the Steward taking over at `now` (Unix seconds) archives the note.
pub fn handover_archive(state_dir: &Path, now: u64) -> PathBuf {
    state_dir
        .join(HANDOVERS_DIR)
        .join(format!("{}.md", utc_stamp(now)))
}

/// The line that tells a Steward where `worker` is and what to run next.
pub fn wake_message(corgi: &Path, worker: &str, state: AgentState) -> String {
    let next = if state == AgentState::Blocked {
        format!("herdr agent read {worker} --source recent --lines 120")
    } else {
        format!("{} report {worker}", corgi.display())
    };
    format!(
        "{WAKE_PREFIX} {worker} is {}. Run: {next}",
        state.label().to_lowercase()
    )
}

/// What a wake is about. Done and idle are one: the agent has stopped, and
/// a done agent becomes idle once someone looks at it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Rest {
    Stopped,
    Blocked,
}

/// One agent's states, as the dashboard sees them refresh by refresh,
/// reduced to the ones worth waking its Steward for. Nothing wakes until the
/// agent has been seen working, so neither the state it had when the
/// dashboard started nor a question on its way up (such as folder trust)
/// counts. After that: each time it rests after working, and each change
/// between blocked and stopped. The same resting state again, with a newer
/// state change, means it went through another state between two refreshes,
/// which counts as having worked.
#[derive(Debug, Default)]
pub struct Transitions {
    seen: Option<(AgentState, u64)>,
    armed: bool,
    worked: bool,
    last: Option<Rest>,
}

impl Transitions {
    /// The state to wake the Steward with, if `state` is news.
    pub fn observe(&mut self, state: AgentState, change: u64) -> Option<AgentState> {
        let previous = self.seen.replace((state, change));
        if previous == Some((state, change)) {
            return None;
        }
        let returned = previous.is_some_and(|(before, _)| before == state);
        let rest = match state {
            AgentState::Working => {
                self.armed = true;
                self.worked = true;
                return None;
            }
            AgentState::Unknown => return None,
            AgentState::Blocked => Rest::Blocked,
            AgentState::Done | AgentState::Idle => Rest::Stopped,
        };
        let wake = self.armed
            && (self.worked
                || returned
                || match rest {
                    Rest::Blocked => self.last != Some(Rest::Blocked),
                    Rest::Stopped => self.last == Some(Rest::Blocked),
                });
        self.worked = false;
        self.last = Some(rest);
        wake.then_some(state)
    }
}

/// What the dashboard sees of a Steward at one refresh, for its handover.
#[derive(Debug, Clone, Copy)]
pub struct Sighting<'a> {
    /// The harness's session identity: a handover is once per session.
    pub session: &'a str,
    pub state: AgentState,
    pub change: u64,
    pub context_percent: Option<u8>,
    /// The tokens of its context, when its session file records them.
    pub context_tokens: Option<u64>,
    /// Whether another agent of its project is working or blocked. The
    /// dashboard cannot tell which of them the Steward dispatched, so any
    /// of them counts as one of its workers.
    pub workers_busy: bool,
    /// The session's [`HANDOVER_TOKEN`], if its pane carries one.
    pub token: Option<&'a str>,
    /// The session's [`BASELINE_TOKEN`], if its pane carries one.
    pub baseline_token: Option<&'a str>,
}

/// When the dashboard hands a Steward over, for one refresh.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Thresholds {
    /// Context usage, in percent of the window.
    pub percent: u8,
    /// How long it has rested, in seconds, if its harness has an idle time.
    pub idle_secs: Option<u64>,
}

/// Why a Steward is asked for its handover note.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Trigger {
    /// Its context is past this percentage.
    Full(u8),
    /// It has rested this many minutes, with its context at least
    /// [`IDLE_BASELINE_FACTOR`] times its baseline and no worker busy.
    Idle(u64),
    /// It worked after writing its note, which no longer covers that turn.
    Update,
}

/// What to do about a Steward's handover at this refresh.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HandoverStep {
    /// Ask it for its note, recording the request at the given time.
    Ask(u64, Trigger),
    /// It wrote its note: replace it with a fresh session.
    Replace,
    /// Its turn ended without a note. It stays, and is asked again only after
    /// a later turn.
    NoNote,
}

/// Where one project's Steward is in handing over, refresh by refresh.
///
/// A session is asked once it is idle or done (never working, and never
/// blocked on a question) with its context at the threshold, or once it has
/// rested for its idle time with no worker busy and its context at least
/// twice its baseline. The baseline is its context at the first refresh that
/// finds it resting with a context: the end of its first turn when the
/// dashboard saw that, else the first rest this dashboard saw, which only
/// asks for more growth. It is kept on the pane as [`BASELINE_TOKEN`]; the
/// rest itself is timed from the first refresh that saw it, so a dashboard
/// that restarts or takes over waking starts that clock again. Its answer is
/// the turn that follows: when that turn ends, a note written since the
/// request means the session is replaced, and no note means it stays. It is
/// asked again only after a later turn of its own, so a Steward that keeps
/// failing to write the note is not asked at every refresh. A request the
/// Steward never takes up (its state does not change within
/// [`HANDOVER_ANSWER_SECS`]) counts as no note.
#[derive(Debug, Default)]
pub struct Handover {
    session: String,
    phase: Phase,
    /// The session's baseline context, in tokens.
    baseline: Option<u64>,
    /// Since when (Unix seconds) the session rests, and its state change
    /// then.
    resting: Option<(u64, u64)>,
}

/// How long a Steward has to start the turn a handover request asks for.
const HANDOVER_ANSWER_SECS: u64 = 120;

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
enum Phase {
    #[default]
    Watching,
    /// The session was asked at `at`, when its state change was `change`.
    /// `turned` once it has been seen at work since.
    Asked {
        at: u64,
        change: Option<u64>,
        turned: bool,
    },
    /// Its note was found when its state change was `change`, and the
    /// replacement waits for its input box to hold no draft. No timeout
    /// applies: a draft only delays it. `worked` once it has been seen at
    /// work since, which makes the note stale.
    Noted { change: u64, worked: bool },
    /// Its note was found and the replacement runs.
    Replacing,
    /// No note came when its state change was `change`.
    Failed { change: u64, turned: bool },
}

impl Handover {
    /// The next step for the Steward as `seen` at `now` (Unix seconds), with
    /// `limits` as the thresholds. `note_since(at)` says whether the note was
    /// written since the request made at `at`, and `may_type()`, asked only
    /// when a request or the replacement is due, whether Corgi may type into
    /// the Steward's pane now: one that cannot stays due for a later refresh.
    pub fn observe(
        &mut self,
        seen: Sighting,
        limits: Thresholds,
        now: u64,
        note_since: impl FnOnce(u64) -> bool,
        may_type: impl FnOnce() -> bool,
    ) -> Option<HandoverStep> {
        if self.phase == Phase::Replacing {
            return None;
        }
        if self.session != seen.session {
            self.session = seen.session.to_string();
            // A request this session already had, from a dashboard that woke
            // Stewards before this one, is still the one being answered.
            self.phase = session_token(seen.token, seen.session).map_or(Phase::Watching, |at| {
                Phase::Asked {
                    at,
                    change: None,
                    turned: true,
                }
            });
            self.baseline = session_token(seen.baseline_token, seen.session);
            self.resting = None;
        }
        let resting = matches!(seen.state, AgentState::Idle | AgentState::Done);
        let busy = matches!(seen.state, AgentState::Working | AgentState::Blocked);
        if resting {
            if self.baseline.is_none() {
                self.baseline = seen.context_tokens.filter(|tokens| *tokens > 0);
            }
            // Done turning idle is the same rest; two changes are a turn.
            if self
                .resting
                .is_none_or(|(_, change)| seen.change >= change + 2)
            {
                self.resting = Some((now, seen.change));
            }
        } else if busy {
            self.resting = None;
        }
        let trigger = self.trigger(seen, limits, now);
        match &mut self.phase {
            Phase::Watching => trigger
                .filter(|_| may_type())
                .map(|trigger| self.ask(seen, now, trigger)),
            Phase::Failed { change, turned } => {
                *turned |= busy || seen.change >= *change + 2;
                let turned = *turned;
                trigger
                    .filter(|_| turned && may_type())
                    .map(|trigger| self.ask(seen, now, trigger))
            }
            Phase::Asked { at, change, turned } => {
                *turned |= busy || change.is_some_and(|change| seen.change >= change + 2);
                let unanswered = !*turned && now >= *at + HANDOVER_ANSWER_SECS;
                if !(resting && *turned || unanswered) {
                    return None;
                }
                if *turned && note_since(*at) {
                    self.phase = Phase::Noted {
                        change: seen.change,
                        worked: false,
                    };
                    self.replace_unless_drafting(may_type)
                } else {
                    self.fail(seen.change);
                    Some(HandoverStep::NoNote)
                }
            }
            Phase::Noted { change, worked } => {
                *worked |= busy || seen.change >= *change + 2;
                // A turn of the user's, sent from the draft, runs first, and
                // then the note is brought up to date before the replacement.
                if !resting {
                    None
                } else if *worked {
                    may_type().then(|| self.ask(seen, now, Trigger::Update))
                } else {
                    self.replace_unless_drafting(may_type)
                }
            }
            Phase::Replacing => None,
        }
    }

    /// Why the Steward as `seen` at `now` is due for a handover, if it is:
    /// only ever while it rests.
    fn trigger(&self, seen: Sighting, limits: Thresholds, now: u64) -> Option<Trigger> {
        let (since, _) = self.resting?;
        if seen
            .context_percent
            .is_some_and(|used| used >= limits.percent)
        {
            return Some(Trigger::Full(limits.percent));
        }
        let rested = now.saturating_sub(since);
        let grown = self
            .baseline
            .zip(seen.context_tokens)
            .is_some_and(|(baseline, tokens)| tokens >= baseline * IDLE_BASELINE_FACTOR);
        (limits.idle_secs.is_some_and(|idle| rested >= idle) && grown && !seen.workers_busy)
            .then_some(Trigger::Idle(rested / 60))
    }

    /// Starts the replacement of a Steward whose note was found, unless
    /// `may_type()` says its input box holds a draft.
    fn replace_unless_drafting(&mut self, may_type: impl FnOnce() -> bool) -> Option<HandoverStep> {
        may_type().then(|| {
            self.phase = Phase::Replacing;
            HandoverStep::Replace
        })
    }

    fn ask(&mut self, seen: Sighting, now: u64, trigger: Trigger) -> HandoverStep {
        self.phase = Phase::Asked {
            at: now,
            change: Some(seen.change),
            turned: false,
        };
        HandoverStep::Ask(now, trigger)
    }

    /// The session's baseline context in tokens, once one is recorded, for
    /// the dashboard to keep on its pane.
    pub fn baseline(&self) -> Option<u64> {
        self.baseline
    }

    fn fail(&mut self, change: u64) {
        self.phase = Phase::Failed {
            change,
            turned: false,
        };
    }

    /// The request could not be sent: the Steward is asked again after a
    /// later turn.
    pub fn unsent(&mut self, change: u64) {
        self.fail(change);
    }

    /// The replacement is over. After a failed one the old session, if it
    /// still runs, is asked again after a later turn.
    pub fn replaced(&mut self, succeeded: bool, change: u64) {
        if succeeded {
            *self = Self::default();
        } else {
            self.fail(change);
        }
    }

    /// Whether the Steward is between its request and its successor's first
    /// prompt, when wakes wait for the Steward that takes over.
    pub fn in_progress(&self) -> bool {
        matches!(
            self.phase,
            Phase::Asked { .. } | Phase::Noted { .. } | Phase::Replacing
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use AgentState::{Blocked, Done, Idle, Working};

    #[test]
    fn steward_identity_survives_renaming_but_not_pane_reuse() {
        let mut agent = AgentInfo {
            name: Some("steward-m-ta-sverige".into()),
            agent_session: Some(crate::model::AgentSession {
                value: "original-session".into(),
                ..Default::default()
            }),
            tokens: [(
                CORGI_STEWARD_TOKEN.into(),
                marker("", Some("original-session")),
            )]
            .into(),
            ..Default::default()
        };
        assert!(is_steward(&agent));
        agent.name = Some("start-your-steward-session-for-m".into());
        assert!(is_steward(&agent));
        // Neither an old bridge token nor the old name can override native identity.
        agent
            .tokens
            .insert("session".into(), "original-session".into());
        agent.agent_session.as_mut().unwrap().value = "replacement-session".into();
        assert!(!is_steward(&agent));
        agent.name = Some("steward-m-ta-sverige".into());
        assert!(!is_steward(&agent));
        agent.agent_session = None;
        assert!(!is_steward(&agent));
        agent.agent_session = Some(Default::default());
        assert!(!is_steward(&agent));
        agent
            .tokens
            .insert(CORGI_STEWARD_TOKEN.into(), "session:".into());
        assert!(!is_steward(&agent));
    }

    #[test]
    fn legacy_and_pre_prompt_markers_require_the_launch_name() {
        let mut agent = AgentInfo {
            name: Some("steward-corgi".into()),
            tokens: [(CORGI_STEWARD_TOKEN.into(), marker("steward-corgi", None))].into(),
            ..Default::default()
        };
        assert!(is_steward(&agent));
        agent.agent_session = Some(crate::model::AgentSession {
            value: "s1".into(),
            ..Default::default()
        });
        assert!(is_steward(&agent));
        agent.name = Some("worker".into());
        assert!(!is_steward(&agent));
        agent.name = None;
        assert!(!is_steward(&agent));
    }

    fn wakes(steps: &[(AgentState, u64)]) -> Vec<Option<AgentState>> {
        let mut transitions = Transitions::default();
        steps
            .iter()
            .map(|&(state, change)| transitions.observe(state, change))
            .collect()
    }

    #[test]
    fn an_agent_wakes_its_steward_each_time_it_stops_after_working() {
        assert_eq!(
            wakes(&[
                (Idle, 1),
                (Working, 2),
                (Done, 3),
                (Done, 3),
                (Idle, 4),
                (Working, 5),
                (Idle, 6),
            ]),
            [None, None, Some(Done), None, None, None, Some(Idle)]
        );
    }

    #[test]
    fn blocked_and_its_answer_each_wake_once() {
        assert_eq!(
            wakes(&[
                (Working, 1),
                (Blocked, 2),
                (Blocked, 2),
                (Working, 3),
                (Blocked, 4),
                (Idle, 5),
            ]),
            [None, Some(Blocked), None, None, Some(Blocked), Some(Idle)]
        );
    }

    #[test]
    fn a_turn_between_two_refreshes_still_wakes() {
        // Done, then a steered turn that ended before the next refresh.
        assert_eq!(
            wakes(&[(Working, 1), (Done, 2), (Done, 9)]),
            [None, Some(Done), Some(Done)]
        );
    }

    #[test]
    fn nothing_before_the_first_work_wakes() {
        // What an agent was doing when the dashboard started is not news.
        assert_eq!(wakes(&[(Done, 4)]), [None]);
        assert_eq!(wakes(&[(Blocked, 4), (Idle, 5)]), [None, None]);
        // Nor is a new agent's startup question, answered before its task.
        assert_eq!(
            wakes(&[(Blocked, 1), (Idle, 2), (Working, 3), (Done, 4)]),
            [None, None, None, Some(Done)]
        );
        // An agent the dashboard first sees at work is armed at once.
        assert_eq!(wakes(&[(Working, 7), (Done, 8)]), [None, Some(Done)]);
    }

    #[test]
    fn wake_messages_name_the_agent_its_state_and_the_next_command() {
        let corgi = Path::new("/opt/corgi/corgi");
        assert_eq!(
            wake_message(corgi, "w-x", Done),
            "[corgi] w-x is done. Run: /opt/corgi/corgi report w-x"
        );
        assert_eq!(
            wake_message(corgi, "w-x", Idle),
            "[corgi] w-x is idle. Run: /opt/corgi/corgi report w-x"
        );
        assert_eq!(
            wake_message(corgi, "w-x", Blocked),
            "[corgi] w-x is blocked. Run: herdr agent read w-x --source recent --lines 120"
        );
    }

    #[test]
    fn the_role_names_this_binary_and_this_projects_state() {
        let role = role(
            Path::new("/opt/corgi/corgi"),
            Path::new("/state/steward/corgi"),
        );
        assert!(role.contains("`/opt/corgi/corgi spawn"));
        assert!(role.contains("`/opt/corgi/corgi fleet`"));
        assert!(role.contains("`/state/steward/corgi`"));
        assert!(!role.contains("{{"));
    }

    #[test]
    fn the_role_fits_every_harness_and_leaves_waking_to_corgi() {
        for claude_only in ["run_in_background", "Bash", "Write or Edit"] {
            assert!(!ROLE.contains(claude_only), "{claude_only}");
        }
        assert!(!ROLE.contains("herdr agent wait"));
        let role = role(Path::new("/opt/corgi/corgi"), Path::new("/state"));
        assert!(role.contains(&format!(
            "{} w-fleet-json is done. Run: /opt/corgi/corgi report w-fleet-json",
            WAKE_PREFIX
        )));
    }

    #[test]
    fn stewards_call_the_installed_plugin_build_when_there_is_one() {
        if env::var_os(BIN_OVERRIDE_ENV).is_some() {
            return;
        }
        let root = env::temp_dir().join(format!("corgi-plugin-root-{}", std::process::id()));
        let bin = root.join("target").join("release").join("corgi");
        fs::create_dir_all(bin.parent().expect("release dir")).expect("create release dir");
        fs::write(&bin, "").expect("create installed binary");
        assert_eq!(corgi_bin(Some(&root)).expect("installed build"), bin);
        fs::remove_dir_all(&root).ok();
        // Without an installed build, the running binary is the fallback.
        assert_eq!(
            corgi_bin(Some(&root)).expect("fallback"),
            env::current_exe().expect("current exe")
        );
        assert_eq!(
            corgi_bin(None).expect("fallback"),
            env::current_exe().expect("current exe")
        );
    }

    #[test]
    fn a_first_task_replaces_the_greeting() {
        let state = Path::new("/state/steward/weather");
        let greeting = first_prompt("/repos/weather", state, "  ");
        assert!(greeting.contains("Follow the start-of-session steps"));
        let task = first_prompt("/repos/weather", state, "Build a forecast CLI\nin Rust");
        assert!(task.contains("skip the greeting"));
        assert!(task.ends_with("\n\nBuild a forecast CLI\nin Rust"));
        assert!(task.contains("Project directory: /repos/weather."));
    }

    /// The 50% threshold alone, as before idleness counted.
    const FULL: Thresholds = Thresholds {
        percent: 50,
        idle_secs: None,
    };

    fn seen(state: AgentState, change: u64, percent: u8) -> Sighting<'static> {
        Sighting {
            session: "s1",
            state,
            change,
            context_percent: Some(percent),
            context_tokens: None,
            workers_busy: false,
            token: None,
            baseline_token: None,
        }
    }

    /// The steps of one Steward session's handover, refresh by refresh, a
    /// second apart, with the note written when `note` says so.
    fn handover_steps(steps: &[(AgentState, u64, u8)], note: bool) -> Vec<Option<HandoverStep>> {
        let mut handover = Handover::default();
        steps
            .iter()
            .enumerate()
            .map(|(second, &(state, change, percent))| {
                handover.observe(
                    seen(state, change, percent),
                    FULL,
                    1_000 + second as u64,
                    |_| note,
                    || true,
                )
            })
            .collect()
    }

    #[test]
    fn the_handover_threshold_is_half_the_window_unless_a_debug_run_lowers_it() {
        assert_eq!(HANDOVER_PERCENT, 50);
        assert_eq!(percent_or_default(None), 50);
        assert_eq!(percent_or_default(Some(" 3 ")), 3);
        for invalid in ["0", "101", "half", ""] {
            assert_eq!(percent_or_default(Some(invalid)), 50, "{invalid}");
        }
        assert_eq!(
            handover_steps(&[(Idle, 1, 49), (Done, 2, 49), (Idle, 3, 50)], true),
            [
                None,
                None,
                Some(HandoverStep::Ask(1_002, Trigger::Full(50)))
            ]
        );
        // No usage read yet is not a full context.
        let mut handover = Handover::default();
        let unknown = Sighting {
            context_percent: None,
            ..seen(Idle, 1, 0)
        };
        assert_eq!(handover.observe(unknown, FULL, 1, |_| true, || true), None);
    }

    #[test]
    fn a_steward_is_asked_once_and_replaced_when_its_turn_ends_with_a_note() {
        assert_eq!(
            handover_steps(
                &[
                    (Idle, 1, 60),
                    // The request is still on its way in.
                    (Idle, 1, 60),
                    (Working, 2, 60),
                    (Working, 2, 61),
                    (Done, 3, 62),
                    // Replacing: nothing more, whatever the refreshes show.
                    (Done, 3, 62),
                    (Idle, 4, 62),
                ],
                true
            ),
            [
                Some(HandoverStep::Ask(1_000, Trigger::Full(50))),
                None,
                None,
                None,
                Some(HandoverStep::Replace),
                None,
                None,
            ]
        );
        // A turn that passed between two refreshes ended too.
        assert_eq!(
            handover_steps(&[(Idle, 1, 60), (Done, 3, 60)], true),
            [
                Some(HandoverStep::Ask(1_000, Trigger::Full(50))),
                Some(HandoverStep::Replace)
            ]
        );
    }

    #[test]
    fn a_steward_blocked_on_a_question_or_at_work_is_not_asked() {
        assert_eq!(
            handover_steps(
                &[(Blocked, 1, 80), (Working, 2, 80), (Blocked, 3, 80)],
                true
            ),
            [None, None, None]
        );
        // Blocked during the handover turn is not its end.
        assert_eq!(
            handover_steps(
                &[
                    (Idle, 1, 80),
                    (Working, 2, 80),
                    (Blocked, 3, 80),
                    (Idle, 4, 80)
                ],
                true
            ),
            [
                Some(HandoverStep::Ask(1_000, Trigger::Full(50))),
                None,
                None,
                Some(HandoverStep::Replace)
            ]
        );
    }

    #[test]
    fn without_a_note_the_steward_stays_and_is_asked_again_only_after_a_later_turn() {
        assert_eq!(
            handover_steps(
                &[
                    (Idle, 1, 60),
                    (Working, 2, 60),
                    (Done, 3, 60),
                    // The same idle period: not again.
                    (Done, 3, 60),
                    (Idle, 4, 60),
                    // A later turn, then its idle period.
                    (Working, 5, 61),
                    (Done, 6, 61),
                ],
                false
            ),
            [
                Some(HandoverStep::Ask(1_000, Trigger::Full(50))),
                None,
                Some(HandoverStep::NoNote),
                None,
                None,
                None,
                Some(HandoverStep::Ask(1_006, Trigger::Full(50))),
            ]
        );
    }

    #[test]
    fn a_request_that_cannot_go_out_stays_due_for_a_later_refresh() {
        let mut handover = Handover::default();
        let idle = seen(Idle, 1, 60);
        let asked = &std::cell::Cell::new(0);
        let may_type = |ok| {
            move || {
                asked.set(asked.get() + 1);
                ok
            }
        };
        // Not due yet: the check is not made.
        assert_eq!(
            handover.observe(seen(Idle, 1, 40), FULL, 9, |_| true, may_type(true)),
            None
        );
        assert_eq!(asked.get(), 0);
        assert_eq!(
            handover.observe(idle, FULL, 10, |_| true, may_type(false)),
            None
        );
        assert!(!handover.in_progress());
        assert_eq!(
            handover.observe(idle, FULL, 11, |_| true, may_type(true)),
            Some(HandoverStep::Ask(11, Trigger::Full(50)))
        );
        assert_eq!(asked.get(), 2);
    }

    #[test]
    fn a_replacement_waits_for_the_draft_however_long_it_takes() {
        let mut handover = Handover::default();
        let mut step = |seen, now, drafting: bool| {
            let step = handover.observe(seen, FULL, now, |_| true, || !drafting);
            (step, handover.in_progress())
        };
        assert_eq!(
            step(seen(Idle, 1, 60), 10, false),
            (Some(HandoverStep::Ask(10, Trigger::Full(50))), true)
        );
        assert_eq!(step(seen(Working, 2, 60), 11, false), (None, true));
        // The note is written, but the user is writing too: nothing is typed,
        // however long past the request, and the handover stays under way.
        assert_eq!(step(seen(Done, 3, 60), 12, true), (None, true));
        let late = 12 + HANDOVER_ANSWER_SECS * 10;
        assert_eq!(step(seen(Idle, 4, 60), late, true), (None, true));
        // The box empties without a turn: the replacement goes ahead.
        assert_eq!(
            step(seen(Idle, 4, 60), late + 1, false),
            (Some(HandoverStep::Replace), true)
        );
        assert_eq!(step(seen(Idle, 4, 60), late + 2, false), (None, true));
    }

    #[test]
    fn a_turn_after_the_note_asks_for_an_update_before_the_replacement() {
        let mut handover = Handover::default();
        // When the note was last written, if at all.
        let written = std::cell::Cell::new(None::<u64>);
        let mut step = |seen, now, drafting: bool| {
            handover.observe(
                seen,
                FULL,
                now,
                |at| written.get().is_some_and(|written| written >= at),
                || !drafting,
            )
        };
        assert_eq!(
            step(seen(Idle, 1, 60), 10, false),
            Some(HandoverStep::Ask(10, Trigger::Full(50)))
        );
        assert_eq!(step(seen(Working, 2, 60), 11, false), None);
        written.set(Some(12));
        assert_eq!(step(seen(Done, 3, 60), 13, true), None);
        // The user sends the draft; once that turn is over, the note, older
        // than it, is asked for again, not replaced from, and not while a
        // new draft waits.
        assert_eq!(step(seen(Working, 4, 61), 20, false), None);
        assert_eq!(step(seen(Done, 5, 61), 30, true), None);
        assert_eq!(
            step(seen(Done, 5, 61), 31, false),
            Some(HandoverStep::Ask(31, Trigger::Update))
        );
        assert_eq!(step(seen(Working, 6, 62), 32, false), None);
        // The note from before the request does not count; the updated one
        // does, and there is one replacement.
        assert_eq!(step(seen(Working, 6, 62), 33, false), None);
        written.set(Some(34));
        assert_eq!(
            step(seen(Done, 7, 62), 35, false),
            Some(HandoverStep::Replace)
        );
        assert_eq!(step(seen(Idle, 8, 62), 36, false), None);
    }

    #[test]
    fn a_note_not_brought_up_to_date_leaves_the_steward_in_place() {
        let mut handover = Handover::default();
        // The note is written at 12 and never again.
        let mut step = |seen, now, drafting: bool| {
            let step = handover.observe(seen, FULL, now, |at| 12 >= at, || !drafting);
            (step, handover.in_progress())
        };
        assert_eq!(
            step(seen(Idle, 1, 60), 10, false),
            (Some(HandoverStep::Ask(10, Trigger::Full(50))), true)
        );
        step(seen(Working, 2, 60), 11, false);
        assert_eq!(step(seen(Done, 3, 60), 13, true), (None, true));
        // A turn passed between two refreshes: the note is stale.
        assert_eq!(
            step(seen(Done, 5, 61), 30, false),
            (Some(HandoverStep::Ask(30, Trigger::Update)), true)
        );
        step(seen(Working, 6, 61), 31, false);
        assert_eq!(
            step(seen(Done, 7, 61), 40, false),
            (Some(HandoverStep::NoNote), false)
        );
        // It is asked afresh only after a later turn of its own.
        assert_eq!(step(seen(Idle, 8, 61), 41, false), (None, false));
        step(seen(Working, 9, 62), 50, false);
        assert_eq!(
            step(seen(Done, 10, 62), 60, false),
            (Some(HandoverStep::Ask(60, Trigger::Full(50))), true)
        );
    }

    #[test]
    fn a_request_the_steward_never_takes_up_counts_as_no_note() {
        let mut handover = Handover::default();
        let idle = seen(Idle, 1, 60);
        assert_eq!(
            handover.observe(idle, FULL, 10, |_| true, || true),
            Some(HandoverStep::Ask(10, Trigger::Full(50)))
        );
        assert_eq!(
            handover.observe(idle, FULL, 10 + HANDOVER_ANSWER_SECS - 1, |_| true, || true),
            None
        );
        assert_eq!(
            handover.observe(idle, FULL, 10 + HANDOVER_ANSWER_SECS, |_| true, || true),
            Some(HandoverStep::NoNote)
        );
    }

    #[test]
    fn a_dashboard_taking_over_waking_does_not_ask_the_same_session_again() {
        // The first dashboard asked at 900 and went away during the turn.
        let token = "900 s1";
        let mut handover = Handover::default();
        let working = Sighting {
            token: Some(token),
            ..seen(Working, 7, 70)
        };
        assert_eq!(
            handover.observe(working, FULL, 1_000, |_| true, || true),
            None
        );
        let mut notes = Vec::new();
        let done = Sighting {
            token: Some(token),
            ..seen(Done, 8, 70)
        };
        assert_eq!(
            handover.observe(
                done,
                FULL,
                1_001,
                |at| {
                    notes.push(at);
                    true
                },
                || true
            ),
            Some(HandoverStep::Replace)
        );
        // The note is judged against the first request.
        assert_eq!(notes, [900]);
        // Another session's token is no request of this one's.
        let mut handover = Handover::default();
        let fresh = Sighting {
            session: "s2",
            token: Some(token),
            ..seen(Idle, 1, 70)
        };
        assert_eq!(
            handover.observe(fresh, FULL, 1_002, |_| true, || true),
            Some(HandoverStep::Ask(1_002, Trigger::Full(50)))
        );
    }

    #[test]
    fn a_new_session_starts_a_new_handover_and_a_failed_replacement_waits_for_a_turn() {
        let mut handover = Handover::default();
        assert_eq!(
            handover.observe(seen(Idle, 1, 60), FULL, 1, |_| true, || true),
            Some(HandoverStep::Ask(1, Trigger::Full(50)))
        );
        assert!(handover.in_progress());
        assert_eq!(
            handover.observe(seen(Done, 3, 60), FULL, 2, |_| true, || true),
            Some(HandoverStep::Replace)
        );
        assert!(handover.in_progress());
        handover.replaced(false, 3);
        assert!(!handover.in_progress());
        assert_eq!(
            handover.observe(seen(Idle, 4, 60), FULL, 3, |_| true, || true),
            None
        );
        handover.replaced(true, 0);
        let successor = Sighting {
            session: "s2",
            ..seen(Idle, 1, 60)
        };
        assert_eq!(
            handover.observe(successor, FULL, 4, |_| true, || true),
            Some(HandoverStep::Ask(4, Trigger::Full(50)))
        );
    }

    const CLAUDE: Thresholds = Thresholds {
        percent: 50,
        idle_secs: Some(CLAUDE_IDLE_SECS),
    };

    /// A Steward whose first turn ended at 30,000 tokens, at 10% of its
    /// window, now at `tokens`.
    fn idle(state: AgentState, change: u64, tokens: u64) -> Sighting<'static> {
        Sighting {
            context_percent: Some(10),
            context_tokens: Some(tokens),
            baseline_token: Some("30000 s1"),
            ..seen(state, change, 10)
        }
    }

    /// The steps for `sightings`, each at its second, on Claude Code.
    fn idle_steps(sightings: &[(u64, Sighting)]) -> Vec<Option<HandoverStep>> {
        let mut handover = Handover::default();
        sightings
            .iter()
            .map(|&(at, seen)| handover.observe(seen, CLAUDE, at, |_| true, || true))
            .collect()
    }

    const ASK_IDLE: Option<HandoverStep> =
        Some(HandoverStep::Ask(CLAUDE_IDLE_SECS, Trigger::Idle(50)));

    #[test]
    fn a_grown_steward_resting_past_its_idle_time_with_no_busy_worker_is_asked_once() {
        let last = CLAUDE_IDLE_SECS;
        assert_eq!(
            idle_steps(&[
                (0, idle(Done, 4, 60_000)),
                // Seen, so done turns idle: the same rest.
                (600, idle(Idle, 5, 60_000)),
                (last - 1, idle(Idle, 5, 60_000)),
                (last, idle(Idle, 5, 60_000)),
                (last + 1, idle(Idle, 5, 60_000)),
            ]),
            [None, None, None, ASK_IDLE, None]
        );
        let request = handover_request(Path::new("/state"), Trigger::Idle(50));
        assert!(request.starts_with(
            "[corgi] You have been idle for 50 minutes and your prompt cache is about to \
             expire, so a fresh Steward session takes over from you. Write /state/handover.md"
        ));
    }

    #[test]
    fn idleness_hands_over_only_a_context_twice_its_baseline() {
        let last = CLAUDE_IDLE_SECS;
        assert_eq!(
            idle_steps(&[(0, idle(Idle, 4, 59_999)), (last, idle(Idle, 4, 59_999))]),
            [None, None]
        );
        // A fresh session, still near the context its first turn ended with,
        // is never asked, however long it rests.
        let fresh = |tokens| Sighting {
            session: "s2",
            baseline_token: None,
            ..idle(Idle, 2, tokens)
        };
        assert_eq!(
            idle_steps(&[
                (0, fresh(31_000)),
                (last, fresh(31_000)),
                (10 * last, fresh(32_000)),
            ]),
            [None, None, None]
        );
        // Nor is a session whose context is unknown.
        let unknown = Sighting {
            context_tokens: None,
            ..idle(Idle, 4, 0)
        };
        assert_eq!(idle_steps(&[(0, unknown), (last, unknown)]), [None, None]);
    }

    #[test]
    fn idleness_waits_for_busy_workers_and_for_the_steward_itself() {
        let last = CLAUDE_IDLE_SECS;
        let waiting = Sighting {
            workers_busy: true,
            ..idle(Idle, 4, 60_000)
        };
        assert_eq!(
            idle_steps(&[
                (0, waiting),
                (last, waiting),
                (last + 5, idle(Idle, 4, 60_000))
            ]),
            [
                None,
                None,
                Some(HandoverStep::Ask(last + 5, Trigger::Idle(50)))
            ]
        );
        // Working or blocked is no rest, and the clock starts again after.
        for busy in [Working, Blocked] {
            assert_eq!(
                idle_steps(&[
                    (0, idle(Idle, 4, 60_000)),
                    (600, idle(busy, 5, 60_000)),
                    (last, idle(busy, 5, 60_000)),
                    (last + 1, idle(Done, 6, 61_000)),
                    (last + 600 + 1, idle(Done, 6, 61_000)),
                    (600 + 2 * last, idle(Idle, 7, 61_000)),
                ]),
                [
                    None,
                    None,
                    None,
                    None,
                    None,
                    Some(HandoverStep::Ask(600 + 2 * last, Trigger::Idle(59)))
                ],
                "{busy:?}"
            );
        }
        // A turn passed between two refreshes restarts it too.
        assert_eq!(
            idle_steps(&[(0, idle(Idle, 4, 60_000)), (last, idle(Done, 6, 60_000))]),
            [None, None]
        );
    }

    #[test]
    fn the_full_context_threshold_goes_first_and_is_unchanged_by_idleness() {
        let full = Sighting {
            context_percent: Some(55),
            workers_busy: true,
            ..idle(Idle, 4, 31_000)
        };
        assert_eq!(
            idle_steps(&[(7, full)]),
            [Some(HandoverStep::Ask(7, Trigger::Full(50)))]
        );
    }

    #[test]
    fn a_codex_steward_rests_half_as_long_and_a_debug_run_shortens_both() {
        assert_eq!(CLAUDE_IDLE_SECS, 50 * 60);
        assert_eq!(idle_secs_or_default(&Harness::Claude, None), Some(3_000));
        assert_eq!(idle_secs_or_default(&Harness::Codex, None), Some(1_500));
        assert_eq!(idle_secs_or_default(&Harness::Gemini, Some("60")), None);
        assert_eq!(
            idle_secs_or_default(&Harness::Codex, Some(" 60 ")),
            Some(60)
        );
        assert_eq!(idle_secs_or_default(&Harness::Claude, Some("60")), Some(60));
        for invalid in ["0", "-5", "soon", ""] {
            assert_eq!(
                idle_secs_or_default(&Harness::Claude, Some(invalid)),
                Some(3_000),
                "{invalid}"
            );
        }
        let codex = Thresholds {
            percent: 50,
            idle_secs: idle_secs_or_default(&Harness::Codex, None),
        };
        let mut handover = Handover::default();
        assert_eq!(
            handover.observe(idle(Idle, 4, 60_000), codex, 0, |_| true, || true),
            None
        );
        assert_eq!(
            handover.observe(
                idle(Idle, 4, 60_000),
                codex,
                CODEX_IDLE_SECS,
                |_| true,
                || true
            ),
            Some(HandoverStep::Ask(CODEX_IDLE_SECS, Trigger::Idle(25)))
        );
    }

    #[test]
    fn the_baseline_is_the_first_rest_seen_and_kept_on_the_pane_across_dashboards() {
        let last = CLAUDE_IDLE_SECS;
        // The dashboard saw the session start: its first turn ends at 30,000.
        let mut handover = Handover::default();
        let first = |state, change, tokens| Sighting {
            baseline_token: None,
            ..idle(state, change, tokens)
        };
        handover.observe(first(Working, 1, 20_000), CLAUDE, 0, |_| true, || true);
        assert_eq!(handover.baseline(), None, "not while its first turn runs");
        handover.observe(first(Done, 2, 30_000), CLAUDE, 1, |_| true, || true);
        assert_eq!(handover.baseline(), Some(30_000));
        handover.observe(first(Working, 3, 50_000), CLAUDE, 2, |_| true, || true);
        handover.observe(first(Done, 4, 60_000), CLAUDE, 3, |_| true, || true);
        assert_eq!(handover.baseline(), Some(30_000));
        // A dashboard that restarts, or takes over waking, reads the pane's
        // token: the same session, grown twofold, is asked after a rest.
        assert_eq!(
            idle_steps(&[(0, idle(Idle, 4, 60_000)), (last, idle(Idle, 4, 60_000))]),
            [None, ASK_IDLE]
        );
        // Without its token, the first rest this dashboard sees is the
        // baseline, which only ever asks for more growth.
        let unrecorded = Sighting {
            baseline_token: Some("30000 s0"),
            ..idle(Idle, 4, 60_000)
        };
        let mut handover = Handover::default();
        assert_eq!(
            handover.observe(unrecorded, CLAUDE, 0, |_| true, || true),
            None
        );
        assert_eq!(handover.baseline(), Some(60_000));
        assert_eq!(
            handover.observe(unrecorded, CLAUDE, last, |_| true, || true),
            None
        );
    }

    #[test]
    fn a_note_counts_only_when_written_since_the_request() {
        let dir = env::temp_dir().join(format!("corgi-handover-note-{}", std::process::id()));
        fs::create_dir_all(&dir).expect("create state dir");
        assert!(!note_written_since(&dir, 0));
        fs::write(dir.join(HANDOVER_NOTE), "").expect("write empty note");
        assert!(!note_written_since(&dir, 0), "an empty note is no note");
        fs::write(dir.join(HANDOVER_NOTE), "# Handover\n").expect("write note");
        assert!(note_written_since(&dir, 0));
        assert!(
            !note_written_since(&dir, u64::MAX / 2),
            "a note older than the request"
        );
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn the_successor_reads_the_note_and_archives_it_under_a_utc_stamp() {
        assert_eq!(utc_stamp(0), "19700101-000000");
        assert_eq!(utc_stamp(1_790_262_245), "20260924-150405");
        assert_eq!(utc_stamp(951_782_400), "20000229-000000");
        let state = Path::new("/state/steward/weather");
        let archive = handover_archive(state, 1_790_262_245);
        assert_eq!(
            archive,
            Path::new("/state/steward/weather/handovers/20260924-150405.md")
        );
        let prompt = takeover_prompt("/repos/weather", state, &archive);
        assert!(prompt.contains("State directory: /state/steward/weather."));
        assert!(prompt.contains(
            "read /state/steward/weather/handover.md and move it to \
             /state/steward/weather/handovers/20260924-150405.md"
        ));
        assert!(prompt.contains("Skip the greeting"));
        let request = handover_request(state, Trigger::Full(50));
        assert!(request.starts_with("[corgi] Your context is past 50%"));
        assert!(request.contains("Write /state/steward/weather/handover.md"));
        assert_eq!(
            handover_request(state, Trigger::Update),
            "[corgi] You worked after writing your handover note; bring \
             /state/steward/weather/handover.md up to date with what happened since you \
             wrote it, then end your turn."
        );
    }

    #[test]
    fn the_role_says_how_to_hand_over_and_how_to_take_over() {
        let role = role(Path::new("/opt/corgi/corgi"), Path::new("/state"));
        assert!(role.contains(&handover_request(
            Path::new("/state"),
            Trigger::Full(HANDOVER_PERCENT)
        )));
        assert!(role.contains(&handover_request(
            Path::new("/state"),
            Trigger::Idle(CLAUDE_IDLE_SECS / 60)
        )));
        assert!(role.contains(&handover_request(Path::new("/state"), Trigger::Update)));
        assert!(role.contains("`/state/handover.md` exists"));
        assert!(role.contains("`/state/handovers/<YYYYMMDD-HHMMSS>.md`"));
        for section in [
            "Open threads with the user",
            "Proposed plans not yet approved",
            "`[steward]` prompts since each worker's last wake",
            "Promises to the user",
        ] {
            assert!(role.contains(section), "{section}");
        }
    }

    #[test]
    fn a_launch_is_recorded_for_the_successor_on_the_same_harness() {
        let dir = env::temp_dir().join(format!("corgi-steward-launch-{}", std::process::id()));
        fs::create_dir_all(&dir).expect("create state dir");
        assert_eq!(launch_of(&dir, &Harness::Codex).kind, "codex");
        let launch = Launch {
            kind: "claude".into(),
            model: "opus".into(),
            effort: "high".into(),
            extra_args: vec!["--verbose".into()],
        };
        save_launch(&dir, &launch).expect("save launch");
        assert_eq!(launch_of(&dir, &Harness::Claude), launch);
        // A Steward now on another harness starts on its defaults.
        assert_eq!(
            launch_of(&dir, &Harness::Codex),
            Launch {
                kind: "codex".into(),
                ..Launch::default()
            }
        );
        fs::remove_dir_all(&dir).ok();
    }
}
