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

The tests used Claude Code 2.1.317 with `--model haiku` in hc terminals
named `spike-*`. A second, isolated core (`HC_SESSION=s`, its own state
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
  between the check and the paste.
- **Can hc's notes replace Corgi's wakes? No.** Notes cover a child that is
  blocked, errors or exits; a child finishing its turn (`done`), which is
  Corgi's main wake, is not a note. Notes merge into the user's draft, go
  only to the terminal that started the child (fixed at launch), and
  `notes = false` is one switch for the whole core.
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
paint instead of `agent.read`'s styled screen. The daemon opens
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
cannot undo it. Closing the hole needs hc: a `send --if-empty`, or the
input-pending flag Claude Code itself knows about. Worth asking upstream.

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
- Draft: yes, with the small race above.
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
- Codex (from docs, `hc --skill`): must run with `--no-daemon`, or its
  commands run outside hc. Its OSC 7501 status is unconfirmed
  (supervisor research note: a hook can emit it). To verify once installed:
  status words through a turn and a permission prompt, the draft
  detector's `›` composer on the observer paint, `--no-daemon` with
  resume.
- OpenCode (from docs/research only): reported to emit OSC 7501 natively;
  unverified. To verify: status words, transcript location, resume argv.

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
   shell), check the observer paint with `draft.rs`, never `--force`.
   Log sends that took over ~1 s as possible draft merges.
6. Keep `inbox`, `report`, `digest`, ledger, roles as they are; folder trust
   becomes `hc keys <t> down enter`.

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

- **A send that refuses a non-empty box.** Only hc can close the
  check-then-send race. Ask upstream for `send --if-empty` or an input-
  pending flag; until then, accept the small race (recommended).
- **Framed protocol for `terminal.attach`.** Needed for a Rust observer
  without a pty; get it from herdr-core's source or maintainers.
- **Which environment starts the core at login.** If the app or another hc
  client starts it from inside an agent session, the markers leak in again.
  Corgi should clean the agent's environment itself (recommended) rather
  than rely on how the core was started.
- **Whether `CLAUDE_CODE_FORCE_SESSION_PERSISTENCE=1` alone is enough**
  under a polluted core; not tested.
- **Codex and OpenCode** need installing to verify status, drafts and
  resume (installs not yet approved).

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
- Kept on purpose: the mod in `~/.config/herdr-app/mods/corgi-spike/`, as
  the spike asked for it there. `~/.config/herdr-app/` did not exist
  before; delete it to undo.
