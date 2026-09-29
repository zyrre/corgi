//! Delivering a new agent's first prompt and confirming it arrived, waiting
//! out startup, and answering the folder-trust question of a Corgi project.

use std::{
    fs,
    path::Path,
    thread,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, bail};

use crate::{
    activity::{MAX_MESSAGE_CHARS, describe, folder_trust_dialog},
    git::trust_root,
    herdr::{HerdrClient, HerdrError, ReadSource},
    model::{ActivityKind, AgentInfo, AgentState},
    projects::is_created_project,
    session::SessionReader,
};

use super::{progress::Progress, rows::has_real_agent_session};

const FIRST_PROMPT_ATTEMPTS: usize = 30;
const FIRST_PROMPT_RETRY_DELAY: Duration = Duration::from_millis(300);
/// How long a new agent that opened on a question for the user, such as
/// whether to trust its directory, may wait for the answer before its first
/// prompt is given up.
const FIRST_PROMPT_ANSWER_WAIT: Duration = Duration::from_secs(10 * 60);
const FIRST_PROMPT_ANSWER_POLL: Duration = Duration::from_millis(500);
/// How long Claude Code may take to leave its folder-trust question once
/// Corgi has answered it.
const FOLDER_TRUST_SETTLE: Duration = Duration::from_secs(10);
/// How long a delivered first prompt may take to show up before it is sent
/// again: about six seconds. A multi-line prompt is only a collapsed paste
/// placeholder on screen until the agent records the turn, which a session
/// starting with a large system prompt can take several seconds to do, and a
/// resend then lands as a duplicate.
const FIRST_PROMPT_VERIFY_ATTEMPTS: usize = 24;
const FIRST_PROMPT_VERIFY_DELAY: Duration = Duration::from_millis(250);

pub(super) fn prompt_started_agent(
    client: &HerdrClient,
    pane_id: &str,
    prompt: &str,
    name: &str,
    progress: &mut dyn Progress,
    mut accepted: impl FnMut(&AgentInfo) -> Result<()>,
) -> Result<()> {
    // The first prompt can be acknowledged while a newly opened agent is
    // still replacing its startup screen. Confirm that it reached the pane
    // before telling the user the launch succeeded.
    for delivery in 1..=2 {
        let agent = send_first_prompt(client, pane_id, prompt, name, progress)?;
        accepted(&agent)?;
        for _ in 0..FIRST_PROMPT_VERIFY_ATTEMPTS {
            if first_prompt_visible(client, pane_id, prompt) {
                return Ok(());
            }
            thread::sleep(FIRST_PROMPT_VERIFY_DELAY);
        }
        if delivery == 1 {
            progress.report(format!("Retrying {name}'s first prompt after startup…"));
        }
    }
    bail!("first prompt was acknowledged but never appeared in {name}'s session")
}

fn send_first_prompt(
    client: &HerdrClient,
    pane_id: &str,
    prompt: &str,
    name: &str,
    progress: &mut dyn Progress,
) -> Result<AgentInfo> {
    let mut attempt = 1;
    let mut trust_tried = false;
    loop {
        match client.prompt_agent_confirmed(pane_id, prompt) {
            Ok(agent) => return Ok(agent),
            // A new agent can open on a question. Corgi answers exactly one:
            // whether to trust the directory of a project it created itself.
            // Any other question is the user's; wait for their answer, which
            // is not a failed attempt.
            Err(error) if HerdrError::is_agent_blocked(&error) => {
                let trusted =
                    !trust_tried && accept_corgi_folder_trust(client, pane_id, name, progress);
                trust_tried = true;
                if !trusted {
                    wait_until_answered(client, pane_id, name, progress)?;
                }
            }
            Err(error)
                if HerdrError::is_transient_startup(&error) && attempt < FIRST_PROMPT_ATTEMPTS =>
            {
                progress.report(format!(
                    "Waiting for {name} to accept its first prompt… ({attempt}/{FIRST_PROMPT_ATTEMPTS})"
                ));
                thread::sleep(FIRST_PROMPT_RETRY_DELAY);
                attempt += 1;
            }
            Err(error) => return Err(error).context("send first prompt after agent startup"),
        }
    }
}

