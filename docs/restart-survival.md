# Surviving a restart: what happens today, and options

Investigation, 3 November 2026, on Herdr **0.9.0** (protocol 22, the
installed Homebrew build) and Corgi 0.2.3. The question: when the computer
restarts or the Herdr server has to stop, how much of a user's agents comes
back by itself (panes, conversations, Corgi's marks and supervisor roles,
wakes), and should Corgi make the rest automatic or put it behind a button?

Claims are tagged **[observed]** (seen on a private Herdr, below),
**[code]** (read from Corgi's source, with the file) or **[docs]** (Herdr's
documentation for 0.9.0, or 0.9.5 where it says so). Nothing here was tried
on the user's own Herdr server, which was not stopped, restarted or handed
off.

## How it was observed

A private Herdr session (`env -i`, private `HOME` and `XDG_*` directories,
the server's `SHELL` a wrapper that keeps the fakes first on `PATH`), with
the plugin linked from a copy of the manifest and Corgi's state under
`target/`. Fake `claude` and `codex` binaries logged every start with its
arguments. On it: a supervisor started with `corgi supervisor`, a Claude
worker and a Codex worker started with `corgi spawn`, each given a native
session through Herdr's own integration hook scripts (installed into the
private home), and a hook pass so Corgi's record held their sessions. Then:

1. a live handoff, sent as `server.live_handoff` with `import_exe` set to the
   same Herdr binary on the private socket;
2. `herdr session stop`, a `corgi notify` while it was down, and a headless
   `herdr server` start;
3. the same again with `[session] resume_agents_on_restore = false` in the
   private config.

A reboot cannot be simulated: the boot identity is the kernel's. From
Herdr's side a reboot is (2) after a cold start; what differs for Corgi is
read from code.

## What happens today

### (a) Live handoff

| | Claude worker | Codex worker | Supervisor |
| --- | --- | --- | --- |
| Process | Same PID, keeps running **[observed]** | Same **[observed]** | Same **[observed]** |
| Herdr's native session ref | Kept **[observed]** | Kept **[observed]** | Kept **[observed]** |
| Corgi's tokens (`corgi_request`, `corgi_handler`, workspace roles) | Dropped by Herdr, put back by the startup hook within 28 ms ("Reconciled 6 of Corgi's pane and workspace marks") **[observed]** | Same **[observed]** | Same; still a supervisor **[observed]** |
| State | `unknown`, then the real state at the next report **[observed]** | Same | Same |

Nothing is lost that matters; the work is already automatic. Two limits:

