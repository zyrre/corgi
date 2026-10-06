//! Whether a supervisor's input box holds text the user is writing, read from
//! its visible screen, so the dashboard never types a wake into a draft.
//!
//! The screen is read with its styling, because plain text cannot tell a
//! draft from what a harness shows dim in an empty box: Claude Code's prompt
//! suggestions and Codex's placeholder look exactly like typed text once the
//! escapes are stripped. Dim text therefore counts as empty, and anything else
//! in the box, a `[Pasted text #1]` marker included, as a draft.

use crate::harness::Harness;

/// Claude Code's input box: a line starting with this, directly below a
/// horizontal rule, and running to the next rule.
const CLAUDE_PROMPT: char = '❯';
const CLAUDE_RULE: char = '─';
/// Codex's composer: the last line starting with this, running to the next
/// blank line. Its history marks the user's past prompts the same way, but
/// always above the composer.
const CODEX_PROMPT: char = '›';

/// Whether the input box on `screen`, an ANSI read of a `harness` agent's
/// visible screen, holds a draft. `None` when the harness's box is unknown or
/// not on the screen.
pub(super) fn holds_draft(harness: &Harness, screen: &str) -> Option<bool> {
    let lines: Vec<Vec<(char, bool)>> = screen.lines().map(styled_chars).collect();
    let text = |line: &[(char, bool)]| line.iter().map(|&(c, _)| c).collect::<String>();
    let is_rule = |line: &[(char, bool)]| {
        let text = text(line);
        let text = text.trim();
        !text.is_empty() && text.chars().all(|c| c == CLAUDE_RULE)
    };
    let starts_with = |line: &[(char, bool)], prompt| line.first().map(|&(c, _)| c) == Some(prompt);
    let (start, rest) = match harness {
        Harness::Claude => {
            let start = (1..lines.len())
                .rev()
                .find(|&i| starts_with(&lines[i], CLAUDE_PROMPT) && is_rule(&lines[i - 1]))?;
            let end = (start + 1..lines.len()).find(|&i| is_rule(&lines[i]))?;
            (start, &lines[start + 1..end])
        }
        Harness::Codex => {
            let start = (0..lines.len())
                .rev()
                .find(|&i| starts_with(&lines[i], CODEX_PROMPT))?;
            let end = (start + 1..lines.len())
                .find(|&i| text(&lines[i]).trim().is_empty())
                .unwrap_or(lines.len());
            (start, &lines[start + 1..end])
        }
        _ => return None,
    };
    let typed = |line: &[(char, bool)]| line.iter().any(|&(c, dim)| !dim && !c.is_whitespace());
    Some(typed(&lines[start][1..]) || rest.iter().any(|line| typed(line)))
}

/// The characters `line` shows, each with whether it is dim, with the
/// escapes that style them taken out.
fn styled_chars(line: &str) -> Vec<(char, bool)> {
    let mut shown = Vec::new();
    let mut dim = false;
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\u{1b}' {
            if !c.is_control() {
                shown.push((c, dim));
            }
            continue;
        }
        match chars.next() {
            Some('[') => {
                let mut params = String::new();
                for next in chars.by_ref() {
                    if ('@'..='~').contains(&next) {
                        if next == 'm' {
                            dim = dim_after(dim, &params);
                        }
                        break;
                    }
                    params.push(next);
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
            _ => {}
        }
    }
    shown
}

