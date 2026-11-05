//! The status bar's machines: each with its connection's dot and its agent
//! count, the one shown tinted; a click switches the whole window to one,
//! a right click opens its menu (Edit, Remove), and `+` opens the "Add
//! machine" form (`machines::MachineForm`). The list is kept in
//! `$STATE/gui-machines.json` with the one shown.

use gpui::{
    AnyElement, ClickEvent, Context, CursorStyle, FontWeight, InteractiveElement, IntoElement,
    MouseButton, MouseDownEvent, ParentElement, Pixels, Point, SharedString,
    StatefulInteractiveElement, Styled, Window, div, point, prelude::FluentBuilder, px,
};

use crate::{
    dialogs::{Dialog, MachineMenu},
    keymap,
    machines::{self, Machine, MachineForm, MachineRow, SavedMachine, SavedMachines},
    rows::Pick,
    store::Connection,
    window::CorgiWindow,
};

impl CorgiWindow {
    // ---- Switching ----

    /// Shows machine `index`: the overview, the side panel, the header and
    /// every action are its from now on, and its selection, collapsed cards
    /// and expanded row are back as it left them. Kept for the next run.
    pub fn switch_machine(&mut self, index: usize, cx: &mut Context<Self>) {
        if index == self.active || self.dialogs.open.is_some() || self.hotkeys.is_some() {
            return;
        }
        self.leave_focus(cx);
        let Some(old) = machines::switch(
            &mut self.machines,
            &mut self.active,
            &mut self.dialogs.all_running,
            index,
        ) else {
            return;
        };
        // The machine left stops reading its expanded session.
        self.sync_watch_of(old);
        self.transcript.reset();
        self.overview_scroll.set_offset(point(px(0.), px(0.)));
        if let Some(selected) = self.machine().selected.clone() {
            self.rows.pick(&selected, Pick::Reveal);
        }
        self.sync_terminal_socket(cx);
        self.recolor(cx);
        self.sync_watch();
        self.save_machines();
        cx.notify();
    }

    /// Keeps the saved machines and the one shown. While `--ssh`'s one-off
    /// shows, the one kept as shown stays what it was.
    fn save_machines(&mut self) {
        let path = self.machines_path.as_deref();
        let active = match &self.machine().saved {
            Some(saved) => Some(saved.host.clone()),
            None if self.active == 0 => None,
            None => SavedMachines::load(path).active,
        };
        let saved = SavedMachines {
            active,
            machines: self
                .machines
                .iter()
                .filter_map(|machine| machine.saved.clone())
                .collect(),
        };
        if let Err(error) = saved.save(path) {
            self.dialogs.say(error, true);
        }
    }

    // ---- Adding, editing, removing ----

    /// `+`: the "Add machine" form, its host's list open.
    pub fn open_add_machine(&mut self, cx: &mut Context<Self>) {
        if self.dialogs.open.is_some() {
            return;
        }
        self.dialogs.open = Some(Dialog::Machine(Box::new(MachineForm::add(
            machines::ssh_config_hosts(),
        ))));
        self.focus_dialog(cx);
        cx.notify();
    }

    /// A right click on machine `index`: its menu, above the click.
    pub fn open_machine_menu(&mut self, index: usize, at: Point<Pixels>, cx: &mut Context<Self>) {
        if self.dialogs.open.is_some() {
            return;
        }
        let machine = &self.machines[index];
        match &machine.saved {
            Some(saved) => {
                self.dialogs.open = Some(Dialog::MachineMenu(MachineMenu {
                    host: saved.host.clone(),
                    at,
                    cursor: 0,
                }));
                self.focus_dialog(cx);
            }
            None if index == 0 => self
                .dialogs
                .say("This machine is always first; add others with +", false),
            None => self.dialogs.say(
                format!(
                    "{} is shown for this run (--ssh); add it with + to keep it",
                    machine.name
                ),
                false,
            ),
        }
        cx.notify();
    }

