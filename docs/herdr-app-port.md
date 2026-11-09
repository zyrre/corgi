# Porting Corgi to the Herdr app: what herdr-core (`hc`) gives us

A spike run on 2026-11-09 on the user's Mac with `hc` 0.1.19 and
`herdr-app` 0.1.14, before any production code for the port is written.
The questions: can Corgi deliver wakes with `hc send` without damaging a
message the user is half-way through typing, and can hc's own notes replace
part of Corgi's wake machinery?

Each finding is marked:

- **tested**: run here, with the command and what was seen;
- **from docs**: read in `hc --help`, `hc <command> --help`, `hc --skill`,
  `hc schema`, `herdr-app --skill` or the app's generated `herdr.d.ts`
  (inside the app binary); not run;
- **not reached**: not looked at in this spike.

The tests used Claude Code 2.1.317 with `--model haiku`, and OpenCode
1.18.33 with `litellm/claude-haiku-5-5` (section 4), in hc terminals named
`spike-*`. A second, isolated core (`HC_SESSION=s`, its own state
directory, a clean environment) was used for anything that stops or
restarts a core, so the user's core was never stopped. A "person" was
simulated by attaching through a pty (`hc attach <t>` under Python's
`pty.fork`) and typing one character every 120 ms; that is the path a
viewer's keys take. All test terminals, the isolated core's state, a test
template and a test LaunchAgent were removed afterwards (see "Clean-up").

## Short answers

