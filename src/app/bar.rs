//! The `--bar-*` endpoints of the Omarchy widget. Their JSON is that widget's
//! contract, pinned by the snapshot tests below, and they drive the same
//! forms, key handlers and launch as the dashboard.

use std::{
    io,
    sync::mpsc::{self, Receiver},
    thread,
    time::Duration,
};

use anyhow::{Context, Result};
use crossterm::event::{KeyCode, KeyEvent};

use crate::{
    choices::Choice, harness::Harness, herdr::HerdrClient, projects::known_projects,
    session::SessionReader, time::unix_now, ui,
};

use super::{
    App, HANDLER_NAME,
    catalog::{ModelCatalogUpdate, effort_choices, harness_choices, model_choices},
    close::CloseTarget,
    form::{Checkout, NewField},
    launch::LaunchState,
    merge::MergePhase,
    overlay::Overlay,
};

/// The same newest-first session history Space opens in the dashboard, read
/// only when the Omarchy popup expands an agent.
pub fn bar_transcript(socket: &str, pane_id: &str) -> Result<()> {
    let snapshot = HerdrClient::from_socket_path(socket).snapshot()?;
    let agent = snapshot
        .agents
        .iter()
        .find(|agent| agent.pane_id == pane_id)
        .context("this agent session is no longer available")?;
    let entries: Vec<_> = SessionReader::default()
        .transcript(agent)
        .iter()
        .map(|entry| {
            serde_json::json!({
                "kind": format!("{:?}", entry.kind).to_lowercase(),
                "text": entry.text,
            })
        })
        .collect();
    serde_json::to_writer(
        io::stdout().lock(),
        &serde_json::json!({"pane_id": pane_id, "entries": entries}),
    )?;
    Ok(())
}

fn popup_color_tone(color: Option<ratatui::style::Color>) -> &'static str {
    use ratatui::style::Color;
    match color {
        Some(Color::Cyan) => "cyan",
        Some(Color::Green) => "green",
        Some(Color::Yellow) => "yellow",
        Some(Color::Red) => "red",
        Some(Color::DarkGray) => "muted",
        _ => "foreground",
    }
}

