use std::{
    io::Read,
    os::unix::net::UnixListener,
    sync::mpsc::{Receiver, RecvTimeoutError},
};

use super::*;
use crate::endpoint::protocol::{
    CellData, ClientKeyCode, ClientKeyKind, CursorState, FrameData, PaneSurfacePane,
    PaneSurfacePatch, PaneSurfacePatchRow, SurfaceGraphicsScene, SurfaceRect, decode_payload,
};

fn snapshot(revision: u64, at: &str) -> ClientShellSnapshot {
    serde_json::from_value(json!({
        "boot_id": "boot", "revision": revision, "config_diagnostic": null,
        "product_announcement": null, "update_available": null,
        "update_install_command": "", "server_keybindings_toml": null,
        "latest_release_notes_available": false, "integration_updates_available": false,
        "worktree_directory": "", "release_notes": null,
        "focused_workspace_id": "w2", "focused_tab_id": at, "focused_pane_id": "w2:p1",
        "tab_bar_right": [], "tab_bar_right_separator": "", "agent_view_label": null,
        "agent_order": [], "workspaces": [], "tabs": [], "panes": [], "agents": [],
        "commands": []
    }))
    .unwrap()
}

fn cell(symbol: &str) -> CellData {
    CellData {
        symbol: symbol.into(),
        fg: 0,
        bg: 0,
        modifier: 0,
        skip: false,
        hyperlink: None,
    }
}

/// A one-pane surface of `text`, one cell per character.
fn surface(projection: u64, revision: u64, text: &str) -> PaneSurfaceFrame {
    let width = text.chars().count() as u16;
    let rect = SurfaceRect {
        x: 0,
        y: 0,
        width,
        height: 1,
    };
    PaneSurfaceFrame {
        boot_id: "boot".into(),
        projection_revision: projection,
        surface_revision: revision,
        frame: FrameData {
            cells: text.chars().map(|c| cell(&c.to_string())).collect(),
            width,
            height: 1,
            cursor: Some(CursorState {
                x: 0,
                y: 0,
                visible: true,
                shape: 0,
            }),
            hyperlinks: Vec::new(),
            graphics: Vec::new(),
        },
        panes: vec![PaneSurfacePane {
            pane_id: "w2:p1".into(),
            content_revision: 1,
            rect,
            inner_rect: rect,
            scrollbar_rect: None,
            scroll: None,
            focused: true,
            mouse_reporting: false,
            sgr_pixel_mouse: false,
            alternate_screen_active: false,
            pixel_width: 0,
            pixel_height: 0,
        }],
        splits: Vec::new(),
        popup: None,
        graphics: SurfaceGraphicsScene::default(),
    }
}

fn snapshot_message(revision: u64, at: &str) -> ServerMessage {
    ServerMessage::EndpointControl {
        kind: ENDPOINT_SNAPSHOT_KIND.into(),
        data: serde_json::to_string(&snapshot(revision, at)).unwrap(),
    }
}

fn text_of(surface: &PaneSurfaceFrame) -> String {
    surface
        .frame
        .cells
        .iter()
        .map(|c| c.symbol.as_str())
        .collect()
}

/// The messages in `frames`, each without its length prefix.
fn messages(frames: &[Vec<u8>]) -> Vec<ClientMessage> {
    frames
        .iter()
        .map(|frame| decode_payload(&frame[4..]).unwrap())
        .collect()
}

/// A request's method, or the message's kind.
fn what(message: &ClientMessage) -> String {
    match message {
        ClientMessage::ClientShellEndpointRequest { request, .. } => {
            let request: Value = serde_json::from_str(request).unwrap();
            request["method"].as_str().unwrap().to_string()
        }
        ClientMessage::ClientShellPaneInput { .. } => "input".into(),
        other => format!("{other:?}"),
    }
}

fn asked(id: &str, method: &str) -> Out {
    Out::Request {
        id: id.into(),
        method: method.into(),
        params: json!({}),
    }
}

fn input() -> Out {
    Out::Message(ClientMessage::ClientShellPaneInput {
        pane_id: "w2:p1".into(),
        events: vec![ClientPaneInputEvent::TextCommit("a".into())],
    })
}

