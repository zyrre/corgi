# Corgi architecture

Corgi is a tool for the [Herdr](https://herdr.dev) app. It lets one
long-lived agent per project, the **supervisor**, dispatch worker agents
and follow them, and it shows the result in the app's sidebar. It has three
parts:

- the `corgi` command-line tools (Rust, one binary), which the supervisor,
  the mods and the installer run;
- `corgi daemon`, the same binary as a service of the user's session, which
  wakes supervisors when their workers stop, types their inboxes in, and
  hands them over to fresh sessions;
- the mods in `herdr-app/mods/` (JavaScript, run by the app): the Projects
  list of the sidebar and its brand row.

Everything runs on **herdr-core** (`hc`), the app's backend: agents are
programs in hc terminals, and Corgi finds its own among them by the labels
it gave them. Corgi has no other backend and draws nothing itself.

## Runtime shape

```text
Herdr app window (herdr-app)
  └─ mods (herdr-app/mods/, JS)
       ├─ projects ──── the sidebar's list: runs corgi status/usage/projects/open/
       │                merge/close/pause/resume --json, here and on other machines
       └─ corgi-brand ─ the sidebar's top row

herdr-core (hc), one core per session
  ├─ <project>-supervisor ── a kept shell terminal with the supervisor typed into it
  └─ workers ──────────────── one terminal each, in a worktree of its own
       labels on each: corgi.project, corgi.role, corgi.request, corgi.task,
       corgi.harness, corgi.session, corgi.resume, corgi.pid, corgi.tag,
       corgi.tag.at, corgi.handover, corgi.baseline, herdr.space

corgi (target/release/corgi, linked as ~/.local/bin/corgi)
  ├─ what a supervisor runs: spawn, fleet, digest, report, inbox, tag, role
  ├─ what a person runs: supervisor, notify, daemon install|status|uninstall,
  │                      app-layout
  ├─ what the mods run: status, usage, projects, open, merge, close, pause, resume
  └─ corgi daemon ── one per core (a LaunchAgent or a systemd user unit):
       follows the core's events on its socket; on each, and on a timer, one pass:
       resume lost terminals → note stops in inboxes → move handovers on →
       type each inbox into its resting supervisor (hc send, never forced)

Corgi state ($XDG_STATE_HOME/corgi, else ~/.local/state/corgi)
  ├─ projects ─────────── one absolute path per line, newest first: the projects
  │                        Corgi started a supervisor in (CORGI_PROJECTS_FILE)
  ├─ created-projects ─── the projects Corgi created, whose folder trust it answers
  ├─ supervisors/<project>/ ─ a supervisor's memory: ROLE.md and launch.json (Corgi
  │                        writes them at launch), decisions.md, ledger.jsonl,
  │                        ledger-archive.jsonl, briefs/, handover.md, handovers/,
  │                        notes/, questions/Q<n>.md, backlog.md (the supervisor
  │                        writes them), and inbox.jsonl, inbox.lock, inbox-reports/,
  │                        reports/ (only Corgi writes them, src/inbox.rs)
  ├─ wake/<socket>.lock ── held by the daemon of that core for as long as it runs
  ├─ wake/<socket>.seen.json ─ what the last pass saw of each agent it follows
  ├─ wake/<socket>.handovers.json ─ where each supervisor's handover stands
  ├─ daemon/<socket>.lock ─ the daemon's pid and build; daemon/daemon.log its log
  ├─ pause/<socket>.json ─ the agents corgi pause stopped, until corgi resume
  ├─ hc/spawn-<hash>.lock ─ held by a corgi spawn --request-id of one project
  ├─ usage/<provider>.json, .lock ─ the plan-usage reading every process shares
  ├─ opencode-plugin ───── the answer to the installer's OpenCode question
  └─ last-harness.json ── read, never written (see "Open ends")

Agent session files (read, never written)
  ├─ {$CLAUDE_CONFIG_DIR,~/.claude}/projects/<slug>/<session>.jsonl
  └─ ~/.codex/sessions/<y>/<m>/<d>/rollout-*.jsonl
```

`<socket>` is the path of the core's socket spelled with letters, digits and
underscores (`paths::socket_state_file`), so each core has its own records.

## The code

One crate, one binary: `src/main.rs` declares every module and dispatches
the command. It used to be a workspace (a core library shared by a terminal
dashboard, a desktop window's server and a bar widget); with the command
line as the only frontend the split bought nothing, and a binary-only crate
has one more advantage: nothing is exported, so rustc's dead-code lint sees
every item. Code no command reaches fails `cargo clippy -- -D warnings`.

| Module | What it is |
| --- | --- |
| `commands/` | One file per group of commands: argument parsing, the command, what it prints. `launch.rs` (spawn, supervisor), `fleet.rs` (fleet, tag, report), `memory.rs` (digest, inbox, notify, role), `status.rs` (status, merge, close), `open.rs` (projects, open), `pause.rs`, `daemon.rs`, `usage.rs`, `app_layout.rs`. `mod.rs` has what they share: a project's root, the inbox footer. |
| `hc/` | herdr-core. `mod.rs` is the client (`HcClient` shells out to `hc … --json`; `HcError` keeps hc's `kind`), the labels, and where the core's socket is. `agents.rs` turns labelled terminals into `Agent`s. `launch.rs` starts supervisors and workers. `close.rs` closes a worker. `pause.rs` is Pause and Resume. `daemon.rs` is the daemon's loop, `events.rs` the core's event stream, `observe.rs` a watch-only look at a terminal's screen. |
| `waker.rs`, `waker/hc.rs` | `SupervisorWaker`: what one pass does (`observe` stops into inboxes, `hand_over`, `deliver`), its records, and the pass itself over a core (`tick`, the `Core` trait and `HcCore`). |
| `supervisor.rs` | What makes a session a supervisor: the role, its state directory, the first prompt, every `[Corgi]` line, and the pure state machines behind waking (`Transitions`) and handover (`Handover`). |
| `inbox.rs`, `inbox_privacy.rs` | A supervisor's inbox file and the permissions of what is in it. |
| `digest.rs`, `digest/` | `corgi digest`: the bounded summary, `--search`, and the lookups (`--show`, `--ledger`, `--brief`, `--decision`). |
| `merge.rs`, `tag.rs`, `git.rs` | The Git work of `corgi merge` and the lines it leaves the supervisor; the checks of `corgi tag`; the plain Git commands. |
| `launch.rs` | What a launch is made of: `LaunchPlan`, `Checkout`, `Role`, the agent CLI's arguments, the supervisor's plan. |
| `harness.rs` | The one place that knows a harness (Claude Code, Codex, OpenCode, …): its CLI, flags, supervisor arguments, exit command, session file format, the default harness. |
| `session/` | Reads each agent's session file: model, effort, context, prompt cache, task, and the newest reply (its report). `locate.rs` finds the file, `tail.rs` reads it incrementally, `claude.rs` and `codex.rs` parse it, `text.rs` holds what they share. |
| `model.rs`, `rows.rs` | `Agent` and `AgentInfo`; an agent's task summary. |
| `projects.rs`, `choices.rs` | The projects Corgi knows and remembers; the filtered list behind `corgi projects`. |
| `usage.rs`, `usage_cache.rs` | Plan usage per provider, and the cache every process shares. |
| `app_layout.rs`, `herdr_app.rs` | The app's layout with Corgi's sidebar; the `herdr-app` it asks. |
| `draft.rs`, `activity.rs` | Whether an input box holds a draft; redaction, and the folder-trust question. |
| `backlog.rs`, `job.rs`, `paths.rs`, `progress.rs`, `time.rs`, `version.rs` | A supervisor's `backlog.md`; a background job; the state directory; progress lines; time without a date crate; `corgi --version`. |
| `brand_tile.rs`, `test_support.rs` | Tests only: the brand row's logo generator; scratch directories and test repositories. |

`build.rs` tells the binary what it was built from: the commit, whether the
files that go into the binary differed from it, and the checkout. `corgi
--version` prints `corgi 0.2.3 (5b50895)` (`--json` for all of it), `corgi
status --json` carries it as `corgi`, with `newer` when that checkout has
commits the build lacks, and the daemon writes its own into its lock, so
the Projects list can say which Corgi runs, that a newer one was merged,
and that the daemon still runs another.

## herdr-core

`hc` is asked through its CLI, one short process per call. Three rules
follow from how hc behaves (`docs/herdr-app-port.md` has the tests behind
them):

- **Any `hc` call starts a stopped core**, with the caller's environment. So
  a command that only reads (`status`, `merge`, `close`, the daemon while it
  waits) first checks that the core's socket accepts a connection
  (`hc::core_running`), and asks nothing when it does not. `corgi status`
  then still lists the projects from their state directories, with
  `"hc": {"running": false}`.
- **hc wraps what one agent's terminal sends to another program** in
  `<hc from="…">…</hc>`, which a shell cannot run. `HcClient::send` removes
  `HC_TERMINAL_ID` from its `hc` process for that reason.
- **A core started from inside an agent session hands that session's
  variables to every terminal.** Every agent therefore starts through
  `hc::launch::agent_script` (`/bin/sh -c`), which unsets the variables
  `SESSION_VARIABLES` lists (Claude Code's own marks, every `HERDR_*`)
  before it runs the agent: a Claude Code that inherits another session's
  marker saves no transcript, which Corgi reads. A user's own `CLAUDE_CODE_*`
  settings pass through.

### Labels

Corgi's agents are the live terminals with `corgi.role` (`supervisor` or
`worker`) and `corgi.project` (the project's primary checkout, without
symlinks). The other labels: `corgi.request` (the `corgi spawn
--request-id`), `corgi.task` (a few words), `corgi.harness`,
`corgi.session` (the harness's session id), `corgi.resume` (the command
line that resumes the session), `corgi.pid` (the pid of the program Corgi
started the agent from), `corgi.tag=merge` with `corgi.tag.at` (the state
and turn it was tagged at), `corgi.handover` and `corgi.baseline` (a
supervisor session's handover request and context baseline), and
`herdr.space` (the app's space).

`hc::agents` reads them into an `Agent`: the terminal's address, its state
(`done` and `idle` both rest; `error` reads as blocked; a shell in front of
a kept supervisor terminal is unknown, and the agent `exited`), hc's `turn`
as `state_change_seq`, the project, whether it works in a linked worktree,
and what its session file says (model, effort, context, prompt cache, task).

### Spaces

The app makes a space for an id it does not know and names it after where
its first terminal is; Corgi never names one. A supervisor Corgi starts in a
terminal of its own gets `corgi-<project>-<hash>` (`project_space`); one
started in a shell the person had open stays in that shell's space. A worker
in a worktree gets a space of its own, which the app's sidebar nests under
the project's by Git alone (a linked worktree goes under the space on its
main checkout); a worker in the primary checkout or another directory is a
tab of the project's space (`worker_space`).

### Starting agents

`corgi supervisor [PROJECT]` starts a project's supervisor: a `--keep` shell
terminal named `<project>-supervisor` in the primary checkout, with the
agent's command line typed into it, so that a handover can exit the agent
and start its successor in the same terminal while its workers keep
running. On a project whose supervisor terminal has its shell in front (its
agent exited), the new session starts there. `--terminal T` starts it in an
existing terminal instead (`adopt_terminal`): a live terminal with a shell
waiting at its prompt and no role of Corgi's, such as a tab the person has
open in the app. `corgi open`, which the Projects list's `+` runs, names a
running supervisor's terminal or starts one, in a shell the mod offers when
that shell waits in the project's primary checkout, on the harness, model
and effort the supervisor last ran with (`launch.json`). The home directory
is never a project and gets no supervisor.

The supervisor's role goes into its instructions, not its conversation, so
compaction does not lose it: Claude Code appends the role file to its
system prompt; Codex takes it as `-c developer_instructions`, written to a
file in the state directory and named in the typed line as
`@corgi-arg-file:<path>`, which the agent script replaces with the file's
text, so 25 KB are never typed into a shell. Claude Code is given its
session id (`--session-id`), so its terminal is labelled before it starts.

`corgi spawn` starts a worker in a terminal of its own (`--no-parent
--no-notify --keep`), by default in a new worktree made with `git worktree
add` under `~/.herdr/worktrees/<repo>/worktree-<name>` on branch
`worktree/<name>`. The task comes from stdin or `--task-file`, never from
arguments; the result is one JSON object on stdout. Spawn refuses a
worktree outside Git and any worker in Corgi's state directory, before
reading the task. `--request-id ID` makes a retry safe: under a lock file
for the project, a running worker with that id is printed with
`"existing": true` and nothing starts. Without `--harness` a worker starts
on its calling supervisor's harness, else the default
(`harness::default_harness`: the one `last-harness.json` names, else
`CORGI_DEFAULT_AGENT`, else the first installed of `Harness::KNOWN`).

Claude Code's folder-trust question is the one question Corgi answers, and
only for a project Corgi created (`created-projects`): the launch presses
Yes once exactly that dialog, about the agent's own folder, is on screen. Codex takes the same answer on its command line.

## The supervisor

The role is `supervisor/ROLE.md`, embedded in the binary and written into
the project's state directory at launch with the path of the Corgi binary
(`CORGI_BIN` chooses another) and the `hc` commands for steering workers
(`hc send`, `hc read`, `hc keys`). It is loaded into every turn, so what a
supervisor needs only at one moment is not in it: how to hand over is
`supervisor/HANDOVER.md`, which `corgi role handover` prints, and what to do
after a merge, a conflict or a failed push is in the `[Corgi]` line that
reports it. Tests in `src/supervisor.rs` pin the role's wording and cap its
size.

A supervisor's memory is plain files in `supervisors/<project>/`, outside
the repository. `supervisor::prepare`, run before each launch, writes the
role, moves a state directory left under an older name (`corgis/`,
`handler/`, `steward/`) with a link at the old path, and compacts the
ledger: lines a later line of the same id supersedes move to
`ledger-archive.jsonl`.

### Digest

A supervisor starts its session with `corgi digest <project>` instead of
reading its files raw, since `decisions.md` and `ledger.jsonl` only grow.
The digest reads only, and prints, bounded: the base branch and HEAD; the
handover note, else the open threads of the newest archived one; every
unfinished ledger id joined with the running agents (by name, else by
request id); the last finished ids; the newest decisions in full (about
8 KB, at least 3) and the titles of the rest; and warnings. `--search`
looks in all of the memory (decisions, the ledger and its archive, briefs,
kept reports, notes, handover notes), best hits first with near-duplicates
folded; `--show`, `--ledger`, `--brief` and `--decision` print one item
whole. The lookups read the state directory alone and need no running
core; the summary needs one, for the agents.

### Waking

The daemon wakes supervisors. `supervisor::Transitions` reduces each
agent's state from pass to pass to wakes: nothing until the agent has been
seen working; after that, each time it rests (done or idle, which count as
one) after working, and each change between blocked and resting. Every
agent of a project with a running supervisor counts, whoever started it,
and one the supervisor spawned counts also while the project has none, so
the next supervisor hears of it. Scratch agents (in the home directory)
wake nobody.

What the last pass saw of each agent is kept in `wake/<socket>.seen.json`,
and every pass starts from that record rather than from what it sees, so
whatever changed while no daemon ran is news: a worker that was working at
the last save and is done now wakes its supervisor once. A wake's id names
the agent, its terminal, the run of state changes it is in, its
`state_change_seq` and the state, so the same stop is never added twice.

### The inbox

Every wake, merge line and note goes through the supervisor's inbox
(`inbox.jsonl`), so none is lost or sent twice when the daemon stops or
restarts. It is append-only JSON lines, written only through `src/inbox.rs`
under `inbox.lock`: items, and records naming the ids delivered. Readers
take no lock and skip a last line still being written. Past 256 KB a write
rewrites the file with every undelivered item and the newest 100 delivered
ones. Projects of the same directory name share a state directory, so each
item names its project root, and a supervisor gets only its own project's.

When an agent rests, its report goes into the item: the newest reply in its
transcript, when it comes after the turn's prompt and its last tool call,
else its screen. A transcript that does not yet show how the turn ended is
read again over two seconds, then the wake goes without a report rather
than with an older turn's. For a worker the supervisor spawned, a report
from its transcript is also kept for good in `reports/`, where `corgi
digest --search` finds it. State directories are 0700 and their inbox and
report files 0600 (`inbox_privacy.rs`); links and foreign-owned files fail
closed.

Delivery types a project's undelivered items into its supervisor as one
prompt, the newest item of each key only, once the supervisor rests with
its agent in front and its input box holds no draft, then records them
delivered. The look at the box is a watch-only viewer's first paint
(`hc attach --observe` in a pseudo-terminal of Corgi's, `hc/observe.rs`),
read with its styling: dim text is the harness's own suggestion, any other
text is a draft (`draft.rs`). A box Corgi cannot find holds nothing back.
`hc send` is never forced, so a blocked supervisor is never answered by a
wake; a refused or uncertain send leaves the items for a later pass, so a
wake may arrive twice but is never lost. A line for one of the supervisor's
own workers with a short report quotes it (`> `), so none passes for a
`[Corgi]` line.

`corgi inbox` prints what waits; `--take` records it delivered, for a
supervisor that acts on it itself. `corgi notify` adds a line. `corgi
fleet`, `digest` and `report` end with a line on stderr when the supervisor
has missed items.

### Handover

`supervisor::Handover` hands a supervisor over to a fresh session, once per
harness session, so compaction does not lose what only its conversation
holds, and only when its prompt cache is about to go cold: a supervisor
that is idle or done, has rested for its harness's idle time (50 minutes on
Claude Code, 25 on Codex: short of the cache life, so the request finds it
warm), and is at 50% context or, on Claude Code, at 200k tokens, gets one
`[Corgi]` line asking it to run `corgi role handover`, write `handover.md`
and end its turn. A second trigger asks a supervisor below the threshold
that has rested that long, with no worker busy and its context at least
twice its baseline. A harness with no known cache life is asked at its
first rest at the threshold.

When the turn ends with a note written since the request, and the input box
holds no draft, the daemon types the harness's exit command, waits for the
kept shell to come to the front, and starts the successor there with a
prompt that has it read the note, archive it, and tell the user it took
over. A turn that ends without a note leaves the supervisor in place, asked
again only after a later turn. The request and the baseline are labels of
the terminal, and where each handover stands is kept in
`wake/<socket>.handovers.json`, so a daemon that restarts goes on with the
same handover. Wakes for the project wait from the request until the
successor's first turn is over.

## The daemon

`hc/daemon.rs` around `waker/hc.rs`, with the command in
`commands/daemon.rs`.

- **One per core, never starting one.** It holds `daemon/<socket>.lock` and
  the core's wake lock for its whole life, waits for the core's socket to
  accept a connection, and follows the events on the socket itself
  (`events.subscribe`).
- **Trigger, then reconcile.** An event of one of Corgi's terminals starts
  a pass once the burst settles (300 ms); so do its start, every reconnect,
  and a timer: three seconds while an inbox waits for a resting supervisor
  or a handover runs, else a minute, which times a resting supervisor's
  prompt cache. The event decides nothing; a pass reads Corgi's terminals
  afresh.
- **Resume.** A kept terminal whose program the core replaced with a shell
  (a core stop, a reboot) has another pid than its `corgi.pid`: once its
  shell has drawn a prompt, the daemon types its `corgi.resume` line there,
  once. A supervisor whose agent exited into its own shell waits for `corgi
  supervisor`.
- **Spaces are not the daemon's**: it never names, makes or moves a space
  of the app, and never talks to the app's window.

`corgi daemon install` writes a LaunchAgent
(`io.github.zyrre.corgi.daemon`) or a systemd user unit, kept alive, with
the login shell's `PATH` and the `HC_*`, `XDG_*` and `CORGI_*` settings of
the environment it runs in, logging to `daemon/daemon.log`, and (re)starts
it; `uninstall` removes it; `status` says how it is.

## The Herdr app

`herdr-app/RECIPE.md` describes the mods and the layout for people and
agents; this is what the binary does for them.

- **Status.** `corgi status --json` is what the lists read: every project
  with a supervisor state directory or a running agent, its agents, the
  open questions and counts from its supervisor's ledger, the inbox items
  waiting, `hc`, `daemon`, `corgi` (the build) and `paused`. It changes
  nothing and may run on the user's real state.
- **Merge.** `corgi merge NAME --project DIR --json` merges a worker's
  clean worktree branch into the branch checked out in the primary
  checkout and pushes it. The base branch's upstream is fetched first:
  commits it has that the base lacks refuse the merge as `behind` (`--pull`
  pulls them in), a failed fetch as `fetch-failed` (`--skip-upstream`
  merges anyway). The merged worker's tag is cleared and the supervisor
  told in its inbox, of a failed push too. A conflict is aborted and handed
  to the running supervisor, which has the worker merge the base into its
  own branch; without one it is left to resolve. `--ask-supervisor ERROR`
  merges nothing: it asks the supervisor to fix what failed. An error
  message reaches two readers, the card under the worker's row and the
  supervisor, so it says what went wrong and what to do without naming a
  key or a flag. The agent never merges, and Corgi never changes the
  user's checked-out branch.
- **Close.** `corgi close NAME --project DIR --json` ends a worker's
  terminal and removes it, then its worktree (`git worktree remove`; the
  branch stays). A dirty worktree is refused once; `--force` discards it.
- **Tag.** `corgi tag NAME merge` (a supervisor runs it) marks a resting
  worker with commits ahead of the base, and no open question, ready to
  merge; the list shows MERGE in place of its state. The tag stops applying
  once the agent works again.
- **Pause and Resume.** `corgi pause` is for leaving the network: every
  agent of Corgi's that is working at that moment, and that Corgi can bring
  back (its terminal holds a resume line, which Claude Code sessions have),
  gets Esc, then its harness's exit, each marked first in
  `pause/<socket>.json`. Nothing is killed; one that kept working or held
  text in its box loses its mark and is named. While any agent is paused
  the daemon resumes no paused terminal, follows the paused agents' stops
  without a wake, types nothing into any supervisor and moves no handover
  on. `corgi resume` brings back exactly the paused ones, each on its own
  conversation.
- **Projects and Open.** `corgi projects --json` lists the projects to
  offer: those with agents, the folders the app's spaces are in, Git
  repositories beside those, then the remembered ones; with `--query` the
  matches, the directories that complete a typed path, and where a new name
  would be created.
- **Usage.** `corgi usage --json` gives each plan's 5-hour and weekly use.
  Codex is asked through `codex app-server`; Claude Code's usage comes from
  Anthropic's usage endpoint with Claude Code's stored login
  (`.credentials.json`, or the macOS Keychain), which is unofficial and
  read-only. One reading per provider is shared by every process on the
  machine (`usage/<provider>.json`) and fetched at most once a minute.
- **Layout.** `corgi app-layout` puts the Projects mod where the sidebar's
  `spaces` block was, takes out its `agents` block, and puts the brand row
  mod in its `brand` block, in the layout the window shows, and changes
  nothing else, so other mods can change the same layout. Herdr's shipped
  layouts are read-only (replaced on every start), so a shipped `main` is
  first cloned to `corgi` and the window switched to it; a personal layout
  is edited in place. `remove` puts Spaces and Agents back.

## Installing

`herdr-app/install.sh` is the one installer. Piped from the repository it
clones the checkout and runs that copy; in a checkout it

1. puts the binary at `target/release/corgi` (`scripts/install.sh`): the
   release binary of the checkout's version (`Cargo.toml`), verified against
   the checksum pins committed in `checksums/`, when HEAD is the release
   tag; else `cargo build --release --locked`, so a developer's checkout is
   never downgraded (`CORGI_BUILD=source|release` chooses). Then it links
   `~/.local/bin/corgi` to it unless something else is there;
2. installs the daemon's service again, which restarts it on the new binary;
3. asks once whether to add the OpenCode status plugin
   (`herdr-app/opencode/corgi-hc-status.js`), without which an OpenCode
   worker reports no state to hc and wakes no supervisor;
4. puts each mod into the app's home, checked with `herdr-app check` and
   swapped in whole, only when it differs;
5. runs `corgi app-layout`.

The mods run that checkout's `target/release/corgi` (named in each mod's
`config.json`). `--uninstall` undoes it.

## Configuration

| Variable | Effect |
| --- | --- |
| `CORGI_DEFAULT_AGENT` | The harness a launch that names none starts on, instead of the first of `codex` or `claude` found |
| `CORGI_CODEX_BIN`, `CORGI_CLAUDE_BIN` | Where that CLI is, when it is outside `PATH` and the usual install directories |
| `CORGI_HC_BIN` | The `hc` to run |
| `CORGI_BIN` | The Corgi binary a supervisor's commands call |
| `CORGI_PROJECTS_FILE`, `CORGI_CREATED_PROJECTS_FILE` | Where those two lists are kept |
| `HC_SESSION`, `HC_SOCKET`, `XDG_STATE_HOME`, `XDG_RUNTIME_DIR` | Which core, and where Corgi keeps its state |
| `CORGI_DEBUG_HANDOVER_PERCENT`, `CORGI_DEBUG_HANDOVER_IDLE_SECS` | Lower the handover thresholds for a debug run; not settings |

## Data and privacy

Corgi sends no telemetry. It stores its own bookkeeping and each
supervisor's memory and inbox, which holds the reports of workers that
stopped, in its state directory. A task and every line Corgi types go
through `hc`, a first prompt through a file in the state directory, never
in process arguments. The only network use is the plan usage, and Git's
own fetch and push during a merge. The Claude Code token is sent to
`api.anthropic.com` only, passed to `curl` on stdin, and never stored.

## Open ends

Kept as they were, each for a decision of its own:

- `last-harness.json` is read for the default harness but nothing on hc
  writes it, so it names whatever the last launch before herdr-core
  recorded.
- `supervisor/ROLE.md` still says "dashboard", "pane" and "Corgi workspace"
  in places; the role's size is capped by a test and its wording pinned.
