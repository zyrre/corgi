//! Read-only access to the locally authenticated plan limits of the coding
//! agent CLIs installed on this machine.
//!
//! Codex exposes these values through its app-server JSON-RPC interface. Corgi
//! starts a short-lived app-server only when it needs a refresh, so it does not
//! need to know where Codex stores credentials or persist any account data.
//!
//! Claude Code has no equivalent local API. Corgi reads the OAuth token that
//! Claude Code already stores (macOS Keychain or `~/.claude/.credentials.json`)
//! and asks Anthropic's usage endpoint for the current windows. The token is
//! passed to `curl` through stdin so it never appears in process arguments.

use std::{
    collections::{HashMap, HashSet},
    env, fs,
    io::{BufRead, BufReader, Write},
    path::PathBuf,
    process::{Command, Stdio},
    sync::mpsc,
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, anyhow, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{
    harness::{Harness, claude_config_dirs},
    time::parse_rfc3339,
};

const REQUEST_TIMEOUT: Duration = Duration::from_secs(6);
const CLAUDE_USAGE_URL: &str = "https://api.anthropic.com/api/oauth/usage";
const CLAUDE_KEYCHAIN_SERVICE: &str = "Claude Code-credentials";

/// A coding-agent vendor whose plan usage Corgi knows how to read: the
/// harnesses with a plan-usage surface, viewed as [`Harness`]es everywhere
/// else.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Provider {
    Codex,
    Claude,
}

impl Provider {
    /// Every provider Corgi can read, in default-preference order.
    const ALL: [Provider; 2] = [Provider::Codex, Provider::Claude];

    /// The harness whose plan this is.
    pub fn harness(self) -> &'static Harness {
        match self {
            Provider::Codex => &Harness::Codex,
            Provider::Claude => &Harness::Claude,
        }
    }

    /// Human label used in the dashboard.
    pub fn label(self) -> &'static str {
        self.harness().title()
    }

    /// Title of this provider's usage card in the dashboard header.
    pub fn card_title(self) -> &'static str {
        match self {
            Provider::Codex => "CODEX",
            Provider::Claude => "CLAUDE",
        }
    }

    /// Whether the provider's CLI can be found; see [`Harness::executable`].
    fn is_installed(self) -> bool {
        self.harness().executable().is_some()
    }
}