#[test]
fn the_writer_waits_for_the_boot_and_keeps_everything_behind_a_request() {
    let mut writer = Writer::default();
    writer.push(input());
    assert!(
        writer.ready().is_empty(),
        "nothing goes before the boot is known"
    );
    writer.push(Out::Boot("boot".into()));
    writer.push(asked("a", "tab.focus"));
    writer.push(input());
    writer.push(asked("b", "pane.scroll"));
    let sent = messages(&writer.ready());
    // The input typed before the boot, the first request; the rest waits
    // for its answer, in order.
    assert_eq!(
        sent.iter().map(what).collect::<Vec<_>>(),
        ["input", "tab.focus"]
    );
    let ClientMessage::ClientShellEndpointRequest { boot_id, request } = &sent[1] else {
        panic!("a request");
    };
    assert_eq!(boot_id, "boot");
    assert_eq!(
        serde_json::from_str::<Value>(request).unwrap(),
        json!({"id": "a", "method": "tab.focus", "params": {}})
    );
    writer.push(Out::Answered("someone else's".into()));
    assert!(writer.ready().is_empty());
    writer.push(Out::Answered("a".into()));
    assert_eq!(
        messages(&writer.ready())
            .iter()
            .map(what)
            .collect::<Vec<_>>(),
        ["input", "pane.scroll"]
    );
    writer.push(Out::Answered("b".into()));
    // `surface.set` never takes the lease: what follows it goes too.
    writer.push(asked("c", SURFACE_SET));
    writer.push(asked("d", "tab.focus"));
    writer.push(input());
    assert_eq!(
        messages(&writer.ready())
            .iter()
            .map(what)
            .collect::<Vec<_>>(),
        [SURFACE_SET, "tab.focus"]
    );
}

#[test]
fn a_surface_shows_once_its_snapshot_is_current_and_patches_apply_to_it() {
    let mut session = Session::default();
    assert!(
        session
            .receive(ServerMessage::PaneSurface(surface(1, 1, "ab")))
            .is_err(),
        "a surface before any snapshot ends the connection"
    );
    let steps = session.receive(snapshot_message(1, "w2:t1")).unwrap();
    assert!(matches!(&steps[0], Step::Boot(boot) if boot == "boot"));
    assert!(matches!(&steps[1], Step::Event(Event::Snapshot(_))));
    // A surface of an older projection is kept, not shown.
    let mut session = Session::default();
    session.receive(snapshot_message(1, "w2:t1")).unwrap();
    let steps = session
        .receive(ServerMessage::PaneSurface(surface(2, 1, "ab")))
        .unwrap();
    assert!(steps.is_empty());
    // Its snapshot shows it.
    let steps = session.receive(snapshot_message(2, "w2:t1")).unwrap();
    let shown: Vec<_> = steps
        .iter()
        .filter_map(|step| match step {
            Step::Surface(surface, _) => Some(text_of(surface)),
            _ => None,
        })
        .collect();
    assert_eq!(shown, ["ab"]);
    // A patch changes the surface it applies to.
    let steps = session
        .receive(ServerMessage::PaneSurfacePatch(PaneSurfacePatch {
            boot_id: "boot".into(),
            projection_revision: 2,
            base_surface_revision: 1,
            surface_revision: 2,
            rows: vec![PaneSurfacePatchRow {
                x: 1,
                y: 0,
                cells: vec![cell("z")],
            }],
            panes: Vec::new(),
            cursor: None,
        }))
        .unwrap();
    let Step::Surface(patched, _) = &steps[0] else {
        panic!("a patched surface");
    };
    assert_eq!(text_of(patched), "az");
    // A patch on the wrong baseline ends the connection.
    assert!(
        session
            .receive(ServerMessage::PaneSurfacePatch(PaneSurfacePatch {
                boot_id: "boot".into(),
                projection_revision: 2,
                base_surface_revision: 7,
                surface_revision: 8,
                rows: Vec::new(),
                panes: Vec::new(),
                cursor: None,
            }))
            .is_err()
    );
    // So does a new boot: Herdr restarted.
    let mut restarted = snapshot(3, "w2:t1");
    restarted.boot_id = "another".into();
    assert!(
        session
            .receive(ServerMessage::EndpointControl {
                kind: ENDPOINT_SNAPSHOT_KIND.into(),
                data: serde_json::to_string(&restarted).unwrap(),
            })
            .is_err()
    );
}

