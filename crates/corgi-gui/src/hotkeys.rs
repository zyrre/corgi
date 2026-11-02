//! MOCKUP: the hotkeys window, in the looks `CORGI_GUI_HOTKEYS_STYLE`
//! picks (`a`, the default: a dialog over the window; `b`: a window of its
//! own; `c`: a denser dialog with a filter). The bindings are made up and
//! kept for this run alone: nothing here rebinds a key the window acts on.
//! `CORGI_GUI_HOTKEYS_DEMO=waiting|clash` opens with a row mid-rebind.

use gpui::{
    AnyElement, App, AppContext, Bounds, ClickEvent, Context, CursorStyle, Entity, FocusHandle,
    Font, FontWeight, InteractiveElement, IntoElement, KeyDownEvent, Keystroke, MouseButton,
    ParentElement, Pixels, Render, ScrollHandle, SharedString, StatefulInteractiveElement, Styled,
    TitlebarOptions, Window, WindowBounds, WindowOptions, div, point, prelude::FluentBuilder, px,
    size,
};

use crate::{theme::Colors, window::CorgiWindow};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Style {
    /// A dialog over the main window, as the answer box is.
    Dialog,
    /// A small settings window of its own.
    Window,
    /// A denser dialog with a filter field.
    Dense,
}

impl Style {
    pub fn from_env() -> Self {
        match std::env::var("CORGI_GUI_HOTKEYS_STYLE").as_deref() {
            Ok("b") => Style::Window,
            Ok("c") => Style::Dense,
            _ => Style::Dialog,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Area {
    Overview,
    Transcript,
    Focus,
}

impl Area {
    const ALL: [Area; 3] = [Area::Overview, Area::Transcript, Area::Focus];

    fn title(self) -> &'static str {
        match self {
            Area::Overview => "Overview",
            Area::Transcript => "Expanded transcript",
            Area::Focus => "Focus view",
        }
    }

    fn note(self) -> &'static str {
        match self {
            Area::Overview => "While no dialog is open.",
            Area::Transcript => "While a row is expanded; the overview's keys work too.",
            Area::Focus => "Every other key goes to the agent's terminal.",
        }
    }

    /// The keys of the overview and of an expanded transcript are live at
    /// once, so they must differ; the focus view's are its own.
    fn shares_keys_with(self, other: Area) -> bool {
        (self == Area::Focus) == (other == Area::Focus)
    }
}

pub struct Action {
    pub area: Area,
    pub name: &'static str,
    pub default: &'static str,
    /// A second key that does the same and cannot be changed (the arrows).
    pub also: Option<&'static str>,
    /// Drawn but not rebindable: the digits.
    pub fixed: bool,
}

const fn action(area: Area, name: &'static str, default: &'static str) -> Action {
    Action {
        area,
        name,
        default,
        also: None,
        fixed: false,
    }
}

const fn also(mut action: Action, key: &'static str) -> Action {
    action.also = Some(key);
    action
}

const fn fixed(mut action: Action) -> Action {
    action.fixed = true;
    action
}

#[cfg(target_os = "macos")]
const LEAVE: &str = "⌘Esc";
#[cfg(not(target_os = "macos"))]
const LEAVE: &str = "Ctrl+Shift+Esc";

pub const ACTIONS: &[Action] = &[
    also(action(Area::Overview, "Select next row", "j"), "↓"),
    also(action(Area::Overview, "Select previous row", "k"), "↑"),
    action(Area::Overview, "Select first row", "g"),
    action(Area::Overview, "Open terminal (focus view)", "↵"),
    action(Area::Overview, "Expand / collapse session", "Space"),
    action(Area::Overview, "Collapse project card", "←"),
    action(Area::Overview, "Expand project card", "→"),
    action(Area::Overview, "Show / hide side panel", "\\"),
    action(Area::Overview, "New agent", "n"),
    action(Area::Overview, "Prompt agent", "p"),
    action(Area::Overview, "Start supervisor", "s"),
    action(Area::Overview, "Merge", "m"),
    action(Area::Overview, "Tag / untag ready to merge", "t"),
    action(Area::Overview, "Close agent", "x"),
    action(Area::Overview, "Refresh", "r"),
    action(Area::Overview, "Answer first question", "a"),
    fixed(action(Area::Overview, "Answer question 1–9", "1–9")),
    action(Area::Transcript, "Collapse session", "Esc"),
    action(Area::Transcript, "Scroll half a page up", "u"),
    action(Area::Transcript, "Scroll half a page down", "d"),
    action(Area::Transcript, "Scroll a page up", "PgUp"),
    action(Area::Transcript, "Scroll a page down", "PgDn"),
    action(Area::Transcript, "Top of transcript", "Home"),
    action(Area::Transcript, "End of transcript", "End"),
    action(Area::Focus, "Back to the overview", LEAVE),
];

/// A key pressed for a row that another action of the same keys has.
pub struct Clash {
    pub row: usize,
    pub key: String,
    pub other: usize,
}

pub struct Hotkeys {
    pub style: Style,
    pub keys: Vec<String>,
    /// The row waiting for its new key.
    pub waiting: Option<usize>,
    pub clash: Option<Clash>,
    pub filter: String,
    /// Look B's sidebar: the area its list shows.
    pub area: Area,
    pub scroll: ScrollHandle,
    pub focus: FocusHandle,
}

/// What a key did in the hotkeys window.
pub enum Outcome {
    Handled,
    Close,
    Ignored,
}

fn index(name: &str) -> usize {
    ACTIONS
        .iter()
        .position(|action| action.name == name)
        .unwrap_or(0)
}

impl Hotkeys {
    pub fn new(style: Style, cx: &mut Context<Self>) -> Self {
        let mut keys: Vec<String> = ACTIONS.iter().map(|a| a.default.to_string()).collect();
        // Made up: two keys the user changed earlier, so Reset shows.
        keys[index("Prompt agent")] = "i".into();
        keys[index("Tag / untag ready to merge")] = "T".into();
        let mut hotkeys = Hotkeys {
            style,
            keys,
            waiting: None,
            clash: None,
            filter: String::new(),
            area: Area::Overview,
            scroll: ScrollHandle::new(),
            focus: cx.focus_handle(),
        };
        match std::env::var("CORGI_GUI_HOTKEYS_DEMO").as_deref() {
            Ok("waiting") => hotkeys.waiting = Some(index("Merge")),
            Ok("clash") => {
                hotkeys.clash = Some(Clash {
                    row: index("Prompt agent"),
                    key: "n".into(),
                    other: index("New agent"),
                })
            }
            _ => {}
        }
        if let Ok(filter) = std::env::var("CORGI_GUI_HOTKEYS_FILTER") {
            hotkeys.filter = filter;
        }
        hotkeys
    }