    /// Enter in a machine's menu, or a click on one of its items.
    pub fn choose_machine_menu(&mut self, item: usize, cx: &mut Context<Self>) {
        let Some(Dialog::MachineMenu(menu)) = &self.dialogs.open else {
            return;
        };
        let host = menu.host.clone();
        let Some(saved) = self.saved_machine(&host).cloned() else {
            self.dialogs.open = None;
            return;
        };
        self.dialogs.open = Some(if item == 0 {
            Dialog::Machine(Box::new(MachineForm::edit(
                &saved,
                machines::ssh_config_hosts(),
            )))
        } else {
            Dialog::RemoveMachine {
                host,
                name: saved.name,
            }
        });
        self.focus_dialog(cx);
        cx.notify();
    }

    fn saved_machine(&self, host: &str) -> Option<&SavedMachine> {
        self.machines
            .iter()
            .filter_map(|machine| machine.saved.as_ref())
            .find(|saved| saved.host == host)
    }

    /// Enter in the machine form: adds the machine, or saves the one
    /// edited, and connects to it at once. A refused value keeps the form
    /// open on its row.
    pub fn submit_machine(&mut self, cx: &mut Context<Self>) {
        let Some(Dialog::Machine(form)) = &self.dialogs.open else {
            return;
        };
        let editing = form.editing.clone();
        let others = self
            .machines
            .iter()
            .filter_map(|machine| machine.saved.as_ref())
            .filter(|saved| Some(&saved.host) != editing.as_ref());
        let (names, hosts): (Vec<String>, Vec<String>) = others
            .map(|saved| (saved.name.clone(), saved.host.clone()))
            .unzip();
        let Some(Dialog::Machine(form)) = &mut self.dialogs.open else {
            return;
        };
        let Some(saved) = form.machine(&names, &hosts) else {
            cx.notify();
            return;
        };
        self.dialogs.open = None;
        let name = saved.name.clone();
        match editing {
            Some(old) => {
                self.update_machine(&old, saved, cx);
                self.dialogs.say(format!("Saved {name}"), false);
            }
            None => {
                self.add_machine(saved, cx);
                self.dialogs
                    .say(format!("Added {name}; connecting…"), false);
            }
        }
        self.save_machines();
        self.focus_dialog(cx);
        cx.notify();
    }

    /// Adds `saved` and connects to it. `--ssh`'s one-off of the same host
    /// becomes it, in its place.
    fn add_machine(&mut self, saved: SavedMachine, cx: &mut Context<Self>) {
        let one_off = self
            .machines
            .iter()
            .position(|m| m.saved.is_none() && m.host() == Some(saved.host.as_str()));
        let index = match one_off {
            Some(at) => {
                self.machines[at] = Machine::saved(&saved);
                at
            }
            None => {
                self.machines.push(Machine::saved(&saved));
                self.machines.len() - 1
            }
        };
        self.connect(index, cx);
        if index == self.active {
            self.machine_reset(cx);
        }
    }

    /// Saves the machine of host `old` as `saved`; a new host or `corgi`
    /// is another `corgi serve`, connected to afresh.
    fn update_machine(&mut self, old: &str, saved: SavedMachine, cx: &mut Context<Self>) {
        let Some(index) = self
            .machines
            .iter()
            .position(|machine| machine.saved.as_ref().is_some_and(|s| s.host == old))
        else {
            return;
        };
        let machine = &mut self.machines[index];
        let same_serve = machine
            .saved
            .as_ref()
            .is_some_and(|s| s.host == saved.host && s.corgi == saved.corgi);
        if same_serve {
            machine.name = saved.name.clone();
            machine.saved = Some(saved);
            return;
        }
        *machine = Machine::saved(&saved);
        self.connect(index, cx);
        if index == self.active {
            self.machine_reset(cx);
        }
    }

    /// The shown machine's connection started over: nothing of the last
    /// one shows.
    fn machine_reset(&mut self, cx: &mut Context<Self>) {
        self.leave_focus(cx);
        self.transcript.reset();
        self.dialogs.all_running = None;
        self.sync_terminal_socket(cx);
        self.recolor(cx);
    }

    /// Enter in the removal's confirmation: the machine leaves the list and
    /// its `corgi serve` ends. Removing the one shown shows this machine.
    pub fn remove_machine(&mut self, cx: &mut Context<Self>) {
        let Some(Dialog::RemoveMachine { host, name }) = self.dialogs.open.take() else {
            return;
        };
        let Some(index) = self
            .machines
            .iter()
            .position(|machine| machine.saved.as_ref().is_some_and(|s| s.host == host))
        else {
            return;
        };
        if index == self.active {
            self.switch_machine(0, cx);
        }
        // Dropping its link closes `corgi serve`'s stdin, which ends it.
        machines::remove(&mut self.machines, &mut self.active, index);
        self.save_machines();
        self.dialogs.say(format!("Removed {name}"), false);
        self.focus_dialog(cx);
        cx.notify();
    }

