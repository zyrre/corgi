//! Everything Corgi knows about one agent harness: its Herdr kind and title,
//! where its CLI is, the flags it starts on, its offline models, what it
//! supports, how a handler runs on it, and which session file it writes.
//!
//! Herdr reports an agent's kind as a string and the new-agent form takes a
//! harness typed by hand, so a kind Corgi knows nothing about stays
//! [`Harness::Other`] and keeps working with the defaults every harness
//! shares.

use std::{
    env, fmt, fs,
    path::{Path, PathBuf},
    process::Command,
};

use anyhow::{Context, Result, bail};
use serde_json::Value;

use crate::{activity::strip_ansi, paths::home, usage::read_codex_models};

/// The flag every supported harness CLI takes to start on a given model.
const MODEL_FLAG: &str = "--model";
/// Codex takes its reasoning level and folder trust as `-c` configuration
/// overrides.
const CODEX_CONFIG_FLAG: &str = "-c";
const CODEX_REASONING_EFFORT_CONFIG: &str = "model_reasoning_effort";
/// Claude Code takes its effort level as a plain flag.
const CLAUDE_EFFORT_FLAG: &str = "--effort";

/// One agent harness, by the kind Herdr reports for it.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Harness {
    Claude,
    Codex,
    Gemini,
    Copilot,
    OpenCode,
    /// A kind Corgi has no knowledge of, exactly as Herdr or the user wrote it.
    Other(Box<str>),
}

/// The session file a harness writes, which Corgi reads for the model,
/// usage, and transcript.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionFormat {
    /// A Claude Code transcript under `projects/`.
    Claude,
    /// A Codex rollout under `~/.codex/sessions`.
    Codex,
}