    fn changed(&self, row: usize) -> bool {
        self.keys[row] != ACTIONS[row].default
    }

    fn any_changed(&self) -> bool {
        (0..ACTIONS.len()).any(|row| self.changed(row))
    }

    pub fn start(&mut self, row: usize) {
        self.clash = None;
        self.waiting = Some(row);
    }

    pub fn reset(&mut self, row: usize) {
        self.keys[row] = ACTIONS[row].default.to_string();
    }

    pub fn reset_all(&mut self) {
        for row in 0..ACTIONS.len() {
            self.reset(row);
        }
        self.waiting = None;
        self.clash = None;
    }

    /// Confirms a clash: the row takes the key, the other action the row's
    /// old key.
    pub fn swap(&mut self) {
        if let Some(clash) = self.clash.take() {
            let old = std::mem::replace(&mut self.keys[clash.row], clash.key);
            self.keys[clash.other] = old;
        }
    }

    pub fn cancel(&mut self) {
        self.waiting = None;
        self.clash = None;
    }

    pub fn key(&mut self, event: &KeyDownEvent) -> Outcome {
        let keystroke = &event.keystroke;
        if keystroke.key.starts_with("shift")
            || keystroke.key.starts_with("control")
            || keystroke.key.starts_with("alt")
            || keystroke.key.starts_with("platform")
        {
            return Outcome::Ignored;
        }
        if self.clash.is_some() {
            match keystroke.key.as_str() {
                "enter" => self.swap(),
                "escape" => self.cancel(),
                _ => {}
            }
            return Outcome::Handled;
        }
        if let Some(row) = self.waiting.take() {
            if keystroke.key == "escape" && !keystroke.modifiers.modified() {
                return Outcome::Handled;
            }
            let label = key_label(keystroke);
            let area = ACTIONS[row].area;
            let other = (0..ACTIONS.len()).find(|&other| {
                other != row
                    && ACTIONS[other].area.shares_keys_with(area)
                    && (self.keys[other] == label || ACTIONS[other].also == Some(label.as_str()))
            });
            match other {
                Some(other) => {
                    self.clash = Some(Clash {
                        row,
                        key: label,
                        other,
                    })
                }
                None => self.keys[row] = label,
            }
            return Outcome::Handled;
        }
        match keystroke.key.as_str() {
            "escape" if !self.filter.is_empty() => self.filter.clear(),
            "escape" => return Outcome::Close,
            "backspace" if self.style == Style::Dense => {
                self.filter.pop();
            }
            _ if self.style == Style::Dense && !keystroke.modifiers.modified() => {
                match keystroke.key_char.as_deref() {
                    Some(text) if text.chars().all(|c| !c.is_control()) => {
                        self.filter.push_str(text)
                    }
                    _ => return Outcome::Ignored,
                }
            }
            _ => return Outcome::Ignored,
        }
        Outcome::Handled
    }

