//! One connection to `herdr-client.sock`: the handshake, the snapshots and
//! surfaces Herdr pushes, and the requests and input the window sends.
//!
//! A reader thread connects, says hello and reads every message; a writer
//! thread writes what the window queues. Herdr runs one endpoint request at a
//! time per connection and answers another with `endpoint_busy`, except
//! `client_shell.surface.set`, which it handles at once; so the writer keeps
//! everything behind a request until its answer comes (first in, first out),
//! and a `surface.set` sent first goes straight out. Nothing is retried: a
//! connection that ends says why, once, and the window connects again.

use std::{
    collections::{HashMap, HashSet, VecDeque},
    io::Write,
    os::unix::net::UnixStream,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex, PoisonError,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc,
    },
    thread,
    time::Duration,
};

use serde_json::{Value, json};

use super::protocol::{
    BLOB_CODEC_V1, ClientHostThemeUpdate, ClientMessage, ClientPaneInputEvent, ClientShellSnapshot,
    ClientSurfaceSize, ENDPOINT_HELLO_KIND, ENDPOINT_PROTOCOL_GENERATION, ENDPOINT_SNAPSHOT_KIND,
    ENDPOINT_WELCOME_KIND, EndpointClientHello, EndpointServerWelcome, INPUT_CODEC_V1,
    MAX_FRAME_SIZE, MAX_GRAPHICS_FRAME_SIZE, PaneSurfaceFrame, SNAPSHOT_CODEC_V1, SURFACE_CODEC_V1,
    ServerMessage, SurfaceGraphicsAssetKey, encode_message, read_message,
};
use super::{SurfaceImage, SurfaceImages, valid_asset};

/// The only request Herdr runs beside another; it never takes the lease.
pub const SURFACE_SET: &str = "client_shell.surface.set";
/// How long the handshake may take before the connection gives up.
const HANDSHAKE: Duration = Duration::from_secs(10);
/// A response bigger than this ends the connection.
const MAX_RESPONSE_BYTES: usize = 8 * 1024 * 1024;
/// Images kept for one connection: Herdr UI's bounds.
const MAX_IMAGES: usize = 256;
const MAX_IMAGE_BYTES: usize = 256 * 1024 * 1024;

/// The terminal's size and its cells' size in pixels, as the hello and each
/// resize give them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Geometry {
    pub size: ClientSurfaceSize,
    pub cell_width_px: u32,
    pub cell_height_px: u32,
}

/// Herdr's refusal of a request: `endpoint_busy`, `surface_inactive`,
/// `tab_not_found` and so on, with its message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApiError {
    pub code: String,
    pub message: String,
}

/// What the connection tells the window.
#[derive(Debug)]
pub enum Event {
    /// The handshake passed; the snapshot follows.
    Welcome(EndpointServerWelcome),
    /// Where this connection is, and the session's tabs and panes.
    Snapshot(Arc<ClientShellSnapshot>),
    /// A new surface waits in [`Connection::take_surface`]. Only one such
    /// event is pending at a time, however many surfaces came meanwhile.
    Surface,
    Response {
        id: String,
        result: Result<Value, ApiError>,
    },
    /// A program in a pane set the clipboard (OSC 52).
    Clipboard(String),
    /// The connection is over; nothing more comes.
    Ended(String),
}

/// What the writer is asked to do.
enum Out {
    /// Known once the first snapshot names the server's boot.
    Boot(String),
    Message(ClientMessage),
    Request {
        id: String,
        method: String,
        params: Value,
    },
    /// The answer to `id` came: a request waiting behind it may go.
    Answered(String),
}

/// The latest surface, for the window to take when it draws.
#[derive(Default)]
struct Latest {
    surface: Option<(Arc<PaneSurfaceFrame>, Arc<SurfaceImages>)>,
}

/// A connection to Herdr's endpoint. Dropping it closes the socket.
pub struct Connection {
    out: mpsc::Sender<Out>,
    stop: Arc<AtomicBool>,
    stream: Arc<Mutex<Option<UnixStream>>>,
    latest: Arc<Mutex<Latest>>,
    next_id: AtomicU64,
}

