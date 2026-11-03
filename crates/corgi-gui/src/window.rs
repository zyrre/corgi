//! The window: its connection to `corgi serve`, the selection, the keys and
//! the actions, and the focus view around an agent's terminal
//! (`crate::terminal`). What it draws is in `view.rs`.

use std::{collections::HashSet, path::PathBuf, sync::Arc, time::Duration};

use corgi_core::protocol::{Agent, BacklogEntry, Command, Project, Question};
use futures::{StreamExt, channel::mpsc};
use gpui::{
    App, AppContext, ClipboardItem, Context, Entity, FocusHandle, Focusable, Font, KeyDownEvent,
    Pixels, ScrollHandle, SharedString, Subscription, Task, Window, WindowVisibility, px,
};

use crate::{
    dialogs::{Dialog, Dialogs, Followup, MergeAction, MergeStage, Orphans},
    form::{self, Key},
    hotkeys::HotkeysDialog,
    keymap::{self, Action, Hit, Key as KeymapKey, Keymap},
    layout::{Layout, Split},
    link::{Backoff, Link, LinkEvent},
    options::Options,
    rows::{Pick, Rows},
    store::{Closing, Connection, Outbox, Purpose, Received, Store},
    terminal::{
        Target, TerminalEvent, TerminalView,
        keys::{self, Chord},
    },
    text_input::{FieldColors, TextInput},
    theme::{self, Colors, forced_light, is_light},
    transcript::Transcript,
};

/// A split being dragged, and where the pointer was last.
#[derive(Debug, Clone, Copy)]
pub struct Drag {
    pub split: Split,
    pub last: Pixels,
}

pub struct CorgiWindow {
    pub options: Options,
    pub store: Store,
    outbox: Outbox,
    pub dialogs: Dialogs,
    pub layout: Layout,
    layout_path: Option<PathBuf>,
    link: Option<Arc<Link>>,
    links_started: u64,
    backoff: Backoff,
    events: mpsc::UnboundedSender<(u64, LinkEvent)>,
    /// The agent the keys act on: none at start, until a key or a click
    /// picks one.
    pub selected: Option<String>,
    /// A row has been picked since the window opened: from then on, when
    /// the selected row goes, another is selected in its place.
    picked: bool,
    /// The selected row is expanded into its session's latest turns
    /// (Space), down to the window's bottom; the expansion follows the
    /// selection.
    pub expanded: bool,
    /// The expanded session's scroll, its text selection and its lines as
    /// last drawn.
    pub transcript: Transcript,
    /// The session `corgi serve` was asked to read the transcript of.
    watching: Option<String>,
    /// The project groups whose cards show only their supervisor.
    pub collapsed: HashSet<String>,
    /// The card groups this connection to `corgi serve` has shown: a card
    /// not among them starts collapsed, and one the user expanded stays so.
    seen: HashSet<String>,
    /// The questions closed with ×, until the ledger drops them.
    pub closing: Closing,
    /// The window is on screen; while it is not, nothing is drawn and the
    /// expanded session is not read.
    pub visible: bool,
    /// The system's appearance, and `CORGI_GUI_APPEARANCE`'s, which wins
    /// over it and the header's toggle (`theme::appearance`).
    system_light: bool,
    forced_light: Option<bool>,
    /// The appearance the header's toggle chose, for this window only:
    /// until it is clicked, the window follows the system.
    chosen_light: Option<bool>,
    pub colors: Colors,
    pub font: Font,
    /// The proportional font a question's text is set in.
    pub ui_font: SharedString,
    pub focus: FocusHandle,
    pub form_focus: FocusHandle,
    pub prompt_input: Entity<TextInput>,
    pub task_input: Entity<TextInput>,
    pub overview_scroll: ScrollHandle,
    /// The orphans dialog's list, which keys keep on the selected card.
    pub orphans_scroll: ScrollHandle,
    /// Where the overview's rows were drawn, to scroll one into view.
    pub rows: Rows,
    /// The answer box's question text, back at its top for each question.
    pub question_scroll: ScrollHandle,
    pub drag: Option<Drag>,
    /// The focus view's terminal, kept while the overview shows so its
    /// connection stays warm, parked and inactive.
    pub terminal: Entity<TerminalView>,
    /// The agent whose terminal the focus view shows; `None` shows the
    /// overview.
    pub focus_view: Option<String>,
    /// The keyboard should move to where the open dialog wants it.
    refocus: bool,
    /// Which key does what (`keymap.rs`), and where it is kept.
    pub keymap: Keymap,
    keymap_path: Option<PathBuf>,
    /// The hotkeys dialog, while it is open (`hotkeys.rs`).
    pub hotkeys: Option<HotkeysDialog>,
    _tasks: Vec<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

impl CorgiWindow {
    pub fn new(
        options: Options,
        font: Font,
        ui_font: SharedString,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let forced_light = forced_light(std::env::var("CORGI_GUI_APPEARANCE").ok().as_deref());
        let system_light = is_light(window.appearance());
        let layout_path = Layout::path();
        let layout = Layout::load(layout_path.as_deref());
        let keymap_path = Keymap::path();
        let keymap = Keymap::load(keymap_path.as_deref(), cfg!(target_os = "macos"));
        let store = Store::default();
        let (light, chosen) = theme::appearance(forced_light, None, system_light);
        let colors = Colors::new(&store.state.palette, light, chosen);
        let field = field_colors(&colors);
        let prompt_input = cx.new(|cx| TextInput::new("Type a prompt", field, cx));
        let terminal = cx.new(|cx| TerminalView::new(font.family.clone(), cx));
        let task_input = cx.new(|cx| TextInput::new("Describe the task", field, cx));
        let (events, mut receiver) = mpsc::unbounded::<(u64, LinkEvent)>();

        // Every line from `corgi serve` comes here; a burst that queued while
        // the window was busy is handled at once and drawn once.
        let pump = cx.spawn(async move |this, cx| {
            while let Some(first) = receiver.next().await {
                let mut batch = vec![first];
                while let Ok(more) = receiver.try_recv() {
                    batch.push(more);
                }
                let alive = this.update(cx, |this, cx| {
                    let mut redraw = false;
                    for (number, event) in batch {
                        redraw |= this.link_event(number, event, cx);
                    }
                    if redraw && this.visible {
                        cx.notify();
                    }
                });
                if alive.is_err() {
                    break;
                }
            }
        });
        // Countdowns (the prompt cache, the reconnect) move once a second,
        // only while the window shows. The focus view, whose terminal
        // otherwise draws only when Herdr sends something, redraws for them
        // once a minute, as its clocks read, but each second when one counts
        // seconds.
        let tick = cx.spawn(async move |this, cx| {
            let mut minute = 0;
            loop {
                cx.background_executor().timer(Duration::from_secs(1)).await;
                let alive = this.update(cx, |this, cx| {
                    let now = this.store.now();
                    let due =
                        this.focus_view.is_none() || now / 60 != minute || this.counts_seconds(now);
                    if this.visible && due {
                        minute = now / 60;
                        cx.notify();
                    }
                });
                if alive.is_err() {
                    break;
                }
            }
        });

        let subscriptions = vec![
            // Followed until the header's toggle chooses.
            cx.observe_window_appearance(window, |this, window, cx| {
                this.system_light = is_light(window.appearance());
                this.recolor(cx);
            }),
            cx.observe_window_visibility(window, |this, visibility, _, cx| {
                this.set_visible(visibility == WindowVisibility::Visible, cx);
            }),
            // Coming back to the window gives the terminal the keyboard
            // again, which takes back its tab's size from Herdr's own
            // window if the user worked there meanwhile.
            cx.observe_window_activation(window, |this, window, cx| {
                this.terminal
                    .update(cx, |terminal, _| terminal.report_focus(window));
            }),
            cx.subscribe(
                &terminal,
                |this, _, event: &TerminalEvent, cx| match event {
                    TerminalEvent::Leave => this.leave_focus(cx),
                    TerminalEvent::ToggleSide => this.toggle_side(cx),
                },
            ),
            // The focus bar says how the terminal is.
            cx.observe(&terminal, |_, _, cx| cx.notify()),
            // Quitting parks and deactivates the terminal's connection.
            cx.on_app_quit(|this, cx| {
                this.terminal
                    .update(cx, |terminal, cx| terminal.shut_down(cx))
            }),
        ];

        let mut this = Self {
            options,
            store,
            outbox: Outbox::default(),
            dialogs: Dialogs::default(),
            layout,
            layout_path,
            link: None,
            links_started: 0,
            backoff: Backoff::default(),
            events,
            selected: None,
            picked: false,
            expanded: false,
            transcript: Transcript::default(),
            watching: None,
            collapsed: HashSet::new(),
            seen: HashSet::new(),
            closing: Closing::default(),
            visible: window.is_visible(),
            system_light,
            forced_light,
            chosen_light: None,
            colors,
            font,
            ui_font,
            focus: cx.focus_handle(),
            form_focus: cx.focus_handle(),
            prompt_input,
            task_input,
            overview_scroll: ScrollHandle::new(),
            orphans_scroll: ScrollHandle::new(),
            rows: Rows::default(),
            question_scroll: ScrollHandle::new(),
            drag: None,
            terminal,
            focus_view: None,
            refocus: false,
            keymap,
            keymap_path,
            hotkeys: None,
            _tasks: vec![pump, tick],
            _subscriptions: subscriptions,
        };
        this.sync_terminal_socket(cx);
        this.color_terminal(cx);
        this.share_keymap(cx);
        let scale = this.layout.text_scale();
        this.terminal
            .update(cx, |terminal, cx| terminal.set_text_scale(scale, cx));
        this.connect(cx);
        window.focus(&this.focus, cx);
        this
    }

