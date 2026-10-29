//! Which keys the focus view keeps for itself. Everything else goes to the
//! pane, where Herdr encodes it for the program's keyboard mode.
//!
//! The view's own keys are chords no terminal program reads: on macOS the
//! Command key, which never reaches a pane, and elsewhere Ctrl+Shift, the
//! terminal emulators' own namespace (Ctrl+Shift+C and V). Plain Esc always
//! goes to the pane: Claude Code and Codex need it.

use gpui::Keystroke;

/// A key the focus view handles itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Chord {
    /// Back to the overview: Cmd+Esc on macOS, Ctrl+Shift+Esc elsewhere.
    Leave,
    /// Show or hide the side panel: Cmd+\ or Ctrl+Shift+\.
    ToggleSide,
    /// Copy the selection: Cmd+C or Ctrl+Shift+C.
    Copy,
    /// Paste the clipboard: Cmd+V, Ctrl+Shift+V or Shift+Insert.
    Paste,
    /// Scroll the pane's history a page: Shift+PageUp and Shift+PageDown,
    /// which reach Herdr's scrollback even when the program reads PageUp.
    Page { up: bool },
}

/// The chord `keystroke` is, on macOS (`mac`) or elsewhere.
pub fn chord(keystroke: &Keystroke, mac: bool) -> Option<Chord> {
    let m = keystroke.modifiers;
    let key = keystroke.key.as_str();
    // The view's modifier, and nothing else but Shift where it is part of it.
    let ours = if mac {
        m.platform && !m.control && !m.alt
    } else {
        m.control && m.shift && !m.alt && !m.platform
    };
    if ours {
        let chord = match key {
            "escape" => Chord::Leave,
            "\\" | "|" => Chord::ToggleSide,
            "c" | "C" if !mac || !m.shift => Chord::Copy,
            "v" | "V" if !mac || !m.shift => Chord::Paste,
            _ => return None,
        };
        return Some(chord);
    }
    let only_shift = m.shift && !m.control && !m.alt && !m.platform;
    match key {
        "insert" if only_shift => Some(Chord::Paste),
        "pageup" if only_shift => Some(Chord::Page { up: true }),
        "pagedown" if only_shift => Some(Chord::Page { up: false }),
        _ => None,
    }
}

/// How the leave chord reads on this platform, for the button that says it.
pub fn leave_label(mac: bool) -> &'static str {
    if mac { "⌘⎋" } else { "Ctrl+Shift+Esc" }
}

/// How the side panel's chord reads.
pub fn side_label(mac: bool) -> &'static str {
    if mac { "⌘\\" } else { "Ctrl+Shift+\\" }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(text: &str) -> Keystroke {
        Keystroke::parse(text).unwrap()
    }

    #[test]
    fn the_views_chords_use_command_on_macos_and_ctrl_shift_elsewhere() {
        assert_eq!(chord(&key("cmd-escape"), true), Some(Chord::Leave));
        assert_eq!(chord(&key("ctrl-shift-escape"), false), Some(Chord::Leave));
        assert_eq!(chord(&key("cmd-\\"), true), Some(Chord::ToggleSide));
        assert_eq!(chord(&key("ctrl-shift-\\"), false), Some(Chord::ToggleSide));
        assert_eq!(chord(&key("cmd-c"), true), Some(Chord::Copy));
        assert_eq!(chord(&key("ctrl-shift-c"), false), Some(Chord::Copy));
        assert_eq!(chord(&key("cmd-v"), true), Some(Chord::Paste));
        assert_eq!(chord(&key("ctrl-shift-v"), false), Some(Chord::Paste));
        assert_eq!(chord(&key("shift-insert"), false), Some(Chord::Paste));
        assert_eq!(
            chord(&key("shift-pageup"), true),
            Some(Chord::Page { up: true })
        );
        assert_eq!(
            chord(&key("shift-pagedown"), false),
            Some(Chord::Page { up: false })
        );
    }

    #[test]
    fn keys_a_terminal_program_reads_go_to_the_pane() {
        // Esc, Shift+Enter, Alt+Enter, Ctrl+J, Ctrl+C and Ctrl+V are the
        // program's, on both platforms.
        for text in [
            "escape",
            "shift-enter",
            "alt-enter",
            "ctrl-j",
            "ctrl-c",
            "ctrl-v",
            "ctrl-escape",
            "pageup",
            "\\",
        ] {
            assert_eq!(chord(&key(text), true), None, "{text} on macOS");
            assert_eq!(chord(&key(text), false), None, "{text} elsewhere");
        }
        // The other platform's chord is a plain key here.
        assert_eq!(chord(&key("ctrl-shift-c"), true), None);
        assert_eq!(chord(&key("cmd-c"), false), None);
    }
}
