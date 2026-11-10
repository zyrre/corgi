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
    let settings = ClaudeWindowSettings {
        model: configured_model.map(str::to_string),
        long_context_disabled: false,
    };
    ClaudeFile::default().facts(path, &settings)
}

fn codex_facts(path: &Path) -> SessionFacts {
    CodexFile::default().facts(path)
}

fn claude_task(path: &Path) -> Option<String> {
    let mut head = Head::default();
    head.update(path, TASK_HEAD_BYTES, super::claude::claude_task);
    head.found
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
            r#"{"type":"assistant","message":{"model":"claude-opus-4-6","usage":{"input_tokens":2,"cache_creation_input_tokens":1000,"cache_read_input_tokens":19000,"output_tokens":500}}}"#,
            r#"{"type":"last-prompt","lastPrompt":"unrelated"}"#,
        ],
    );

    // This record names no requested model, as older versions wrote it, and
    // an older model without a long-context one configured fills the default
    // window.
    assert_eq!(
        claude_facts(&path, None),
        SessionFacts {
            model: Some("Opus 4.6".into()),
            context_percent: Some(10),
            context_tokens: Some(20_002),
            ..SessionFacts::default()
        }
    );
    // Without a requested model, the configured variant is all that reveals
    // a long-context session before it passes the default window.
    assert_eq!(
        claude_facts(&path, Some("opus[1m]")),
        SessionFacts {
            model: Some("Opus 4.6 1M".into()),
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
            context_percent: Some(2),
            context_tokens: Some(20_002),
            ..SessionFacts::default()
        }
    );
}

/// The window of a session with no settings of note.
fn window_of(model_id: &str, requested: Option<&str>, tokens: u64) -> u64 {
    claude_context_window(
        model_id,
        requested,
        &ClaudeWindowSettings::default(),
        tokens,
    )
}

const DISABLED: ClaudeWindowSettings = ClaudeWindowSettings {
    model: None,
    long_context_disabled: true,
};

#[test]
fn a_model_has_the_window_of_claude_codes_own_table_from_the_first_request() {
    // Each of these was checked against what the real CLI reported as
    // `context_window_size` to its status line.
    for long in [
        "claude-opus-5-5",
        "claude-opus-5",
        "claude-opus-4-8",
        "claude-opus-4-7",
        "claude-sonnet-5-5",
        "claude-sonnet-5",
        "claude-haiku-5-5",
        "claude-fable-5-1",
        "claude-fable-5",
        "claude-mythos-5-1",
        "claude-mythos-preview",
        // A build date or a cloud provider's spelling changes nothing.
        "claude-opus-5-5-20260601",
        "us.anthropic.claude-opus-4-8",
        "anthropic.claude-sonnet-5-5-v1:0",
        "claude-opus-5-5@20260601",
    ] {
        assert_eq!(window_of(long, None, 20_000), CLAUDE_LONG_WINDOW, "{long}");
    }
    for default in [
        "claude-opus-4-6",
        "claude-opus-4-5",
        "claude-opus-4-1-20250805",
        "claude-opus-4-20250514",
        "claude-sonnet-4-6",
        "claude-sonnet-4-5",
        "claude-sonnet-4-20250514",
        "claude-haiku-4-5-20251001",
        "claude-3-7-sonnet-20250219",
        "claude-3-5-haiku-20241022",
    ] {
        assert_eq!(
            window_of(default, None, 20_000),
            CLAUDE_DEFAULT_WINDOW,
            "{default}"
        );
    }
}

#[test]
fn a_newer_version_of_a_long_family_keeps_the_long_window() {
    for newer in ["claude-opus-6", "claude-sonnet-5-6", "claude-haiku-6-1"] {
        assert_eq!(
            window_of(newer, None, 20_000),
            CLAUDE_LONG_WINDOW,
            "{newer}"
        );
    }
}