impl Harness {
    /// The harnesses the new-agent form offers, in its order. Any other kind
    /// Herdr supports can still be typed into the field.
    pub const KNOWN: &'static [Harness] = &[
        Harness::Codex,
        Harness::Claude,
        Harness::Gemini,
        Harness::Copilot,
        Harness::OpenCode,
    ];

    /// The harnesses a handler runs on, the first being the default.
    pub const HANDLERS: &'static [Harness] = &[Harness::Claude, Harness::Codex];

    /// The harness Herdr reports as `kind`. The match is exact, so every kind
    /// comes back from [`Harness::kind`] as it went in.
    pub fn from_kind(kind: &str) -> Self {
        match kind {
            "claude" => Self::Claude,
            "codex" => Self::Codex,
            "gemini" => Self::Gemini,
            "copilot" => Self::Copilot,
            "opencode" => Self::OpenCode,
            other => Self::Other(other.into()),
        }
    }

    /// The harness a user typed, in any case and with stray spaces.
    pub fn from_typed(text: &str) -> Self {
        Self::from_kind(&text.trim().to_lowercase())
    }

    /// Herdr's kind for this harness, which is also its CLI's name.
    pub fn kind(&self) -> &str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
            Self::Gemini => "gemini",
            Self::Copilot => "copilot",
            Self::OpenCode => "opencode",
            Self::Other(kind) => kind,
        }
    }

    /// The harness's name as its vendor writes it.
    pub fn title(&self) -> &str {
        match self {
            Self::Claude => "Claude",
            Self::Codex => "Codex",
            Self::Gemini => "Gemini",
            Self::Copilot => "Copilot",
            Self::OpenCode => "OpenCode",
            Self::Other(kind) => kind,
        }
    }

    /// The terminal titles the CLI sets before its first turn, which only
    /// name it and never summarize a task.
    pub fn cli_titles(&self) -> &'static [&'static str] {
        match self {
            Self::Claude => &["claude", "claude code"],
            Self::Codex => &["codex", "codex cli"],
            Self::Gemini => &["gemini", "gemini cli"],
            Self::Copilot => &["copilot"],
            Self::OpenCode => &["opencode"],
            Self::Other(_) => &[],
        }
    }

    /// Locates the harness's CLI. `CORGI_CODEX_BIN` / `CORGI_CLAUDE_BIN` win,
    /// then `PATH`, then the usual per-user and package-manager directories.
    /// The fallback matters because Herdr runs plugin panes with a minimal
    /// `PATH` (`/usr/bin:/bin:/usr/sbin:/sbin`).
    pub fn executable(&self) -> Option<PathBuf> {
        let override_var = match self {
            Self::Codex => Some("CORGI_CODEX_BIN"),
            Self::Claude => Some("CORGI_CLAUDE_BIN"),
            _ => None,
        };
        if let Some(path) = override_var
            .and_then(env::var_os)
            .filter(|path| !path.is_empty())
        {
            let path = PathBuf::from(path);
            return is_executable_file(&path).then_some(path);
        }
        find_executable(self.kind())
    }

    /// The known harnesses whose CLI [`Harness::executable`] finds, in the
    /// form's order.
    pub fn installed() -> Vec<Harness> {
        Self::KNOWN
            .iter()
            .filter(|harness| harness.executable().is_some())
            .cloned()
            .collect()
    }

    /// The CLI to run: the one [`Harness::executable`] finds, or else its
    /// bare name left to `PATH`, so a missing CLI fails as it always did.
    pub fn command(&self) -> PathBuf {
        self.executable()
            .unwrap_or_else(|| PathBuf::from(self.kind()))
    }

    /// Models the form offers when none were discovered, on top of the
    /// harness default the CLI picks for itself. A model the CLI knows but
    /// this list does not can still be typed into the field.
    pub fn fallback_models(&self) -> &'static [&'static str] {
        match self {
            Self::Claude => &["opus", "opus[1m]", "sonnet", "sonnet[1m]", "haiku", "fable"],
            Self::Codex => &["gpt-6-luna", "gpt-6-sol", "gpt-6-astra"],
            Self::Gemini => &["gemini-2.5-pro", "gemini-2.5-flash"],
            Self::Copilot => &["claude-sonnet-4.5", "gpt-5"],
            Self::OpenCode | Self::Other(_) => &[],
        }
    }

    /// Whether the CLI takes an effort level at start: Codex as a config
    /// override, Claude Code as `--effort`. Both use the same five levels.
    pub fn supports_effort(&self) -> bool {
        matches!(self, Self::Codex | Self::Claude)
    }

    /// Whether the harness has a local, account-aware model-discovery
    /// surface. Others keep their fallback list and free-text entry.
    pub fn supports_model_discovery(&self) -> bool {
        matches!(self, Self::Codex | Self::OpenCode)
    }

    /// The harnesses a handler runs on, as one phrase: `claude or codex`.
    pub fn handler_kinds() -> String {
        Self::HANDLERS
            .iter()
            .map(Harness::kind)
            .collect::<Vec<_>>()
            .join(" or ")
    }

    /// Whether a handler can run on this harness.
    pub fn supports_handler(&self) -> bool {
        Self::HANDLERS.contains(self)
    }

    /// The models the harness's CLI currently offers. Codex asks its
    /// authenticated app-server; OpenCode lists its own provider cache.
    pub fn discover_models(&self) -> Result<Vec<String>> {
        match self {
            Self::Codex => read_codex_models(),
            Self::OpenCode => read_opencode_models(),
            other => bail!("{other} does not expose a model catalog"),
        }
    }

    /// The arguments that pin the CLI to one model; none when the harness
    /// default is in effect. Every harness takes `--model`.
    pub fn model_args(&self, model: &str) -> Vec<String> {
        let model = model.trim();
        if model.is_empty() {
            Vec::new()
        } else {
            vec![MODEL_FLAG.to_string(), model.to_string()]
        }
    }

    /// The arguments for an effort level; none for the harness default or a
    /// harness without an effort control.
    pub fn effort_args(&self, effort: &str) -> Vec<String> {
        let effort = effort.trim();
        if effort.is_empty() {
            return Vec::new();
        }
        match self {
            Self::Codex => vec![
                CODEX_CONFIG_FLAG.to_string(),
                format!("{CODEX_REASONING_EFFORT_CONFIG}={effort}"),
            ],
            Self::Claude => vec![CLAUDE_EFFORT_FLAG.to_string(), effort.to_string()],
            _ => Vec::new(),
        }
    }

    /// The `agent.start` arguments for a selected model and effort level.
    pub fn launch_args(&self, model: &str, effort: &str) -> Vec<String> {
        let mut args = self.model_args(model);
        args.extend(self.effort_args(effort));
        args
    }

    /// The arguments that answer the CLI's folder-trust question for the
    /// main checkout `root`, when it takes the answer on its command line.
    /// Codex does, as a `-c` override that is a whole `projects` table,
    /// because Codex splits a dotted `-c` key at every dot and project paths
    /// often have dots; it stands in for the configured table for this
    /// session only. Claude Code asks on screen instead.
    pub fn trusted_project_args(&self, root: &Path) -> Vec<String> {
        match self {
            Self::Codex => vec![
                CODEX_CONFIG_FLAG.to_string(),
                format!(
                    "projects={{{}={{trust_level=\"trusted\"}}}}",
                    toml_string(&root.to_string_lossy())
                ),
            ],
            _ => Vec::new(),
        }
    }

    /// The arguments that make a session a project's handler: its `role`
    /// (also written to `role_file`) on top of the harness's own
    /// instructions, and its `state_dir` writable next to the project.
    pub fn handler_args(&self, role_file: &Path, role: &str, state_dir: &Path) -> Vec<String> {
        let state_dir = state_dir.to_string_lossy().into_owned();
        match self {
            // Codex has no flag that appends to its system prompt. Developer
            // instructions add to Codex's own, where `model_instructions_file`
            // would replace them. Commands run in the workspace-write sandbox,
            // which keeps the handler's writes to the project and its state
            // directory; network access is what lets them reach Herdr's socket,
            // which `herdr` and `corgi` need. Anything else the sandbox refuses,
            // such as the bootstrap's `git init`, Codex asks the user to approve.
            Self::Codex => vec![
                "--sandbox".into(),
                "workspace-write".into(),
                "--ask-for-approval".into(),
                "on-request".into(),
                CODEX_CONFIG_FLAG.into(),
                "sandbox_workspace_write.network_access=true".into(),
                "--add-dir".into(),
                state_dir,
                CODEX_CONFIG_FLAG.into(),
                format!("developer_instructions={}", toml_string(role)),
            ],
            _ => vec![
                "--append-system-prompt-file".into(),
                role_file.to_string_lossy().into_owned(),
                "--add-dir".into(),
                state_dir,
            ],
        }
    }

    /// What the CLI exits on, typed as a prompt.
    pub fn exit_command(&self) -> &'static str {
        match self {
            Self::Codex => "/quit",
            _ => "/exit",
        }
    }

    /// The session file the CLI writes, if Corgi can read it.
    pub fn session_format(&self) -> Option<SessionFormat> {
        match self {
            Self::Claude => Some(SessionFormat::Claude),
            Self::Codex => Some(SessionFormat::Codex),
            _ => None,
        }
    }

    /// What starts an assistant message on the CLI's screen. A harness Corgi
    /// does not know gets the generic labels and both known ones.
    pub fn reply_prefixes(&self) -> &'static [&'static str] {
        match self {
            Self::Codex => &["codex:", "•", "●"],
            Self::Claude => &["claude:", "⏺", "●"],
            _ => &["assistant:", "agent:", "codex:", "claude:"],
        }
    }
}

