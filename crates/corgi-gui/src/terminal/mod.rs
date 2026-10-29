//! The focus view's terminal: one agent's real Herdr pane, live, painted
//! with Herdr UI's painter from the cells Herdr sends over the endpoint
//! connection (`crate::endpoint`), with no terminal emulator of our own.
//!
//! This is the interactive layer around the painter: keys and text (Herdr
//! encodes each for the pane's mode, so Shift+Enter, Alt+Enter and Ctrl+J
//! behave as in Herdr), IME and dead keys through the platform's input
//! handler (`input.rs`), paste, selection and copy (`selection.rs`), the
//! wheel, links (`links.rs`), resizing, and the view's own chords
//! (`keys.rs`). It shows a terminal only while the window is on screen; the
//! connection parks otherwise.

mod input;
pub mod keys;
pub mod links;
pub mod selection;

use std::{
    cell::RefCell,
    num::NonZeroU16,
    path::PathBuf,
    rc::Rc,
    sync::Arc,
    time::{Duration, Instant},
};

use corgi_core::{
    herdr::HerdrClient,
    palette::{Palette, Rgb},
};
use futures::{StreamExt, channel::mpsc};
use gpui::{
    AppContext, Bounds, ClipboardItem, Context, CursorStyle, EventEmitter, FocusHandle, Font,
    InteractiveElement, IntoElement, KeyDownEvent, Modifiers, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, ParentElement, Pixels, Point, Render, ScrollWheelEvent,
    SharedString, Styled, Task, Window, canvas, div, fill, px, rgb,
};
use serde_json::{Value, json};

use crate::endpoint::{
    self, Connection, Event, Geometry,
    protocol::{
        ClientHostAppearance, ClientHostColor, ClientHostDefaultColorKind, ClientHostThemeUpdate,
        ClientMouseButton, ClientMouseKind, ClientPaneInputEvent, ClientShellSnapshot,
        PaneSurfaceFrame, PaneSurfacePane,
    },
    viewer::{PARK_LABEL, Status, Viewer},
};
use crate::fonts;
use crate::herdr_ui::{
    config::Theme,
    terminal::{InputTarget, WheelAccumulator, key_input, viewport, wheel_target},
    terminal_painter::{Ground, Highlight, ImageTarget, PlacedImages, TerminalPainter, Tint},
};
use keys::Chord;
use links::Link;
use selection::{Region, Selection};

/// The terminal's text size and cell height, in pixels.
pub const FONT_SIZE: f32 = 13.0;
pub const CELL_HEIGHT: f32 = 17.0;
/// Room between the grid and the view's edges.
const PADDING: f32 = 6.0;
/// A size the layout settles on for this long is sent to Herdr.
const RESIZE_SETTLE: Duration = Duration::from_millis(30);
/// Lines one wheel notch scrolls: Herdr's `ui.mouse_scroll_lines` default.
const NOTCH_LINES: u16 = 3;
/// The first reconnect waits this long, doubling to `MAX_BACKOFF`.
const FIRST_BACKOFF: Duration = Duration::from_millis(500);
const MAX_BACKOFF: Duration = Duration::from_secs(10);
/// How long quitting gives Herdr to park the connection.
const PARK_ON_QUIT: Duration = Duration::from_millis(100);

/// What the view asks of the window around it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerminalEvent {
    Leave,
    ToggleSide,
}

/// The agent whose terminal the view shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    pub pane_id: String,
    pub tab_id: String,
    pub name: String,
}

pub struct TerminalView {
    pub focus: FocusHandle,
    /// The session's `herdr.sock`, from `corgi serve`'s hello; `None`
    /// before it, and over ssh, where terminals are not offered yet.
    json_socket: Option<PathBuf>,
    pub remote: bool,
    connection: Option<Connection>,
    /// Which connection an event came from; older ones' are dropped.
    generation: u64,
    viewer: Viewer,
    snapshot: Option<Arc<ClientShellSnapshot>>,
    surface: Option<Arc<PaneSurfaceFrame>>,
    images: Arc<endpoint::SurfaceImages>,
    pub target: Option<Target>,
    visible: bool,
    painter: Rc<RefCell<TerminalPainter>>,
    font: Font,
    cell_width: f32,
    host_theme: Vec<ClientHostThemeUpdate>,
    background: u32,
    /// The size the view's bounds hold, and the one Herdr was last told.
    grid: Option<Geometry>,
    sent: Option<Geometry>,
    resize: Option<Task<()>>,
    bounds: Bounds<Pixels>,
    selection: Option<Selection>,
    wheel: WheelAccumulator,
    /// The pane a program reading the mouse gets a press's drag and release.
    gesture: Option<String>,
    marked: String,
    marked_selection: Option<std::ops::Range<usize>>,
    hover: Option<Link>,
    pointer: Option<Point<Pixels>>,
    /// A copy waiting for Herdr's text, with the painted text to fall back on.
    copying: Option<(String, Option<String>)>,
    making_park: bool,
    /// Why the last connection ended, while the next one waits.
    ended: Option<String>,
    backoff: Duration,
    events: mpsc::UnboundedSender<(u64, Event)>,
    /// When a key was sent, and when the surface it changed came, for the
    /// latency log (`CORGI_GUI_LATENCY`).
    typed: Option<Instant>,
    arrived: Option<Instant>,
    /// Whether Herdr was last told the terminal has the keyboard.
    reported_focus: Option<bool>,
    _tasks: Vec<Task<()>>,
}