impl Connection {
    /// Connects to `socket` on a thread of its own and says hello with
    /// `geometry`, inactive or not; every outcome, a failure to connect
    /// included, arrives through `events`.
    pub fn open(
        socket: PathBuf,
        geometry: Geometry,
        active: bool,
        events: impl Fn(Event) + Send + Sync + 'static,
    ) -> Self {
        let (out, to_writer) = mpsc::channel();
        let stop = Arc::new(AtomicBool::new(false));
        let stream = Arc::new(Mutex::new(None));
        let latest = Arc::new(Mutex::new(Latest::default()));
        let events: Arc<dyn Fn(Event) + Send + Sync> = Arc::new(events);
        let reader = Reader {
            out: out.clone(),
            stop: stop.clone(),
            stream: stream.clone(),
            latest: latest.clone(),
            events: events.clone(),
        };
        let spawned = thread::Builder::new()
            .name("herdr-endpoint-read".into())
            .spawn(move || reader.run(&socket, geometry, active, to_writer));
        if let Err(error) = spawned {
            stop.store(true, Ordering::Release);
            events(Event::Ended(format!("cannot start a thread: {error}")));
        }
        Self {
            out,
            stop,
            stream,
            latest,
            next_id: AtomicU64::new(1),
        }
    }

    /// Queues an endpoint request; its answer comes as [`Event::Response`]
    /// with the id returned here.
    pub fn request(&self, method: &str, params: Value) -> String {
        let id = format!("corgi-gui-{}", self.next_id.fetch_add(1, Ordering::Relaxed));
        let _ = self.out.send(Out::Request {
            id: id.clone(),
            method: method.to_string(),
            params,
        });
        id
    }

    /// Keys, text, paste and mouse events for `pane_id`.
    pub fn input(&self, pane_id: &str, events: Vec<ClientPaneInputEvent>) {
        if events.is_empty() {
            return;
        }
        let _ = self
            .out
            .send(Out::Message(ClientMessage::ClientShellPaneInput {
                pane_id: pane_id.to_string(),
                events,
            }));
    }

    pub fn resize(&self, geometry: Geometry) {
        let _ = self
            .out
            .send(Out::Message(ClientMessage::ClientShellResize {
                cell_width_px: geometry.cell_width_px,
                cell_height_px: geometry.cell_height_px,
                surface_size: geometry.size,
                pixel_mouse: false,
            }));
    }

    /// Tells Herdr the window's colors, which it answers a program's color
    /// queries (OSC 10, 11, 4) with.
    pub fn host_theme(&self, updates: Vec<ClientHostThemeUpdate>) {
        for update in updates {
            let _ = self
                .out
                .send(Out::Message(ClientMessage::ClientShellHostTheme { update }));
        }
    }

    /// Whether the window has keyboard focus, which Herdr passes on to a
    /// program that asked for focus reports.
    pub fn focus(&self, focused: bool) {
        let _ = self
            .out
            .send(Out::Message(ClientMessage::ClientShellFocus { focused }));
    }

    /// The newest surface since the last call, with the images it may place.
    pub fn take_surface(&self) -> Option<(Arc<PaneSurfaceFrame>, Arc<SurfaceImages>)> {
        lock(&self.latest).surface.take()
    }

    /// Ends the connection now; nothing queued is sent.
    pub fn close(&self) {
        self.stop.store(true, Ordering::Release);
        if let Some(stream) = lock(&self.stream).as_ref() {
            let _ = stream.shutdown(std::net::Shutdown::Both);
        }
    }
}

impl Drop for Connection {
    fn drop(&mut self) {
        self.close();
    }
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

struct Reader {
    out: mpsc::Sender<Out>,
    stop: Arc<AtomicBool>,
    stream: Arc<Mutex<Option<UnixStream>>>,
    latest: Arc<Mutex<Latest>>,
    events: Arc<dyn Fn(Event) + Send + Sync>,
}

impl Reader {
    fn run(self, socket: &Path, geometry: Geometry, active: bool, to_writer: mpsc::Receiver<Out>) {
        let why = match self.serve(socket, geometry, active, to_writer) {
            Ok(()) => "Herdr closed the connection".to_string(),
            Err(_) if self.stop.load(Ordering::Acquire) => "closed".to_string(),
            Err(error) => error,
        };
        self.stop.store(true, Ordering::Release);
        if let Some(stream) = lock(&self.stream).take() {
            let _ = stream.shutdown(std::net::Shutdown::Both);
        }
        (self.events)(Event::Ended(why));
    }