#[test]
fn a_model_nothing_is_known_about_falls_back_to_the_default_window() {
    // Claude Code itself assumes the default window for these.
    for unknown in ["my-gateway-model", "opus-5-5", "claude-newfamily-1", ""] {
        assert_eq!(
            window_of(unknown, None, 20_000),
            CLAUDE_DEFAULT_WINDOW,
            "{unknown:?}"
        );
    }
}

#[test]
fn the_requested_model_shows_the_long_context_variant_of_an_older_model() {
    let opus = "claude-opus-4-6";
    assert_eq!(
        window_of(opus, Some("claude-opus-4-6[1m]"), 20_000),
        CLAUDE_LONG_WINDOW
    );
    // Claude Code matches the tag in any case.
    assert_eq!(
        window_of(opus, Some("claude-opus-4-6[1M]"), 20_000),
        CLAUDE_LONG_WINDOW
    );
    assert_eq!(window_of(opus, Some(opus), 20_000), CLAUDE_DEFAULT_WINDOW);
    // A native model's tag is dropped before it is recorded, so its absence
    // must not take the long window away.
    assert_eq!(
        window_of("claude-opus-5-5", Some("claude-opus-5-5"), 20_000),
        CLAUDE_LONG_WINDOW
    );
}

#[test]
fn the_configured_model_stands_in_only_for_a_record_without_a_requested_model() {
    let settings = ClaudeWindowSettings {
        model: Some("opus[1m]".into()),
        long_context_disabled: false,
    };
    let window = |model, requested| claude_context_window(model, requested, &settings, 20_000);

    assert_eq!(window("claude-opus-4-6", None), CLAUDE_LONG_WINDOW);
    // Another family says nothing about this one.
    assert_eq!(window("claude-sonnet-4-6", None), CLAUDE_DEFAULT_WINDOW);
    // The session's own record outranks a setting that may be newer than it,
    // or that its `--model` overrode.
    assert_eq!(
        window("claude-opus-4-6", Some("claude-opus-4-6")),
        CLAUDE_DEFAULT_WINDOW
    );
}

#[test]
fn turning_long_context_off_holds_a_session_to_the_default_window() {
    for (model, requested) in [
        ("claude-opus-5-5", Some("claude-opus-5-5")),
        ("claude-fable-5-1", None),
        ("claude-opus-4-6", Some("claude-opus-4-6[1m]")),
    ] {
        assert_eq!(
            claude_context_window(model, requested, &DISABLED, 20_000),
            CLAUDE_DEFAULT_WINDOW,
            "{model}"
        );
    }
}

#[test]
fn a_session_past_the_default_window_has_the_long_one_whatever_else_says() {
    assert_eq!(
        window_of("claude-opus-4-6", Some("claude-opus-4-6"), 220_000),
        CLAUDE_LONG_WINDOW
    );
    assert_eq!(
        window_of("my-gateway-model", None, 220_000),
        CLAUDE_LONG_WINDOW
    );
    // The flag may have been set after this session started.
    assert_eq!(
        claude_context_window("claude-opus-5-5", None, &DISABLED, 220_000),
        CLAUDE_LONG_WINDOW
    );
    // Exactly full is still within the default window.
    assert_eq!(
        window_of("claude-opus-4-6", None, CLAUDE_DEFAULT_WINDOW),
        CLAUDE_DEFAULT_WINDOW
    );
}

#[test]
fn only_a_long_window_the_model_does_not_have_by_default_is_labelled() {
    let name = claude_model_name;
    assert_eq!(name("claude-fable-5-1", CLAUDE_LONG_WINDOW), "Fable 5.1");
    assert_eq!(name("claude-opus-5-5", CLAUDE_LONG_WINDOW), "Opus 5.5");
    assert_eq!(name("claude-opus-5-5", CLAUDE_DEFAULT_WINDOW), "Opus 5.5");
    assert_eq!(name("claude-opus-4-6", CLAUDE_LONG_WINDOW), "Opus 4.6 1M");
    assert_eq!(name("claude-opus-4-6", CLAUDE_DEFAULT_WINDOW), "Opus 4.6");
    assert_eq!(
        name("claude-haiku-4-5-20251001", CLAUDE_DEFAULT_WINDOW),
        "Haiku 4.5"
    );
}

