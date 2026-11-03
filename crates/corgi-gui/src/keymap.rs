//! The window's keys: which key does each action, as the user set them in
//! the hotkeys dialog (`hotkeys.rs`) or by default. The overview's and the
//! focus view's key handling read it, and so does everything that shows a
//! key (the toolbar, the focus bar, the status bar, the card frames, the
//! side panel, the dialog itself), so a key shown is always a key that
//! works. It is kept in `hotkeys.json` beside `gui-layout.json`; a missing,
//! unreadable or older file gives the defaults.
//!
//! The overview's and an expanded session's keys are one key space: both
//! are live while a row is expanded. The focus view's two are chords no
//! terminal program reads (Command on macOS, Ctrl+Shift elsewhere), as
//! every other key goes to the agent's terminal. Some keys are fixed: the
//! digits that answer questions, copy and paste, and the window's own.

use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

use gpui::Keystroke;
use serde::{Deserialize, Serialize};

/// The file's format; another number is an older or newer file, and gives
/// the defaults.
const VERSION: u32 = 1;

/// Where a key works.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Area {
    Overview,
    Transcript,
    Focus,
    /// The merge dialog's, while it shows a failure.
    MergeDialog,
    /// The window's own keys, all fixed.
    Window,
}

impl Area {
    pub const ALL: [Area; 5] = [
        Area::Overview,
        Area::Transcript,
        Area::Focus,
        Area::MergeDialog,
        Area::Window,
    ];

    pub fn title(self) -> &'static str {
        match self {
            Area::Overview => "Overview",
            Area::Transcript => "Expanded session",
            Area::Focus => "Focus view",
            Area::MergeDialog => "Merge dialog",
            Area::Window => "Window",
        }
    }

    pub fn note(self) -> &'static str {
        match self {
            Area::Overview => "While no dialog is open.",
            Area::Transcript => "While a row is expanded; the overview's keys work too.",
            Area::Focus => "Every other key goes to the agent's terminal.",
            Area::MergeDialog => "While it shows why a merge failed or cannot start.",
            Area::Window => "Everywhere.",
        }
    }

    /// The overview's keys and an expanded session's are live at once; the
    /// focus view's and the merge dialog's are each live alone.
    fn shares_keys_with(self, other: Area) -> bool {
        let alone = |area: Area| matches!(area, Area::Focus | Area::MergeDialog);
        self == other || (!alone(self) && !alone(other))
    }
}

/// Something a key does that the user can rebind.
#[derive(Clone, Copy, PartialEq, Eq, Debug, PartialOrd, Ord)]
pub enum Action {
    Next,
    Previous,
    First,
    Last,
    Open,
    Expand,
    CollapseCard,
    ExpandCard,
    SidePanel,
    NewAgent,
    Prompt,
    Supervisor,
    Merge,
    Tag,
    Close,
    Refresh,
    AnswerFirst,
    Collapse,
    ScrollUp,
    ScrollDown,
    Newest,
    Oldest,
    Leave,
    FocusSide,
    AskSupervisor,
}

impl Action {
    pub const ALL: [Action; 25] = [
        Action::Next,
        Action::Previous,
        Action::First,
        Action::Last,
        Action::Open,
        Action::Expand,
        Action::CollapseCard,
        Action::ExpandCard,
        Action::SidePanel,
        Action::NewAgent,
        Action::Prompt,
        Action::Supervisor,
        Action::Merge,
        Action::Tag,
        Action::Close,
        Action::Refresh,
        Action::AnswerFirst,
        Action::Collapse,
        Action::ScrollUp,
        Action::ScrollDown,
        Action::Newest,
        Action::Oldest,
        Action::Leave,
        Action::FocusSide,
        Action::AskSupervisor,
    ];