    /// The rows the filter leaves, in the list's order.
    fn shown(&self, area: Area) -> Vec<usize> {
        let filter = self.filter.to_lowercase();
        (0..ACTIONS.len())
            .filter(|&row| ACTIONS[row].area == area)
            .filter(|&row| {
                filter.is_empty()
                    || ACTIONS[row].name.to_lowercase().contains(&filter)
                    || self.keys[row].to_lowercase() == filter
            })
            .collect()
    }
}

/// How a key reads on a chip: the symbols the toolbar uses.
pub fn key_label(keystroke: &Keystroke) -> String {
    let m = keystroke.modifiers;
    let mut label = String::new();
    let mac = cfg!(target_os = "macos");
    if m.control {
        label.push_str(if mac { "⌃" } else { "Ctrl+" });
    }
    if m.alt {
        label.push_str(if mac { "⌥" } else { "Alt+" });
    }
    if m.platform {
        label.push_str(if mac { "⌘" } else { "Super+" });
    }
    let named = match keystroke.key.as_str() {
        "enter" => Some("↵"),
        "space" => Some("Space"),
        "escape" => Some("Esc"),
        "up" => Some("↑"),
        "down" => Some("↓"),
        "left" => Some("←"),
        "right" => Some("→"),
        "pageup" => Some("PgUp"),
        "pagedown" => Some("PgDn"),
        "home" => Some("Home"),
        "end" => Some("End"),
        "tab" => Some("Tab"),
        "backspace" => Some("⌫"),
        _ => None,
    };
    match named {
        Some(name) => {
            if m.shift {
                label.push_str(if mac { "⇧" } else { "Shift+" });
            }
            label.push_str(name);
        }
        // A shifted letter reads as the capital it types.
        None => match keystroke.key_char.as_deref() {
            Some(text) if m.shift && !m.control && !m.alt && !m.platform => label.push_str(text),
            _ => {
                if m.shift {
                    label.push_str(if mac { "⇧" } else { "Shift+" });
                }
                label.push_str(&keystroke.key);
            }
        },
    }
    label
}

/// What the looks draw with.
#[derive(Clone)]
pub struct Paint {
    pub colors: Colors,
    pub font: Font,
    pub ui_font: SharedString,
    pub scale: f32,
}

impl Paint {
    fn scaled(&self, size: f32) -> Pixels {
        px(size * self.scale)
    }
}

/// A key on a chip: click it to give the action a new key.
fn chip(
    hotkeys: &Entity<Hotkeys>,
    state: &Hotkeys,
    paint: &Paint,
    row: usize,
    dense: bool,
) -> AnyElement {
    let colors = &paint.colors;
    let roles = &colors.roles;
    let action = &ACTIONS[row];
    let waiting = state.waiting == Some(row);
    let clash = state.clash.as_ref().filter(|clash| clash.row == row);
    let (text, border, color) = if waiting {
        ("Press a key…".to_string(), roles.accent, roles.accent)
    } else if let Some(clash) = clash {
        (clash.key.clone(), roles.warning, roles.warning)
    } else {
        (
            state.keys[row].clone(),
            if action.fixed {
                roles.keycap
            } else {
                roles.rule
            },
            if action.fixed {
                colors.secondary(roles.popup_fill)
            } else {
                roles.accent
            },
        )
    };
    let entity = hotkeys.clone();
    div()
        .id(SharedString::from(format!("hotkey-chip-{row}")))
        .flex_none()
        .min_w(paint.scaled(if dense { 52.0 } else { 64.0 }))
        .px_2()
        .when(!dense, |chip| chip.py(px(1.0)))
        .rounded_sm()
        .border_1()
        .border_color(border)
        .bg(roles.keycap)
        .text_color(color)
        .font(paint.font.clone())
        .font_weight(FontWeight::BOLD)
        .flex()
        .justify_center()
        .child(text)
        .when(!action.fixed, |chip| {
            chip.cursor(CursorStyle::PointingHand)
                .hover(|style| style.border_color(roles.accent))
                .on_click(move |_: &ClickEvent, _, cx: &mut App| {
                    entity.update(cx, |hotkeys, cx| {
                        hotkeys.start(row);
                        cx.notify();
                    });
                })
        })
        .into_any_element()
}

/// A small text button in the dialog's style.
fn text_button(
    id: SharedString,
    label: &str,
    color: gpui::Hsla,
    paint: &Paint,
    on_click: impl Fn(&mut App) + 'static,
) -> AnyElement {
    let roles = &paint.colors.roles;
    div()
        .id(id)
        .flex_none()
        .px_1()
        .rounded_sm()
        .text_color(color)
        .cursor(CursorStyle::PointingHand)
        .hover(|style| style.bg(roles.keycap))
        .child(label.to_string())
        .on_click(move |_: &ClickEvent, _, cx: &mut App| on_click(cx))
        .into_any_element()
}

fn update<F: Fn(&mut Hotkeys) + 'static>(
    hotkeys: &Entity<Hotkeys>,
    f: F,
) -> impl Fn(&mut App) + 'static + use<F> {
    let entity = hotkeys.clone();
    move |cx: &mut App| {
        entity.update(cx, |hotkeys, cx| {
            f(hotkeys);
            cx.notify();
        })
    }
}

