//! The hotkeys dialog the header's Hotkeys button opens: every action's key
//! on a chip, grouped by where it works, with a filter field. A click on a
//! chip waits for the new key; a key another action has is swapped only
//! when confirmed, and a fixed one is refused. Esc while waiting cancels,
//! and a second Esc binds Esc itself. Changes apply and are kept at once
//! (`keymap.rs`).

use gpui::{
    AnyElement, App, AppContext, ClickEvent, Context, CursorStyle, Entity, FocusHandle, Focusable,
    FontWeight, Hsla, InteractiveElement, IntoElement, KeyDownEvent, MouseButton, ParentElement,
    ScrollHandle, SharedString, StatefulInteractiveElement, Styled, Subscription, Window, div,
    prelude::FluentBuilder, px, relative,
};

use crate::{
    keymap::{self, Action, Area, Key, Refusal},
    text_input::TextInput,
    window::{CorgiWindow, field_colors},
};

/// The dialog's room: the answer box's measures, 110 above and 24 below.
const TOP: f32 = 110.0;
const BOTTOM: f32 = 24.0;
/// Its width, and the least at which it shows two columns.
const WIDTH: f32 = 900.0;
const TWO_COLUMNS: f32 = 820.0;

pub struct HotkeysDialog {
    pub filter: Entity<TextInput>,
    /// The dialog's own focus while a key is awaited, so the key reaches no
    /// field.
    focus: FocusHandle,
    /// The action whose chip waits for its new key.
    waiting: Option<Action>,
    /// Esc cancelled waiting for this action's key; Esc again binds Esc.
    esc_armed: Option<Action>,
    /// A key an action cannot take as it is, and why.
    refused: Option<(Action, Key, Refusal)>,
    scroll: ScrollHandle,
    _filter_changes: Subscription,
}

/// A line of the list: an action, or a fixed key (`keymap::fixed`).
#[derive(Clone, Copy)]
enum Line {
    Action(Action),
    Fixed(usize),
}