#[test]
fn the_window_shows_in_the_percentage_and_the_label_of_a_real_shaped_transcript() {
    let directory = TempDir::new("claude-window");
    let path = directory.join("session.jsonl");
    let facts_of = |model: &str, requested: &str| {
        let usage = r#""usage":{"input_tokens":2,"cache_creation_input_tokens":1000,"cache_read_input_tokens":90000,"output_tokens":5}"#;
        write(
            &path,
            &[&format!(
                r#"{{"type":"assistant","requestedModel":"{requested}","message":{{"model":"{model}",{usage}}}}}"#
            )],
        );
        // A setting that would mislead, if it were asked before the record.
        claude_facts(&path, Some("sonnet[1m]"))
    };

    let native = facts_of("claude-opus-5-5", "claude-opus-5-5");
    assert_eq!(native.context_percent, Some(9));
    assert_eq!(native.model.as_deref(), Some("Opus 5.5"));

    let default = facts_of("claude-opus-4-6", "claude-opus-4-6");
    assert_eq!(default.context_percent, Some(46));
    assert_eq!(default.model.as_deref(), Some("Opus 4.6"));

    let tagged = facts_of("claude-opus-4-6", "claude-opus-4-6[1m]");
    assert_eq!(tagged.context_percent, Some(9));
    assert_eq!(tagged.model.as_deref(), Some("Opus 4.6 1M"));
}

#[test]
fn window_settings_are_read_local_first_and_over_the_environment() {
    let directory = TempDir::new("claude-window-settings");
    let (dir, other) = (directory.join("config"), directory.join("home"));
    fs::create_dir_all(&dir).expect("create config dir");
    fs::create_dir_all(&other).expect("create home dir");
    let dirs = [dir.clone(), other.clone()];
    let off = |dirs: &[PathBuf], environment| {
        claude_window_settings(dirs, environment).long_context_disabled
    };

    // Nothing anywhere leaves long context on.
    assert_eq!(
        claude_window_settings(&dirs, None),
        ClaudeWindowSettings::default()
    );
    // Claude Code's truthy values, and nothing else.
    for on in ["1", "true", "TRUE", " yes ", "on"] {
        assert!(off(&dirs, Some(on)), "{on:?}");
    }
    for not in ["0", "false", "", "off", "2"] {
        assert!(!off(&dirs, Some(not)), "{not:?}");
    }

    fs::write(
        other.join("settings.json"),
        r#"{"model":"opus[1m]","env":{"CLAUDE_CODE_DISABLE_1M_CONTEXT":"1"}}"#,
    )
    .expect("write settings");
    assert_eq!(
        claude_window_settings(&dirs, None),
        ClaudeWindowSettings {
            model: Some("opus[1m]".into()),
            long_context_disabled: true,
        }
    );
    // Settings override what the session inherited, and local ones the user's.
    assert!(off(&dirs, Some("0")));
    fs::write(
        dir.join("settings.local.json"),
        r#"{"env":{"CLAUDE_CODE_DISABLE_1M_CONTEXT":"0"}}"#,
    )
    .expect("write settings");
    assert!(!off(&dirs, Some("1")));
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
            // report.
            r#"{"type":"user","message":{"role":"user","content":"Thanks"}}"#,
        ],
    );

    let facts = claude_facts(&path, None);
    // The report is the assistant's reply whole, paragraphs and all.
    let report = facts.report.expect("report");
    assert_eq!(report.kind, ActivityKind::Message);
    assert_eq!(report.text, long_reply);
    assert_eq!(report.text.chars().count(), long_reply.chars().count());
    assert!(report.text.chars().count() > MAX_MESSAGE_CHARS);
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
            // report.
            r#"{"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"Thanks"}]}}"#,
        ],
    );

    let facts = codex_facts(&path);
    // The report is the assistant's reply whole, paragraphs and all.
    let report = facts.report.expect("report");
    assert_eq!(report.kind, ActivityKind::Message);
    assert_eq!(report.text, long_reply);
    assert_eq!(report.text.chars().count(), long_reply.chars().count());
    assert!(report.text.chars().count() > MAX_MESSAGE_CHARS);
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
    assert_eq!(first.report, None);

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
    assert_eq!(facts.report, activity(ActivityKind::Message, "Tests pass."));
    // What only the older records said is remembered.
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