/// The line under a row that waits or clashes.
fn row_note(
    state: &Hotkeys,
    hotkeys: &Entity<Hotkeys>,
    paint: &Paint,
    row: usize,
) -> Option<AnyElement> {
    let roles = &paint.colors.roles;
    let muted = paint.colors.secondary(roles.popup_fill);
    if state.waiting == Some(row) {
        let note = if ACTIONS[row].area == Area::Focus {
            "Press the new chord: it needs ⌘, as plain keys go to the terminal. Esc cancels."
        } else {
            "Press the new key, with or without a modifier. Esc cancels."
        };
        return Some(
            div()
                .font_family(paint.ui_font.clone())
                .text_color(muted)
                .child(note)
                .into_any_element(),
        );
    }
    let clash = state.clash.as_ref().filter(|clash| clash.row == row)?;
    let other = &ACTIONS[clash.other];
    let old = &state.keys[row];
    Some(
        div()
            .flex()
            .flex_row()
            .flex_wrap()
            .items_center()
            .gap_x_2()
            .text_color(roles.warning)
            .child(div().font_family(paint.ui_font.clone()).child(format!(
                "⚠ {} is {}'s key{}.",
                clash.key,
                other.name,
                if other.area == ACTIONS[row].area {
                    String::new()
                } else {
                    format!(" ({})", other.area.title().to_lowercase())
                }
            )))
            .child(text_button(
                "hotkey-swap".into(),
                &format!("↵ Swap: {} gets {old}", other.name),
                roles.accent,
                paint,
                update(hotkeys, Hotkeys::swap),
            ))
            .child(text_button(
                "hotkey-cancel".into(),
                &format!("Esc Keep {old}"),
                muted,
                paint,
                update(hotkeys, Hotkeys::cancel),
            ))
            .into_any_element(),
    )
}