    // ---- The connection ----

    fn connect(&mut self, cx: &mut Context<Self>) {
        self.links_started += 1;
        let number = self.links_started;
        self.store.connecting();
        let (program, args) = self.options.command();
        let events = self.events.clone();
        let sender = Box::new(move |number, event| {
            let _ = events.unbounded_send((number, event));
        });
        match Link::start(number, &program, &args, sender) {
            Ok(link) => self.link = Some(Arc::new(link)),
            Err(error) => {
                let _ = self
                    .events
                    .unbounded_send((number, LinkEvent::Ended(error.to_string())));
            }
        }
        cx.notify();
    }

    /// Handles one event of connection `number`; true when the window
    /// should draw again.
    fn link_event(&mut self, number: u64, event: LinkEvent, cx: &mut Context<Self>) -> bool {
        if number != self.links_started {
            return false;
        }
        match event {
            LinkEvent::Message(message) => {
                let hello = matches!(*message, corgi_core::protocol::ServerMessage::Hello(_));
                if hello {
                    self.backoff.reset();
                }
                let received = self.store.receive(*message);
                if hello {
                    self.sync_terminal_socket(cx);
                }
                match received {
                    None => false,
                    Some(Received::State) => {
                        let projects = &self.store.state.projects;
                        let selected = self.selected.as_deref();
                        if let Some(supervisor) =
                            collapse_new(&mut self.collapsed, &mut self.seen, projects, selected)
                        {
                            self.selected = Some(supervisor);
                        }
                        self.reconcile(cx);
                        true
                    }
                    Some(Received::Resync) => {
                        self.send(Command::Resync, Purpose::Quiet);
                        false
                    }
                    Some(Received::Progress(progress)) => {
                        if let Some(purpose) = self.outbox.purpose(progress.id).cloned() {
                            self.dialogs.progress(&purpose, &progress);
                        }
                        true
                    }
                    Some(Received::Reply(reply)) => {
                        if let Some(purpose) = self.outbox.answered(&reply) {
                            self.closing.replied(&purpose, &reply);
                            if let Some(followup) = self.dialogs.reply(purpose, reply) {
                                self.follow(followup, cx);
                            }
                        }
                        self.focus_dialog(cx);
                        true
                    }
                }
            }
            LinkEvent::Ended(why) => {
                self.link = None;
                // The next connection's state is fresh: every card collapses.
                self.seen.clear();
                self.watching = None;
                let delay = self.backoff.next_delay();
                self.store.lost(why, delay);
                let unanswered = self.outbox.lost();
                self.dialogs.lost(&unanswered);
                self.closing.clear();
                let retry = cx.spawn(async move |this, cx| {
                    cx.background_executor().timer(delay).await;
                    let _ = this.update(cx, |this, cx| {
                        if this.link.is_none() && this.links_started == number {
                            this.connect(cx);
                        }
                    });
                });
                self._tasks.push(retry);
                self._tasks.retain(|task| !task.is_ready());
                true
            }
        }
    }

    /// Connects again now rather than when the wait ends.
    pub fn retry_now(&mut self, cx: &mut Context<Self>) {
        if self.link.is_none() {
            self.connect(cx);
        }
    }

    /// Sends `command`; false when it could not be.
    pub fn send(&mut self, command: Command, purpose: Purpose) -> bool {
        let Some(link) = self.link.clone().filter(|_| self.store.connected()) else {
            if purpose != Purpose::Quiet {
                self.dialogs.say(
                    "Not connected to corgi serve; try again once it reconnects",
                    true,
                );
            }
            return false;
        };
        let Some(request) = self.outbox.request(&self.store, command, purpose) else {
            self.dialogs.say(
                "That command would move every Herdr client's view, so the window never sends it",
                true,
            );
            return false;
        };
        if let Err(error) = link.send(&request) {
            self.outbox.unsent(request.id);
            self.dialogs
                .say(format!("Could not reach corgi serve: {error}"), true);
            return false;
        }
        true
    }

    // ---- Selection and what follows it ----

    /// The agents the keys step through, in the overview's order: every
    /// row but the workers of a collapsed card.
    pub fn visible_agents(&self) -> Vec<&Agent> {
        let state = &self.store.state;
        let mut agents = Vec::new();
        for project in &state.projects {
            let collapsed = project.card && self.collapsed.contains(&project.group);
            for pane_id in &project.agents {
                if collapsed && project.supervisor.as_deref() != Some(pane_id.as_str()) {
                    continue;
                }
                if let Some(agent) = self.store.agent(pane_id) {
                    agents.push(agent);
                }
            }
        }
        // Rows no project lists (a server that sends none) still show.
        for agent in &state.agents {
            if !agents.iter().any(|shown| shown.pane_id == agent.pane_id)
                && !state
                    .projects
                    .iter()
                    .any(|p| p.agents.contains(&agent.pane_id))
            {
                agents.push(agent);
            }
        }
        agents
    }