/// The newest turn's state in `facts`, and the report they give.
fn turn_of(facts: SessionFacts) -> (Option<TurnState>, Option<String>) {
    (facts.turn, facts.report.map(|report| report.text))
}

#[test]
fn a_claude_turn_whose_reply_is_not_written_yet_is_open_until_it_is() {
    let directory = TempDir::new("claude-turn-open");
    let path = directory.join("session.jsonl");
    write(
        &path,
        &[
            r#"{"type":"user","message":{"content":"Wait for discovery."}}"#,
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"Waiting for discovery to write the list."}]}}"#,
            r#"{"type":"system","subtype":"turn_duration","durationMs":5}"#,
            r#"{"type":"user","message":{"content":"[supervisor] Noted. What is your plan?"}}"#,
            r#"{"type":"assistant","message":{"content":[{"type":"thinking","thinking":"Plan it."}]}}"#,
        ],
    );
    let mut file = ClaudeFile::default();
    // The prompt and the thinking are written; the reply is not: the newest
    // reply is the turn before's.
    assert_eq!(
        turn_of(file.facts(&path, &ClaudeWindowSettings::default())),
        (
            Some(TurnState::Open),
            Some("Waiting for discovery to write the list.".into())
        )
    );
    append(
        &path,
        concat!(
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"Understood. My plan: disable it."}]}}"#,
            "\n",
        ),
    );
    assert_eq!(
        turn_of(file.facts(&path, &ClaudeWindowSettings::default())),
        (
            Some(TurnState::Answered),
            Some("Understood. My plan: disable it.".into())
        )
    );
}

