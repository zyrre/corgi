use super::{claude::*, codex::*, locate::*, tail::*, text::*, *};
use crate::{
    activity::MAX_MESSAGE_CHARS,
    model::{ActivityKind, AgentSession, PromptCacheKind},
    time::parse_rfc3339,
};
use std::{
    fs::{self, File},
    io::Write,
    ops::Deref,
    path::Path,
    sync::atomic::{AtomicUsize, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

/// A directory of the test's own, removed when the test ends, a panic
/// included. The process ID, a counter, and the clock keep parallel runs,
/// from this checkout or a sibling worktree, out of each other's files.
struct TempDir(PathBuf);

impl TempDir {
    fn new(label: &str) -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|elapsed| elapsed.as_nanos())
            .unwrap_or_default();
        let path = std::env::temp_dir().join(format!(
            "corgi-{label}-{}-{}-{stamp}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).expect("create temp dir");
        Self(path)
    }
}

impl Deref for TempDir {
    type Target = Path;

    fn deref(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).ok();
    }
}

fn claude_facts(path: &Path, configured_model: Option<&str>) -> SessionFacts {
    ClaudeFile::default().facts(path, configured_model)
}

fn codex_facts(path: &Path) -> SessionFacts {
    CodexFile::default().facts(path)
}

fn claude_task(path: &Path) -> Option<String> {
    let mut head = Head::default();
    head.update(path, TASK_HEAD_BYTES, super::claude::claude_task);
    head.found
}

fn entries(path: &Path, events: Events, limit: usize) -> Vec<Activity> {
    let mut tail = Tail::<Entries>::default();
    tail.update(path, &[TRANSCRIPT_TAIL_BYTES], |record| {
        Entries::of(record, events)
    });
    tail.found().newest_first().take(limit).cloned().collect()
}

fn claude_entries(path: &Path, limit: usize) -> Vec<Activity> {
    entries(path, claude_events, limit)
}

fn codex_entries(path: &Path, limit: usize) -> Vec<Activity> {
    entries(path, codex_events, limit)
}

fn write(path: &Path, lines: &[&str]) {
    let mut file = File::create(path).expect("create session file");
    for line in lines {
        writeln!(file, "{line}").expect("write record");
    }
}

#[test]
fn a_claude_transcript_reports_the_model_and_the_live_context_share() {
    let directory = TempDir::new("claude-transcript");
    let path = directory.join("session.jsonl");
    write(
        &path,
        &[
            r#"{"type":"user","message":{"role":"user"}}"#,
            r#"{"type":"assistant","message":{"model":"claude-opus-5","usage":{"input_tokens":2,"cache_creation_input_tokens":1000,"cache_read_input_tokens":19000,"output_tokens":500}}}"#,
            r#"{"type":"last-prompt","lastPrompt":"unrelated"}"#,
        ],
    );

    // Without a long-context model configured the session fills the
    // default window.
    assert_eq!(
        claude_facts(&path, None),
        SessionFacts {
            model: Some("Opus 5".into()),
            context_percent: Some(10),
            context_tokens: Some(20_002),
            ..SessionFacts::default()
        }
    );
    // The configured variant is the only thing that reveals a long-context
    // session before it passes the default window.
    assert_eq!(
        claude_facts(&path, Some("opus[1m]")),
        SessionFacts {
            model: Some("Opus 5 1M".into()),
            context_percent: Some(2),
            context_tokens: Some(20_002),
            ..SessionFacts::default()
        }
    );
    // A configured variant of another family says nothing about this one.
    assert_eq!(
        claude_facts(&path, Some("sonnet[1m]")).context_percent,
        Some(10)
    );
}

#[test]
fn a_pasted_multi_line_prompt_is_the_task_not_injected_context() {
    let directory = TempDir::new("claude-pasted-prompt");
    let path = directory.join("session.jsonl");
    write(
        &path,
        &[
            r#"{"type":"user","message":{"role":"user","content":"<system-reminder>context</system-reminder>"}}"#,
            r#"{"type":"user","message":{"role":"user","content":"\n\n<pasted_content id=\"e524\">\nStart the session.\nRead the role.\n</pasted_content>"}}"#,
        ],
    );

    assert_eq!(
        claude_task(&path).as_deref(),
        Some("Start the session. Read the role.")
    );
    assert_eq!(
        claude_facts(&path, None)
            .message
            .map(|message| message.text),
        Some("Start the session. Read the role.".into())
    );
    assert_eq!(
        claude_entries(&path, 10)
            .first()
            .map(|entry| entry.text.as_str()),
        Some("Start the session.\nRead the role.")
    );
}

#[test]
fn a_claude_transcript_says_when_its_prompt_cache_lapses() {
    let directory = TempDir::new("claude-cache");
    let path = directory.join("session.jsonl");
    // The first turn wrote the cache for an hour; the newest turn only
    // read it, and the tool result after that is a request in flight.
    write(
        &path,
        &[
            r#"{"type":"assistant","timestamp":"2026-09-11T18:00:00Z","message":{"model":"claude-opus-5","usage":{"input_tokens":2,"cache_creation_input_tokens":19000,"cache_read_input_tokens":0,"cache_creation":{"ephemeral_5m_input_tokens":0,"ephemeral_1h_input_tokens":19000}}}}"#,
            r#"{"type":"assistant","timestamp":"2026-09-11T18:10:00Z","message":{"model":"claude-opus-5","usage":{"input_tokens":2,"cache_creation_input_tokens":0,"cache_read_input_tokens":20000,"cache_creation":{"ephemeral_5m_input_tokens":0,"ephemeral_1h_input_tokens":0}}}}"#,
            r#"{"type":"assistant","isSidechain":true,"timestamp":"2026-09-11T18:12:00Z","message":{"model":"claude-haiku-4-5-20251001","usage":{"input_tokens":2,"cache_creation_input_tokens":500,"cache_read_input_tokens":0,"cache_creation":{"ephemeral_5m_input_tokens":500,"ephemeral_1h_input_tokens":0}}}}"#,
            r#"{"type":"user","timestamp":"2026-09-11T18:11:00Z","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"t1","content":"ok"}]}}"#,
        ],
    );

    let facts = claude_facts(&path, None);
    let refreshed_at = parse_rfc3339("2026-09-11T18:11:00Z").expect("timestamp");
    assert_eq!(
        facts.cache,
        Some(PromptCache {
            expires_at: refreshed_at + CLAUDE_CACHE_1H,
            tokens: 20_002,
            kind: PromptCacheKind::Exact,
        })
    );

    // Dropping to the five-minute lifetime mid-session shows at once.
    write(
        &path,
        &[
            r#"{"type":"assistant","timestamp":"2026-09-11T18:00:00Z","message":{"model":"claude-opus-5","usage":{"input_tokens":2,"cache_creation_input_tokens":19000,"cache_read_input_tokens":0,"cache_creation":{"ephemeral_5m_input_tokens":0,"ephemeral_1h_input_tokens":19000}}}}"#,
            r#"{"type":"assistant","timestamp":"2026-09-11T18:10:00Z","message":{"model":"claude-opus-5","usage":{"input_tokens":2,"cache_creation_input_tokens":300,"cache_read_input_tokens":19000,"cache_creation":{"ephemeral_5m_input_tokens":300,"ephemeral_1h_input_tokens":0}}}}"#,
        ],
    );
    let refreshed_at = parse_rfc3339("2026-09-11T18:10:00Z").expect("timestamp");
    assert_eq!(
        claude_facts(&path, None)
            .cache
            .map(|cache| cache.expires_at),
        Some(refreshed_at + CLAUDE_CACHE_5M)
    );

    // A transcript from a CLI that never recorded a lifetime says nothing
    // rather than guessing one.
    write(
        &path,
        &[
            r#"{"type":"assistant","timestamp":"2026-09-11T18:00:00Z","message":{"model":"claude-opus-5","usage":{"input_tokens":2,"cache_creation_input_tokens":1000,"cache_read_input_tokens":19000}}}"#,
        ],
    );
    assert_eq!(claude_facts(&path, None).cache, None);
}

#[test]
fn a_claude_transcript_reports_the_effort_recorded_beside_the_turn() {
    let directory = TempDir::new("claude-effort");
    let path = directory.join("session.jsonl");
    write(
        &path,
        &[
            r#"{"type":"assistant","effort":"high","message":{"model":"claude-opus-5","usage":{"input_tokens":2,"cache_creation_input_tokens":1000,"cache_read_input_tokens":19000,"output_tokens":500}}}"#,
            r#"{"type":"assistant","effort":"xhigh","message":{"model":"claude-opus-5","usage":{"input_tokens":2,"cache_creation_input_tokens":1000,"cache_read_input_tokens":19000,"output_tokens":500}}}"#,
        ],
    );

    // The newest turn decides, so switching effort mid-session shows.
    assert_eq!(
        claude_facts(&path, None),
        SessionFacts {
            model: Some("Opus 5".into()),
            effort: Some("xhigh".into()),
            context_percent: Some(10),
            context_tokens: Some(20_002),
            ..SessionFacts::default()
        }
    );
}

#[test]
fn a_session_past_the_default_window_must_be_a_long_context_one() {
    assert_eq!(
        claude_context_window("claude-opus-5", None, 20_000),
        CLAUDE_DEFAULT_WINDOW
    );
    assert_eq!(
        claude_context_window("claude-opus-5", None, 220_000),
        CLAUDE_LONG_WINDOW
    );
    // Fable answers in a long window without being asked for one.
    assert_eq!(
        claude_context_window("claude-fable-5-1", None, 20_000),
        CLAUDE_LONG_WINDOW
    );
    assert_eq!(
        claude_model_name("claude-fable-5-1", CLAUDE_LONG_WINDOW),
        "Fable 5.1"
    );
    assert_eq!(
        claude_model_name("claude-haiku-4-5-20251001", CLAUDE_DEFAULT_WINDOW),
        "Haiku 4.5"
    );
}

#[test]
fn a_session_without_an_id_falls_back_to_the_newest_transcript_of_its_directory() {
    let projects = TempDir::new("claude-projects");
    let project = projects.join("-repos-corgi");
    fs::create_dir_all(&project).expect("create temp project dir");
    let older = project.join("older.jsonl");
    let newer = project.join("newer.jsonl");
    write(&older, &[r#"{"type":"user"}"#]);
    write(&newer, &[r#"{"type":"user"}"#]);
    filetime(&newer);

    assert_eq!(newest_file(&project), Some(newer));
    // A directory with no transcript at all stays unresolved rather than
    // borrowing another session's numbers.
    assert_eq!(newest_file(&projects), None);
}

/// Marks a file as the most recently written one, whatever order the file
/// system recorded the writes in.
fn filetime(path: &Path) {
    let mut file = fs::OpenOptions::new()
        .append(true)
        .open(path)
        .expect("touch transcript");
    std::thread::sleep(Duration::from_millis(20));
    writeln!(file, r#"{{"type":"user"}}"#).expect("append record");
}

#[test]
fn a_codex_rollout_reports_the_model_and_the_window_it_records() {
    let directory = TempDir::new("codex-rollout");
    let path = directory.join("rollout-2026-09-10T09-00-00-thread.jsonl");
    write(
        &path,
        &[
            r#"{"type":"session_meta","payload":{"id":"thread","cwd":"/repos/corgi","model":"gpt-5-codex"}}"#,
            r#"{"type":"turn_context","collaboration_mode":{"settings":{"reasoning_effort":"xhigh"}}}"#,
            r#"{"type":"event_msg","payload":{"type":"token_count","info":{"model_context_window":272000,"last_token_usage":{"input_tokens":60000,"cached_input_tokens":50000,"output_tokens":1600,"total_tokens":61600}}}}"#,
        ],
    );

    assert_eq!(
        codex_facts(&path),
        SessionFacts {
            model: Some("gpt-5-codex".into()),
            effort: Some("xhigh".into()),
            context_percent: Some(23),
            context_tokens: Some(61_600),
            ..SessionFacts::default()
        }
    );
    assert_eq!(codex_rollout_cwd(&path).as_deref(), Some("/repos/corgi"));
}

#[test]
fn a_gpt_5_6_rollout_estimates_cache_warmth_from_the_newest_hit() {
    let directory = TempDir::new("codex-cache");
    let path = directory.join("rollout-2026-09-11T18-00-00-thread.jsonl");
    write(
        &path,
        &[
            r#"{"type":"session_meta","payload":{"id":"thread","model":"gpt-5.6-terra"}}"#,
            r#"{"timestamp":"2026-09-11T18:00:00Z","type":"token_usage_record","payload":{"usage":{"input_tokens":10000,"cached_input_tokens":0,"cache_write_input_tokens":9000}}}"#,
            r#"{"timestamp":"2026-09-11T18:03:00Z","type":"event_msg","payload":{"type":"token_count","info":{"last_token_usage":{"input_tokens":12000,"cached_input_tokens":9984,"cache_write_input_tokens":0}}}}"#,
        ],
    );

    let refreshed_at = parse_rfc3339("2026-09-11T18:03:00Z").expect("timestamp");
    assert_eq!(
        codex_facts(&path).cache,
        Some(PromptCache {
            expires_at: refreshed_at + CODEX_CACHE_5_6,
            tokens: 12_000,
            kind: PromptCacheKind::Estimated,
        })
    );
    assert!(codex_has_documented_cache_minimum("gpt-5.6-terra"));
    assert!(!codex_has_documented_cache_minimum("gpt-5-codex"));
}

fn activity(kind: ActivityKind, text: &str) -> Option<Activity> {
    Some(Activity {
        kind,
        text: text.into(),
    })
}

#[test]
fn a_claude_transcript_reports_what_was_said_last_and_the_newest_tool_call_in_full() {
    let directory = TempDir::new("claude-conversation");
    let path = directory.join("session.jsonl");
    let long_command = format!("cargo test -- {}", "very_long_test_name ".repeat(12));
    write(
        &path,
        &[
            r#"{"type":"user","message":{"role":"user","content":"Fix the failing merge test"}}"#,
            r#"{"type":"user","isMeta":true,"message":{"role":"user","content":"Caveat: injected by the CLI"}}"#,
            r#"{"type":"assistant","message":{"model":"claude-opus-5","content":[{"type":"thinking","thinking":"The test compares paths."}],"usage":{"input_tokens":2}}}"#,
            r#"{"type":"assistant","message":{"model":"claude-opus-5","content":[{"type":"text","text":"I'll run the suite first."}],"usage":{"input_tokens":2}}}"#,
            &format!(
                r#"{{"type":"assistant","message":{{"model":"claude-opus-5","content":[{{"type":"tool_use","id":"t1","name":"Bash","input":{{"command":"{long_command}","description":"Run tests"}}}}],"usage":{{"input_tokens":2}}}}}}"#
            ),
            r#"{"type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"t1","content":"test result: FAILED"}]}}"#,
            r#"{"type":"user","isSidechain":true,"message":{"role":"user","content":"Subagent prompt that is not the conversation"}}"#,
            r#"{"type":"assistant","message":{"model":"claude-opus-5","content":[{"type":"thinking","thinking":""},{"type":"text","text":"Two tests fail. Should I fix both?"}],"usage":{"input_tokens":2}}}"#,
        ],
    );

    let facts = claude_facts(&path, None);
    // The assistant spoke last, and asked something.
    assert_eq!(
        facts.message,
        activity(ActivityKind::Question, "Two tests fail. Should I fix both?")
    );
    // The command is the whole command, not what fits on the screen.
    assert_eq!(
        facts.tool,
        activity(ActivityKind::Command, long_command.trim_end())
    );
    assert_eq!(facts.model.as_deref(), Some("Opus 5"));
    assert_eq!(facts.task.as_deref(), Some("Fix the failing merge test"));

    // The user's turn shows as soon as it is written, before any reply.
    write(
        &path,
        &[
            r#"{"type":"user","message":{"role":"user","content":[{"type":"text","text":"<system-reminder>injected</system-reminder>"},{"type":"text","text":"Yes, fix both"}]}}"#,
            r#"{"type":"assistant","message":{"model":"claude-opus-5","content":[{"type":"tool_use","id":"t2","name":"Edit","input":{"file_path":"src/app.rs","old_string":"a","new_string":"b"}}],"usage":{"input_tokens":2}}}"#,
        ],
    );
    let facts = claude_facts(&path, None);
    assert_eq!(
        facts.message,
        activity(ActivityKind::Prompt, "Yes, fix both")
    );
    assert_eq!(facts.tool, activity(ActivityKind::Tool, "Edit(src/app.rs)"));

    // Thinking counts as the newest thing said while nothing follows it.
    write(
        &path,
        &[
            r#"{"type":"user","message":{"role":"user","content":"Go on"}}"#,
            r#"{"type":"assistant","message":{"model":"claude-opus-5","content":[{"type":"thinking","thinking":"Both failures share a cause."}]}}"#,
        ],
    );
    let facts = claude_facts(&path, None);
    assert_eq!(
        facts.message,
        activity(ActivityKind::Thinking, "Both failures share a cause.")
    );
    assert_eq!(facts.tool, None);
    assert_eq!(facts.model, None);
}

#[test]
fn a_claude_transcript_reports_the_newest_reply_whole_even_past_the_row_cap() {
    let directory = TempDir::new("claude-report");
    let path = directory.join("session.jsonl");
    let paragraph = "This paragraph explains one part of the change. "
        .repeat(100)
        .trim_end()
        .to_string();
    let long_reply = format!("{paragraph}\n\n{paragraph}\n\n{paragraph}");
    assert!(long_reply.chars().count() > 10_000);
    let record = serde_json::json!({
        "type": "assistant",
        "message": {
            "model": "claude-opus-5",
            "content": [{"type": "text", "text": long_reply}],
            "usage": {"input_tokens": 2}
        }
    });
    write(
        &path,
        &[
            r#"{"type":"user","message":{"role":"user","content":"Summarize the change"}}"#,
            &record.to_string(),
            // A user prompt sent after the reply does not replace the
            // report, even though it becomes the newest thing said in the
            // row.
            r#"{"type":"user","message":{"role":"user","content":"Thanks"}}"#,
        ],
    );

    let facts = claude_facts(&path, None);
    // The row shows the newest thing said, folded to its usual cap...
    assert_eq!(facts.message, activity(ActivityKind::Prompt, "Thanks"));
    // ...but the report is the assistant's reply whole, paragraphs and all.
    let report = facts.report.expect("report");
    assert_eq!(report.kind, ActivityKind::Message);
    assert_eq!(report.text, long_reply);
    assert_eq!(report.text.chars().count(), long_reply.chars().count());
    assert!(report.text.chars().count() > MAX_MESSAGE_CHARS);
}

#[test]
fn a_codex_rollout_reports_the_conversation_and_the_shell_command_it_runs() {
    let directory = TempDir::new("codex-conversation");
    let path = directory.join("rollout-2026-09-10T09-00-00-thread.jsonl");
    write(
        &path,
        &[
            r#"{"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"<environment_context>cwd</environment_context>"}]}}"#,
            r#"{"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"Add a retry to the uploader"}]}}"#,
            r#"{"type":"response_item","payload":{"type":"reasoning","summary":[{"type":"summary_text","text":"Looking for the upload loop"}]}}"#,
            r#"{"type":"response_item","payload":{"type":"function_call","name":"shell","arguments":"{\"command\":[\"bash\",\"-lc\",\"rg -n upload src\"]}","call_id":"c1"}}"#,
            r#"{"type":"response_item","payload":{"type":"function_call_output","call_id":"c1","output":"src/upload.rs:10"}}"#,
        ],
    );
    let facts = codex_facts(&path);
    assert_eq!(
        facts.message,
        activity(ActivityKind::Thinking, "Looking for the upload loop")
    );
    assert_eq!(
        facts.tool,
        activity(ActivityKind::Command, "rg -n upload src")
    );
    // The rollout begins with harness context, but the task is the first
    // actual user prompt rather than the injected AGENTS.md instructions.
    assert_eq!(facts.task.as_deref(), Some("Add a retry to the uploader"));

    write(
        &path,
        &[
            r#"{"type":"event_msg","payload":{"type":"exec_command_begin","command":["cargo","test","-p","uploader"],"cwd":"/repos/x"}}"#,
            r#"{"type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"The uploader now retries three times."}]}}"#,
        ],
    );
    let facts = codex_facts(&path);
    assert_eq!(
        facts.message,
        activity(
            ActivityKind::Message,
            "The uploader now retries three times."
        )
    );
    assert_eq!(
        facts.tool,
        activity(ActivityKind::Command, "cargo test -p uploader")
    );

    // Current Codex builds wrap tool calls in a JavaScript `exec`
    // transport. The dashboard shows the command, not that transport.
    write(
        &path,
        &[
            r#"{"type":"response_item","payload":{"type":"custom_tool_call","name":"exec","input":"const r = await tools.exec_command({\"cmd\":\"cargo clippy -- -D warnings\",\"yield_time_ms\":30000}); text(r.output);"}}"#,
        ],
    );
    let facts = codex_facts(&path);
    assert_eq!(
        facts.tool,
        activity(ActivityKind::Command, "cargo clippy -- -D warnings")
    );
}

#[test]
fn a_codex_rollout_reports_the_newest_reply_whole_even_past_the_row_cap() {
    let directory = TempDir::new("codex-report");
    let path = directory.join("rollout-2026-09-10T09-00-00-thread.jsonl");
    let paragraph = "This paragraph explains one part of the change. "
        .repeat(100)
        .trim_end()
        .to_string();
    let long_reply = format!("{paragraph}\n\n{paragraph}\n\n{paragraph}");
    assert!(long_reply.chars().count() > 10_000);
    let record = serde_json::json!({
        "type": "response_item",
        "payload": {
            "type": "message",
            "role": "assistant",
            "content": [{"type": "output_text", "text": long_reply}]
        }
    });
    write(
        &path,
        &[
            r#"{"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"Summarize the change"}]}}"#,
            &record.to_string(),
            // A user prompt sent after the reply does not replace the
            // report, even though it becomes the newest thing said in the
            // row.
            r#"{"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"Thanks"}]}}"#,
        ],
    );

    let facts = codex_facts(&path);
    // The row shows the newest thing said, folded to its usual cap...
    assert_eq!(facts.message, activity(ActivityKind::Prompt, "Thanks"));
    // ...but the report is the assistant's reply whole, paragraphs and all.
    let report = facts.report.expect("report");
    assert_eq!(report.kind, ActivityKind::Message);
    assert_eq!(report.text, long_reply);
    assert_eq!(report.text.chars().count(), long_reply.chars().count());
    assert!(report.text.chars().count() > MAX_MESSAGE_CHARS);
}

#[test]
fn a_claude_transcript_reads_back_the_turns_newest_first() {
    let directory = TempDir::new("claude-entries");
    let path = directory.join("session.jsonl");
    write(
        &path,
        &[
            r#"{"type":"user","message":{"role":"user","content":"<system-reminder>injected</system-reminder>"}}"#,
            r#"{"type":"user","message":{"role":"user","content":"Fix the uploader retry"}}"#,
            r#"{"type":"assistant","message":{"content":[{"type":"thinking","thinking":"Reading the loop"},{"type":"tool_use","name":"Bash","input":{"command":"rg -n retry src"}}]}}"#,
            r#"{"type":"user","message":{"content":[{"type":"tool_result","content":"src/upload.rs:10"}]}}"#,
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"Done.\n\nThe uploader now retries."}]}}"#,
            r#"{"type":"assistant","isSidechain":true,"message":{"content":[{"type":"text","text":"Subagent chatter"}]}}"#,
        ],
    );

    let entries = claude_entries(&path, TRANSCRIPT_ENTRIES);
    // Newest first, and the subagent's own turns are not this session's.
    assert_eq!(
        entries,
        vec![
            Activity {
                kind: ActivityKind::Message,
                // Paragraphs survive, because the newest reply is read
                // rather than skimmed.
                text: "Done.\n\nThe uploader now retries.".into(),
            },
            Activity {
                kind: ActivityKind::Command,
                text: "rg -n retry src".into(),
            },
            Activity {
                kind: ActivityKind::Prompt,
                text: "Fix the uploader retry".into(),
            },
        ]
    );

    // The limit counts from the newest turn backwards.
    assert_eq!(
        claude_entries(&path, 1),
        vec![Activity {
            kind: ActivityKind::Message,
            text: "Done.\n\nThe uploader now retries.".into(),
        }]
    );
}

#[test]
fn a_codex_rollout_reads_back_the_turns_without_doubling_them() {
    let directory = TempDir::new("codex-entries");
    let path = directory.join("rollout-2026-09-10T09-00-00-thread.jsonl");
    write(
        &path,
        &[
            r#"{"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"Add a retry to the uploader"}]}}"#,
            r#"{"type":"response_item","payload":{"type":"reasoning","summary":[{"type":"summary_text","text":"Looking for the upload loop"}]}}"#,
            r#"{"type":"event_msg","payload":{"type":"exec_command_begin","command":["bash","-lc","rg -n upload src"]}}"#,
            r#"{"type":"response_item","payload":{"type":"function_call","name":"shell","arguments":"{\"command\":[\"bash\",\"-lc\",\"rg -n upload src\"]}","call_id":"c1"}}"#,
            r#"{"type":"response_item","payload":{"type":"function_call_output","call_id":"c1","output":"src/upload.rs:10"}}"#,
            r#"{"type":"event_msg","payload":{"type":"agent_message","message":"The uploader now retries."}}"#,
            r#"{"type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"The uploader now retries."}]}}"#,
        ],
    );

    // The `event_msg` echo of the same command and the same reply is not
    // a second turn, and reasoning is not one of the turns shown.
    assert_eq!(
        codex_entries(&path, TRANSCRIPT_ENTRIES),
        vec![
            Activity {
                kind: ActivityKind::Message,
                text: "The uploader now retries.".into(),
            },
            Activity {
                kind: ActivityKind::Command,
                text: "rg -n upload src".into(),
            },
            Activity {
                kind: ActivityKind::Prompt,
                text: "Add a retry to the uploader".into(),
            },
        ]
    );
}

#[test]
fn tool_calls_read_as_the_command_or_as_the_tool_and_what_it_touches() {
    let bash = serde_json::json!({"command": "git status", "description": "Show status"});
    assert_eq!(
        tool_call("Bash", &bash),
        (ActivityKind::Command, "git status".into())
    );
    let read = serde_json::json!({"file_path": "/repo/src/ui.rs", "limit": 20});
    assert_eq!(
        tool_call("Read", &read),
        (ActivityKind::Tool, "Read(/repo/src/ui.rs)".into())
    );
    let agent = serde_json::json!({"description": "Find the parser", "prompt": "Long prompt"});
    assert_eq!(
        tool_call("Agent", &agent),
        (ActivityKind::Tool, "Agent(Find the parser)".into())
    );
    assert_eq!(
        tool_call("TodoWrite", &serde_json::json!({"todos": []})),
        (ActivityKind::Tool, "TodoWrite".into())
    );
    // Codex wraps commands in a shell; the script is what the user reads.
    assert_eq!(shell_words(&["/bin/zsh", "-lc", "ls -la"]), "ls -la");
    assert_eq!(
        shell_words(&["python3", "-c", "print(1)"]),
        "python3 -c print(1)"
    );
}

#[test]
fn codex_exec_wrappers_show_the_tools_inside_them() {
    let command = serde_json::json!({
        "input": r#"const r = await tools.exec_command({cmd:"rg -n \"upload\" src", workdir:"/repo"}); text(r.output);"#
    });
    assert_eq!(
        codex_custom_tool_call("exec", &command),
        (ActivityKind::Command, "rg -n \"upload\" src".into())
    );

    let commands = serde_json::json!({
        "input": r#"const results = await Promise.all([
                tools.exec_command({cmd:'cargo fmt --check'}),
                tools.exec_command({cmd:`cargo test`})
            ]);"#
    });
    assert_eq!(
        codex_custom_tool_call("exec", &commands),
        (
            ActivityKind::Command,
            "cargo fmt --check · cargo test".into()
        )
    );

    let patch = serde_json::json!({
        "input": r#"const patch = "*** Begin Patch\\n*** End Patch";
            text(await tools.apply_patch(patch));"#
    });
    assert_eq!(
        codex_custom_tool_call("exec", &patch),
        (ActivityKind::Tool, "apply_patch".into())
    );

    // A command string mentioning a tool-like name does not invent a
    // second nested call.
    assert_eq!(
        javascript_tool_names(
            r#"const r = await tools.exec_command({cmd:"rg tools.apply_patch src"});"#
        ),
        vec!["exec_command"]
    );
}