    /// Its name in `hotkeys.json`, which must not change.
    fn id(self) -> &'static str {
        match self {
            Action::Next => "next",
            Action::Previous => "previous",
            Action::First => "first",
            Action::Last => "last",
            Action::Open => "open",
            Action::Expand => "expand",
            Action::CollapseCard => "collapse_card",
            Action::ExpandCard => "expand_card",
            Action::SidePanel => "side_panel",
            Action::NewAgent => "new_agent",
            Action::Prompt => "prompt",
            Action::Supervisor => "supervisor",
            Action::Merge => "merge",
            Action::Tag => "tag",
            Action::Close => "close",
            Action::Refresh => "refresh",
            Action::AnswerFirst => "answer_first",
            Action::Collapse => "collapse",
            Action::ScrollUp => "scroll_up",
            Action::ScrollDown => "scroll_down",
            Action::Newest => "newest",
            Action::Oldest => "oldest",
            Action::Leave => "leave",
            Action::FocusSide => "focus_side",
            Action::AskSupervisor => "ask_supervisor",
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Action::Next => "Select next row",
            Action::Previous => "Select previous row",
            Action::First => "Select first row",
            Action::Last => "Select last row",
            Action::Open => "Open terminal (focus view)",
            Action::Expand => "Expand / collapse session",
            Action::CollapseCard => "Collapse project card",
            Action::ExpandCard => "Expand project card",
            Action::SidePanel => "Show / hide side panel",
            Action::NewAgent => "New agent",
            Action::Prompt => "Prompt agent",
            Action::Supervisor => "Start supervisor",
            Action::Merge => "Merge",
            Action::Tag => "Tag / untag ready to merge",
            Action::Close => "Close agent",
            Action::Refresh => "Refresh",
            Action::AnswerFirst => "Answer first question",
            Action::Collapse => "Collapse session",
            Action::ScrollUp => "Scroll up a view",
            Action::ScrollDown => "Scroll down a view",
            Action::Newest => "Jump to newest turn",
            Action::Oldest => "Jump to oldest turn",
            Action::Leave => "Back to the overview",
            Action::FocusSide => "Show / hide side panel",
            Action::AskSupervisor => "Ask supervisor to fix",
        }
    }

    pub fn area(self) -> Area {
        match self {
            Action::Collapse
            | Action::ScrollUp
            | Action::ScrollDown
            | Action::Newest
            | Action::Oldest => Area::Transcript,
            Action::Leave | Action::FocusSide => Area::Focus,
            Action::AskSupervisor => Area::MergeDialog,
            _ => Area::Overview,
        }
    }

    /// Its key unless the user chose another, on macOS (`mac`) or elsewhere.
    fn default_key(self, mac: bool) -> &'static str {
        match self {
            Action::Next => "j",
            Action::Previous => "k",
            Action::First => "g",
            Action::Last => "shift-g",
            Action::Open => "enter",
            Action::Expand => "space",
            Action::CollapseCard => "left",
            Action::ExpandCard => "right",
            Action::SidePanel => "\\",
            Action::NewAgent => "n",
            Action::Prompt => "p",
            Action::Supervisor => "s",
            Action::Merge => "m",
            Action::Tag => "t",
            Action::Close => "x",
            Action::Refresh => "r",
            Action::AnswerFirst => "a",
            Action::Collapse => "escape",
            Action::ScrollUp => "u",
            Action::ScrollDown => "d",
            Action::Newest => "home",
            Action::Oldest => "end",
            Action::Leave if mac => "cmd-escape",
            Action::Leave => "ctrl-shift-escape",
            Action::FocusSide if mac => "cmd-\\",
            Action::FocusSide => "ctrl-shift-\\",
            // The dashboard's key too, so it is the same everywhere.
            Action::AskSupervisor => ASK_SUPERVISOR,
        }
    }

    /// A key that does the same and cannot be changed, unless an overview
    /// action is given it: the arrows, the page keys, and Home and End
    /// while no row is expanded.
    pub fn also(self) -> Option<&'static str> {
        match self {
            Action::Next => Some("down"),
            Action::Previous => Some("up"),
            Action::First => Some("home"),
            Action::Last => Some("end"),
            Action::ScrollUp => Some("pageup"),
            Action::ScrollDown => Some("pagedown"),
            _ => None,
        }
    }
}

/// [`corgi_core::merge::ASK_SUPERVISOR_KEY`], which the dashboard's merge
/// dialog takes, as a default key here.
const ASK_SUPERVISOR: &str = "s";

/// A key and its modifiers, as compared: Shift is dropped from a typed
/// symbol (`?`, `|`) and kept on a letter (`shift-g`) or a named key.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Key {
    pub key: String,
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub cmd: bool,
}

impl Key {
    pub fn parse(source: &str) -> Option<Key> {
        Keystroke::parse(source)
            .ok()
            .map(|keystroke| Key::from_keystroke(&keystroke))
    }

    pub fn from_keystroke(keystroke: &Keystroke) -> Key {
        let m = keystroke.modifiers;
        let mut key = Key {
            key: keystroke.key.to_lowercase(),
            ctrl: m.control,
            alt: m.alt,
            shift: m.shift,
            cmd: m.platform,
        };
        let symbol = |text: &str| {
            let mut chars = text.chars();
            matches!((chars.next(), chars.next()), (Some(c), None) if !c.is_alphanumeric())
        };
        if key.shift && symbol(&key.key) {
            // Shift+\ arrives as `|` or as `\` with Shift; either reads `|`.
            if let Some(typed) = keystroke.key_char.as_deref().filter(|typed| symbol(typed)) {
                key.key = typed.to_string();
            } else if key.key == "\\" && !key.ctrl && !key.cmd {
                key.key = "|".into();
            }
            key.shift = key.ctrl || key.cmd;
        }
        // Ctrl+Shift+| is Ctrl+Shift+\ with the Shift spelled out.
        if key.shift && key.key == "|" {
            key.key = "\\".into();
        }
        key
    }

