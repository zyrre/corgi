//! Sessions menu and deliberate, cancel-first confirmations. Slow work uses Job.
use super::{App, Overlay};
use crate::{
    job::{Job, Update},
    recovery::{self, Row, Snapshot, Store, Target},
    time::{rfc3339_utc, unix_now},
};
use anyhow::Context;
use crossterm::event::{KeyCode, KeyEvent};
use std::{
    collections::BTreeMap,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::Duration,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Stage {
    Menu,
    Save,
    Browser,
    Select,
    Confirm,
    Continue,
    Working,
    Result,
}
#[derive(Debug)]
pub(crate) struct Sessions {
    pub stage: Stage,
    pub cursor: usize,
    pub scroll_limit: usize,
    pub can_confirm: bool,
    pub confirm: bool,
    pub acknowledged: bool,
    pub accepted_setup: bool,
    pub saved: Option<Snapshot>,
    pub snapshots: Vec<Snapshot>,
    pub rows: Vec<Row>,
    pub selected: BTreeMap<usize, Target>,
    pub previous_restore: bool,
    pub messages: Vec<String>,
    pub stop: Arc<AtomicBool>,
}
impl Default for Sessions {
    fn default() -> Self {
        Self {
            stage: Stage::Menu,
            cursor: 0,
            scroll_limit: 0,
            can_confirm: true,
            confirm: false,
            acknowledged: false,
            accepted_setup: false,
            saved: None,
            snapshots: Vec::new(),
            rows: Vec::new(),
            selected: BTreeMap::new(),
            previous_restore: false,
            messages: Vec::new(),
            stop: Arc::new(AtomicBool::new(false)),
        }
    }
}

pub(super) enum Event {
    Inventory(Snapshot),
    Browser(Vec<Snapshot>),
    Plan(Snapshot, Vec<Row>, bool),
    Holds(Vec<String>),
    Finished(String),
}
#[derive(Debug)]
enum Action {
    None,
    Close,
    Inventory(bool),
    Browse,
    Inspect(Snapshot),
    Save(Snapshot),
    Restore,
    Holds,
    Continue,
}

impl Sessions {
    pub fn title(&self) -> &'static str {
        match self.stage {
            Stage::Menu => "Sessions",
            Stage::Save => "Save sessions",
            Stage::Browser => "Saved sessions",
            Stage::Select => "Restore sessions — select",
            Stage::Confirm => "Confirm restore",
            Stage::Continue => "Continue supervisor coordination?",
            Stage::Working => "Sessions — progress",
            Stage::Result => "Sessions — result",
        }
    }
    fn old(&self) -> bool {
        self.saved.as_ref().is_some_and(|s| {
            unix_now().saturating_sub(s.created) > 7 * 86400 || s.created > unix_now()
        }) || self.previous_restore
    }
    fn differences(&self) -> bool {
        self.saved.as_ref().is_some_and(|s| {
            self.selected.keys().any(|i| {
                !s.entries[*i].warnings.is_empty()
                    || s.entries[*i].settings.unknown
                    || !self.rows[*i].differences.is_empty()
            })
        })
    }
    fn key(&mut self, key: KeyEvent) -> Action {
        if self.stage == Stage::Working {
            if matches!(key.code, KeyCode::Esc | KeyCode::Char('s')) {
                self.stop.store(true, Ordering::Relaxed);
            }
            return Action::None;
        }
        if key.code == KeyCode::Esc {
            return Action::Close;
        }
        let count = match self.stage {
            Stage::Menu => 3,
            Stage::Browser => self.snapshots.len(),
            Stage::Select => self.rows.len(),
            _ => self.scroll_limit + 1,
        };
        match key.code {
            KeyCode::Down | KeyCode::Char('j') => {
                self.cursor = (self.cursor + 1).min(count.saturating_sub(1))
            }
            KeyCode::Up | KeyCode::Char('k') => self.cursor = self.cursor.saturating_sub(1),
            KeyCode::PageDown => self.cursor = (self.cursor + 8).min(count.saturating_sub(1)),
            KeyCode::PageUp => self.cursor = self.cursor.saturating_sub(8),
            KeyCode::Tab
                if matches!(self.stage, Stage::Save | Stage::Confirm | Stage::Continue) =>
            {
                self.confirm = !self.confirm
            }
            KeyCode::Char('o' | 'O') if self.stage == Stage::Confirm => {
                self.acknowledged = !self.acknowledged
            }
            KeyCode::Char('a' | 'A') if self.stage == Stage::Confirm => {
                self.accepted_setup = !self.accepted_setup;
            }
            KeyCode::Char('r') if self.stage == Stage::Save => return Action::Inventory(false),
            KeyCode::Char('w') if self.stage == Stage::Save => return Action::Inventory(true),
            KeyCode::Char('r') if self.stage == Stage::Select => {
                return Action::Inspect(self.saved.clone().unwrap());
            }
            KeyCode::Char(' ') | KeyCode::Char('f') if self.stage == Stage::Select => {
                if self.selected.remove(&self.cursor).is_none() {
                    let Some(row) = self.rows.get(self.cursor) else {
                        return Action::None;
                    };
                    let target = if key.code == KeyCode::Char('f') {
                        row.alternative.as_ref()
                    } else {
                        row.target.as_ref()
                    };
                    if let Some(target) = target {
                        self.selected.insert(self.cursor, target.clone());
                    }
                }
            }
            KeyCode::Enter if self.confirm && !self.can_confirm => {}
            KeyCode::Enter => match self.stage {
                Stage::Menu => {
                    return match self.cursor {
                        0 => Action::Inventory(false),
                        1 => Action::Browse,
                        _ => Action::Holds,
                    };
                }
                Stage::Browser => {
                    if let Some(saved) = self.snapshots.get(self.cursor) {
                        return Action::Inspect(saved.clone());
                    }
                }
                Stage::Select if !self.selected.is_empty() => {
                    self.stage = Stage::Confirm;
                    self.cursor = 0;
                    self.confirm = false;
                    self.acknowledged = false;
                    self.accepted_setup = false;
                }
                Stage::Confirm if !self.confirm => {
                    self.stage = Stage::Select;
                    self.cursor = 0;
                }
                Stage::Confirm
                    if !self.selected.is_empty()
                        && (!self.old() || self.acknowledged)
                        && (!self.differences() || self.accepted_setup) =>
                {
                    return Action::Restore;
                }
                Stage::Save if self.confirm => return Action::Save(self.saved.clone().unwrap()),
                Stage::Continue if self.confirm => return Action::Continue,
                Stage::Save | Stage::Continue | Stage::Result => {
                    return Action::Close;
                }
                _ => {}
            },
            _ => {}
        }
        Action::None
    }
    pub fn lines(&self) -> Vec<String> {
        let mut lines = Vec::new();
        if let Some(s) = &self.saved {
            lines.push(format!(
                "Saved {} · {} minutes old · {}",
                rfc3339_utc(s.created),
                unix_now().saturating_sub(s.created) / 60,
                s.id
            ));
            lines.push(format!("Herdr scope: {}", s.scope));
        }
        match self.stage {
            Stage::Menu => {
                lines.push(
                    "Corgi's agents come back by themselves after a Herdr restart. Save keeps a snapshot to restore by hand.".into(),
                );
                for (i, label) in [
                    "Save sessions…",
                    "Restore sessions…",
                    "Continue held coordination…",
                ]
                .iter()
                .enumerate()
                {
                    lines.push(format!(
                        "{} {label}",
                        if self.cursor == i { ">" } else { " " }
                    ));
                }
                lines.push("Other named/remote sessions are outside this save's scope.".into());
            }
            Stage::Save => {
                if let Some(s) = &self.saved {
                    let unidentified = s.entries.iter().filter(|e| e.native_id.is_none()).count();
                    let unsupported = s
                        .entries
                        .iter()
                        .filter(|e| !matches!(e.kind.as_str(), "codex" | "claude"))
                        .count();
                    let busy = s
                        .entries
                        .iter()
                        .filter(|e| {
                            matches!(
                                e.state,
                                crate::model::AgentState::Working
                                    | crate::model::AgentState::Blocked
                                    | crate::model::AgentState::Unknown
                            )
                        })
                        .count();
                    lines.extend([format!("{} sessions · {unidentified} missing IDs · {unsupported} unsupported · {busy} busy/unknown",s.entries.len()),
                    "References and recoverable setup only. Agents keep running; this does not make reboot safe.".into(),
                    "No tools, RAM, unsent input or terminal processes are checkpointed.".into()]);
                    lines.push(RESTARTS_LINE.into());
                    for e in &s.entries {
                        lines.push(format!(
                            "{} · {} · {}",
                            e.label(),
                            e.state.label(),
                            e.warnings.join("; ")
                        ));
                    }
                }
            }
            Stage::Browser => {
                lines.push("Enter browses only. No sessions are selected or started.".into());
                for (i, s) in self.snapshots.iter().enumerate() {
                    lines.push(format!(
                        "{} {} · {} sessions · {}",
                        if i == self.cursor { ">" } else { " " },
                        rfc3339_utc(s.created),
                        s.entries.len(),
                        s.id
                    ));
                }
                if self.snapshots.is_empty() {
                    lines.push("No saved sessions.".into());
                }
            }
            Stage::Select => {
                if let Some(s) = &self.saved {
                    lines.push("Original terminals are preferred. An agent Herdr already resumed is listed as running.".into());
                    for (i, (e, row)) in s.entries.iter().zip(&self.rows).enumerate() {
                        lines.push(format!(
                            "{} [{}] {} · {}",
                            if self.cursor == i { ">" } else { " " },
                            if self.selected.contains_key(&i) {
                                "x"
                            } else {
                                " "
                            },
                            e.label(),
                            self.selected
                                .get(&i)
                                .map(Target::description)
                                .unwrap_or_else(|| row.reason.clone())
                        ));
                        if self.cursor == i {
                            lines.push(format!(
                                "    Directory: {} · model {} · effort {}",
                                e.cwd.display(),
                                known(&e.settings.model),
                                known(&e.settings.effort)
                            ));
                            lines.push(settings_line(e));
                            for warning in e.warnings.iter().chain(&row.differences) {
                                lines.push(format!("    Setup/coverage: {warning}"));
                            }
                        }
                    }
                    lines.push(format!("Selected {} / {}. Space selects ready; F explicitly chooses offered NEW placement.",self.selected.len(),s.entries.len()));
                }
            }
            Stage::Confirm => {
                if let Some(s) = &self.saved {
                    lines.push(format!(
                        "Restore {} selected conversations; no prompt will be sent.",
                        self.selected.len()
                    ));
                    let mut projects = BTreeMap::<String, usize>::new();
                    for (i, target) in &self.selected {
                        let e = &s.entries[*i];
                        *projects
                            .entry(if e.role == "scratch" {
                                "Scratch".into()
                            } else {
                                e.project.clone()
                            })
                            .or_default() += 1;
                        lines.push(format!(
                            "{} · {} · {}",
                            e.name,
                            e.role,
                            target.description()
                        ));
                        lines.push(format!(
                            "  cwd {} · model {} · effort {}",
                            e.cwd.display(),
                            known(&e.settings.model),
                            known(&e.settings.effort)
                        ));
                        lines.push(settings_line(e));
                        for w in e.warnings.iter().chain(&self.rows[*i].differences) {
                            lines.push(format!("  Accepted difference required: {w}"));
                        }
                    }
                    for (project, count) in projects {
                        lines.push(format!("Project {project}: {count} selected"));
                    }
                    for (i, row) in self
                        .rows
                        .iter()
                        .enumerate()
                        .filter(|(i, _)| !self.selected.contains_key(i))
                    {
                        lines.push(format!("Skipped {}: {}", s.entries[i].name, row.reason));
                    }
                    lines.push(
                        "Supervisor coordination stays held until explicitly continued.".into(),
                    );
                    lines.push("CLI startup hooks may run. Native history opens at its latest durable state.".into());
                    if self.old() {
                        lines.push(format!("[{}] O/o: I reviewed this old/clock-different/previously restored snapshot",if self.acknowledged { "x" } else { " " }));
                    }
                    if self.differences() {
                        lines.push(format!("[{}] A/a: accept the listed unknown setup / interrupted-work differences",if self.accepted_setup { "x" } else { " " }));
                    }
                }
            }
            Stage::Continue => {
                lines.extend(self.messages.clone());
                lines.push("Enable inbox delivery and automatic handovers for these recovered supervisors.".into());
            }
            Stage::Working => {
                lines.push("Working… No agent prompts are sent by recovery.".into());
                lines.extend(self.messages.clone());
            }
            Stage::Result => lines.extend(self.messages.clone()),
        }
        lines
    }
    pub fn footer(&self) -> String {
        match self.stage {
            Stage::Save=>format!("{} Cancel    {} Save now (previous saves retained) · Tab choose · Enter · R refresh · W wait up to 30s",if self.confirm { " " } else { ">" },if self.confirm { ">" } else { " " }),
            Stage::Confirm=>format!("{} Back/Cancel    {} Restore {} sessions · Tab choose · Enter · O/o old-save ack · A/a setup ack",if self.confirm { " " } else { ">" },if self.confirm { ">" } else { " " },self.selected.len()),
            Stage::Continue=>format!("{} Cancel    {} Confirm · Tab choose · Enter",if self.confirm { " " } else { ">" },if self.confirm { ">" } else { " " }),
            Stage::Working=>"Esc / S: stop scheduling (successful starts remain open)".into(),
            Stage::Select=>"↑/↓ select row · Space select ready · F new-placement offer · Enter review · R revalidate · Esc cancel".into(),
            _=>"↑/↓ browse/scroll · Enter · Esc close".into(),
        }
    }
}