impl fmt::Display for Harness {
    /// Herdr's kind, the way the harness is named everywhere Corgi shows it.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.kind())
    }
}

/// `text` as a TOML string, the form Codex parses a `-c` value in. A JSON
/// string is a TOML basic string, with the same quotes and escapes, once the
/// one control character JSON leaves raw is escaped too.
pub fn toml_string(text: &str) -> String {
    serde_json::Value::String(text.to_string())
        .to_string()
        .replace('\u{7f}', "\\u007F")
}

fn is_executable_file(path: &Path) -> bool {
    // `is_file` follows symlinks, which is what we want for Homebrew shims.
    path.is_file()
}

/// Finds the CLI named `binary` in [`candidate_dirs`], so a harness is found
/// from a Herdr plugin pane's minimal `PATH` as well as from a login shell.
pub(crate) fn find_executable(binary: &str) -> Option<PathBuf> {
    candidate_dirs()
        .into_iter()
        .map(|dir| dir.join(binary))
        .find(|candidate| is_executable_file(candidate))
}

/// `PATH` entries first, then well-known install locations.
fn candidate_dirs() -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = env::var_os("PATH")
        .map(|path| env::split_paths(&path).collect())
        .unwrap_or_default();
    if let Some(home) = home() {
        dirs.extend(
            [
                ".local/bin",
                ".claude/local",
                ".codex/bin",
                ".opencode/bin",
                ".npm-global/bin",
                ".bun/bin",
                ".cargo/bin",
                ".volta/bin",
                "bin",
            ]
            .into_iter()
            .map(|relative| home.join(relative)),
        );
        // nvm keeps one bin directory per Node version.
        if let Ok(versions) = fs::read_dir(home.join(".nvm/versions/node")) {
            dirs.extend(versions.flatten().map(|entry| entry.path().join("bin")));
        }
    }
    dirs.extend(
        ["/opt/homebrew/bin", "/usr/local/bin", "/usr/bin", "/bin"]
            .into_iter()
            .map(PathBuf::from),
    );
    dirs.dedup();
    dirs
}