    /// Only a modifier, which no action can take.
    fn is_modifier(&self) -> bool {
        matches!(
            self.key.as_str(),
            "shift" | "control" | "alt" | "platform" | "function" | "capslock"
        )
    }

    fn modified(&self) -> bool {
        self.ctrl || self.alt || self.cmd
    }

    /// Kept in `hotkeys.json` as GPUI writes a keystroke.
    fn source(&self) -> String {
        let mut source = String::new();
        for (on, name) in [
            (self.ctrl, "ctrl-"),
            (self.alt, "alt-"),
            (self.shift, "shift-"),
            (self.cmd, "cmd-"),
        ] {
            if on {
                source.push_str(name);
            }
        }
        source.push_str(&self.key);
        source
    }

    /// How it reads on a key cap, on macOS (`mac`) or elsewhere.
    pub fn label(&self, mac: bool) -> String {
        let mut label = String::new();
        let mut join = |mac_symbol: &str, word: &str| {
            label.push_str(if mac { mac_symbol } else { word });
        };
        if self.ctrl {
            join("⌃", "Ctrl+");
        }
        if self.alt {
            join("⌥", "Alt+");
        }
        let letter = self.key.chars().count() == 1;
        // A shifted letter reads as its capital, unless with a modifier.
        let capital = self.shift && letter && !self.modified();
        if self.shift && !capital {
            join("⇧", "Shift+");
        }
        if self.cmd {
            join("⌘", "Super+");
        }
        let name = match self.key.as_str() {
            "enter" => "↵",
            "space" => "Space",
            "escape" => "Esc",
            "up" => "↑",
            "down" => "↓",
            "left" => "←",
            "right" => "→",
            "pageup" => "PgUp",
            "pagedown" => "PgDn",
            "home" => "Home",
            "end" => "End",
            "tab" => "Tab",
            "backspace" => "⌫",
            "delete" => "Del",
            "insert" => "Ins",
            // A letter with a modifier reads as a menu writes it: ⌘L.
            key if capital || (letter && self.modified()) => return label + &key.to_uppercase(),
            key => {
                let mut chars = key.chars();
                match (chars.next(), chars.as_str()) {
                    // F1…F12 and other named keys, capitalised.
                    (Some(first), rest) if !rest.is_empty() => {
                        return label + &first.to_uppercase().to_string() + rest;
                    }
                    _ => key,
                }
            }
        };
        label + name
    }
}

/// A key that is not the user's to move, and what it does.
pub struct Fixed {
    pub area: Area,
    pub name: &'static str,
    pub label: &'static str,
    keys: &'static [&'static str],
}

/// The window's fixed keys, which `window::bind_app_keys` binds: quitting
/// (`QUIT_MAC` on macOS), and on macOS what the title bar's buttons did.
pub const QUIT: &str = "ctrl-q";
pub const QUIT_MAC: &str = "cmd-q";
pub const CLOSE_WINDOW: &str = "cmd-w";
pub const MINIMIZE: &str = "cmd-m";
pub const FULL_SCREEN: &str = "ctrl-cmd-f";