/// Providers whose CLI is installed, in [`Provider::ALL`] order.
pub fn installed_providers() -> Vec<Provider> {
    Provider::ALL
        .into_iter()
        .filter(|provider| provider.is_installed())
        .collect()
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UsageWindow {
    pub used_percent: u8,
    /// Unix timestamp, or 0 when the provider did not report a reset time.
    pub resets_at: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanUsage {
    pub provider: Provider,
    pub plan_type: String,
    pub five_hour: Option<UsageWindow>,
    pub week: Option<UsageWindow>,
    /// Codex-only: rate-limit reset credits. `None` when not applicable.
    pub banked_resets: Option<u32>,
}

/// Reads the current plan usage for one provider without changing
/// authentication, account state, or rate-limit credits.
pub fn read_plan_usage(provider: Provider) -> Result<PlanUsage> {
    match provider {
        Provider::Codex => read_codex_usage(),
        Provider::Claude => read_claude_usage(),
    }
}

/// Reads the models that the locally authenticated Codex CLI currently offers
/// in its picker. This deliberately uses the app-server catalog rather than
/// the public API catalog: the app-server knows the account, workspace policy,
/// and Codex-specific availability that the CLI will use.
pub fn read_codex_models() -> Result<Vec<String>> {
    let mut server = AppServer::start()?;
    let mut models: Vec<String> = Vec::new();
    let mut cursor = None;
    loop {
        let response = server.call(
            "model/list",
            json!({
                "cursor": cursor,
                "includeHidden": false,
                "limit": 100,
            }),
        )?;
        let (page, next_cursor) = parse_codex_models(&response)?;
        for model in page {
            if !models
                .iter()
                .any(|known| known.eq_ignore_ascii_case(&model))
            {
                models.push(model);
            }
        }
        let Some(next_cursor) = next_cursor else {
            break;
        };
        cursor = Some(next_cursor);
    }
    if models.is_empty() {
        bail!("Codex returned no picker-visible models")
    }
    Ok(models)
}

/// Reads Codex's generated, human-facing name for each known thread.
///
/// The app-server protocol is experimental, so an unavailable method or an
/// older CLI simply returns an error to the caller. Corgi keeps the transcript
/// task fallback in that case. A missing name is normal for a brand-new thread
/// whose title generation has not completed yet.
pub fn read_codex_thread_titles(thread_ids: &[String]) -> Result<HashMap<String, String>> {
    let mut seen = HashSet::new();
    let thread_ids: Vec<String> = thread_ids
        .iter()
        .map(|thread_id| thread_id.trim())
        .filter(|thread_id| !thread_id.is_empty())
        .filter(|thread_id| seen.insert((*thread_id).to_string()))
        .map(str::to_string)
        .collect();
    if thread_ids.is_empty() {
        return Ok(HashMap::new());
    }

    let mut server = AppServer::start()?;
    let mut titles = HashMap::new();
    for thread_id in &thread_ids {
        let response = server.call(
            "thread/read",
            json!({ "threadId": thread_id, "includeTurns": false }),
        )?;
        if let Some(title) = parse_codex_thread_title(&response) {
            titles.insert(thread_id.clone(), title);
        }
    }
    Ok(titles)
}

// ---------------------------------------------------------------------------
// Codex
// ---------------------------------------------------------------------------

fn read_codex_usage() -> Result<PlanUsage> {
    let mut server = AppServer::start()?;
    let account = server.call("account/read", json!({ "refreshToken": false }))?;
    let rate_limits = server.call("account/rateLimits/read", Value::Null)?;
    parse_codex_usage(&account, &rate_limits)
}

/// A short-lived `codex app-server` subprocess speaking newline-delimited
/// JSON-RPC on stdin/stdout. `Drop` kills and waits for the child so an early
/// return or `?` from any caller can never leak the process.
struct AppServer {
    child: std::process::Child,
    input: std::process::ChildStdin,
    messages: mpsc::Receiver<String>,
    next_id: u64,
}

impl AppServer {
    fn start() -> Result<Self> {
        let codex = Harness::Codex.executable().context("codex CLI not found")?;
        let mut child = Command::new(codex)
            .arg("app-server")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .context("start codex app-server")?;
        let input = child.stdin.take().context("open Codex app-server input")?;
        let output = child
            .stdout
            .take()
            .context("open Codex app-server output")?;
        let messages = app_server_messages(output);
        let mut server = AppServer {
            child,
            input,
            messages,
            next_id: 1,
        };
        server.initialize()?;
        Ok(server)
    }

    fn initialize(&mut self) -> Result<()> {
        self.call(
            "initialize",
            json!({
                "clientInfo": {
                    "name": "corgi",
                    "title": "Corgi",
                    "version": env!("CARGO_PKG_VERSION"),
                }
            }),
        )?;
        self.notify("initialized", json!({}))
    }

    /// Sends a request and waits for its matching response. `params` is
    /// omitted from the request entirely when `Value::Null`, matching the
    /// shape Codex's app-server expects for parameterless methods.
    fn call(&mut self, method: &str, params: Value) -> Result<Value> {
        let id = self.next_id;
        self.next_id += 1;
        let mut message = json!({ "method": method, "id": id });
        if !params.is_null() {
            message["params"] = params;
        }
        self.send(message)?;
        self.wait_for_response(id)
    }

    /// Sends a notification, which has no response to wait for.
    fn notify(&mut self, method: &str, params: Value) -> Result<()> {
        self.send(json!({ "method": method, "params": params }))
    }

    fn send(&mut self, message: Value) -> Result<()> {
        serde_json::to_writer(&mut self.input, &message)
            .context("encode Codex app-server request")?;
        self.input
            .write_all(b"\n")
            .context("write Codex app-server request")?;
        self.input.flush().context("flush Codex app-server request")
    }

    fn wait_for_response(&self, id: u64) -> Result<Value> {
        wait_for_response(&self.messages, id)
    }
}

impl Drop for AppServer {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn app_server_messages(output: impl std::io::Read + Send + 'static) -> mpsc::Receiver<String> {
    let (messages_tx, messages_rx) = mpsc::channel();
    thread::spawn(move || {
        for line in BufReader::new(output).lines().map_while(Result::ok) {
            if messages_tx.send(line).is_err() {
                break;
            }
        }
    });
    messages_rx
}

fn wait_for_response(messages: &mpsc::Receiver<String>, id: u64) -> Result<Value> {
    let deadline = Instant::now() + REQUEST_TIMEOUT;
    loop {
        let remaining = deadline
            .checked_duration_since(Instant::now())
            .context("Codex app-server did not respond in time")?;
        let line = messages
            .recv_timeout(remaining)
            .context("Codex app-server closed before responding")?;
        // Launchers such as mise can print a one-line diagnostic before it
        // execs Codex. It shares the inherited stdout with the JSON-RPC
        // stream, so ignore non-JSON noise rather than failing the entire
        // refresh.
        let Ok(message) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        if message.get("id").and_then(Value::as_u64) != Some(id) {
            continue;
        }
        if let Some(error) = message.get("error") {
            let detail = error
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("unknown error");
            bail!("Codex app-server request failed: {detail}");
        }
        return message
            .get("result")
            .cloned()
            .context("Codex app-server response did not include a result");
    }
}

fn parse_codex_models(response: &Value) -> Result<(Vec<String>, Option<String>)> {
    let data = response
        .get("data")
        .and_then(Value::as_array)
        .context("Codex did not return a model list")?;
    let models = data
        .iter()
        .filter(|model| {
            !model
                .get("hidden")
                .and_then(Value::as_bool)
                .unwrap_or(false)
        })
        .filter_map(|model| {
            model
                .get("model")
                .or_else(|| model.get("id"))
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|model| !model.is_empty())
                .map(str::to_string)
        })
        .collect();
    let next_cursor = response
        .get("nextCursor")
        .and_then(Value::as_str)
        .filter(|cursor| !cursor.is_empty())
        .map(str::to_string);
    Ok((models, next_cursor))
}

/// The optional generated task title in a `thread/read` response.
fn parse_codex_thread_title(response: &Value) -> Option<String> {
    response
        .pointer("/thread/name")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|title| !title.is_empty())
        .map(str::to_string)
}

fn parse_codex_usage(account: &Value, rate_limits: &Value) -> Result<PlanUsage> {
    let limits = rate_limits
        .get("rateLimits")
        .context("Codex did not return rate limits")?;
    let plan_type = account
        .pointer("/account/planType")
        .or_else(|| limits.get("planType"))
        .and_then(Value::as_str)
        .filter(|plan| !plan.is_empty())
        .map(display_plan_type)
        .unwrap_or_else(|| "ChatGPT".into());
    let banked_resets = rate_limits
        .pointer("/rateLimitResetCredits/availableCount")
        .and_then(Value::as_u64)
        .unwrap_or_default()
        .min(u32::MAX as u64) as u32;

    Ok(PlanUsage {
        provider: Provider::Codex,
        plan_type,
        five_hour: parse_codex_window(limits.get("primary")),
        week: parse_codex_window(limits.get("secondary")),
        banked_resets: Some(banked_resets),
    })
}

fn parse_codex_window(value: Option<&Value>) -> Option<UsageWindow> {
    let value = value?;
    Some(UsageWindow {
        used_percent: value.get("usedPercent").and_then(Value::as_u64)?.min(100) as u8,
        resets_at: value.get("resetsAt").and_then(Value::as_u64)?,
    })
}

// ---------------------------------------------------------------------------
// Claude Code
// ---------------------------------------------------------------------------

struct ClaudeCredentials {
    access_token: String,
    subscription_type: Option<String>,
    expires_at_ms: Option<u64>,
}

fn read_claude_usage() -> Result<PlanUsage> {
    let credentials = load_claude_credentials()?;
    if let Some(expires_at_ms) = credentials.expires_at_ms {
        let now_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        if expires_at_ms <= now_ms {
            bail!("Claude login has expired; run Claude Code once to refresh it");
        }
    }
    let body = fetch_claude_usage(&credentials.access_token)?;
    let response: Value = serde_json::from_str(&body).context("decode Claude usage response")?;
    parse_claude_usage(&response, credentials.subscription_type.as_deref())
}

fn load_claude_credentials() -> Result<ClaudeCredentials> {
    let mut attempts: Vec<String> = Vec::new();
    for path in claude_credential_paths() {
        match fs::read_to_string(&path) {
            Ok(raw) => return parse_claude_credentials(&raw),
            Err(error) => attempts.push(format!("{}: {error}", path.display())),
        }
    }
    if cfg!(target_os = "macos") {
        match read_claude_keychain() {
            Ok(raw) => return parse_claude_credentials(&raw),
            Err(error) => attempts.push(format!("Keychain: {error}")),
        }
    }
    Err(anyhow!(
        "no Claude Code login found ({}); run `claude` and sign in",
        attempts.join("; ")
    ))
}

fn claude_credential_paths() -> Vec<PathBuf> {
    claude_config_dirs()
        .into_iter()
        .map(|dir| dir.join(".credentials.json"))
        .collect()
}

fn read_claude_keychain() -> Result<String> {
    let output = Command::new("security")
        .args(["find-generic-password", "-s", CLAUDE_KEYCHAIN_SERVICE, "-w"])
        .stdin(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .context("run security")?;
    if !output.status.success() {
        let detail = String::from_utf8_lossy(&output.stderr).trim().to_string();
        bail!(
            "{}",
            if detail.is_empty() {
                "no entry".into()
            } else {
                detail
            }
        );
    }
    String::from_utf8(output.stdout).context("Keychain entry is not UTF-8")
}

fn parse_claude_credentials(raw: &str) -> Result<ClaudeCredentials> {
    let value: Value =
        serde_json::from_str(raw.trim()).context("decode Claude Code credentials")?;
    let oauth = value
        .get("claudeAiOauth")
        .context("Claude Code credentials have no claude.ai login")?;
    let access_token = oauth
        .get("accessToken")
        .and_then(Value::as_str)
        .filter(|token| !token.is_empty())
        .context("Claude Code credentials have no access token")?
        .to_string();
    Ok(ClaudeCredentials {
        access_token,
        subscription_type: oauth
            .get("subscriptionType")
            .and_then(Value::as_str)
            .filter(|plan| !plan.is_empty())
            .map(str::to_string),
        expires_at_ms: oauth.get("expiresAt").and_then(Value::as_u64),
    })
}

/// Fetches the usage document with `curl`, passing the bearer token through a
/// stdin config file so it is never visible in the process table.
fn fetch_claude_usage(access_token: &str) -> Result<String> {
    let mut child = Command::new("curl")
        .args(["--silent", "--show-error", "--config", "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("start curl")?;
    {
        let mut input = child.stdin.take().context("open curl input")?;
        // The status code is appended after the body so a non-2xx answer can
        // still be reported with the server's own explanation.
        let config = format!(
            "url = {}\nmax-time = {}\nwrite-out = {}\nheader = {}\nheader = {}\nheader = {}\nheader = {}\n",
            curl_quote(CLAUDE_USAGE_URL),
            REQUEST_TIMEOUT.as_secs(),
            curl_quote("\n%{http_code}"),
            curl_quote(&format!("Authorization: Bearer {access_token}")),
            curl_quote("anthropic-beta: oauth-2025-04-20"),
            curl_quote("Accept: application/json"),
            curl_quote(&format!("User-Agent: corgi/{}", env!("CARGO_PKG_VERSION"))),
        );
        input
            .write_all(config.as_bytes())
            .context("write curl config")?;
    }
    let output = child.wait_with_output().context("wait for curl")?;
    if !output.status.success() {
        let detail = String::from_utf8_lossy(&output.stderr).trim().to_string();
        bail!(
            "Claude usage request failed: {}",
            if detail.is_empty() {
                format!("curl exited with {}", output.status)
            } else {
                detail
            }
        );
    }
    let stdout = String::from_utf8(output.stdout).context("Claude usage response is not UTF-8")?;
    let (body, status) = split_curl_status(&stdout)?;
    match status {
        200..=299 => Ok(body.to_string()),
        401 | 403 => bail!("Claude login was rejected; run Claude Code once to refresh it"),
        _ => bail!(
            "Claude usage endpoint answered HTTP {status}: {}",
            summarize_body(body)
        ),
    }
}

fn split_curl_status(stdout: &str) -> Result<(&str, u16)> {
    let (body, status) = stdout
        .trim_end()
        .rsplit_once('\n')
        .unwrap_or(("", stdout.trim_end()));
    let status = status
        .trim()
        .parse::<u16>()
        .with_context(|| format!("curl did not report an HTTP status (got {status:?})"))?;
    Ok((body, status))
}

/// Pulls the server's error message out of a JSON error body when possible and
/// keeps the rest short enough for the status line.
fn summarize_body(body: &str) -> String {
    let body = body.trim();
    if body.is_empty() {
        return "empty response".into();
    }
    let message = serde_json::from_str::<Value>(body).ok().and_then(|value| {
        ["/error/message", "/message", "/error", "/detail"]
            .into_iter()
            .find_map(|pointer| {
                value
                    .pointer(pointer)
                    .and_then(Value::as_str)
                    .map(str::to_string)
            })
    });
    let text = message.unwrap_or_else(|| body.replace(['\n', '\r'], " "));
    text.chars().take(160).collect()
}

fn curl_quote(value: &str) -> String {
    let mut quoted = String::with_capacity(value.len() + 2);
    quoted.push('"');
    for character in value.chars() {
        match character {
            '"' => quoted.push_str("\\\""),
            '\\' => quoted.push_str("\\\\"),
            '\n' => quoted.push_str("\\n"),
            '\r' => quoted.push_str("\\r"),
            other => quoted.push(other),
        }
    }
    quoted.push('"');
    quoted
}

fn parse_claude_usage(response: &Value, subscription_type: Option<&str>) -> Result<PlanUsage> {
    let five_hour = parse_claude_window(response.get("five_hour"));
    let week = parse_claude_window(response.get("seven_day"));
    if five_hour.is_none() && week.is_none() {
        bail!("Claude did not return any usage windows");
    }
    Ok(PlanUsage {
        provider: Provider::Claude,
        plan_type: subscription_type
            .map(display_plan_type)
            .unwrap_or_else(|| Provider::Claude.label().into()),
        five_hour,
        week,
        banked_resets: None,
    })
}

fn parse_claude_window(value: Option<&Value>) -> Option<UsageWindow> {
    let value = value?;
    if value.is_null() {
        return None;
    }
    let used_percent = value
        .get("utilization")
        .and_then(Value::as_f64)?
        .clamp(0.0, 100.0)
        .round() as u8;
    let resets_at = match value.get("resets_at") {
        Some(Value::String(text)) => parse_rfc3339(text).unwrap_or_default(),
        Some(Value::Number(number)) => number.as_u64().unwrap_or_default(),
        _ => 0,
    };
    Some(UsageWindow {
        used_percent,
        resets_at,
    })
}

fn display_plan_type(value: &str) -> String {
    let mut characters = value.chars();
    let Some(first) = characters.next() else {
        return String::new();
    };
    format!("{}{}", first.to_uppercase(), characters.as_str())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{
        AppServer, PlanUsage, Provider, UsageWindow, curl_quote, parse_claude_credentials,
        parse_claude_usage, parse_codex_models, parse_codex_thread_title, parse_codex_usage,
        read_codex_thread_titles, split_curl_status, summarize_body, wait_for_response,
    };
    use crate::test_support::ScratchDir;

    /// `CORGI_CODEX_BIN` is process-wide, so every test that overrides it
    /// must hold this lock for as long as the override is in effect. Nothing
    /// else in this crate reads that variable, so unrelated tests are
    /// unaffected and keep running in parallel.
    static CODEX_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    struct FakeCodex {
        _guard: std::sync::MutexGuard<'static, ()>,
        _scratch: ScratchDir,
    }

    /// Writes an executable `sh` script standing in for `codex app-server`
    /// and points `CORGI_CODEX_BIN` at it until the returned value drops.
    fn fake_codex_app_server(script: &str) -> FakeCodex {
        let guard = CODEX_ENV_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let scratch = ScratchDir::new("fake-codex");
        let path = scratch.join("codex");
        std::fs::write(&path, format!("#!/bin/sh\n{script}\n")).expect("write fake codex script");
        let mut permissions = std::fs::metadata(&path)
            .expect("stat fake codex script")
            .permissions();
        std::os::unix::fs::PermissionsExt::set_mode(&mut permissions, 0o755);
        std::fs::set_permissions(&path, permissions).expect("make fake codex script executable");
        // SAFETY: CODEX_ENV_LOCK ensures no other test observes this value
        // while it's set.
        unsafe {
            std::env::set_var("CORGI_CODEX_BIN", &path);
        }
        FakeCodex {
            _guard: guard,
            _scratch: scratch,
        }
    }

    #[test]
    fn codex_protocol_ignores_non_json_launcher_noise() {
        let (tx, rx) = std::sync::mpsc::channel();
        tx.send("mise ~/.config/mise/config.toml tools: codex@0.151.0".to_string())
            .expect("send launcher line");
        tx.send(r#"{"id":2,"result":{"data":[]}}"#.to_string())
            .expect("send response");

        assert_eq!(
            wait_for_response(&rx, 2).expect("parse JSON-RPC response"),
            json!({ "data": [] })
        );
    }

    #[test]
    fn parses_visible_codex_models_and_the_next_page_cursor() {
        let (models, next_cursor) = parse_codex_models(&json!({
            "data": [
                { "id": "default", "model": "gpt-5.6-sol", "hidden": false },
                { "id": "hidden", "model": "gpt-5.6-internal", "hidden": true },
                { "id": "legacy", "hidden": false }
            ],
            "nextCursor": "second-page"
        }))
        .expect("model response parses");

        assert_eq!(models, ["gpt-5.6-sol", "legacy"]);
        assert_eq!(next_cursor.as_deref(), Some("second-page"));
    }

    #[test]
    fn reads_a_generated_codex_thread_title_when_one_is_available() {
        assert_eq!(
            parse_codex_thread_title(&json!({
                "thread": { "name": "  Investigate Codex task titles  " }
            })),
            Some("Investigate Codex task titles".into())
        );
        assert_eq!(
            parse_codex_thread_title(&json!({ "thread": { "name": null } })),
            None
        );
    }

    #[test]
    #[ignore = "requires a locally authenticated Codex CLI"]
    fn live_codex_model_catalog() {
        let models = super::read_codex_models().expect("read the Codex model catalog");
        assert!(models.iter().any(|model| model.starts_with("gpt-")));
    }

    #[test]
    fn parses_codex_plan_windows_and_banked_resets() {
        let usage = parse_codex_usage(
            &json!({ "account": { "type": "chatgpt", "planType": "pro" } }),
            &json!({
                "rateLimits": {
                    "primary": { "usedPercent": 31, "resetsAt": 1_800_000_000 },
                    "secondary": { "usedPercent": 7, "resetsAt": 1_800_500_000 }
                },
                "rateLimitResetCredits": { "availableCount": 2 }
            }),
        )
        .expect("parse rate limits");

        assert_eq!(
            usage,
            PlanUsage {
                provider: Provider::Codex,
                plan_type: "Pro".into(),
                five_hour: Some(UsageWindow {
                    used_percent: 31,
                    resets_at: 1_800_000_000
                }),
                week: Some(UsageWindow {
                    used_percent: 7,
                    resets_at: 1_800_500_000
                }),
                banked_resets: Some(2),
            }
        );
    }

    #[test]
    fn parses_claude_usage_windows() {
        let usage = parse_claude_usage(
            &json!({
                "five_hour": { "utilization": 12.4, "resets_at": "2027-01-15T10:00:00Z" },
                "seven_day": { "utilization": 55.6, "resets_at": "2027-01-20T00:00:00.000000+00:00" },
                "seven_day_opus": null
            }),
            Some("max"),
        )
        .expect("parse claude usage");

        assert_eq!(
            usage,
            PlanUsage {
                provider: Provider::Claude,
                plan_type: "Max".into(),
                five_hour: Some(UsageWindow {
                    used_percent: 12,
                    resets_at: 1_800_007_200
                }),
                week: Some(UsageWindow {
                    used_percent: 56,
                    resets_at: 1_800_403_200
                }),
                banked_resets: None,
            }
        );
    }

    #[test]
    fn claude_usage_without_windows_is_an_error() {
        assert!(parse_claude_usage(&json!({ "five_hour": null }), None).is_err());
    }

    #[test]
    fn reads_claude_credentials_file_shape() {
        let credentials = parse_claude_credentials(
            r#"{"claudeAiOauth":{"accessToken":"sk-ant-oat01-test","refreshToken":"r","expiresAt":1900000000000,"scopes":["user:inference"],"subscriptionType":"pro"}}"#,
        )
        .expect("parse credentials");
        assert_eq!(credentials.access_token, "sk-ant-oat01-test");
        assert_eq!(credentials.subscription_type.as_deref(), Some("pro"));
        assert_eq!(credentials.expires_at_ms, Some(1_900_000_000_000));
        assert!(parse_claude_credentials(r#"{"other":{}}"#).is_err());
    }

    #[test]
    fn curl_output_is_split_into_body_and_status() {
        let (body, status) = split_curl_status("{\"a\":1}\n200\n").expect("split");
        assert_eq!((body, status), ("{\"a\":1}", 200));
        let (body, status) = split_curl_status("\n429").expect("split empty body");
        assert_eq!((body, status), ("", 429));
        assert!(split_curl_status("no status").is_err());
    }

    #[test]
    fn error_bodies_are_summarized() {
        assert_eq!(
            summarize_body(r#"{"error":{"type":"rate_limit_error","message":"slow down"}}"#),
            "slow down"
        );
        assert_eq!(summarize_body("   "), "empty response");
        assert_eq!(summarize_body("plain\ntext"), "plain text");
    }

    #[test]
    fn curl_config_values_are_quoted() {
        assert_eq!(curl_quote(r#"a"b\c"#), r#""a\"b\\c""#);
        assert_eq!(curl_quote("\n%{http_code}"), r#""\n%{http_code}""#);
    }

    #[test]
    fn the_wrapper_completes_a_full_round_trip_against_a_fake_app_server() {
        let _scratch = fake_codex_app_server(
            r#"
            while IFS= read -r line; do
                id=$(printf '%s' "$line" | sed -n 's/.*"id":\([0-9]*\).*/\1/p')
                case "$line" in
                    *'"method":"initialize"'*)
                        printf '{"id":%s,"result":{}}\n' "$id" ;;
                    *'"method":"account/read"'*)
                        printf '{"id":%s,"result":{"account":{"planType":"pro"}}}\n' "$id" ;;
                    *'"method":"account/rateLimits/read"'*)
                        printf '{"id":%s,"result":{"rateLimits":{"primary":{"usedPercent":31,"resetsAt":100},"secondary":{"usedPercent":7,"resetsAt":200}}}}\n' "$id" ;;
                esac
            done
            "#,
        );

        let usage = super::read_codex_usage().expect("read usage from the fake app server");

        assert_eq!(
            usage,
            PlanUsage {
                provider: Provider::Codex,
                plan_type: "Pro".into(),
                five_hour: Some(UsageWindow {
                    used_percent: 31,
                    resets_at: 100
                }),
                week: Some(UsageWindow {
                    used_percent: 7,
                    resets_at: 200
                }),
                banked_resets: Some(0),
            }
        );
    }

    #[test]
    fn the_wrapper_kills_the_child_and_returns_the_error_when_a_request_fails() {
        let _scratch = fake_codex_app_server(
            r#"
            while IFS= read -r line; do
                id=$(printf '%s' "$line" | sed -n 's/.*"id":\([0-9]*\).*/\1/p')
                case "$line" in
                    *'"method":"initialize"'*)
                        printf '{"id":%s,"result":{}}\n' "$id" ;;
                    *'"method":"thread/read"'*)
                        printf '{"id":%s,"error":{"message":"thread/read is not supported"}}\n' "$id" ;;
                esac
            done
            "#,
        );

        // If the child were left running on this early return, it would still
        // be blocked reading its next line and the test would hang until
        // REQUEST_TIMEOUT; returning promptly demonstrates AppServer's Drop
        // tore it down instead.
        let error = read_codex_thread_titles(&["thread-1".into()])
            .expect_err("an app-server error should surface as an error");
        assert!(error.to_string().contains("thread/read is not supported"));
    }

    #[test]
    fn dropping_the_wrapper_kills_a_child_blocked_on_its_next_request() {
        let _scratch = fake_codex_app_server(
            r#"
            while IFS= read -r line; do
                id=$(printf '%s' "$line" | sed -n 's/.*"id":\([0-9]*\).*/\1/p')
                case "$line" in
                    *'"method":"initialize"'*)
                        printf '{"id":%s,"result":{}}\n' "$id" ;;
                esac
            done
            "#,
        );

        let server = AppServer::start().expect("start the fake app server");
        let pid = server.child.id();
        drop(server);

        // The child was blocked on its next `read`, waiting for a request
        // that never comes; Drop must have killed and reaped it rather than
        // leaving it running.
        let status = std::process::Command::new("ps")
            .args(["-p", &pid.to_string()])
            .status()
            .expect("run ps");
        assert!(!status.success(), "child process {pid} is still running");
    }

    /// Talks to the real local Claude Code login and Anthropic's usage
    /// endpoint. Run by hand with
    /// `cargo test -- --ignored live_claude_usage --nocapture`.
    #[test]
    #[ignore]
    fn live_claude_usage() {
        let usage = super::read_plan_usage(Provider::Claude).expect("read live Claude usage");
        println!("{usage:#?}");
        assert!(usage.five_hour.is_some() || usage.week.is_some());
    }
}