    /// Whether a clock the window shows counts seconds now: a prompt cache
    /// in its last minute, or the wait before connecting again.
    fn counts_seconds(&self, now: u64) -> bool {
        !self.store.connected()
            || self.store.state.agents.iter().any(|agent| {
                agent
                    .cache
                    .as_ref()
                    .is_some_and(|cache| cache.expires_at > now && cache.expires_at - now <= 60)
            })
    }

    pub fn selected_agent(&self) -> Option<&Agent> {
        self.store.agent(self.selected.as_deref()?)
    }

    /// Keeps the selection on a row that exists, and the transcript read
    /// for it.
    fn reconcile(&mut self, cx: &mut Context<Self>) {
        let visible: Vec<String> = self
            .visible_agents()
            .iter()
            .map(|agent| agent.pane_id.clone())
            .collect();
        let before = self.selected.clone();
        self.selected = reconciled(self.selected.take(), self.picked, &visible);
        if self.selected != before {
            // Another session's turns start at its newest.
            self.transcript.reset();
        }
        self.expanded &= self.selected.is_some();
        self.closing.prune(&self.store.state);
        self.recolor(cx);
        self.sync_watch();
    }

    /// Selects `pane_id`'s row. A click on it leaves the overview where it
    /// is, so the row stays under the mouse; any other `pick` scrolls just
    /// enough to show the row in full.
    pub fn select(&mut self, pane_id: String, pick: Pick, cx: &mut Context<Self>) {
        self.rows.pick(&pane_id, pick);
        if self.selected.as_deref() != Some(pane_id.as_str()) {
            self.transcript.reset();
        }
        self.selected = Some(pane_id);
        self.picked = true;
        self.sync_watch();
        cx.notify();
    }

    fn step(&mut self, delta: isize, cx: &mut Context<Self>) {
        let agents = self.visible_agents();
        if agents.is_empty() {
            return;
        }
        // With nothing selected, a step down selects the first row and a
        // step up the last.
        let before = if delta > 0 { -1 } else { agents.len() as isize };
        let at = self
            .selected
            .as_ref()
            .and_then(|selected| agents.iter().position(|agent| &agent.pane_id == selected))
            .map_or(before, |at| at as isize);
        let next = (at + delta).clamp(0, agents.len() as isize - 1) as usize;
        let pane_id = agents[next].pane_id.clone();
        self.select(pane_id, Pick::Reveal, cx);
    }

    /// Asks `corgi serve` to read the selected agent's transcript while its
    /// row is expanded and the window is on screen, and to stop otherwise.
    fn sync_watch(&mut self) {
        if !self.store.connected() {
            return;
        }
        let wanted = self
            .selected
            .clone()
            .filter(|_| self.visible && self.expanded && self.focus_view.is_none());
        if wanted != self.watching {
            self.watching = wanted.clone();
            self.send(Command::Watch { pane_id: wanted }, Purpose::Quiet);
        }
    }

    pub fn set_visible(&mut self, visible: bool, cx: &mut Context<Self>) {
        if self.visible == visible {
            return;
        }
        self.visible = visible;
        // Said on stderr, for checking the pause on a compositor.
        eprintln!(
            "corgi-gui: window {}",
            if visible {
                "shown: drawing and reading the transcript again"
            } else {
                "hidden: drawing and the transcript paused"
            }
        );
        self.sync_watch();
        self.terminal
            .update(cx, |terminal, cx| terminal.set_visible(visible, cx));
        if visible {
            cx.notify();
        }
    }

    /// Gives the terminal the Herdr session `corgi serve` reads, from its
    /// hello: terminals are local only, so none over ssh.
    fn sync_terminal_socket(&mut self, cx: &mut Context<Self>) {
        let remote = self.options.ssh.is_some();
        let Some(hello) = self.store.hello.as_ref().filter(|_| !remote) else {
            if remote {
                self.terminal
                    .update(cx, |terminal, cx| terminal.set_socket(None, true, cx));
            }
            return;
        };
        let socket = Some(PathBuf::from(&hello.socket));
        self.terminal
            .update(cx, |terminal, cx| terminal.set_socket(socket, false, cx));
    }

    /// Whether to paint light, and whether the header's toggle chose it.
    fn appearance(&self) -> (bool, bool) {
        theme::appearance(self.forced_light, self.chosen_light, self.system_light)
    }

    fn color_terminal(&self, cx: &mut Context<Self>) {
        let (light, chosen) = self.appearance();
        let palette = theme::terminal_palette(&self.store.state.palette, light, chosen);
        let roles = &self.colors.roles;
        let (accent, muted, light) = (
            theme::rgb(roles.accent),
            theme::rgb(roles.muted),
            self.colors.light,
        );
        self.terminal.update(cx, |terminal, cx| {
            terminal.set_colors(&palette, accent, muted, light);
            cx.notify();
        });
    }

    fn recolor(&mut self, cx: &mut Context<Self>) {
        let (light, chosen) = self.appearance();
        let colors = Colors::new(&self.store.state.palette, light, chosen);
        if colors != self.colors {
            self.colors = colors;
            let field = field_colors(&self.colors);
            let filter = self.hotkeys.as_ref().map(|dialog| &dialog.filter);
            for input in [&self.prompt_input, &self.task_input]
                .into_iter()
                .chain(filter)
            {
                input.update(cx, |input, cx| {
                    input.colors = field;
                    cx.notify();
                });
            }
            self.color_terminal(cx);
            cx.notify();
        }
    }

    /// The header's sun or moon: paints the appearance other than the one
    /// showing, at once, and holds it over the system's until the window
    /// closes; it is not kept. `CORGI_GUI_APPEARANCE` gives way.
    pub fn toggle_appearance(&mut self, cx: &mut Context<Self>) {
        self.forced_light = None;
        self.chosen_light = Some(!self.colors.light);
        self.recolor(cx);
        cx.notify();
    }

    // ---- Keys ----

    /// Keeps the keys the hotkeys dialog changed, and gives the focus
    /// view's terminal its own two.
    pub fn keymap_changed(&mut self, cx: &mut Context<Self>) {
        self.keymap.save(self.keymap_path.as_deref());
        self.share_keymap(cx);
        cx.notify();
    }

    fn share_keymap(&mut self, cx: &mut Context<Self>) {
        let keymap = self.keymap.clone();
        self.terminal
            .update(cx, |terminal, _| terminal.keymap = keymap);
    }

    // ---- Layout ----

    pub fn save_layout(&self) {
        self.layout.save(self.layout_path.as_deref());
    }

    /// Keeps `window`'s size for the next run, as it closes.
    pub fn keep_window_size(&mut self, window: &Window) {
        let size = window.bounds().size;
        self.layout.window = Some((f32::from(size.width), f32::from(size.height)));
        self.save_layout();
    }

    /// The window's text a point `larger` or smaller, the terminal's with
    /// it, kept for the next run.
    pub fn step_font(&mut self, larger: bool, cx: &mut Context<Self>) {
        if !self.layout.step_font(larger) {
            return;
        }
        self.save_layout();
        let scale = self.layout.text_scale();
        self.terminal
            .update(cx, |terminal, cx| terminal.set_text_scale(scale, cx));
        cx.notify();
    }