    // ---- Drawing ----

    /// The machines, as `● name · N agents │ … │ +`.
    pub fn machine_list(&self, window: &Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = &self.colors;
        let rule = || div().flex_none().text_color(colors.roles.rule).child("│");
        // It keeps its room; what follows it gives way (`status_bar`). When
        // even the bar is too narrow, the names of the machines not shown
        // are cut short, the longest first, so `+` still shows.
        let names = self.fitted_names(window);
        let mut list = div()
            .id("machines")
            .flex()
            .flex_row()
            .flex_none()
            .items_center()
            .gap_1();
        for (index, (machine, name)) in self.machines.iter().zip(names).enumerate() {
            if index > 0 {
                list = list.child(rule());
            }
            list = list.child(self.machine_chip(index, machine, name, cx));
        }
        let add = div()
            .id("machine-add")
            .flex_none()
            .px_1p5()
            .rounded_sm()
            .text_color(colors.roles.accent)
            .font_weight(FontWeight::BOLD)
            .cursor(CursorStyle::PointingHand)
            .hover(|style| style.bg(colors.roles.keycap))
            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                this.open_add_machine(cx);
                cx.stop_propagation();
            }))
            .child("+");
        list.child(rule()).child(add)
    }

    /// Each machine's label as the bar has room for it.
    fn fitted_names(&self, window: &Window) -> Vec<String> {
        let cell = self.monospace_cell_width(window);
        let scale = self.layout.text_scale();
        let labels: Vec<String> = self.machines.iter().map(Machine::label).collect();
        // Everything but the names: each chip's padding, gaps, dot, `·`
        // and count, the rules between them, `+`, and the bar's padding.
        let chips: f32 = self
            .machines
            .iter()
            .map(|machine| {
                (12.0 + 12.0) * scale + (2 + chip_count(machine).chars().count()) as f32 * cell
            })
            .sum();
        let pieces = 2 * self.machines.len() + 1;
        let fixed = chips
            + self.machines.len() as f32 * cell
            + (12.0 * scale + cell)
            + (pieces - 1) as f32 * 4.0 * scale
            + 32.0 * scale;
        let room = f32::from(window.viewport_size().width);
        let lengths: Vec<usize> = labels.iter().map(|label| label.chars().count()).collect();
        let fitted = fit_names(&lengths, self.active, fixed, cell, room);
        labels
            .into_iter()
            .zip(fitted)
            .map(|(label, most)| shortened(&label, most))
            .collect()
    }

    fn machine_chip(
        &self,
        index: usize,
        machine: &Machine,
        name: String,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let colors = &self.colors;
        let shown = index == self.active;
        let store = &machine.store;
        let (dot, dot_color) = match store.connection {
            Connection::Connected => ("●", colors.roles.success),
            Connection::Connecting => ("○", colors.roles.warning),
            Connection::Lost { .. } => ("○", colors.roles.danger),
        };
        let has_state = store.seq.is_some() || !store.state.agents.is_empty();
        let what = chip_count(machine);
        let fill = if shown {
            colors.selected_row()
        } else {
            colors.background
        };
        let muted = colors.secondary(fill);
        div()
            .id(SharedString::from(format!("machine-{index}")))
            .flex()
            .flex_row()
            .items_center()
            .gap_1()
            .px_1p5()
            .rounded_sm()
            .flex_none()
            .when(shown, |chip| chip.bg(fill))
            .when(!shown, |chip| {
                chip.cursor(CursorStyle::PointingHand)
                    .hover(|style| style.bg(colors.roles.keycap))
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                        this.switch_machine(index, cx);
                        cx.stop_propagation();
                    }))
            })
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                    this.open_machine_menu(index, event.position, cx);
                    cx.stop_propagation();
                }),
            )
            .child(div().flex_none().text_color(dot_color).child(dot))
            .child(
                div()
                    .text_color(if shown { colors.roles.text } else { muted })
                    .when(shown, |name| name.font_weight(FontWeight::BOLD))
                    .child(name),
            )
            .child(div().flex_none().text_color(muted).child("·"))
            .child(
                div()
                    .flex_none()
                    .text_color(if has_state { colors.roles.text } else { muted })
                    .child(what),
            )
    }

    /// A machine's menu, a small list above where it was right-clicked; a
    /// click outside closes it, as Esc does.
    pub fn machine_menu_layer(
        &self,
        menu: &MachineMenu,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let colors = &self.colors;
        let height = window.viewport_size().height;
        let items = MachineMenu::ITEMS.iter().enumerate().map(|(item, label)| {
            let focused = item == menu.cursor;
            div()
                .id(SharedString::from(format!("machine-menu-{item}")))
                .px_2()
                .rounded_sm()
                .cursor(CursorStyle::PointingHand)
                .when(focused, |row| row.bg(colors.roles.focus_bar))
                .hover(|style| style.bg(colors.roles.keycap))
                .text_color(if item == 1 {
                    colors.roles.danger
                } else {
                    colors.roles.text
                })
                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                    this.choose_machine_menu(item, cx);
                    cx.stop_propagation();
                }))
                .child(label.to_string())
        });
        div()
            .id("machine-menu-layer")
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .occlude()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, window, cx| {
                    cx.stop_propagation();
                    this.escape_dialog(window, cx);
                }),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|this, _, window, cx| {
                    cx.stop_propagation();
                    this.escape_dialog(window, cx);
                }),
            )
            .child(
                div()
                    .id("machine-menu")
                    .key_context("Dialog")
                    .track_focus(&self.form_focus)
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .absolute()
                    .left(menu.at.x)
                    .bottom(height - menu.at.y + px(4.))
                    .flex()
                    .flex_col()
                    .min_w(self.scaled(120.))
                    .p_1()
                    .rounded_md()
                    .border_1()
                    .border_color(colors.roles.accent)
                    .bg(colors.roles.popup_fill)
                    .shadow_lg()
                    .children(items),
            )
    }

    /// The "Add machine" form, or the same to edit one.
    pub fn machine_dialog(&self, form: &MachineForm, cx: &mut Context<Self>) -> AnyElement {
        let colors = &self.colors;
        let mut rows = div().flex().flex_col();
        for row in MachineRow::ALL {
            let focused = form.row == row;
            let shown = form.shown(row).to_string();
            let empty = shown.is_empty();
            let value = div()
                .flex()
                .flex_row()
                .gap_2()
                .child(
                    div()
                        .text_color(if empty {
                            colors.secondary(colors.roles.popup_fill)
                        } else {
                            colors.roles.text
                        })
                        .child(match (empty, focused) {
                            (true, true) => "▏".to_string(),
                            (true, false) => "—".to_string(),
                            (false, true) => format!("{shown}▏"),
                            (false, false) => shown,
                        }),
                )
                .child(
                    div()
                        .text_color(colors.secondary(colors.roles.popup_fill))
                        .truncate()
                        .child(form.detail(row)),
                );
            rows = rows.child(
                div()
                    .id(SharedString::from(format!("machine-form-{}", row.label())))
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .px_1()
                    .py(self.scaled(2.))
                    .rounded_sm()
                    .when(focused, |r| r.bg(colors.roles.focus_bar))
                    .child(
                        div()
                            .w(self.scaled(12.))
                            .text_color(colors.roles.accent)
                            .child(if focused { "▶" } else { "" }),
                    )
                    .child(
                        div()
                            .w(self.scaled(80.))
                            .flex_none()
                            .text_color(if focused {
                                colors.roles.text
                            } else {
                                colors.secondary(colors.roles.popup_fill)
                            })
                            .when(focused, |d| d.font_weight(FontWeight::BOLD))
                            .child(row.label()),
                    )
                    .child(div().flex_1().min_w_0().child(value))
                    .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                        if let Some(Dialog::Machine(form)) = &mut this.dialogs.open {
                            form.row = row;
                            form.picker = (row == MachineRow::Host).then(Default::default);
                        }
                        this.focus_dialog_now(window, cx);
                        cx.notify();
                    })),
            );
            if focused && form.picker.is_some() {
                let cursor = form.picker.as_ref().map_or(0, |picker| picker.cursor);
                let choices = form.picker_rows();
                rows = rows.child(self.choice_list(None, &choices, cursor));
            }
        }
        let save = if form.editing.is_some() {
            "save"
        } else {
            "add machine"
        };
        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(self.dialog_title(form.title().to_string()))
            .child(
                div()
                    .text_color(colors.secondary(colors.roles.popup_fill))
                    .child("Corgi runs corgi serve there over ssh, as you would by hand, without asking for a password."),
            )
            .child(rows)
            .children(
                form.error
                    .as_ref()
                    .map(|error| div().text_color(colors.roles.danger).child(error.clone())),
            )
            .child(self.hints(&[
                (keymap::ENTER, save),
                ("↑↓ ⇥", "rows"),
                (keymap::ESCAPE, "cancel"),
            ]))
            .into_any_element()
    }

    /// The confirmation before a machine leaves the list.
    pub fn remove_machine_dialog(
        &self,
        host: &str,
        name: &str,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(self.dialog_title(format!("Remove {name}")))
            .child(div().child(format!(
                "The window stops showing {host} and ends its corgi serve. Nothing on {host} changes, and + adds it again."
            )))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .gap_2()
                    .pt_1()
                    .child(self.button(
                        keymap::ENTER,
                        "Remove",
                        true,
                        true,
                        cx,
                        |this, _, cx| this.remove_machine(cx),
                    ))
                    .child(self.button(
                        keymap::ESCAPE,
                        "Cancel",
                        true,
                        false,
                        cx,
                        |this, _, cx| {
                            this.dialogs.open = None;
                            cx.notify();
                        },
                    )),
            )
            .into_any_element()
    }
}