/// A dialog as the Omarchy popup draws it: its title without the mark the
/// dashboard's frame leads with, and its keys as a line of words under the
/// text, since the popup draws no keycaps.
fn popup_dialog_state(phase: &str, content: ui::DialogContent<'_>) -> serde_json::Value {
    let ui::DialogContent {
        title,
        color,
        mut lines,
        legend,
        ..
    } = content;
    if !legend.is_empty() {
        lines.push(ratatui::text::Line::raw(""));
        lines.push(ratatui::text::Line::styled(
            ui::legend_text(&legend),
            ratatui::style::Style::default().fg(ratatui::style::Color::DarkGray),
        ));
    }
    let text = lines
        .iter()
        .map(|line| {
            line.spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n");
    let lines: Vec<_> = lines.into_iter().map(|line| {
        let spans: Vec<_> = line.spans.into_iter().map(|span| serde_json::json!({
            "text": span.content, "tone": popup_color_tone(span.style.fg),
            "bold": span.style.add_modifier.contains(ratatui::style::Modifier::BOLD)
        })).collect();
        serde_json::json!({"spans": spans, "left": line.alignment == Some(ratatui::layout::Alignment::Left)})
    }).collect();
    serde_json::json!({"phase": phase, "title": title.trim(), "text": text,
        "tone": popup_color_tone(Some(color)), "lines": lines})
}

/// The bar uses the same presets, selectors, validation and launch lifecycle as the TUI.
pub fn bar_new_options(socket: &str, pane_id: &str, kind: &str, model: &str) -> Result<()> {
    let app = App::headless(HerdrClient::from_socket_path(socket));
    let options = bar_new_options_value(
        app,
        pane_id,
        kind,
        model,
        &Harness::installed(),
        Harness::discover_models,
    )?;
    serde_json::to_writer(io::stdout(), &options)?;
    Ok(())
}

/// What `--bar-new-options` prints: the new-agent form preset for the agent
/// `pane_id`, on harness `kind` and `model`, with its selector lists. The
/// model list comes from the same catalog as the form's, discovered with
/// `fetch_models` for a harness that has one and the curated fallback
/// otherwise, and `installed` badges the harness list.
fn bar_new_options_value(
    mut app: App,
    pane_id: &str,
    kind: &str,
    model: &str,
    installed: &[Harness],
    fetch_models: impl FnOnce(&Harness) -> Result<Vec<String>>,
) -> Result<serde_json::Value> {
    app.refresh();
    anyhow::ensure!(app.connected, "Herdr is unavailable");
    app.selected = app
        .agents
        .iter()
        .position(|agent| agent.info.pane_id == pane_id)
        .unwrap_or(0);
    app.begin_new_agent();
    let form = app.overlay.new_agent_form_mut().context("new agent form")?;
    if !kind.is_empty() {
        form.kind = kind.to_string();
    }
    form.model = model.to_string();
    app.refresh_form_defaults();
    let harness = app
        .overlay
        .new_agent_form()
        .context("new agent form")?
        .harness();
    if harness.supports_model_discovery() {
        let models = fetch_models(&harness).map_err(|error| format!("{error:#}"));
        app.install_model_catalog(ModelCatalogUpdate { harness, models });
    }
    let form = app.overlay.new_agent_form().context("new agent form")?;
    let models = app.model_options(&form.harness());
    let choices = |items: Vec<Choice>| {
        items.into_iter().map(|choice| serde_json::json!({"value": choice.value, "label": choice.label, "detail": choice.detail, "badge": choice.badge})).collect::<Vec<_>>()
    };
    let projects = known_projects(&app.agents, &app.workspaces, &app.projects)
        .into_iter()
        .map(|project| project.root)
        .collect::<Vec<_>>();
    Ok(serde_json::json!({
        "kind": form.kind, "project": form.project, "projects": projects,
        "harnesses": choices(harness_choices(installed, &form.kind)),
        "models": choices(model_choices(&models, model, form.defaults.model.as_deref())),
        "efforts": choices(effort_choices(form.defaults.effort.as_deref())),
        "supportsEffort": form.supports_effort()
    }))
}

pub fn bar_new_agent(socket: &str) -> Result<()> {
    use std::io::{BufRead, Write};
    let mut line = String::new();
    io::stdin().lock().read_line(&mut line)?;
    let request: serde_json::Value = serde_json::from_str(&line)?;
    let value = |key: &str| request[key].as_str().unwrap_or("").to_string();
    let mut app = App::headless(HerdrClient::from_socket_path(socket));
    app.refresh();
    anyhow::ensure!(app.connected, "Herdr is unavailable");
    app.begin_new_agent_in(value("project"));
    let form = app.overlay.new_agent_form_mut().context("new agent form")?;
    form.apply(NewField::Project, &value("project"));
    form.kind = value("kind");
    form.model = value("model");
    form.effort = value("effort");
    form.prompt = value("task");
    form.checkout = Checkout::from_value(&value("checkout")).context("invalid checkout")?;
    app.submit_new_agent();
    let mut previous = String::new();
    loop {
        app.poll_launch();
        let (phase, text) = match &app.overlay {
            // The form stays open when it did not validate.
            Overlay::NewAgent(form) => ("failed", form.error.clone().unwrap_or_default()),
            Overlay::Launch(launch) => match &launch.state {
                LaunchState::Running { status, .. } => ("running", status.clone()),
                LaunchState::Started { message, .. } => ("succeeded", message.clone()),
                LaunchState::Failed(error) => ("failed", format!("New agent failed: {error}")),
            },
            _ => ("succeeded", app.status.clone()),
        };
        let state = serde_json::json!({"phase": phase, "text": text}).to_string();
        if state != previous {
            println!("{state}");
            io::stdout().flush()?;
            previous = state;
        }
        if phase != "running" {
            break;
        }
        thread::sleep(Duration::from_millis(50));
    }
    Ok(())
}

/// Interactive popup actions reuse the dashboard forms and key handlers.
/// Each output line is a dialog state; stdin accepts enter, escape, or x.
pub fn bar_action(socket: &str, pane_id: &str, action: &str) -> Result<()> {
    use std::io::BufRead;
    anyhow::ensure!(matches!(action, "x" | "m"), "unknown popup action");
    let app = App::headless(HerdrClient::from_socket_path(socket));
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        for line in io::stdin().lock().lines() {
            let Ok(line) = line else { break };
            if tx.send(line).is_err() {
                break;
            }
        }
    });
    bar_action_with(app, pane_id, action, &rx, &mut io::stdout())
}