/// The fixed keys, on macOS (`mac`) or elsewhere: shown in the dialog, and
/// refused for an action.
pub fn fixed(mac: bool) -> Vec<Fixed> {
    let mut fixed = vec![
        Fixed {
            area: Area::Overview,
            name: "Answer question 1–9",
            label: "1–9",
            keys: &["1", "2", "3", "4", "5", "6", "7", "8", "9"],
        },
        Fixed {
            area: Area::MergeDialog,
            name: "The default button / close",
            label: "↵ Esc",
            keys: &["enter", "escape"],
        },
    ];
    if mac {
        fixed.extend([
            Fixed {
                area: Area::Transcript,
                name: "Copy the selection",
                label: "⌘C",
                keys: &["cmd-c"],
            },
            Fixed {
                area: Area::Focus,
                name: "Copy / paste",
                label: "⌘C ⌘V",
                keys: &["cmd-c", "cmd-v"],
            },
            Fixed {
                area: Area::Window,
                name: "Quit",
                label: "⌘Q",
                keys: &[QUIT_MAC],
            },
            Fixed {
                area: Area::Window,
                name: "Close window",
                label: "⌘W",
                keys: &[CLOSE_WINDOW],
            },
            Fixed {
                area: Area::Window,
                name: "Minimize",
                label: "⌘M",
                keys: &[MINIMIZE],
            },
            Fixed {
                area: Area::Window,
                name: "Full screen",
                label: "⌃⌘F",
                keys: &[FULL_SCREEN],
            },
        ]);
    } else {
        fixed.extend([
            Fixed {
                area: Area::Transcript,
                name: "Copy the selection",
                label: "Ctrl+Shift+C",
                keys: &["ctrl-shift-c"],
            },
            Fixed {
                area: Area::Focus,
                name: "Copy / paste",
                label: "Ctrl+Shift+C/V",
                keys: &["ctrl-shift-c", "ctrl-shift-v", "shift-insert"],
            },
            Fixed {
                area: Area::Window,
                name: "Quit",
                label: "Ctrl+Q",
                keys: &[QUIT],
            },
        ]);
    }
    fixed
}

/// The keys every dialog takes, which are not the user's to move: confirm
/// and cancel, as their hints and buttons show them.
pub const ENTER: &str = "↵";
pub const ESCAPE: &str = "Esc";

/// What the key pressed for an action does.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Hit {
    Action(Action),
    /// Answer the project's `n`th question, from 0.
    Answer(usize),
}

/// Why an action cannot take a key.
#[derive(Clone, Debug, PartialEq)]
pub enum Refusal {
    /// Another action of the same keys has it; confirming swaps them.
    Clash(Action),
    /// A fixed key: what it does.
    Fixed(&'static str),
    /// The focus view's keys need the view's modifier.
    NeedsChord,
}

#[derive(Serialize, Deserialize, Default)]
struct File {
    version: u32,
    #[serde(default)]
    keys: BTreeMap<String, String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Keymap {
    mac: bool,
    keys: Vec<Key>,
}

impl Keymap {
    pub fn defaults(mac: bool) -> Self {
        Keymap {
            mac,
            keys: Action::ALL
                .iter()
                .map(|action| Key::parse(action.default_key(mac)).expect("a default key parses"))
                .collect(),
        }
    }

    pub fn path() -> Option<PathBuf> {
        corgi_core::paths::corgi_state_dir().map(|dir| dir.join("hotkeys.json"))
    }

    /// The keys kept at `path`: the defaults where the file has none, cannot
    /// be read, or is of another version, and for a key it gives that
    /// another action already has or no action can take.
    pub fn load(path: Option<&Path>, mac: bool) -> Self {
        let mut keymap = Keymap::defaults(mac);
        let Some(file) = path
            .and_then(|path| fs::read_to_string(path).ok())
            .and_then(|text| serde_json::from_str::<File>(&text).ok())
            .filter(|file| file.version == VERSION)
        else {
            return keymap;
        };
        // All at once, as a swap leaves two actions each with the other's
        // default; then any that still clashes or is refused goes back to
        // its default, until none does.
        for action in Action::ALL {
            if let Some(key) = file.keys.get(action.id()).and_then(|key| Key::parse(key)) {
                keymap.set(action, key);
            }
        }
        while let Some(action) = Action::ALL.into_iter().find(|&action| {
            !keymap.is_default(action) && keymap.check(action, keymap.key(action)).is_err()
        }) {
            keymap.reset(action);
        }
        keymap
    }

    /// Keeps the keys that differ from the defaults at `path`, through a
    /// rename.
    pub fn save(&self, path: Option<&Path>) {
        let Some(path) = path else { return };
        let defaults = Keymap::defaults(self.mac);
        let file = File {
            version: VERSION,
            keys: Action::ALL
                .iter()
                .filter(|&&action| self.key(action) != defaults.key(action))
                .map(|&action| (action.id().to_string(), self.key(action).source()))
                .collect(),
        };
        let Ok(text) = serde_json::to_string_pretty(&file) else {
            return;
        };
        if let Some(dir) = path.parent() {
            let _ = fs::create_dir_all(dir);
        }
        let temporary = path.with_extension("json.tmp");
        if fs::write(&temporary, text).is_ok() {
            let _ = fs::rename(&temporary, path);
        }
    }

    pub fn mac(&self) -> bool {
        self.mac
    }

    fn index(action: Action) -> usize {
        Action::ALL
            .iter()
            .position(|&other| other == action)
            .unwrap_or(0)
    }

    pub fn key(&self, action: Action) -> &Key {
        &self.keys[Keymap::index(action)]
    }