/// What a machine's chip says after its name: its agents, or why there are
/// none to count.
fn chip_count(machine: &Machine) -> String {
    let store = &machine.store;
    let count = store.state.agents.len();
    match &store.connection {
        _ if store.seq.is_some() || count > 0 => {
            format!("{count} agent{}", if count == 1 { "" } else { "s" })
        }
        Connection::Lost { .. } => "unreachable".into(),
        _ => "connecting".into(),
    }
}

/// The shortest a name is cut to.
const NAME_LEAST: usize = 4;

/// The most characters of each name (`lengths`) that fit `room` pixels
/// beside `fixed` pixels of everything else, at `cell` pixels a character:
/// the longest name not `shown` loses a character at a time, down to
/// `NAME_LEAST`; the one shown is never cut.
fn fit_names(lengths: &[usize], shown: usize, fixed: f32, cell: f32, room: f32) -> Vec<usize> {
    let mut fitted = lengths.to_vec();
    let mut width = fixed + fitted.iter().sum::<usize>() as f32 * cell;
    while width > room {
        let longest = fitted
            .iter()
            .enumerate()
            .filter(|(index, length)| *index != shown && **length > NAME_LEAST)
            .max_by_key(|(index, length)| (**length, std::cmp::Reverse(*index)))
            .map(|(index, _)| index);
        let Some(longest) = longest else { break };
        fitted[longest] -= 1;
        width -= cell;
    }
    fitted
}

/// `label` in at most `most` characters, its end an ellipsis when cut.
fn shortened(label: &str, most: usize) -> String {
    if label.chars().count() <= most {
        return label.to_string();
    }
    let kept: String = label.chars().take(most.saturating_sub(1)).collect();
    format!("{kept}…")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_cut_longest_first_to_fit_and_the_shown_one_never() {
        // Room for everything: nothing is cut.
        assert_eq!(fit_names(&[12, 7, 20], 1, 100.0, 10.0, 1000.0), [12, 7, 20]);
        // Ten characters too many: the longest gives them.
        assert_eq!(fit_names(&[12, 7, 20], 1, 100.0, 10.0, 410.0), [12, 7, 12]);
        // Then the longest by turns, the earlier first on a tie.
        assert_eq!(fit_names(&[12, 7, 20], 1, 100.0, 10.0, 300.0), [6, 7, 7]);
        // Never below the least, and never the one shown.
        assert_eq!(fit_names(&[12, 7, 20], 1, 100.0, 10.0, 0.0), [4, 7, 4]);
        assert_eq!(shortened("a-rather-long-name", 6), "a-rat…");
        assert_eq!(shortened("box", 4), "box");
    }
}