impl CorgiWindow {
    /// The header's button: "Hotkeys", under the A−, A+ and appearance
    /// buttons.
    pub fn hotkeys_button(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = &self.colors;
        div()
            .id("hotkeys-button")
            .flex()
            .justify_center()
            .px_2()
            .rounded_md()
            .border_1()
            .border_color(colors.roles.rule)
            .text_color(colors.roles.text)
            .font_weight(FontWeight::BOLD)
            .cursor(CursorStyle::PointingHand)
            .hover(|style| {
                style
                    .bg(colors.roles.keycap)
                    .border_color(colors.roles.accent)
            })
            .child("Hotkeys")
            .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                this.open_hotkeys(window, cx);
                cx.stop_propagation();
            }))
    }

    pub fn open_hotkeys(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.hotkeys.is_some() {
            return;
        }
        let colors = field_colors(&self.colors);
        let filter = cx.new(|cx| TextInput::new("Type to filter actions or keys", colors, cx));
        let changes = cx.observe(&filter, |_, _, cx| cx.notify());
        window.focus(&filter.read(cx).focus_handle(cx), cx);
        self.hotkeys = Some(HotkeysDialog {
            filter,
            focus: cx.focus_handle(),
            waiting: None,
            esc_armed: None,
            refused: None,
            scroll: ScrollHandle::new(),
            _filter_changes: changes,
        });
        cx.notify();
    }

    fn close_hotkeys(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.hotkeys = None;
        self.focus_dialog_now(window, cx);
        cx.notify();
    }

    /// Where the keyboard goes while the dialog is open.
    pub fn hotkeys_focus(&self, cx: &App) -> Option<FocusHandle> {
        let dialog = self.hotkeys.as_ref()?;
        Some(if dialog.waiting.is_some() || dialog.refused.is_some() {
            dialog.focus.clone()
        } else {
            dialog.filter.read(cx).focus_handle(cx)
        })
    }

    fn refocus_hotkeys(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(target) = self.hotkeys_focus(cx) {
            window.focus(&target, cx);
        }
        cx.notify();
    }

    /// Gives `action` `key`, or says why it cannot have it.
    fn offer_key(&mut self, action: Action, key: Key, cx: &mut Context<Self>) {
        match self.keymap.check(action, &key) {
            Ok(()) => {
                self.keymap.assign(action, key, false);
                self.keymap_changed(cx);
            }
            Err(refusal) => {
                if let Some(dialog) = self.hotkeys.as_mut() {
                    dialog.refused = Some((action, key, refusal));
                }
            }
        }
    }

    fn start_waiting(&mut self, action: Action, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(dialog) = self.hotkeys.as_mut() {
            dialog.waiting = Some(action);
            dialog.esc_armed = None;
            dialog.refused = None;
        }
        self.refocus_hotkeys(window, cx);
    }

    /// Confirms a clash: the action takes the key, the other its old key.
    fn swap(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some((action, key, Refusal::Clash(_))) = self
            .hotkeys
            .as_mut()
            .and_then(|dialog| dialog.refused.take())
        {
            self.keymap.assign(action, key, true);
            self.keymap_changed(cx);
        }
        self.refocus_hotkeys(window, cx);
    }

    fn keep(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(dialog) = self.hotkeys.as_mut() {
            dialog.refused = None;
        }
        self.refocus_hotkeys(window, cx);
    }

    fn reset_key(&mut self, action: Action, cx: &mut Context<Self>) {
        self.keymap.reset(action);
        self.keymap_changed(cx);
        cx.notify();
    }

    fn reset_all_keys(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        for action in Action::ALL {
            self.keymap.reset(action);
        }
        self.keymap_changed(cx);
        if let Some(dialog) = self.hotkeys.as_mut() {
            dialog.waiting = None;
            dialog.esc_armed = None;
            dialog.refused = None;
        }
        self.refocus_hotkeys(window, cx);
    }

    /// A key while the dialog is open. Keys it does not take go on to the
    /// filter field.
    pub fn hotkeys_key(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(dialog) = self.hotkeys.as_mut() else {
            return;
        };
        let keystroke = &event.keystroke;
        if keymap::is_modifier(keystroke) {
            return;
        }
        let plain = !keystroke.modifiers.modified();
        let escape = plain && keystroke.key == "escape";
        if let Some((_, _, refusal)) = &dialog.refused {
            let clash = matches!(refusal, Refusal::Clash(_));
            match keystroke.key.as_str() {
                "enter" if clash => self.swap(window, cx),
                "enter" | "escape" => self.keep(window, cx),
                _ => {}
            }
            cx.stop_propagation();
            return;
        }
        if let Some(action) = dialog.waiting.take() {
            if escape {
                dialog.esc_armed = Some(action);
            } else {
                self.offer_key(action, Key::from_keystroke(keystroke), cx);
            }
            cx.stop_propagation();
            self.refocus_hotkeys(window, cx);
            return;
        }
        if let Some(action) = dialog.esc_armed.take()
            && escape
        {
            self.offer_key(action, Key::from_keystroke(keystroke), cx);
            cx.stop_propagation();
            self.refocus_hotkeys(window, cx);
            return;
        }
        match keystroke.key.as_str() {
            "escape" if plain => {
                let filter = dialog.filter.clone();
                if filter.read(cx).text().is_empty() {
                    self.close_hotkeys(window, cx);
                } else {
                    filter.update(cx, |filter, cx| filter.clear(cx));
                }
            }
            // The filter is one line.
            "enter" => {}
            _ => {
                cx.notify();
                return;
            }
        }
        cx.stop_propagation();
        cx.notify();
    }

    // ---- Drawing ----

    /// The dialog over the window, as the other dialogs are.
    pub fn hotkeys_layer(&self, window: &Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        let dialog = self.hotkeys.as_ref()?;
        let colors = &self.colors;
        let scale = self.layout.text_scale();
        let viewport = window.viewport_size();
        let filter = dialog.filter.read(cx).text().trim().to_lowercase();
        let wide = f32::from(viewport.width) * 0.92 >= TWO_COLUMNS * scale;
        // Filtered, the few lines left read best in one column.
        let columns: Vec<&[Area]> = if wide && filter.is_empty() {
            vec![
                &[Area::Overview],
                &[
                    Area::Transcript,
                    Area::Focus,
                    Area::MergeDialog,
                    Area::Window,
                ],
            ]
        } else {
            vec![&Area::ALL]
        };
        let fixed = keymap::fixed(self.keymap.mac());
        let total = Action::ALL.len() + fixed.len();
        let mut shown = 0;
        let mut list = div().flex().flex_row().gap_4();
        for areas in columns {
            let mut column = div().flex().flex_col().flex_1().min_w_0();
            let mut first = true;
            for &area in areas {
                let lines = self.hotkey_lines(area, &filter, &fixed);
                if lines.is_empty() {
                    continue;
                }
                shown += lines.len();
                column = column.child(self.area_heading(area, first));
                first = false;
                for line in lines {
                    column = column.child(self.hotkey_line(line, &fixed, cx));
                }
            }
            list = list.child(column);
        }
        let field = div()
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .px_2()
            .py_1()
            .rounded_sm()
            .border_1()
            .border_color(if filter.is_empty() {
                colors.roles.rule
            } else {
                colors.roles.accent
            })
            .bg(colors.background)
            .child(div().text_color(colors.roles.muted).child("⌕"))
            .child(div().flex_1().min_w_0().child(dialog.filter.clone()))
            .when(!filter.is_empty(), |field| {
                field.child(
                    div()
                        .flex_none()
                        .text_color(colors.roles.muted)
                        .child(format!("{shown} of {total}")),
                )
            });
        let empty = (shown == 0).then(|| {
            div()
                .px_2()
                .text_color(colors.secondary(colors.roles.popup_fill))
                .child("No action or key matches.")
        });
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
                .pt(px(TOP))
                .bg(colors.background.opacity(0.55))
                .occlude()
                // A click outside the dialog closes it, as Esc does.
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, window, cx| {
                        cx.stop_propagation();
                        this.close_hotkeys(window, cx);
                    }),
                )
                .child(
                    div()
                        .id("hotkeys")
                        .key_context("Hotkeys")
                        .track_focus(&dialog.focus)
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .w(self.scaled(WIDTH))
                        .max_w(relative(0.92))
                        .max_h(px(f32::from(viewport.height) - TOP - BOTTOM))
                        .flex()
                        .flex_col()
                        .gap_2()
                        .p_4()
                        .rounded_lg()
                        .border_1()
                        .border_color(colors.roles.accent)
                        .bg(colors.roles.popup_fill)
                        .shadow_lg()
                        .child(self.hotkeys_title())
                        .child(field)
                        .child(
                            div()
                                .id("hotkeys-list")
                                .flex_1()
                                .min_h_0()
                                .overflow_y_scroll()
                                .track_scroll(&dialog.scroll)
                                .child(list)
                                .children(empty),
                        )
                        .child(self.hotkeys_footer(cx)),
                )
                .into_any_element(),
        )
    }

    /// The lines of `area` the filter leaves: a word of the action's name,
    /// or its key as the chip reads.
    fn hotkey_lines(&self, area: Area, filter: &str, fixed: &[keymap::Fixed]) -> Vec<Line> {
        let matches = |name: &str, label: &str| {
            filter.is_empty()
                || name.to_lowercase().contains(filter)
                || label.to_lowercase() == filter
        };
        let actions = Action::ALL
            .into_iter()
            .filter(|action| action.area() == area)
            .filter(|&action| matches(action.name(), &self.keymap.label(action)))
            .map(Line::Action);
        let fixed = fixed
            .iter()
            .enumerate()
            .filter(|(_, fixed)| fixed.area == area && matches(fixed.name, fixed.label))
            .map(|(index, _)| Line::Fixed(index));
        actions.chain(fixed).collect()
    }

    fn hotkeys_title(&self) -> impl IntoElement {
        let colors = &self.colors;
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
                    .text_color(colors.roles.accent)
                    .font_weight(FontWeight::BOLD)
                    .child("◆")
                    .child("Hotkeys"),
            )
            .child(
                div()
                    .font_family(self.ui_font.clone())
                    .text_color(colors.secondary(colors.roles.popup_fill))
                    .child("Click a key to change it."),
            )
    }

    fn area_heading(&self, area: Area, first: bool) -> impl IntoElement {
        let colors = &self.colors;
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
            .border_color(colors.roles.rule)
            .child(
                div()
                    .text_color(colors.roles.accent)
                    .font_weight(FontWeight::BOLD)
                    .child(area.title().to_uppercase()),
            )
            .child(
                div()
                    .text_color(colors.secondary(colors.roles.popup_fill))
                    .font_family(self.ui_font.clone())
                    .text_size(self.scaled(12.0))
                    .child(area.note()),
            )
    }

    /// One action, or a fixed key: its name, its chip, the key that also
    /// works or `fixed`, and Reset when it is not the default.
    fn hotkey_line(
        &self,
        line: Line,
        fixed: &[keymap::Fixed],
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let colors = &self.colors;
        let muted = colors.secondary(colors.roles.popup_fill);
        let (name, changed, busy, chip, side, note) = match line {
            Line::Fixed(index) => {
                let fixed = &fixed[index];
                (
                    fixed.name,
                    false,
                    false,
                    self.chip(None, fixed.label.to_string(), cx),
                    Some(div().text_color(muted).child("fixed").into_any_element()),
                    None,
                )
            }
            Line::Action(action) => {
                let dialog = self.hotkeys.as_ref();
                let waiting = dialog.is_some_and(|dialog| dialog.waiting == Some(action));
                let refused = dialog
                    .and_then(|dialog| dialog.refused.as_ref())
                    .filter(|(refused, _, _)| *refused == action);
                let armed = dialog.is_some_and(|dialog| dialog.esc_armed == Some(action));
                let changed = !self.keymap.is_default(action);
                let side = if waiting || refused.is_some() {
                    None
                } else if changed {
                    Some(self.text_button(
                        SharedString::from(format!("hotkey-reset-{action:?}")),
                        format!("Reset to {}", self.keymap.default_label(action)),
                        colors.roles.accent,
                        cx.listener(move |this, _: &ClickEvent, _, cx| this.reset_key(action, cx)),
                    ))
                } else {
                    self.keymap.also(action).map(|also| {
                        div()
                            .text_color(muted)
                            .child(format!("also {}", also.label(self.keymap.mac())))
                            .into_any_element()
                    })
                };
                let label = match refused {
                    _ if waiting => "Press a key…".to_string(),
                    Some((_, key, _)) => key.label(self.keymap.mac()),
                    None => self.keymap.label(action),
                };
                let note = if waiting {
                    Some(self.waiting_note(action))
                } else if let Some((_, key, refusal)) = refused {
                    Some(self.refusal_note(action, key, refusal, cx))
                } else if armed {
                    Some(
                        div()
                            .font_family(self.ui_font.clone())
                            .text_color(muted)
                            .child("Cancelled. Esc again gives it Esc.")
                            .into_any_element(),
                    )
                } else {
                    None
                };
                let tone = if waiting {
                    Some(colors.roles.accent)
                } else if refused.is_some() {
                    Some(colors.roles.warning)
                } else {
                    None
                };
                (
                    action.name(),
                    changed,
                    waiting || refused.is_some(),
                    self.chip(Some((action, tone)), label, cx),
                    side,
                    note,
                )
            }
        };
        div()
            .flex()
            .flex_col()
            .px_2()
            .py(px(1.0))
            .rounded_sm()
            .when(busy, |line| line.bg(colors.roles.focus_bar))
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
                            .font_family(self.ui_font.clone())
                            .text_color(colors.foreground)
                            .when(changed, |name| name.font_weight(FontWeight::BOLD))
                            .child(name),
                    )
                    .child(chip)
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .flex_none()
                            .w(self.scaled(120.0))
                            .children(side),
                    ),
            )
            .children(note)
            .into_any_element()
    }

    /// A key on a chip; an action's takes a click to change it, in `tone`
    /// while it waits or is refused.
    fn chip(
        &self,
        action: Option<(Action, Option<Hsla>)>,
        label: String,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let colors = &self.colors;
        let roles = &colors.roles;
        let tone = action.and_then(|(_, tone)| tone);
        let (border, text) = match action {
            Some(_) => (roles.rule, roles.accent),
            None => (roles.keycap, colors.secondary(roles.keycap)),
        };
        let id = match action {
            Some((action, _)) => format!("hotkey-chip-{action:?}"),
            None => format!("hotkey-fixed-{label}"),
        };
        div()
            .id(SharedString::from(id))
            .flex_none()
            .flex()
            .justify_center()
            .min_w(self.scaled(52.0))
            .px_2()
            .rounded_sm()
            .border_1()
            .border_color(tone.unwrap_or(border))
            .bg(roles.keycap)
            .text_color(tone.unwrap_or(text))
            .font_weight(FontWeight::BOLD)
            .child(label)
            .when_some(action, |chip, (action, _)| {
                chip.cursor(CursorStyle::PointingHand)
                    .hover(|style| style.border_color(roles.accent))
                    .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                        this.start_waiting(action, window, cx);
                        cx.stop_propagation();
                    }))
            })
            .into_any_element()
    }

    fn waiting_note(&self, action: Action) -> AnyElement {
        let note = match (action.area(), self.keymap.mac()) {
            (Area::Focus, true) => {
                "Press the new chord, with ⌘: plain keys go to the terminal. Esc cancels."
            }
            (Area::Focus, false) => {
                "Press the new chord, with Ctrl+Shift or Super: plain keys go to the terminal. Esc cancels."
            }
            _ => "Press the new key. Esc cancels; Esc twice gives it Esc.",
        };
        div()
            .font_family(self.ui_font.clone())
            .text_color(self.colors.secondary(self.colors.roles.focus_bar))
            .child(note)
            .into_any_element()
    }

    /// Why the key pressed cannot be the action's, and what can be done.
    fn refusal_note(
        &self,
        action: Action,
        key: &Key,
        refusal: &Refusal,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let colors = &self.colors;
        let mac = self.keymap.mac();
        let pressed = key.label(mac);
        let old = self.keymap.label(action);
        let reason = match refusal {
            Refusal::Clash(other) => format!(
                "⚠ {pressed} is {}'s key{}.",
                other.name(),
                if other.area() == action.area() {
                    String::new()
                } else {
                    format!(" ({})", other.area().title().to_lowercase())
                }
            ),
            Refusal::Fixed(name) => format!("⚠ {pressed} is fixed: {name}."),
            Refusal::NeedsChord if mac => "⚠ It needs ⌘: plain keys go to the terminal.".into(),
            Refusal::NeedsChord => {
                "⚠ It needs Ctrl+Shift or Super: plain keys go to the terminal.".into()
            }
        };
        let swap = match refusal {
            Refusal::Clash(other) => Some(self.text_button(
                "hotkey-swap".into(),
                format!("{} Swap: {} gets {old}", keymap::ENTER, other.name()),
                colors.roles.accent,
                cx.listener(|this, _: &ClickEvent, window, cx| this.swap(window, cx)),
            )),
            _ => None,
        };
        div()
            .flex()
            .flex_row()
            .flex_wrap()
            .items_center()
            .gap_x_2()
            .child(
                div()
                    .font_family(self.ui_font.clone())
                    .text_color(colors.roles.warning)
                    .child(reason),
            )
            .children(swap)
            .child(self.text_button(
                "hotkey-keep".into(),
                format!("{} Keep {old}", keymap::ESCAPE),
                colors.secondary(colors.roles.focus_bar),
                cx.listener(|this, _: &ClickEvent, window, cx| this.keep(window, cx)),
            ))
            .into_any_element()
    }

    fn text_button(
        &self,
        id: SharedString,
        label: String,
        color: Hsla,
        on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> AnyElement {
        div()
            .id(id)
            .flex_none()
            .px_1()
            .rounded_sm()
            .text_color(color)
            .cursor(CursorStyle::PointingHand)
            .hover(|style| style.bg(self.colors.roles.keycap))
            .child(label)
            .on_click(move |event, window, cx| {
                cx.stop_propagation();
                on_click(event, window, cx)
            })
            .into_any_element()
    }

    /// Reset all, and the key that closes the dialog.
    fn hotkeys_footer(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = &self.colors;
        let muted = colors.secondary(colors.roles.popup_fill);
        let any = Action::ALL
            .iter()
            .any(|&action| !self.keymap.is_default(action));
        div()
            .flex()
            .flex_row()
            .flex_wrap()
            .items_center()
            .gap_3()
            .pt_2()
            .border_t_1()
            .border_color(colors.roles.rule)
            .child(if any {
                self.text_button(
                    "hotkeys-reset-all".into(),
                    "⟲ Reset all".into(),
                    colors.roles.accent,
                    cx.listener(|this, _: &ClickEvent, window, cx| this.reset_all_keys(window, cx)),
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
                    .font_family(self.ui_font.clone())
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
                            .bg(colors.roles.keycap)
                            .text_color(colors.roles.accent)
                            .font_weight(FontWeight::BOLD)
                            .child(keymap::ESCAPE),
                    )
                    .child(div().text_color(muted).child("close")),
            )
    }
}
