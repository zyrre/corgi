//! Turn a visible terminal screen into one short, safe description of an agent's
//! current activity.  This is deliberately heuristic: Herdr tells us which kind
//! of agent owns a pane, while the terminal remains the source of the useful
//! human-facing detail.

use std::sync::LazyLock;

use regex::Regex;

use crate::{
    harness::Harness,
    model::{Activity, ActivityKind, AgentState},
};

/// A message row holds one sentence or so; longer text is cut with an ellipsis.
pub const MAX_MESSAGE_CHARS: usize = 180;
/// A tool row shows the whole command. The terminal clips it to its width, so
/// the cap only keeps a pathological argument from being copied around.
pub const MAX_TOOL_CHARS: usize = 600;
/// An entry of the expanded transcript keeps its paragraphs, because the
/// newest reply is often a summary the reader wants in full. The cap only
/// bounds what one pathological turn can hand the renderer.
pub const MAX_TRANSCRIPT_CHARS: usize = 4_000;
static SECRET_ASSIGNMENT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r#"(?i)\b(bearer|authorization|api[_ -]?key|access[_ -]?token|refresh[_ -]?token|secret|password)\s*[:=]\s*[^\s,;'\"]+"#,
    )
    .expect("valid secret assignment regex")
});
static BEARER_AUTHORIZATION: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?i)\b(authorization)\s*:\s*bearer\s+[^\s,;'\"]+"#)
        .expect("valid bearer authorization regex")
});
static KNOWN_TOKEN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"\b(?:sk-(?:proj-)?[A-Za-z0-9_-]{12,}|sk-ant-[A-Za-z0-9_-]{12,}|gh[pousr]_[A-Za-z0-9]{12,}|github_pat_[A-Za-z0-9_]{12,}|xox[baprs]-[A-Za-z0-9-]{12,}|AKIA[0-9A-Z]{12,})\b",
    )
    .expect("valid token regex")
});
static BACKTICK_CONTENT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"`([^`]+)`").expect("valid backtick regex"));

/// The newest thing said on a pane's visible screen: a question the agent is
/// waiting on, an assistant message, or a progress notice.
///
/// `detected_agent` is Herdr's plain agent detection (`codex`, `claude`, etc.).
/// It only changes how likely terminal decorations are interpreted; unknown
/// agents still get the generic, conservative rules. `None` when the screen
/// shows nothing that reads as a message, so a caller can keep what it knew.
pub fn message_from_screen(detected_agent: &str, visible_screen: &str) -> Option<Activity> {
    let lines = useful_lines(visible_screen);

    if let Some(question) = lines.iter().rev().find_map(|line| question_from_line(line)) {
        return Some(describe(
            ActivityKind::Question,
            &question,
            MAX_MESSAGE_CHARS,
        ));
    }

    if let Some(message) = lines
        .iter()
        .rev()
        .find_map(|line| assistant_message_from_line(line, detected_agent))
    {
        return Some(describe(ActivityKind::Message, &message, MAX_MESSAGE_CHARS));
    }

    lines
        .iter()
        .rev()
        .find_map(|line| progress_from_line(line))
        .map(|progress| describe(ActivityKind::Thinking, &progress, MAX_MESSAGE_CHARS))
}

/// The command a pane's visible screen shows the agent running, as far as the
/// CLI printed it. The agent's own transcript is preferred when it can be
/// read, because CLIs abbreviate long commands on screen.
pub fn command_from_screen(visible_screen: &str) -> Option<Activity> {
    useful_lines(visible_screen)
        .iter()
        .rev()
        .find_map(|line| command_from_line(line))
        .map(|command| describe(ActivityKind::Command, &command, MAX_TOOL_CHARS))
}

/// What a message row says when neither the transcript nor the screen offers
/// anything: the agent's state, in words.
pub fn message_from_state(state: AgentState) -> Activity {
    match state {
        AgentState::Blocked => describe(
            ActivityKind::Question,
            "Waiting for your input",
            MAX_MESSAGE_CHARS,
        ),
        AgentState::Working => describe(ActivityKind::Thinking, "Working", MAX_MESSAGE_CHARS),
        AgentState::Idle | AgentState::Done | AgentState::Unknown => {
            describe(ActivityKind::Ready, "Ready", MAX_MESSAGE_CHARS)
        }
    }
}

/// One safe display line: whitespace collapsed, common credentials redacted,
/// and cut at `max_chars` with an ellipsis.
/// Claude Code's folder-trust question, as it appears on a new session's
/// screen when the directory is not trusted yet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FolderTrustDialog {
    /// The directory the question asks about.
    pub path: String,
    /// Whether the highlighted answer is "Yes, I trust this folder" rather
    /// than the default "No, exit".
    pub yes_selected: bool,
}