impl App {
    pub(super) fn begin_sessions(&mut self) {
        self.overlay = Overlay::Sessions(Sessions::default());
    }
    pub(super) fn handle_sessions_key(&mut self, key: KeyEvent) -> bool {
        let Overlay::Sessions(form) = &mut self.overlay else {
            return false;
        };
        let action = form.key(key);
        match action {
            Action::None => return false,
            Action::Close => {
                self.overlay = Overlay::None;
                return false;
            }
            _ => {}
        }
        let client = self.client.clone();
        let displayed = self.agents.clone();
        let stop = Arc::new(AtomicBool::new(false));
        form.stop = stop.clone();
        let saved = form.saved.clone();
        let chosen = form
            .selected
            .iter()
            .map(|(i, t)| (*i, t.clone(), form.rows[*i].differences.clone()))
            .collect::<Vec<_>>();
        let expected_holds = form
            .messages
            .iter()
            .cloned()
            .collect::<std::collections::BTreeSet<_>>();
        form.stage = Stage::Working;
        form.confirm = false;
        form.cursor = 0;
        form.messages.clear();
        self.recovery_job = Job::spawn(move |progress| {
            let store = Store::for_client(&client).context("recovery storage unavailable")?;
            match action {
                Action::Inventory(wait) => {
                    let mut s = recovery::capture(&client, &displayed)?;
                    if wait {
                        for _ in 0..30 {
                            if stop.load(Ordering::Relaxed)
                                || s.entries.iter().all(|e| {
                                    matches!(
                                        e.state,
                                        crate::model::AgentState::Idle
                                            | crate::model::AgentState::Done
                                    )
                                })
                            {
                                break;
                            }
                            progress.send(
                                "Waiting for idle; agents keep running. Esc stops waiting.".into(),
                            );
                            thread::sleep(Duration::from_secs(1));
                            s = recovery::capture(&client, &displayed)?;
                        }
                    }
                    Ok(Event::Inventory(s))
                }
                Action::Browse => Ok(Event::Browser(store.snapshots()?)),
                Action::Inspect(s) => {
                    let rows = recovery::plan(&client, &s)?;
                    let previous = store.journal()?.restored.contains_key(&s.id);
                    Ok(Event::Plan(s, rows, previous))
                }
                Action::Save(s) => {
                    // Re-observe without silently replacing the reviewed inventory.
                    let mut fresh = recovery::capture(&client, &displayed)?;
                    anyhow::ensure!(
                        s.entries == fresh.entries,
                        "session inventory/setup changed; open Save and review again"
                    );
                    fresh.id = s.id;
                    fresh.created = s.created;
                    store.save(&fresh)?;
                    Ok(Event::Finished(format!(
                        "Saved {} · {} sessions · previous saves retained. Coverage gaps remain listed in the saved details. Agents were not stopped or prompted.",
                        fresh.id,
                        fresh.entries.len()
                    )))
                }
                Action::Restore => {
                    recovery::restore(
                        &client,
                        &saved.context("snapshot missing")?,
                        &chosen,
                        &stop,
                        |m| progress.send(m),
                    )?;
                    Ok(Event::Finished("Finished checking selected entries. Read each outcome above; pending/unknown launches are never automatically retried. Continue held coordination separately from Sessions.".into()))
                }
                Action::Holds => Ok(Event::Holds(
                    recovery::held_projects(&client, &displayed)?
                        .into_iter()
                        .collect(),
                )),
                Action::Continue => {
                    let current = recovery::held_projects(&client, &displayed)?;
                    anyhow::ensure!(
                        current == expected_holds,
                        "Held projects changed; review coordination again"
                    );
                    recovery::continue_coordination(&client, &expected_holds)?;
                    Ok(Event::Finished(
                        "Recovered supervisor coordination explicitly continued.".into(),
                    ))
                }
                _ => unreachable!(),
            }
        });
        false
    }
    pub(super) fn poll_recovery(&mut self) {
        for update in self.recovery_job.poll() {
            let Overlay::Sessions(form) = &mut self.overlay else {
                continue;
            };
            let event = match update {
                Update::Progress(message) => {
                    form.messages.push(message);
                    if form.messages.len() > 1000 {
                        form.messages.remove(0);
                    }
                    continue;
                }
                Update::Finished(result) => result,
                Update::Panicked => Err(anyhow::anyhow!(
                    "Recovery worker stopped. Inspect pending outcomes before retrying."
                )),
            };
            form.cursor = 0;
            form.confirm = false;
            match event {
                Ok(Event::Inventory(saved)) => {
                    form.saved = Some(saved);
                    form.stage = Stage::Save;
                }
                Ok(Event::Browser(snapshots)) => {
                    form.snapshots = snapshots;
                    form.saved = None;
                    form.stage = Stage::Browser;
                }
                Ok(Event::Plan(saved, rows, previous)) => {
                    form.saved = Some(saved);
                    form.rows = rows;
                    form.previous_restore = previous;
                    form.selected.clear();
                    form.stage = Stage::Select;
                }
                Ok(Event::Holds(projects)) => {
                    let empty = projects.is_empty();
                    form.messages = if projects.is_empty() {
                        vec!["No recovered supervisor coordination is held.".into()]
                    } else {
                        projects
                    };
                    form.stage = if empty {
                        Stage::Result
                    } else {
                        Stage::Continue
                    };
                }
                Ok(Event::Finished(message)) => {
                    form.messages.push(message);
                    form.stage = Stage::Result;
                }
                Err(error) => {
                    form.messages.push(format!("Recovery stopped: {error}"));
                    form.stage = Stage::Result;
                }
            }
        }
    }
}