/// Answers Claude Code's folder-trust question with Yes, and does nothing
/// else: only when exactly that question is on screen, about the agent's own
/// directory, in a project Corgi created. Returns whether the question was
/// answered and the agent moved on; every other case stays with the user.
fn accept_corgi_folder_trust(
    client: &HerdrClient,
    pane_id: &str,
    name: &str,
    progress: &mut dyn Progress,
) -> bool {
    let dialog = || {
        client
            .read_agent(pane_id, ReadSource::Visible, Some(80))
            .ok()
            .and_then(|read| folder_trust_dialog(&read.text))
    };
    let Some(question) = dialog() else {
        return false;
    };
    let asked = Path::new(&question.path);
    let own_directory = client.snapshot().ok().is_some_and(|snapshot| {
        snapshot
            .agents
            .iter()
            .find(|agent| agent.pane_id == pane_id)
            .is_some_and(|agent| same_directory(Path::new(agent.cwd()), asked))
    });
    if !own_directory || !is_created_project(&trust_root(asked)) {
        return false;
    }
    progress.report(format!(
        "Trusting {} for {name}: Corgi created this project…",
        asked.display()
    ));
    if !question.yes_selected {
        if client.send_agent_keys(pane_id, &["down"]).is_err() {
            return false;
        }
        thread::sleep(FIRST_PROMPT_ANSWER_POLL);
        // Confirm only an answer that is visibly "Yes" for the same folder.
        if !dialog().is_some_and(|now| now.yes_selected && now.path == question.path) {
            return false;
        }
    }
    if client.send_agent_keys(pane_id, &["enter"]).is_err() {
        return false;
    }
    let deadline = Instant::now() + FOLDER_TRUST_SETTLE;
    while Instant::now() < deadline {
        thread::sleep(FIRST_PROMPT_ANSWER_POLL);
        if dialog().is_none() {
            return true;
        }
    }
    false
}

fn same_directory(left: &Path, right: &Path) -> bool {
    match (fs::canonicalize(left), fs::canonicalize(right)) {
        (Ok(left), Ok(right)) => left == right,
        _ => left == right,
    }
}

/// Waits while the agent in `pane_id` is blocked on a question for the user,
/// saying so, and gives up after [`FIRST_PROMPT_ANSWER_WAIT`].
fn wait_until_answered(
    client: &HerdrClient,
    pane_id: &str,
    name: &str,
    progress: &mut dyn Progress,
) -> Result<()> {
    progress.report(format!(
        "{name} is asking you something in its pane, such as whether to trust this \
         directory. Answer it there, and Corgi sends the first prompt."
    ));
    let deadline = Instant::now() + FIRST_PROMPT_ANSWER_WAIT;
    while Instant::now() < deadline {
        thread::sleep(FIRST_PROMPT_ANSWER_POLL);
        let blocked = client.snapshot().is_ok_and(|snapshot| {
            snapshot
                .agents
                .iter()
                .any(|agent| agent.pane_id == pane_id && agent.state == AgentState::Blocked)
        });
        if !blocked {
            return Ok(());
        }
    }
    bail!("{name} is still waiting for an answer in its pane; its first prompt was not sent")
}

fn first_prompt_visible(client: &HerdrClient, pane_id: &str, prompt: &str) -> bool {
    let expected = prompt.split_whitespace().collect::<Vec<_>>().join(" ");
    if client
        .read_agent(pane_id, ReadSource::RecentUnwrapped, Some(200))
        .is_ok_and(|read| {
            read.text
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
                .contains(&expected)
        })
    {
        return true;
    }
    let Ok(snapshot) = client.snapshot() else {
        return false;
    };
    let Some(agent) = snapshot
        .agents
        .iter()
        .find(|agent| agent.pane_id == pane_id)
    else {
        return false;
    };
    if !has_real_agent_session(agent) {
        return false;
    }
    SessionReader::default().facts(agent).task.as_deref()
        == Some(
            describe(ActivityKind::Prompt, prompt, MAX_MESSAGE_CHARS)
                .text
                .as_str(),
        )
}

