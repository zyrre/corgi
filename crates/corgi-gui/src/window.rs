//! The window: its connection to `corgi serve`, the selection, the keys and
//! the actions, and the focus view around an agent's terminal
//! (`crate::terminal`). What it draws is in `view.rs`.

use std::{
    collections::HashSet,
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};

use corgi_core::protocol::{
    Agent, BacklogEntry, Command, Project, Question, Reply, Transcript as Turns,
};
use futures::{StreamExt, channel::mpsc};
use gpui::{
    App, AppContext, ClipboardItem, Context, Entity, FocusHandle, Focusable, Font, KeyDownEvent,
    Pixels, ScrollHandle, SharedString, Subscription, Task, Window, WindowVisibility, px,
};

use crate::{
    dialogs::{Dialog, Dialogs, Followup, MachineMenu, MergeAction, MergeStage, Orphans},
    form::{self, Key},
    hotkeys::HotkeysDialog,
    keymap::{self, Action, Hit, Key as KeymapKey, Keymap},
    layout::{Layout, Split},
    link::{Link, LinkEvent},
    machines::{self, FormEffect, Machine, SavedMachines},
    options::Options,
    rows::{Pick, Rows},
    store::{Connection, Purpose, Received, Store},
    terminal::{
        Target, TerminalEvent, TerminalView,
        keys::{self, Chord},
    },
    text_input::{FieldColors, TextInput},
    theme::{self, Colors, forced_light, is_light},
    transcript::Transcript,
};

/// How long Space holds a row back while its turns are on their way: a
/// server of this version sends them in a few milliseconds (over `ssh`,
/// a round trip more), an older one after its next refresh, which takes
/// 0.1 to 0.3 s. Past it the row grows with its activity lines and
/// "Reading the session…" until they come.
const EXPANSION_WAIT: Duration = Duration::from_millis(300);

/// A Space whose row waits for its turns before it grows, so that it grows
/// once, with them, rather than first with a placeholder and then again.
#[derive(Debug, Clone)]
pub struct Expanding {
    pane_id: String,
    until: Instant,
}

/// Whether `expanding` still holds back `selected`'s row: until the
/// state's `transcript` is that row's, and at most until its time is up.
fn held_back(
    expanding: Option<&Expanding>,
    selected: Option<&str>,
    transcript: Option<&Turns>,
    now: Instant,
) -> bool {
    expanding.is_some_and(|expanding| {
        selected == Some(expanding.pane_id.as_str())
            && now < expanding.until
            && transcript.is_none_or(|transcript| transcript.pane_id != expanding.pane_id)
    })
}

/// A split being dragged, and where the pointer was last.
#[derive(Debug, Clone, Copy)]
pub struct Drag {
    pub split: Split,
    pub last: Pixels,
}

pub struct CorgiWindow {
    /// This machine and the ones added (`machines.rs`), each with its own
    /// connection to `corgi serve`, all kept open.
    pub machines: Vec<Machine>,
    /// The machine the window shows and every action goes to.
    pub active: usize,
    pub(crate) machines_path: Option<PathBuf>,
    pub dialogs: Dialogs,
    pub layout: Layout,
    layout_path: Option<PathBuf>,
    /// Connections started, for numbering the next.
    links_started: u64,
    events: mpsc::UnboundedSender<(u64, LinkEvent)>,
    /// The expanded session's scroll, its text selection and its lines as
    /// last drawn.
    pub transcript: Transcript,
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
    /// The window's title as last set: the machine shown.
    title: String,
    /// Which key does what (`keymap.rs`), and where it is kept.
    pub keymap: Keymap,
    keymap_path: Option<PathBuf>,
    /// The hotkeys dialog, while it is open (`hotkeys.rs`).
    pub hotkeys: Option<HotkeysDialog>,
    /// A Space waiting for its row's turns.
    expanding: Option<Expanding>,
    _tasks: Vec<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

impl CorgiWindow {
    pub fn new(
        options: &Options,
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
        let machines_path = SavedMachines::path();
        let (machines, active) =
            machines::machines_of_run(options, &SavedMachines::load(machines_path.as_deref()));
        let (light, chosen) = theme::appearance(forced_light, None, system_light);
        let colors = Colors::new(&machines[active].store.state.palette, light, chosen);
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
                    let now = this.store().now();
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
            machines,
            active,
            machines_path,
            dialogs: Dialogs::default(),
            layout,
            layout_path,
            links_started: 0,
            events,
            transcript: Transcript::default(),
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
            title: String::new(),
            keymap,
            keymap_path,
            hotkeys: None,
            expanding: None,
            _tasks: vec![pump, tick],
            _subscriptions: subscriptions,
        };
        this.sync_terminal_socket(cx);
        this.color_terminal(cx);
        this.share_keymap(cx);
        let scale = this.layout.text_scale();
        this.terminal
            .update(cx, |terminal, cx| terminal.set_text_scale(scale, cx));
        for index in 0..this.machines.len() {
            this.connect(index, cx);
        }
        window.focus(&this.focus, cx);
        this
    }