/// Recognizes exactly Claude Code's folder-trust question and nothing else:
/// its heading, the directory it names, its safety-check wording, and its two
/// answers must all be on screen. Any other dialog is `None`.
pub fn folder_trust_dialog(screen: &str) -> Option<FolderTrustDialog> {
    let screen = strip_ansi(screen);
    let lines: Vec<&str> = screen.lines().map(str::trim).collect();
    let heading = lines
        .iter()
        .position(|line| *line == "Accessing workspace:")?;
    let path = lines[heading + 1..]
        .iter()
        .find(|line| !line.is_empty())
        .filter(|line| line.starts_with('/'))?;
    let asks = lines.iter().any(|line| {
        line.starts_with("Quick safety check: Is this a project you created or one you trust?")
    });
    let answer = |text: &str| {
        lines
            .iter()
            .find(|line| line.trim_start_matches('❯').trim() == text)
            .map(|line| line.starts_with('❯'))
    };
    let (Some(no_selected), Some(yes_selected)) =
        (answer("No, exit"), answer("Yes, I trust this folder"))
    else {
        return None;
    };
    (asks && no_selected != yes_selected).then(|| FolderTrustDialog {
        path: path.to_string(),
        yes_selected,
    })
}

pub fn describe(kind: ActivityKind, text: &str, max_chars: usize) -> Activity {
    Activity {
        kind,
        text: cap_display(&redact(&collapse_whitespace(text)), max_chars),
    }
}

/// A safe display block that keeps the shape it was written in: line breaks
/// and the spacing inside a line survive, runs of blank lines squeeze to one,
/// every line is redacted, and the whole block is cut at `max_chars`.
///
/// This is [`describe`] for text that is read rather than skimmed, such as the
/// agent's closing summary in the expanded transcript. Folding that onto one
/// line, or collapsing the spaces inside it, would cost exactly the bullet
/// lists, tables, and indented blocks that make a summary readable.
pub fn describe_block(kind: ActivityKind, text: &str, max_chars: usize) -> Activity {
    let mut lines: Vec<String> = Vec::new();
    for line in text.lines() {
        let line = redact(&line.replace('\t', "    ").replace('\u{a0}', " "));
        let line = line.trim_end().to_string();
        if line.is_empty() && lines.last().is_none_or(|last: &String| last.is_empty()) {
            continue;
        }
        lines.push(line);
    }
    while lines.last().is_some_and(|last| last.is_empty()) {
        lines.pop();
    }
    Activity {
        kind,
        text: cap_display(&lines.join("\n"), max_chars),
    }
}

fn useful_lines(screen: &str) -> Vec<String> {
    strip_ansi(screen)
        .lines()
        .map(clean_line)
        .filter(|line| !line.is_empty() && !is_chrome(line))
        .collect()
}

fn clean_line(line: &str) -> String {
    let line = line.replace('\u{a0}', " ");
    let line = line.trim();
    let line = line
        .trim_matches(|c: char| matches!(c, '│' | '┃' | '╎' | '╏'))
        .trim();
    let line = strip_spinner_prefix(line);
    redact(&collapse_whitespace(line))
}

fn strip_spinner_prefix(line: &str) -> &str {
    const SPINNERS: &[char] = &[
        '⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏', '◐', '◓', '◑', '◒', '◴', '◷', '◶', '◵',
        '✢', '✳', '✻',
    ];
    let trimmed = line.trim_start();
    match trimmed.chars().next() {
        Some(first) if SPINNERS.contains(&first) => trimmed[first.len_utf8()..].trim_start(),
        _ => trimmed,
    }
}

fn is_chrome(line: &str) -> bool {
    let lower = line.to_ascii_lowercase();
    let all_border = line.chars().all(|c| {
        c.is_whitespace()
            || matches!(
                c,
                '─' | '━'
                    | '═'
                    | '│'
                    | '┃'
                    | '║'
                    | '┌'
                    | '┐'
                    | '└'
                    | '┘'
                    | '├'
                    | '┤'
                    | '┬'
                    | '┴'
                    | '┼'
                    | '╭'
                    | '╮'
                    | '╰'
                    | '╯'
                    | '╪'
                    | '-'
                    | '='
            )
    });
    all_border
        || lower.starts_with("context window")
        || lower.starts_with("tokens:")
        || lower.starts_with("model:")
        || lower.starts_with("cwd:")
        || lower.starts_with("session:")
        || lower.contains("ctrl+c to interrupt")
        || lower.contains("esc to interrupt")
        || lower.contains("shift+tab to")
        || lower.contains("for shortcuts")
}