/// OpenCode maintains its provider model catalog itself. Its command does not
/// currently offer machine-readable output, so accept any provider/model
/// tokens it renders, whether it uses a compact list or a table. We don't pass
/// `--refresh`: opening Corgi should never force a network refresh; users can
/// refresh OpenCode's cache on their own schedule.
fn read_opencode_models() -> Result<Vec<String>> {
    let output = Command::new(Harness::OpenCode.command())
        .arg("models")
        .output()
        .context("run opencode models")?;
    if !output.status.success() {
        let detail = String::from_utf8_lossy(&output.stderr).trim().to_string();
        bail!(
            "opencode models failed: {}",
            if detail.is_empty() {
                format!("exited with {}", output.status)
            } else {
                detail
            }
        );
    }
    let output = String::from_utf8(output.stdout).context("OpenCode model list is not UTF-8")?;
    let models = parse_opencode_models(&output);
    if models.is_empty() {
        bail!("OpenCode returned no provider/model entries")
    }
    Ok(models)
}

pub(crate) fn parse_opencode_models(output: &str) -> Vec<String> {
    let output = strip_ansi(output);
    let mut models = Vec::new();
    for token in output.split_whitespace() {
        let token = token.trim_matches(|character: char| {
            !character.is_ascii_alphanumeric() && !matches!(character, '/' | '.' | '_' | '-' | ':')
        });
        let Some((provider, model)) = token.split_once('/') else {
            continue;
        };
        let valid = !provider.is_empty()
            && !model.is_empty()
            && provider.chars().all(|character| {
                character.is_ascii_alphanumeric() || matches!(character, '.' | '_' | '-')
            })
            && model.chars().all(|character| {
                character.is_ascii_alphanumeric()
                    || matches!(character, '/' | '.' | '_' | '-' | ':')
            });
        if valid
            && !models
                .iter()
                .any(|known: &String| known.eq_ignore_ascii_case(token))
        {
            models.push(token.to_string());
        }
    }
    models
}