#[cfg(test)]
mod tests {
    use std::fs;

    use serde_json::json;

    use crate::{
        app::test_helpers::Recorded,
        herdr::HerdrClient,
        test_support::{answer, fake_herdr},
    };

    use super::*;

    #[test]
    fn first_prompt_retries_until_the_started_agent_is_ready() {
        let (socket_path, server) = fake_herdr("prompt-retry", move |listener| {
            for ready in [false, true, true] {
                answer(&listener, |request| {
                    if request["method"] == "agent.read" {
                        assert_eq!(request["params"]["target"], "w9:p1");
                        return json!({
                            "result": { "type": "pane_read", "read": { "text": "› Build it" } }
                        });
                    }
                    assert_eq!(request["method"], "agent.prompt");
                    assert_eq!(request["params"]["target"], "w9:p1");
                    assert_eq!(request["params"]["text"], "Build it");
                    assert_eq!(request["params"]["wait"]["timeout_ms"], 7000);
                    assert_eq!(
                        request["params"]["wait"]["until"],
                        json!(["working", "blocked", "done", "idle"])
                    );

                    if ready {
                        json!({
                            "result": {
                                "type": "agent_prompted",
                                "agent": {
                                    "agent": "codex",
                                    "agent_status": "working",
                                    "pane_id": "w9:p1",
                                    "workspace_id": "w9",
                                    "tab_id": "w9:t1"
                                }
                            }
                        })
                    } else {
                        json!({
                            "error": {
                                "code": "agent_not_ready",
                                "message": "agent is still launching"
                            }
                        })
                    }
                });
            }
        });

        let client = HerdrClient::from_socket_path(&socket_path);
        let mut progress = Recorded::default();
        prompt_started_agent(
            &client,
            "w9:p1",
            "Build it",
            "corgi-worker",
            &mut progress,
            |_| Ok(()),
        )
        .expect("eventually prompt agent");
        assert!(!progress.0.is_empty());

        server.join().expect("fake server panicked");
        fs::remove_file(socket_path).expect("remove fake socket");
    }

    #[test]
    fn first_prompt_is_resent_when_startup_acknowledges_but_drops_it() {
        let (socket_path, server) = fake_herdr("dropped-prompt", move |listener| {
            let mut prompts = 0;
            let mut reads = 0;
            loop {
                answer(&listener, |request| {
                    let result = match request["method"].as_str().expect("method") {
                        "agent.prompt" => {
                            prompts += 1;
                            assert_eq!(request["params"]["text"], "Build it");
                            json!({"type": "agent_prompted", "agent": {
                                "agent": "codex", "agent_status": "idle", "pane_id": "w9:p1",
                                "workspace_id": "w9", "tab_id": "w9:t1"
                            }})
                        }
                        "agent.read" => {
                            reads += 1;
                            json!({"type": "pane_read", "read": {
                                "text": if prompts == 1 { "› Ask Codex to do anything" } else { "› Build it" }
                            }})
                        }
                        "session.snapshot" => json!({"type": "session_snapshot", "snapshot": {}}),
                        other => panic!("unexpected method {other}"),
                    };
                    json!({"result": result})
                });
                if prompts == 2 && reads > FIRST_PROMPT_VERIFY_ATTEMPTS {
                    break;
                }
            }
            assert_eq!(prompts, 2);
        });

        let client = HerdrClient::from_socket_path(&socket_path);
        let mut progress = Recorded::default();
        prompt_started_agent(
            &client,
            "w9:p1",
            "Build it",
            "corgi-worker",
            &mut progress,
            |_| Ok(()),
        )
        .expect("resend dropped prompt");
        assert!(
            progress
                .0
                .iter()
                .any(|message| message.contains("Retrying"))
        );
        server.join().expect("fake server panicked");
        fs::remove_file(socket_path).expect("remove fake socket");
    }
}