    /// `size` pixels at the window's text size.
    pub fn scaled(&self, size: f32) -> Pixels {
        px(size * self.layout.text_scale())
    }

    /// Shows or hides the side panel: the focus view's own, or the
    /// overview's.
    pub fn toggle_side(&mut self, cx: &mut Context<Self>) {
        if self.focus_view.is_some() {
            self.layout.focus_side_open = !self.layout.focus_side_open;
        } else {
            self.layout.side_open = !self.layout.side_open;
        }
        self.save_layout();
        cx.notify();
    }

    /// Space: grows the selected row into its session's latest turns, down
    /// to the window's bottom, or back. With no row selected there is
    /// nothing to grow.
    pub fn toggle_expanded(&mut self, cx: &mut Context<Self>) {
        let Some(pane_id) = self.selected.clone() else {
            return;
        };
        self.expanded = !self.expanded;
        self.transcript.reset();
        self.rows.pick(&pane_id, Pick::Reveal);
        self.sync_watch();
        cx.notify();
    }

    /// Puts the expanded session's selected text on the clipboard; false
    /// when nothing is selected.
    fn copy_selection(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(text) = self.transcript.copied().filter(|_| self.expanded) else {
            return false;
        };
        cx.write_to_clipboard(ClipboardItem::new_string(text));
        true
    }

    /// Collapses `group`'s card, or expands it if collapsed: ← and →, a
    /// click on its top edge, a right click anywhere in its frame. The
    /// selection stays, unless collapsing hides it.
    pub fn toggle_card(&mut self, group: &str, cx: &mut Context<Self>) {
        let Some(project) = self.store.project(group) else {
            return;
        };
        let moved = flip_card(&mut self.collapsed, project, self.selected.as_deref());
        if let Some(supervisor) = moved {
            self.select(supervisor, Pick::Reveal, cx);
        }
        self.reconcile(cx);
        cx.notify();
    }

    pub fn drag_to(&mut self, position: gpui::Point<Pixels>, cx: &mut Context<Self>) {
        let Some(drag) = self.drag.as_mut() else {
            return;
        };
        let delta = f32::from(position.x - drag.last);
        drag.last = position.x;
        let split = drag.split;
        self.layout.drag(split, delta);
        cx.notify();
    }

    pub fn end_drag(&mut self) {
        if self.drag.take().is_some() {
            self.save_layout();
        }
    }

    // ---- Actions ----

    /// What a double click on an agent's row, or Enter on it, does: opens
    /// the focus view on the agent's live terminal.
    pub fn open_agent(&mut self, pane_id: String, window: &mut Window, cx: &mut Context<Self>) {
        let Some(agent) = self.store.agent(&pane_id).cloned() else {
            return;
        };
        // A double click selected the row with its first click, and Enter
        // acts on the selected row: neither moves the overview.
        if self.selected.as_deref() != Some(pane_id.as_str()) {
            self.select(pane_id.clone(), Pick::Reveal, cx);
        }
        self.focus_view = Some(pane_id);
        let target = Target {
            pane_id: agent.pane_id,
            tab_id: agent.tab_id,
            name: agent.herdr_name.unwrap_or(agent.name),
        };
        self.terminal
            .update(cx, |terminal, cx| terminal.open(target, cx));
        let focus = self.terminal.read(cx).focus.clone();
        window.focus(&focus, cx);
        self.sync_watch();
        cx.notify();
    }

    /// Back to the overview; the terminal's connection parks.
    pub fn leave_focus(&mut self, cx: &mut Context<Self>) {
        if self.focus_view.take().is_none() {
            return;
        }
        self.terminal.update(cx, |terminal, cx| terminal.leave(cx));
        self.focus_dialog(cx);
        self.sync_watch();
        cx.notify();
    }

    /// Opens the focus view's agent again, after its terminal was gone.
    pub fn reopen(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(pane_id) = self.focus_view.clone() {
            self.open_agent(pane_id, window, cx);
        }
    }

    /// Opens the prompt box for the selected agent.
    pub fn open_prompt(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(agent) = self
            .selected
            .as_deref()
            .and_then(|id| self.store.agent(id))
            .cloned()
        else {
            return;
        };
        if !agent.can_prompt {
            self.dialogs
                .say("Blocked agents must be handled in their own pane", true);
            cx.notify();
            return;
        }
        let name = agent.herdr_name.clone().unwrap_or(agent.name.clone());
        self.prompt_input.update(cx, |input, cx| {
            input.clear(cx);
            input.set_placeholder(format!("Prompt {name}"));
        });
        self.dialogs.open = Some(Dialog::Prompt {
            pane_id: agent.pane_id.clone(),
            agent: name,
            sending: false,
            error: None,
        });
        self.focus_dialog_now(window, cx);
        cx.notify();
    }

    pub fn open_answer(
        &mut self,
        project_root: String,
        question: Question,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let handle = if question.id.is_empty() {
            "the question".to_string()
        } else {
            question.id.clone()
        };
        self.prompt_input.update(cx, |input, cx| {
            input.clear(cx);
            input.set_placeholder(format!("Your answer to {handle}"));
        });
        self.question_scroll = ScrollHandle::new();
        self.dialogs.open = Some(Dialog::Answer {
            project_root,
            question,
            sending: false,
            error: None,
        });
        self.focus_dialog_now(window, cx);
        cx.notify();
    }

    /// Shows the backlog entry `entry` of `project_root`'s supervisor
    /// whole, with a field to write the supervisor about it.
    pub fn open_backlog(
        &mut self,
        project_root: String,
        entry: BacklogEntry,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.prompt_input.update(cx, |input, cx| {
            input.clear(cx);
            input.set_placeholder("Write the supervisor about this entry");
        });
        self.question_scroll = ScrollHandle::new();
        self.dialogs.open = Some(Dialog::Backlog {
            project_root,
            entry,
            sending: false,
            error: None,
        });
        self.focus_dialog_now(window, cx);
        cx.notify();
    }

    /// Closes `question` of `project_root` without answering it; the card
    /// shows it closing until the ledger drops it.
    pub fn close_question(
        &mut self,
        project_root: String,
        question: String,
        cx: &mut Context<Self>,
    ) {
        if self.closing.contains(&project_root, &question) {
            return;
        }
        let command = Command::CloseQuestion {
            project_root: project_root.clone(),
            question: question.clone(),
        };
        let purpose = Purpose::CloseQuestion {
            project_root: project_root.clone(),
            question: question.clone(),
        };
        if self.send(command, purpose) {
            self.closing.start(&project_root, &question);
        }
        cx.notify();
    }

    /// Answers the `index`th open question of the selected agent's project.
    pub fn answer_nth(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(project) = self
            .selected
            .as_deref()
            .and_then(|id| self.store.project_of(id))
        else {
            return;
        };
        let root = project.root.clone();
        match project.questions.get(index).cloned() {
            Some(question) => self.open_answer(root, question, window, cx),
            None => {
                self.dialogs.say("No open question with that number", true);
                cx.notify();
            }
        }
    }