impl EventEmitter<TerminalEvent> for TerminalView {}

impl TerminalView {
    pub fn new(font: SharedString, cx: &mut Context<Self>) -> Self {
        let (events, mut receiver) = mpsc::unbounded::<(u64, Event)>();
        // Every event of the connection comes here; a burst is handled at
        // once and drawn once.
        let pump = cx.spawn(async move |this, cx| {
            while let Some(first) = receiver.next().await {
                let mut batch = vec![first];
                while let Ok(more) = receiver.try_recv() {
                    batch.push(more);
                }
                let alive = this.update(cx, |this, cx| {
                    for (generation, event) in batch {
                        this.event(generation, event, cx);
                    }
                });
                if alive.is_err() {
                    break;
                }
            }
        });
        let font = gpui::font(font);
        // The terminal's font is looked up, and loaded, off the UI thread:
        // a Nerd Font when one is installed, else the window's.
        let text = cx.text_system().clone();
        let fallback = font.clone();
        let fonts = cx.spawn(async move |this, cx| {
            let chosen = cx
                .background_spawn(async move {
                    let wanted = std::env::var("CORGI_GUI_TERMINAL_FONT")
                        .ok()
                        .filter(|font| !font.trim().is_empty());
                    let installed = text.all_font_names();
                    let family = fonts::terminal_font(wanted, &installed, &fallback.family);
                    let font = fonts::with_fallbacks(family, &installed);
                    for weight in [gpui::FontWeight::NORMAL, gpui::FontWeight::BOLD] {
                        for style in [gpui::FontStyle::Normal, gpui::FontStyle::Italic] {
                            text.resolve_font(&Font {
                                weight,
                                style,
                                ..font.clone()
                            });
                        }
                    }
                    font
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                if chosen != this.font {
                    eprintln!(
                        "corgi-gui: the terminal draws with the font {}",
                        chosen.family
                    );
                    this.font = chosen;
                    this.grid = None;
                    cx.notify();
                }
            });
        });
        Self {
            focus: cx.focus_handle(),
            json_socket: None,
            remote: false,
            connection: None,
            generation: 0,
            viewer: Viewer::new(false),
            snapshot: None,
            surface: None,
            images: Arc::default(),
            target: None,
            visible: true,
            painter: Rc::new(RefCell::new(TerminalPainter::default())),
            font,
            cell_width: 8.0,
            host_theme: Vec::new(),
            background: 0,
            grid: None,
            sent: None,
            resize: None,
            bounds: Bounds::default(),
            selection: None,
            wheel: WheelAccumulator::default(),
            gesture: None,
            marked: String::new(),
            marked_selection: None,
            hover: None,
            pointer: None,
            copying: None,
            making_park: false,
            ended: None,
            backoff: FIRST_BACKOFF,
            events,
            typed: None,
            arrived: None,
            reported_focus: None,
            _tasks: vec![pump, fonts],
        }
    }

    // ---- What the window asks ----

    /// The font the grid draws with, once it is chosen.
    #[cfg(feature = "capture")]
    pub fn font(&self) -> &Font {
        &self.font
    }

    /// The Herdr session `corgi serve` reads; `None` over ssh. A different
    /// session drops the connection to the old one.
    pub fn set_socket(
        &mut self,
        json_socket: Option<PathBuf>,
        remote: bool,
        cx: &mut Context<Self>,
    ) {
        self.remote = remote;
        if self.json_socket == json_socket {
            return;
        }
        self.json_socket = json_socket;
        self.disconnect();
        self.viewer = Viewer::new(false);
        self.ensure_connection(cx);
        self.drive(cx);
    }

    /// Shows `target`'s terminal.
    pub fn open(&mut self, target: Target, cx: &mut Context<Self>) {
        if self.target.as_ref().map(|t| &t.tab_id) != Some(&target.tab_id) {
            self.selection = None;
            self.hover = None;
            self.marked.clear();
        }
        self.target = Some(target);
        self.viewer.show(self.wanted());
        self.ensure_connection(cx);
        self.drive(cx);
    }

    /// Back to the overview: the connection parks.
    pub fn leave(&mut self, cx: &mut Context<Self>) {
        self.target = None;
        self.selection = None;
        self.hover = None;
        self.marked.clear();
        self.viewer.show(None);
        self.drive(cx);
    }