    // ---- The connection ----

    /// The machine shown.
    pub fn machine(&self) -> &Machine {
        &self.machines[self.active]
    }

    pub fn machine_mut(&mut self) -> &mut Machine {
        &mut self.machines[self.active]
    }

    /// The state the shown machine's `corgi serve` sent.
    pub fn store(&self) -> &Store {
        &self.machine().store
    }

    /// Starts machine `index`'s `corgi serve`.
    pub fn connect(&mut self, index: usize, cx: &mut Context<Self>) {
        self.links_started += 1;
        let number = self.links_started;
        let machine = &mut self.machines[index];
        machine.link_number = number;
        machine.store.connecting();
        let (program, args) = machine.options.command();
        let events = self.events.clone();
        let sender = Box::new(move |number, event| {
            let _ = events.unbounded_send((number, event));
        });
        match Link::start(number, &program, &args, sender) {
            Ok(link) => machine.link = Some(Arc::new(link)),
            Err(error) => {
                let _ = self
                    .events
                    .unbounded_send((number, LinkEvent::Ended(error.to_string())));
            }
        }
        cx.notify();
    }

    /// The machine whose latest connection is `number`.
    fn machine_of_link(&self, number: u64) -> Option<usize> {
        self.machines
            .iter()
            .position(|machine| machine.link_number == number)
    }