fn command_from_line(line: &str) -> Option<String> {
    let line = line.trim().trim_start_matches(['•', '●', '⏺']).trim_start();
    for prefix in ["$ ", "❯ ", "> "] {
        if let Some(command) = line.strip_prefix(prefix) {
            // `$` is unambiguously a shell prompt, and is not constrained to a
            // short allow-list: projects quite reasonably run custom scripts,
            // `uv`, `nix`, and many other commands. `❯` and `>` are also the
            // input lines of the agent CLIs, where the user types prompts and
            // slash commands, so those have to look like a command.
            if (prefix == "$ " && !command.trim().is_empty() && !command.ends_with('?'))
                || looks_like_command(command)
            {
                return Some(command.to_owned());
            }
        }
    }

    let lower = line.to_ascii_lowercase();
    for prefix in ["running", "executing", "ran", "command"] {
        if let Some(rest) = lower.strip_prefix(prefix) {
            let offset = line.len() - rest.len();
            let command = line[offset..].trim_start_matches([':', ' ']).trim();
            if let Some(captures) = BACKTICK_CONTENT.captures(command) {
                return captures.get(1).map(|value| value.as_str().to_owned());
            }
            if looks_like_command(command) {
                return Some(command.to_owned());
            }
        }
    }

    if let Some(open) = line.find("Bash(") {
        let rest = &line[open + "Bash(".len()..];
        let command = rest.strip_suffix(')').unwrap_or(rest).trim();
        if !command.is_empty() {
            return Some(command.trim_matches('`').to_owned());
        }
    }
    None
}

fn looks_like_command(value: &str) -> bool {
    let value = value.trim().trim_matches('`');
    if value.is_empty() || value.ends_with('?') {
        return false;
    }
    let first = value.split_whitespace().next().unwrap_or_default();
    // `/clear` and other slash commands are typed to the CLI, not to a shell;
    // an absolute path has a second separator or an extension.
    let slash_command = first.starts_with('/') && !first[1..].contains(['/', '.']);
    !slash_command
        && (first.starts_with("./")
            || first.starts_with("/")
            || first.contains('/')
            || first.starts_with("git")
            || first.starts_with("cargo")
            || first.starts_with("npm")
            || first.starts_with("pnpm")
            || first.starts_with("yarn")
            || first.starts_with("bun")
            || first.starts_with("rg")
            || first.starts_with("grep")
            || first.starts_with("ls")
            || first.starts_with("cat")
            || first.starts_with("sed")
            || first.starts_with("find")
            || first.starts_with("make")
            || first.starts_with("just")
            || first.starts_with("python")
            || first.starts_with("node")
            || first.starts_with("go")
            || first.starts_with("docker")
            || first.starts_with("kubectl")
            || first.starts_with("curl")
            || first.starts_with("ssh"))
}

fn question_from_line(line: &str) -> Option<String> {
    let lower = line.to_ascii_lowercase();
    let asks = line.ends_with('?')
        || lower.starts_with("do you ")
        || lower.starts_with("would you ")
        || lower.starts_with("should i ")
        || lower.starts_with("may i ")
        || lower.starts_with("can i ")
        || lower.starts_with("which ")
        || lower.starts_with("what ")
        || lower.starts_with("please choose")
        || lower.starts_with("select ")
        || lower.starts_with("enter ");

    asks.then(|| line.to_owned())
}

fn assistant_message_from_line(line: &str, detected_agent: &str) -> Option<String> {
    let line = line.trim();
    let lower = line.to_ascii_lowercase();
    let prefixes = Harness::from_typed(detected_agent).reply_prefixes();

    for prefix in prefixes {
        if lower.starts_with(prefix) || line.starts_with(prefix) {
            let message = line[prefix.len()..].trim_start_matches([':', ' ']).trim();
            if is_human_message(message) {
                return Some(message.to_owned());
            }
        }
    }
    None
}

fn is_human_message(message: &str) -> bool {
    let lower = message.to_ascii_lowercase();
    !message.is_empty()
        && message.chars().any(char::is_alphabetic)
        && !looks_like_command(message)
        && !lower.starts_with("thinking")
        && !lower.starts_with("working")
        && !lower.starts_with("planning")
        && !lower.starts_with("exploring")
        && !lower.starts_with("running")
        && !lower.starts_with("ran ")
}