fn append(path: &Path, text: &str) {
    fs::OpenOptions::new()
        .append(true)
        .open(path)
        .expect("open session file")
        .write_all(text.as_bytes())
        .expect("append to session file");
}

fn agent(kind: &str, pane_id: &str, session: &str, cwd: &str) -> AgentInfo {
    AgentInfo {
        agent: Some(kind.into()),
        pane_id: pane_id.into(),
        cwd: Some(cwd.into()),
        agent_session: Some(AgentSession {
            value: session.into(),
            ..AgentSession::default()
        }),
        ..AgentInfo::default()
    }
}

/// A reader that looks for session files under `home`, which holds no
/// Claude Code settings.
fn reader(home: &Path) -> SessionReader {
    SessionReader {
        home: Some(home.to_path_buf()),
        ..SessionReader::default()
    }
}

/// Moves every lookup of `reader` back in time, as if `by` had passed.
fn age_lookups(reader: &mut SessionReader, by: Duration) {
    for lookup in reader.files.values_mut() {
        lookup.at = lookup.at.checked_sub(by).expect("an earlier instant");
        lookup.searched_everywhere_at = lookup
            .searched_everywhere_at
            .checked_sub(by)
            .expect("an earlier instant");
    }
}

fn claude_project(home: &Path, cwd_slug: &str) -> PathBuf {
    let project = home.join(".claude").join("projects").join(cwd_slug);
    fs::create_dir_all(&project).expect("create project dir");
    project
}