    /// The window was hidden or shown: a hidden window's connection parks
    /// and deactivates, so Herdr sends it nothing and sizes nothing by it.
    pub fn set_visible(&mut self, visible: bool, cx: &mut Context<Self>) {
        if self.visible == visible {
            return;
        }
        self.visible = visible;
        if !matches!(self.viewer.status(), Status::Gone { .. }) {
            self.viewer.show(self.wanted());
        }
        self.ensure_connection(cx);
        self.drive(cx);
    }

    /// The window's colors: the cells' and the ones Herdr answers a
    /// program's color queries with.
    pub fn set_colors(&mut self, palette: &Palette, accent: Rgb, muted: Rgb, light: bool) {
        let pack = |(r, g, b): Rgb| (u32::from(r) << 16) | (u32::from(g) << 8) | u32::from(b);
        let mut theme = Theme {
            background: pack(palette.background),
            foreground: pack(palette.foreground),
            cursor: pack(palette.foreground),
            muted: pack(muted),
            accent: Some(pack(accent)),
            ..Theme::default()
        };
        for (slot, color) in theme.palette.iter_mut().zip(palette.ansi) {
            *slot = pack(color);
        }
        self.background = theme.background;
        self.painter
            .borrow_mut()
            .set_appearance(FONT_SIZE, CELL_HEIGHT, theme);
        let color = |(r, g, b): Rgb| ClientHostColor { r, g, b };
        self.host_theme = vec![
            ClientHostThemeUpdate::DefaultColor {
                kind: ClientHostDefaultColorKind::Foreground,
                color: color(palette.foreground),
            },
            ClientHostThemeUpdate::DefaultColor {
                kind: ClientHostDefaultColorKind::Background,
                color: color(palette.background),
            },
            ClientHostThemeUpdate::PaletteColors(
                (0u8..)
                    .zip(palette.ansi)
                    .map(|(index, rgb)| (index, color(rgb)))
                    .collect(),
            ),
            ClientHostThemeUpdate::Appearance(if light {
                ClientHostAppearance::Light
            } else {
                ClientHostAppearance::Dark
            }),
        ];
        if let Some(connection) = &self.connection {
            connection.host_theme(self.host_theme.clone());
        }
    }

    /// What the window's status line says about the terminal.
    pub fn status_text(&self) -> String {
        let name = self
            .target
            .as_ref()
            .map_or("the agent", |t| t.name.as_str());
        if self.remote {
            return "Terminals are local only for now: this window reads Corgi over ssh".into();
        }
        if self.json_socket.is_none() {
            return "Waiting for corgi serve to name its Herdr session…".into();
        }
        if let Some(why) = &self.ended {
            return format!("Lost Herdr's endpoint ({why}); connecting again…");
        }
        match self.viewer.status() {
            Status::Connecting => "Connecting to Herdr…".into(),
            Status::Opening => format!("Opening {name}'s terminal…"),
            Status::Showing(_) => format!("{name} · live"),
            Status::Parked => "Parked".into(),
            Status::Gone { why, .. } => format!("{name}'s terminal is gone: {why}"),
        }
    }

    /// The terminal shows live cells now.
    pub fn live(&self) -> bool {
        matches!(self.viewer.status(), Status::Showing(_))
    }

    pub fn gone(&self) -> bool {
        matches!(self.viewer.status(), Status::Gone { .. })
    }

    /// For quitting: queues the park and the deactivation, gives Herdr
    /// `PARK_ON_QUIT` to take them (GPUI allows quitting 200 ms and runs no
    /// other task meanwhile, so nothing waits for the answers), then closes
    /// the connection, which releases whatever it still held, and the park
    /// tab's workspace.
    pub fn shut_down(&mut self, cx: &mut Context<Self>) -> impl Future<Output = ()> + use<> {
        let busy = self.connection.is_some() && !self.viewer.at_rest();
        self.target = None;
        self.viewer.show(None);
        self.drive(cx);
        let park = self.park_workspace();
        debug(|| {
            format!(
                "quitting: park tab {:?} in {park:?}",
                self.viewer.park_tab()
            )
        });
        let socket = self.json_socket.clone();
        let connection = self.connection.take();
        self.generation += 1;
        let wait = cx
            .background_executor()
            .timer(if busy { PARK_ON_QUIT } else { Duration::ZERO });
        async move {
            wait.await;
            if let Some(connection) = connection {
                connection.close();
            }
            if let (Some(socket), Some(workspace)) = (socket, park) {
                let _ = HerdrClient::from_socket_path(socket).close_workspace(&workspace);
            }
        }
    }

    /// The workspace of the park tab, to close with the window.
    fn park_workspace(&self) -> Option<String> {
        let park = self.viewer.park_tab()?;
        self.snapshot
            .as_ref()?
            .tabs
            .iter()
            .find(|tab| tab.tab_id == park)
            .map(|tab| tab.workspace_id.clone())
    }

    // ---- The connection ----

    fn wanted(&self) -> Option<String> {
        self.target
            .as_ref()
            .filter(|_| self.visible)
            .map(|target| target.tab_id.clone())
    }