- **Can a wake avoid a half-typed draft? Yes, but not with `hc send`
  alone.** `hc send` (and hc's notes, which take the same path) waits for
  ~3 s without keystrokes from a viewer, then pastes after whatever is in
  the box and presses Enter: a paused draft is submitted together with the
  wake. hc has no "box is empty" condition. Corgi's existing draft check
  (`crates/corgi-core/src/draft.rs`) works unchanged on the styled first
  paint of a watch-only attach (`hc attach --observe`, API
  `terminal.attach {observe: true}`), so the daemon can check, then send
  only when the box is empty. A small race stays: the user starting to type
  between the check and the paste. Decided (user, 2026-11-09): accept it;
  no upstream request for a `send --if-empty`.
- **Can hc's notes replace Corgi's wakes? No.** Notes cover a child that is
  blocked, errors or exits; a child finishing its turn (`done`), which is
  Corgi's main wake, is not a note. Notes merge into the user's draft, go
  only to the terminal that started the child (fixed at launch), and
  `notes = false` is one switch for the whole core.
- **OpenCode 1.18.33 reports no status to hc** (tested), so hc treats it
  as a plain program: no wakes from events, `--if`/`--wait` useless, and a
  plain send answers its permission dialog. A small project-local plugin
  that writes OSC 7501 fixed all three in a test. Its drafts, sessions,
  resume and role work (section 4). Codex is covered from docs only.
- **Recommendation: design A**, a `corgi daemon` under launchd following
  `hc events` and delivering with a draft-checked `hc send`, with workers
  started `--no-notify` so hc never types into the supervisor on its own.
  Details and the comparison are under "Design".

## 1. `hc send` and half-typed input

The documented rule (from docs, `hc schema`, `terminal.send`): "Paste text
and press Enter, once nobody has typed for ~3 s; keys a person presses
meanwhile are held, then delivered. Refuses a blocked program unless
`force`." Options: `--if idle|working`, `--force`, `--wait`, `--timeout`.

### a. Idle, empty box (tested)

`hc send spike-a "Reply with just the word PONG."` returned in 0.3 s with
`{"started":true,"status":"idle","turn":0}`; Claude answered. No wait.

### b. Idle, half-typed text in the box (tested)

- Draft typed with `hc keys spike-a h e l l o space d r a f t`, send one
  second later: send returned at once (0.26 s) and the box submitted
  `hello draftReply with just the word SENT-B.` Keys pressed through the
  API (`hc keys`) do not count as a person typing.
- Draft typed by an attached "person" (`half typed`, last key at
  10:18:21), send started at 10:18:22: send returned at 10:18:24, about 3 s
  after the last key, and submitted `half typedReply with just the word
  SENT-B2.`

So send **appends to the draft and submits both**. It never replaces or
clears the draft. "A pause" is ~3 s without a key from an attached viewer
(the app or `hc attach`), measured from the last key; text sitting in the
box does not count. There is no option to wait until the box is empty
(from docs: none in `hc send --help` or the schema).

### c. Someone actively typing (tested)

The person typed 69 characters over ~8 s; send started 3 s into it. Send
held until 3 s after the last key (typing ended 10:18:53, send returned
10:18:56), then appended to the finished text and submitted it:
`aaaa … nnnnReply with just the word SENT-C.` Nothing was interleaved
character by character, but the draft was still submitted.

### d. Claude working, mid-turn (tested)

- `hc send --if idle` while working: refused, exit 10,
  `{"error":{"hint":"nothing was typed","kind":"status_mismatch","message":"t1 is working, not idle"}}`.
- Plain send while working: returned at once
  (`{"started":true,"status":"working","turn":7}`). Claude Code put it in
  its own queue (`ctrl+enter to send now`, box hint `Press up to edit
  queued messages`) and ran it after the turn. Nothing lost.
- A draft typed mid-turn, then a plain send: Claude queued
  `mid draftThen reply MERGE-D.`; same merge as (b).
- **Trap:** after its first turn Claude Code reports `done`, not `idle`,
  and the core keeps `done` until the next turn (attaching does not change
  it; tested). So `--if idle` refuses a resting Claude
  (`t1 is done, not idle`, exit 10), and `--if` has no `done` value. Treat
  `done` and `idle` both as "resting". (From docs, `herdr.d.ts`: the app's
  `agents()` turns `done` into `idle` once a person has looked; the core
  does not.)

### e. Blocked on a permission prompt (tested)

A child Claude in `--permission-mode default` asked to run
`printf 'hi' > note.txt` in the scratch folder. Its status was
`{"app":"claude-code","kind":"permission","msg":"approve Bash: printf 'hi' > note.txt && cat note.txt","state":"blocked"}`.

- `hc send spike-kid "Reply X"`: refused, exit 9,
  `{"error":{"hint":"typing would answer its dialog; read its screen first (hc read t2), or --force","kind":"blocked","message":"t2 is blocked (permission)"}}`.
- `hc send --force spike-kid "Reply X"`: **approved the command.** The
  pasted text went nowhere, Enter picked the highlighted `1. Yes`, the file
  was written and the turn finished. Corgi must never use `--force` for a
  wake.
- A Claude whose login had expired reported
  `{"kind":"auth","msg":"Login expired · Please run /login","state":"blocked"}`
  (see section 3, daemon placement).
- Claude's folder-trust dialog at first start reports **no status at all**
  (`reported: false`), so a template or `--wait` cannot tell it from a slow
  start. Corgi's folder-trust handling (`agent.send_keys`) stays, as
  `hc keys <t> down enter`.

### f. `--wait` and its results (tested)

- Normal: `{"started":true,"status":"done","turn":10,"wait":{"ended":true,"reason":"status","status":"done","turn":11,…}}`.
- A slash command that starts no turn (`/cost`): `{"started":false,…,"wait":{"ended":false,"reason":"not_started",…}}`.
- `--timeout 3` on a long turn: `"wait":{"ended":false,"reason":"timeout","status":"working",…}`; the turn keeps running.
- Other `reason` values (from docs): `exit`, `output`, `message` (someone
  messaged the waiter), `children`.

### Detecting a draft through hc

- `hc read` / `terminal.read` (tested): plain text only (`text`,
  `complete`, `how`, `why`). It shows `❯ my draft`, but it cannot tell a
  draft from dim text in an empty box, such as Claude's `Press up to edit
  queued messages` hint or a prompt suggestion: the reason `draft.rs`
  reads styles.
- No status, field or event says input is pending (from docs: `Terminal`,
  `Status` and the event types in the schema have none).
- `hc attach --observe` (tested; API `terminal.attach` with
  `observe: true`, "Watch only: never type or resize"): the first paint is
  the whole screen as ANSI, one `\r\n`-separated row per line with its SGR
  styling (dim is `\x1b[2m`), then the cursor position and the last
  OSC 7501 report. A Python port of `draft::holds_draft` returned `False`
  on the paint of an empty box and `True` with `my draft` typed. The box
  line arrives as `❯ my draft`; `draft.rs` treats the NBSP as
  whitespace (Rust's `char::is_whitespace`), so it needs no change.
- A normal (non-observe) attach resized the terminal to the attaching pty
  (`size` events 200×50 and back, tested); the observer did not.

**Porting `draft.rs`:** keep it as it is; feed it the observer's first
paint instead of `agent.read`'s styled screen. (Found while building the
daemon: the viewer first asks its terminal for its colours, cell size and
attributes, `OSC 10/11/12 ?`, `CSI 16 t` and `CSI c`, and paints only after
waiting about half a second for answers, starting with a clear screen.
`corgi daemon` answers nothing and reads from the clear screen on; see
ARCHITECTURE.md, "`corgi daemon`".) The daemon opens
`terminal.attach {observe: true}` on the framed connection, reads the first
paint, closes it. The framed protocol (`hello {framed: true}`, channels,
DATA/CLOSE frames) is in the schema only by name: its byte layout is
**not reached** and needs herdr-core's source or docs. A fallback is to
run `hc attach --observe <t>` in a pty and read two seconds of output, as
this spike did, which works but is clumsy.

**The remaining race:** check, then send. If nobody typed in the last 3 s
(the check just saw an empty box), send pastes within ~0.3 s, so the window
is the time between the paint and the paste. If the user starts typing in
that window, send waits for their pause and merges. The daemon can notice
afterwards (the send took longer than ~1 s, so it waited for a person) but
cannot undo it. Only hc could close the hole (a `send --if-empty`, or an
input-pending flag). **Decided (user, 2026-11-09): accept the race and do
not ask herdr-core upstream for it.** The daemon may still log a send that
took over ~1 s as a possible merge, so a damaged draft can be found in the
log afterwards.

## 2. hc's notes as Corgi's wakes

### When hc types a note, and its exact text (tested)

Into the parent terminal, for these child events, verbatim as they appeared
in the parent's box:

- blocked:
  `<hc>t2 spike-kid is blocked (permission). See: hc read t2</hc>`
- exit with output:
  ```text
  <hc>t3 spike-p1 exited (exit 3). Last lines:
  bye
  Read: hc read t3</hc>
  ```
- exit without output: `<hc>t4 spike-p2 exited (exit 0). Read: hc read t4</hc>`
- killed with `hc kill`: `<hc>t11 spike-p6 exited (signal 1). Read: hc read t11</hc>`
  (a deliberate kill is noted too)
- error: from docs only ("blocked, error, exited"); not triggered.

**No note when a child finishes a turn** (`done`, tested twice: a Claude
child that printed `DONE-KID`, and a template child that printed `TPL`).
`done` is "news" only for `hc wait --children any`, which a parent must
call itself (tested: it returned the template child with
`"reason":"status","status":"done"`). The design (`hc --skill`) is that a
child reports its own result with `hc send <parent>`, and hc notes only
when "a child cannot report".

### Messages between agents (tested)

A send whose caller is an agent terminal (`HC_TERMINAL_ID` set) is wrapped:

- parent to child: `<hc from="t1 spike-a">Reply with just R1.</hc>`
- child to parent: `<hc from="t2 spike-kid">Reply with just R2.</hc>`
- unrelated terminal: `<hc from="t5 spike-par">Reply with just R3.</hc>`
- a plain shell as caller, or no caller (a daemon outside hc): not wrapped.
- template launch from an agent:
  ```text
  <hc from="t1 spike-a" reach="hc send t1 when done or stuck">You are a test worker. Keep every answer to one line.
  Reply with just TPL.</hc>
  ```
  The body of `~/.config/herdr-core/templates/<name>.md`, a newline, the
  task. `reach=` is the only thing hc injects into a child: a hint to
  report back to the **caller**. A template launched with `--parent t1`
  from outside any terminal got the same text **unwrapped, with no
  `reach`**. Plain `hc run … claude "task"` passes the task as Claude's
  argv and injects nothing.

So the child's own `hc send <parent>` is the intended report path, and a
note cannot carry a custom payload (Corgi's `[Corgi] … Run: corgi report …`
line or the child's report).

### Same send path as `hc send` (tested)

- **Draft:** an exit note arriving while the parent held a paused draft
  was appended and submitted: `parent draft<hc>t3 spike-p1 exited (exit 3). …`.
- **Parent working:** typed at once; Claude queued it and ran it after the
  turn.
- **Parent blocked:** held. Two exit notes that arrived while the parent
  sat on a permission prompt were typed once it was answered (Esc), both
  in one message.
- **Parent not an agent yet:** a note for a child of a plain shell terminal
  was held, not typed into the shell, and delivered when `claude` was later
  started in that shell.
- **Core restart while held:** a note held for a blocked parent across
  `hc server restart` was not typed when the parent went from blocked to
  `idle` (interrupted), but was delivered at its next stop (`done`) one turn
  later, batched with a newer note. Delayed, not lost.
- **Parent gone:** a child started `--outlive-parent` that exited after its
  parent terminal had finished produced no note anywhere; only the
  `finished` event records it.

### Parent/child (tested unless marked)

- `--parent` defaults to the calling terminal; from outside hc there is no
  parent unless `--parent` is given (tested: `"parent":null`).
- The parent is "set once at start" (from docs, schema). No command or API
  method changes it, so a new supervisor terminal cannot adopt the old
  one's workers.
- Children end when their **parent terminal's program** ends, unless
  `--outlive-parent`. A Claude supervisor run directly as the terminal's
  program (`hc run claude`) took its non-outliving child down on `/exit`
  (`reason: killed`); the outliving child lived on.
- **A supervisor run inside a shell terminal keeps its workers across a
  handover:** `hc run --bg` (no command: a login shell), then
  `hc send <t> "claude --model haiku"`. After `/exit` the shell remains, the
  terminal stays live at the same address, its children stay alive, and
  after starting a new `claude` in the same shell the next child's exit
  note reached the new session. This is the hc equivalent of Corgi's
  "relaunch in the same pane".
- `--no-notify` on `hc run` turns notes off per child (from docs; the
  `notify` field showed `true` by default).

### Window closed, restart (tested)

The app was never opened in this spike: every note above was typed by the
core alone, so notes and `hc send` work with the window closed. After
`hc server restart` terminals and their programs kept running (same pid),
labels kept, a held note survived (see above).

### `notes = false` (tested in the isolated core's own config)

With `notes = false` in the core's `config.toml` and a core restart, a
child's exit produced no note (its `notify` field still said `true`). It is
read at core start and applies to the whole core: every user of hc on that
machine loses notes and `<hc>` tags, not just Corgi. Tag suppression is
from docs; it could not be shown apart, since a plain shell caller is never
tagged.

### `hc events` (tested)

- One JSON line each when piped: a snapshot first
  (`{"channel":null,"seq":N,"terminals":[…full Terminal objects…]}`), then
  `{"method":"event","params":{"type":…,"terminal":"t1","seq":…,"data":…}}`.
- Types seen: `created` (full terminal), `status` (`{status, reported, records}`),
  `turn` (`{state, turn}`, after each turn: `turn` goes up whenever status
  leaves `working`), `title`, `size`, `foreground` (`{argv, cwd, name, pid}`,
  or `null`), `labels` (the full label map), `finished`
  (`{exit_code, finished_ms, reason, signal}`), `lost`
  (`{"reason":"server_stop"}`). From docs: `restarted`, `keep`, `removed`,
  `doc`. `{"method":"heartbeat","params":{"seq":N}}` every 15 s.
- Filters: `--terminal`, `--children-of`, `--label k=v` (repeatable; tested
  with `--label corgi-spike=1`).
- **The stream ends on `hc server stop` and on `hc server restart`, with
  exit code 0 and no error line**, after a last `lost` event (stop) or bare
  `foreground: null` events (restart). `seq` starts again from a small
  number in the new core: never compare `seq` across connections.
- **Any hc client starts a stopped core**, `hc events` included (tested),
  with that client's environment (see section 3).
- A subscriber that falls behind gets `missed` and a fresh snapshot (from
  docs; not seen).
- No event says a note or a message was typed into a terminal.

So a daemon can see every transition Corgi's hooks see today
(`pane.agent_status_changed` ≈ `status`/`turn`, `pane.exited` ≈
`finished`, `pane.agent_detected` ≈ `foreground` + first `status`) and
needs a reconnect loop that treats each new snapshot as the truth:
Corgi's "trigger, then reconcile" fits as it is.

## Design: which parts of Corgi's wake machinery hc can replace

Requirements: wakes with the window closed; no damage to the user's draft;
nothing lost; the cache/ctx handover trigger.

### (A) Daemon on `hc events` + draft-checked `hc send`

A `corgi daemon` per machine, under launchd, subscribes to `events` for
Corgi's labels, runs the existing `Transitions` per worker on `status`/
`turn` events, writes wakes to `inbox.jsonl`, and delivers when the
supervisor rests (`done` or `idle`) and the observer paint shows no draft.
Workers are started `--no-notify`, so hc types nothing on its own.

- Window closed: yes (launchd + core).
- Draft: yes, with the small race above, which is accepted.
- Nothing lost: yes. The inbox holds wakes while the supervisor works, is
  blocked, has a draft, is between sessions, or the daemon or core is
  down; `seen.json` (or hc's `turn` counter) catches stops missed while
  the daemon was down.
- Handover: yes, unchanged logic (`Sighting`, `Thresholds`), triggered by
  events instead of a dashboard refresh, so no "only when a dashboard is
  open".
- Workers' own `hc send` reports: not needed, and better avoided (they
  merge into drafts and are refused while the supervisor is blocked).

### (B) hc notes, workers as children of the supervisor

Workers started by the supervisor (or with `--parent`), told to report
with `hc send <parent>`; hc notes cover blocked/error/exit.

- Window closed: yes.
- Draft: **no**. Notes and the workers' own sends both merge into a
  paused draft, and Corgi cannot hold them.
- Nothing lost: **no**. A worker's own report to a blocked supervisor is
  refused (exit 9) and the worker has to retry by itself; a report sent
  while the supervisor is between sessions in its shell is typed into
  the shell and run as a command (hc holds its own notes in that state,
  but a plain `hc send` to a shell pastes and presses Enter: tested, that
  is how `claude` was started in the shell terminal); an outliving child of a finished supervisor terminal
  reports to nobody. `done` is not a note, so a worker that forgets to
  report is silent.
- Handover: **no**; still needs a daemon.

B saves little: it still needs the daemon for handovers, and gives up the
two properties the user asked for. The 2026-11-09 rejection of "workers
report with `hc send`" stands: hc does not solve either problem.

### (C) Mix: daemon for `done` wakes and handover, hc notes for blocked/exit

- Draft: **no** for the noted events (same merge).
- Nothing lost: mostly; but a note is typed once and forgotten, so it can
  never be retried, and an outliving child's notes go nowhere.
- Saves only Corgi's handling of blocked/exited workers, which is a few
  lines of `Transitions` and `wake_item`.

### Recommendation: A, using hc's structure but not its notes

Keep notes on in the core (the user's own non-Corgi agents may want them)
and turn them off per Corgi worker with `--no-notify`. Use hc for what it
does well: events instead of plugin hooks, labels instead of pane tokens
and `markers.json`, the turn counter, a shell-wrapped supervisor terminal
for handovers.

What A deletes or keeps (sizes from `wc -l` at 9a28374 and the inventory
note, which counts `waker.rs` as ~1,510 logic + ~1,750 tests; the
per-part splits below are estimates, not measured):

| Part | Today | Under A |
|---|---|---|
| Trigger: `herdr-plugin.toml` `[[events]]` + `corgi hook`, 5 s pass limit, 30 s watchdog | plugin + `src/app` | **delete**; the daemon's event loop is the trigger |
| `.dirty` marker, `mark_dirty`/`is_dirty`, dashboard presence (`dashboard_open`, `presence_path`) | `waker.rs`, ~100 lines | **delete** |
| Leader lock (`wake/<socket>.lock`, `begin`/`hold`/`take_lock`, takeover between dashboards) | `waker.rs`, ~150 lines + tests | **shrink** to one `flock` held by the daemon for its lifetime |
| `seen.json` + `Transitions` | `waker.rs`, `supervisor.rs` ~60 lines + tests | **keep**; optionally key it by hc's `turn` (wake when `turn` passed the last reported one, or status turned `blocked`), which is simpler and survives daemon restarts; decide when porting |
| Inbox (`inbox.rs` 1,238, `inbox_privacy.rs` 155) | files | **keep** as is |
| Delivery (`deliver`, `tick`) | `waker.rs` | **keep**, retargeted: rest = `done`/`idle`; never `--force`; check the foreground is the agent, not the shell |
| Draft detection (`draft.rs` 193) | `agent.read` styled | **keep** the parser; input = observer first paint |
| Report capture (inbox, transcript, screen) | `waker.rs` | **keep**; screen via `hc read --lines N` |
| Handover (`hand_over`, `replace_supervisor`, `Sighting`, `Thresholds`) | `waker.rs`, `supervisor.rs` | **keep** the logic; replacement = `/exit` + send the successor argv into the same shell terminal |
| Herdr TUI socket client `herdr.rs` (1,617) | NDJSON socket | **replace** with an hc client (JSON-RPC lines on the hc socket, or shelling out to `hc --json`) |

## 3. The rest of the port

### Labels, parent/child, spawn (tested)

- `--label k=v` at `hc run`, `hc label <t> k=v` later (`k=` removes);
  each change is a `labels` event with the full map; labels survive core
  restart and stop/start. `hc ls` JSON leaves labels out; `hc get` and
  events carry them. `hc run` output also has `"agent":"claude"` (from the
  command name; a template's `sh -c` shows `"agent":"sh"`).
- Idempotent spawn: a request label (`corgi.request=<id>`) can replace the
  `corgi_request` pane token; check `events`/`get` for it before
  starting. `--name` is unique among live terminals, which also guards
  against a double start.
- Workers that should show in the app need `herdr.space=<id>` (and
  optionally `herdr.tab=<id>`); "unlabeled workers stay hidden" (from
  docs, `herdr-app --skill`).
- Handover survival: see section 2; with the supervisor in a shell
  terminal, workers, address and labels survive `/exit` and relaunch.
  With `claude` as the terminal's program, the terminal finishes on
  `/exit` and takes non-outliving children with it.

### Restart behaviour (tested, isolated core)

- `hc server restart`: programs keep running (same Claude pid), labels and
  `--keep` kept, `hc send --wait` worked right after. The events stream
  ends (exit 0).
- `hc server stop` then `start`: programs end. The `--keep` supervisor came
  back as `/bin/zsh -l` at the same address with its labels
  (`corgi-session`, `corgi-resume`); the non-kept child finished with
  `reason: server_stop`.
- Resume from a label: `hc send sup "cd <project> && claude --model haiku --resume <id>"`
  brought the session back; asked what it last replied, it answered with
  the word from before the stop. The session id was chosen at launch with
  `claude --session-id <uuid>` and stored as a label, so no transcript
  scanning is needed to find it. Whether the kept shell comes back in the
  project folder was not checked; always `cd` first.

### Daemon placement: launchd (tested)

A throwaway LaunchAgent was loaded:
`~/Library/LaunchAgents/io.github.zyrre.corgi-spike.plist`, label
`io.github.zyrre.corgi-spike`, `RunAtLoad`, `KeepAlive`,
`ThrottleInterval` 5, running `/bin/sh -c` → `hc events` against the
isolated core, appending to a scratch log. It was unloaded with
`launchctl bootout gui/$UID/io.github.zyrre.corgi-spike` and the plist
deleted.

- launchd's `PATH` is `/usr/bin:/bin:/usr/sbin:/sbin`: `hc` in
  `~/.local/bin` was not found (exit 127) until called by full path.
- When the core stopped, `hc events` exited 0 and launchd restarted it
  after the throttle; the restarted follower **started a new core**, which
  inherited launchd's bare environment: its terminals got
  `PATH=/usr/bin:/bin:/usr/sbin:/sbin`, and a `claude` started there
  reported `blocked`, `kind: auth`, `Login expired · Please run /login`.
  The cause of the auth failure (keychain from a launchd context, or a
  missing variable) was not pinned down.
- A background hc terminal is no place for the daemon: it ends on
  `hc server stop` or a reboot and comes back, if kept, as a bare shell,
  and nothing restarts it. It would also inherit whatever environment the
  core has.

So: a LaunchAgent (systemd user unit on Linux) with `KeepAlive`, running
`corgi daemon` through the user's login shell (`/bin/zsh -lc 'exec corgi daemon'`)
or with `PATH` and the user's environment set in the plist, since the
daemon may be the one that starts the core at login. The daemon reconnects
whenever its event stream ends.

### The core's environment (tested): an important trap

The user's running core had been started from inside a Claude Code session
in a Herdr TUI pane, so it, and every terminal it starts, carries
`CLAUDE_CODE_CHILD_SESSION`, `CLAUDE_CODE_SESSION_ID`, the messaging
socket and token, and `HERDR_PANE_ID`/`HERDR_SOCKET_PATH`/… of that pane.
Every Claude in it showed `⚠ Transcript saving is off — inherited
CLAUDE_CODE_CHILD_SESSION marker · restart with
CLAUDE_CODE_FORCE_SESSION_PERSISTENCE=1 to keep future transcripts`.
Corgi reads transcripts for reports and for the handover's context and
cache figures, so Corgi must launch agents with those variables removed
(an `env -u … claude …` argv, or `terminal.run`'s `env`, which can only
set, e.g. `CLAUDE_CODE_FORCE_SESSION_PERSISTENCE=1`; not tested). The
`HERDR_*` variables would also point today's `corgi` at the TUI. In the
isolated core started with a clean environment, transcripts were written
to the usual `~/.claude/projects/<folder slug>/<session id>.jsonl`.

### Role injection and transcripts

- Claude (tested): flags work as outside hc: `--session-id`,
  `--append-system-prompt "…"`, `--resume`; transcript location as above.
  A template's body becomes the start of the first message, not a system
  prompt; it fits a short instruction, while Corgi's ROLE.md (25.8 KB)
  should keep coming in as today.
- OpenCode and Codex: see section 4.

### A minimal mod (tested with `herdr-app check`; the window was not opened)

`~/.config/herdr-app/mods/corgi-spike/main.js`:

```js
import { View, div } from "gpui-kit";
import { v_flex, h_flex } from "gpui-base";
import { agents, exec, theme } from "herdr";

export default class CorgiSpike extends View {
  init(_props, cx) {
    this.hcVersion = "…";
    cx.spawn(async (cx) => {
      const r = await exec("hc", ["--version"]);
      this.hcVersion = r.code === 0 ? r.stdout.trim() : `exec failed (${r.code})`;
      cx.notify();
    });
  }
  render(cx) {
    const t = theme();
    const rows = agents();
    return v_flex().size_full().bg(t.surface).p(10).gap(4).text_size(13)
      .child(div().text_color(t.muted).child(`exec: ${this.hcVersion} · ${rows.length} agents`))
      .children(rows.map((a) =>
        h_flex().id(`a-${a.address}`).gap(8)
          .child(div().text_color(t.foreground).child(a.address))
          .child(div().text_color(t.dim).child(`${a.status} ${a.app ?? ""} ${a.title}`))));
  }
}
```

`herdr-app check ~/.config/herdr-app/mods/corgi-spike` → `check passed`.
Pitfalls: `View` and `div` come from `"gpui-kit"`, `v_flex`/`h_flex` from
`"gpui-base"`, data and `exec` from `"herdr"` (importing `View` from
`"herdr"` fails with `Could not find export 'View'`); `render` must return
an element, not `null`; `check` writes `gpui-kit.d.ts` and `jsconfig.json`
into the mod folder; `AGENTS.md` and `herdr.d.ts` are written to the app
home only when the app starts (they are also readable with `strings` on
the app binary). From docs: `exec` runs with the user's real environment;
`terminals()` is `hc ls --json` plus status; `render` reruns whenever live
data changes, so no polling timers; actions reach a mod only as
`herdr-app do <mod>.<command>` with no payload.

## 4. OpenCode and Codex under hc

### OpenCode 1.18.33 (tested)

Run as `/Users/peter.krantz/.opencode/bin/opencode -m litellm/claude-haiku-5-5`
(it is on the interactive shell's `PATH` only, so hc needs the full path or
a `PATH` set for the terminal) in the scratch project, on the user's core
(the one with the leaked `CLAUDE_CODE_*`/`HERDR_*` environment).

**Status: OpenCode does not emit OSC 7501.** `hc get` showed
`"status":null,"reported":false` at start, through a turn and on a
permission prompt; the observer paint's only OSCs were the title
(`OC | <session title>`) and colour queries (10/11/12); the binary contains
no 7501 escape. So for hc it is a plain program:

- `hc send` answers `{"started":null,"status":null,"turn":0}`;
  `--if idle` and `--if working` both refuse (`t16 is silent, not idle`,
  exit 10); `send --wait` runs to its timeout (`reason: timeout`), since
  "a program that never reported status is working until it exits";
  `hc wait` likewise; `turn` never moves; no `status`/`turn` events, so
  Corgi's daemon would see no transitions at all.
- **A send into its permission dialog approves it, without `--force`.**
  Asked to read `/etc/hosts`, OpenCode showed `△ Permission required ←
  Access external directory /etc` with `Allow once  Allow always  Reject`;
  a plain `hc send spike-oc "Reply X"` went in (hc had no `blocked` to
  refuse on) and Enter chose `Allow once`; the file was read.
- **A send during OpenCode's boot was dropped silently** (`started: null`
  could not tell): the box stayed empty. hc's manual warns of this for any
  agent ("a send to a booting agent can be dropped").

**A project-local plugin makes it report (tested).**
`<project>/.opencode/plugin/hc-status.js` (no global config) listening to
OpenCode's plugin events and writing `ESC ]7501;state=…:app=opencode ESC \`
to `/dev/tty`:

```js
import { writeFileSync } from "node:fs";
const say = (state, extra = "") => {
  try { writeFileSync("/dev/tty", `\x1b]7501;state=${state}:app=opencode${extra}\x1b\\`); } catch {}
};
export const HcStatus = async () => ({
  event: async ({ event }) => {
    switch (event.type) {
      case "session.status":
        if (event.properties?.status?.type === "busy") say("working");
        else if (event.properties?.status?.type === "idle") say("done");
        break;
      case "session.idle": say("done"); break;
      case "permission.asked": say("blocked", ":kind=permission"); break;
      case "question.asked": say("blocked", ":kind=question"); break;
      case "permission.replied": say("working"); break;
      case "session.error": say("error"); break;
    }
  },
});
```

With it, hc saw `{"app":"opencode","state":"done"}`, `turn` 1 after a
turn; `send --wait` returned `"reason":"status","status":"done"`; on the
`/etc/shells` permission prompt the status was
`{"app":"opencode","kind":"permission","state":"blocked"}` and a plain send
was refused (exit 9, `typing would answer its dialog`). Event names seen in
its log: `session.created`, `session.updated`, `session.status`
(`{"type":"busy"}` / `{"type":"idle"}`), `session.idle`, `session.diff`.
Gaps: nothing is reported until the first event, so a fresh OpenCode has
no status (the plugin could say `idle` when it loads; not tried), and
`session.error`/`question.asked` were not triggered. Corgi would install
this plugin (globally in `~/.config/opencode/plugin/`, or per worktree)
the way the Codex hook below would be installed; the user's call.

**Sends and drafts (tested, same as Claude):**

- idle, empty box: the text was submitted at once.
- `hc keys` draft `my draft`, then send: `my draftReply with just SENT-B.`
- a person's paused draft: send waited until ~3 s after the last key and submitted
  `half typedReply with just SENT-B2.`
- a person typing: send held until ~4 s after the last key, then submitted
  `aaaa … hhhhReply with just SENT-C.`
- mid-turn: OpenCode queues the message itself (a `QUEUED` badge under it)
  and runs it after the turn (`QUEUED-D` answered).

**Draft detection on its observer paint (tested):** the paint has the same
shape as Claude's (rows separated by `\r\n`, SGR styling, cursor position
at the end) but a different box, and it styles with truecolor, not dim:

```text
  ┃  my draft                         ← box text rows (one or more)
  ┃                                     
  ┃  Build · claude-haiku-5-5 LiteLLM  ← agent · model row, still in the box
  ╹▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀ ← bottom edge
```

- The box is the run of rows starting (after two cells of margin) with
  `┃` in the accent colour (`38;2;92;156;245` here), ending at the row
  with `╹`. The row just above `╹` is the agent/model line and not input.
- Box cells have the box background (`48;2;30;30;30`); a sidebar on the
  same rows (wide terminals) has another background (`48;2;20;20;20`), so
  the parser must keep only cells on the box background.
- Typed text is in the text colour (`38;2;238;238;238`); the placeholder
  (`Ask anything… "Fix broken tests"`, shown at start) is in the muted
  colour (`38;2;128;128;128`), the same colour as the `·` and `LiteLLM`
  on the agent row. So: **draft = any non-space cell on the box background,
  in the box's text rows, whose colour is not the muted one.** The colours
  come from OpenCode's theme, so read them from the agent row rather than
  hard-coding them.
- The cursor helps too: column 6 with an empty box, column 14 after
  `my draft` (row 114 both times); the input starts two cells after `┃`.
- `draft.rs` needs a third `Harness::OpenCode` branch for this, with
  `styled_chars` extended to keep each cell's foreground and background
  colour, not only whether it is dim. Corgi's `Harness::OpenCode` exists
  already (`crates/corgi-core/src/harness.rs`).

**Sessions and resume (tested):** sessions are rows in SQLite,
`~/.local/share/opencode/opencode.db` (tables `session`, `message`, `part`,
…), with OpenCode-chosen ids such as `ses_e4047094affe2nD7pkoQn2fWfR`;
there is no flag to choose one at launch. `opencode session list --format
json -n N` gives `{id, title, updated, created, projectId, directory}`
across all folders, so Corgi finds a worker's session as the newest one
with its worktree as `directory` created after the launch (the terminal
title `OC | <title>` matches the session's `title` too).
`opencode export <id>` prints `{info, messages}` with every part's text and
each assistant message's `tokens` (`input`, `output`, `reasoning`,
`cache.read`, `cache.write`) and `cost`: what Corgi's report and ctx/cache
readings need. Resume: `opencode -m <model> -s <id>` in the same folder;
after a kill, a relaunch with `-s` answered `PONG2` when asked for its first
reply. `corgi.resume` label: `opencode -m litellm/claude-haiku-5-5 -s
ses_…` (with `--agent <name>` if it had one).

**Role (tested):** an inline agent through the environment, no file in the
user's config:

```sh
env 'OPENCODE_CONFIG_CONTENT={"agent":{"corgi-spike":{"mode":"primary","model":"litellm/claude-haiku-5-5","prompt":"{file:/abs/role.md}"}}}' \
  opencode --agent corgi-spike
```

Asked "What is your role?", it answered `ROLE-FILE-OK` as `role.md` told
it to. From docs/binary only: `OPENCODE_CONFIG_DIR`, an `instructions`
list in the config, and the project's `AGENTS.md` are other ways in;
`--prompt` sets only the first message.

**Leaked environment (tested):** OpenCode ran normally in the polluted core
and saved its sessions; its binary reads no `CLAUDE_CODE_CHILD_SESSION`,
no `HERDR_*` and no `HC_*` variable (it knows only
`CLAUDE_CODE_SKILLS`/`_SSE_PORT`/`_PROMPT`/`_AGENT_RULE_DISABLED`, none of
which leak). The `PATH` point above matters, though: a core with launchd's
bare `PATH` cannot find it by name.

### Codex (from docs and web sources only; not installed, not run)

Corgi already runs Codex supervisors and workers (from Corgi's code:
`Harness::Codex` in `crates/corgi-core/src/harness.rs` passes the role as
`-c developer_instructions=<TOML string>` and trusts the project with
`-c projects={<root>={trust_level="trusted"}}`; it exits with `/quit`;
`crates/corgi-core/src/session/locate.rs` reads rollouts under
`~/.codex/sessions`). What hc changes:

- **`--no-daemon`.** hc's manual: "Codex must run with `--no-daemon`
  (including templates), or its commands execute outside hc"
  (`hc --skill`). Since Codex 0.157 an interactive session starts or
  reuses a persistent background server (`codex exec-server`), which
  outlives the session and ignores per-session overrides when one already
  runs; config `[daemon] auto_start = false` turns it off
  ([third-party blog](https://codex.danielvaughan.com/2026/09/25/codex-cli-daemon-auto-start-stable-default-v0157/)).
  The flag itself appears only in hc's manual and in a user report run as
  `codex.exe --no-daemon` ([openai/codex#60326](https://github.com/openai/codex/issues/60326)),
  not in OpenAI's docs found; check `codex --help` once installed. For
  Corgi: add `--no-daemon` to every Codex argv it starts under hc, and to
  the `corgi.resume` label; otherwise Codex's commands would not run in the
  hc terminal (nor end with it).
- **OSC 7501: sources disagree.** "Codex reports natively since 0.163
  (`app=codex`; nothing while its "Hooks need review" panel is up)", with
  0.166 "observed live in throwaway hc terminals"
  ([keinstn/drover#369](https://github.com/keinstn/drover/pull/369)).
  A later-merged PR (2026-11-08) says Codex does not, though its author
  could not run Codex ("both logins expired"), and emits the record
  from Codex's hooks instead: `ESC ] 7501 ; state=<s>[:kind=<k>]:app=<agent> ESC \`
  to `/dev/tty` (else the agent's tty found through its ancestors) when
  `$HC_TERMINAL_ID` is set; PermissionRequest → `blocked:kind=permission`,
  Stop/agent-turn-complete → `done`, SessionStart → `idle`, StopFailure →
  `error`, prompt/tool events → `working`
  ([sebastienrousseau/dotfiles#1398](https://github.com/sebastienrousseau/dotfiles/pull/1398)).
  Known gap: with a background terminal still running after a turn, Codex
  keeps reporting `working` (Codex 0.167.0-alpha.3.2 under herdr-core
  0.1.19; open, [openai/codex#60326](https://github.com/openai/codex/issues/60326)).
  Codex's own `notify = [...]` runs a program only on
  `agent-turn-complete` ([config sample](https://developers.openai.com/codex/config-sample)),
  too little for blocked/working. So: check native reporting first on the
  installed version; if missing, a hook like the OpenCode plugin above.
- **Resume:** `codex resume <SESSION_ID>` (or `--last`); transcripts are
  `~/.codex/sessions/YYYY/MM/DD/rollout-<timestamp>-<uuid>.jsonl`, the
  uuid being the session id, with an index in `~/.codex/state_*.sqlite`
  ([Codex session lifecycle, third-party](https://codex.danielvaughan.com/2026/04/11/codex-cli-session-lifecycle-resume-fork-rollouts),
  [session management, third-party](https://inventivehq.com/knowledge-base/openai/how-to-resume-sessions)).
  No source showed a way to choose the id at launch; Corgi already finds
  the rollout after launch. `corgi.resume` label: `codex --no-daemon
  resume <id>` plus Corgi's `-c` overrides (role, trust), since a resume
  does not keep them (as `restart.rs` already handles for Herdr's
  `resume_argv`).
- **Role:** `developer_instructions` ("injected before AGENTS.md") adds
  to Codex's own instructions; `model_instructions_file` replaces the
  built-in base instructions; both in `~/.codex/config.toml` or a
  project-scoped `.codex/config.toml`; `AGENTS.md` is read up to
  `project_doc_max_bytes` (32768)
  ([config sample](https://developers.openai.com/codex/config-sample)).
  Corgi's existing `-c developer_instructions=…` works the same under hc.
- **Drafts:** Corgi's `draft.rs` already parses Codex's `›` composer from a
  styled screen; on the observer paint it should work unchanged, but that
  is untested.
- **Environment:** whether Codex reacts to the leaked `CLAUDE_CODE_*`/
  `HERDR_*` variables is unknown; nothing found in the sources.

To verify once Codex may be installed: native 7501 words through a turn, a
permission prompt and the hooks-review panel; `--no-daemon` exists and
keeps tool commands in the terminal (`hc get`'s `foreground` during a
command); resume with the `-c` overrides; `draft.rs` on its observer paint.

## Plan changes for the port's next workers

### CLI and daemon

1. Replace `herdr.rs` with an hc client: `terminal.run/get/list/read/send/keys/label/kill/rm`,
   `events.subscribe`, `terminal.attach {observe}`. Decide JSON-RPC on the
   socket (needs the framed-mode spec for attach) or shelling out to
   `hc --json` (simpler; attach via a pty).
2. `corgi daemon`: a launchd LaunchAgent / systemd user unit with
   `KeepAlive`, started through the login shell (or with the user's `PATH`
   in the unit), holding a `flock` so one runs per core. It follows
   `events` filtered by a Corgi label, reconnects whenever the stream ends,
   and reconciles from each snapshot (never compare `seq` across
   connections). It also does the handover and resume-after-stop passes.
3. Spawn: `hc run --bg --name <worker> --cwd <worktree> --no-notify
   --label corgi.project=… --label corgi.role=worker --label corgi.request=<id>
   [--label herdr.space=<id>]`, with the `CLAUDE_CODE_*`/`HERDR_*`
   variables removed from the agent's environment. Parent: optional,
   for display only; if set, also `--outlive-parent`.
4. Supervisor launch: a `--keep` shell terminal (`hc run --bg --keep
   --name <project>-supervisor --label corgi.role=supervisor …` with no
   command), then `hc send` the agent's argv into it. Store
   `corgi.session` (from `claude --session-id`) and `corgi.resume` labels.
   Handover: `/exit`, wait for the `foreground` event that shows the shell,
   send the successor's argv. After a core stop or reboot, the daemon finds
   kept shells with `corgi.resume` and sends it.
5. Wakes: rest = `done` or `idle` (not `--if idle`); before each send,
   check that the foreground is the agent (never type a wake into the
   shell), check the observer paint with `draft.rs` (per harness: see
   section 4 for OpenCode's box), never `--force`. The check-then-send race
   is accepted; log sends that took over ~1 s as possible draft merges.
6. Keep `inbox`, `report`, `digest`, ledger, roles as they are; folder trust
   becomes `hc keys <t> down enter`.
7. Harnesses: Codex argv gets `--no-daemon`. OpenCode needs Corgi's
   status plugin (section 4) before the daemon can see it at all; without
   it, an OpenCode worker gives no wake and a send can answer its
   permission dialog, so Corgi must not send to an OpenCode terminal that
   has never reported. OpenCode's session id is found after launch
   (`opencode session list --format json`), its report and token counts
   come from `opencode export <id>`, its role from an inline agent in
   `OPENCODE_CONFIG_CONTENT`. Launch it by full path or with `PATH` set.

### JS mod

- Lists Corgi's agents from `agents()`/`terminals()` filtered by labels and
  acts through `exec("corgi", […])`; renders on data change, no polling.
- It never triggers or delivers wakes (it runs only while the window is
  open). It may show the inbox and open questions by reading files.
- Mind that the app turns `done` into `idle` once seen; the daemon, which
  reads the core, does not see that.

### Removal (once the mod works)

The Herdr TUI plugin (`herdr-plugin.toml`, `corgi hook`, `scripts/install.sh`
plugin parts), `corgi serve` and its protocol, `corgi-gui`, the ratatui
TUI, the dashboard-presence and `.dirty` code, the lock takeover between
dashboards, and `markers.json`/pane tokens where labels replace them.

## Open questions

- ~~A send that refuses a non-empty box.~~ Settled (user, 2026-11-09):
  accept the check-then-send race; no upstream request. The daemon may log
  sends that took over ~1 s as possible merges.
- **Framed protocol for `terminal.attach`.** Needed for a Rust observer
  without a pty; get it from herdr-core's source or maintainers.
- **Which environment starts the core at login.** If the app or another hc
  client starts it from inside an agent session, the markers leak in again.
  Corgi should clean the agent's environment itself (recommended) rather
  than rely on how the core was started.
- **Whether `CLAUDE_CODE_FORCE_SESSION_PERSISTENCE=1` alone is enough**
  under a polluted core; not tested.
- **Installing Corgi's OpenCode status plugin.** Globally
  (`~/.config/opencode/plugin/`, one file, every OpenCode session reports
  to hc, harmless outside hc since `/dev/tty` gets an ignored OSC) or per
  worktree (`.opencode/plugin/`, which lands in the user's repos). I
  recommend globally, installed by `corgi` with the user's consent, like
  the Codex hook if Codex turns out not to report natively.
- **Codex** stays unverified (not to be installed now): native 7501,
  `--no-daemon`, resume with overrides, drafts on the observer paint.

## Clean-up done

- Test terminals t1–t15 on the user's core (all named `spike-*`, started
  by this spike) killed and removed by address; `hc ls --all` →
  `{"hidden_finished":0,"terminals":[]}`.
- The isolated core (`HC_SESSION=s`, state in `~/.corgi-spike-hc`)
  emptied, stopped, and its state folder deleted. Its `notes = false`
  config lived in the session scratchpad, never in `~/.config/herdr-core`.
- `~/.config/herdr-core/` (created only for the throwaway template
  `templates/corgi-spike.md`; it did not exist before) deleted.
- The LaunchAgent above unloaded and its plist deleted.
- The test sessions' Claude project folder (keyed by the scratch folder)
  deleted from `~/.claude/projects/`.
- OpenCode follow-up: test terminals t16–t19 and t1b (`spike-oc*`)
  killed and removed by address; `hc ls --all` empty again. The three
  OpenCode sessions created in the scratch project deleted with
  `opencode session delete`. The status plugin lived only in the scratch
  project's `.opencode/plugin/`; no OpenCode config was changed.
- Kept on purpose: the mod in `~/.config/herdr-app/mods/corgi-spike/`, as
  the spike asked for it there. `~/.config/herdr-app/` did not exist
  before; delete it to undo.