fn progress_from_line(line: &str) -> Option<String> {
    let lower = line.to_ascii_lowercase();
    const PROGRESS: &[&str] = &[
        "thinking",
        "working",
        "planning",
        "exploring",
        "analyzing",
        "analysing",
        "reading",
        "writing",
        "searching",
        "processing",
        "reasoning",
        "waiting",
        "running",
    ];
    PROGRESS
        .iter()
        .any(|word| lower.starts_with(word) || lower.contains(&format!(" {word}")))
        .then(|| line.to_owned())
}

fn redact(text: &str) -> String {
    let text = BEARER_AUTHORIZATION.replace_all(text, "$1: [redacted]");
    let text = SECRET_ASSIGNMENT.replace_all(&text, "$1: [redacted]");
    KNOWN_TOKEN.replace_all(&text, "[redacted]").into_owned()
}

fn cap_display(text: &str, max_chars: usize) -> String {
    let mut chars = text.chars();
    let prefix: String = chars.by_ref().take(max_chars).collect();
    if chars.next().is_some() {
        let truncated: String = prefix.chars().take(max_chars - 1).collect();
        format!("{}…", truncated.trim_end())
    } else {
        prefix
    }
}

fn collapse_whitespace(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

pub(crate) fn strip_ansi(value: &str) -> String {
    let mut output = String::with_capacity(value.len());
    let mut chars = value.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch != '\u{1b}' {
            if !ch.is_control() || matches!(ch, '\n' | '\t') {
                output.push(ch);
            }
            continue;
        }

        match chars.next() {
            Some('[') => {
                for next in chars.by_ref() {
                    if ('@'..='~').contains(&next) {
                        break;
                    }
                }
            }
            Some(']') => {
                while let Some(next) = chars.next() {
                    if next == '\u{7}' {
                        break;
                    }
                    if next == '\u{1b}' && chars.peek() == Some(&'\\') {
                        chars.next();
                        break;
                    }
                }
            }
            Some(_) | None => {}
        }
    }
    output
}

#[cfg(test)]
mod tests {
    const TRUST_SCREEN: &str = "\
 claude --model haiku

 Accessing workspace:

 /Users/me/.herdr/worktrees/weather/worktree-calm-valley-7479

 Quick safety check: Is this a project you created or one you trust? (Like your own code, a well-known open source project, or work from your
 team). If not, take a moment to review what's in this folder first.

 Claude Code'll be able to read, edit, and execute files here.

 Security guide

 ❯ No, exit
   Yes, I trust this folder

 Enter to confirm · Esc to cancel";

    #[test]
    fn only_claude_codes_folder_trust_question_is_recognized() {
        assert_eq!(
            super::folder_trust_dialog(TRUST_SCREEN),
            Some(super::FolderTrustDialog {
                path: "/Users/me/.herdr/worktrees/weather/worktree-calm-valley-7479".into(),
                yes_selected: false,
            })
        );
        let moved = TRUST_SCREEN
            .replace("❯ No, exit", "  No, exit")
            .replace("  Yes, I trust", "❯ Yes, I trust");
        assert!(super::folder_trust_dialog(&moved).is_some_and(|dialog| dialog.yes_selected));

        // A permission prompt, or a dialog missing any part of the question,
        // is not the trust question.
        for other in [
            "Do you want to run this command?\n❯ Yes\n  No".to_string(),
            TRUST_SCREEN.replace("Quick safety check", "Heads up"),
            TRUST_SCREEN.replace("Yes, I trust this folder", "Yes"),
            TRUST_SCREEN.replace("/Users/me/", "Users/me/"),
        ] {
            assert_eq!(super::folder_trust_dialog(&other), None, "{other}");
        }
    }

    use super::*;

    #[test]
    fn the_command_and_the_message_are_read_from_the_same_screen_separately() {
        let screen =
            "⠋ Thinking about the change\nDo you want to continue?\n$ cargo test --workspace";
        assert_eq!(
            command_from_screen(screen),
            Some(Activity {
                kind: ActivityKind::Command,
                text: "cargo test --workspace".into(),
            })
        );
        assert_eq!(
            message_from_screen("codex", screen),
            Some(Activity {
                kind: ActivityKind::Question,
                text: "Do you want to continue?".into(),
            })
        );
    }

    #[test]
    fn detects_claude_bash_command_and_redacts_it() {
        let activity = command_from_screen(
            "⏺ Bash(curl -H 'Authorization: Bearer sk-proj-abcdefghijklmnop' https://example.test)",
        )
        .expect("command");
        assert_eq!(activity.kind, ActivityKind::Command);
        assert_eq!(
            activity.text,
            "curl -H 'Authorization: [redacted]' https://example.test"
        );
    }

