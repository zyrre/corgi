//! Everything Corgi knows about one agent harness: its kind and title,
//! where its CLI is, the flags it starts on, its offline models, what it
//! supports, how a supervisor runs on it, and which session file it writes.
//!
//! A kind is a string (`--harness`, a terminal's `corgi.harness` label), so
//! one Corgi knows nothing about stays [`Harness::Other`] and keeps working
//! with the defaults every harness shares.

use std::{
    env, fmt, fs,
    path::{Path, PathBuf},
};

use serde_json::Value;

use crate::paths::{corgi_state_dir, home};

/// The flag every supported harness CLI takes to start on a given model.
const MODEL_FLAG: &str = "--model";
/// Codex takes its reasoning level and folder trust as `-c` configuration
/// overrides.
const CODEX_CONFIG_FLAG: &str = "-c";
const CODEX_REASONING_EFFORT_CONFIG: &str = "model_reasoning_effort";
/// Claude Code takes its effort level as a plain flag.
const CLAUDE_EFFORT_FLAG: &str = "--effort";

/// One agent harness, by its kind.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Harness {
    Claude,
    Codex,
    Gemini,
    Copilot,
    OpenCode,
    /// A kind Corgi has no knowledge of, exactly as it was written.
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
    /// The harnesses Corgi knows, in the order a default is picked from. Any
    /// other kind can still be named with `--harness`.
    pub const KNOWN: &'static [Harness] = &[
        Harness::Codex,
        Harness::Claude,
        Harness::Gemini,
        Harness::Copilot,
        Harness::OpenCode,
    ];

    /// The harnesses a supervisor runs on, the first being the default.
    pub const SUPERVISOR_HARNESSES: &'static [Harness] = &[Harness::Claude, Harness::Codex];

    /// The harness of `kind`. The match is exact, so every kind
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

    /// The kind of this harness, which is also its CLI's name.
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
    /// The fallback matters because a service such as `corgi daemon` runs
    /// with a minimal `PATH` (`/usr/bin:/bin:/usr/sbin:/sbin`).
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

    /// Whether the CLI takes an effort level at start: Codex as a config
    /// override, Claude Code as `--effort`. Both use the same five levels.
    pub fn supports_effort(&self) -> bool {
        matches!(self, Self::Codex | Self::Claude)
    }

    /// The harnesses a supervisor runs on, as one phrase: `claude or codex`.
    pub fn supervisor_kinds() -> String {
        Self::SUPERVISOR_HARNESSES
            .iter()
            .map(Harness::kind)
            .collect::<Vec<_>>()
            .join(" or ")
    }

    /// Whether a supervisor can run on this harness.
    pub fn supports_supervisor(&self) -> bool {
        Self::SUPERVISOR_HARNESSES.contains(self)
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

    /// The arguments that make a session a project's supervisor: its `role`
    /// (also written to `role_file`) on top of the harness's own
    /// instructions, and its `state_dir` writable next to the project.
    pub fn supervisor_args(&self, role_file: &Path, role: &str, state_dir: &Path) -> Vec<String> {
        match self {
            // Codex has no flag that appends to its system prompt. Developer
            // instructions add to Codex's own, where `model_instructions_file`
            // would replace them.
            Self::Codex => {
                let mut args = codex_supervisor_flags(state_dir);
                args.extend(codex_developer_instructions(role));
                args
            }
            _ => vec![
                "--append-system-prompt-file".into(),
                role_file.to_string_lossy().into_owned(),
                "--add-dir".into(),
                state_dir.to_string_lossy().into_owned(),
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
}

impl fmt::Display for Harness {
    /// Its kind, the way the harness is named everywhere Corgi shows it.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.kind())
    }
}

/// A Codex supervisor's permissions, without its role. Commands run in the
/// workspace-write sandbox, which keeps the supervisor's writes to the
/// project and its `state_dir`; network access is what lets them reach
/// herdr-core's socket, which `hc` and `corgi` need. Anything else the sandbox
/// refuses, such as the bootstrap's `git init`, Codex asks the user to
/// approve.
pub fn codex_supervisor_flags(state_dir: &Path) -> Vec<String> {
    vec![
        "--sandbox".into(),
        "workspace-write".into(),
        "--ask-for-approval".into(),
        "on-request".into(),
        CODEX_CONFIG_FLAG.into(),
        "sandbox_workspace_write.network_access=true".into(),
        "--add-dir".into(),
        state_dir.to_string_lossy().into_owned(),
    ]
}

/// The key of Codex's developer instructions, as a `-c` override.
pub const CODEX_DEVELOPER_INSTRUCTIONS: &str = "developer_instructions=";

/// `text` as Codex's developer instructions.
pub fn codex_developer_instructions(text: &str) -> [String; 2] {
    [
        CODEX_CONFIG_FLAG.into(),
        format!("{CODEX_DEVELOPER_INSTRUCTIONS}{}", toml_string(text)),
    ]
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
/// from a service's minimal `PATH` as well as from a login shell.
pub fn find_executable(binary: &str) -> Option<PathBuf> {
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

/// The reasoning levels Corgi offers when starting a Codex or Claude Code
/// session. The empty value leaves the harness or model default in charge.
pub const EFFORT_LEVELS: &[&str] = &["low", "medium", "high", "xhigh", "max"];

/// The harness a launch that names none starts on: the one someone last
/// chose ([`record_chosen_harness`]), if it is still installed; otherwise `CORGI_DEFAULT_AGENT` if
/// set, otherwise the first installed harness of [`Harness::KNOWN`],
/// otherwise `codex`.
pub fn default_harness() -> Harness {
    default_harness_from(
        last_chosen_harness(),
        std::env::var("CORGI_DEFAULT_AGENT").ok(),
        &Harness::installed(),
    )
}

fn default_harness_from(
    last_used: Option<String>,
    override_kind: Option<String>,
    installed: &[Harness],
) -> Harness {
    last_used
        .map(|kind| Harness::from_typed(&kind))
        .filter(|harness| installed.contains(harness))
        .or_else(|| {
            override_kind
                .map(|kind| Harness::from_typed(&kind))
                .filter(|harness| !harness.kind().is_empty())
        })
        .or_else(|| {
            Harness::KNOWN
                .iter()
                .find(|harness| installed.contains(harness))
                .cloned()
        })
        .unwrap_or_else(|| Harness::KNOWN[0].clone())
}

/// The file in Corgi's state directory that names the harness someone last
/// chose for a launch ([`record_chosen_harness`]).
const LAST_HARNESS_FILE: &str = "last-harness.json";

/// The harness kind in [`LAST_HARNESS_FILE`]. Tests read none, so they never
/// depend on the developer's own state.
fn last_chosen_harness() -> Option<String> {
    if cfg!(test) {
        return None;
    }
    read_last_harness(&corgi_state_dir()?.join(LAST_HARNESS_FILE))
}

/// Records `harness` as the one last chosen, once a launch on it has
/// started: `corgi supervisor`, `corgi open` and a `corgi spawn` that a
/// person ran (not a supervisor) with `--harness`. A launch that names no
/// harness chose nothing, so the record stays as it was. Failing to record
/// fails no launch, and tests of the crate record nothing, so they never
/// touch the developer's own state.
pub fn record_chosen_harness(harness: &Harness) {
    if cfg!(test) {
        return;
    }
    if let Some(dir) = corgi_state_dir() {
        let _ = write_last_harness(&dir.join(LAST_HARNESS_FILE), harness);
    }
}

/// Replaces the record at `path` with `harness` in one step: a temporary file
/// of this process's own is renamed over it, so a reader sees the old record
/// or the new one, never half of either.
fn write_last_harness(path: &Path, harness: &Harness) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let mut temporary = path.as_os_str().to_owned();
    temporary.push(format!(".{}.tmp", std::process::id()));
    let temporary = PathBuf::from(temporary);
    let record = serde_json::json!({ "harness": harness.kind() }).to_string();
    let written = fs::write(&temporary, record).and_then(|()| fs::rename(&temporary, path));
    if written.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    written
}

/// The kind recorded in the file at `path` (`{"harness":"claude"}`), or
/// `None` when there is no file, it cannot be read or parsed, or it names no
/// harness.
fn read_last_harness(path: &Path) -> Option<String> {
    let record: Value = serde_json::from_slice(&fs::read(path).ok()?).ok()?;
    let kind = Harness::from_typed(record.get("harness")?.as_str()?)
        .kind()
        .to_string();
    (!kind.is_empty()).then_some(kind)
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
        // A kind is matched exactly, so any other string, in any case,
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
    fn the_form_offers_codex_first_and_a_supervisor_defaults_to_claude() {
        let kinds: Vec<&str> = Harness::KNOWN.iter().map(Harness::kind).collect();
        assert_eq!(kinds, ["codex", "claude", "gemini", "copilot", "opencode"]);
        assert_eq!(Harness::SUPERVISOR_HARNESSES[0], Harness::Claude);
        assert!(Harness::Claude.supports_supervisor() && Harness::Codex.supports_supervisor());
        assert!(!Harness::Gemini.supports_supervisor());
        assert!(!Harness::from_kind("").supports_supervisor());
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

    fn supervisor_args(harness: &Harness) -> Vec<String> {
        harness.supervisor_args(
            Path::new("/state/supervisors/weather/ROLE.md"),
            "# You are the project's supervisor\nRun `corgi \"fleet\"`.\n",
            Path::new("/state/supervisors/weather"),
        )
    }

    #[test]
    fn a_claude_supervisor_appends_its_role_file_to_the_system_prompt() {
        assert_eq!(
            supervisor_args(&Harness::Claude),
            [
                "--append-system-prompt-file",
                "/state/supervisors/weather/ROLE.md",
                "--add-dir",
                "/state/supervisors/weather",
            ]
        );
    }

    #[test]
    fn a_codex_supervisor_gets_its_role_as_developer_instructions_in_a_sandbox_that_reaches_herdr()
    {
        assert_eq!(
            supervisor_args(&Harness::Codex),
            [
                "--sandbox",
                "workspace-write",
                "--ask-for-approval",
                "on-request",
                "-c",
                "sandbox_workspace_write.network_access=true",
                "--add-dir",
                "/state/supervisors/weather",
                "-c",
                r##"developer_instructions="# You are the project's supervisor\nRun `corgi \"fleet\"`.\n""##,
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
        // A service runs with only the system directories on PATH, so
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
    fn only_a_chosen_model_reaches_the_agent_command_line() {
        assert!(Harness::Claude.model_args("  ").is_empty());
        assert_eq!(
            Harness::Claude.model_args(" opus[1m] "),
            ["--model", "opus[1m]"]
        );
    }

    #[test]
    fn default_harness_prefers_last_used_then_override_then_installed_cli() {
        let both = [Harness::Codex, Harness::Claude];
        // The last spawn wins over the override and the installed order.
        assert_eq!(
            default_harness_from(Some("claude".into()), Some("codex".into()), &both),
            Harness::Claude
        );
        assert_eq!(
            default_harness_from(Some(" Codex ".into()), Some("claude".into()), &both),
            Harness::Codex
        );
        // A last-used harness that is gone falls back to the override, then
        // to the installed order.
        assert_eq!(
            default_harness_from(
                Some("claude".into()),
                Some("opencode".into()),
                &[Harness::Codex]
            ),
            Harness::OpenCode
        );
        assert_eq!(
            default_harness_from(Some("claude".into()), None, &[Harness::OpenCode]),
            Harness::OpenCode
        );
        assert_eq!(
            default_harness_from(Some("aider".into()), None, &both),
            Harness::Codex
        );
        assert_eq!(
            default_harness_from(Some("claude".into()), None, &[]),
            Harness::Codex
        );
        // With nothing recorded: the override, then the first installed.
        assert_eq!(
            default_harness_from(None, Some(" Claude ".into()), &[Harness::Codex]),
            Harness::Claude
        );
        assert_eq!(
            default_harness_from(None, Some(String::new()), &[Harness::Claude]),
            Harness::Claude
        );
        assert_eq!(
            default_harness_from(None, None, &[Harness::OpenCode, Harness::Codex]),
            Harness::Codex
        );
        assert_eq!(
            default_harness_from(None, None, &[Harness::OpenCode]),
            Harness::OpenCode
        );
        assert_eq!(default_harness_from(None, None, &[]), Harness::Codex);
    }

    #[test]
    fn the_last_harness_file_names_a_harness_or_nothing() {
        let scratch = crate::test_support::ScratchDir::new("last-harness");
        let path = scratch.join("last-harness.json");
        assert_eq!(read_last_harness(&path), None);
        fs::write(&path, r#"{"harness":" Claude "}"#).unwrap();
        assert_eq!(read_last_harness(&path).as_deref(), Some("claude"));
        for corrupt in ["", "{", "{}", r#"{"harness":""}"#, r#"{"harness":7}"#] {
            fs::write(&path, corrupt).unwrap();
            assert_eq!(read_last_harness(&path), None, "{corrupt:?}");
        }
    }

    #[test]
    fn a_recorded_harness_is_read_back_and_replaces_the_one_before() {
        let scratch = crate::test_support::ScratchDir::new("last-harness-write");
        // The state directory may not be there yet.
        let path = scratch.join("state/corgi").join(LAST_HARNESS_FILE);
        write_last_harness(&path, &Harness::Codex).unwrap();
        assert_eq!(read_last_harness(&path).as_deref(), Some("codex"));
        write_last_harness(&path, &Harness::Claude).unwrap();
        assert_eq!(read_last_harness(&path).as_deref(), Some("claude"));
        // ... and what it read back is the default, while installed.
        assert_eq!(
            default_harness_from(
                read_last_harness(&path),
                None,
                &[Harness::Codex, Harness::Claude]
            ),
            Harness::Claude
        );
        // No temporary file is left beside it.
        let left: Vec<_> = fs::read_dir(path.parent().unwrap())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert_eq!(left, [LAST_HARNESS_FILE]);
    }
}