/// Claude Code's configuration directories, the most specific first:
/// `CLAUDE_CONFIG_DIR` when set, then `~/.claude`.
pub fn claude_config_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(dir) = env::var_os("CLAUDE_CONFIG_DIR").filter(|dir| !dir.is_empty()) {
        dirs.push(PathBuf::from(dir));
    }
    if let Some(home) = home() {
        let dir = home.join(".claude");
        if !dirs.contains(&dir) {
            dirs.push(dir);
        }
    }
    dirs
}

/// Claude Code's settings files under `dirs` that parse, the most specific
/// first: in each directory `settings.local.json`, then `settings.json`.
pub fn claude_settings(dirs: &[PathBuf]) -> Vec<Value> {
    dirs.iter()
        .flat_map(|dir| [dir.join("settings.local.json"), dir.join("settings.json")])
        .filter_map(|path| fs::read_to_string(path).ok())
        .filter_map(|raw| serde_json::from_str::<Value>(&raw).ok())
        .collect()
}

/// The first non-empty string `pick` finds in `settings`, in their order.
pub fn first_setting(settings: &[Value], pick: impl Fn(&Value) -> &Value) -> Option<String> {
    settings
        .iter()
        .filter_map(|value| pick(value).as_str())
        .map(str::trim)
        .find(|value| !value.is_empty())
        .map(str::to_string)
}

#[cfg(test)]
mod tests {
    use std::{fs, path::PathBuf};

    use serde_json::json;

    use super::*;

    #[test]
    fn every_kind_round_trips_exactly_and_only_typed_text_is_normalized() {
        for harness in Harness::KNOWN {
            assert_eq!(&Harness::from_kind(harness.kind()), harness);
            assert_eq!(harness.to_string(), harness.kind());
        }
        assert_eq!(Harness::from_kind("opencode"), Harness::OpenCode);
        // Herdr's kind is matched exactly, so any other string, in any case,
        // is kept as it was written.
        for kind in ["aider", "Codex", " claude ", "agent", ""] {
            let harness = Harness::from_kind(kind);
            assert_eq!(harness, Harness::Other(kind.into()));
            assert_eq!(harness.kind(), kind);
        }
        // What a user typed is trimmed and lower-cased first.
        assert_eq!(Harness::from_typed(" Codex "), Harness::Codex);
        assert_eq!(Harness::from_typed("CLAUDE"), Harness::Claude);
        assert_eq!(
            Harness::from_typed(" Aider "),
            Harness::Other("aider".into())
        );
    }

    #[test]
    fn the_form_offers_codex_first_and_a_handler_defaults_to_claude() {
        let kinds: Vec<&str> = Harness::KNOWN.iter().map(Harness::kind).collect();
        assert_eq!(kinds, ["codex", "claude", "gemini", "copilot", "opencode"]);
        assert_eq!(Harness::HANDLERS[0], Harness::Claude);
        assert!(Harness::Claude.supports_handler() && Harness::Codex.supports_handler());
        assert!(!Harness::Gemini.supports_handler());
        assert!(!Harness::from_kind("").supports_handler());
    }