    fn serve(
        &self,
        socket: &Path,
        geometry: Geometry,
        active: bool,
        to_writer: mpsc::Receiver<Out>,
    ) -> Result<(), String> {
        let mut stream = UnixStream::connect(socket)
            .map_err(|error| format!("cannot connect to {}: {error}", socket.display()))?;
        let writer_stream = stream.try_clone().map_err(|error| error.to_string())?;
        *lock(&self.stream) = Some(stream.try_clone().map_err(|error| error.to_string())?);
        if self.stop.load(Ordering::Acquire) {
            return Err("closed".into());
        }
        stream
            .set_write_timeout(Some(Duration::from_secs(2)))
            .map_err(|error| error.to_string())?;
        let hello = hello(geometry, active);
        let frame = encode_message(
            &ClientMessage::EndpointControl {
                kind: ENDPOINT_HELLO_KIND.into(),
                data: serde_json::to_string(&hello).map_err(|error| error.to_string())?,
            },
            MAX_FRAME_SIZE,
        )
        .map_err(|error| error.to_string())?;
        stream
            .write_all(&frame)
            .map_err(|error| format!("cannot say hello to Herdr: {error}"))?;

        stream
            .set_read_timeout(Some(HANDSHAKE))
            .map_err(|error| error.to_string())?;
        let welcome = match read_message::<_, ServerMessage>(&mut stream, MAX_GRAPHICS_FRAME_SIZE) {
            Ok(message) => check_welcome(message)?,
            Err(error) => {
                return Err(format!(
                    "Herdr did not welcome the connection ({error}); is it older than 0.9.0?"
                ));
            }
        };
        (self.events)(Event::Welcome(welcome));
        let stop = self.stop.clone();
        thread::Builder::new()
            .name("herdr-endpoint-write".into())
            .spawn(move || write_loop(writer_stream, to_writer, &stop))
            .map_err(|error| error.to_string())?;

        let mut state = Session::default();
        let mut snapshot_seen = false;
        loop {
            let message = read_message::<_, ServerMessage>(&mut stream, MAX_GRAPHICS_FRAME_SIZE)
                .map_err(|error| format!("lost Herdr's endpoint: {error}"))?;
            if !snapshot_seen {
                // The first snapshot ends the handshake's time limit.
                snapshot_seen = matches!(&message, ServerMessage::EndpointControl { kind, .. } if kind == ENDPOINT_SNAPSHOT_KIND);
                if snapshot_seen {
                    stream
                        .set_read_timeout(None)
                        .map_err(|error| error.to_string())?;
                }
            }
            for step in state.receive(message)? {
                match step {
                    Step::Boot(boot) => {
                        let _ = self.out.send(Out::Boot(boot));
                    }
                    Step::Answered(id) => {
                        let _ = self.out.send(Out::Answered(id));
                    }
                    Step::Surface(surface, images) => {
                        let wake = {
                            let mut latest = lock(&self.latest);
                            let wake = latest.surface.is_none();
                            latest.surface = Some((surface, images));
                            wake
                        };
                        if wake {
                            (self.events)(Event::Surface);
                        }
                    }
                    Step::Event(event) => (self.events)(event),
                }
            }
        }
    }
}

/// The hello: generation 1, the window's size, and the codecs Herdr 0.9.0
/// speaks. No optional surface encodings, no graphics passthrough, no pixel
/// mouse, and none of Herdr's key bindings: every key is the pane's.
fn hello(geometry: Geometry, active: bool) -> EndpointClientHello {
    EndpointClientHello {
        generation: ENDPOINT_PROTOCOL_GENERATION,
        cell_width_px: geometry.cell_width_px,
        cell_height_px: geometry.cell_height_px,
        surface_size: geometry.size,
        pixel_mouse: false,
        direct_graphics: false,
        endpoint_keybindings: false,
        mouse_capture: false,
        surface_active: active,
        surface_reuse: false,
        surface_delta: false,
        surface_scroll: false,
        snapshot_codecs: vec![SNAPSHOT_CODEC_V1.into()],
        surface_codecs: vec![SURFACE_CODEC_V1.into()],
        input_codecs: vec![INPUT_CODEC_V1.into()],
        blob_codecs: vec![BLOB_CODEC_V1.into()],
    }
}

/// The welcome, or why the connection cannot go on with it.
fn check_welcome(message: ServerMessage) -> Result<EndpointServerWelcome, String> {
    let ServerMessage::EndpointControl { kind, data } = message else {
        return Err("Herdr answered the hello with something other than a welcome".into());
    };
    if kind != ENDPOINT_WELCOME_KIND {
        return Err(format!("Herdr answered the hello with `{kind}`"));
    }
    let welcome: EndpointServerWelcome =
        serde_json::from_str(&data).map_err(|error| format!("unreadable welcome: {error}"))?;
    if welcome.generation != ENDPOINT_PROTOCOL_GENERATION {
        return Err(format!(
            "Herdr {} speaks endpoint generation {}, not {ENDPOINT_PROTOCOL_GENERATION}",
            welcome.server_version, welcome.generation
        ));
    }
    if let Some(error) = &welcome.error {
        return Err(format!("Herdr refused the connection: {}", error.message));
    }
    if welcome.snapshot_codec != SNAPSHOT_CODEC_V1
        || welcome.surface_codec != SURFACE_CODEC_V1
        || welcome.input_codec != INPUT_CODEC_V1
    {
        return Err(format!(
            "Herdr {} chose codecs this window does not read",
            welcome.server_version
        ));
    }
    Ok(welcome)
}

/// What one message leads to.
enum Step {
    Boot(String),
    Answered(String),
    Surface(Arc<PaneSurfaceFrame>, Arc<SurfaceImages>),
    Event(Event),
}

/// The reader's state: the snapshot and surface it holds, the images, and
/// the answers being put together from their chunks.
#[derive(Default)]
struct Session {
    snapshot: Option<Arc<ClientShellSnapshot>>,
    surface: Option<Arc<PaneSurfaceFrame>>,
    images: Images,
    chunks: HashMap<String, Vec<u8>>,
}

impl Session {
    fn receive(&mut self, message: ServerMessage) -> Result<Vec<Step>, String> {
        let mut steps = Vec::new();
        match message {
            ServerMessage::EndpointControl { kind, data } if kind == ENDPOINT_SNAPSHOT_KIND => {
                let next: ClientShellSnapshot = serde_json::from_str(&data)
                    .map_err(|error| format!("unreadable snapshot: {error}"))?;
                if next.boot_id.is_empty()
                    || self.snapshot.as_ref().is_some_and(|old| {
                        old.boot_id != next.boot_id || next.revision < old.revision
                    })
                {
                    return Err("Herdr restarted under the connection".into());
                }
                if self.snapshot.is_none() {
                    steps.push(Step::Boot(next.boot_id.clone()));
                }
                let next = Arc::new(next);
                self.snapshot = Some(next.clone());
                steps.push(Step::Event(Event::Snapshot(next.clone())));
                // A surface that came before its snapshot shows now.
                if let Some(surface) = self
                    .surface
                    .as_ref()
                    .filter(|surface| surface.projection_revision == next.revision)
                {
                    steps.push(Step::Surface(surface.clone(), self.images.published()));
                }
            }
            // Optional controls this window never asked for.
            ServerMessage::EndpointControl { .. } => {}
            ServerMessage::PaneSurface(mut next) => {
                let snapshot = self
                    .snapshot
                    .as_ref()
                    .ok_or("a surface came before the snapshot")?;
                if next.boot_id != snapshot.boot_id
                    || self
                        .surface
                        .as_ref()
                        .is_some_and(|old| next.surface_revision <= old.surface_revision)
                {
                    return Err("a surface out of order".into());
                }
                next.frame.validate().map_err(|error| error.to_string())?;
                if let Some(popup) = &next.popup {
                    popup.frame.validate().map_err(|error| error.to_string())?;
                }
                self.images.receive(&mut next);
                let next = Arc::new(next);
                if next.projection_revision == snapshot.revision {
                    steps.push(Step::Surface(next.clone(), self.images.published()));
                }
                self.surface = Some(next);
            }
            ServerMessage::PaneSurfacePatch(patch) => {
                let current = self
                    .surface
                    .as_mut()
                    .ok_or("a surface patch came before its surface")?;
                Arc::make_mut(current)
                    .apply_patch(patch)
                    .map_err(|error| error.to_string())?;
                if self
                    .snapshot
                    .as_ref()
                    .is_some_and(|snapshot| snapshot.revision == current.projection_revision)
                {
                    steps.push(Step::Surface(current.clone(), self.images.published()));
                }
            }
            ServerMessage::ClientShellEndpointResponseChunk {
                request_id,
                final_chunk,
                data,
                ..
            } => {
                let total: usize = self.chunks.values().map(Vec::len).sum();
                if data.len() > MAX_RESPONSE_BYTES.saturating_sub(total) {
                    return Err("an endpoint answer over its size limit".into());
                }
                self.chunks
                    .entry(request_id.clone())
                    .or_default()
                    .extend(data);
                if final_chunk {
                    let bytes = self.chunks.remove(&request_id).unwrap_or_default();
                    let response: Value = serde_json::from_slice(&bytes)
                        .map_err(|error| format!("unreadable endpoint answer: {error}"))?;
                    steps.push(Step::Answered(request_id.clone()));
                    steps.push(Step::Event(Event::Response {
                        id: request_id,
                        result: answer(response),
                    }));
                }
            }
            ServerMessage::Clipboard { data } => steps.push(Step::Event(Event::Clipboard(data))),
            ServerMessage::ServerShutdown { reason } => {
                return Err(reason.unwrap_or_else(|| "Herdr shut down".into()));
            }
            ServerMessage::ClientShellError { message } => {
                return Err(format!("Herdr: {message}"));
            }
            _ => {}
        }
        Ok(steps)
    }
}

/// An endpoint answer's result, or Herdr's error.
fn answer(response: Value) -> Result<Value, ApiError> {
    match response.get("error").filter(|error| !error.is_null()) {
        Some(error) => Err(ApiError {
            code: error
                .get("code")
                .and_then(Value::as_str)
                .unwrap_or("herdr_error")
                .to_string(),
            message: error
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
        }),
        None => Ok(response.get("result").cloned().unwrap_or(Value::Null)),
    }
}

/// The writer: holds everything until the boot is known, then writes in
/// order, waiting behind a request until its answer has come.
fn write_loop(mut stream: UnixStream, queue: mpsc::Receiver<Out>, stop: &AtomicBool) {
    let mut writer = Writer::default();
    while let Ok(next) = queue.recv() {
        if stop.load(Ordering::Acquire) {
            return;
        }
        writer.push(next);
        while let Ok(more) = queue.try_recv() {
            writer.push(more);
        }
        for frame in writer.ready() {
            if stream.write_all(&frame).is_err() {
                return;
            }
        }
    }
}

/// The writer's order: the frames it may write now, in the order asked.
#[derive(Default)]
struct Writer {
    boot: Option<String>,
    queue: VecDeque<Out>,
    /// The request whose answer the rest waits for.
    waiting: Option<String>,
}

impl Writer {
    fn push(&mut self, out: Out) {
        match out {
            Out::Boot(boot) => self.boot = Some(boot),
            Out::Answered(id) => {
                if self.waiting.as_deref() == Some(id.as_str()) {
                    self.waiting = None;
                }
            }
            other => self.queue.push_back(other),
        }
    }

