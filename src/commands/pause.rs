//! `corgi pause --json` and `corgi resume --json`: the Herdr app's
//! "⏸ Agents" and "▶ Agents" (`crate::hc::pause`), and what
//! `corgi status --json` says of a pause.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};

use crate::{
    hc::{
        self, TASK_LABEL,
        pause::{self, NOTHING_PAUSED, NOTHING_WORKING, Outcome, Record},
    },
    progress::Silent,
};

use crate::time::unix_now;

pub const PAUSE_USAGE: &str = "\
Usage: corgi pause [--json]

Pauses the agents Corgi runs on herdr-core (hc) that are working at this
moment, before the network goes: each is interrupted with Esc, which ends its
turn, then exits through its harness's own exit (/exit, /quit), so its
conversation is saved. Agents that are idle, done or blocked are left as they
are, and with none working nothing changes. Only an agent Corgi can resume is
paused (one whose terminal holds its resume line); nothing is ever killed: an
agent that keeps working, holds text in its input box or does not exit is left
running and named. While any agent is paused, corgi daemon types nothing into
any supervisor and moves no handover on; wakes wait in the inboxes until
corgi resume. The pause outlasts a restart of the core, the app and the
daemon.

With --json it prints {\"result\", \"message\", \"paused\", \"failed\",
\"left\"}: result is paused, partial, failed, nothing (no agent was working)
or refused (hc is not running); paused names the agents stopped, failed each
one that was tried and left running ({\"name\", \"why\"}), left each working
agent that was not tried. It exits 0 unless it was refused or every agent
tried failed.";

pub const RESUME_USAGE: &str = "\
Usage: corgi resume [--json]

Brings back exactly the agents corgi pause stopped on herdr-core (hc), each on
its own conversation: a supervisor in its kept shell, a worker in a new
terminal with its name, labels and space, its finished one removed. An agent
that does not come back is named, and holds nothing any more. Once corgi
daemon sees them back, the wakes that waited go out.

With --json it prints {\"result\", \"message\", \"resumed\", \"failed\",
\"left\"}: result is resumed, partial, failed, nothing (no agent was paused) or
refused (hc is not running). It exits 0 unless it was refused or every agent
failed.";

/// The pause file of this environment's core.
fn record_path() -> Result<PathBuf> {
    hc::expected_socket()
        .and_then(|socket| pause::record_path(&socket))
        .context("Corgi has no state directory for the pause (HOME is not set)")
}

/// What `corgi status --json` says of the pause: its own object, and each
/// paused agent's row with state `paused`.
pub(crate) fn paused_status() -> Record {
    record_path()
        .map(|path| pause::read(&path))
        .unwrap_or_default()
}

/// `corgi pause`.
pub fn pause_command(args: &[String]) -> Result<()> {
    let json_output = parse(args, PAUSE_USAGE)?;
    let outcome = run("a pause", |client, path| {
        pause::pause(
            client,
            path,
            hc::calling_terminal().as_deref(),
            unix_now(),
            &mut Silent,
        )
    });
    finish(outcome, "paused", "Paused", NOTHING_WORKING, json_output)
}

/// `corgi resume`.
pub fn resume_command(args: &[String]) -> Result<()> {
    let json_output = parse(args, RESUME_USAGE)?;
    let outcome = run("a resume", |client, path| {
        pause::resume(client, path, unix_now(), &mut Silent)
    });
    finish(outcome, "resumed", "Resumed", NOTHING_PAUSED, json_output)
}

fn parse(args: &[String], usage: &str) -> Result<bool> {
    let mut json_output = false;
    for arg in args {
        match arg.as_str() {
            "--json" => json_output = true,
            other => bail!("unknown option {other:?}\n\n{usage}"),
        }
    }
    Ok(json_output)
}

/// Runs `act` on hc when its core runs; a refusal saying why otherwise. A
/// core that does not run is not started: any `hc` call would start one.
fn run(
    what: &str,
    act: impl FnOnce(&hc::HcClient, &Path) -> Result<Outcome>,
) -> std::result::Result<Outcome, String> {
    if !hc::core_running() {
        return Err(format!(
            "herdr-core (hc) is not running; Corgi does not start it for {what}"
        ));
    }
    let client = hc::HcClient::from_env().map_err(|error| format!("{error:#}"))?;
    let path = record_path().map_err(|error| format!("{error:#}"))?;
    act(&client, &path).map_err(|error| format!("{error:#}"))
}

fn finish(
    outcome: std::result::Result<Outcome, String>,
    done: &'static str,
    verb: &str,
    nothing: &str,
    json_output: bool,
) -> Result<()> {
    let (value, succeeded) = match &outcome {
        Ok(outcome) => (
            outcome.json(done, verb, nothing),
            outcome.result(done) != "failed",
        ),
        Err(message) => {
            let mut value =
                json!({"result": "refused", "message": message, "failed": [], "left": []});
            value[done] = json!([]);
            (value, false)
        }
    };
    if json_output {
        println!("{}", serde_json::to_string(&value)?);
    } else if succeeded {
        println!("{}", value["message"].as_str().unwrap_or_default());
    } else {
        eprintln!("{}", value["message"].as_str().unwrap_or_default());
    }
    if !succeeded {
        std::process::exit(1);
    }
    Ok(())
}

/// Puts the pause `record` into `value`, a `corgi status --json` object:
/// `paused` ({"any", "agents"}), and each paused agent in its project, with
/// state `paused` and `"paused": true`. A worker's terminal finished with its
/// agent, so it is added from the pause file, with its old terminal.
pub(crate) fn add_to_status(value: &mut Value, record: &Record) {
    value["paused"] = pause::status_value(record);
    let Some(projects) = value["projects"].as_array_mut() else {
        return;
    };
    for paused in record.agents.iter().filter(|paused| paused.is_paused()) {
        let Some(project) = projects
            .iter_mut()
            .find(|project| project["dir"] == paused.project)
        else {
            continue;
        };
        let is_it = |agent: &Value| {
            agent["terminal"] == paused.address.as_str() || agent["name"] == paused.name.as_str()
        };
        let mark = |agent: &mut Value| {
            agent["state"] = json!("paused");
            agent["paused"] = json!(true);
        };
        if project["supervisor"].is_object() && is_it(&project["supervisor"]) {
            mark(&mut project["supervisor"]);
        }
        let Some(agents) = project["agents"].as_array_mut() else {
            continue;
        };
        match agents.iter_mut().find(|agent| is_it(agent)) {
            Some(agent) => mark(agent),
            None => {
                let worktree = (!Path::new(&paused.cwd).starts_with(&paused.project))
                    .then(|| paused.cwd.clone());
                agents.push(json!({
                    "name": paused.name,
                    "role": if paused.supervisor { "supervisor" } else { "worker" },
                    "state": "paused",
                    "paused": true,
                    "task": paused.labels.get(TASK_LABEL).cloned().unwrap_or_else(|| paused.name.clone()),
                    "model": null,
                    "harness": paused.harness,
                    "ctx_pct": null,
                    "cwd": paused.cwd,
                    "worktree": worktree,
                    "tag": null,
                    "terminal": paused.address,
                }));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hc::pause::{PausedAgent, Phase};

    fn paused(name: &str, address: &str, supervisor: bool, phase: Phase) -> PausedAgent {
        PausedAgent {
            address: address.into(),
            name: name.into(),
            project: "/repos/weather".into(),
            supervisor,
            harness: "claude".into(),
            session: Some("s".into()),
            resume: "claude --resume s".into(),
            cwd: format!("/w/{name}"),
            labels: [(TASK_LABEL.to_string(), "Fix the radar".to_string())].into(),
            phase,
            at: 1,
        }
    }

    #[test]
    fn status_shows_paused_agents_in_their_projects_and_whether_any_is() {
        let supervisor = json!({"name": "weather-supervisor", "role": "supervisor", "state": "exited", "terminal": "t1"});
        let mut value = json!({"projects": [{
            "dir": "/repos/weather",
            "supervisor": supervisor,
            "agents": [supervisor, {"name": "w-cache", "role": "worker", "state": "idle", "terminal": "t3"}],
        }]});
        let record = Record {
            agents: vec![
                paused("weather-supervisor", "t1", true, Phase::Paused),
                paused("w-radar", "t2", false, Phase::Paused),
                paused("w-back", "t9", false, Phase::Resuming),
            ],
        };
        add_to_status(&mut value, &record);
        assert_eq!(value["paused"]["any"], true);
        assert_eq!(value["paused"]["agents"].as_array().unwrap().len(), 2);
        let project = &value["projects"][0];
        assert_eq!(project["supervisor"]["state"], "paused");
        let agents = project["agents"].as_array().unwrap();
        assert_eq!(agents.len(), 3);
        assert_eq!(agents[0]["state"], "paused");
        assert_eq!(agents[1]["state"], "idle");
        assert!(agents[1].get("paused").is_none());
        assert_eq!(agents[2]["name"], "w-radar");
        assert_eq!(agents[2]["state"], "paused");
        assert_eq!(agents[2]["terminal"], "t2");
        assert_eq!(agents[2]["task"], "Fix the radar");
        assert_eq!(agents[2]["worktree"], "/w/w-radar");

        let mut none = json!({"projects": []});
        add_to_status(&mut none, &Record::default());
        assert_eq!(none["paused"], json!({"any": false, "agents": []}));
    }
}