- **This user cannot start a handoff the documented way.** `herdr update
  --handoff` is disabled for Homebrew installs **[docs]**, and the user's
  Herdr is Homebrew's. The socket method `server.live_handoff` (in `herdr api
  schema --json`, with `import_exe`, `expected_version`,
  `expected_protocol`) did hand off a private server to the same binary
  **[observed]**; whether 0.9.0 can hand off to a newer Homebrew build that
  way was not tried.
- In-flight API calls, waits and client sockets may be cut **[docs]**: a
  `corgi spawn` or merge running at that moment can fail at its current
  stage and say so.

### (b) Herdr server stop, then start

Herdr restores workspaces, tabs and panes under the same IDs **[observed]**,
keeps each pane's native session ref **[observed]**, and drops every
metadata token **[observed, docs]**. Then, with the default
`resume_agents_on_restore = true` (the user's config has no `[session]`
table, so it is on):

| | Claude worker | Codex worker | Supervisor |
| --- | --- | --- | --- |
| Process | Herdr runs `claude --resume <id>` in the pane, about a second after the server starts, **with no client attached** **[observed]** | `codex resume <id>` **[observed]**, but only with Herdr's Codex integration installed; the user's is not, so their Codex panes come back as plain shells **[observed: `herdr integration status`]** | `claude --resume <id>` **[observed]** |
| Launch options | **Lost**: only `--resume <id>`; a model, effort or permission flag given at launch is not passed **[observed]** | **Lost**: only `resume <id>` **[observed]** | **Lost: `--append-system-prompt-file ROLE.md` and `--add-dir <state dir>`** **[observed]**. The resumed supervisor has no role instructions and no write access to its memory. Corgi's own restore adds both back (`recovery/native.rs`) **[code]** |
| Corgi's tokens | Put back by the startup hook, matched by native session **[observed]** | Same **[observed]** | Same: Corgi treats it as the supervisor again **[observed]** |
| Wakes | Followed as before (`seen.json` is on disk) **[code]** | Same | **An inbox item queued while Herdr was down was typed into the resumed supervisor** at the first pass **[observed]**, so Corgi wakes a supervisor that has lost its role |

With `resume_agents_on_restore = false` every pane comes back as a shell in
its directory, still carrying its session ref; the startup hook puts back
the workspace marks and keeps the pane marks for later **[observed]**. That
is the state Corgi's `S` → Restore flow expects (its Manual recovery setup
sets this value): Restore then starts each saved conversation in its
original pane with its full options and the role **[code, and the flow's
own verification in `docs/session-recovery-design.md`]**.

Herdr's docs say restored agents resume "after a client attaches"; 0.9.0
resumed them headless, with no client at all **[observed]**.

### (c) Machine reboot

From Herdr's side, the same as (b). On top of that:

- **Who starts Herdr.** The user has Homebrew's launchd service
  `sh.brew.herdr` (`RunAtLoad`, `KeepAlive`, `herdr server`), so a server
  can start at login with no window **[observed: plist]**. On the current boot
  the running server's command line is not the plist's, so a client started
  it, and the service has exited with "herdr server is already running" every
  few seconds since: `/opt/homebrew/var/log/herdr.log` holds 25 MB of that
  line **[observed]**. Either way, Corgi's startup hook runs as the server
  starts, before any window is open **[observed in (b)]**.
- **Nothing saves at shutdown on macOS.** Herdr's shutdown-time save is
  Linux/logind only **[docs 0.9.5]**; on macOS the layout is whatever
  `session.json` last held.
- **Recovery holds.** Only a reboot counts as a new boot: a server restart
  without one changes no boot id **[code: `recovery/store.rs` `boot`]**. If
  the user pressed `S` → Save before the reboot, every saved supervisor that
  comes back with the same native id is **held**: no stop reports, inbox
  deliveries or handovers until `S` → Continue held coordination
  **[code: `recovery::held_projects`]**. Inbox items that were pending at Save
  stay quarantined after the reboot until `corgi inbox <project> --take`
  **[code: `recovery::begin_delivery`]**. Without a Save, nothing is held,
  and (b) applies unchanged.
- **If Herdr auto-resumed first,** `S` → Restore lists those agents as
  "Already running … (reuse)" by native id and starts nothing for them
  **[code: `recovery::plan_entry`]**, so it cannot give a roleless
  supervisor its role back. The user would have to quit the agent first.

### What survives, by kind of state

| State | Where it lives | Survives (b)/(c) |
| --- | --- | --- |
| Conversations | Claude's and Codex's own stores | Yes **[docs]** |
| Corgi's pane and workspace marks | `markers/<socket>.json`, put back by the startup hook | Yes, when the pane holds the same native session **[observed]** |
| Followed workers (`seen.json`), handover phase (`handovers.json`), inboxes, delivery journal | On disk under `wake/`, `supervisors/`, `recovery/` | Yes **[code]** |
| MERGE tags (`corgi_merge`) | In the marks record | Put back, but the agent's state counter restarts at 1, so a tag set at an "idle N" turn reads as stale and is cleared unless N happens to match **[code: `supervisor::merge_tag`]** |
| A handover in progress | `handovers.json` | Resumed from its phase against the restarted counter: an unanswered `Asked` times out to Failed; a `Noted` or answered phase can replace the resumed supervisor on the first pass **[code: `supervisor.rs` around `Handover::observe`]** |
| A supervisor replacement mid-flight | Its job lived in the process that ran it | Lost; the project waits up to `REPLACEMENT_SECS` (3 min), then counts it failed **[code]** |
| Idle prompt-cache handover timer | `resting` in `handovers.json`; only a dashboard acts on it | Kept from before the restart, so a dashboard can fire it at once **[code]** |
| Stops while Herdr was down | Nowhere: Herdr runs no hooks while down | Lost; the first pass compares what is there **[docs, code]** |
| Typed but unsent prompt text, running tools | The processes | Lost **[docs]** |

### The GUI

`corgi serve` and corgi-gui have no Sessions flow: no Save, Restore,
Manual setup or Continue **[code: `docs/serve-protocol.md`,
`crates/corgi-core/src/protocol.rs`]**. A project held after a reboot shows
only as the status line ("N recovered project(s): coordination held · S
sessions"), and the project's `held` count covers quarantined deliveries,
not recovery holds **[code: `waker.rs`, `serve/state.rs`]**. A GUI-only user
cannot release a hold.

## The gaps

1. **Herdr's default auto-resume brings supervisors back without their role
   or memory access**, and Corgi then marks and wakes them as supervisors.
   Workers lose their launch model, effort and permission flags. This is the
   default for every user who has not run `S` → Manual recovery setup.
2. **Codex agents come back only with Herdr's Codex integration**, which this
   user does not have. Without it there is no native id: neither Herdr's
   resume nor Corgi's Save can bring a Codex conversation back.
3. **The recovery that does it right is manual and TUI-only:** set up once,
   Save before the restart, Restore after it with each entry picked by
   hand (zero start selected), then Continue. Forgetting the Save before an
   unplanned restart (a crash, a power cut, a forced update) leaves nothing
   to restore.
4. Smaller, read from code and not reproduced live:
   - The state counter's restart makes resting agents look freshly returned
     (`supervisor::Transitions::observe`), which can send a supervisor wake
     right after a restart.
   - A held project whose supervisor was replaced rather than restored (a
     new native id) cannot be continued: delivery refuses on the saved
     entry's project, but Continue lists only live matching supervisors
     (`recovery::begin_delivery` against `held_projects`).
   - Each leading pass starts `sysctl` and `ioreg` processes once a recovery
     store exists, to read the boot and host identity.

Fixed on the way (its own commit, with a test): on macOS the boot identity
was the whole `kern.boottime` text, which ends in a local-time date, so a
time zone change looked like a reboot and armed every same-boot Save's
holds. It now keeps only `{ sec, usec }`, and older records still compare.

## Options

### 1. Herdr resumes Corgi's agents with the right command (automatic)

Herdr 0.9.2 and later take a `resume_argv` with an agent's report, and on
restart run that instead of the built-in `claude --resume <id>` **[docs
0.9.5]**. Corgi would report, at launch and whenever an agent's session
changes, `claude --resume <id> --append-system-prompt-file ROLE.md --add-dir
<state>` for a supervisor, and the launch's model, effort and extra
arguments for a worker (Codex likewise). After any restart, Herdr starts
every agent correctly with no click, and the startup hook already restores
the marks.

- Cost: small in Corgi (one report at launch, re-checked by the
  reconciliation pass), plus upgrading Herdr from 0.9.0 (Homebrew has 0.9.5)
  and installing the Codex integration.
- Risk: Herdr keeps the command "only while the same `source` and `agent`
  hold the pane". Herdr's own `herdr:claude` and `herdr:codex` integrations
  hold these panes, and its docs tell other reporters not to use a `herdr:`
  source. Whether Corgi can attach a command without taking over state
  reporting, and whether the integration's next session report drops it, is
  **untested**. The command must start with a plain name on `PATH`, not a
  path such as `CORGI_CLAUDE_BIN`, and no argument may contain an apostrophe.

### 2. Corgi repairs what Herdr's resume lost (automatic, works on 0.9.0)

After a server start, the startup hook and the passes after it compare each
Corgi agent's foreground argv (`pane.process_info`) with what Corgi
launched it with (the launch profile `recovery::record_launch` writes, and
the supervisor arguments). A supervisor without its role, or a worker
without its options, is exited while it is idle with an empty input box and
started again in the same pane with `--resume <id>` and its full arguments
(the restore's `native` adapter). Its project's coordination is held until
the new process reports the same session.

- Cost: medium. A replacement-like background job in the hook (the
  supervisor replacement already runs one for up to 90 seconds), the argv
  check, and tests against the fake Herdr.
- Risk: Corgi stops a process Herdr started seconds earlier. The idle and
  empty-box checks guard against stopping a user mid-sentence, but the
  first seconds after a restart are when the user may start typing. Matching
  argv is a heuristic for options Corgi did not set itself.

### 3. One button: "Restore after restart" in the TUI and GUI

Keep Herdr's auto-resume off (the existing Manual setup, offered once). Make
saving automatic: a hook pass writes a fresh manifest when the set of Corgi
agents changed, so an unplanned restart still has one. After a server start,
the TUI and GUI show "Herdr restarted · N agents to restore · Restore all",
which runs the existing restore with every eligible entry of the newest
manifest preselected and one confirmation, and continues coordination for
each supervisor that came back with its confirmed identity. A "Prepare for
restart" action saves and lists what will not survive (busy agents, missing
ids, a missing Codex integration). The GUI gets Sessions through
`corgi serve`.

- Cost: medium to large: new serve commands and GUI views, automatic saves
  within the store's 1,000-manifest bound, and a change to the approved
  design's "zero entries start selected".
- Risk: low (the user confirms once), but a restart still needs a press,
  and with auto-resume off globally, Claude panes Corgi did not start come
  back as shells unless Restore includes them.

## Recommendation

**Option 1, if a private test on Herdr ≥ 0.9.2 shows Corgi's command is
kept**, with option 2's check kept as a warning only: a status line and a
GUI badge "supervisor resumed without its role", not an automatic restart.
Option 1 is the only route where the right process comes back with no click
and no extra launches, and it leaves Herdr in charge of what Herdr does
well. If the test shows Herdr will not keep a command Corgi reports, choose
option 3: it builds on the reviewed restore engine, and its one press is
cheaper and safer than option 2's stop-and-relaunch.

Whatever is chosen, two setup steps help at once and need no code: install
Herdr's Codex integration (`herdr integration install codex`), without which
Codex conversations cannot come back at all, and look at the Homebrew
launchd service, which retries every few seconds while a server runs and
fills `/opt/homebrew/var/log/herdr.log`.