    /// Handles one event of connection `number`; true when the window
    /// should draw again. A machine not shown keeps its state and its dot;
    /// what it replies is only said.
    fn link_event(&mut self, number: u64, event: LinkEvent, cx: &mut Context<Self>) -> bool {
        let Some(index) = self.machine_of_link(number) else {
            return false;
        };
        let shown = index == self.active;
        match event {
            LinkEvent::Message(message) => {
                let hello = matches!(*message, corgi_core::protocol::ServerMessage::Hello(_));
                let machine = &mut self.machines[index];
                if hello {
                    machine.backoff.reset();
                }
                let received = machine.store.receive(*message);
                crate::latency::expansion(|| {
                    let transcript = machine.store.state.transcript.as_ref();
                    format!(
                        "message: {}, transcript {:?}",
                        match &received {
                            Some(Received::State) => "state",
                            Some(Received::Reply(_)) => "reply",
                            Some(_) => "other",
                            None => "none",
                        },
                        transcript.map(|t| (&t.pane_id, t.entries.len()))
                    )
                });
                if hello && shown {
                    self.sync_terminal_socket(cx);
                }
                match received {
                    None => false,
                    Some(Received::State) => {
                        if shown && self.expanding.is_some() && !self.expansion_held() {
                            self.expanding = None;
                        }
                        let machine = &mut self.machines[index];
                        let projects = &machine.store.state.projects;
                        let selected = machine.selected.as_deref();
                        if let Some(supervisor) = collapse_new(
                            &mut machine.collapsed,
                            &mut machine.seen,
                            projects,
                            selected,
                        ) {
                            machine.selected = Some(supervisor);
                        }
                        if shown {
                            self.reconcile(cx);
                        } else {
                            let machine = &mut self.machines[index];
                            machine.closing.prune(&machine.store.state);
                        }
                        // The bar counts every machine's agents.
                        true
                    }
                    Some(Received::Resync) => {
                        self.send_to(index, Command::Resync, Purpose::Quiet);
                        false
                    }
                    Some(Received::Progress(progress)) => {
                        if !shown {
                            return false;
                        }
                        if let Some(purpose) = self.machine().outbox.purpose(progress.id).cloned() {
                            self.dialogs.progress(&purpose, &progress);
                        }
                        true
                    }
                    Some(Received::Reply(reply)) => {
                        let machine = &mut self.machines[index];
                        let Some(purpose) = machine.outbox.answered(&reply) else {
                            return false;
                        };
                        machine.closing.replied(&purpose, &reply);
                        if !shown {
                            self.background_reply(index, purpose, reply);
                            return true;
                        }
                        if let Some(followup) = self.dialogs.reply(purpose, reply) {
                            self.follow(followup, cx);
                        }
                        self.focus_dialog(cx);
                        true
                    }
                }
            }
            LinkEvent::Ended(why) => {
                let machine = &mut self.machines[index];
                machine.link = None;
                // The next connection's state is fresh: every card collapses.
                machine.seen.clear();
                machine.watching = None;
                let delay = machine.backoff.next_delay();
                machine.store.lost(why.clone(), delay);
                let unanswered = machine.outbox.lost();
                machine.closing.clear();
                if shown {
                    self.dialogs.lost(&unanswered);
                } else {
                    let machine = &mut self.machines[index];
                    if unanswered
                        .iter()
                        .any(|p| matches!(p, Purpose::PauseAll | Purpose::ResumeAll))
                    {
                        machine.all_running = None;
                    }
                    // Said as it goes, or first fails, not at each retry.
                    if machine.backoff.failures() == 1 {
                        let line = why.lines().last().unwrap_or_default();
                        let name = machine.name.clone();
                        self.dialogs.say(format!("{name}: {line}"), true);
                    }
                }
                let retry = cx.spawn(async move |this, cx| {
                    cx.background_executor().timer(delay).await;
                    let _ = this.update(cx, |this, cx| {
                        if let Some(index) = this.machine_of_link(number)
                            && this.machines[index].link.is_none()
                        {
                            this.connect(index, cx);
                        }
                    });
                });
                self._tasks.push(retry);
                self._tasks.retain(|task| !task.is_ready());
                true
            }
        }
    }

    /// A reply from a machine not shown: its dialog, if any, is gone with
    /// the switch, so only what it says is said, with the machine's name.
    fn background_reply(&mut self, index: usize, purpose: Purpose, reply: Reply) {
        let machine = &mut self.machines[index];
        if matches!(purpose, Purpose::PauseAll | Purpose::ResumeAll) {
            machine.all_running = None;
        }
        let quiet = purpose == Purpose::Quiet && reply.ok;
        let mut aside = Dialogs::default();
        aside.reply(purpose, reply);
        if let Some(flash) = aside.flash.filter(|_| !quiet) {
            let name = machine.name.clone();
            self.dialogs
                .say(format!("{name}: {}", flash.text), flash.error);
        }
    }

    /// Connects the shown machine again now rather than when the wait ends.
    pub fn retry_now(&mut self, cx: &mut Context<Self>) {
        if self.machine().link.is_none() {
            self.connect(self.active, cx);
        }
    }

    /// Sends `command` to the shown machine; false when it could not be.
    pub fn send(&mut self, command: Command, purpose: Purpose) -> bool {
        self.send_to(self.active, command, purpose)
    }