    fn ensure_connection(&mut self, cx: &mut Context<Self>) {
        if self.connection.is_some() || self.target.is_none() || !self.visible || self.remote {
            return;
        }
        let (Some(socket), Some(geometry)) = (self.json_socket.clone(), self.grid) else {
            return;
        };
        self.generation += 1;
        let generation = self.generation;
        let events = self.events.clone();
        // A connection made to show a terminal says hello already active:
        // Herdr then claims the size only of a tab nobody owns, where an
        // activation would claim the tab it starts on, the user's.
        let active = self.wanted().is_some();
        self.viewer.reset_active(active);
        self.sent = Some(geometry);
        self.reported_focus = None;
        self.connection = Some(Connection::open(
            endpoint::client_socket(&socket),
            geometry,
            active,
            move |event| {
                let _ = events.unbounded_send((generation, event));
            },
        ));
        cx.notify();
    }

    fn disconnect(&mut self) {
        if let Some(connection) = self.connection.take() {
            connection.close();
        }
        self.generation += 1;
        self.viewer.reset();
        self.snapshot = None;
        self.surface = None;
    }

    /// Sends what the viewer wants sent now, and makes a park tab when it
    /// needs one.
    fn drive(&mut self, cx: &mut Context<Self>) {
        if self.viewer.needs_park() {
            self.make_park(cx);
        }
        if let Some(connection) = &self.connection {
            for step in self.viewer.next() {
                let (method, params) = step.request();
                let id = connection.request(method, params);
                debug(|| format!("sent {id}: {step:?}"));
                self.viewer.sent(id, step);
            }
        }
        cx.notify();
    }

    /// The park tab: one empty tab in a workspace of its own, made with the
    /// JSON API's `workspace.create`, which never moves anyone's focus.
    fn make_park(&mut self, cx: &mut Context<Self>) {
        let Some(socket) = self.json_socket.clone().filter(|_| !self.making_park) else {
            return;
        };
        self.making_park = true;
        let task = cx.spawn(async move |this, cx| {
            let made = cx
                .background_spawn(async move {
                    let herdr = HerdrClient::from_socket_path(socket);
                    let home = corgi_core::paths::home();
                    herdr
                        .create_workspace(home.as_deref(), Some(PARK_LABEL))
                        .map(|created| created.tab_id)
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.making_park = false;
                match made {
                    Ok(tab_id) => this.viewer.set_park(tab_id),
                    Err(error) => {
                        this.ended = Some(format!("cannot make its park tab: {error:#}"));
                    }
                }
                this.drive(cx);
            });
        });
        self._tasks.push(task);
        self._tasks.retain(|task| !task.is_ready());
    }

    fn event(&mut self, generation: u64, event: Event, cx: &mut Context<Self>) {
        if generation != self.generation {
            return;
        }
        match event {
            Event::Welcome(welcome) => {
                self.ended = None;
                self.backoff = FIRST_BACKOFF;
                if welcome.server_version != endpoint::TESTED_HERDR {
                    eprintln!(
                        "corgi-gui: Herdr {} (the terminal was tested with {})",
                        welcome.server_version,
                        endpoint::TESTED_HERDR
                    );
                }
                if let Some(connection) = &self.connection {
                    connection.host_theme(self.host_theme.clone());
                }
            }
            Event::Snapshot(snapshot) => {
                debug(|| {
                    format!(
                        "snapshot {} at {:?} / {:?} / {:?}; workspaces {:?}",
                        snapshot.revision,
                        snapshot.focused_workspace_id,
                        snapshot.focused_tab_id,
                        snapshot.focused_pane_id,
                        snapshot
                            .workspaces
                            .iter()
                            .map(|w| (&w.workspace_id, &w.label, &w.active_tab_id))
                            .collect::<Vec<_>>()
                    )
                });
                self.viewer.snapshot(&snapshot);
                self.snapshot = Some(snapshot);
                self.drive(cx);
            }
            Event::Surface => {
                if let Some((surface, images)) =
                    self.connection.as_ref().and_then(Connection::take_surface)
                {
                    self.surface = Some(surface);
                    self.images = images;
                    if let Some(typed) = self.typed.take()
                        && std::env::var_os("CORGI_GUI_LATENCY").is_some()
                    {
                        eprintln!(
                            "corgi-gui: latency key→surface {:.2} ms",
                            typed.elapsed().as_secs_f64() * 1000.0
                        );
                        self.arrived = Some(typed);
                    }
                    if self.visible {
                        cx.notify();
                    }
                }
            }
            Event::Response { id, result } => {
                debug(|| format!("answer {id}: {result:?}"));
                if self.viewer.answered(&id, &result) {
                    if matches!(self.viewer.status(), Status::Gone { .. }) {
                        self.selection = None;
                    }
                    self.drive(cx);
                } else if self.copying.as_ref().is_some_and(|(asked, _)| *asked == id) {
                    let (_, fallback) = self.copying.take().unwrap_or_default();
                    let text = result
                        .ok()
                        .and_then(|value| {
                            value.get("text").and_then(Value::as_str).map(String::from)
                        })
                        .or(fallback);
                    if let Some(text) = text.filter(|text| !text.is_empty()) {
                        cx.write_to_clipboard(ClipboardItem::new_string(text));
                    }
                }
            }
            Event::Clipboard(text) => cx.write_to_clipboard(ClipboardItem::new_string(text)),
            Event::Ended(why) => {
                self.connection = None;
                self.viewer.reset();
                self.snapshot = None;
                self.surface = None;
                self.ended = Some(why);
                self.reconnect_later(cx);
                cx.notify();
            }
        }
    }