#[test]
fn answers_are_put_together_from_their_chunks() {
    let mut session = Session::default();
    session.receive(snapshot_message(1, "w2:t1")).unwrap();
    let chunk = |data: &str, last: bool| ServerMessage::ClientShellEndpointResponseChunk {
        boot_id: "boot".into(),
        request_id: "r1".into(),
        final_chunk: last,
        data: data.as_bytes().to_vec(),
    };
    assert!(
        session
            .receive(chunk(r#"{"id":"r1","result":{"type":"#, false))
            .unwrap()
            .is_empty()
    );
    let steps = session
        .receive(chunk(r#""pane_selection","text":"hi"}}"#, true))
        .unwrap();
    assert!(matches!(&steps[0], Step::Answered(id) if id == "r1"));
    let Step::Event(Event::Response { id, result }) = &steps[1] else {
        panic!("a response");
    };
    assert_eq!(id, "r1");
    assert_eq!(result.as_ref().unwrap()["text"], "hi");
    let steps = session
        .receive(ServerMessage::ClientShellEndpointResponseChunk {
            boot_id: "boot".into(),
            request_id: "r2".into(),
            final_chunk: true,
            data: br#"{"id":"r2","error":{"code":"endpoint_busy","message":"busy"}}"#.to_vec(),
        })
        .unwrap();
    let Step::Event(Event::Response { result, .. }) = &steps[1] else {
        panic!("a response");
    };
    assert_eq!(
        result.as_ref().unwrap_err(),
        &ApiError {
            code: "endpoint_busy".into(),
            message: "busy".into()
        }
    );
}

#[test]
fn a_program_s_clipboard_write_arrives_decoded() {
    let written = |data: &str| match Session::default()
        .receive(ServerMessage::Clipboard { data: data.into() })
        .unwrap()
        .as_slice()
    {
        [Step::Event(Event::Clipboard(text))] => Some(text.clone()),
        [] => None,
        _ => panic!("one clipboard write at most"),
    };
    // What Claude Code's copy sent: Herdr base64-encodes the OSC 52 bytes.
    assert_eq!(
        written("Vz1+Ly5oZXJkci93b3JrdHJlZXMvY29yZ2kv").as_deref(),
        Some("W=~/.herdr/worktrees/corgi/")
    );
    assert_eq!(
        written("aMOpbGxv4pyTCg==").as_deref(),
        Some("h\u{e9}llo\u{2713}\n")
    );
    // Not base64, or not text: nothing reaches the clipboard.
    assert_eq!(written("not base64!"), None);
    assert_eq!(written("/w=="), None);
}

/// Reads one framed message from `stream`, as Herdr's server does.
fn read_client(stream: &mut UnixStream) -> ClientMessage {
    let mut prefix = [0; 4];
    stream.read_exact(&mut prefix).unwrap();
    let mut payload = vec![0; u32::from_le_bytes(prefix) as usize];
    stream.read_exact(&mut payload).unwrap();
    decode_payload(&payload).unwrap()
}

fn send(stream: &mut UnixStream, message: &ServerMessage) {
    stream
        .write_all(&encode_message(message, MAX_GRAPHICS_FRAME_SIZE).unwrap())
        .unwrap();
}

fn next_event(events: &Receiver<Event>) -> Event {
    match events.recv_timeout(Duration::from_secs(5)) {
        Ok(event) => event,
        Err(RecvTimeoutError::Timeout) => panic!("no event within five seconds"),
        Err(RecvTimeoutError::Disconnected) => panic!("the connection's events ended"),
    }
}

#[test]
fn a_connection_says_hello_and_talks_to_a_fake_endpoint() {
    // A short path: macOS limits a socket path to 104 bytes.
    let socket = std::env::temp_dir().join(format!("cg-ep-{}.sock", std::process::id()));
    let _ = std::fs::remove_file(&socket);
    let listener = UnixListener::bind(&socket).unwrap();
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream.set_nonblocking(false).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let ClientMessage::EndpointControl { kind, data } = read_client(&mut stream) else {
            panic!("the hello first");
        };
        assert_eq!(kind, ENDPOINT_HELLO_KIND);
        let hello: EndpointClientHello = serde_json::from_str(&data).unwrap();
        assert_eq!(hello.generation, 1);
        assert!(hello.surface_active);
        assert!(!hello.endpoint_keybindings, "every key is the pane's");
        assert_eq!(hello.surface_size, ClientSurfaceSize { cols: 80, rows: 24 });
        let welcome = EndpointServerWelcome {
            generation: 1,
            server_version: "0.9.0".into(),
            snapshot_codec: SNAPSHOT_CODEC_V1.into(),
            surface_codec: SURFACE_CODEC_V1.into(),
            input_codec: INPUT_CODEC_V1.into(),
            blob_codec: BLOB_CODEC_V1.into(),
            methods: vec!["tab.focus".into()],
            capabilities: Vec::new(),
            error: None,
        };
        send(
            &mut stream,
            &ServerMessage::EndpointControl {
                kind: ENDPOINT_WELCOME_KIND.into(),
                data: serde_json::to_string(&welcome).unwrap(),
            },
        );
        send(&mut stream, &snapshot_message(1, "w1:t1"));
        send(
            &mut stream,
            &ServerMessage::PaneSurface(surface(1, 1, "hi")),
        );
        // The request, with the boot the snapshot named; then the input
        // queued behind it only once it is answered.
        let ClientMessage::ClientShellEndpointRequest { boot_id, request } =
            read_client(&mut stream)
        else {
            panic!("the request");
        };
        assert_eq!(boot_id, "boot");
        let request: Value = serde_json::from_str(&request).unwrap();
        assert_eq!(request["method"], "tab.focus");
        let id = request["id"].as_str().unwrap().to_string();
        stream
            .set_read_timeout(Some(Duration::from_millis(200)))
            .unwrap();
        let mut byte = [0; 1];
        assert!(
            stream.read(&mut byte).is_err(),
            "nothing comes while the request is unanswered"
        );
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        send(
            &mut stream,
            &ServerMessage::ClientShellEndpointResponseChunk {
                boot_id: "boot".into(),
                request_id: id.clone(),
                final_chunk: true,
                data: json!({"id": id, "result": {"type": "tab_info"}})
                    .to_string()
                    .into_bytes(),
            },
        );
        let ClientMessage::ClientShellPaneInput { pane_id, events } = read_client(&mut stream)
        else {
            panic!("the input");
        };
        assert_eq!(pane_id, "w2:p1");
        assert_eq!(
            events,
            vec![ClientPaneInputEvent::Key {
                code: ClientKeyCode::Enter,
                modifiers: 1,
                kind: ClientKeyKind::Press,
                repeat_count: 1,
                shifted_codepoint: None,
                generated_text: None,
                tracks_release: false,
                physical_key_id: None,
                windows_record: None,
            }]
        );
    });

    let (sender, events) = mpsc::channel();
    let sender = Mutex::new(sender);
    let geometry = Geometry {
        size: ClientSurfaceSize { cols: 80, rows: 24 },
        cell_width_px: 8,
        cell_height_px: 17,
    };
    let connection = Connection::open(socket.clone(), geometry, true, move |event| {
        let _ = lock(&sender).send(event);
    });
    let id = connection.request("tab.focus", json!({"tab_id": "w2:t1"}));
    connection.input(
        "w2:p1",
        vec![ClientPaneInputEvent::Key {
            code: ClientKeyCode::Enter,
            modifiers: 1,
            kind: ClientKeyKind::Press,
            repeat_count: 1,
            shifted_codepoint: None,
            generated_text: None,
            tracks_release: false,
            physical_key_id: None,
            windows_record: None,
        }],
    );
    assert!(matches!(next_event(&events), Event::Welcome(w) if w.server_version == "0.9.0"));
    assert!(
        matches!(next_event(&events), Event::Snapshot(s) if s.focused_tab_id.as_deref() == Some("w1:t1"))
    );
    assert!(matches!(next_event(&events), Event::Surface));
    let (shown, _) = connection.take_surface().unwrap();
    assert_eq!(text_of(&shown), "hi");
    assert!(connection.take_surface().is_none(), "taken once");
    match next_event(&events) {
        Event::Response {
            id: answered,
            result,
        } => {
            assert_eq!(answered, id);
            assert_eq!(result.unwrap()["type"], "tab_info");
        }
        other => panic!("expected the answer, got {other:?}"),
    }
    server.join().unwrap();
    // The fake hung up: the connection says so, once.
    assert!(matches!(next_event(&events), Event::Ended(_)));
    drop(connection);
    let _ = std::fs::remove_file(&socket);
}

#[test]
fn a_connection_that_cannot_connect_says_why() {
    let (sender, events) = mpsc::channel();
    let sender = Mutex::new(sender);
    let geometry = Geometry {
        size: ClientSurfaceSize { cols: 80, rows: 24 },
        cell_width_px: 8,
        cell_height_px: 17,
    };
    let _connection = Connection::open(
        std::env::temp_dir().join("cg-no-such.sock"),
        geometry,
        false,
        move |event| {
            let _ = lock(&sender).send(event);
        },
    );
    match next_event(&events) {
        Event::Ended(why) => assert!(why.contains("cannot connect"), "{why}"),
        other => panic!("expected the end, got {other:?}"),
    }
}