    /// The action's key as a key cap shows it.
    pub fn label(&self, action: Action) -> String {
        self.key(action).label(self.mac)
    }

    pub fn is_default(&self, action: Action) -> bool {
        Key::parse(action.default_key(self.mac)).as_ref() == Some(self.key(action))
    }

    pub fn default_label(&self, action: Action) -> String {
        Key::parse(action.default_key(self.mac))
            .map(|key| key.label(self.mac))
            .unwrap_or_default()
    }

    fn set(&mut self, action: Action, key: Key) {
        self.keys[Keymap::index(action)] = key;
    }

    pub fn reset(&mut self, action: Action) {
        if let Some(key) = Key::parse(action.default_key(self.mac)) {
            self.set(action, key);
        }
    }

    /// The action's fixed second key, unless an overview action has it.
    pub fn also(&self, action: Action) -> Option<Key> {
        let also = Key::parse(action.also()?)?;
        let taken = Action::ALL
            .iter()
            .any(|&other| other.area() == Area::Overview && *self.key(other) == also);
        (!taken).then_some(also)
    }

    /// Whether `action` can take `key`.
    pub fn check(&self, action: Action, key: &Key) -> Result<(), Refusal> {
        let area = action.area();
        if area == Area::Focus {
            // The view's modifier and nothing else but Shift where it is
            // part of it: any other key is the terminal program's.
            let chord = if self.mac {
                key.cmd && !key.ctrl && !key.alt
            } else {
                key.ctrl && key.shift && !key.alt && !key.cmd
            };
            if !chord {
                return Err(Refusal::NeedsChord);
            }
        }
        for fixed in fixed(self.mac) {
            let shared = fixed.area == Area::Window || fixed.area.shares_keys_with(area);
            if shared
                && fixed
                    .keys
                    .iter()
                    .any(|k| Key::parse(k).as_ref() == Some(key))
            {
                return Err(Refusal::Fixed(fixed.name));
            }
        }
        match Action::ALL.iter().find(|&&other| {
            other != action && other.area().shares_keys_with(area) && self.key(other) == key
        }) {
            Some(&other) => Err(Refusal::Clash(other)),
            None => Ok(()),
        }
    }

    /// Gives `action` `key` if it can take it; on a clash, gives the other
    /// action `action`'s old key. True when the keys changed.
    pub fn assign(&mut self, action: Action, key: Key, swap: bool) -> bool {
        match self.check(action, &key) {
            Ok(()) => {
                self.set(action, key);
                true
            }
            Err(Refusal::Clash(other)) if swap => {
                let old = self.key(action).clone();
                self.set(action, key);
                self.set(other, old);
                true
            }
            Err(_) => false,
        }
    }

    /// The action the overview takes `keystroke` for, with a row expanded
    /// or not: an expanded session's keys first, then the overview's, then
    /// the fixed second keys and the digits.
    pub fn overview_hit(&self, keystroke: &Keystroke, expanded: bool) -> Option<Hit> {
        let key = Key::from_keystroke(keystroke);
        let bound = |area: Area| {
            Action::ALL
                .iter()
                .find(|&&action| action.area() == area && *self.key(action) == key)
                .copied()
        };
        if expanded && let Some(action) = bound(Area::Transcript) {
            return Some(Hit::Action(action));
        }
        if let Some(action) = bound(Area::Overview) {
            return Some(Hit::Action(action));
        }
        for action in Action::ALL {
            let live = action.area() == Area::Overview || expanded;
            if live && self.also(action).as_ref() == Some(&key) {
                return Some(Hit::Action(action));
            }
        }
        if !key.modified() && !key.shift {
            let mut chars = key.key.chars();
            if let (Some(digit @ '1'..='9'), None) = (chars.next(), chars.next()) {
                return Some(Hit::Answer(digit as usize - '1' as usize));
            }
        }
        None
    }

    /// The action the merge dialog takes `keystroke` for.
    pub fn merge_dialog_hit(&self, keystroke: &Keystroke) -> Option<Action> {
        let key = Key::from_keystroke(keystroke);
        Action::ALL
            .into_iter()
            .find(|&action| action.area() == Area::MergeDialog && *self.key(action) == key)
    }

    /// The keys the status bar reminds of in the overview.
    pub fn overview_hints(&self) -> String {
        format!(
            "{}/{} select · {} expand · {} terminal · {}/{} or right-click card · 1–9 answer",
            self.label(Action::Next),
            self.label(Action::Previous),
            self.label(Action::Expand),
            self.label(Action::Open),
            self.label(Action::CollapseCard),
            self.label(Action::ExpandCard),
        )
    }