    fn reconnect_later(&mut self, cx: &mut Context<Self>) {
        let delay = self.backoff;
        self.backoff = (self.backoff * 2).min(MAX_BACKOFF);
        let generation = self.generation;
        let task = cx.spawn(async move |this, cx| {
            cx.background_executor().timer(delay).await;
            let _ = this.update(cx, |this, cx| {
                if this.generation == generation && this.connection.is_none() {
                    this.ensure_connection(cx);
                    this.drive(cx);
                }
            });
        });
        self._tasks.push(task);
        self._tasks.retain(|task| !task.is_ready());
    }

    // ---- What the view shows ----

    /// The surface of the target's tab, while the terminal shows it.
    fn shown(&self) -> Option<&Arc<PaneSurfaceFrame>> {
        let Status::Showing(tab_id) = self.viewer.status() else {
            return None;
        };
        let snapshot = self.snapshot.as_ref()?;
        let surface = self.surface.as_ref()?;
        let ours = surface.panes.iter().all(|pane| {
            snapshot
                .panes
                .iter()
                .any(|known| known.pane_id == pane.pane_id && known.tab_id == tab_id)
        });
        (ours && !surface.panes.is_empty()).then_some(surface)
    }

    /// The pane of the agent, or the pane under a point in grid pixels.
    fn pane_at(&self, x: f32, y: f32) -> Option<&PaneSurfacePane> {
        let surface = self.shown()?;
        let (cell_width, cell_height) = (self.cell_width, CELL_HEIGHT);
        surface
            .panes
            .iter()
            .find(|pane| {
                let r = pane.inner_rect;
                x >= f32::from(r.x) * cell_width
                    && x < f32::from(r.x + r.width) * cell_width
                    && y >= f32::from(r.y) * cell_height
                    && y < f32::from(r.y + r.height) * cell_height
            })
            .or_else(|| self.agent_pane())
    }

    fn agent_pane(&self) -> Option<&PaneSurfacePane> {
        let pane_id = &self.target.as_ref()?.pane_id;
        self.shown()?
            .panes
            .iter()
            .find(|pane| &pane.pane_id == pane_id)
    }

    /// Input for the agent's pane, while it shows.
    fn send(&mut self, events: Vec<ClientPaneInputEvent>) {
        let (Some(connection), Some(target)) = (&self.connection, &self.target) else {
            return;
        };
        if self.live() {
            connection.input(&target.pane_id, events);
        }
    }

    /// A point in window coordinates, in the grid's pixels.
    fn local(&self, position: Point<Pixels>) -> (f32, f32) {
        (
            f32::from(position.x - self.bounds.origin.x),
            f32::from(position.y - self.bounds.origin.y),
        )
    }