/// One action: its name, its key's chip, the key that also works, and
/// Reset when it is not the default.
fn row(
    hotkeys: &Entity<Hotkeys>,
    state: &Hotkeys,
    paint: &Paint,
    row: usize,
    dense: bool,
) -> AnyElement {
    let colors = &paint.colors;
    let roles = &colors.roles;
    let action = &ACTIONS[row];
    let muted = colors.secondary(roles.popup_fill);
    let busy = state.waiting == Some(row) || state.clash.as_ref().is_some_and(|c| c.row == row);
    let reset = (state.changed(row) && !busy).then(|| {
        text_button(
            SharedString::from(format!("hotkey-reset-{row}")),
            &format!("Reset to {}", action.default),
            roles.accent,
            paint,
            update(hotkeys, move |hotkeys| hotkeys.reset(row)),
        )
    });
    let side = div()
        .flex()
        .flex_row()
        .items_center()
        .gap_2()
        .flex_none()
        .w(paint.scaled(if dense { 120.0 } else { 130.0 }))
        .when_some(action.also, |side, also| {
            side.child(div().text_color(muted).child(format!("also {also}")))
        })
        .when(action.fixed, |side| {
            side.child(div().text_color(muted).child("fixed"))
        })
        .children(reset);
    div()
        .flex()
        .flex_col()
        .px_2()
        .when(dense, |row| row.py(px(1.0)))
        .when(!dense, |row| row.py_1())
        .rounded_sm()
        .when(busy, |row| row.bg(roles.focus_bar))
        .child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap_3()
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .font_family(paint.ui_font.clone())
                        .text_color(paint.colors.foreground)
                        .when(state.changed(row), |name| {
                            name.font_weight(FontWeight::BOLD)
                        })
                        .child(action.name),
                )
                .child(chip(hotkeys, state, paint, row, dense))
                .child(side),
        )
        .children(row_note(state, hotkeys, paint, row))
        .into_any_element()
}

fn area_heading(paint: &Paint, area: Area, first: bool) -> AnyElement {
    let roles = &paint.colors.roles;
    div()
        .flex()
        .flex_row()
        .flex_wrap()
        .items_baseline()
        .gap_x_3()
        .px_2()
        .when(!first, |heading| heading.mt_3())
        .pb(px(2.0))
        .border_b_1()
        .border_color(roles.rule)
        .child(
            div()
                .text_color(roles.accent)
                .font_weight(FontWeight::BOLD)
                .child(area.title().to_uppercase()),
        )
        .child(
            div()
                .text_color(paint.colors.secondary(roles.popup_fill))
                .font_family(paint.ui_font.clone())
                .text_size(paint.scaled(12.0))
                .child(area.note()),
        )
        .into_any_element()
}

/// Reset all, and the keys the window takes.
fn footer(hotkeys: &Entity<Hotkeys>, state: &Hotkeys, paint: &Paint, close: &str) -> AnyElement {
    let roles = &paint.colors.roles;
    let muted = paint.colors.secondary(roles.popup_fill);
    let any = state.any_changed();
    div()
        .flex()
        .flex_row()
        .flex_wrap()
        .items_center()
        .gap_3()
        .pt_2()
        .border_t_1()
        .border_color(roles.rule)
        .child(if any {
            text_button(
                "hotkeys-reset-all".into(),
                "⟲ Reset all",
                roles.accent,
                paint,
                update(hotkeys, Hotkeys::reset_all),
            )
        } else {
            div()
                .px_1()
                .text_color(muted)
                .child("⟲ Reset all")
                .into_any_element()
        })
        .child(
            div()
                .flex_1()
                .min_w_0()
                .text_color(muted)
                .font_family(paint.ui_font.clone())
                .child("Changes apply at once and are kept."),
        )
        .child(
            div()
                .flex()
                .flex_row()
                .gap_1()
                .child(
                    div()
                        .px_1()
                        .rounded_sm()
                        .bg(roles.keycap)
                        .text_color(roles.accent)
                        .font_weight(FontWeight::BOLD)
                        .child("Esc"),
                )
                .child(div().text_color(muted).child(close.to_string())),
        )
        .into_any_element()
}