/// Whether text is dim after the SGR sequence `params`, when it was `dim`
/// before. A color's own numbers are skipped, or the `2` of `38;2;r;g;b`
/// would read as dim.
fn dim_after(mut dim: bool, params: &str) -> bool {
    let mut params = params.split([';', ':']);
    while let Some(param) = params.next() {
        match param {
            "" | "0" | "22" => dim = false,
            "2" => dim = true,
            "38" | "48" | "58" => {
                let skip = match params.next() {
                    Some("5") => 1,
                    Some("2") => 3,
                    _ => 0,
                };
                params.by_ref().take(skip).for_each(drop);
            }
            _ => {}
        }
    }
    dim
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Screens read from Claude Code 2.1 panes through Herdr; Codex's are
    /// rebuilt from its composer's layout, as no Codex was at hand.
    const CLAUDE_EMPTY: &str = include_str!("fixtures/input_box/claude_empty.ansi");
    const CLAUDE_SUGGESTION: &str = include_str!("fixtures/input_box/claude_suggestion.ansi");
    const CLAUDE_DRAFT: &str = include_str!("fixtures/input_box/claude_draft.ansi");
    const CLAUDE_MULTILINE: &str = include_str!("fixtures/input_box/claude_multiline_draft.ansi");
    const CLAUDE_PASTED: &str = include_str!("fixtures/input_box/claude_pasted_text.ansi");
    const CODEX_EMPTY: &str = include_str!("fixtures/input_box/codex_empty.ansi");
    const CODEX_PLACEHOLDER: &str = include_str!("fixtures/input_box/codex_placeholder.ansi");
    const CODEX_DRAFT: &str = include_str!("fixtures/input_box/codex_draft.ansi");
    const CODEX_MULTILINE: &str = include_str!("fixtures/input_box/codex_multiline_draft.ansi");

    #[test]
    fn a_claude_box_holds_a_draft_only_when_it_shows_typed_text() {
        let claude = |screen| holds_draft(&Harness::Claude, screen);
        assert_eq!(claude(CLAUDE_EMPTY), Some(false));
        // A prompt suggestion is dim, and not the user's.
        assert_eq!(claude(CLAUDE_SUGGESTION), Some(false));
        assert_eq!(claude(CLAUDE_DRAFT), Some(true));
        assert_eq!(claude(CLAUDE_MULTILINE), Some(true));
        assert_eq!(claude(CLAUDE_PASTED), Some(true));
    }

    #[test]
    fn a_codex_composer_holds_a_draft_only_when_it_shows_typed_text() {
        let codex = |screen| holds_draft(&Harness::Codex, screen);
        assert_eq!(codex(CODEX_EMPTY), Some(false));
        // The placeholder is dim; the past prompt above the composer is not
        // what the box holds.
        assert_eq!(codex(CODEX_PLACEHOLDER), Some(false));
        assert_eq!(codex(CODEX_DRAFT), Some(true));
        // Typed below an empty first line.
        assert_eq!(codex(CODEX_MULTILINE), Some(true));
    }

    #[test]
    fn a_box_that_is_not_on_the_screen_is_unknown() {
        assert_eq!(holds_draft(&Harness::Claude, ""), None);
        assert_eq!(holds_draft(&Harness::Codex, "a shell prompt $"), None);
        // A menu's pointer is not the input box, nor is a box cut off at the
        // bottom of the screen.
        let menu = "Is this a folder you trust?\n❯ No, exit\n  Yes, I trust this folder";
        assert_eq!(holds_draft(&Harness::Claude, menu), None);
        assert_eq!(holds_draft(&Harness::Claude, "────\n❯ fix it"), None);
        // Another harness's screen, and one Corgi knows no box for.
        assert_eq!(holds_draft(&Harness::Claude, CODEX_DRAFT), None);
        assert_eq!(holds_draft(&Harness::Gemini, CLAUDE_DRAFT), None);
    }

    #[test]
    fn a_past_prompt_in_the_claude_transcript_is_not_the_box() {
        let screen = "❯ add retries\n\n⏺ Added them.\n\n────\n❯\u{a0}\n────\n  footer";
        assert_eq!(holds_draft(&Harness::Claude, screen), Some(false));
    }

    #[test]
    fn only_the_dim_attribute_makes_text_dim() {
        let dim = |line: &str| {
            styled_chars(line)
                .into_iter()
                .map(|(c, dim)| if dim { c.to_ascii_uppercase() } else { c })
                .collect::<String>()
        };
        assert_eq!(dim("\x1b[2ma\x1b[22mb\x1b[2;1mc\x1b[0md"), "AbCd");
        assert_eq!(dim("\x1b[38;2;2;2;2ma\x1b[48;5;2mb\x1b[2m\x1b[mc"), "abc");
        assert_eq!(dim("\x1b]0;title\x07\x1b[2mx\r"), "X");
    }
}