    /// Sends `command` to machine `index`.
    fn send_to(&mut self, index: usize, command: Command, purpose: Purpose) -> bool {
        let machine = &mut self.machines[index];
        let Some(link) = machine.link.clone().filter(|_| machine.store.connected()) else {
            if purpose != Purpose::Quiet {
                self.dialogs.say(
                    "Not connected to corgi serve; try again once it reconnects",
                    true,
                );
            }
            return false;
        };
        let Some(request) = machine.outbox.request(&machine.store, command, purpose) else {
            self.dialogs.say(
                "That command would move every Herdr client's view, so the window never sends it",
                true,
            );
            return false;
        };
        if let Err(error) = link.send(&request) {
            machine.outbox.unsent(request.id);
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
        let state = &self.store().state;
        let mut agents = Vec::new();
        for project in &state.projects {
            let collapsed = project.card && self.machine().collapsed.contains(&project.group);
            for pane_id in &project.agents {
                if collapsed && project.supervisor.as_deref() != Some(pane_id.as_str()) {
                    continue;
                }
                if let Some(agent) = self.store().agent(pane_id) {
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
        !self.store().connected()
            || self.store().state.agents.iter().any(|agent| {
                agent
                    .cache
                    .as_ref()
                    .is_some_and(|cache| cache.expires_at > now && cache.expires_at - now <= 60)
            })
    }

    pub fn selected_agent(&self) -> Option<&Agent> {
        self.store().agent(self.machine().selected.as_deref()?)
    }

    /// Keeps the selection on a row that exists, and the transcript read
    /// for it.
    fn reconcile(&mut self, cx: &mut Context<Self>) {
        let visible: Vec<String> = self
            .visible_agents()
            .iter()
            .map(|agent| agent.pane_id.clone())
            .collect();
        let machine = self.machine_mut();
        let before = machine.selected.clone();
        machine.selected = reconciled(machine.selected.take(), machine.picked, &visible);
        let changed = machine.selected != before;
        machine.expanded &= machine.selected.is_some();
        machine.closing.prune(&machine.store.state);
        if changed {
            // Another session's turns start at its newest.
            self.transcript.reset();
        }
        self.recolor(cx);
        self.sync_watch();
    }

    /// Selects `pane_id`'s row. A click on it leaves the overview where it
    /// is, so the row stays under the mouse; any other `pick` scrolls just
    /// enough to show the row in full.
    pub fn select(&mut self, pane_id: String, pick: Pick, cx: &mut Context<Self>) {
        self.rows.pick(&pane_id, pick);
        if self.machine().selected.as_deref() != Some(pane_id.as_str()) {
            self.transcript.reset();
            self.expanding = None;
        }
        let machine = self.machine_mut();
        machine.selected = Some(pane_id);
        machine.picked = true;
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
            .machine()
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
    pub(crate) fn sync_watch(&mut self) {
        let index = self.active;
        self.sync_watch_of(index);
    }

    /// Machine `index`'s watch: only the shown machine's expanded row is
    /// read.
    pub(crate) fn sync_watch_of(&mut self, index: usize) {
        let reading = index == self.active && self.visible && self.focus_view.is_none();
        let machine = &mut self.machines[index];
        if !machine.store.connected() {
            return;
        }
        let wanted = machine
            .selected
            .clone()
            .filter(|_| reading && machine.expanded);
        if wanted != machine.watching {
            machine.watching = wanted.clone();
            self.send_to(index, Command::Watch { pane_id: wanted }, Purpose::Quiet);
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
    /// hello, and the host it runs on over ssh, whose Herdr the terminal
    /// reaches through a tunnel.
    pub(crate) fn sync_terminal_socket(&mut self, cx: &mut Context<Self>) {
        let host = self.machine().options.ssh.clone();
        let socket = self
            .store()
            .hello
            .as_ref()
            .map(|hello| PathBuf::from(&hello.socket));
        self.terminal
            .update(cx, |terminal, cx| terminal.set_socket(socket, host, cx));
    }

    /// Whether to paint light, and whether the header's toggle chose it.
    fn appearance(&self) -> (bool, bool) {
        theme::appearance(self.forced_light, self.chosen_light, self.system_light)
    }

    fn color_terminal(&self, cx: &mut Context<Self>) {
        let (light, chosen) = self.appearance();
        let palette = theme::terminal_palette(&self.store().state.palette, light, chosen);
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

    pub(crate) fn recolor(&mut self, cx: &mut Context<Self>) {
        let (light, chosen) = self.appearance();
        let colors = Colors::new(&self.store().state.palette, light, chosen);
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
        let Some(pane_id) = self.machine().selected.clone() else {
            return;
        };
        let machine = self.machine_mut();
        machine.expanded = !machine.expanded;
        let expanded = machine.expanded;
        let has_turns = machine
            .store
            .state
            .transcript
            .as_ref()
            .is_some_and(|transcript| transcript.pane_id == pane_id);
        self.expanding = None;
        if expanded {
            crate::latency::expanded();
        }
        if expanded && !has_turns {
            self.expanding = Some(Expanding {
                pane_id: pane_id.clone(),
                until: Instant::now() + EXPANSION_WAIT,
            });
            let wait = cx.spawn(async move |this, cx| {
                cx.background_executor().timer(EXPANSION_WAIT).await;
                let _ = this.update(cx, |this, cx| {
                    let due = this
                        .expanding
                        .as_ref()
                        .is_some_and(|expanding| expanding.until <= Instant::now());
                    if due {
                        this.expanding = None;
                        cx.notify();
                    }
                });
            });
            self._tasks.retain(|task| !task.is_ready());
            self._tasks.push(wait);
        }
        self.transcript.reset();
        self.rows.pick(&pane_id, Pick::Reveal);
        self.sync_watch();
        // A row held back looks as it did: the frame that grows it comes
        // with its turns, and drawing one now would only keep the window
        // busy when they arrive.
        if !self.expansion_held() {
            cx.notify();
        }
    }

    /// Whether the selected row waits for its turns before it grows.
    pub(crate) fn expansion_held(&self) -> bool {
        let machine = self.machine();
        held_back(
            self.expanding.as_ref(),
            machine.selected.as_deref(),
            machine.store.state.transcript.as_ref(),
            Instant::now(),
        )
    }

    /// Puts the expanded session's selected text on the clipboard; false
    /// when nothing is selected.
    fn copy_selection(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(text) = self.transcript.copied().filter(|_| self.machine().expanded) else {
            return false;
        };
        cx.write_to_clipboard(ClipboardItem::new_string(text));
        true
    }

    /// Collapses `group`'s card, or expands it if collapsed: ← and →, a
    /// click on its top edge, a right click anywhere in its frame. The
    /// selection stays, unless collapsing hides it.
    pub fn toggle_card(&mut self, group: &str, cx: &mut Context<Self>) {
        let machine = self.machine_mut();
        let Some(project) = machine.store.project(group) else {
            return;
        };
        let moved = flip_card(&mut machine.collapsed, project, machine.selected.as_deref());
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
        let Some(agent) = self.store().agent(&pane_id).cloned() else {
            return;
        };
        // A double click selected the row with its first click, and Enter
        // acts on the selected row: neither moves the overview.
        if self.machine().selected.as_deref() != Some(pane_id.as_str()) {
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
            .machine()
            .selected
            .as_deref()
            .and_then(|id| self.store().agent(id))
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
        if self.machine().closing.contains(&project_root, &question) {
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
            self.machine_mut().closing.start(&project_root, &question);
        }
        cx.notify();
    }

    /// Answers the `index`th open question of the selected agent's project.
    pub fn answer_nth(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(project) = self
            .machine()
            .selected
            .as_deref()
            .and_then(|id| self.store().project_of(id))
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
        let pane_id = self.machine().selected.clone();
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
        let Some(pane_id) = pane_id.or(self.machine().selected.clone()) else {
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

    /// Starts the selected agent again when it has exited to its shell:
    /// the same session, in its pane, with the options Corgi launched it
    /// with.
    pub fn resume(&mut self, cx: &mut Context<Self>) {
        let Some(agent) = self.selected_agent() else {
            return;
        };
        if !agent.exited {
            self.dialogs.say(
                "Resume starts an agent that has exited to its shell; this one runs",
                true,
            );
            cx.notify();
            return;
        }
        let pane_id = agent.pane_id.clone();
        self.dialogs.say(format!("Resuming {}…", agent.name), false);
        self.send(Command::Resume { pane_id }, Purpose::Resume);
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
                if let Some(agent) = self.store().agent(&pane_id).cloned() {
                    // A card collapsed still shows its supervisor.
                    self.machine_mut().collapsed.remove(&agent.project_group);
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
        let Some(hit) = self.keymap.overview_hit(keystroke, self.machine().expanded) else {
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
                if let Some(pane_id) = self.machine().selected.clone() {
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
                if collapse != self.machine().collapsed.contains(&group) {
                    self.toggle_card(&group, cx);
                }
            }
            Action::Prompt => self.open_prompt(window, cx),
            Action::NewAgent => self.new_agent(cx),
            Action::Merge => self.merge_check(cx),
            Action::Close => self.close_check(None, cx),
            Action::Supervisor => self.start_supervisor(cx),
            Action::Tag => self.toggle_tag(cx),
            Action::Resume => self.resume(cx),
            Action::Refresh => self.refresh(cx),
            Action::Orphans => self.find_orphans(cx),
            Action::PauseAll => self.pause_or_resume_all(cx),
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
        // The machine form's rows take a paste, its first line.
        if let Some(Dialog::Machine(form)) = &mut self.dialogs.open
            && keys::chord(keystroke, cfg!(target_os = "macos")) == Some(Chord::Paste)
        {
            if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
                form.type_text(text.lines().next().unwrap_or_default());
            }
            cx.stop_propagation();
            cx.notify();
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
            Some(Dialog::Pause(_)) => match key {
                "enter" => {
                    self.confirm_pause(cx);
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
            Some(Dialog::Machine(form)) => {
                let Some(form_key) = form_key(event) else {
                    return;
                };
                match form.key(form_key) {
                    FormEffect::None => {}
                    FormEffect::Submit => self.submit_machine(cx),
                    FormEffect::Cancel => self.dialogs.open = None,
                }
                true
            }
            Some(Dialog::MachineMenu(menu)) => match key {
                "up" | "k" => {
                    menu.cursor = menu.cursor.saturating_sub(1);
                    true
                }
                "down" | "j" | "tab" => {
                    menu.cursor = (menu.cursor + 1).min(MachineMenu::ITEMS.len() - 1);
                    true
                }
                "enter" => {
                    let item = menu.cursor;
                    self.choose_machine_menu(item, cx);
                    true
                }
                _ => false,
            },
            Some(Dialog::RemoveMachine { .. }) => match key {
                "enter" => {
                    self.remove_machine(cx);
                    true
                }
                _ => false,
            },
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
    pub(crate) fn focus_dialog(&mut self, cx: &mut Context<Self>) {
        self.refocus = true;
        cx.notify();
    }

    /// Names the machine shown in the window's title, when it changed.
    pub fn sync_title(&mut self, window: &mut Window) {
        let title = format!("Corgi — {}", self.machine().options.place());
        if title != self.title {
            window.set_window_title(&title);
            self.title = title;
        }
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
        &self.store().connection
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

    #[test]
    fn space_holds_its_row_back_until_its_turns_come_or_the_wait_is_up() {
        let now = Instant::now();
        let expanding = Expanding {
            pane_id: "w1:p1".into(),
            until: now + EXPANSION_WAIT,
        };
        let turns = |pane_id: &str| Turns {
            pane_id: pane_id.into(),
            entries: Vec::new(),
        };
        let held = |selected, transcript: Option<&Turns>, at| {
            held_back(Some(&expanding), Some(selected), transcript, at)
        };
        assert!(held("w1:p1", None, now));
        // The last row's turns are no use.
        assert!(held("w1:p1", Some(&turns("w2:p1")), now));
        // Its own end the wait, even with no turns written yet.
        assert!(!held("w1:p1", Some(&turns("w1:p1")), now));
        // So do the time and another row's selection.
        assert!(!held("w1:p1", None, now + EXPANSION_WAIT));
        assert!(!held("w2:p1", None, now));
        assert!(!held_back(None, Some("w1:p1"), None, now));
    }

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