    /// The grid's bounds, from the canvas's layout: the size Herdr is told,
    /// once it has settled, and the first connection's.
    fn layout(&mut self, bounds: Bounds<Pixels>, cx: &mut Context<Self>) {
        self.bounds = bounds;
        let size = viewport(
            f32::from(bounds.size.width),
            f32::from(bounds.size.height),
            self.cell_width,
            CELL_HEIGHT,
        );
        let geometry = Geometry {
            size,
            cell_width_px: self.cell_width.round().max(1.0) as u32,
            cell_height_px: CELL_HEIGHT as u32,
        };
        if self.grid == Some(geometry) {
            return;
        }
        // Said on stderr, for aiming scripted clicks and checking resizes.
        eprintln!(
            "corgi-gui: terminal grid at ({:.0}, {:.0}), {}x{} cells of {:.2}x{CELL_HEIGHT} px",
            f32::from(bounds.origin.x),
            f32::from(bounds.origin.y),
            size.cols,
            size.rows,
            self.cell_width,
        );
        self.grid = Some(geometry);
        if self.connection.is_none() {
            self.ensure_connection(cx);
            self.drive(cx);
            return;
        }
        // Resizes settle first, so a drag sends one size, not every one.
        self.resize = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(RESIZE_SETTLE).await;
            let _ = this.update(cx, |this, _| {
                if let (Some(connection), Some(grid)) = (&this.connection, this.grid)
                    && this.sent != Some(grid)
                {
                    connection.resize(grid);
                    this.sent = Some(grid);
                }
            });
        }));
    }

    // ---- Keys ----

    fn key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let mac = cfg!(target_os = "macos");
        if let Some(chord) = keys::chord(&event.keystroke, mac) {
            match chord {
                Chord::Leave => cx.emit(TerminalEvent::Leave),
                Chord::ToggleSide => cx.emit(TerminalEvent::ToggleSide),
                Chord::Copy => {
                    // With nothing selected, Ctrl+Shift+C is the pane's.
                    if !self.copy(cx) && !mac {
                        self.send_key(event);
                    }
                }
                Chord::Paste => self.paste(cx),
                Chord::Page { up } => self.page(up),
            }
            cx.stop_propagation();
            window.prevent_default();
            return;
        }
        // While an input method composes, its keys are its own.
        if !self.marked.is_empty() {
            return;
        }
        if self.send_key(event) {
            cx.stop_propagation();
            window.prevent_default();
        }
    }

    /// Sends a key the pane takes as a key event; text comes through the
    /// input handler instead, so layouts, dead keys and IMEs work.
    fn send_key(&mut self, event: &KeyDownEvent) -> bool {
        let Some(input) = key_input(event, false) else {
            return false;
        };
        if !self.live() {
            return true;
        }
        self.selection = None;
        self.typed = Some(Instant::now());
        self.send(vec![input]);
        true
    }

    fn text(&mut self, text: &str) {
        if text.is_empty() || !self.live() {
            return;
        }
        self.selection = None;
        self.typed = Some(Instant::now());
        self.send(vec![ClientPaneInputEvent::TextCommit(text.to_string())]);
    }

    /// Pastes the clipboard's text; Herdr brackets it when the program
    /// asked for bracketed paste.
    fn paste(&mut self, cx: &mut Context<Self>) {
        let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) else {
            return;
        };
        if !text.is_empty() {
            self.selection = None;
            self.send(vec![ClientPaneInputEvent::Paste(text)]);
        }
    }

    /// Copies the selection; false when there is none.
    fn copy(&mut self, cx: &mut Context<Self>) -> bool {
        let (Some(selection), Some(surface)) = (&self.selection, self.shown()) else {
            return false;
        };
        let Some(pane) = surface
            .panes
            .iter()
            .find(|pane| pane.pane_id == selection.pane_id)
        else {
            return false;
        };
        let region = Region::of(pane);
        let Some((start, end)) = selection.range(&region) else {
            return false;
        };
        let painted = selection.text(&region, &surface.frame);
        // Herdr's text knows where lines wrapped and what scrolled away.
        let asked = self.connection.as_ref().map(|connection| {
            connection.request(
                "pane.selection.read",
                json!({
                    "pane_id": selection.pane_id,
                    "anchor": {"row": start.row, "col": start.col},
                    "cursor": {"row": end.row, "col": end.col},
                }),
            )
        });
        match asked {
            Some(id) => self.copying = Some((id, painted)),
            None => {
                if let Some(text) = painted {
                    cx.write_to_clipboard(ClipboardItem::new_string(text));
                }
            }
        }
        true
    }

    /// A page of history up or down, through Herdr's scrollback whatever
    /// the program does with PageUp.
    fn page(&mut self, up: bool) {
        let (Some(pane), Some(connection)) = (self.agent_pane(), &self.connection) else {
            return;
        };
        let Some(scroll) = pane.scroll else {
            return;
        };
        let page = u64::from(pane.inner_rect.height.saturating_sub(1).max(1));
        let offset = if up {
            (scroll.offset_from_bottom + page).min(scroll.max_offset_from_bottom)
        } else {
            scroll.offset_from_bottom.saturating_sub(page)
        };
        connection.request(
            "pane.scroll",
            json!({"pane_id": pane.pane_id, "offset_from_bottom": offset}),
        );
    }

    // ---- The mouse ----

    fn link_modifier(modifiers: &Modifiers) -> bool {
        if cfg!(target_os = "macos") {
            modifiers.platform
        } else {
            modifiers.control
        }
    }

    fn link_at(&self, x: f32, y: f32) -> Option<Link> {
        let surface = self.shown()?;
        let pane = self.pane_at(x, y)?;
        let (column, row) = (x / self.cell_width, y / CELL_HEIGHT);
        if column < 0.0 || row < 0.0 {
            return None;
        }
        links::at(&surface.frame, &Region::of(pane), column as u16, row as u16)
    }

    fn update_hover(&mut self, modifiers: &Modifiers, cx: &mut Context<Self>) {
        let hover = self
            .pointer
            .filter(|_| Self::link_modifier(modifiers))
            .and_then(|pointer| {
                let (x, y) = self.local(pointer);
                self.link_at(x, y)
            })
            .filter(|link| links::openable(&link.url));
        if hover != self.hover {
            self.hover = hover;
            cx.notify();
        }
    }

    fn mouse_down(&mut self, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus, cx);
        let (x, y) = self.local(event.position);
        if Self::link_modifier(&event.modifiers)
            && event.button == MouseButton::Left
            && let Some(link) = self.link_at(x, y).filter(|link| links::openable(&link.url))
        {
            open_url(&link.url, cx);
            return;
        }
        let Some(surface) = self.shown().cloned() else {
            return;
        };
        // A program that reads the mouse gets the press; Shift selects anyway.
        if let Some(target) = wheel_target(&surface, x, y, self.cell_width, CELL_HEIGHT)
            && target.mouse_reporting
            && !event.modifiers.shift
            && let InputTarget::Pane(pane_id) = &target.target
        {
            let button = match event.button {
                MouseButton::Left => ClientMouseButton::Left,
                MouseButton::Right => ClientMouseButton::Right,
                MouseButton::Middle => ClientMouseButton::Middle,
                _ => return,
            };
            let press = target.mouse_event(ClientMouseKind::Down(button), event.modifiers);
            if let Some(connection) = &self.connection {
                connection.input(pane_id, vec![press]);
            }
            self.gesture = Some(pane_id.clone());
            return;
        }
        if event.button != MouseButton::Left {
            return;
        }
        let Some(pane) = self.pane_at(x, y) else {
            return;
        };
        self.selection = Selection::begin(
            &Region::of(pane),
            &surface.frame,
            x / self.cell_width,
            y / CELL_HEIGHT,
            event.click_count,
        );
        cx.notify();
    }

    fn mouse_move(&mut self, event: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        self.pointer = Some(event.position);
        let (x, y) = self.local(event.position);
        if let Some(pane_id) = self.gesture.clone() {
            if let (Some(surface), Some(button)) = (self.shown().cloned(), event.pressed_button) {
                let button = match button {
                    MouseButton::Left => ClientMouseButton::Left,
                    MouseButton::Right => ClientMouseButton::Right,
                    _ => ClientMouseButton::Middle,
                };
                if let Some(target) = wheel_target(&surface, x, y, self.cell_width, CELL_HEIGHT)
                    && let Some(connection) = &self.connection
                {
                    let drag = target.mouse_event(ClientMouseKind::Drag(button), event.modifiers);
                    connection.input(&pane_id, vec![drag]);
                }
            }
            return;
        }
        if let Some(selection) = self.selection.as_ref().filter(|s| s.dragging) {
            if event.pressed_button != Some(MouseButton::Left) {
                self.end_selection(cx);
            } else if let Some(surface) = self.shown().cloned()
                && let Some(pane) = surface
                    .panes
                    .iter()
                    .find(|pane| pane.pane_id == selection.pane_id)
            {
                let region = Region::of(pane);
                if let Some(selection) = self.selection.as_mut() {
                    selection.extend(
                        &region,
                        &surface.frame,
                        x / self.cell_width,
                        y / CELL_HEIGHT,
                    );
                }
                cx.notify();
            }
        }
        self.update_hover(&event.modifiers, cx);
    }

    fn mouse_up(&mut self, event: &MouseUpEvent, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(pane_id) = self.gesture.take() {
            if let Some(surface) = self.shown().cloned() {
                let (x, y) = self.local(event.position);
                let button = match event.button {
                    MouseButton::Left => ClientMouseButton::Left,
                    MouseButton::Right => ClientMouseButton::Right,
                    _ => ClientMouseButton::Middle,
                };
                if let Some(target) = wheel_target(&surface, x, y, self.cell_width, CELL_HEIGHT)
                    && let Some(connection) = &self.connection
                {
                    let release = target.mouse_event(ClientMouseKind::Up(button), event.modifiers);
                    connection.input(&pane_id, vec![release]);
                }
            }
            return;
        }
        self.end_selection(cx);
    }

    fn end_selection(&mut self, cx: &mut Context<Self>) {
        let Some(selection) = self.selection.as_ref() else {
            return;
        };
        // A plain click selects nothing.
        let empty = self
            .shown()
            .and_then(|surface| {
                surface
                    .panes
                    .iter()
                    .find(|p| p.pane_id == selection.pane_id)
            })
            .is_none_or(|pane| selection.range(&Region::of(pane)).is_none());
        if empty {
            self.selection = None;
        } else if let Some(selection) = self.selection.as_mut() {
            selection.dragging = false;
        }
        cx.notify();
    }

    fn scroll_wheel(&mut self, event: &ScrollWheelEvent, _: &mut Window, cx: &mut Context<Self>) {
        let Some(surface) = self.shown().cloned() else {
            return;
        };
        let (x, y) = self.local(event.position);
        let Some(target) = wheel_target(&surface, x, y, self.cell_width, CELL_HEIGHT) else {
            self.wheel = WheelAccumulator::default();
            return;
        };
        let notch = NonZeroU16::new(NOTCH_LINES).unwrap_or(NonZeroU16::MIN);
        let steps = self
            .wheel
            .steps(&target, event, self.cell_width, CELL_HEIGHT, notch);
        let inputs: Vec<_> = target.wheel_events(steps, event.modifiers).collect();
        if let (InputTarget::Pane(pane_id), Some(connection)) = (&target.target, &self.connection) {
            connection.input(pane_id, inputs);
        }
        cx.stop_propagation();
    }
}