    pub fn new_agent(&mut self, cx: &mut Context<Self>) {
        self.task_input.update(cx, |input, cx| input.clear(cx));
        let pane_id = self.selected.clone();
        let sent = self.send(
            Command::NewAgentForm {
                pane_id,
                project: None,
                harness: None,
                model: None,
            },
            Purpose::NewAgentForm,
        );
        // The form waits on the harnesses' model lists; say so at once.
        if sent {
            self.dialogs.open = Some(Dialog::NewAgentLoading);
            self.focus_dialog(cx);
        }
        cx.notify();
    }

    pub fn start_supervisor(&mut self, cx: &mut Context<Self>) {
        let Some(agent) = self.selected_agent() else {
            return;
        };
        if agent.scratch || agent.project_root.is_empty() {
            self.dialogs.say(
                "Scratch sessions belong to no project, so they have no supervisor",
                true,
            );
            cx.notify();
            return;
        }
        let project_root = agent.project_root.clone();
        self.layout.side_open = true;
        self.save_layout();
        self.send(
            Command::StartSupervisor { project_root },
            Purpose::StartSupervisor,
        );
        cx.notify();
    }

    pub fn merge_check(&mut self, cx: &mut Context<Self>) {
        let Some(agent) = self.selected_agent() else {
            return;
        };
        if !agent.can_merge {
            self.dialogs
                .say("Only an agent in its own Git worktree can be merged", true);
            cx.notify();
            return;
        }
        let pane_id = agent.pane_id.clone();
        self.send(Command::MergeCheck { pane_id }, Purpose::MergeCheck);
        cx.notify();
    }

    pub fn close_check(&mut self, pane_id: Option<String>, cx: &mut Context<Self>) {
        let Some(pane_id) = pane_id.or(self.selected.clone()) else {
            return;
        };
        self.send(Command::CloseCheck { pane_id }, Purpose::CloseCheck);
        cx.notify();
    }

    pub fn toggle_tag(&mut self, cx: &mut Context<Self>) {
        let Some(agent) = self.selected_agent() else {
            return;
        };
        let (pane_id, merge) = (agent.pane_id.clone(), !agent.ready_to_merge);
        self.send(Command::Tag { pane_id, merge }, Purpose::Tag);
        cx.notify();
    }

    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        self.send(Command::Refresh, Purpose::Quiet);
        cx.notify();
    }

    /// The header's "Find orphaned sessions" and its key: opens the dialog
    /// and asks `corgi serve` to look. Only now is anything looked for.
    pub fn find_orphans(&mut self, cx: &mut Context<Self>) {
        if self.dialogs.open.is_some() {
            return;
        }
        if self.send(Command::Orphans, Purpose::Orphans) {
            self.dialogs.open = Some(Dialog::Orphans(Box::new(Orphans::looking())));
            self.focus_dialog(cx);
        }
        cx.notify();
    }

    /// Scrolls the orphans' list just far enough to show card `index` in
    /// full, as the overview does its selected row (`rows::revealed`):
    /// GPUI's `scroll_to_item` leaves a tall card's foot under the edge.
    fn reveal_orphan(&self, index: usize) {
        let scroll = &self.orphans_scroll;
        let Some(card) = scroll.bounds_for_item(index) else {
            return;
        };
        let view = scroll.bounds();
        let offset = scroll.offset();
        let y = crate::rows::revealed(
            offset.y,
            (view.top(), view.bottom()),
            (card.top(), card.bottom()),
        );
        scroll.set_offset(gpui::point(offset.x, y));
    }

    /// Applies the choice made for the selected orphan.
    pub fn apply_orphan(&mut self, cx: &mut Context<Self>) {
        let Some(Dialog::Orphans(orphans)) = &mut self.dialogs.open else {
            return;
        };
        if !orphans.idle() {
            return;
        }
        let Some((candidate, action)) = orphans.choices.choice() else {
            return;
        };
        let key = candidate.key.clone();
        let command = Command::OrphanApply {
            key: key.clone(),
            fingerprint: candidate.fingerprint.clone(),
            action: action.clone(),
        };
        orphans.applying = Some(key.clone());
        orphans.choices.error = None;
        if !self.send(command, Purpose::OrphanApply { key })
            && let Some(Dialog::Orphans(orphans)) = &mut self.dialogs.open
        {
            orphans.applying = None;
        }
        cx.notify();
    }

    pub fn merge_action(&mut self, action: MergeAction, cx: &mut Context<Self>) {
        let Some(Dialog::Merge(merge)) = &mut self.dialogs.open else {
            return;
        };
        let pane_id = merge.form.pane_id.clone();
        match action {
            MergeAction::Run(upstream) => {
                merge.stage = MergeStage::Running;
                merge.progress.step = None;
                merge.progress.status = None;
                self.send(Command::Merge { pane_id, upstream }, Purpose::Merge);
            }
            MergeAction::Handoff => {
                if !merge.offers(MergeAction::Handoff) {
                    return;
                }
                let error = merge.handoff_error();
                self.send(Command::MergeHandoff { pane_id, error }, Purpose::MergeEnd);
            }
            MergeAction::Dismiss => {
                self.send(Command::MergeDismiss { pane_id }, Purpose::MergeEnd);
            }
            MergeAction::CloseAgent => {
                self.dialogs.open = None;
                self.close_check(Some(pane_id), cx);
            }
            MergeAction::Done => self.dialogs.open = None,
        }
        cx.notify();
    }

    pub fn confirm_close(&mut self, cx: &mut Context<Self>) {
        let Some(Dialog::Close(close)) = &mut self.dialogs.open else {
            return;
        };
        if close.running {
            return;
        }
        close.running = true;
        close.error = None;
        let command = Command::Close {
            pane_id: close.form.pane_id.clone(),
            force: close.dirty.is_some(),
        };
        self.send(command, Purpose::Close);
        cx.notify();
    }

    fn submit_text(&mut self, cx: &mut Context<Self>) {
        let text = self.prompt_input.read(cx).text().trim().to_string();
        let command = match &mut self.dialogs.open {
            Some(Dialog::Prompt {
                pane_id,
                sending,
                error,
                ..
            }) if !*sending => {
                if text.is_empty() {
                    *error = Some("Type a prompt first".into());
                    return;
                }
                *sending = true;
                *error = None;
                (
                    Command::Prompt {
                        pane_id: pane_id.clone(),
                        text,
                    },
                    Purpose::Prompt,
                )
            }
            Some(Dialog::Answer {
                project_root,
                question,
                sending,
                error,
            }) if !*sending => {
                if text.is_empty() {
                    *error = Some("Type an answer first".into());
                    return;
                }
                *sending = true;
                *error = None;
                (
                    Command::Answer {
                        project_root: project_root.clone(),
                        question: question.id.clone(),
                        text,
                    },
                    Purpose::Answer,
                )
            }
            Some(Dialog::Backlog {
                project_root,
                entry,
                sending,
                error,
            }) if !*sending => {
                if text.is_empty() {
                    *error = Some("Type a message first".into());
                    return;
                }
                *sending = true;
                *error = None;
                (
                    Command::BacklogMessage {
                        project_root: project_root.clone(),
                        heading: entry.heading.clone(),
                        text,
                    },
                    Purpose::BacklogMessage,
                )
            }
            _ => return,
        };
        if !self.send(command.0, command.1) {
            // Not sent: let the user try again.
            if let Some((sending, _)) = self.dialogs.open.as_mut().and_then(Dialog::text_state) {
                *sending = false;
            }
        }
        cx.notify();
    }