#[test]
fn a_backward_scan_reads_every_record_once_across_its_windows() {
    let directory = TempDir::new("scan-back");
    let path = directory.join("session.jsonl");
    let lines: Vec<String> = (0..20).map(|n| format!(r#"{{"n":{n}}}"#)).collect();
    write(&path, &lines.iter().map(String::as_str).collect::<Vec<_>>());
    // The newest record is still being written.
    append(&path, r#"{"n":"#);
    let length = fs::metadata(&path).expect("metadata").len();

    // Windows that end mid-record, so records cross from one to the next.
    let windows = [10, 45, 1000];
    let mut seen = Vec::new();
    let end = scan_back(&path, 0, &windows, |record| {
        seen.push(record["n"].as_u64().expect("number"));
        true
    });
    assert_eq!(seen, (0..20).rev().collect::<Vec<_>>());
    assert_eq!(end, Some(length - 5));

    // A scan from a record boundary sees only what follows it.
    let from = lines[..15].iter().map(|line| line.len() as u64 + 1).sum();
    let mut seen = Vec::new();
    scan_back(&path, from, &windows, |record| {
        seen.push(record["n"].as_u64().expect("number"));
        true
    });
    assert_eq!(seen, vec![19, 18, 17, 16, 15]);
}

#[test]
fn a_growing_transcript_is_read_on_from_where_the_last_read_stopped() {
    let home = TempDir::new("reader-incremental");
    let path = claude_project(&home, "-repos-corgi").join("s1.jsonl");
    write(
        &path,
        &[
            r#"{"type":"user","timestamp":"2026-09-11T18:00:00Z","message":{"role":"user","content":"Fix the uploader"}}"#,
            r#"{"type":"assistant","timestamp":"2026-09-11T18:01:00Z","effort":"high","message":{"model":"claude-opus-5","content":[{"type":"tool_use","name":"Bash","input":{"command":"cargo test"}}],"usage":{"input_tokens":2,"cache_creation_input_tokens":1000,"cache_read_input_tokens":19000,"cache_creation":{"ephemeral_5m_input_tokens":0,"ephemeral_1h_input_tokens":1000}}}}"#,
        ],
    );
    let mut reader = reader(&home);
    let agent = agent("claude", "p1", "s1", "/repos/corgi");
    let first = reader.facts(&agent);
    assert_eq!(first.model.as_deref(), Some("Opus 5"));
    assert_eq!(
        first.message,
        activity(ActivityKind::Prompt, "Fix the uploader")
    );

    // Blank out everything already read, keeping the length. A reader that
    // parsed those bytes again would lose every fact they held.
    let length = fs::metadata(&path).expect("metadata").len() as usize;
    fs::write(&path, " ".repeat(length - 1) + "\n").expect("blank transcript");
    // Half a record is not a record yet.
    let reply = r#"{"type":"assistant","timestamp":"2026-09-11T18:02:00Z","message":{"content":[{"type":"text","text":"Tests pass."}]}}"#;
    let (start, rest) = reply.split_at(40);
    append(&path, start);
    assert_eq!(reader.facts(&agent), first);

    append(&path, &format!("{rest}\n"));
    let facts = reader.facts(&agent);
    assert_eq!(
        facts.message,
        activity(ActivityKind::Message, "Tests pass.")
    );
    // What only the older records said is remembered.
    assert_eq!(facts.tool, activity(ActivityKind::Command, "cargo test"));
    assert_eq!(facts.model.as_deref(), Some("Opus 5"));
    assert_eq!(facts.effort.as_deref(), Some("high"));
    assert_eq!(facts.task.as_deref(), Some("Fix the uploader"));
    // The cache lapses an hour after the newest record, which wrote none.
    let touched_at = parse_rfc3339("2026-09-11T18:02:00Z").expect("timestamp");
    assert_eq!(
        facts.cache.map(|cache| cache.expires_at),
        Some(touched_at + CLAUDE_CACHE_1H)
    );
}

#[test]
fn the_transcript_is_shared_until_the_session_writes_more() {
    let home = TempDir::new("reader-transcript");
    let path = claude_project(&home, "-repos-corgi").join("s1.jsonl");
    write(
        &path,
        &[
            r#"{"type":"user","message":{"role":"user","content":"Fix the uploader"}}"#,
            r#"{"type":"assistant","message":{"content":[{"type":"tool_use","name":"Bash","input":{"command":"cargo test"}}]}}"#,
        ],
    );
    let mut reader = reader(&home);
    let agent = agent("claude", "p1", "s1", "/repos/corgi");
    let first = reader.transcript(&agent);
    assert_eq!(first.len(), 2);
    assert!(Arc::ptr_eq(&first, &reader.transcript(&agent)));

    write_line(
        &path,
        r#"{"type":"assistant","message":{"content":[{"type":"text","text":"Tests pass."}]}}"#,
    );
    let grown = reader.transcript(&agent);
    assert_eq!(
        grown
            .iter()
            .map(|entry| entry.text.as_str())
            .collect::<Vec<_>>(),
        ["Tests pass.", "cargo test", "Fix the uploader"]
    );
}

fn write_line(path: &Path, line: &str) {
    append(path, &format!("{line}\n"));
}

#[test]
fn a_session_file_written_after_a_miss_is_found_once_the_miss_expires() {
    let home = TempDir::new("reader-miss");
    let mut reader = reader(&home);
    let agent = agent("claude", "p1", "s1", "/repos/corgi");
    assert_eq!(reader.facts(&agent), SessionFacts::default());

    let path = claude_project(&home, "-repos-corgi").join("s1.jsonl");
    write(
        &path,
        &[r#"{"type":"user","message":{"role":"user","content":"Fix it"}}"#],
    );
    // The miss is trusted for a while, so a refresh does not search again.
    assert_eq!(reader.facts(&agent).task, None);
    age_lookups(&mut reader, MISS_RETRY);
    assert_eq!(reader.facts(&agent).task.as_deref(), Some("Fix it"));
}

#[test]
fn a_retried_codex_lookup_searches_the_newest_days_and_everything_now_and_then() {
    let home = TempDir::new("reader-codex-miss");
    let sessions = home.join(".codex").join("sessions");
    for day in ["2026/08/30", "2026/09/23", "2026/09/24"] {
        fs::create_dir_all(sessions.join(day)).expect("create day dir");
    }
    let mut reader = reader(&home);
    let today = agent("codex", "p1", "today", "/repos/a");
    let resumed = agent("codex", "p2", "resumed", "/repos/b");
    assert_eq!(reader.facts(&today), SessionFacts::default());
    assert_eq!(reader.facts(&resumed), SessionFacts::default());

    for (day, thread) in [("2026/09/24", "today"), ("2026/08/30", "resumed")] {
        write(
            &sessions
                .join(day)
                .join(format!("rollout-2026-09-24T09-00-00-{thread}.jsonl")),
            &[r#"{"type":"session_meta","payload":{"cwd":"/elsewhere","model":"gpt-5-codex"}}"#],
        );
    }
    age_lookups(&mut reader, MISS_RETRY);
    assert_eq!(reader.facts(&today).model.as_deref(), Some("gpt-5-codex"));
    // An older day is searched again only once in a while.
    assert_eq!(reader.facts(&resumed).model, None);
    age_lookups(&mut reader, FULL_SEARCH_RETRY);
    assert_eq!(reader.facts(&resumed).model.as_deref(), Some("gpt-5-codex"));
}

#[test]
fn retain_forgets_the_sessions_of_closed_panes() {
    let home = TempDir::new("reader-retain");
    let project = claude_project(&home, "-repos-corgi");
    for session in ["s1", "s2"] {
        write(
            &project.join(format!("{session}.jsonl")),
            &[r#"{"type":"user","message":{"role":"user","content":"Fix it"}}"#],
        );
    }
    let mut reader = reader(&home);
    let kept = agent("claude", "p1", "s1", "/repos/corgi");
    let closed = agent("claude", "p2", "s2", "/repos/corgi");
    for agent in [&kept, &closed] {
        reader.facts(agent);
        reader.transcript(agent);
    }

    reader.retain([&kept].into_iter());
    let kept_path = project.join("s1.jsonl");
    assert_eq!(
        reader.files.keys().collect::<Vec<_>>(),
        [&session_key(&kept)]
    );
    assert_eq!(reader.parsed.keys().collect::<Vec<_>>(), [&kept_path]);
    assert_eq!(reader.transcripts.keys().collect::<Vec<_>>(), [&kept_path]);
    assert_eq!(reader.facts(&kept).task.as_deref(), Some("Fix it"));
}