/// What Save says about restarts, now that Herdr resumes Corgi's agents
/// with the command Corgi reports.
const RESTARTS_LINE: &str = "Herdr 0.9.2 or later resumes Corgi's agents after a restart with their launch options; Save is for restoring by hand.";

fn known(value: &str) -> &str {
    if value.is_empty() {
        "unverified native default"
    } else {
        value
    }
}
fn settings_line(entry: &recovery::Entry) -> String {
    let s = &entry.settings;
    format!(
        "  Sandbox {} · approval {} · permission {} · network {} · extra roots {}{}",
        s.sandbox.as_deref().unwrap_or("native default"),
        s.approval.as_deref().unwrap_or("native default"),
        s.permission.as_deref().unwrap_or("native default"),
        match s.network {
            Some(true) => "enabled",
            Some(false) => "disabled",
            None => "native default",
        },
        s.writable
            .iter()
            .map(|p| p.display().to_string())
            .collect::<Vec<_>>()
            .join(", "),
        entry
            .supervisor
            .as_ref()
            .map(|s| format!(
                " · supervisor role {} · memory {}",
                s.role_file.display(),
                s.state_dir.display()
            ))
            .unwrap_or_default()
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyEventKind, KeyModifiers};
    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }
    #[test]
    fn browsing_and_repeated_enter_never_confirm_restore() {
        let mut form = Sessions {
            stage: Stage::Confirm,
            ..Sessions::default()
        };
        assert!(matches!(form.key(key(KeyCode::Enter)), Action::None));
        assert_eq!(form.stage, Stage::Select);
        for _ in 0..5 {
            assert!(matches!(form.key(key(KeyCode::Enter)), Action::None));
        }
    }

    #[test]
    fn the_menu_has_no_manual_setup_any_more() {
        let form = Sessions::default();
        let lines = form.lines().join("\n");
        assert!(lines.contains("Save sessions"));
        assert!(lines.contains("Restore sessions"));
        assert!(lines.contains("Continue held coordination"));
        assert!(!lines.contains("Manual recovery setup"));
        let mut form = Sessions::default();
        form.key(key(KeyCode::Down));
        form.key(key(KeyCode::Down));
        form.key(key(KeyCode::Down));
        assert_eq!(form.cursor, 2);
        assert!(matches!(form.key(key(KeyCode::Enter)), Action::Holds));
    }
    #[test]
    fn no_initial_selection_and_an_empty_inventory_is_safe() {
        let mut form = Sessions {
            stage: Stage::Select,
            ..Sessions::default()
        };
        assert!(form.selected.is_empty());
        assert!(matches!(form.key(key(KeyCode::Char(' '))), Action::None));
        assert!(matches!(form.key(key(KeyCode::Enter)), Action::None));
        assert_eq!(form.stage, Stage::Select);
    }
    #[test]
    fn top_level_discards_repeat_events_before_sessions_can_execute() {
        let mut app = crate::test_support::test_app();
        app.begin_sessions();
        let mut repeat = key(KeyCode::Enter);
        repeat.kind = KeyEventKind::Repeat;
        app.handle_key(repeat);
        assert!(!app.recovery_job.is_running());
        assert!(matches!(&app.overlay,Overlay::Sessions(f) if f.stage==Stage::Menu));
    }
    #[test]
    fn final_confirmation_requires_selection_tab_and_inline_acknowledgments() {
        let (_scratch, _client, mut saved, _) = recovery::fixture::hold_fixture();
        saved.created = 1;
        saved.entries[0].settings.unknown = true;
        let mut form = Sessions {
            stage: Stage::Select,
            saved: Some(saved),
            rows: vec![Row {
                target: Some(Target::Original {
                    workspace: "w1".into(),
                    tab: "t1".into(),
                    pane: "w1:p1".into(),
                }),
                alternative: None,
                reason: "Ready".into(),
                differences: Vec::new(),
            }],
            ..Sessions::default()
        };
        form.key(key(KeyCode::Char(' ')));
        form.key(key(KeyCode::Enter));
        assert_eq!(form.stage, Stage::Confirm);
        assert!(!form.confirm);
        form.key(key(KeyCode::Tab));
        assert!(matches!(form.key(key(KeyCode::Enter)), Action::None));
        form.key(key(KeyCode::Char('o')));
        assert!(matches!(form.key(key(KeyCode::Enter)), Action::None));
        form.key(key(KeyCode::Char('a')));
        form.can_confirm = false;
        assert!(matches!(form.key(key(KeyCode::Enter)), Action::None));
        form.can_confirm = true;
        assert!(matches!(form.key(key(KeyCode::Enter)), Action::Restore));
    }

    #[test]
    fn running_cancel_stops_scheduling_without_closing_the_job() {
        let mut form = Sessions {
            stage: Stage::Working,
            ..Sessions::default()
        };
        assert!(matches!(form.key(key(KeyCode::Esc)), Action::None));
        assert!(form.stop.load(Ordering::Relaxed));
        assert_eq!(form.stage, Stage::Working);
    }
    #[test]
    fn space_does_not_change_recovery_acknowledgments() {
        for stage in [
            Stage::Menu,
            Stage::Save,
            Stage::Browser,
            Stage::Confirm,
            Stage::Continue,
            Stage::Result,
            Stage::Working,
        ] {
            let mut form = Sessions {
                stage,
                confirm: true,
                ..Sessions::default()
            };
            assert!(matches!(form.key(key(KeyCode::Char(' '))), Action::None));
            assert!(!form.accepted_setup && !form.acknowledged);
            assert_eq!(form.stage, stage);
        }
    }

    #[test]
    fn restore_acknowledgment_hints_accept_both_letter_cases() {
        let (_scratch, _client, mut saved, _) = recovery::fixture::hold_fixture();
        saved.created = 1;
        saved.entries[0].settings.unknown = true;
        for (old, setup) in [('o', 'a'), ('O', 'A')] {
            let mut form = Sessions {
                stage: Stage::Confirm,
                saved: Some(saved.clone()),
                selected: BTreeMap::from([(
                    0,
                    Target::Original {
                        workspace: "w1".into(),
                        tab: "t1".into(),
                        pane: "w1:p1".into(),
                    },
                )]),
                rows: vec![Row {
                    target: None,
                    alternative: None,
                    reason: String::new(),
                    differences: Vec::new(),
                }],
                ..Sessions::default()
            };
            assert!(matches!(form.key(key(KeyCode::Char(old))), Action::None));
            assert!(form.acknowledged);
            assert!(matches!(form.key(key(KeyCode::Char(setup))), Action::None));
            assert!(form.accepted_setup);
            assert!(!form.confirm);
            form.key(key(KeyCode::Tab));
            assert!(matches!(form.key(key(KeyCode::Enter)), Action::Restore));
            form.key(key(KeyCode::Char(old)));
            assert!(matches!(form.key(key(KeyCode::Enter)), Action::None));
            form.key(key(KeyCode::Char(old)));
            form.key(key(KeyCode::Char(setup)));
            assert!(matches!(form.key(key(KeyCode::Enter)), Action::None));
        }
    }
}