    fn submit_form(&mut self, cx: &mut Context<Self>) {
        let task = self.task_input.read(cx).text().to_string();
        let Some(Dialog::NewAgent(new)) = &mut self.dialogs.open else {
            return;
        };
        let spawn = new.spawn(&task);
        new.error = None;
        new.running = Some(Default::default());
        if !self.send(Command::Spawn(spawn), Purpose::Spawn)
            && let Some(Dialog::NewAgent(new)) = &mut self.dialogs.open
        {
            new.running = None;
        }
        cx.notify();
    }

    fn follow(&mut self, followup: Followup, cx: &mut Context<Self>) {
        match followup {
            Followup::ShowSupervisor(pane_id) => {
                self.layout.side_open = true;
                self.save_layout();
                if let Some(agent) = self.store.agent(&pane_id).cloned() {
                    // A card collapsed still shows its supervisor.
                    self.collapsed.remove(&agent.project_group);
                }
                self.select(pane_id, Pick::Reveal, cx);
            }
            Followup::Form(_) => {}
        }
    }

    // ---- Keys ----

    /// Keys while no dialog is open.
    pub fn overview_key(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let keystroke = &event.keystroke;
        // The focus view's copy keys copy the expanded session's selection.
        if keys::chord(keystroke, cfg!(target_os = "macos")) == Some(Chord::Copy) {
            if self.copy_selection(cx) {
                cx.stop_propagation();
            }
            return;
        }
        let selected_group = self
            .selected_agent()
            .map(|agent| agent.project_group.clone());
        let Some(hit) = self.keymap.overview_hit(keystroke, self.expanded) else {
            return;
        };
        let action = match hit {
            Hit::Answer(index) => {
                self.answer_nth(index, window, cx);
                cx.stop_propagation();
                return;
            }
            Hit::Action(action) => action,
        };
        match action {
            Action::Next => self.step(1, cx),
            Action::Previous => self.step(-1, cx),
            Action::First => self.step(-10_000, cx),
            Action::Last => self.step(10_000, cx),
            Action::Open => {
                if let Some(pane_id) = self.selected.clone() {
                    self.open_agent(pane_id, window, cx);
                }
            }
            Action::Expand | Action::Collapse => self.toggle_expanded(cx),
            Action::ScrollUp | Action::ScrollDown => {
                let pages = if action == Action::ScrollUp {
                    -1.0
                } else {
                    1.0
                };
                self.transcript.scroll_pages(pages);
                cx.notify();
            }
            Action::Newest | Action::Oldest => {
                self.transcript.scroll_to_end(action == Action::Oldest);
                cx.notify();
            }
            Action::SidePanel => self.toggle_side(cx),
            Action::CollapseCard | Action::ExpandCard => {
                let Some(group) = selected_group else { return };
                let collapse = action == Action::CollapseCard;
                if collapse != self.collapsed.contains(&group) {
                    self.toggle_card(&group, cx);
                }
            }
            Action::Prompt => self.open_prompt(window, cx),
            Action::NewAgent => self.new_agent(cx),
            Action::Merge => self.merge_check(cx),
            Action::Close => self.close_check(None, cx),
            Action::Supervisor => self.start_supervisor(cx),
            Action::Tag => self.toggle_tag(cx),
            Action::Refresh => self.refresh(cx),
            Action::Orphans => self.find_orphans(cx),
            Action::AnswerFirst => self.answer_nth(0, window, cx),
            // The focus view's and the merge dialog's own.
            Action::Leave | Action::FocusSide | Action::AskSupervisor => return,
        }
        cx.stop_propagation();
    }

    /// Keys in the focus view while its terminal does not have the keyboard
    /// (after a click in the side panel): with nothing typed into a program,
    /// Esc and `\` are free for the view. The terminal's own chords work too.
    pub fn focus_key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let terminal_focus = self.terminal.read(cx).focus.clone();
        if terminal_focus.is_focused(window) {
            return;
        }
        let keystroke = &event.keystroke;
        let plain = !keystroke.modifiers.modified();
        let side = KeymapKey::from_keystroke(keystroke) == *self.keymap.key(Action::SidePanel);
        match self.keymap.focus_hit(keystroke) {
            Some(Action::Leave) => self.leave_focus(cx),
            Some(_) => self.toggle_side(cx),
            None if plain && keystroke.key == "escape" => self.leave_focus(cx),
            None if side => self.toggle_side(cx),
            None if plain && keystroke.key == "enter" => window.focus(&terminal_focus, cx),
            None => return,
        }
        cx.stop_propagation();
    }

    /// Keys while a dialog is open; the text field inside has the rest.
    pub fn dialog_key(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let keystroke = &event.keystroke;
        // The merge dialog's rebindable key, which may have a modifier.
        if let Some(Dialog::Merge(merge)) = &self.dialogs.open
            && self.keymap.merge_dialog_hit(keystroke) == Some(Action::AskSupervisor)
        {
            if merge.offers(MergeAction::Handoff) {
                self.merge_action(MergeAction::Handoff, cx);
            }
            cx.stop_propagation();
            return;
        }
        let modifiers = keystroke.modifiers;
        if modifiers.platform || modifiers.control {
            return;
        }
        let key = keystroke.key.as_str();
        if key == "escape" {
            cx.stop_propagation();
            self.escape_dialog(window, cx);
            return;
        }
        let handled = match &mut self.dialogs.open {
            None => false,
            Some(Dialog::Prompt { .. } | Dialog::Answer { .. } | Dialog::Backlog { .. }) => {
                match key {
                    "enter" => {
                        self.submit_text(cx);
                        true
                    }
                    _ => false,
                }
            }
            Some(Dialog::Merge(merge)) => match key {
                "enter" => {
                    if let Some(action) = merge.default_action() {
                        self.merge_action(action, cx);
                    }
                    true
                }
                _ => false,
            },
            Some(Dialog::NewAgentLoading) => true,
            Some(Dialog::Close(_)) => match key {
                "enter" => {
                    self.confirm_close(cx);
                    true
                }
                _ => false,
            },
            Some(Dialog::Orphans(orphans)) => {
                if !orphans.idle() {
                    true
                } else {
                    let choices = &mut orphans.choices;
                    match key {
                        "enter" => {
                            self.apply_orphan(cx);
                            true
                        }
                        "j" | "down" | "k" | "up" => {
                            choices.select_by(if matches!(key, "j" | "down") { 1 } else { -1 });
                            let selected = choices.selected;
                            self.reveal_orphan(selected);
                            true
                        }
                        "right" | "tab" if !modifiers.shift => {
                            choices.cycle(1);
                            true
                        }
                        "left" | "tab" => {
                            choices.cycle(-1);
                            true
                        }
                        "s" | "w" | "x" | "l" => {
                            choices.choose_key(key.chars().next().unwrap_or_default());
                            true
                        }
                        _ => false,
                    }
                }
            }
            Some(Dialog::NewAgent(new)) => {
                let Some(form_key) = form_key(event) else {
                    return;
                };
                match new.key(form_key) {
                    form::Effect::Unhandled => false,
                    form::Effect::None => true,
                    form::Effect::FocusTask | form::Effect::BlurTask => true,
                    form::Effect::Submit => {
                        self.submit_form(cx);
                        true
                    }
                    form::Effect::Cancel => {
                        self.dialogs.open = None;
                        true
                    }
                    form::Effect::Reload { harness, model } => {
                        let project = (!new.new_project).then(|| new.project.clone());
                        self.send(
                            Command::NewAgentForm {
                                pane_id: None,
                                project,
                                harness: Some(harness),
                                model: Some(model).filter(|model| !model.is_empty()),
                            },
                            Purpose::NewAgentForm,
                        );
                        true
                    }
                }
            }
        };
        if handled {
            cx.stop_propagation();
            self.focus_dialog_now(window, cx);
            cx.notify();
        }
    }

    /// Esc in a dialog, or a click outside it: see `Dialogs::escape`.
    pub fn escape_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(action) = self.dialogs.escape() {
            self.merge_action(action, cx);
        }
        self.focus_dialog_now(window, cx);
        cx.notify();
    }

    /// Puts the keyboard where the open dialog wants it, at the next frame.
    fn focus_dialog(&mut self, cx: &mut Context<Self>) {
        self.refocus = true;
        cx.notify();
    }

    pub fn take_refocus(&mut self) -> bool {
        std::mem::take(&mut self.refocus)
    }

    pub fn focus_dialog_now(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let target = match &self.dialogs.open {
            _ if self.hotkeys.is_some() => match self.hotkeys_focus(cx) {
                Some(target) => target,
                None => return,
            },
            None if self.focus_view.is_some() => self.terminal.read(cx).focus.clone(),
            None => self.focus.clone(),
            Some(Dialog::Prompt { .. } | Dialog::Answer { .. } | Dialog::Backlog { .. }) => {
                self.prompt_input.read(cx).focus_handle(cx)
            }
            Some(Dialog::NewAgent(new)) if new.row == form::Row::Task && new.picker.is_none() => {
                self.task_input.read(cx).focus_handle(cx)
            }
            Some(_) => self.form_focus.clone(),
        };
        if !target.is_focused(window) {
            window.focus(&target, cx);
        }
    }

    pub fn connection(&self) -> &Connection {
        &self.store.connection
    }
}