    fn ready(&mut self) -> Vec<Vec<u8>> {
        let mut frames = Vec::new();
        let Some(boot) = self.boot.clone() else {
            return frames;
        };
        while self.waiting.is_none() {
            let Some(next) = self.queue.pop_front() else {
                break;
            };
            let message = match next {
                Out::Message(message) => message,
                Out::Request { id, method, params } => {
                    if method != SURFACE_SET {
                        self.waiting = Some(id.clone());
                    }
                    ClientMessage::ClientShellEndpointRequest {
                        boot_id: boot.clone(),
                        request: json!({"id": id, "method": method, "params": params}).to_string(),
                    }
                }
                Out::Boot(_) | Out::Answered(_) => continue,
            };
            if let Ok(frame) = encode_message(&message, MAX_FRAME_SIZE) {
                frames.push(frame);
            }
        }
        frames
    }
}

/// The images the latest surfaces place, by key: each surface brings the
/// bytes of new ones and names the ones it keeps.
#[derive(Default)]
struct Images {
    images: HashMap<SurfaceGraphicsAssetKey, SurfaceImage>,
    bytes: usize,
    published: Option<Arc<SurfaceImages>>,
}

impl Images {
    fn receive(&mut self, surface: &mut PaneSurfaceFrame) {
        let scene = &mut surface.graphics;
        let assets = std::mem::take(&mut scene.assets);
        let referenced: HashSet<&SurfaceGraphicsAssetKey> = scene
            .placements
            .iter()
            .map(|placement| &placement.asset)
            .chain(&scene.retained_assets)
            .collect();
        let before = self.images.len();
        let mut freed = 0;
        self.images.retain(|key, image| {
            let keep = referenced.contains(key);
            if !keep {
                freed += image.data().len();
            }
            keep
        });
        self.bytes -= freed;
        let mut changed = self.images.len() != before;
        for asset in assets {
            if self.images.contains_key(&asset.key)
                || !referenced.contains(&asset.key)
                || !valid_asset(&asset.key, &asset.data)
                || self.images.len() >= MAX_IMAGES
                || asset.data.len() > MAX_IMAGE_BYTES.saturating_sub(self.bytes)
            {
                continue;
            }
            self.bytes += asset.data.len();
            self.images.insert(asset.key, SurfaceImage::new(asset.data));
            changed = true;
        }
        if changed {
            self.published = None;
        }
    }

    fn published(&mut self) -> Arc<SurfaceImages> {
        self.published
            .get_or_insert_with(|| Arc::new(SurfaceImages::new(self.images.clone())))
            .clone()
    }
}

#[cfg(test)]
mod tests;