fn title(paint: &Paint, text: &str) -> AnyElement {
    let roles = &paint.colors.roles;
    div()
        .flex()
        .flex_row()
        .items_baseline()
        .gap_2()
        .child(
            div()
                .flex()
                .flex_row()
                .gap_2()
                .text_color(roles.accent)
                .font_weight(FontWeight::BOLD)
                .child("◆")
                .child(text.to_string()),
        )
        .child(
            div()
                .text_color(paint.colors.secondary(roles.popup_fill))
                .font_family(paint.ui_font.clone())
                .child("Click a key to change it."),
        )
        .into_any_element()
}

/// Looks A and C: the dialog's body, `room` pixels tall at most.
pub fn dialog(
    hotkeys: &Entity<Hotkeys>,
    paint: &Paint,
    room: f32,
    wide: bool,
    cx: &App,
) -> AnyElement {
    let state = hotkeys.read(cx);
    let dense = state.style == Style::Dense;
    let roles = &paint.colors.roles;
    // Filtered, the few rows left read best in one column.
    let list = if dense && wide && state.filter.is_empty() {
        // Two columns: the overview's keys, then the transcript's and the
        // focus view's.
        let column = |areas: &[Area]| {
            let mut column = div().flex().flex_col().flex_1().min_w_0();
            let mut first = true;
            for &area in areas {
                let rows = state.shown(area);
                if rows.is_empty() {
                    continue;
                }
                column = column.child(area_heading(paint, area, first));
                first = false;
                for index in rows {
                    column = column.child(row(hotkeys, state, paint, index, true));
                }
            }
            column
        };
        div()
            .flex()
            .flex_row()
            .gap_4()
            .child(column(&[Area::Overview]))
            .child(column(&[Area::Transcript, Area::Focus]))
    } else {
        let mut list = div().flex().flex_col();
        let mut first = true;
        for area in Area::ALL {
            let rows = state.shown(area);
            if rows.is_empty() {
                continue;
            }
            list = list.child(area_heading(paint, area, first));
            first = false;
            for index in rows {
                list = list.child(row(hotkeys, state, paint, index, dense));
            }
        }
        list
    };
    let filter = dense.then(|| {
        let empty = state.filter.is_empty();
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .px_2()
            .py_1()
            .rounded_sm()
            .border_1()
            .border_color(if empty { roles.rule } else { roles.accent })
            .bg(paint.colors.background)
            .child(div().text_color(roles.muted).child("⌕"))
            .child(
                div()
                    .flex_1()
                    .text_color(if empty {
                        paint.colors.secondary(paint.colors.background)
                    } else {
                        paint.colors.foreground
                    })
                    .child(if empty {
                        "Type to filter actions or keys".to_string()
                    } else {
                        format!("{}▏", state.filter)
                    }),
            )
            .when(!empty, |field| {
                field.child(div().text_color(roles.muted).child(format!(
                        "{} of {}",
                        Area::ALL
                            .iter()
                            .map(|&a| state.shown(a).len())
                            .sum::<usize>(),
                        ACTIONS.len()
                    )))
            })
    });
    div()
        .flex()
        .flex_col()
        .gap_2()
        .max_h(px(room))
        .child(title(paint, "Hotkeys"))
        .children(filter)
        .child(
            div()
                .id("hotkeys-list")
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                .track_scroll(&state.scroll)
                .child(list),
        )
        .child(footer(hotkeys, state, paint, "close"))
        .into_any_element()
}

/// Look B: the hotkeys in a window of their own, a sidebar of areas
/// beside the chosen area's keys.
pub struct HotkeysWindow {
    pub hotkeys: Entity<Hotkeys>,
    pub paint: Paint,
}

impl HotkeysWindow {
    pub fn new(
        hotkeys: Entity<Hotkeys>,
        paint: Paint,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        cx.observe(&hotkeys, |_, _, cx| cx.notify()).detach();
        let focus = hotkeys.read(cx).focus.clone();
        window.focus(&focus, cx);
        HotkeysWindow { hotkeys, paint }
    }
}

