//! The window: its connection to `corgi serve`, the selection, the keys and
//! the actions. What it draws is in `view.rs`.

use std::{collections::HashSet, path::PathBuf, sync::Arc, time::Duration};

use corgi_core::protocol::{Agent, Command, Question};
use futures::{StreamExt, channel::mpsc};
use gpui::{
    App, AppContext, Context, Entity, FocusHandle, Focusable, KeyDownEvent, Pixels, ScrollHandle,
    SharedString, Subscription, Task, Window, WindowVisibility,
};

use crate::{
    dialogs::{Dialog, Dialogs, Followup, MergeAction, MergeStage},
    form::{self, Key},
    layout::{Layout, Split},
    link::{Backoff, Link, LinkEvent},
    options::Options,
    store::{Connection, Outbox, Purpose, Received, Store},
    text_input::{FieldColors, TextInput},
    theme::{Colors, forced_light, is_light},
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
    /// The agent the keys act on.
    pub selected: Option<String>,
    /// The session `corgi serve` was asked to read the transcript of.
    watching: Option<String>,
    /// The project groups whose cards show only their supervisor.
    pub collapsed: HashSet<String>,
    /// The window is on screen; while it is not, nothing is drawn and the
    /// transcript is not read.
    pub visible: bool,
    pub system_light: bool,
    pub colors: Colors,
    pub font: SharedString,
    pub focus: FocusHandle,
    pub form_focus: FocusHandle,
    pub prompt_input: Entity<TextInput>,
    pub task_input: Entity<TextInput>,
    pub overview_scroll: ScrollHandle,
    pub transcript_scroll: ScrollHandle,
    pub drag: Option<Drag>,
    /// The keyboard should move to where the open dialog wants it.
    refocus: bool,
    _tasks: Vec<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

impl CorgiWindow {
    pub fn new(
        options: Options,
        font: SharedString,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let forced_light = forced_light(std::env::var("CORGI_GUI_APPEARANCE").ok().as_deref());
        let system_light = forced_light.unwrap_or_else(|| is_light(window.appearance()));
        let store = Store::default();
        let colors = Colors::new(&store.state.palette, system_light);
        let field = field_colors(&colors);
        let prompt_input = cx.new(|cx| TextInput::new("Type a prompt", field, cx));
        let task_input = cx.new(|cx| TextInput::new("Describe the task", field, cx));
        let layout_path = Layout::path();
        let layout = Layout::load(layout_path.as_deref());
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
        // only while the window shows.
        let tick = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_secs(1)).await;
                let alive = this.update(cx, |this, cx| {
                    if this.visible {
                        cx.notify();
                    }
                });
                if alive.is_err() {
                    break;
                }
            }
        });

        let subscriptions = vec![
            cx.observe_window_appearance(window, move |this, window, cx| {
                if forced_light.is_none() {
                    this.system_light = is_light(window.appearance());
                    this.recolor(cx);
                }
            }),
            cx.observe_window_visibility(window, |this, visibility, _, cx| {
                this.set_visible(visibility == WindowVisibility::Visible, cx);
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
            watching: None,
            collapsed: HashSet::new(),
            visible: window.is_visible(),
            system_light,
            colors,
            font,
            focus: cx.focus_handle(),
            form_focus: cx.focus_handle(),
            prompt_input,
            task_input,
            overview_scroll: ScrollHandle::new(),
            transcript_scroll: ScrollHandle::new(),
            drag: None,
            refocus: false,
            _tasks: vec![pump, tick],
            _subscriptions: subscriptions,
        };
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
                if matches!(*message, corgi_core::protocol::ServerMessage::Hello(_)) {
                    self.backoff.reset();
                }
                match self.store.receive(*message) {
                    None => false,
                    Some(Received::State) => {
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
                        if let Some(purpose) = self.outbox.answered(&reply)
                            && let Some(followup) = self.dialogs.reply(purpose, reply)
                        {
                            self.follow(followup, cx);
                        }
                        self.focus_dialog(cx);
                        true
                    }
                }
            }
            LinkEvent::Ended(why) => {
                self.link = None;
                self.watching = None;
                let delay = self.backoff.next_delay();
                self.store.lost(why, delay);
                let unanswered = self.outbox.lost();
                self.dialogs.lost(&unanswered);
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

    pub fn selected_agent(&self) -> Option<&Agent> {
        self.store.agent(self.selected.as_deref()?)
    }

    /// Keeps the selection on a row that exists, and the transcript read
    /// for it.
    fn reconcile(&mut self, cx: &mut Context<Self>) {
        let visible = self.visible_agents();
        let keep = self
            .selected
            .as_ref()
            .is_some_and(|selected| visible.iter().any(|agent| &agent.pane_id == selected));
        if !keep {
            self.selected = visible.first().map(|agent| agent.pane_id.clone());
        }
        self.recolor(cx);
        self.sync_watch();
    }

    pub fn select(&mut self, pane_id: String, cx: &mut Context<Self>) {
        self.selected = Some(pane_id);
        if let Some(index) = self
            .visible_agents()
            .iter()
            .position(|agent| Some(&agent.pane_id) == self.selected.as_ref())
        {
            self.scroll_to_row(index);
        }
        self.sync_watch();
        cx.notify();
    }

    fn scroll_to_row(&self, index: usize) {
        // The overview's children are its projects; find the selected
        // agent's project.
        let Some(selected) = self.selected.as_deref() else {
            return;
        };
        let _ = index;
        if let Some(project) = self
            .store
            .state
            .projects
            .iter()
            .position(|project| project.agents.iter().any(|id| id == selected))
        {
            self.overview_scroll.scroll_to_item(project);
        }
    }

    fn step(&mut self, delta: isize, cx: &mut Context<Self>) {
        let agents = self.visible_agents();
        if agents.is_empty() {
            return;
        }
        let at = self
            .selected
            .as_ref()
            .and_then(|selected| agents.iter().position(|agent| &agent.pane_id == selected))
            .map_or(0, |at| at as isize);
        let next = (at + delta).clamp(0, agents.len() as isize - 1) as usize;
        let pane_id = agents[next].pane_id.clone();
        self.select(pane_id, cx);
    }

    /// Asks `corgi serve` to read the selected agent's transcript while the
    /// transcript shows and the window is on screen, and to stop otherwise.
    fn sync_watch(&mut self) {
        if !self.store.connected() {
            return;
        }
        let wanted = self
            .selected
            .clone()
            .filter(|_| self.visible && self.layout.transcript_open);
        if wanted != self.watching {
            self.watching = wanted.clone();
            self.send(Command::Watch { pane_id: wanted }, Purpose::Quiet);
        }
    }

    fn set_visible(&mut self, visible: bool, cx: &mut Context<Self>) {
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
        if visible {
            cx.notify();
        }
    }

    fn recolor(&mut self, cx: &mut Context<Self>) {
        let colors = Colors::new(&self.store.state.palette, self.system_light);
        if colors != self.colors {
            self.colors = colors;
            let field = field_colors(&self.colors);
            for input in [&self.prompt_input, &self.task_input] {
                input.update(cx, |input, cx| {
                    input.colors = field;
                    cx.notify();
                });
            }
            cx.notify();
        }
    }

    // ---- Layout ----

    pub fn save_layout(&self) {
        self.layout.save(self.layout_path.as_deref());
    }

    pub fn toggle_side(&mut self, cx: &mut Context<Self>) {
        self.layout.side_open = !self.layout.side_open;
        self.save_layout();
        cx.notify();
    }

    pub fn toggle_transcript(&mut self, cx: &mut Context<Self>) {
        self.layout.transcript_open = !self.layout.transcript_open;
        self.save_layout();
        self.sync_watch();
        cx.notify();
    }

    pub fn toggle_card(&mut self, group: &str, cx: &mut Context<Self>) {
        if !self.collapsed.remove(group) {
            self.collapsed.insert(group.to_string());
        }
        self.reconcile(cx);
        cx.notify();
    }

    pub fn drag_to(&mut self, position: gpui::Point<Pixels>, cx: &mut Context<Self>) {
        let Some(drag) = self.drag.as_mut() else {
            return;
        };
        let now = match drag.split {
            Split::Side => position.x,
            Split::Transcript => position.y,
        };
        let delta = f32::from(now - drag.last);
        drag.last = now;
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

    pub fn open_prompt(
        &mut self,
        pane_id: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(agent) = pane_id
            .as_deref()
            .or(self.selected.as_deref())
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
        self.dialogs.open = Some(Dialog::Answer {
            project_root,
            question,
            sending: false,
            error: None,
        });
        self.focus_dialog_now(window, cx);
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
                self.send(Command::MergeHandoff { pane_id }, Purpose::MergeEnd);
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
            _ => return,
        };
        if !self.send(command.0, command.1) {
            // Not sent: let the user try again.
            if let Some(Dialog::Prompt { sending, .. } | Dialog::Answer { sending, .. }) =
                &mut self.dialogs.open
            {
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
                self.select(pane_id, cx);
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
        let modifiers = keystroke.modifiers;
        if modifiers.platform || modifiers.control || modifiers.alt {
            return;
        }
        let selected_group = self
            .selected_agent()
            .map(|agent| agent.project_group.clone());
        match keystroke.key.as_str() {
            "j" | "down" => self.step(1, cx),
            "k" | "up" => self.step(-1, cx),
            "home" | "g" => self.step(-10_000, cx),
            "end" => self.step(10_000, cx),
            "enter" | "space" => self.toggle_transcript(cx),
            "\\" => self.toggle_side(cx),
            "left" | "right" => {
                let Some(group) = selected_group else { return };
                let collapse = keystroke.key == "left";
                let is_card = self.store.project(&group).is_some_and(|p| p.card);
                if is_card && collapse != self.collapsed.contains(&group) {
                    self.toggle_card(&group, cx);
                    // Collapsing moves the selection onto the supervisor.
                    if collapse
                        && let Some(supervisor) = self
                            .store
                            .project(&group)
                            .and_then(|p| p.supervisor.clone())
                    {
                        self.select(supervisor, cx);
                    }
                }
            }
            "p" => self.open_prompt(None, window, cx),
            "n" => self.new_agent(cx),
            "m" => self.merge_check(cx),
            "x" => self.close_check(None, cx),
            "s" => self.start_supervisor(cx),
            "t" => self.toggle_tag(cx),
            "r" => self.refresh(cx),
            "a" => self.answer_nth(0, window, cx),
            key if key.len() == 1 && ('1'..='9').contains(&key.chars().next().unwrap_or('0')) => {
                let index = key.parse::<usize>().unwrap_or(1) - 1;
                self.answer_nth(index, window, cx);
            }
            _ => return,
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
        let modifiers = keystroke.modifiers;
        if modifiers.platform || modifiers.control {
            return;
        }
        let key = keystroke.key.as_str();
        let handled = match &mut self.dialogs.open {
            None => false,
            Some(Dialog::Prompt { .. } | Dialog::Answer { .. }) => match key {
                "enter" => {
                    self.submit_text(cx);
                    true
                }
                "escape" => {
                    self.dialogs.open = None;
                    true
                }
                _ => false,
            },
            Some(Dialog::Merge(merge)) => match key {
                "enter" => {
                    if let Some((_, action)) = merge.actions().first().copied() {
                        self.merge_action(action, cx);
                    }
                    true
                }
                "escape" => {
                    // A running merge goes on; its outcome is said when it ends.
                    let action = merge
                        .actions()
                        .iter()
                        .rev()
                        .find(|(_, action)| {
                            matches!(action, MergeAction::Done | MergeAction::Dismiss)
                        })
                        .map(|(_, action)| *action);
                    match action {
                        Some(MergeAction::Dismiss) => self.merge_action(MergeAction::Dismiss, cx),
                        _ => self.dialogs.open = None,
                    }
                    true
                }
                _ => false,
            },
            Some(Dialog::NewAgentLoading) => {
                if key == "escape" {
                    self.dialogs.open = None;
                }
                true
            }
            Some(Dialog::Close(_)) => match key {
                "enter" => {
                    self.confirm_close(cx);
                    true
                }
                "escape" => {
                    self.dialogs.open = None;
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
            None => self.focus.clone(),
            Some(Dialog::Prompt { .. } | Dialog::Answer { .. }) => {
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

/// Keeps the app's keys: quitting.
pub fn bind_app_keys(cx: &mut App) {
    let quit = if cfg!(target_os = "macos") {
        "cmd-q"
    } else {
        "ctrl-q"
    };
    cx.bind_keys([gpui::KeyBinding::new(quit, Quit, None)]);
    cx.on_action(|_: &Quit, cx| cx.quit());
}

gpui::actions!(corgi, [Quit]);