/// Runs the popup action `action` on the agent `pane_id`: writes each dialog
/// state to `output` as a line of JSON, and answers the lines from `input`,
/// until the dialog is done or the input ends with nothing left running.
fn bar_action_with(
    mut app: App,
    pane_id: &str,
    action: &str,
    input: &Receiver<String>,
    output: &mut impl io::Write,
) -> Result<()> {
    app.dashboard_pane_id = None;
    app.refresh();
    app.selected = app
        .agents
        .iter()
        .position(|agent| agent.info.pane_id == pane_id)
        .context("this agent session is no longer available")?;
    app.handle_key(KeyEvent::from(KeyCode::Char(if action == "x" {
        'x'
    } else {
        'm'
    })));
    let mut previous = String::new();
    loop {
        app.poll_merge();
        let mut state = serde_json::json!({"phase": "done", "title": "Corgi", "text": app.status});
        match &app.overlay {
            Overlay::Close(form) => {
                state = popup_dialog_state("confirm", ui::close_dialog_lines(form));
                state["confirm"] = serde_json::json!(match form.target {
                    CloseTarget::Worktree { force: true, .. } => "Discard and remove",
                    CloseTarget::Worktree { force: false, .. } => "Remove worktree",
                    CloseTarget::Tab { .. } => "Close tab",
                    CloseTarget::Workspace => "Close workspace",
                });
            }
            Overlay::Merge(form) => {
                let content = ui::merge_dialog_lines(form, crate::motion::SPINNER[0]);
                let phase = match form.phase {
                    MergePhase::Confirm => "confirm",
                    MergePhase::Running(_) => "running",
                    MergePhase::Succeeded => "succeeded",
                    MergePhase::Failed(_) => "failed",
                };
                state = popup_dialog_state(phase, content);
                state["confirm"] = serde_json::json!("Merge + push");
            }
            _ => {}
        }
        let encoded = serde_json::to_string(&state)?;
        if encoded != previous {
            writeln!(output, "{encoded}")?;
            output.flush()?;
            previous = encoded;
        }
        if state["phase"] == "done" {
            break;
        }
        match input.recv_timeout(Duration::from_millis(100)) {
            Ok(input) => {
                let code = match input.as_str() {
                    "enter" => KeyCode::Enter,
                    "escape" => KeyCode::Esc,
                    "x" => KeyCode::Char('x'),
                    _ => continue,
                };
                let was_confirming = match &app.overlay {
                    Overlay::Close(_) => true,
                    Overlay::Merge(form) => matches!(form.phase, MergePhase::Confirm),
                    _ => false,
                };
                app.handle_key(KeyEvent::from(code));
                if input == "escape" && was_confirming {
                    app.status = "Cancelled".into();
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                if !app.merge_job.is_running() {
                    break;
                }
            }
        }
    }
    Ok(())
}

/// Newline-delimited dashboard data for the optional Omarchy frontend.
/// The socket is explicit because the desktop shell is outside a Herdr pane.
pub fn bar_stream(socket: &str) -> Result<()> {
    use std::io::Write;
    // A reader launched for testing from an agent pane must still include that
    // agent. Only the interactive TUI owns HERDR_PANE_ID as its dashboard pane.
    let mut app = App::headless(HerdrClient::from_socket_path(socket));
    let mut output = io::stdout().lock();
    loop {
        app.poll_plan_usage();
        app.poll_codex_task_titles();
        app.refresh_plan_usage();
        app.refresh();
        serde_json::to_writer(&mut output, &bar_stream_frame(&app, unix_now()))?;
        writeln!(output)?;
        output.flush()?;
        thread::sleep(Duration::from_secs(2));
    }
}

/// One line of `--bar-stream`: every agent row and usage card as the
/// dashboard last refreshed them, with countdowns measured against `now`
/// (Unix seconds). The field names are the Omarchy widget's contract.
fn bar_stream_frame(app: &App, now: u64) -> serde_json::Value {
    let agents: Vec<_> = app
        .agents
        .iter()
        .map(|agent| {
            serde_json::json!({
                "pane_id": agent.info.pane_id,
                "name": if agent.handler { HANDLER_NAME } else { agent.info.display_name() },
                "kind": agent.info.kind(),
                "agent_status": agent.info.state,
                "project_group": agent.project_group,
                "checkout": agent.worktree_label,
                "task": agent.task,
                "model": agent.model,
                "effort": agent.effort,
                "context_percent": agent.context_percent,
                "message": agent.message.text,
                "message_kind": format!("{:?}", agent.message.kind).to_lowercase(),
                "tool": agent.tool.text,
                "tool_kind": format!("{:?}", agent.tool.kind).to_lowercase(),
                "details": ui::status_fields(agent, now).into_iter().flat_map(|field| {
                    std::iter::once(serde_json::json!({"text": field.separator, "tone": "muted"})).chain(field.spans.into_iter().map(|span| {
                        use ratatui::style::Color;
                        let indexed = match span.style.fg { Some(Color::Indexed(value)) => Some(value), _ => None };
                        serde_json::json!({"text": span.content, "tone": popup_color_tone(span.style.fg), "indexed": indexed, "bold": span.style.add_modifier.contains(ratatui::style::Modifier::BOLD)})
                    }))
                }).collect::<Vec<_>>(),
                "cache_estimated": agent.cache.as_ref().is_some_and(|cache| cache.kind == crate::model::PromptCacheKind::Estimated),
                "cache_seconds": agent.cache.as_ref().map(|cache| cache.remaining(now)),
            })
        })
        .collect();
    let usage: Vec<_> = app.usage.iter().filter(|slot| ui::show_usage_card(slot)).map(|slot| {
        let lines: Vec<_> = ui::usage_lines_for_slot(slot, now).into_iter().map(|line| {
            let spans: Vec<_> = line.spans.into_iter().map(|span| {
                serde_json::json!({"text": span.content, "tone": popup_color_tone(span.style.fg), "bold": span.style.add_modifier.contains(ratatui::style::Modifier::BOLD)})
            }).collect();
            serde_json::json!({"spans": spans, "right": line.alignment == Some(ratatui::layout::Alignment::Right)})
        }).collect();
        serde_json::json!({"provider": slot.provider.label(), "lines": lines})
    }).collect();
    // A provider with no card carries its reason instead, for the same top
    // right corner the dashboard puts it in.
    let usage_notes: Vec<_> = app
        .usage
        .iter()
        .filter(|slot| !ui::show_usage_card(slot))
        .map(|slot| {
            serde_json::json!({
                "provider": slot.provider.label(),
                "reason": slot.error.clone().unwrap_or_default(),
            })
        })
        .collect();
    serde_json::json!({
        "connected": app.connected, "agents": agents, "usage": usage,
        "usage_notes": usage_notes,
    })
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        path::{Path, PathBuf},
        sync::mpsc,
        thread,
    };

    use serde_json::{Value, json};

    use crate::{
        app::{
            UsageSlot,
            project_main::{
                CORGI_AGENT_WORKSPACE_ROLE, CORGI_PROJECT_MAIN_ROLE, CORGI_PROJECT_MAIN_TAB_TOKEN,
                CORGI_PROJECT_ROOT_TOKEN, CORGI_WORKSPACE_ROLE_TOKEN,
            },
        },
        handler::CORGI_HANDLER_TOKEN,
        herdr::HerdrClient,
        test_support::{answer, fake_herdr, test_app},
        usage::Provider,
    };

    use super::*;

    /// A fake Herdr that answers `replies` in order, one connection each, and
    /// checks that every request calls the method its reply is paired with.
    fn herdr_replying(
        label: &str,
        replies: Vec<(&'static str, Value)>,
    ) -> (PathBuf, thread::JoinHandle<()>) {
        let (socket_path, server) = fake_herdr(label, move |listener| {
            for (method, result) in replies {
                answer(&listener, |request| {
                    assert_eq!(request["method"], method);
                    json!({ "result": result })
                });
            }
        });
        (socket_path, server)
    }

    /// The Herdr calls one popup refresh makes: the snapshot, then a screen
    /// read for each of its two agents, whose session files do not exist.
    fn bar_refresh_replies() -> Vec<(&'static str, Value)> {
        let snapshot = json!({
            "type": "session_snapshot",
            "snapshot": {
                "workspaces": [
                    {
                        "workspace_id": "w-main",
                        "label": "corgi handler",
                        "tokens": {
                            CORGI_WORKSPACE_ROLE_TOKEN: CORGI_PROJECT_MAIN_ROLE,
                            CORGI_PROJECT_MAIN_TAB_TOKEN: "w-main:t1",
                            CORGI_PROJECT_ROOT_TOKEN: "/repos/corgi"
                        },
                        "worktree": {
                            "repo_root": "/repos/corgi",
                            "checkout_path": "/repos/corgi",
                            "is_linked_worktree": false
                        }
                    },
                    {
                        "workspace_id": "w-worker",
                        "label": "worktree-quiet-owl",
                        "tokens": { CORGI_WORKSPACE_ROLE_TOKEN: CORGI_AGENT_WORKSPACE_ROLE },
                        "worktree": {
                            "repo_root": "/repos/corgi",
                            "repo_name": "corgi",
                            "checkout_path": "/worktrees/corgi/quiet-owl",
                            "is_linked_worktree": true
                        }
                    }
                ],
                "tabs": [{ "tab_id": "w-main:t1", "workspace_id": "w-main", "label": "corgi" }],
                "agents": [
                    {
                        "agent": "claude",
                        "name": "corgi-quiet-owl",
                        "agent_status": "working",
                        "agent_session": { "agent": "claude", "value": "corgi-bar-test-worker" },
                        "pane_id": "w-worker:p1",
                        "workspace_id": "w-worker",
                        "tab_id": "w-worker:t1",
                        "cwd": "/worktrees/corgi/quiet-owl",
                        "tokens": {
                            "task": "Pin the bar contract",
                            "model": "opus",
                            "effort": "high",
                            "ctx": "42%"
                        }
                    },
                    {
                        "agent": "claude",
                        "name": "handler-corgi",
                        "agent_status": "idle",
                        "agent_session": { "agent": "claude", "value": "corgi-bar-test-handler" },
                        "pane_id": "w-main:p1",
                        "workspace_id": "w-main",
                        "tab_id": "w-main:t1",
                        "cwd": "/repos/corgi",
                        "tokens": { CORGI_HANDLER_TOKEN: "handler-corgi" }
                    }
                ],
                "panes": []
            }
        });
        let read = |pane_id: &str, text: &str| json!({ "type": "pane_read", "read": { "pane_id": pane_id, "text": text } });
        vec![
            ("session.snapshot", snapshot),
            (
                "agent.read",
                read(
                    "w-worker:p1",
                    "⏺ Pinning the bar contract\n⏺ Bash(cargo test)\n",
                ),
            ),
            ("agent.read", read("w-main:p1", "")),
        ]
    }

    /// A sealed [`test_app`] that talks to the fake Herdr at `socket_path`,
    /// compact as the popup's is.
    fn bar_app(socket_path: &Path) -> App {
        let mut app = test_app();
        app.client = HerdrClient::from_socket_path(socket_path);
        app.compact = true;
        app
    }

    #[test]
    fn the_bar_stream_keeps_the_omarchy_field_names_and_values() {
        let (socket_path, server) = herdr_replying("bar-stream", bar_refresh_replies());
        let mut app = bar_app(&socket_path);
        app.dashboard_pane_id = None;
        app.usage = vec![
            UsageSlot {
                provider: Provider::Codex,
                usage: Some(crate::usage::PlanUsage {
                    provider: Provider::Codex,
                    plan_type: "Pro".into(),
                    five_hour: Some(crate::usage::UsageWindow {
                        used_percent: 30,
                        resets_at: 1_000_000 + 2 * 3_600,
                    }),
                    week: None,
                    banked_resets: None,
                }),
                fetched_at: Some(1_000_000 - 180),
                error: None,
            },
            UsageSlot {
                provider: Provider::Claude,
                usage: None,
                fetched_at: None,
                error: Some("no Claude Code login found".into()),
            },
        ];
        app.refresh();
        server.join().expect("fake server panicked");
        fs::remove_file(socket_path).expect("remove fake socket");

        let frame = bar_stream_frame(&app, 1_000_000);
        assert_eq!(
            frame,
            json!({
                "agents": [
                    {
                        "agent_status": "idle",
                        "cache_estimated": false,
                        "cache_seconds": null,
                        "checkout": null,
                        "context_percent": null,
                        "details": [
                            {
                                "text": " · ",
                                "tone": "muted"
                            },
                            {
                                "bold": false,
                                "indexed": 208,
                                "text": "claude",
                                "tone": "foreground"
                            }
                        ],
                        "effort": null,
                        "kind": "claude",
                        "message": "Ready",
                        "message_kind": "ready",
                        "model": null,
                        "name": "Project handler",
                        "pane_id": "w-main:p1",
                        "project_group": "corgi",
                        "task": "Project handler",
                        "tool": "No command yet",
                        "tool_kind": "ready"
                    },
                    {
                        "agent_status": "working",
                        "cache_estimated": false,
                        "cache_seconds": null,
                        "checkout": "quiet-owl",
                        "context_percent": 42,
                        "details": [
                            {
                                "text": " · ",
                                "tone": "muted"
                            },
                            {
                                "bold": false,
                                "indexed": 208,
                                "text": "opus",
                                "tone": "foreground"
                            },
                            {
                                "text": " ",
                                "tone": "muted"
                            },
                            {
                                "bold": false,
                                "indexed": null,
                                "text": "high",
                                "tone": "cyan"
                            },
                            {
                                "text": " · ",
                                "tone": "muted"
                            },
                            {
                                "bold": false,
                                "indexed": 149,
                                "text": "42% ctx",
                                "tone": "foreground"
                            },
                            {
                                "text": " · ",
                                "tone": "muted"
                            },
                            {
                                "bold": false,
                                "indexed": null,
                                "text": "⑂ ",
                                "tone": "cyan"
                            },
                            {
                                "bold": false,
                                "indexed": null,
                                "text": "quiet-owl",
                                "tone": "muted"
                            }
                        ],
                        "effort": "high",
                        "kind": "claude",
                        "message": "Bash(cargo test)",
                        "message_kind": "message",
                        "model": "opus",
                        "name": "corgi-quiet-owl",
                        "pane_id": "w-worker:p1",
                        "project_group": "corgi",
                        "task": "Pin the bar contract",
                        "tool": "cargo test",
                        "tool_kind": "command"
                    }
                ],
                "connected": true,
                "usage": [
                    {
                        "lines": [
                            {
                                "right": false,
                                "spans": [
                                    {
                                        "bold": true,
                                        "text": "Pro",
                                        "tone": "foreground"
                                    },
                                    {
                                        "bold": false,
                                        "text": " ",
                                        "tone": "foreground"
                                    },
                                    {
                                        "bold": false,
                                        "text": "3m ago",
                                        "tone": "muted"
                                    }
                                ]
                            },
                            {
                                "right": false,
                                "spans": [
                                    {
                                        "bold": false,
                                        "text": "5h     ",
                                        "tone": "muted"
                                    },
                                    {
                                        "bold": true,
                                        "text": "30%",
                                        "tone": "green"
                                    }
                                ]
                            },
                            {
                                "right": true,
                                "spans": [
                                    {
                                        "bold": false,
                                        "text": "2h 00m",
                                        "tone": "cyan"
                                    }
                                ]
                            },
                            {
                                "right": false,
                                "spans": [
                                    {
                                        "bold": false,
                                        "text": "Weekly   ",
                                        "tone": "muted"
                                    },
                                    {
                                        "bold": false,
                                        "text": "—",
                                        "tone": "muted"
                                    }
                                ]
                            },
                            {
                                "right": true,
                                "spans": [
                                    {
                                        "bold": false,
                                        "text": "—",
                                        "tone": "cyan"
                                    }
                                ]
                            }
                        ],
                        "provider": "Codex"
                    }
                ],
                "usage_notes": [
                    {
                        "provider": "Claude",
                        "reason": "no Claude Code login found"
                    }
                ]
            })
        );
    }

    /// The lines one popup action prints, from the agent `pane_id` of the
    /// fixture, answering with `input`.
    fn bar_action_lines(label: &str, pane_id: &str, action: &str, input: &[&str]) -> Vec<Value> {
        let (socket_path, server) = herdr_replying(label, bar_refresh_replies());
        let app = bar_app(&socket_path);
        let (tx, rx) = mpsc::channel();
        for line in input {
            tx.send((*line).to_string()).expect("queue popup input");
        }
        drop(tx);
        let mut output = Vec::new();
        bar_action_with(app, pane_id, action, &rx, &mut output).expect("run popup action");
        server.join().expect("fake server panicked");
        fs::remove_file(socket_path).expect("remove fake socket");
        String::from_utf8(output)
            .expect("popup output is UTF-8")
            .lines()
            .map(|line| serde_json::from_str(line).expect("each popup line is JSON"))
            .collect()
    }

    #[test]
    fn the_bar_close_action_keeps_the_omarchy_dialog_fields() {
        let lines = bar_action_lines("bar-close", "w-worker:p1", "x", &["escape"]);
        assert_eq!(
            Value::from(lines),
            json!([
                {
                    "confirm": "Remove worktree",
                    "lines": [
                        {
                            "left": false,
                            "spans": [
                                {
                                    "bold": false,
                                    "text": "Remove worktree corgi/quiet-owl?",
                                    "tone": "foreground"
                                }
                            ]
                        },
                        {
                            "left": false,
                            "spans": []
                        },
                        {
                            "left": false,
                            "spans": [
                                {
                                    "bold": false,
                                    "text": "/worktrees/corgi/quiet-owl",
                                    "tone": "foreground"
                                }
                            ]
                        },
                        {
                            "left": false,
                            "spans": [
                                {
                                    "bold": false,
                                    "text": "Stops its panes and deletes the checkout from disk; the branch is kept.",
                                    "tone": "foreground"
                                }
                            ]
                        },
                        {
                            "left": false,
                            "spans": []
                        },
                        {
                            "left": false,
                            "spans": [
                                {
                                    "bold": false,
                                    "text": "Enter remove · Esc cancel",
                                    "tone": "foreground"
                                }
                            ]
                        }
                    ],
                    "phase": "confirm",
                    "text": "Remove worktree corgi/quiet-owl?\n\n/worktrees/corgi/quiet-owl\nStops its panes and deletes the checkout from disk; the branch is kept.\n\nEnter remove · Esc cancel",
                    "title": "Remove worktree",
                    "tone": "red"
                },
                {
                    "phase": "done",
                    "text": "Cancelled",
                    "title": "Corgi"
                }
            ])
        );
    }

    #[test]
    fn the_bar_merge_action_reports_why_it_is_unavailable_as_done() {
        let lines = bar_action_lines("bar-merge", "w-main:p1", "m", &[]);
        assert_eq!(
            Value::from(lines),
            json!([
                {
                    "phase": "done",
                    "text": "Merge is available only for an agent in a linked worktree",
                    "title": "Corgi"
                }
            ])
        );
    }

    #[test]
    fn the_bar_new_options_use_the_forms_discovered_model_catalog() {
        let (socket_path, server) = herdr_replying("bar-new-options", bar_refresh_replies());
        let app = bar_app(&socket_path);
        let mut asked = Vec::new();
        let options = bar_new_options_value(
            app,
            "w-worker:p1",
            "opencode",
            "",
            &[Harness::Claude],
            |kind| {
                asked.push(kind.to_string());
                Ok(vec![
                    "openai/gpt-5.6-sol".into(),
                    "anthropic/claude-sonnet-4.5".into(),
                ])
            },
        )
        .expect("popup options");
        server.join().expect("fake server panicked");
        fs::remove_file(socket_path).expect("remove fake socket");

        assert_eq!(asked, ["opencode"]);
        assert_eq!(
            options,
            json!({
                "efforts": [
                    {
                        "badge": "",
                        "detail": "",
                        "label": "Harness default",
                        "value": ""
                    },
                    {
                        "badge": "",
                        "detail": "",
                        "label": "low",
                        "value": "low"
                    },
                    {
                        "badge": "",
                        "detail": "",
                        "label": "medium",
                        "value": "medium"
                    },
                    {
                        "badge": "",
                        "detail": "",
                        "label": "high",
                        "value": "high"
                    },
                    {
                        "badge": "",
                        "detail": "",
                        "label": "xhigh",
                        "value": "xhigh"
                    },
                    {
                        "badge": "",
                        "detail": "",
                        "label": "max",
                        "value": "max"
                    }
                ],
                "harnesses": [
                    {
                        "badge": "installed",
                        "detail": "",
                        "label": "claude",
                        "value": "claude"
                    },
                    {
                        "badge": "",
                        "detail": "",
                        "label": "codex",
                        "value": "codex"
                    },
                    {
                        "badge": "",
                        "detail": "",
                        "label": "gemini",
                        "value": "gemini"
                    },
                    {
                        "badge": "",
                        "detail": "",
                        "label": "copilot",
                        "value": "copilot"
                    },
                    {
                        "badge": "",
                        "detail": "",
                        "label": "opencode",
                        "value": "opencode"
                    }
                ],
                "kind": "opencode",
                "models": [
                    {
                        "badge": "",
                        "detail": "",
                        "label": "Harness default",
                        "value": ""
                    },
                    {
                        "badge": "",
                        "detail": "",
                        "label": "openai/gpt-5.6-sol",
                        "value": "openai/gpt-5.6-sol"
                    },
                    {
                        "badge": "",
                        "detail": "",
                        "label": "anthropic/claude-sonnet-4.5",
                        "value": "anthropic/claude-sonnet-4.5"
                    }
                ],
                "project": "/repos/corgi",
                "projects": [
                    "/repos/corgi"
                ],
                "supportsEffort": false
            })
        );
    }
}