    #[test]
    fn launch_arguments_per_harness_model_and_effort() {
        let other = Harness::Other("aider".into());
        let cases: Vec<(&Harness, &str, &str, &[&str])> = vec![
            (&Harness::Claude, "", "", &[]),
            (&Harness::Claude, " opus ", "", &["--model", "opus"]),
            (&Harness::Claude, "", " high ", &["--effort", "high"]),
            (
                &Harness::Claude,
                " opus ",
                " high ",
                &["--model", "opus", "--effort", "high"],
            ),
            (&Harness::Codex, "", "", &[]),
            (&Harness::Codex, " opus ", "", &["--model", "opus"]),
            (
                &Harness::Codex,
                "",
                " high ",
                &["-c", "model_reasoning_effort=high"],
            ),
            (
                &Harness::Codex,
                " opus ",
                " high ",
                &["--model", "opus", "-c", "model_reasoning_effort=high"],
            ),
            (&Harness::Gemini, "", "", &[]),
            (&Harness::Gemini, " opus ", "", &["--model", "opus"]),
            (&Harness::Gemini, "", " high ", &[]),
            (&Harness::Gemini, " opus ", " high ", &["--model", "opus"]),
            (&Harness::Copilot, "", "", &[]),
            (&Harness::Copilot, " opus ", "", &["--model", "opus"]),
            (&Harness::Copilot, "", " high ", &[]),
            (&Harness::Copilot, " opus ", " high ", &["--model", "opus"]),
            (&Harness::OpenCode, "", "", &[]),
            (&Harness::OpenCode, " opus ", "", &["--model", "opus"]),
            (&Harness::OpenCode, "", " high ", &[]),
            (&Harness::OpenCode, " opus ", " high ", &["--model", "opus"]),
            (&other, "", "", &[]),
            (&other, " opus ", "", &["--model", "opus"]),
            (&other, "", " high ", &[]),
            (&other, " opus ", " high ", &["--model", "opus"]),
        ];
        for (harness, model, effort, expected) in cases {
            assert_eq!(
                harness.launch_args(model, effort),
                expected,
                "{harness} {model:?} {effort:?}"
            );
            assert_eq!(
                harness.supports_effort(),
                matches!(harness, Harness::Claude | Harness::Codex),
                "{harness}"
            );
        }
    }

    #[test]
    fn only_codex_takes_folder_trust_and_quits_rather_than_exits() {
        let root = Path::new("/repos/weather");
        assert_eq!(
            Harness::Codex.trusted_project_args(root),
            [
                "-c",
                r#"projects={"/repos/weather"={trust_level="trusted"}}"#
            ]
        );
        assert!(Harness::Claude.trusted_project_args(root).is_empty());
        assert_eq!(Harness::Claude.exit_command(), "/exit");
        assert_eq!(Harness::Codex.exit_command(), "/quit");
    }

    fn handler_args(harness: &Harness) -> Vec<String> {
        harness.handler_args(
            Path::new("/state/handler/weather/ROLE.md"),
            "# You are the Project handler\nRun `corgi \"fleet\"`.\n",
            Path::new("/state/handler/weather"),
        )
    }

    #[test]
    fn a_claude_handler_appends_its_role_file_to_the_system_prompt() {
        assert_eq!(
            handler_args(&Harness::Claude),
            [
                "--append-system-prompt-file",
                "/state/handler/weather/ROLE.md",
                "--add-dir",
                "/state/handler/weather",
            ]
        );
    }

    #[test]
    fn a_codex_handler_gets_its_role_as_developer_instructions_in_a_sandbox_that_reaches_herdr() {
        assert_eq!(
            handler_args(&Harness::Codex),
            [
                "--sandbox",
                "workspace-write",
                "--ask-for-approval",
                "on-request",
                "-c",
                "sandbox_workspace_write.network_access=true",
                "--add-dir",
                "/state/handler/weather",
                "-c",
                r##"developer_instructions="# You are the Project handler\nRun `corgi \"fleet\"`.\n""##,
            ]
        );
    }

