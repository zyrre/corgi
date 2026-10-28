# A desktop GUI for Corgi on the Herdr backend

Research study, October 2026, against Herdr 0.9.0 (installed) and its source
at tag `v0.9.0` (current stable is 0.9.5). It answers what a desktop GUI for
Corgi could cover, what Herdr offers it, which stack fits macOS and Linux, how
to split Corgi's code, and what to build first. Nothing here is built.

Evidence marks used below:

- **[spike]**: run against a separate Herdr session (`herdr --session
  corgigui-spike`, stopped and deleted afterwards); the user's live session
  was only read (`herdr api snapshot`).
- **[source]**: read in Herdr's source at `v0.9.0`
  ([herdrdev/herdr](https://github.com/herdrdev/herdr), Apache-2.0); paths
  are relative to that repository.
- **[docs]**: Herdr's documentation as published in that repository
  (`docs/next/website/src/content/docs/`, served at herdr.dev/docs).
- **[spike 2]**: a standalone endpoint generation 1 client run against a
  second separate session (`endpointspike`, also stopped and deleted), §8.
- **[spike 3]**: an egui prototype and protocol follow-ups, each against its
  own separate session (`guiproto`, `endpointspike2`, both deleted), §9.
- **[spike 4]**: a GPUI prototype using Herdr UI's painter, session
  `gpuiproto` (deleted), §10.
- **[web]**: third-party sources, linked; **[assumption]**: not verified.

## Summary

- Herdr already gives a GUI everything it needs except a documented
  message-level spec for its own UI protocol. Lifecycle events are pushed, not
  polled. Pane screens can be streamed, and input and resize can be sent.
  Herdr's own UI is a "client-owned shell" over a stable, versioned endpoint
  protocol, so a third-party client can draw panes exactly as Herdr's does.
- A full Herdr replacement already exists: **Herdr UI**
  ([penso/herdr-gpui](https://github.com/penso/herdr-gpui), Rust + GPUI,
  Apache-2.0, about 1.7k stars, macOS/Linux/Windows). It paints Herdr's own
  cells, with no second terminal emulator. Corgi should not rebuild that.
- **Update after the prototypes (§9–§10):** the terminal must sit inside the
  GUI, with Herdr's endpoint protocol feeding it. Both egui and GPUI prototypes
  are fast enough. GPUI can reuse Herdr UI's terminal painter; egui is more
  mature and more responsive. The open decision is in Open questions.
- **Recommendation (original):** a hybrid GUI focused on what only Corgi has. That means
  project cards, the supervisor, merges, the inbox, decisions, ledger, briefs
  and reports. Agent terminals open on demand inside the GUI later, and until
  then "focus" hands off to Herdr's TUI or Herdr UI. Build it in Rust with
  **egui/eframe**. It talks to a new `corgi serve` JSON-lines endpoint that runs
  on the Herdr host, locally or over `ssh`.
- **First milestone:** `corgi serve` plus a GUI dashboard without embedded
  terminals. That is about 3.5–5 worker-weeks for one or two workers.
- **Terminals in the GUI:** use endpoint generation 1, behind one adapter.
  A spike showed it encodes Shift+Enter and other keys correctly for each pane's
  keyboard mode; the documented bridge cannot (§8). The bridge's `observe` stays
  the fallback, and the way to show read-only previews.

## 1. Scope options

| Option | What it means for you | Cost | Overlap |
| --- | --- | --- | --- |
| **a. Corgi-only GUI** | A window with the dashboard, supervisor conversation, merges, reports, and a browser for decisions and memory. Terminals stay in Herdr's TUI; `f` focuses the pane there, and you switch windows yourself unless Corgi raises the terminal (it can on Hyprland, `src/app/navigate.rs`). | Smallest. No terminal rendering. | None. |
| **b. Full frontend** | Replaces Herdr's TUI: workspaces, tabs, split panes, every terminal drawn by the GUI on top of the Herdr server. | Largest: endpoint protocol, cell renderer, layout, mouse, selection, graphics, copy mode. | Herdr UI already does exactly this. |
| **c. Hybrid** | The Corgi GUI from (a), plus a terminal pane for the selected agent, opened on demand next to its card. Prompting, answering a question or watching a run happen without leaving Corgi. Herdr's TUI or Herdr UI remains the full workspace view. | (a) plus one terminal widget, without Herdr's layout and tab model. | Small. |

Option (b) would duplicate Herdr UI. That app is growing its own
orchestration crates too (`herdr-orchestrator`, `herdr-vcs`, `herdr-usage`,
`herdr-github` in its workspace), so it will overlap Corgi from the other side.
(c) gives the room asked for and keeps Corgi's identity. Start it as (a) and add
terminals through the endpoint path compared in §8.

## 2. What Herdr's backend exposes

Herdr has two sockets per session. Both are created with mode `0600`
(`src/server/socket_paths.rs:12`) **[source]**:

- `herdr.sock` is the JSON API: newline-delimited JSON requests and responses,
  plus long-lived subscriptions. Corgi uses only this one today (`src/herdr.rs`).
- `herdr-client.sock` is the UI client protocol: u32-length-prefixed bincode
  frames (`src/protocol/wire.rs:20-35`, `:1602`) **[source]**. Herdr's TUI,
  `herdr --remote`, saved SSH machines and the terminal bridges below use it.

`herdr api schema --json` prints the JSON API's JSON Schema: protocol 22,
schema version 1, 102 request methods **[spike]**.

| Need | What Herdr has | Evidence |
| --- | --- | --- |
| Listing | `session.snapshot` and `workspace/tab/pane/agent/worktree.list`. On the live session (19 workspaces, 19 panes) a snapshot is 43.6 KB and takes about 10 ms. | [spike] |
| Events | `events.subscribe` keeps the connection open and pushes events. Subscriptions cover `workspace.*`, `tab.*`, `pane.created/updated/closed/focused/moved/exited/agent_detected/scroll_changed`, `pane.agent_status_changed` (per pane), `pane.output_matched` (substring or regex, per pane), `layout.updated` and `worktree.*`. A split, a new tab and a printed marker produced `pane_created`, `layout_updated`, `tab_created` and `output_matched` within about 0.1 s. Events from before the subscription are not replayed. | [spike], [docs] `socket-api.mdx` "Event subscriptions" |
| Pane output, snapshots | `pane.read` / `agent.read`: `visible`, `recent`, `recent_unwrapped` or `detection`, as `text` or `ansi`. This is what Corgi polls today. | [docs], `src/herdr.rs` |
| Pane output, streaming | Not on the JSON socket. `herdr terminal session observe <pane> --cols N --rows N` streams JSON lines `{"type":"terminal.frame","seq","width","height","full","bytes":<base64 ANSI>}` and finally `terminal.closed`. A pane printing about 50 coloured lines a second gave 91 frames in 4 s (one full, the rest diffs), 25 KB/s, frames ≤ 2.5 KB. **The frames are Herdr's re-rendered cell diffs** (cursor moves, SGR, synchronized output), not the program's own bytes, so any small VT parser can apply them. Many observers may watch one pane. | [spike], [docs] `cli-reference.mdx` "Direct terminal attach", [source] `src/client/terminal_sessions.rs:122-157`, `src/protocol/render_ansi.rs:1-27` |
| Input | JSON: `pane.send_text`, `pane.send_keys`, `pane.send_input`, `agent.prompt` (bracketed-paste aware, optional wait). Bridge: `herdr terminal session control <pane>` reads `terminal.input` (text or base64 bytes), `terminal.resize`, `terminal.scroll` and `terminal.release` on stdin. Typed input echoed back in a frame within 1.3 ms locally. Control-mode bytes go **raw** to the PTY; only a complete bracketed paste is re-encoded for the pane's mode. | [spike], [source] `src/server/pane_input.rs:187-201` |
| Resize | `pane.resize` moves splits. A `control` session's viewport **resizes the PTY**: `stty size` read `30 100`, then `20 90` after `terminal.resize`, while Herdr's layout still said 60×40. The size stayed after release (headless session, no TUI attached). An observer never resizes; it gets frames at its own requested size (50×10 here), not the pane's. When a client-owned shell is attached, it reclaims the size once the `control` session ends **[spike 2]**. With several UI clients on one tab, "the last one to focus, select, or interact with it controls that tab's pane sizes". | [spike], [docs] `concepts.mdx` "Client and server" |
| Create | `workspace.create`, `tab.create`, `pane.split`, `agent.start`, `worktree.create/list/open/remove`, `layout.apply`. Corgi already uses most of them. | [docs], `src/herdr.rs` |
| Notifications | `notification.show` raises Herdr's toast. OS notifications would be the GUI's own job, driven by `pane.agent_status_changed`. | [docs] |
| Attach like Herdr's client | Yes. Herdr's UI is a **client-owned shell** speaking "endpoint generation 1". Over `herdr-client.sock`, the client sends a JSON hello (`endpoint.hello.v1` in an `EndpointControl` frame; the older `ClientShellHello` is rejected) and receives a JSON welcome (38 navigation and topology methods, no read methods), a JSON snapshot (`shell.snapshot.v1`) and pane surface frames (`shell.surface.v1`: cells, patches, cursor, graphics). A standalone client was built in about 730 lines of Rust **[spike 2, §8]**. It sends **semantic** key, mouse and paste events (`shell.input.semantic.v1`), which the server encodes for each pane with its ghostty-vt encoder, kitty keyboard protocol included. Capabilities include `surface_interest` (stream only the selected machine's panes) and `health_check`. | [source] `src/protocol/endpoint.rs`, `src/protocol/wire.rs:470-625`, `:1081-1271`, `src/server/pane_input.rs:292-318`, `src/pane/terminal.rs:607` |
| Stability | Endpoint generation 1 "must remain available indefinitely unless retired for a security reason"; new fields are optional and new enum values get an `Unknown` fallback. Client and server builds need not match. The numbered protocol (22) is private: same-install only, and it also covers raw `terminal attach`. JSON clients must ignore unknown fields and treat a missing method as a normal error. | [source] `src/protocol/endpoint.rs:1-9`, [docs] `socket-api.mdx` "Protocol stability" |

**Which path Herdr recommends.** The docs point third parties at the JSON API
and, for terminals, at the bridges: "For third-party bridges that only need
rendered terminal bytes, use a read-only terminal session observer… For an
interactive bridge, use a writable terminal session controller"
(`persistence-remote.mdx`). Endpoint generation 1 is described only as what
"the client-rendered Herdr UI uses". Outside clients such as Herdr UI use it
without Herdr's endorsement. No maintainer statement either way was found in
Herdr's Discussions **[web]**.

**Corgi's current features against Herdr.** Corgi calls 31 distinct
JSON-API methods. All of them stay available to a GUI unchanged, and none
needs the endpoint protocol or the bridges.

| Feature | Herdr calls | In a GUI |
| --- | --- | --- |
| Rows, cards, plan usage, transcript tail | `session.snapshot`, `agent.read`, `pane.process_info`, `workspace/worktree.list`; session files | Unchanged |
| Prompt, inbox wakes, draft detection, first prompt | `agent.prompt`, `pane.send_input`, `agent.send_keys`, `agent.read` (styled) | Unchanged |
| New agent, spawn, supervisor, handover | `workspace.create`, `worktree.create`, `tab.create`, `agent.start`, `*.report_metadata`, `workspace.rename` | Unchanged |
| Merge, close, edit, recovery | Git; `workspace.close`, `worktree.remove/open`, `tab.create/close`, `notification.show`, `server.reload_config` | Unchanged |
| Focus (`f`/Enter) | `agent/workspace/tab/pane.focus`, then Hyprland raises the terminal window (`src/app/navigate.rs`) | The JSON `pane.focus` moves **every** attached UI client, the GUI included if it is one (`client_views.rs:165`) **[spike 2]**. Use it to send Herdr's window to an agent; move only the GUI's own view through its endpoint connection. |
| Running inside a Herdr pane | `HERDR_SOCKET_PATH`/`HERDR_PANE_ID` (`src/herdr.rs:58`, `src/app/mod.rs:231`), `pane.current`, `plugin.pane.open/focus`, the `corgi setup` key binding | **Replaced**: find sockets with `herdr session list`, no own pane to hide, no plugin popup. |
| Colours | Terminal query on `/dev/tty`, answered by Herdr (`src/theme/resolve.rs:211`) | **Replaced** by the system and Omarchy readers (§4) |
| *New:* a live terminal in the GUI | — | Endpoint or bridge (§8) |

What is missing or awkward:

1. **No message-level specification of endpoint generation 1.** It is a
   compatibility promise in a source comment plus Rust types. A client must
   mirror those types, as Herdr UI does in its own `herdr-protocol` crate
   (Apache-2.0, unpublished; a git dependency or a vendored copy).
   kcosr/herdr-web vendors Herdr types and pins protocol 22
   ([README](https://github.com/kcosr/herdr-web)), which breaks on upgrades
   **[web]**.
2. **The `control` bridge cannot carry modern key encoding.** Input is raw
   bytes, and the frames do not tell the client which keyboard, mouse or paste
   modes the program has enabled. A client therefore cannot send the kitty
   keyboard protocol correctly; Shift+Enter in Claude Code is the obvious
   casualty **[source; confirmed by spike 2, §8]**. Mouse events on `control` were added after 0.9.0 (#4685) **[source:
   git log]**.
3. **One PTY, one size.** A GUI terminal and Herdr's TUI showing the same pane
   at different sizes will fight over it. The GUI should take control only
   while it has focus and release it afterwards, or use observe mode at the
   pane's real size.
4. **Upstream does not accept unsolicited pull requests**; feature requests go
   to GitHub Discussions (`CONTRIBUTING.md`) **[source]**. A documented
   endpoint spec, or semantic input on `terminal session control`, can be
   asked for there. Plan on not getting either.

## 3. Remote use

Herdr's own remote paths **[docs]** `persistence-remote.mdx`,
`connecting-machines.mdx`, **[source]** `src/remote/attach.rs`:

- `herdr --remote <ssh-target>` runs a local UI against a remote server.
  `ssh target herdr … remote-client-bridge` carries `herdr-client.sock` over
  SSH stdio. It uses your OpenSSH config and agent, adds keepalives, and on
  macOS and Linux reuses a private ControlMaster socket. Herdr installs or
  updates a compatible remote binary only after asking.
- `herdr machine add` saves SSH machines, and one Herdr window shows them all.
  Each reconnects with bounded backoff, and quiet links are probed via
  `health_check`. Only the selected machine streams pane screens. Disconnected
  machines stay visible, dimmed and read-only. Profiles store no secrets.

What this means for a Corgi GUI on a laptop with Herdr on another machine:

- **Corgi's logic must run on the Herdr host.** It reads agent transcripts
  (`~/.claude`, `~/.codex`), runs Git in worktrees, holds its state directory,
  and its waker must survive the laptop sleeping. So the GUI should be a thin
  client of a Corgi process on that host, not of Herdr's socket directly.
- **Transport: SSH stdio, with no listening port.** The GUI spawns
  `corgi serve` locally or `ssh host corgi serve` remotely and speaks JSON
  lines over the pipe. Terminal panes use `ssh host herdr terminal session
  control …`, or, with the endpoint protocol, the remote `herdr-client.sock`
  over the same stdio bridge Herdr uses. One ControlMaster connection can
  multiplex all of them.
- **Unix socket forwarding** (`ssh -L local.sock:remote/herdr.sock`) also
  works and is what herdrm uses ([README](https://github.com/missuo/herdrm))
  **[web]**. It only reaches Herdr, though, not Corgi's files, so it suits
  option (b), not Corgi.
- **Security:** OpenSSH authentication, sockets at `0600`, nothing exposed on
  the network, and no telemetry, which keeps Corgi's current promises. Never
  bridge either socket to TCP or WebSocket.
- **Latency and reconnects:** the stream is small (25 KB/s for a busy 80×24
  pane), and echo was 1.3 ms locally **[spike]**. Over a real SSH link it was
  not measured, because no SSH target was available **[assumption: one RTT per
  keystroke, as with `ssh` itself]**. The GUI must reconnect with backoff, show
  cached state dimmed while disconnected, as Herdr does, and never retry an
  unconfirmed prompt.

## 4. Stacks for macOS and Linux

Requirements: Wayland (Hyprland) and X11 on Linux, plus macOS; keyboard first;
system light/dark; Rust reuse; possibly many terminal panes later. Sizes marked
\* are estimates. Also considered and set aside: Dioxus (desktop is the same
WebKitGTK webview as Tauri; its native renderer is experimental), Qt via cxx-qt
(C++/QML and Qt licensing for little Rust reuse), and Makepad, Xilem and Floem
(immature, no terminal widget).

| Stack | Terminal path | Keyboard / IME | System scheme | Accessibility | Size | Packaging | Maturity | Rust reuse |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| **egui / eframe** 0.37 | Own cell-grid widget (egui_term is immature) | Raw press/release, IME ok | macOS via winit; Linux needs a portal reader | AccessKit | 5–15 MB\* | cargo-packager: .app/DMG, deb, AppImage, pacman. Notarize yourself | Mature, frequent releases | Highest; `egui_ratatui` can host ratatui widgets |
| **GPUI** (Zed) | Zed-proven alacritty grid; Herdr UI paints endpoint cells | Good; Wayland IME being fixed | Manual | Experimental | 15–30 MB\* | DIY, as Herdr UI does: brew tap, deb/rpm/pacman | crates.io `gpui` stale (0.2.2); Herdr UI pins `gpui-pre =0.3.6`; upstream "Zed's needs first" | High |
| **iced** 0.14 | iced_term (alacritty, maintained) | IME improved | Portal via `mundy` | None | 10–20 MB\* | cargo-packager | Slow releases | High |
| **Slint** 1.16 | None; DIY | Shortcuts API | Portal accent on Linux | AccessKit | 5–10 MB\* | cargo-packager | Stable; GPLv3 or royalty-free licence | Medium (DSL) |
| **Tauri 2** | xterm.js / ghostty-web | Browser-grade; xterm.js kitty keys only in 6.1+/7 | Linux `prefers-color-scheme` unreliable | Browser | 3–10 MB (AppImage ~76 MB) | Best bundler: DMG, notarization, deb, rpm, AppImage | Stable, but Linux WebKitGTK is fragile (Wayland, NVIDIA DMABUF, slow WebGL); the CEF runtime only exists in Tauri 3 alpha | Backend only |
| **Electron** | xterm.js (VS Code-proven) | Good; Wayland IME behind a flag | Good | Good | 85–100 MB | electron-builder | Very mature | Low (sidecar or napi) |
| **GTK4 + VTE** | VTE, mature and fast | Native, but **no kitty keyboard protocol** | Native on Linux, weak on macOS | AT-SPI | Small + GTK | Flatpak great, macOS poor | Mature | Medium |

Terminal engines **[web]**:

- **alacritty_terminal** 0.26: on crates.io, used by Zed and Warp. It has no
  key encoder, so the embedder writes the kitty and legacy encoders.
- **libghostty-vt**: Rust bindings 0.2, "API not yet stable", needs Zig 0.17.
  It ships key and mouse encoders. Herdr itself is built on it (`ghostty-vt`
  in Herdr's `Cargo.toml`) **[source]**.
- **wezterm-term**: not on crates.io.
- **vt100**: enough for snapshots.
- **xterm.js**: limited by browsers to about 16 live WebGL contexts, so busy
  dashboards give WebGL only to visible panes.
- **VTE**: has no kitty keyboard support.

Sources **[web]**: [alacritty_terminal](https://crates.io/crates/alacritty_terminal),
[libghostty-vt](https://docs.rs/libghostty-vt),
[Tauri Linux graphics](https://v2.tauri.app/develop/debug/linux-graphics/),
[Tauri CEF](https://v3.tauri.app/develop/cef/),
[egui](https://github.com/emilk/egui/releases),
[gpui](https://crates.io/crates/gpui),
[VTE kitty](https://gitlab.gnome.org/GNOME/vte/-/issues/2601),
[xterm.js WebGL](https://github.com/rdfitted/hive-manager/issues/282),
[cargo-packager](https://github.com/crabnebula-dev/cargo-packager).

**Recommendation: egui/eframe.** Corgi's GUI is mostly lists, cards, forms and
text, driven from the keyboard, which suits egui's immediate mode well. It
keeps everything in Rust with one dependency tree, has AccessKit, and is the
lowest-risk path on Hyprland and macOS. Herdr's endpoint protocol delivers
ready-made cells, so the terminal widget is a cell painter, not an emulator. On
the bridge path it is a small ANSI applier over `vt100` or alacritty_terminal.
That removes the main reason to pick a terminal-centric toolkit.

**Alternative: GPUI.** Choose it if you want to converge with Herdr UI: reuse
its crates, look like it, or merge efforts. Its costs are a pinned pre-release
GPUI, experimental accessibility, and an upstream focused on Zed.

Avoid Tauri, Dioxus desktop and GTK+VTE as the primary stack, for the
WebKitGTK and kitty-keyboard reasons above. Avoid Electron to keep Rust reuse
and size.

Theming in either Rust toolkit:

- Reuse Corgi's palette resolution: Omarchy `alacritty.toml`, then the system
  preference (macOS `defaults`, the freedesktop portal, `gsettings`), then
  Catppuccin.
- Drop the OSC terminal query (a GUI has no tty).
- Watch the portal's `SettingChanged` signal and the Omarchy theme file so
  colours follow live.
- Brand colours stay fixed.

## 5. Architecture: one core, several frontends

The code is closer to this split than it looks. Outside `src/ui/`,
`src/motion.rs`, `src/theme/mod.rs`, `src/app/mod.rs` and `src/app/bar.rs`,
no non-test code uses ratatui. Neither the refresh pipeline nor the waker
touches `App`.

**Already core (no ratatui, no `App`):**

- `herdr.rs`, `model.rs`, `activity.rs`, `harness.rs`, `session/`,
  `supervisor.rs`
- `inbox.rs`, `digest/`, `usage.rs`, `usage_cache.rs`, `projects.rs`
- `choices.rs`, `defaults.rs`, `git.rs`, `paths.rs`, `time.rs`
- `recovery/`, except that `recovery/mod.rs:460`, `:1147` and `:1240` call
  back into `app/recovery.rs:649-710`. That dependency has to be inverted.

**Core in all but location (under `src/app/`, no TUI):**

- `waker.rs`, `markers.rs`, `rows.rs`, `draft.rs`, `first_prompt.rs`
- `sweep.rs`, `notice.rs`, `progress.rs`, `setup.rs`
- `RefreshState::run`/`refresh` (`refresh.rs:143-212`)

**Mixed (`impl App` flows with crossterm keys inside):**

- `form.rs`, `launch.rs`, `merge.rs`, `close.rs`, `prompt.rs`, `cards.rs`,
  `app/recovery.rs`, `catalog.rs`
- Their real work is already in free functions: `launch::launch_agent`
  (`launch.rs:387`), the merge Git steps (`merge.rs:436-550`) and
  `ModelCatalogs` (`catalog.rs:49-60`).
- Lift those out, and leave the form state machines and key handling in each
  frontend.

**TUI-bound:**

- `src/ui/*`, `motion.rs`, the event loop in `app/mod.rs:521-738`
- `theme/mod.rs`, whose `Roles` are ratatui `Color`s. Split it into a core
  palette with RGB roles plus a ratatui mapping. `Theme::rgb` (`:391-402`)
  already does the conversion.

**Existing second frontend:** the Omarchy widget reads `corgi --bar-stream`
(a JSON snapshot every 2 s) and acts through `--bar-*` commands
(`src/app/bar.rs`). Its JSON is serialised ratatui spans (`bar.rs:58-97`), so
it is presentation, not data. It is the model to generalise.

**Proposed shape:**

```text
corgi-core (lib)   Herdr client, rows/model, sessions, supervisor, waker,
                   markers, inbox, digest, launch/merge/close, palette
corgi              CLI + ratatui dashboard (unchanged behaviour)
corgi serve        JSON-lines over stdio: snapshot + deltas of structured rows,
                   commands (prompt, spawn, merge, close, tag, focus, digest…)
corgi-gui          egui app; spawns `corgi serve` locally or via `ssh host`
```

**Where the background work lives.** Each process that runs the refresh
pipeline already contends for a per-socket file lock
(`$STATE/wake/<socket>.lock`, `waker.rs:267-275`, `:982-984`). Only the holder
reconciles markers, wakes supervisors, delivers the inbox and runs handovers.
Another process takes over when the holder exits, and it replays from
`wake/<socket>.seen.json`. So:

- **`corgi serve` runs the same refresh pipeline** and contends like a
  dashboard does. The GUI, the TUI and the bar can run in any combination
  without new coordination. The waker needs an explicit socket
  (`HerdrClient::from_socket_path`) rather than `HERDR_SOCKET_PATH`
  (`herdr.rs:57-63`).
- **An optional `corgi daemon`** (`corgi serve` with no client) keeps wakes
  flowing when no frontend is open. Today nothing wakes then
  (ARCHITECTURE.md, "The supervisor"). It would run as a launchd or systemd user
  unit, or start lazily from `corgi spawn`/`corgi supervisor`. This reverses
  the documented "no daemon" stance, so it is your call (Open questions).
- **The GUI holds no state of its own** beyond view preferences. The
  `selection/<socket>.<pid>` file used by `corgi edit` becomes a `serve`
  command.
- Moving the Omarchy widget onto `corgi serve`'s structured rows later removes
  the ratatui-span JSON.

**Herdr events** (planned in ARCHITECTURE.md "Future extensions") would let
`corgi serve` push changes at once instead of polling. It is optional, because
a snapshot costs about 10 ms.

## 6. Prior art

| Project | Stack | Licence | Terminals or chat | Remote | Note |
| --- | --- | --- | --- | --- | --- |
| [Herdr UI (herdr-gpui)](https://github.com/penso/herdr-gpui) | Rust + GPUI | Apache-2.0 | Herdr's cells via endpoint protocol | Saved machines | Closest to option (b); adding orchestration features |
| [herdrm](https://github.com/missuo/herdrm) | SwiftUI + libghostty, macOS | PolyForm NC | `herdr agent attach` PTYs | `ssh -L` socket forwarding | Transcript viewer |
| [herdr-web](https://github.com/kcosr/herdr-web) | Rust bridge + React + ghostty-web | MIT | Herdr frames over WebSocket | Browser | Pins protocol 22; asks upstream for a public bridge |
| [cmux](https://github.com/manaflow-ai/cmux) | Swift + libghostty | GPL-3.0 | Real terminals | SSH | Attention rings from OSC notifications |
| [Claude Squad](https://github.com/smtg-ai/claude-squad) | Go TUI + tmux | AGPL-3.0 | Real terminals | No | Worktree per session |
| [Pane](https://github.com/greenfield-inc/Pane) (Crystal fork) | Electron + xterm.js | AGPL-3.0 | Terminals | SSH/Tailscale | Merge, rebase, squash built in |
| [Conductor](https://conductor.build) | Tauri, macOS only | Closed | Chat (SDK) + terminal panel | Cloud | Broad GitHub scopes criticised; chat "loses the feel" |
| [vibe-kanban](https://github.com/BloopAI/vibe-kanban) | Rust + React web | Apache-2.0 | Kanban + logs | Proxy | Company sunset it; community-maintained |
| [opcode](https://github.com/winfunc/opcode) | Tauri 2 + React | AGPL-3.0 | Chat over CLI | No | Stalled since Oct 2025 |
| Claude Code desktop, Codex app, Cursor agents | Electron | Closed | Chat | SSH or cloud | State drift with many sessions; Mac-first launches criticised |

All of these are **[web]**, gathered October 2026. Herdr UI's licence, stack
and protocol use were checked against its repository. Herdr's own comparison
page positions it as a runtime that keeps agents alive while every window is
closed, unlike Conductor or Superset ([herdr.dev/compare](https://herdr.dev/compare/)).

Lessons:

1. **Real terminals beat chat wrappers** for CLI agents. Subscription logins
   only work in the official CLIs
   ([The Register](https://www.theregister.com/software/2026/02/20/anthropic-clarifies-ban-on-third-party-tool-access-to-claude/5014546)),
   and wrappers lag new CLI features. Corgi's structured views add to the
   terminal; they do not replace it.
2. **Let the runtime own processes.** The GUI is disposable; Herdr and Corgi's
   state survive it. Corgi already works this way.
3. **Protocol coupling is the main technical risk.** herdr-web broke by
   pinning the private protocol. Prefer the documented surfaces and the
   endpoint generation, behind one adapter.
4. **The gaps are review, merge and attention**, not "another terminal": diff
   review with notes sent back to the agent, worktree hygiene (env files,
   setup), and notifications that reach you. Corgi's supervisor, MERGE
   badges, numbered questions and memory are exactly this, and no other tool
   has them.
5. **Remote is expected.** SSH-first, no exposed ports.
6. **Longevity and licence matter.** Several tools died or were relicensed.
   An MIT client (Corgi) on an Apache-2.0 runtime, with no cloud dependency,
   is a strength.

## 7. Recommendation and phased plan

**Approach:** a hybrid Corgi GUI (option c) in egui/eframe, over a
`corgi serve` JSON-lines protocol on the Herdr host, reached locally or by
`ssh`. Terminals are on demand and come last. Herdr's TUI or Herdr UI stays
the full workspace view.

| Phase | Content | Effort | Workers |
| --- | --- | --- | --- |
| 0. Spike | Mostly done (§9). Left: real SSH, Linux/Hyprland, IME and dead keys typed by hand, Codex, system theme live. | 2–3 days | 1 |
| 1. Core + `corgi serve` | Cargo workspace with `corgi-core`. Move `waker`, `markers`, `rows`, `draft`, `first_prompt`, `sweep`, `RefreshState` and the free launch, merge and close functions out of `src/app/`. Invert the recovery back-references. Split the palette from ratatui roles. Add `corgi serve` (structured snapshot, deltas and commands) with snapshot tests. The TUI behaves exactly as before. | 1.5–2 weeks | 1 |
| 2. **First milestone: GUI dashboard** | egui app that spawns `corgi serve` locally or `ssh host corgi serve`:<br>• project cards and agent rows, plan usage header, expanded transcript tail<br>• prompt; new agent (preset defaults, inline lists); start or focus supervisor<br>• merge with confirmation and outcome; close; focus in Herdr<br>• answer numbered questions<br>• system light/dark; reconnect with dimmed cached state<br>• Linux tarball and pacman package, unsigned macOS build. | 2–3 weeks | 1–2 |
| 3. Terminals on demand | One terminal pane beside the selected card, through the endpoint adapter (§8), with `observe` for previews. Take control while focused and release afterwards. Read-only previews for the rest. | 2–4 weeks | 1–2 |
| 4. Corgi-only views | Decisions, ledger and brief browser (`corgi digest`), worker reports, inbox, diff review before merge, OS notifications from agent status, several hosts in one window. Optional `corgi daemon` unit. Signed and notarized macOS builds, Homebrew cask, AUR. | Ongoing, 1–2 weeks per feature | 1 |

Phases 1 and 2 together are the first useful milestone: about 3.5–5
worker-weeks. Phase 1 pays off even if the GUI stops there: it gives the
Omarchy widget clean data and makes the optional daemon a small step.

**Main risks:**

- **Terminal input fidelity.** The bridge path cannot encode keys for the
  pane's mode (§8), so terminals depend on the endpoint adapter.
- **Endpoint protocol coupling.** It is stable by promise but specified only
  in Rust types, and upstream is closed to PRs. Keep it behind one adapter and
  pin a tested Herdr version range.
- **Overlap with Herdr UI**, which is adding orchestration. Corgi's value has
  to stay in the supervisor and memory layer.
- **Two frontends to maintain.** Phase 1's core split is what keeps the TUI
  and GUI from drifting; frontend code should contain no Herdr calls.
- **Signing and notarization** need an Apple Developer account (about $99 a
  year) before wide macOS distribution.

## 8. Terminal paths compared, for Corgi

A second spike built a standalone endpoint client (about 730 lines of Rust,
under the session scratchpad, not in the repository) against a separate
Herdr 0.9.0 session. It drove a key-logging program in a pane through both
paths. All rows are **[spike 2]** unless marked otherwise.

What Corgi needs from a GUI terminal:
- one agent's terminal beside its card, with keys that work in Claude Code and
  Codex;
- the same remotely, over SSH;
- no disturbance to your Herdr window;
- low upkeep across Herdr upgrades.

| | Bridge: `herdr terminal session observe` / `control` | Endpoint generation 1 |
| --- | --- | --- |
| Herdr's stance | Documented for third-party bridges, since 0.7.2 | Herdr's own UI; no stated support for outside clients |
| Shift+Enter | **Guessed** by the GUI; the frames don't say which mode the app chose. In Claude Code 2.1.280 `\e[13;2u`, `\e[27;2;13~`, `\e\r` and `\n` all gave a new line, because it enables kitty flags 5 and modifyOtherKeys 2 (spike 3). A plain `\r` submits, and `\e[13;2u` reaches a legacy-mode program untranslated. | **Right**: `\e[13;2u` (kitty) or `\e[27;2;13~` (legacy). Herdr encodes each key for the pane's mode, so keys behave exactly as in Herdr's TUI ([source] `pane_input.rs:314`). |
| Paste | Bracketed only if the GUI sends a complete bracketed paste | Wrapped when the app enabled bracketed paste |
| What it shows | One pane, at the size requested | The whole active tab of the client's own location, laid out at the client's size. One pane means cropping its rect, or a tab holding only that pane, as Corgi's agent tabs normally do. Zoom is tab-global, so other clients see it too. |
| Data per busy pane | ANSI diffs, 25 KB/s | Cell patches, 79 KB/s at 29 frames/s. 0.9.0 has no scroll operation; 0.9.5 adds opt-in scroll and delta encodings. Bursts collapse to the latest frame. |
| Echo, local | 1.3 ms | 0.3 ms (p50) |
| GUI side | Small ANSI applier (`vt100` / alacritty_terminal) plus our own key encoder | Paint cells; no emulator, no key encoder |
| Your Herdr window | `control` resizes the PTY while held, and in spike 3 also other tabs' panes; `observe` never resizes | The client that last interacted with a tab sets its size. Your TUI shows the GUI's size, cropped or padded, until you type there. Inherent to one PTY per pane. |
| Navigation | n/a (targets a pane) | Per client through its own connection; the JSON `pane.focus` moves all clients |
| Remote | `ssh host herdr terminal session control <pane>`: plain stdio, one channel per pane | Needs a byte pipe to the remote `herdr-client.sock`. Options are `ssh -L` socket forwarding, or a relay command such as a `corgi` subcommand. Health check and reconnection are built into the protocol. **[assumption: not tried over SSH]** |
| Upgrade risk | The `herdr` CLI must speak the **exact** private protocol of the running server (`check_client_version`, `wire.rs:1749`). Fine on one host, but it breaks between `herdr update` and a server restart whenever 22 is bumped. | Message order is "frozen for endpoint generation 1" ([source] `wire.rs:467`, `:1326`). JSON parts tolerate new fields. Surface structs are positional bincode, so Herdr adds new encodings only behind hello flags, as 0.9.5 did. No written spec; upstream is closed to PRs. |
| Code to own | About 200 lines (process, JSON, key encoder) **[assumption]** | About 730 lines of mirrored types and client (spike), or Herdr UI's `herdr-protocol` crate as a git dependency |

**For Corgi's use, the endpoint wins**, by a smaller margin than first
thought. Keys are still the deciding row. Claude Code accepts guessed key
sequences, but every other program in a pane (shells, editors, Codex) depends
on the guess too. An agent terminal where Shift+Enter or Ctrl+J does the wrong
thing is worse than none.
The endpoint makes the GUI behave exactly like Herdr's own window, and that is
also the surest compatibility promise Herdr makes. Its costs are manageable:
- **Unofficial use:** keep it behind one adapter, test against each Herdr
  release, and pin a known-good range.
- **Tab-shaped surfaces:** fine, because Corgi's agents normally have a tab
  each (ARCHITECTURE.md:313-314).
- **SSH plumbing:** one relay to build.

Use the bridge's `observe` for read-only previews of other agents, since it is
documented, cheap and never resizes, and as the fallback if an upgrade breaks
the endpoint adapter. Keys then degrade to legacy encoding until the adapter is
fixed.

Worth asking upstream, as a Discussion: semantic input on
`terminal session control`. With that, the documented path would be enough.

**Still to prototype (phase 0):**
1. The endpoint adapter over SSH, through `ssh -L` or a relay, with reconnect.
2. An egui cell painter for the surface frames.
3. A real Claude Code and Codex session: Shift+Enter, IME and dead keys on
   macOS and Hyprland, scrolling.
4. The system scheme read live on both platforms.

## 9. Prototype and protocol tests (spike 3)

The focus-view design was then tested. The GUI would be mostly a Corgi
overview, with one terminal, at most two, in a focus view, laid out with tiles.
Two throwaway tests ran against separate Herdr 0.9.0 sessions, both stopped and
deleted afterwards, on an Apple M5 with a 3440×1440 60 Hz display:

1. **An egui prototype.** It is about 2,000 lines of Rust: eframe and egui 0.37
   with egui_tiles 0.18. It has a mock overview, an open-questions panel,
   transcript tiles, and one or two live terminal tiles. Each tile is its own
   endpoint connection to its own single-pane Herdr tab.
2. **Protocol follow-ups.** Two connections on two tabs, warm connections, a
   real Claude Code input box, and a byte relay standing in for SSH.

**Performance: not sluggish.**

| Measure | Result |
| --- | --- |
| Two 150×60 terminals both redrawing the full screen 60×/s, plus the overview (window) | 60 fps; frame p50 5.0 ms, p95 6.6 ms; app CPU 34 %, Herdr 20 % |
| One 200×60 terminal under a scrolling flood (window) | frame p95 4.3 ms; app CPU 10 % |
| Idle | 0 frames, 0 % CPU (repaints only on new surfaces or input) |
| Keystroke to painted frame, local (window) | p50 1.9 ms, p90 2.7 ms, plus up to one display refresh. With the other pane redrawing at full speed, Herdr itself adds up to about 13–19 ms before the echo. |
| Resize to a correctly sized screen | about 68 ms, 40 ms of it the debounce, which can be shortened |
| Cold start to first terminal | 250–390 ms |
| Memory, binary | 104–208 MB resident with a window; 15 MB unstripped |
| Warm connection activated | p50 3.4 ms (cold connect: about 275 ms, because Herdr checks for new connections every 250 ms) |

**What the prototype got wrong, all fixable on our side:**

- **egui's system font fallback blocked the first frame for up to 3.3 s.** The
  fix is to ship or name fonts (CJK, emoji, a Nerd Font) and load them off the
  UI thread.
- **Box-drawing lines had gaps,** because the glyphs are shorter than the cell.
  Draw them as shapes, as terminal emulators do.
- **Emoji and Nerd Font icons were blank** without fallback fonts.
- **Rows are laid out as text again on every change.** That costs about 4 ms a
  frame with two full-screen redraws. A glyph-atlas renderer (a wgpu paint
  callback) would leave room for more terminals or a slower machine.
- Colours, bold, italic, underline, strikethrough, dim and reverse were right.
  Wide CJK characters were right once their width came from Unicode tables:
  Herdr sends the second half of a wide character as an ordinary space.

**Herdr behaviours the GUI must design around [spike 3; source at v0.9.0]:**

- **Navigation from outside.** The JSON focus calls, creates with
  `focus: true`, and closing the tab a connection shows all move connections
  (`client_views.rs:165`, `clients.rs:109-129`). A moved connection that later
  interacts claims its new tab's size.
  - The GUI must treat any change of its `focused_tab_id` that it didn't ask
    for as a lost tab, and park the connection.
  - Corgi's creates already pass `focus: false` (`src/herdr.rs:331`, `:430`,
    `:451`, `:506`). In the GUI, Corgi must not call `*.focus` at all.
- **Activation claims the connection's current tab.** It gets that tab's size
  before `tab.focus` can follow, and `tab.focus` is refused while inactive.
  Use one empty **park tab**: park a connection there before deactivating, and
  send `surface.set` plus `tab.focus` together.
- **Keep two connections warm, parked and inactive.** Inactive connections
  claim no size but still receive status snapshots (about 9.6 KB each, no
  deltas). Entering the focus view then takes milliseconds.
- **With one active connection, every tab follows its size**
  (`client_views.rs:571-583`). So resizing the focus view resizes every agent's
  terminal, exactly as a single Herdr TUI window does today.
- **Switching typing between two connections** sends one full frame (34 KB at
  120×40). That is harmless.

**Claude Code input (2.1.280).** Through the endpoint, Shift+Enter, Alt+Enter
and Ctrl+J each gave a new line without submitting. Through the bridge, the
CSI-u, modifyOtherKeys, `\e\r` and `\n` sequences did too. Nothing was
submitted in either test. Codex was not installed.

**Remote (relay stand-in; real SSH was not available).** The protocol worked
unchanged through a stdio byte relay.

| Link (one-way delay) | Keystroke echo p50 |
| --- | --- |
| Direct | 0.4 ms |
| Relay, no delay | 0.5 ms |
| Relay, 20 ms | 47 ms |
| Relay, 60 ms | 127 ms |

- Echo is round-trip time plus about 5 ms; no client-side cost was added.
- On a slow link (1 MB/s), Herdr coalesces full-screen redraws to about 5 fps.
- Large relay buffers queue control messages behind frame data: ping rose to
  800 ms with 1 MB of buffering, 310 ms with 32 KB. So the relay should buffer
  little and let Herdr's single-slot render lane drop stale frames.
- A local full-screen redraw at 200×60 is 8.5 MB/s. A scrolling flood is
  165 KB/s.

**Verdict.** The proposed stack is fast enough:
- egui with egui_tiles;
- endpoint generation 1;
- one connection per visible terminal, plus two warm parked connections.

The work left is ordinary terminal-renderer craft (fonts, box drawing, a glyph
atlas) and the connection-management rules above, not a change of approach.
Still unverified: real SSH, Codex, IME and dead keys typed by a person, and
Linux/Hyprland.

## 10. GPUI with Herdr UI's terminal painter (spike 4)

You then set two priorities: the terminal must sit **inside** the GUI, with
performance second. You also don't want to build a terminal. So a third
prototype checked whether Herdr UI's terminal code can be reused, in its
toolkit GPUI (`gpui-pre =0.3.6`, as Herdr UI pins it), against a separate
`gpuiproto` session (deleted afterwards). The egui prototype was rerun
alongside it on the same machine and display: Apple M5, 2560×1440 at 144 Hz.

**The lift.**

- **The painter compiled unchanged.** That is `terminal_painter.rs` and six
  submodules, about 2,300 lines: glyph cache, box drawing as exact shapes,
  powerline symbols, colour emoji, images. So did Herdr UI's protocol crates
  (`herdr-protocol`, `herdr-client`). The stubs needed were a theme struct, an
  error type and about 200 lines of `terminal.rs`. About 1,000 new lines and
  one hour of work.
- **Not reusable as-is: the interactive layer.** Herdr UI has no standalone
  terminal view. Its canvas, selection, links, IME, text input and scrolling
  live in its 1,000-line window renderer. Those would still be ours to port or
  write, in either toolkit.
- **Version fragmentation showed at once.** gpui-component 0.8.1, the dock
  library, needs `gpui-pre =0.3.10`, which cannot mix with Herdr UI's 0.3.6.
  The prototype used a fixed split instead of a dock.

**Side by side.** "Frame" is each toolkit's own per-frame work; CPU is the app
followed by Herdr's server.

| Scenario | GPUI fps, frame p50/p95 (ms) | egui fps, frame p50/p95 (ms) | CPU, GPUI | CPU, egui |
| --- | --- | --- | --- | --- |
| Idle | 0 | 0 | 0.9 % / 0.3 % | 0.0 % / 0.3 % |
| One 200×60, scrolling flood | 28, 3.5 / 5.7 | 58, 2.2 / 5.4 | 14 % / 6 % | 18 % / 6 % |
| One 200×60, full redraw | 53, 3.9 / 4.8 | 103, 1.3 / 3.9 | 28 % / 16 % | 27 % / 12 % |
| Two 150×60 redrawing + overview | 58, 4.5 / 5.2 | 157, 0.6 / 4.6 | 35 % / 20 % | 37 % / 17 % |
| Keystroke to frame, idle (p50/p90) | **8.0 / 9.9 ms** | **1.8 / 2.9 ms** | | |
| Keystroke to frame, other pane redrawing | 18.5 / 26.7 ms | 15.5 / 23.0 ms | | |
| Cold start to first terminal | median 246 ms | median 265 ms | | |
| Memory under load, binary | 69–94 MB, 12.7 MB | 108–136 MB, 15.0 MB | | |

Notes on the numbers:

- **Both kept up with every surface Herdr sent.** egui simply paints more
  often.
- **GPUI's extra input latency** comes from waiting for the next display tick
  (4.3 ms from arrival to frame start). Its frame cost is GPUI re-laying out
  the whole window each frame. The painter itself takes 0.9–2.4 ms; the rest
  is the overview panel, which a cached view would avoid.
- **Herdr UI's client polls each socket every 10 ms,** about 1 % CPU at idle.

**Rendering correctness.** GPUI with the lifted painter was right out of the
box: box drawing, emoji, CJK alignment, powerline symbols. Nerd Font icons
were missing only because no Nerd Font is installed. egui still needs that
work (§9).

**Reading.** Both are fast enough: neither was sluggish, and both kept up
with every surface Herdr sent. The trade-off is clear:

- **GPUI** reuses the hard rendering polish and uses less memory. It costs a
  pinned pre-release toolkit whose versions already conflict, about 6 ms more
  keystroke latency at idle, and experimental accessibility.
- **egui** is the more mature, more responsive toolkit. We'd write the
  rendering polish ourselves.

Either way, the interactive layer (selection, links, IME, scrolling) is ours.

## 11. Terminal inside a UI: how others do it (October 2026)

A survey of about 35 agentic IDEs, orchestrators and terminal apps, and of
the technology behind them **[web]**. Each claim is sourced in the research
notes; the main links are below.

**What the field does:**

1. **Electron + xterm.js (WebGL renderer) + node-pty is the most common
   approach.** VS Code and its forks (Cursor, Windsurf, Kiro), the Codex app,
   Superset, Orca, Pane, Emdash, Wave and Tabby all use it.
   - Serious apps run xterm.js 6.1 betas to get synchronized output and the
     kitty keyboard protocol.
   - The recurring complaints with Claude Code and Codex are scroll jumps and
     flicker, plus Electron's memory use.
2. **libghostty is replacing older terminal engines,** in three forms:
   - **Full embedding,** where Ghostty draws the terminal (cmux, Supacode,
     Farol, herdrm, Heeler). It is **macOS only**: the API calls itself
     internal to Ghostty's macOS app, and Ghostty has said a Linux version is
     not planned.
   - **libghostty-vt with the app's own renderer** (JetBrains 2026.3,
     Paneflow, moai-studio). It has no renderer of its own and no way to load
     cells, and its API "is definitely going to change".
   - **ghostty-web,** compiled to WebAssembly (Nimbalyst, Xum, herdr-web).
     It is still at 0.4 and missing pieces.
3. **alacritty_terminal plus an own GPU painter** is used by Zed (GPL, so a
   reference only), Warp, okena (MIT, GPUI, Wayland, kitty keys, IME) and the
   egui and iced widgets.
4. **Platform widgets** each tie the app to one toolkit and platform: VTE (no
   kitty keyboard yet), SwiftTerm, QTermWidget, JediTerm.
5. **Not possible on Wayland:** embedding another app's window, or a web
   terminal inside a native Rust window. wry's child webview is X11-only.
   Tauri on Linux has had 0.5 s echo lag from WebKitGTK's slow WebGL path.

**Where emulation happens.** Almost every app emulates in the client.
Server-owned designs (tmux `-CC`, Zellij web, herdrm and herdr-web on Herdr)
usually emulate twice: on the server, then again in the client. Only
**Herdr UI paints the server's cells directly.** Shared lessons from those
designs:

- Scrollback and copy beyond the visible screen must come from the server.
- Keys are only right when the server encodes them.
- herdr-web shows the costs of the alternative: guessed keys, copy limited to
  the visible screen, reconnects after lost frames, and a pin to Herdr's
  private protocol.

**What the well-regarded apps did.** cmux, Ghostty, Supacode, Herdr UI and
Paneflow are the ones praised as feeling native and fast. They:

- reuse a proven terminal engine instead of writing one;
- support synchronized output (DEC 2026) and the kitty keyboard protocol,
  which fix the flicker and the Shift+Enter problems;
- paint with GPU quads, a glyph atlas, and box drawing as sprites;
- keep reading the program's output while hidden, but skip painting hidden
  panes;
- use a binary transport per pane with flow control;
- keep sessions in a daemon, so the UI can restart without losing them.

**The best way, in general and for Corgi.**

In general, when an app owns its programs, the best-regarded native recipe is
libghostty-vt or alacritty_terminal plus a GPU cell painter. The mainstream
recipe is Electron with xterm.js.

Corgi's case is the rarer one: Herdr already runs the programs, interprets
their output and encodes keys. So the best fit is the one Herdr UI uses:
**paint the endpoint's cells natively, with no client-side emulator.** That
avoids double emulation, guessed keys and the web stacks' Linux problems. It
matches §8–§10.

- **The painter:** Herdr UI's (Apache-2.0, GPUI), or a wgpu painter for egui
  modelled on Rio's `sugarloaf` (MIT).
- **The interactive layer is ours** whichever painter: selection, links,
  IME, wheel and scroll sent to the server, copy beyond the screen read from
  the server. okena (MIT) is the permissive reference for input, IME and
  links.
- **Fallback:** if the endpoint ever fails us, feed the bridge's frames to
  libghostty-vt in-process.

Main sources:
- [libghostty header](https://github.com/ghostty-org/ghostty/blob/main/include/ghostty.h)
- [Ghostty Linux embedding](https://github.com/ghostty-org/ghostty/discussions/11722)
- [JetBrains on Ghostty](https://blog.jetbrains.com/platform/2026/10/jetbrains-terminal-now-uses-ghostty/)
- [Paneflow](https://paneflow.dev/blog/libghostty-linux)
- [okena](https://github.com/contember/okena)
- [sugarloaf](https://lib.rs/crates/rio-backend)
- [wry child webview](https://docs.rs/wry/latest/wry/struct.WebViewBuilder.html)
- [Tauri WebGL lag](https://github.com/VoltiusApp/voltius/pull/313)
- [xterm.js kitty keyboard](https://github.com/xtermjs/xterm.js/pull/5600)
- [Zellij web](https://poor.dev/blog/building-zellij-web-terminal/)
- [Claude Code fullscreen notes](https://code.claude.com/docs/en/fullscreen)
- [TerminalX transport](https://github.com/terminalx-ai/terminalx/issues/232)

## 12. The prototypes on Hyprland (spike 5)

Both prototypes were run on the Omarchy machine:
- **Hardware:** Ryzen 5 3600, Radeon RX 6700 XT on Mesa, 2560×1440 at 165 Hz,
  scale 1.0.
- **Software:** Hyprland 0.56, Herdr 0.9.1.
- **Full report:** `results/omarchy.md` on the `gui-prototypes` branch.

**Platform support.**
- Both ran as native Wayland clients (no XWayland), and no system packages
  were missing.
- egui needed no changes.
- GPUI needed three configuration fixes:
  - enable `gpui-pre-platform`'s `wayland` feature;
  - set a Wayland `app_id`;
  - pick a monospace font that exists on Linux.

**Performance.** The macOS gap mostly closed.
- Under load, frame times, CPU and memory were within about 10 % of each
  other.
- Keystroke to frame at idle: GPUI 4.4 ms, egui 1.2 ms. Both are within one
  165 Hz refresh.
- With another pane redrawing: 16.7 ms and 15.8 ms.
- Load cost about twice the CPU of the M5 for both, and for Herdr's server
  too.
- Memory was about 125–144 MB for both.

**Behaviour.**
- **Compositor resizes** (tiling, swaps, a simulated drag) gave a correctly
  sized terminal in about 50 ms for both. GPUI's timing was the steadier.
- **Swedish keys** (å ä ö, AltGr brackets) worked in both, and so did Claude
  Code's Shift+Enter, Alt+Enter and Ctrl+J.
- **Rendering:** GPUI rendered box drawing and Nerd Font icons correctly.
  egui kept its macOS defects: gaps in box borders, and icons overlapping the
  next cell.

**One fix needed in GPUI: hidden windows.** On a hidden workspace, Hyprland
stops sending frame callbacks.
- egui then stops painting; Herdr, seeing no acknowledgements, throttles, and
  both go almost idle.
- GPUI kept receiving and decoding every surface. Under a full redraw that
  cost the app 11 % and Herdr 35 % CPU.
- **Fix:** the GUI should deactivate its connections while the window is
  occluded, with `client_shell.surface.set {active:false}` (§9).

**Not checked:** flicker while the compositor resizes, judged by eye;
fractional scaling; dead keys.

**Verdict: GPUI.** It passes check 2 of the stack question. Check 1 is still
open: the painter on the `gpui-pre` version that gpui-component's dock needs.

## 13. Herdr as the always-on backend: plugin hooks (spike 6)

Instead of a Corgi daemon, Herdr could run a short `corgi hook` command per
event. The plugin manifest declares `[[startup]]` and `[[events]]` hooks, on
`pane.agent_detected`, `pane.agent_status_changed`, `pane.exited`,
`workspace.closed` and so on.

This was tested against a private Herdr 0.9.0, isolated under its own `HOME`
and `XDG_*` directories, because plugin registrations are per user, not per
session. The live session's plugin and session lists were unchanged
afterwards. The hook was a small compiled Rust program.

**It works, and it's fast [spike 6].**

| Measure | Result |
| --- | --- |
| Status change to hook start | p50 6.6 ms, p90 7.8 ms |
| Worker reports idle → wake text typed into an idle supervisor pane | p50 10.7 ms, p90 24.7 ms; 35 of 35 delivered |
| Herdr server CPU per hook | about 0.9 ms |
| Steady 1 event/s | 0.43 % server CPU |
| Hook process spawn and exit | about 2.5 ms |

More observations:

- **No window needed.** Hooks ran with a headless server and no client
  attached.
- **Restarts are covered.** The startup hook ran 41–47 ms after the server
  started, and re-applied Corgi's tokens by about 50 ms. It ran again after a
  live handoff (`herdr server live-handoff`), 131 ms after the request.
  Without it, tokens and reported agent state were gone after both a restart
  and a handoff.

**Limits, and the design rules they imply:**

1. **Bursts drop events.** Herdr runs at most 32 plugin commands at once,
   across all plugins (`runtime.rs:12`). Beyond that it drops the event
   rather than queueing it. At about 100 events/s nothing was lost; at
   400–800/s, 34–59 of 100 were dropped.
   - A hung hook holds a slot forever: 0.9.0 has no timeout.
   - Hooks start in parallel and can finish out of order.

   **Rule:** treat an event as a trigger, not as data. Every hook reconciles
   from Herdr's snapshot and Corgi's records, as the waker already does with
   `seen.json`, so a dropped event is caught by the next one. Hooks must be
   quick and enforce their own time limit. One hook at a time reconciles
   under a lock; a hook that finds the lock taken marks the state as changed
   and exits, and the running hook goes round once more.
2. **The event JSON is thin.** It has the pane, workspace, agent and new
   status, but no previous status, no `state_change_seq` and no session. One
   `agent.get` (about 0.6 ms) or the snapshot fills that in. An unseen idle
   arrives as `done`; Corgi already treats `done` and `idle` as one.
3. **Gaps the startup hook must cover.** Events while Herdr is down are
   lost, so the startup hook reconciles everything. `pane.closed` fires only
   for an explicit close; a shell exit gives only `pane.exited`. Token
   changes trigger no hooks.
4. **No timers.** Neither token expiry nor time passing triggers hooks. That
   affects one feature, the idle prompt-cache handover: it either becomes
   lazy (at the next event or when the app opens) or needs a small
   Herdr-hosted pane.
5. **An unexplained 100 ms stall** on some API calls after a quiet period.
   Minor; not investigated.

**Result.** With the "trigger, then reconcile" rule, Herdr's hooks can do all
of Corgi's always-on work without a Corgi process:

- restore markers after restarts and handoffs;
- record new sessions at once;
- queue and deliver wakes;
- start context handovers;
- do the bookkeeping when panes exit and workspaces close.

The GUI, TUI and widget can then be closed at will and read the state when
opened.

## Decisions (2026-10-28)

- **The terminal sits inside the GUI;** performance comes second. External
  terminal windows and embedding your own terminal app are out (§8–§11).
- **Terminals come through Herdr's endpoint protocol** (generation 1),
  painted natively with no client-side emulator. Only the GUI's own
  connections navigate; Corgi does not call Herdr's JSON focus methods in GUI
  mode. Connections are deactivated while the window is hidden (§9, §12).
- **Toolkit: GPUI,** with Herdr UI's terminal painter (Apache-2.0) on the
  `gpui-pre` version it pins. It passed on macOS and Hyprland (§10, §12).
- **Herdr is the always-on backend (§13).** Corgi runs no process of its own.
  Its plugin's startup and event hooks do the always-on work: they restore
  markers, record sessions, queue and deliver wakes, start context handovers
  and do bookkeeping. Each hook reconciles from the whole state, so a dropped
  event costs nothing. The GUI, TUI and widget can be closed at any time and
  read the state when opened.
- **The idle prompt-cache handover runs only while a Corgi window is open.**
  It is the one timed feature, and Herdr has no timer hooks. Context handovers
  stay event-driven and always on.
- **No dock library.** Layouts are fixed: an overview, a focus view with one
  or two terminals, and side panels such as the supervisor and open
  questions. Splits are resizable and sidebars can be opened and closed, with
  sizes remembered. That is plain GPUI layout, so gpui-component's dock and
  its version conflict are not needed. A dock can be added later if free
  arrangement is ever wanted.
- **Still to build or check during the work:**
  - the interactive layer (selection, links, IME, scrolling, copy beyond the
    screen read from Herdr);
  - a font chosen per platform;
  - flicker during compositor resizes, judged by eye.

## Open questions

1. **Daemon.** Superseded by §13: Herdr's plugin hooks can do the always-on
   work with no Corgi process, which is the recommendation. The original
   question was whether Corgi should get an optional long-running `corgi daemon` (a
   systemd or launchd user unit) so supervisors are woken when no dashboard or
   GUI is open? It reverses ARCHITECTURE.md's "no daemon". Recommended:
   optional, off by default, added in phase 4.
2. **Build our own versus join Herdr UI.** Herdr UI (Apache-2.0) already
   covers option (b) and is adding orchestration. The alternatives are:
   - (i) our own GUI as recommended;
   - (ii) GPUI with Herdr UI's crates, for a shared look and future merging;
   - (iii) no GUI, and make `corgi serve` a backend that other frontends,
     Herdr UI included, could use.

   Recommended: (i), keeping (iii) possible, since `corgi serve` is the same
   work either way.
3. **Stack:** decided: GPUI, with fixed layouts and no dock (see Decisions).
   The options were (§10):
   - **GPUI with Herdr UI's painter:** less rendering work, pre-release
     toolkit.
   - **egui with our own painter:** a more mature toolkit, lower latency.

   Recommended: GPUI, if two checks pass. First, the painter moves to
   `gpui-pre 0.3.10` so gpui-component's dock can be used. Second, it runs well
   on Hyprland. Otherwise egui.