impl Render for HotkeysWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let paint = &self.paint;
        let colors = &paint.colors;
        let roles = &colors.roles;
        window.set_rem_size(px(16.0 * paint.scale));
        let hotkeys = self.hotkeys.clone();
        let state = hotkeys.read(cx);
        let narrow = f32::from(window.viewport_size().width) < 620.0 * paint.scale;
        let sidebar = div()
            .flex()
            .flex_col()
            .flex_none()
            .gap_1()
            .w(paint.scaled(if narrow { 130.0 } else { 160.0 }))
            .p_2()
            .bg(colors.panel)
            .border_r_1()
            .border_color(roles.rule)
            .children(Area::ALL.iter().map(|&area| {
                let chosen = state.area == area;
                let changed = (0..ACTIONS.len())
                    .filter(|&row| ACTIONS[row].area == area && state.changed(row))
                    .count();
                let count = (0..ACTIONS.len())
                    .filter(|&row| ACTIONS[row].area == area)
                    .count();
                let entity = hotkeys.clone();
                div()
                    .id(SharedString::from(format!("hotkeys-area-{area:?}")))
                    .flex()
                    .flex_col()
                    .px_2()
                    .py_1()
                    .rounded_sm()
                    .cursor(CursorStyle::PointingHand)
                    .when(chosen, |item| item.bg(roles.focus_bar))
                    .when(!chosen, |item| item.hover(|style| style.bg(roles.keycap)))
                    .child(
                        div()
                            .font_family(paint.ui_font.clone())
                            .text_color(if chosen {
                                roles.accent
                            } else {
                                colors.foreground
                            })
                            .font_weight(if chosen {
                                FontWeight::BOLD
                            } else {
                                FontWeight::NORMAL
                            })
                            .child(area.title()),
                    )
                    .child(
                        div()
                            .text_size(paint.scaled(12.0))
                            .text_color(colors.secondary(if chosen {
                                roles.focus_bar
                            } else {
                                colors.panel
                            }))
                            .child(match (count, changed) {
                                (1, 0) => "1 key".to_string(),
                                (count, 0) => format!("{count} keys"),
                                (count, changed) => format!("{count} keys · {changed} changed"),
                            }),
                    )
                    .on_click(move |_: &ClickEvent, _, cx: &mut App| {
                        entity.update(cx, |hotkeys, cx| {
                            hotkeys.area = area;
                            hotkeys.cancel();
                            cx.notify();
                        })
                    })
            }));
        let area = state.area;
        let mut list = div().flex().flex_col();
        for index in state.shown(area) {
            list = list.child(row(&hotkeys, state, paint, index, false));
        }
        let entity = hotkeys.clone();
        div()
            .id("hotkeys-window")
            .key_context("Hotkeys")
            .track_focus(&state.focus)
            .size_full()
            .flex()
            .flex_row()
            .bg(roles.popup_fill)
            .text_color(roles.text)
            .font(paint.font.clone())
            .text_size(paint.scaled(crate::layout::FONT_SIZE))
            .line_height(paint.scaled(crate::layout::FONT_SIZE * 1.45))
            .on_key_down(move |event: &KeyDownEvent, window, cx: &mut App| {
                let outcome = entity.update(cx, |hotkeys, cx| {
                    let outcome = hotkeys.key(event);
                    cx.notify();
                    outcome
                });
                if let Outcome::Close = outcome {
                    window.remove_window();
                }
            })
            .on_mouse_down(MouseButton::Left, {
                let focus = state.focus.clone();
                move |_, window, cx| window.focus(&focus, cx)
            })
            .child(sidebar)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .gap_2()
                    .p_4()
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .child(
                                div()
                                    .text_color(roles.accent)
                                    .font_weight(FontWeight::BOLD)
                                    .child(area.title()),
                            )
                            .child(
                                div()
                                    .font_family(paint.ui_font.clone())
                                    .text_color(colors.secondary(roles.popup_fill))
                                    .child(format!("{} Click a key to change it.", area.note())),
                            ),
                    )
                    .child(
                        div()
                            .id("hotkeys-window-list")
                            .flex_1()
                            .min_h_0()
                            .overflow_y_scroll()
                            .track_scroll(&state.scroll)
                            .child(list),
                    )
                    .child(footer(&hotkeys, state, paint, "close")),
            )
    }
}