    #[test]
    fn toml_strings_escape_what_toml_forbids_raw() {
        assert_eq!(toml_string("a\"b\\c\nd\te"), r#""a\"b\\c\nd\te""#);
        assert_eq!(toml_string("\u{1}\u{7f}é"), r#""\u0001\u007Fé""#);
    }

    #[test]
    fn only_claude_and_codex_write_a_session_file_corgi_reads() {
        assert_eq!(
            Harness::Claude.session_format(),
            Some(SessionFormat::Claude)
        );
        assert_eq!(Harness::Codex.session_format(), Some(SessionFormat::Codex));
        assert_eq!(Harness::Gemini.session_format(), None);
        assert_eq!(Harness::from_kind("agent").session_format(), None);
    }

    #[test]
    fn harness_clis_are_looked_for_beyond_a_minimal_path() {
        // Herdr plugin panes run with only the system directories on PATH, so
        // every harness CLI, OpenCode's included, is also looked for where the
        // package managers and its own installer put it.
        let dirs = candidate_dirs();
        for dir in ["/opt/homebrew/bin", "/usr/local/bin"] {
            assert!(dirs.contains(&PathBuf::from(dir)), "{dir}");
        }
        if let Some(home) = home() {
            assert!(dirs.contains(&home.join(".opencode/bin")));
            assert!(dirs.contains(&home.join(".local/bin")));
        }
        assert!(find_executable("sh").is_some_and(|sh| sh.is_absolute()));
        assert_eq!(
            Harness::from_kind("corgi-no-such-cli").command(),
            PathBuf::from("corgi-no-such-cli")
        );
    }

    #[test]
    fn claude_settings_are_read_most_specific_first_across_config_dirs() {
        let root = env::temp_dir().join(format!("corgi-claude-settings-{}", std::process::id()));
        let (config, home) = (root.join("config"), root.join("home"));
        fs::create_dir_all(&config).expect("create config dir");
        fs::create_dir_all(&home).expect("create home dir");
        fs::write(config.join("settings.json"), r#"{"model":"opus"}"#).expect("write");
        fs::write(home.join("settings.local.json"), "not json").expect("write");
        fs::write(
            home.join("settings.json"),
            r#"{"model":"sonnet","effortLevel":"high"}"#,
        )
        .expect("write");

        let settings = claude_settings(&[config, home]);
        assert_eq!(
            settings,
            [
                json!({"model": "opus"}),
                json!({"model": "sonnet", "effortLevel": "high"})
            ]
        );
        assert_eq!(
            first_setting(&settings, |value| &value["model"]).as_deref(),
            Some("opus")
        );
        assert_eq!(
            first_setting(&settings, |value| &value["effortLevel"]).as_deref(),
            Some("high")
        );
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn effort_reaches_codex_and_claude_in_their_own_flags_and_no_other_harness() {
        assert_eq!(
            Harness::from_kind("codex").launch_args("gpt-5.6-sol", "xhigh"),
            [
                "--model",
                "gpt-5.6-sol",
                "-c",
                "model_reasoning_effort=xhigh"
            ]
        );
        assert_eq!(
            Harness::from_kind("claude").launch_args("opus", "high"),
            ["--model", "opus", "--effort", "high"]
        );
        assert_eq!(
            Harness::from_kind("claude").launch_args("", "xhigh"),
            ["--effort", "xhigh"]
        );
        assert_eq!(
            Harness::from_kind("gemini").launch_args("gemini-2.5-pro", "high"),
            ["--model", "gemini-2.5-pro"]
        );
        assert_eq!(
            Harness::from_kind("codex").launch_args("", ""),
            Vec::<String>::new()
        );
    }
    #[test]
    fn opencode_model_parser_accepts_colored_rows_and_deduplicates() {
        let models = crate::harness::parse_opencode_models(
            "\u{1b}[36mopenai/gpt-5.6-sol\u{1b}[0m\nopenrouter/anthropic/claude-sonnet\nopenai/gpt-5.6-sol\n",
        );

        assert_eq!(
            models,
            ["openai/gpt-5.6-sol", "openrouter/anthropic/claude-sonnet"]
        );
    }
    #[test]
    fn only_a_chosen_model_reaches_the_agent_command_line() {
        assert!(Harness::Claude.model_args("  ").is_empty());
        assert_eq!(
            Harness::Claude.model_args(" opus[1m] "),
            ["--model", "opus[1m]"]
        );
    }
}
