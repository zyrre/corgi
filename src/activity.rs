//! Text of an agent made safe to show: redacted of common credentials, on
//! one line or with its paragraphs kept, and capped. Also the one screen
//! Corgi reads for itself: Claude Code's folder-trust question.

use std::sync::LazyLock;

use regex::Regex;

use crate::model::{Activity, ActivityKind};

/// A message row holds one sentence or so; longer text is cut with an ellipsis.
pub const MAX_MESSAGE_CHARS: usize = 180;
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

pub fn strip_ansi(value: &str) -> String {
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
    fn a_transcript_block_keeps_its_shape_and_still_redacts() {
        let summary =
            "Done.\n\n\n- Fixed the parser\n- token=ghp_abcdefghijklmnop\n\n  | a | b |\n";

        let block = describe_block(ActivityKind::Message, summary, 4_000);

        assert_eq!(block.kind, ActivityKind::Message);
        assert_eq!(
            block.text,
            // The blank run squeezes to one, the trailing blank goes, the
            // indented table row keeps its columns, and the token is gone.
            "Done.\n\n- Fixed the parser\n- token=[redacted]\n\n  | a | b |"
        );

        let long = describe_block(ActivityKind::Message, &"x".repeat(5_000), 4_000);
        assert!(long.text.chars().count() <= 4_000);
        assert!(long.text.ends_with('…'));
    }

    #[test]
    fn redacts_common_tokens_and_caps_display_text() {
        let long = format!("token=ghp_abcdefghijklmnop {}", "x".repeat(220));
        let activity = describe(ActivityKind::Message, &long, MAX_MESSAGE_CHARS);
        assert_eq!(activity.kind, ActivityKind::Message);
        assert!(activity.text.contains("[redacted]"));
        assert!(activity.text.ends_with('…'));
        assert!(activity.text.chars().count() <= MAX_MESSAGE_CHARS);
    }
}