impl CorgiWindow {
    fn paint(&self) -> Paint {
        Paint {
            colors: self.colors.clone(),
            font: self.font.clone(),
            ui_font: self.ui_font.clone(),
            scale: self.layout.text_scale(),
        }
    }

    /// The header's Hotkeys button: the dialog, or look B's window (brought
    /// to the front when it is open already).
    pub fn open_hotkeys(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let style = Style::from_env();
        if style != Style::Window {
            let hotkeys = cx.new(|cx| Hotkeys::new(style, cx));
            cx.observe(&hotkeys, |_, _, cx| cx.notify()).detach();
            window.focus(&self.focus, cx);
            self.hotkeys = Some(hotkeys);
            cx.notify();
            return;
        }
        if let Some(open) = self.hotkeys_window
            && open
                .update(cx, |_, window, _| window.activate_window())
                .is_ok()
        {
            return;
        }
        let hotkeys = cx.new(|cx| Hotkeys::new(style, cx));
        let paint = self.paint();
        let background = std::env::var_os("CORGI_GUI_BACKGROUND").is_some();
        // Over the main window's middle, as a settings window opens.
        let main = window.bounds();
        let size = size(px(680.0), px(660.0));
        let origin = point(
            main.origin.x + (main.size.width - size.width) / 2.0,
            main.origin.y + px(60.0),
        );
        let opened = cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds { origin, size })),
                app_id: Some("corgi-gui".into()),
                titlebar: Some(TitlebarOptions {
                    title: Some("Corgi — Hotkeys".into()),
                    ..Default::default()
                }),
                window_min_size: Some(gpui::size(px(560.0), px(400.0))),
                focus: !background,
                ..Default::default()
            },
            |window, cx| cx.new(|cx| HotkeysWindow::new(hotkeys, paint, window, cx)),
        );
        self.hotkeys_window = opened.ok();
    }

    /// A key while the hotkeys dialog is open: all of them are its.
    pub fn hotkeys_key(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) {
        let Some(hotkeys) = self.hotkeys.clone() else {
            return;
        };
        let outcome = hotkeys.update(cx, |hotkeys, cx| {
            let outcome = hotkeys.key(event);
            cx.notify();
            outcome
        });
        if let Outcome::Close = outcome {
            self.hotkeys = None;
        }
        cx.stop_propagation();
        cx.notify();
    }

    /// Looks A and C over the window, as the dialogs are.
    pub fn hotkeys_layer(&self, window: &Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        let hotkeys = self.hotkeys.as_ref()?;
        let paint = self.paint();
        let colors = &self.colors;
        let scale = paint.scale;
        let dense = hotkeys.read(cx).style == Style::Dense;
        let viewport = window.viewport_size();
        // The answer box's measures: 110 above, 24 below, its padding.
        let room = f32::from(viewport.height) - 110.0 - 24.0 - (32.0 * scale + 2.0);
        let width = if dense { 900.0 } else { 580.0 } * scale;
        let wide = f32::from(viewport.width) * 0.92 >= 860.0 * scale;
        Some(
            div()
                .id("hotkeys-layer")
                .absolute()
                .top_0()
                .left_0()
                .size_full()
                .flex()
                .items_start()
                .justify_center()
                .pt(px(110.0))
                .bg(colors.background.opacity(0.55))
                .occlude()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _, cx| {
                        cx.stop_propagation();
                        this.hotkeys = None;
                        cx.notify();
                    }),
                )
                .child(
                    div()
                        .id("hotkeys-dialog")
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .w(px(width))
                        .max_w(gpui::relative(0.92))
                        .flex()
                        .flex_col()
                        .p_4()
                        .rounded_lg()
                        .border_1()
                        .border_color(colors.roles.accent)
                        .bg(colors.roles.popup_fill)
                        .shadow_lg()
                        .child(dialog(hotkeys, &paint, room, wide, cx)),
                )
                .into_any_element(),
        )
    }
}