    /// The keys the status bar reminds of while a row is expanded.
    pub fn expanded_hints(&self) -> String {
        let copy = if self.mac { "⌘C" } else { "Ctrl+Shift+C" };
        format!(
            "{}/{} collapse · {}/{} scroll · drag, {copy} copy · {}/{} next session",
            self.label(Action::Expand),
            self.label(Action::Collapse),
            self.label(Action::ScrollUp),
            self.label(Action::ScrollDown),
            self.label(Action::Next),
            self.label(Action::Previous),
        )
    }

    /// The keys the status bar reminds of in the focus view.
    pub fn focus_hints(&self) -> String {
        let (modifier, copy) = if self.mac {
            ("⌘", "⌘C/⌘V")
        } else {
            ("Ctrl", "Ctrl+Shift+C/V")
        };
        format!(
            "{} overview · {} side panel · {copy} copy/paste · {modifier}-click a link",
            self.label(Action::Leave),
            self.label(Action::FocusSide),
        )
    }

    /// The focus view's action for `keystroke`.
    pub fn focus_hit(&self, keystroke: &Keystroke) -> Option<Action> {
        let key = Key::from_keystroke(keystroke);
        [Action::Leave, Action::FocusSide]
            .into_iter()
            .find(|&action| *self.key(action) == key)
    }
}

/// Whether `keystroke` is only a modifier going down.
pub fn is_modifier(keystroke: &Keystroke) -> bool {
    Key::from_keystroke(keystroke).is_modifier()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn press(text: &str) -> Keystroke {
        Keystroke::parse(text).unwrap()
    }

    fn key(text: &str) -> Key {
        Key::parse(text).unwrap()
    }

    #[test]
    fn the_defaults_are_the_keys_the_window_always_had() {
        let keymap = Keymap::defaults(true);
        let hit = |text: &str, expanded| keymap.overview_hit(&press(text), expanded);
        assert_eq!(hit("j", false), Some(Hit::Action(Action::Next)));
        assert_eq!(hit("down", false), Some(Hit::Action(Action::Next)));
        assert_eq!(hit("space", false), Some(Hit::Action(Action::Expand)));
        assert_eq!(hit("enter", false), Some(Hit::Action(Action::Open)));
        assert_eq!(hit("3", false), Some(Hit::Answer(2)));
        // Home and End select rows until a row is expanded, then jump in it.
        assert_eq!(hit("home", false), Some(Hit::Action(Action::First)));
        assert_eq!(hit("home", true), Some(Hit::Action(Action::Newest)));
        assert_eq!(hit("end", false), Some(Hit::Action(Action::Last)));
        assert_eq!(hit("end", true), Some(Hit::Action(Action::Oldest)));
        assert_eq!(hit("u", false), None);
        assert_eq!(hit("u", true), Some(Hit::Action(Action::ScrollUp)));
        assert_eq!(hit("pagedown", true), Some(Hit::Action(Action::ScrollDown)));
        assert_eq!(hit("escape", false), None);
        assert_eq!(hit("escape", true), Some(Hit::Action(Action::Collapse)));
        assert_eq!(hit("shift-g", false), Some(Hit::Action(Action::Last)));
        assert_eq!(keymap.focus_hit(&press("cmd-escape")), Some(Action::Leave));
        assert_eq!(keymap.focus_hit(&press("cmd-\\")), Some(Action::FocusSide));
        let linux = Keymap::defaults(false);
        assert_eq!(
            linux.focus_hit(&press("ctrl-shift-escape")),
            Some(Action::Leave)
        );
        assert_eq!(
            linux.focus_hit(&press("ctrl-shift-|")),
            Some(Action::FocusSide)
        );
        assert_eq!(linux.focus_hit(&press("escape")), None);
    }

    #[test]
    fn the_merge_dialog_asks_the_supervisor_with_the_dashboards_key() {
        assert_eq!(
            ASK_SUPERVISOR,
            corgi_core::merge::ASK_SUPERVISOR_KEY.to_string()
        );
        let mut keymap = Keymap::defaults(true);
        assert_eq!(
            keymap.merge_dialog_hit(&press("s")),
            Some(Action::AskSupervisor)
        );
        // The overview's s starts a supervisor; the dialog's is its own.
        assert_eq!(
            keymap.overview_hit(&press("s"), true),
            Some(Hit::Action(Action::Supervisor))
        );
        assert_eq!(keymap.merge_dialog_hit(&press("j")), None);
        assert_eq!(
            keymap.check(Action::AskSupervisor, &key("enter")),
            Err(Refusal::Fixed("The default button / close"))
        );
        assert!(keymap.assign(Action::AskSupervisor, key("h"), false));
        assert_eq!(keymap.label(Action::AskSupervisor), "h");
        assert_eq!(
            keymap.merge_dialog_hit(&press("h")),
            Some(Action::AskSupervisor)
        );
        assert_eq!(keymap.merge_dialog_hit(&press("s")), None);
    }

    #[test]
    fn every_default_is_free_of_clashes() {
        for mac in [true, false] {
            let keymap = Keymap::defaults(mac);
            for action in Action::ALL {
                assert_eq!(
                    keymap.check(action, keymap.key(action)),
                    Ok(()),
                    "{action:?}"
                );
            }
        }
    }

    #[test]
    fn a_rebound_key_moves_its_action_and_what_shows_it() {
        let mut keymap = Keymap::defaults(true);
        assert!(keymap.assign(Action::Prompt, key("i"), false));
        assert_eq!(keymap.label(Action::Prompt), "i");
        assert_eq!(
            keymap.overview_hit(&press("i"), false),
            Some(Hit::Action(Action::Prompt))
        );
        assert_eq!(keymap.overview_hit(&press("p"), false), None);
        assert!(!keymap.is_default(Action::Prompt));
        keymap.reset(Action::Prompt);
        assert!(keymap.is_default(Action::Prompt));
    }

    #[test]
    fn a_taken_key_swaps_only_when_confirmed() {
        let mut keymap = Keymap::defaults(true);
        assert_eq!(
            keymap.check(Action::Prompt, &key("n")),
            Err(Refusal::Clash(Action::NewAgent))
        );
        assert!(!keymap.assign(Action::Prompt, key("n"), false));
        assert_eq!(keymap.label(Action::Prompt), "p");
        assert!(keymap.assign(Action::Prompt, key("n"), true));
        assert_eq!(keymap.label(Action::Prompt), "n");
        assert_eq!(keymap.label(Action::NewAgent), "p");
        // The overview's keys and an expanded session's are one space.
        assert_eq!(
            keymap.check(Action::Merge, &key("u")),
            Err(Refusal::Clash(Action::ScrollUp))
        );
        // The focus view's are their own.
        assert_eq!(keymap.check(Action::Leave, &key("cmd-u")), Ok(()));
    }

    #[test]
    fn fixed_keys_and_plain_focus_keys_are_refused() {
        let keymap = Keymap::defaults(true);
        assert_eq!(
            keymap.check(Action::Merge, &key("4")),
            Err(Refusal::Fixed("Answer question 1–9"))
        );
        assert_eq!(
            keymap.check(Action::Merge, &key("cmd-w")),
            Err(Refusal::Fixed("Close window"))
        );
        assert_eq!(
            keymap.check(Action::Leave, &key("cmd-c")),
            Err(Refusal::Fixed("Copy / paste"))
        );
        assert_eq!(
            keymap.check(Action::Leave, &key("q")),
            Err(Refusal::NeedsChord)
        );
        assert_eq!(
            keymap.check(Action::Leave, &key("ctrl-shift-escape")),
            Err(Refusal::NeedsChord)
        );
    }

    #[test]
    fn a_key_given_to_an_overview_action_stops_being_a_second_key() {
        let mut keymap = Keymap::defaults(true);
        assert_eq!(keymap.also(Action::Next), Some(key("down")));
        assert!(keymap.assign(Action::Merge, key("down"), false));
        assert_eq!(keymap.also(Action::Next), None);
        assert_eq!(
            keymap.overview_hit(&press("down"), false),
            Some(Hit::Action(Action::Merge))
        );
    }

    #[test]
    fn labels_read_as_the_toolbar_writes_keys() {
        assert_eq!(key("shift-g").label(true), "G");
        assert_eq!(key("cmd-escape").label(true), "⌘Esc");
        assert_eq!(key("ctrl-shift-escape").label(false), "Ctrl+Shift+Esc");
        assert_eq!(key("cmd-\\").label(true), "⌘\\");
        assert_eq!(key("enter").label(true), "↵");
        assert_eq!(key("space").label(true), "Space");
        assert_eq!(key("pageup").label(true), "PgUp");
        assert_eq!(key("cmd-shift-k").label(true), "⇧⌘K");
        assert_eq!(key("f5").label(true), "F5");
        assert_eq!(key("enter").label(false), ENTER);
        assert_eq!(key("escape").label(false), ESCAPE);
    }

    #[test]
    fn the_status_bar_says_the_keys_as_they_are() {
        let mut keymap = Keymap::defaults(true);
        assert_eq!(
            keymap.overview_hints(),
            "j/k select · Space expand · ↵ terminal · ←/→ or right-click card · 1–9 answer"
        );
        assert_eq!(
            keymap.expanded_hints(),
            "Space/Esc collapse · u/d scroll · drag, ⌘C copy · j/k next session"
        );
        assert_eq!(
            keymap.focus_hints(),
            "⌘Esc overview · ⌘\\ side panel · ⌘C/⌘V copy/paste · ⌘-click a link"
        );
        keymap.assign(Action::Next, key("n"), true);
        keymap.assign(Action::Leave, key("cmd-l"), false);
        assert!(keymap.overview_hints().starts_with("n/k select"));
        assert!(keymap.focus_hints().starts_with("⌘L overview"));
    }

    /// README's line on the desktop window names the default keys.
    #[test]
    fn the_readme_names_the_default_keys() {
        let readme = include_str!("../../../README.md");
        let keymap = Keymap::defaults(true);
        let k = |action| keymap.label(action);
        let line = format!(
            "`{}`/`{}` select, `{}` expands the selected row, `{}` opens its terminal, `{}` the side panel, `{}` prompt, `{}` new agent, `{}` supervisor, `{}` merge, `{}` tag, `{}` close, `1`–`9` answer a question",
            k(Action::Next),
            k(Action::Previous),
            k(Action::Expand),
            k(Action::Open),
            k(Action::SidePanel),
            k(Action::Prompt),
            k(Action::NewAgent),
            k(Action::Supervisor),
            k(Action::Merge),
            k(Action::Tag),
            k(Action::Close),
        );
        assert!(readme.contains(&line), "README should say: {line}");
        let session = format!(
            "`{}` again or `{}` collapses it, `{}`/`{}` take the expansion to the next row, and `{}`/`{}`, `PgUp`/`PgDn` and `{}`/`{}` scroll it",
            k(Action::Expand),
            k(Action::Collapse),
            k(Action::Next),
            k(Action::Previous),
            k(Action::ScrollUp),
            k(Action::ScrollDown),
            k(Action::Newest),
            k(Action::Oldest),
        );
        assert!(readme.contains(&session), "README should say: {session}");
    }

    #[test]
    fn a_shifted_symbol_is_the_symbol_it_types() {
        let mut keystroke = press("shift-/");
        keystroke.key_char = Some("?".into());
        assert_eq!(Key::from_keystroke(&keystroke), key("?"));
        assert_eq!(Key::from_keystroke(&press("shift-\\")), key("|"));
    }

    #[test]
    fn the_file_keeps_the_changes_and_an_old_or_broken_one_gives_the_defaults() {
        let dir = std::env::temp_dir().join(format!("corgi-keymap-{}", std::process::id()));
        let path = dir.join("hotkeys.json");
        let mut keymap = Keymap::defaults(true);
        keymap.assign(Action::Prompt, key("i"), false);
        keymap.assign(Action::Leave, key("cmd-l"), false);
        keymap.save(Some(&path));
        let text = fs::read_to_string(&path).unwrap();
        assert!(text.contains("\"prompt\": \"i\""), "{text}");
        assert!(text.contains("\"leave\": \"cmd-l\""), "{text}");
        assert!(!text.contains("merge"), "only the changes: {text}");
        assert_eq!(Keymap::load(Some(&path), true), keymap);
        // A swap is kept: each action has the other's default.
        let mut swapped = Keymap::defaults(true);
        swapped.assign(Action::Prompt, key("n"), true);
        swapped.save(Some(&path));
        let loaded = Keymap::load(Some(&path), true);
        assert_eq!(loaded.label(Action::Prompt), "n");
        assert_eq!(loaded.label(Action::NewAgent), "p");

        fs::write(&path, r#"{"version": 0, "keys": {"prompt": "i"}}"#).unwrap();
        assert_eq!(Keymap::load(Some(&path), true), Keymap::defaults(true));
        fs::write(&path, "not json").unwrap();
        assert_eq!(Keymap::load(Some(&path), true), Keymap::defaults(true));
        // A taken or refused key, or an action no longer known, is skipped.
        fs::write(
            &path,
            r#"{"version": 1, "keys": {"prompt": "n", "leave": "q", "gone": "z", "merge": "z"}}"#,
        )
        .unwrap();
        let loaded = Keymap::load(Some(&path), true);
        assert_eq!(loaded.label(Action::Prompt), "p");
        assert_eq!(loaded.label(Action::Leave), "⌘Esc");
        assert_eq!(loaded.label(Action::Merge), "z");
        assert_eq!(
            Keymap::load(Some(&dir.join("none.json")), true),
            Keymap::defaults(true)
        );
        let _ = fs::remove_dir_all(&dir);
    }
}