/// A key as the new-agent form reads it.
fn form_key(event: &KeyDownEvent) -> Option<Key> {
    let keystroke = &event.keystroke;
    let shift = keystroke.modifiers.shift;
    Some(match keystroke.key.as_str() {
        "up" => Key::Up,
        "down" => Key::Down,
        "tab" if shift => Key::BackTab,
        "tab" => Key::Tab,
        "left" => Key::Left,
        "right" => Key::Right,
        "space" => Key::Space,
        "backspace" => Key::Backspace,
        "enter" => Key::Enter,
        "escape" => Key::Escape,
        _ => {
            let typed = keystroke.key_char.as_deref()?;
            let mut chars = typed.chars();
            let c = chars.next()?;
            if chars.next().is_some() || c.is_control() {
                return None;
            }
            Key::Char(c)
        }
    })
}

pub fn field_colors(colors: &Colors) -> FieldColors {
    FieldColors {
        text: colors.roles.text,
        placeholder: colors.roles.muted,
        cursor: colors.roles.accent,
        selection: colors.roles.accent.opacity(0.3),
    }
}

/// Binds the window's fixed keys (`keymap::fixed`): quitting, and on macOS
/// closing, minimizing and full screen.
pub fn bind_app_keys(cx: &mut App) {
    let quit = if cfg!(target_os = "macos") {
        keymap::QUIT_MAC
    } else {
        keymap::QUIT
    };
    cx.bind_keys([gpui::KeyBinding::new(quit, Quit, None)]);
    cx.on_action(|_: &Quit, cx| cx.quit());
    // macOS draws no title bar buttons (`main.rs`), so their usual keys do
    // what they did.
    if cfg!(target_os = "macos") {
        cx.bind_keys([
            gpui::KeyBinding::new(keymap::CLOSE_WINDOW, CloseWindow, None),
            gpui::KeyBinding::new(keymap::MINIMIZE, Minimize, None),
            gpui::KeyBinding::new(keymap::FULL_SCREEN, ToggleFullScreen, None),
        ]);
        cx.on_action(|_: &CloseWindow, cx| {
            on_active_window(cx, |view, window| {
                view.keep_window_size(window);
                window.remove_window();
            });
        });
        cx.on_action(|_: &Minimize, cx| on_active_window(cx, |_, window| window.minimize_window()));
        cx.on_action(|_: &ToggleFullScreen, cx| {
            on_active_window(cx, |_, window| window.toggle_fullscreen());
        });
    }
}

gpui::actions!(corgi, [Quit, CloseWindow, Minimize, ToggleFullScreen]);

/// Runs `act` on the window in front, once the key that asked for it is
/// handled: the window cannot be updated while its key is dispatched.
fn on_active_window(cx: &mut App, act: impl FnOnce(&mut CorgiWindow, &mut Window) + 'static) {
    let Some(handle) = cx
        .active_window()
        .and_then(|handle| handle.downcast::<CorgiWindow>())
    else {
        return;
    };
    cx.defer(move |cx| {
        let _ = handle.update(cx, |view, window, _| act(view, window));
    });
}

/// Collapses each card of `projects` not yet `seen`, so the window opens,
/// and a project new to it appears, on its supervisor; a card already seen
/// keeps what the user made of it. Returns the row to select when a
/// collapse hides `selected`: its project's supervisor.
fn collapse_new(
    collapsed: &mut HashSet<String>,
    seen: &mut HashSet<String>,
    projects: &[Project],
    selected: Option<&str>,
) -> Option<String> {
    let mut moved = None;
    for project in projects.iter().filter(|project| project.card) {
        if seen.insert(project.group.clone()) {
            collapsed.insert(project.group.clone());
            moved = moved.or_else(|| hidden_by(project, selected));
        }
    }
    moved
}

/// The supervisor to select when collapsing `project` hides `selected`, one
/// of its workers.
fn hidden_by(project: &Project, selected: Option<&str>) -> Option<String> {
    let hidden = selected.is_some_and(|selected| {
        project.supervisor.as_deref() != Some(selected)
            && project.agents.iter().any(|id| id == selected)
    });
    if hidden {
        project.supervisor.clone()
    } else {
        None
    }
}

/// The selection once the rows changed to `visible`: kept while its row
/// shows; else the first row, once the user has `picked` one; before
/// that, none.
fn reconciled(selected: Option<String>, picked: bool, visible: &[String]) -> Option<String> {
    match selected {
        Some(selected) if visible.contains(&selected) => Some(selected),
        _ if picked => visible.first().cloned(),
        _ => None,
    }
}