/// Writes what the connection does to stderr, with
/// `CORGI_GUI_DEBUG_ENDPOINT` set.
fn debug(line: impl FnOnce() -> String) {
    if std::env::var_os("CORGI_GUI_DEBUG_ENDPOINT").is_some() {
        eprintln!("corgi-gui: endpoint {}", line());
    }
}

/// Opens `url` in the system's browser. A capture build given
/// `CORGI_GUI_URL_LOG` writes it to that file instead, so a scripted check
/// opens nothing on the desktop.
fn open_url(url: &str, cx: &mut Context<TerminalView>) {
    #[cfg(feature = "capture")]
    if let Some(log) = std::env::var_os("CORGI_GUI_URL_LOG") {
        use std::io::Write;
        if let Ok(mut file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(log)
        {
            let _ = writeln!(file, "{url}");
        }
        return;
    }
    cx.open_url(url);
}

impl Render for TerminalView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.cell_width = self.painter.borrow_mut().cell_width(&self.font, window, cx);
        // A program that asked for focus reports learns when the terminal
        // has the keyboard.
        let focused = self.focus.is_focused(window);
        if let Some(connection) = &self.connection
            && self.reported_focus != Some(focused)
        {
            connection.focus(focused);
            self.reported_focus = Some(focused);
        }
        let typed = self.arrived.take();
        let entity = cx.entity();
        let weak = entity.downgrade();
        let focus = self.focus.clone();
        let painter = self.painter.clone();
        let font = self.font.clone();
        let cell_width = self.cell_width;
        let background = self.background;
        let surface = self.shown().cloned();
        let images = self.images.clone();
        let highlights: Vec<Highlight> = match (&self.selection, &surface) {
            (Some(selection), Some(surface)) => surface
                .panes
                .iter()
                .find(|pane| pane.pane_id == selection.pane_id)
                .map(|pane| {
                    selection
                        .highlights(&Region::of(pane))
                        .into_iter()
                        .map(|(row, columns)| Highlight {
                            row,
                            columns,
                            tint: Tint::Selection,
                        })
                        .collect()
                })
                .unwrap_or_default(),
            _ => Vec::new(),
        };
        let hover = self.hover.clone().filter(|_| surface.is_some());
        let marked = self.marked.clone();
        let terminal = canvas(
            move |bounds, _, cx| {
                entity.update(cx, |this, cx| this.layout(bounds, cx));
            },
            move |bounds, (), window, cx| {
                window.handle_input(&focus, input::TerminalInputHandler::new(bounds, weak), cx);
                window.paint_quad(fill(bounds, rgb(background)));
                let Some(surface) = surface else {
                    return;
                };
                window.with_content_mask(Some(gpui::ContentMask { bounds }), |window| {
                    let placed = (!surface.graphics.placements.is_empty() && !images.is_empty())
                        .then(|| PlacedImages {
                            placements: &surface.graphics.placements,
                            images: &images,
                            target: ImageTarget::Main,
                        });
                    let mut painter = painter.borrow_mut();
                    painter.paint_frame(
                        &surface.frame,
                        bounds.origin,
                        Some(bounds.size),
                        Ground::Opaque,
                        cell_width,
                        &font,
                        &highlights,
                        &surface.panes,
                        None,
                        placed,
                        window,
                        cx,
                    );
                    if placed.is_none() {
                        painter.release_idle_images(window);
                    }
                    if let Some(typed) = typed {
                        eprintln!(
                            "corgi-gui: latency key→painted frame {:.2} ms",
                            typed.elapsed().as_secs_f64() * 1000.0
                        );
                    }
                    if let Some(link) = &hover {
                        painter.paint_link(
                            &surface.frame,
                            bounds.origin,
                            cell_width,
                            &link.cells,
                            window,
                        );
                    }
                    if !marked.is_empty() {
                        let cursor = crate::herdr_ui::terminal::input_cursor_bounds(
                            Some(&surface),
                            bounds.origin,
                            cell_width,
                            CELL_HEIGHT,
                        );
                        painter.paint_composition(&marked, cursor.origin, bounds, &font, window);
                    }
                });
            },
        )
        .size_full();
        div()
            .id("terminal")
            .key_context("Terminal")
            .track_focus(&self.focus)
            .size_full()
            .bg(rgb(self.background))
            .cursor(if self.hover.is_some() {
                CursorStyle::PointingHand
            } else {
                CursorStyle::IBeam
            })
            .on_key_down(cx.listener(Self::key_down))
            .on_modifiers_changed(cx.listener(
                |this, event: &gpui::ModifiersChangedEvent, _, cx| {
                    this.update_hover(&event.modifiers, cx);
                },
            ))
            .on_any_mouse_down(cx.listener(Self::mouse_down))
            .on_mouse_move(cx.listener(Self::mouse_move))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::mouse_up))
            .on_mouse_up(MouseButton::Right, cx.listener(Self::mouse_up))
            .on_mouse_up(MouseButton::Middle, cx.listener(Self::mouse_up))
            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::mouse_up))
            .on_scroll_wheel(cx.listener(Self::scroll_wheel))
            .child(div().size_full().p(px(PADDING)).child(terminal))
    }
}
