# Surviving a restart: what happens today, and options

Investigation, 3 November 2026, on Herdr **0.9.0** (protocol 22, the
installed Homebrew build) and Corgi 0.2.3; option 1 was also tested on the
official Herdr **0.9.5** release binary, run only as a private server. The question: when the computer
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
4. on a private Herdr 0.9.5, resume commands reported in several ways,
   then three stop/start rounds (see option 1).

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

## Options considered

### 1. Herdr resumes Corgi's agents with the right command (automatic)

Herdr 0.9.2 and later take a `resume_argv` with an agent's report, and on
restart run that instead of the built-in `claude --resume <id>` **[docs
0.9.5]**. Corgi would report, at launch and whenever an agent's session
changes, `claude --resume <id> --append-system-prompt-file ROLE.md --add-dir
<state>` for a supervisor, and the launch's model, effort and extra
arguments for a worker (Codex likewise). After any restart, Herdr starts
every agent correctly with no click, and the startup hook already restores
the marks.

**Tested on a private Herdr 0.9.5** (the official release binary, its
SHA-256 matching GitHub's digest), six panes, each given its session by
Herdr's own v11/v10 integration hook scripts, then three stop/start rounds
**[observed]**:

| Report | Result after restart |
| --- | --- |
| `pane.report_agent_session` from Corgi's own source (`plugin:io.github.zyrre.corgi`), session id and `resume_argv`, on a pane `herdr:claude` holds | **Kept and used**: `claude --resume <id> --append-system-prompt-file …/ROLE.md --add-dir …/state`. Corgi does not take over the pane's state: the session ref still names `herdr:claude` |
| The same for Codex on a `herdr:codex` pane: a worker's `--model` and `-c model_reasoning_effort`, a supervisor's `--sandbox`, `--ask-for-approval`, network and `--add-dir` | **Kept and used** |
| `pane.report_agent` from Corgi's source with a state | Used too, but it takes the pane's state authority: not wanted |
| The same command reported under `herdr:claude` or `herdr:codex` | **Ignored**: Herdr runs its built-in command for its own sources |
| A second restart with no new report | Kept |
| The official hook reporting the same session again (as Claude does on `SessionStart` after a resume) | Kept |
| The official hook reporting a **new** session (as after `/clear`) | **Herdr still ran Corgi's command, resuming the old conversation** |
| `pane.release_agent` from Corgi's source after that | Command dropped; Herdr's built-in resume of the new session is used, and the pane stays `herdr:claude`'s |
| The agent exits to the shell | Nothing resumed; the pane comes back as a shell |
| Over 8 KiB, an apostrophe, a newline (a control character), or a path as the command | Refused with `invalid_resume_argv` |

So option 1 works for Claude and Codex workers and Claude supervisors, with
these requirements on Corgi:

- **Report from Corgi's own source with `pane.report_agent_session`**, never
  under `herdr:` and never with `pane.report_agent`.
- **Keep the command's session current.** Each pass that sees a pane's
  native session differ from the one in the command Corgi reported must
  report again, or release it for a pane Corgi no longer owns. A session
  change just before a crash, before any pass saw it, resumes the previous
  conversation: the window is one hook pass (an `/clear` comes with state
  changes, which run hooks).
- **A Codex supervisor's role does not fit.** Corgi passes it inline as
  `-c developer_instructions=…`, and `supervisor/ROLE.md` is 25,768 bytes
  against Herdr's 8 KiB, with 58 apostrophes (escapable as `\u0027`). Codex
  supervisors need another way to carry the role on resume: see "Codex
  supervisors" under the chosen design.
- The command's first word must be a plain name on the pane's `PATH`
  (`claude`, `codex`), not `CORGI_CLAUDE_BIN`'s path.
- Herdr 0.9.0, which the user runs, ignores the field **[docs]**, so the
  report is harmless before the upgrade and needs no version check.
- With `resume_agents_on_restore = false` nothing is resumed **[docs]**,
  so the existing Manual recovery setup would turn this off as well.

Cost: small to medium in Corgi: one report after launch (where
`recovery::record_launch` already writes the launch profile), a check in
the leading pass, tests against the fake Herdr; plus upgrading Herdr to
0.9.5 and installing the Codex integration.

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

## Chosen design

Decided by the user on 3 November 2026 and built the same day; see "What
was built" below.

### Automatic restarts: option 1

After Corgi launches an agent, and whenever a pass sees its native
session change, Corgi reports the agent's full resume command to Herdr
(Herdr ≥ 0.9.2) with `pane.report_agent_session` from its own source
`plugin:io.github.zyrre.corgi`, following the requirements under option 1.
After a server restart or a reboot, Herdr then starts each agent in its
pane with its session and options, and the startup hook puts Corgi's
marks back. Nothing needs a click. A pane Corgi no longer owns has its
command released.

- **Claude supervisor:** `claude --resume <id> --append-system-prompt-file
  <ROLE.md> --add-dir <state dir>`, as tested.
- **Workers, Claude and Codex:** `--resume <id>` (`resume <id>` for Codex)
  plus the launch's model, effort and extra arguments, from the launch
  profile `recovery::record_launch` writes.
- **Codex supervisors:** see below.

### Codex supervisors

A Codex supervisor gets its role as `-c developer_instructions=<ROLE.md
text>`, too large for a resume command. Codex's own source shows that
`codex resume <id>` **does not keep those instructions for good** **[code:
openai/codex at tag `rust-v0.166.0`, read on GitHub; not tested, since
Codex is not installed on this machine and the question turns on a
compaction, which needs a model turn and so a login]**:

- The session's developer instructions are taken from the configuration of
  the process that runs, not from the rollout: `developer_instructions:
  config.developer_instructions.clone()` when the session is built
  (`codex-rs/core/src/session/mod.rs:940`). The per-turn record that resume
  reads back, `TurnContextItem`, has no field for them
  (`codex-rs/protocol/src/protocol.rs:3535`), so a resume cannot restore
  them and does not notice that they changed (`session/mod.rs:5132`).
- What survives at first is the conversation itself: resume rebuilds the
  model-visible history from the rollout (`session/mod.rs:1693`–`1764`),
  including the developer message with the original role, and restores the
  context baseline (`session/rollout_reconstruction.rs:172`–`181`), so no
  fresh initial context is sent.
- The first compaction after the resume keeps only the user messages and
  the summary (`codex-rs/core/src/compact.rs:587`–`612`) and rebuilds the
  initial context from the running session's developer instructions
  (`compact.rs:91`, `session/mod.rs:4725`–`4731`). A supervisor resumed
  with only its short flags would lose its role there.

So for automatic restores only, a Codex supervisor's resume command carries
its short flags (`--sandbox workspace-write --ask-for-approval on-request
-c sandbox_workspace_write.network_access=true --add-dir <state dir>`,
tested) and a short developer instruction that tells it to read its
`ROLE.md` before anything else. Until the first compaction it still has
its full role from the conversation; after it, the short instruction is
what Codex re-injects. A normal launch keeps the full inline role. The
instruction must stay within Herdr's rules: no apostrophe, no newline, and
the whole command under 8 KiB.

### Sessions (`S`)

**The Manual recovery setup step is retired once automatic restarts
ship**: it turns Herdr's auto-resume off, which would turn the automatic
restarts off too. `S` keeps Save and the reviewed Restore, for restoring
from a saved snapshot by hand.

### An agent that exits while Herdr runs

Herdr resumes only after a server start: an agent that exits or is
killed while Herdr keeps running (as on 3 November, when a test's watchdog
killed every agent) leaves a shell in its pane. For that, a **Resume**
action, a button and a key, relaunches the agent in the same pane with its
session and full options, the same command Corgi reports to Herdr. It is
shown **only on a pane where Corgi saw an agent session running before**,
never on an arbitrary shell. Corgi does not relaunch on its own: cleanup
after such an incident goes through these resumes and the separate work on
orphaned sessions.

### Rejected

- **Option 2**, Corgi exiting and relaunching agents Herdr resumed without
  their options: no longer needed, since option 1 gives Herdr the right
  command, and stopping an agent seconds after a restart risks losing what
  the user types. Its check survives only as a warning for agents launched
  before the change.
- **Option 3**, a one-click restore after each restart with automatic
  saves: works on Herdr 0.9.0 but still needs a press after every restart,
  and needs auto-resume off.
- **Reporting under Herdr's `herdr:claude` or `herdr:codex` source**:
  Herdr ignores the command there (tested).

## What was built

`crates/corgi-core/src/restart.rs`, its tests in `restart/tests.rs`, the
argv builder `recovery::native::restart_argv`, and "Resume commands" in
ARCHITECTURE.md, which describes the design as code:

- Every launch is recorded by pane in `markers/<socket>.launches.json`, and
  the pass holding the wake lock reports each agent's command, keeps it on
  the live session, and releases it when the pane holds another agent.
  Supervisors and recovery-profiled workers launched before the record are
  taken in.
- Codex supervisors resume with their flags and a short instruction to read
  `ROLE.md`; a normal launch keeps the inline role.
- Resume: a row with `EXITED` for an agent that dropped to its shell, `R`
  in the dashboard, a Resume button and `⇧R` (rebindable) in the window,
  `resume` in `corgi serve`.
- `S` lost Manual recovery setup; Restore no longer requires it.
- The warning: `⚠ no role` or `⚠ no launch options` on the row, and the
  status line, for an agent whose command line lacks what Corgi launched it
  with.

**Verified on a private Herdr 0.9.5** (release binary, SHA-256 matching
GitHub's digest), with fake `claude` and `codex` first on the panes' `PATH`,
started with `corgi supervisor` and `corgi spawn` **[observed]**:

| Agent | Launched with | After `herdr session stop` and a start |
| --- | --- | --- |
| Claude supervisor | `--model opus --effort high` | `claude --resume <id> --model opus --effort high --append-system-prompt-file …/ROLE.md --add-dir …/supervisors/proj` |
| Claude worker, after `/clear` | `--model sonnet --effort low --verbose` | `claude --resume <new id> --model sonnet --effort low --verbose`: the session after the clear |
| Codex supervisor | `--model gpt-6-luna` | `codex resume <id> --cd …/cdx --model gpt-6-luna --sandbox workspace-write --ask-for-approval on-request -c sandbox_workspace_write.network_access=true --add-dir …/supervisors/cdx -c developer_instructions="You are the Corgi supervisor of this project, resumed after a restart. Before anything else, read your role instructions in …/ROLE.md …"` |
| Codex worker, exited and resumed with Resume | `--model gpt-6-sol --effort high -- --search` | `codex resume <id> --cd <worktree> --model gpt-6-sol -c model_reasoning_effort=high --search` |

- A Codex worker that ran `/quit` showed as an `EXITED` row (two other
  shells, where Corgi saw no session, did not); `resume` over `corgi serve`
  started `codex resume <id> --cd … --model gpt-6-sol -c
  model_reasoning_effort=high --search` in its pane, and the window's `⇧R`
  did the same for an exited Claude worker.
- A Claude supervisor that exited and was started again by hand as `claude
  --resume <id>` showed `⚠ no role` and "Running without its supervisor role
  and memory access: exit it, then Resume".
- The startup hook put the supervisors' marks back after each start, and
  the dashboard rows listed them as supervisors again.

Found on the way **[observed]**, each now handled:

- `agent.start`'s reply names no harness (`agent` is absent), so the
  launch's own harness is recorded.
- Herdr forgets a reported command with the agent it was for: an agent
  resumed after an exit came back from the next restart as Herdr's own
  `codex resume <id>` until each new process had its command reported again.
- For about 300 ms after an agent exits, the snapshot still lists it with
  `agent: null` and no session; that is no other agent.
- A fake reporting a new session for `/clear` must pass
  `--session-start-source` (Herdr's integration does); without it Herdr
  keeps the old session.

**Herdr 0.9.0** (the installed build, run only as a private server):
`pane.report_agent_session` from Corgi's source with `resume_argv` is
accepted; the session ref stays `herdr:claude`'s, the pane's state still
follows the screen, and a restart runs Herdr's own `claude --resume <id>`
**[observed]**. So the report is harmless before the upgrade, and the
warning then shows on every resumed supervisor.

### Setup steps, no code

Upgrade Herdr to 0.9.5 (`brew upgrade herdr`, then restart or hand off the
server when convenient); install Herdr's Codex integration (`herdr
integration install codex`), without which Codex conversations cannot come
back at all; and look at the Homebrew launchd service, which retries every
few seconds while a server runs and fills `/opt/homebrew/var/log/herdr.log`.