/// Collapses `project`'s card in `collapsed`, or expands it if collapsed; a
/// project with no card has nothing to collapse. Returns the row to select
/// when collapsing hides `selected`, one of its workers: its supervisor,
/// which a collapsed card still shows.
fn flip_card(
    collapsed: &mut HashSet<String>,
    project: &Project,
    selected: Option<&str>,
) -> Option<String> {
    if !project.card || collapsed.remove(&project.group) {
        return None;
    }
    collapsed.insert(project.group.clone());
    hidden_by(project, selected)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn card(group: &str) -> Project {
        Project {
            group: group.into(),
            card: true,
            supervisor: Some(format!("{group}-sup")),
            agents: vec![format!("{group}-sup"), format!("{group}-worker")],
            ..Project::default()
        }
    }

    #[test]
    fn a_card_flips_and_keeps_the_selection_while_it_shows() {
        let mut collapsed = HashSet::new();
        let shop = card("shop");
        // Another project's row, or the supervisor, stays selected.
        assert_eq!(flip_card(&mut collapsed, &shop, Some("other")), None);
        assert!(collapsed.contains("shop"));
        assert_eq!(flip_card(&mut collapsed, &shop, Some("other")), None);
        assert!(collapsed.is_empty(), "a second flip expands it");
        assert_eq!(flip_card(&mut collapsed, &shop, Some("shop-sup")), None);
        assert_eq!(flip_card(&mut collapsed, &shop, None), None);
        assert!(collapsed.is_empty());
    }

    #[test]
    fn collapsing_a_card_over_its_selected_worker_selects_its_supervisor() {
        let mut collapsed = HashSet::new();
        let shop = card("shop");
        assert_eq!(
            flip_card(&mut collapsed, &shop, Some("shop-worker")),
            Some("shop-sup".to_string())
        );
        // Expanding hides nothing.
        assert_eq!(flip_card(&mut collapsed, &shop, Some("shop-sup")), None);
    }

    #[test]
    fn the_window_starts_with_every_card_collapsed() {
        let scratch = Project {
            card: false,
            ..card("scratch")
        };
        let (mut collapsed, mut seen) = (HashSet::new(), HashSet::new());
        let projects = [card("shop"), scratch, card("blog")];
        assert_eq!(
            collapse_new(&mut collapsed, &mut seen, &projects, None),
            None
        );
        assert_eq!(
            collapsed,
            HashSet::from(["shop".to_string(), "blog".to_string()])
        );
        // Collapsed, a card shows its supervisor; expanding it hides nothing.
        assert_eq!(flip_card(&mut collapsed, &card("shop"), None), None);
        assert!(!collapsed.contains("shop"));
    }

    #[test]
    fn a_new_card_starts_collapsed_and_an_expanded_one_stays_until_a_reconnect() {
        let (mut collapsed, mut seen) = (HashSet::new(), HashSet::new());
        let shop = card("shop");
        collapse_new(&mut collapsed, &mut seen, std::slice::from_ref(&shop), None);
        // The user expands shop and selects its worker.
        flip_card(&mut collapsed, &shop, None);
        let selected = Some("shop-worker");
        // Refreshes keep it expanded; a project new to the window collapses,
        // and the selection, not in it, stays.
        let both = [shop.clone(), card("blog")];
        assert_eq!(
            collapse_new(&mut collapsed, &mut seen, &both, selected),
            None
        );
        assert_eq!(collapsed, HashSet::from(["blog".to_string()]));
        assert_eq!(
            collapse_new(&mut collapsed, &mut seen, &both, selected),
            None
        );
        assert!(!collapsed.contains("shop"));
        // A reconnect forgets what was seen: every card collapses again, and
        // the hidden worker's selection moves to its supervisor.
        seen.clear();
        assert_eq!(
            collapse_new(&mut collapsed, &mut seen, &both, selected),
            Some("shop-sup".to_string())
        );
        assert_eq!(
            collapsed,
            HashSet::from(["shop".to_string(), "blog".to_string()])
        );
    }

    #[test]
    fn nothing_is_selected_until_a_row_is_picked_then_a_gone_row_gives_way() {
        let rows = |ids: &[&str]| ids.iter().map(|id| id.to_string()).collect::<Vec<_>>();
        // At start, and as snapshots come, no row is picked for the user.
        assert_eq!(reconciled(None, false, &rows(&["a", "b"])), None);
        assert_eq!(reconciled(None, false, &[]), None);
        // A picked row stays while it shows.
        let b = Some("b".to_string());
        assert_eq!(reconciled(b.clone(), true, &rows(&["a", "b"])), b);
        // Once picked, a row that goes gives way to the first, as before.
        assert_eq!(
            reconciled(b.clone(), true, &rows(&["a", "c"])),
            Some("a".to_string())
        );
        assert_eq!(reconciled(b, true, &[]), None);
        assert_eq!(reconciled(None, true, &rows(&["c"])), Some("c".to_string()));
    }

    #[test]
    fn space_expands_and_the_sessions_keys_work_only_while_expanded() {
        let keymap = Keymap::defaults(true);
        let hit = |key: &str, expanded| {
            keymap.overview_hit(&gpui::Keystroke::parse(key).unwrap(), expanded)
        };
        let action = |action| Some(Hit::Action(action));
        assert_eq!(hit("space", false), action(Action::Expand));
        assert_eq!(hit("space", true), action(Action::Expand));
        assert_eq!(hit("escape", true), action(Action::Collapse));
        for (key, scroll) in [
            ("u", Action::ScrollUp),
            ("pageup", Action::ScrollUp),
            ("d", Action::ScrollDown),
            ("pagedown", Action::ScrollDown),
        ] {
            assert_eq!(hit(key, true), action(scroll), "{key}");
        }
        assert_eq!(hit("home", true), action(Action::Newest));
        assert_eq!(hit("end", true), action(Action::Oldest));
        // Collapsed, they are the overview's: Home and End select rows, and
        // Esc, u and d do nothing there.
        assert_eq!(hit("home", false), action(Action::First));
        assert_eq!(hit("end", false), action(Action::Last));
        for key in ["escape", "u", "d", "pageup", "pagedown"] {
            assert_eq!(hit(key, false), None, "{key}");
        }
        // j and k move the selection, and the expansion with it, either way.
        assert_eq!(hit("j", true), action(Action::Next));
        assert_eq!(hit("down", true), action(Action::Next));
        assert_eq!(hit("enter", true), action(Action::Open));
        // The view's modifiers reach no overview key.
        assert_eq!(hit("cmd-j", false), None);
    }

    #[test]
    fn the_overviews_copy_keys_are_the_focus_views() {
        let key = |text: &str| gpui::Keystroke::parse(text).unwrap();
        assert_eq!(keys::chord(&key("cmd-c"), true), Some(Chord::Copy));
        assert_eq!(keys::chord(&key("ctrl-shift-c"), false), Some(Chord::Copy));
        // Plain c, and Ctrl+C on Linux, are not copy.
        assert_eq!(keys::chord(&key("c"), true), None);
        assert_eq!(keys::chord(&key("ctrl-c"), false), None);
    }

    #[test]
    fn a_project_with_no_card_does_not_collapse() {
        let mut collapsed = HashSet::new();
        let scratch = Project {
            card: false,
            ..card("scratch")
        };
        assert_eq!(
            flip_card(&mut collapsed, &scratch, Some("scratch-worker")),
            None
        );
        assert!(collapsed.is_empty());
    }
}