#[test]
fn a_claude_turn_of_tool_calls_alone_ends_silent_and_its_results_start_nothing() {
    let directory = TempDir::new("claude-turn-silent");
    let path = directory.join("session.jsonl");
    let earlier = [
        r#"{"type":"user","message":{"content":"Say hi."}}"#,
        r#"{"type":"assistant","message":{"content":[{"type":"text","text":"Hi."}]}}"#,
        r#"{"type":"system","subtype":"turn_duration","durationMs":5}"#,
        r#"{"type":"user","message":{"content":"Touch the file."}}"#,
        r#"{"type":"assistant","message":{"content":[{"type":"text","text":"Touching it."}]}}"#,
        r#"{"type":"assistant","message":{"content":[{"type":"tool_use","name":"Bash","input":{"command":"touch f"}}]}}"#,
        r#"{"type":"user","message":{"content":[{"type":"tool_result","content":"done"}]}}"#,
    ];
    write(&path, &earlier);
    // A reply before the tool call is not the turn's last word.
    assert_eq!(claude_facts(&path, None).turn, Some(TurnState::Open));
    write(
        &path,
        &[
            &earlier[..],
            &[r#"{"type":"system","subtype":"turn_duration","durationMs":5}"#],
        ]
        .concat(),
    );
    assert_eq!(claude_facts(&path, None).turn, Some(TurnState::Silent));
    // A reply after the tool result answers the turn, ended or not.
    write(
        &path,
        &[
            &earlier[..],
            &[r#"{"type":"assistant","message":{"content":[{"type":"text","text":"Touched."}]}}"#],
        ]
        .concat(),
    );
    assert_eq!(
        turn_of(claude_facts(&path, None)),
        (Some(TurnState::Answered), Some("Touched.".into()))
    );
}

#[test]
fn a_claude_turn_starts_with_any_user_text_even_one_no_view_shows() {
    let directory = TempDir::new("claude-turn-starts");
    let path = directory.join("session.jsonl");
    let answered = [
        r#"{"type":"user","message":{"content":"Run the tests."}}"#,
        r#"{"type":"assistant","message":{"content":[{"type":"text","text":"They pass."}]}}"#,
        r#"{"type":"system","subtype":"turn_duration","durationMs":5}"#,
    ];
    for start in [
        // A background task's notification.
        r#"{"type":"user","message":{"content":"<task-notification>\n<task-id>b7</task-id>\n</task-notification>"}}"#,
        // Another session's message, which Claude Code records as meta.
        r#"{"type":"user","isMeta":true,"message":{"content":"Another Claude session sent a message: hi"}}"#,
        // A pasted brief.
        r#"{"type":"user","message":{"content":[{"type":"text","text":"<pasted_content id=\"a1\">\nThe brief\n</pasted_content>"}]}}"#,
    ] {
        write(&path, &[&answered[..], &[start]].concat());
        assert_eq!(
            turn_of(claude_facts(&path, None)),
            (Some(TurnState::Open), Some("They pass.".into())),
            "{start}"
        );
    }
    // A subagent's records belong to no turn of the session's.
    write(
        &path,
        &[
            &answered[..],
            &[r#"{"type":"user","isSidechain":true,"message":{"content":"Search the code."}}"#],
        ]
        .concat(),
    );
    assert_eq!(claude_facts(&path, None).turn, Some(TurnState::Answered));
}

/// A Codex rollout of `records`, each a `type` and a `payload`.
fn codex_rollout_of(path: &Path, records: &[&str]) {
    let lines: Vec<String> = records
        .iter()
        .map(|record| format!(r#"{{"timestamp":"2026-10-30T12:00:00Z",{record}}}"#))
        .collect();
    write(path, &lines.iter().map(String::as_str).collect::<Vec<_>>());
}

#[test]
fn a_codex_turn_is_open_until_its_reply_is_written_and_silent_when_it_ends_without_one() {
    let directory = TempDir::new("codex-turn");
    let path = directory.join("rollout.jsonl");
    let earlier = [
        r#""type":"event_msg","payload":{"type":"task_started"}"#,
        r#""type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"Say hi."}]}"#,
        r#""type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"Hi."}]}"#,
        r#""type":"event_msg","payload":{"type":"agent_message","message":"Hi."}"#,
        r#""type":"event_msg","payload":{"type":"task_complete","last_agent_message":"Hi."}"#,
        r#""type":"event_msg","payload":{"type":"task_started"}"#,
        r#""type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"[supervisor] List the files."}]}"#,
        r#""type":"event_msg","payload":{"type":"user_message","message":"[supervisor] List the files."}"#,
        r#""type":"response_item","payload":{"type":"function_call","name":"shell","arguments":"{\"command\":[\"ls\"]}"}"#,
        r#""type":"response_item","payload":{"type":"function_call_output","output":"a b"}"#,
    ];
    codex_rollout_of(&path, &earlier);
    assert_eq!(
        turn_of(codex_facts(&path)),
        (Some(TurnState::Open), Some("Hi.".into()))
    );
    let mut ended = earlier.to_vec();
    ended
        .push(r#""type":"event_msg","payload":{"type":"task_complete","last_agent_message":null}"#);
    codex_rollout_of(&path, &ended);
    assert_eq!(codex_facts(&path).turn, Some(TurnState::Silent));
    let mut answered = earlier.to_vec();
    answered.extend([
        r#""type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"a and b."}]}"#,
        r#""type":"event_msg","payload":{"type":"agent_message","message":"a and b."}"#,
    ]);
    codex_rollout_of(&path, &answered);
    assert_eq!(
        turn_of(codex_facts(&path)),
        (Some(TurnState::Answered), Some("a and b.".into()))
    );
    // A turn that starts with no prompt the views show is a turn all the same.
    let mut started = answered.clone();
    started.push(r#""type":"event_msg","payload":{"type":"turn_started"}"#);
    codex_rollout_of(&path, &started);
    assert_eq!(codex_facts(&path).turn, Some(TurnState::Open));
}