    #[test]
    fn treats_any_raw_shell_prompt_as_a_command() {
        let activity = command_from_screen("$ uv run pytest -q").expect("command");
        assert_eq!(activity.kind, ActivityKind::Command);
        assert_eq!(activity.text, "uv run pytest -q");
        assert_eq!(command_from_screen("• I updated the parser safely."), None);
        // A slash command or a prompt typed at the CLI's input line is not
        // something the agent ran in a shell.
        assert_eq!(command_from_screen("❯ /clear"), None);
        assert_eq!(command_from_screen("> /clear"), None);
        assert_eq!(command_from_screen("❯ please rerun the failing test"), None);
        assert_eq!(
            command_from_screen("$ ./scripts/release.sh").map(|a| a.text),
            Some("./scripts/release.sh".into())
        );
        assert_eq!(
            command_from_screen("> /usr/bin/env python3 -V").map(|a| a.text),
            Some("/usr/bin/env python3 -V".into())
        );
    }

    #[test]
    fn detects_blocking_questions_before_assistant_messages() {
        let activity = message_from_screen(
            "codex",
            "• I found two possible approaches.\nWould you like me to update the lockfile?",
        )
        .expect("message");
        assert_eq!(activity.kind, ActivityKind::Question);
        assert_eq!(activity.text, "Would you like me to update the lockfile?");
    }

    #[test]
    fn understands_agent_specific_assistant_markers() {
        let codex = message_from_screen("codex", "• I updated the parser safely.").expect("codex");
        let claude = message_from_screen("claude", "⏺ The tests now pass.").expect("claude");
        assert_eq!(codex.kind, ActivityKind::Message);
        assert_eq!(codex.text, "I updated the parser safely.");
        assert_eq!(claude.kind, ActivityKind::Message);
        assert_eq!(claude.text, "The tests now pass.");
    }

    #[test]
    fn ignores_ansi_borders_and_status_chrome() {
        let screen = "\x1b[38;5;12m╭──────────╮\x1b[0m\n│ Tokens: 3k │\n│ ⠋ Exploring src/activity.rs │\n╰──────────╯\n";
        let activity = message_from_screen("generic", screen).expect("progress");
        assert_eq!(activity.kind, ActivityKind::Thinking);
        assert_eq!(activity.text, "Exploring src/activity.rs");
    }

    #[test]
    fn a_screen_without_signal_leaves_the_state_to_speak() {
        assert_eq!(message_from_screen("codex", "\n\n"), None);
        assert_eq!(command_from_screen("\n\n"), None);
        assert_eq!(
            message_from_state(AgentState::Blocked),
            Activity {
                kind: ActivityKind::Question,
                text: "Waiting for your input".into(),
            }
        );
        assert_eq!(
            message_from_state(AgentState::Done),
            Activity {
                kind: ActivityKind::Ready,
                text: "Ready".into(),
            }
        );
    }

    #[test]
    fn a_transcript_block_keeps_its_shape_and_still_redacts() {
        let summary =
            "Done.\n\n\n- Fixed the parser\n- token=ghp_abcdefghijklmnop\n\n  | a | b |\n";

        let block = describe_block(ActivityKind::Message, summary, MAX_TRANSCRIPT_CHARS);

        assert_eq!(block.kind, ActivityKind::Message);
        assert_eq!(
            block.text,
            // The blank run squeezes to one, the trailing blank goes, the
            // indented table row keeps its columns, and the token is gone.
            "Done.\n\n- Fixed the parser\n- token=[redacted]\n\n  | a | b |"
        );

        let long = describe_block(
            ActivityKind::Message,
            &"x".repeat(5_000),
            MAX_TRANSCRIPT_CHARS,
        );
        assert!(long.text.chars().count() <= MAX_TRANSCRIPT_CHARS);
        assert!(long.text.ends_with('…'));
    }

    #[test]
    fn redacts_common_tokens_and_caps_display_text() {
        let long = format!("Assistant: token=ghp_abcdefghijklmnop {}", "x".repeat(220));
        let activity = message_from_screen("generic", &long).expect("message");
        assert_eq!(activity.kind, ActivityKind::Message);
        assert!(activity.text.contains("[redacted]"));
        assert!(activity.text.ends_with('…'));
        assert!(activity.text.chars().count() <= MAX_MESSAGE_CHARS);

        // A tool row keeps far more of its text, and folds a multi-line
        // command onto the one line it has.
        let command = format!("cat <<'EOF' > notes.txt\n{}\nEOF", "y".repeat(300));
        let tool = describe(ActivityKind::Command, &command, MAX_TOOL_CHARS);
        assert!(tool.text.starts_with("cat <<'EOF' > notes.txt yyyy"));
        assert!(!tool.text.contains('\n'));
        assert!(!tool.text.ends_with('…'));
    }
}
