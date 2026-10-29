# Corgi architecture

Corgi does not run its own backend service. Herdr is the control plane and the
terminal multiplexer; each Corgi pane is a short-lived client of the Unix socket
that Herdr injects as `HERDR_SOCKET_PATH`. What has to happen while no Corgi
window is open (putting back marks after a restart, recording new agents'
sessions, waking supervisors, starting handovers) Herdr runs itself, through
the plugin's startup and event hooks: each is one short `corgi hook` process
(see "Herdr's hooks" below).

## Runtime shape

```text
Herdr session
  ├─ coding-agent panes
  └─ Corgi plugin pane
       ├─ session.snapshot ── agent identity, state, project/task metadata
       ├─ agent.read ──────── visible terminal activity, and a supervisor's input box
       ├─ agent.prompt ────── prompt an existing agent, and wake a project's supervisor
       ├─ agent.focus ─────── focus an existing agent pane
       ├─ workspace.close ─── close a plain or unused Corgi project workspace
       ├─ tab.create/close ── open or close a shared-checkout agent's tab
       ├─ pane.current, pane.send_input ── open an agent's checkout in the editor
       ├─ worktree.remove ─── delete a worktree checkout and close its workspace
       ├─ *.report_metadata ─ mark Corgi's project workspaces and the supervisor's pane,
       │                      and put those marks back after a Herdr restart
       ├─ agent.send_keys ─── answer the folder-trust question of a project Corgi created
       ├─ git merge + push ── merge and push a reviewed worktree branch in the primary checkout
       └─ worktree.list → workspace.create/reuse → worktree.create(workspace_id)
                        → agent.start → agent.prompt
                              create an agentless, Corgi-owned project parent,
                              then check out, create, and prompt a worker
                              (workspace.create when the directory is not a Git repo)

Corgi state ($XDG_STATE_HOME/corgi, else ~/.local/state/corgi)
  ├─ projects ─────────── one absolute path per line, newest first; read at start,
  │                        written when a project is used or seen open; never
  │                        a path inside this directory, and a directory gone is
  │                        dropped when it is written
  ├─ created-projects ─── the projects Corgi created, whose folder trust it answers
  ├─ expanded-projects ── the projects whose dashboard card is expanded, one
  │                        heading per line; missing or unreadable is all collapsed
  ├─ selection/<socket>.<pid> ─ the pane ID of the agent selected in the
  │                        dashboard running as process <pid>, empty for none, for
  │                        `corgi edit`; replaced through a rename, removed on quit,
  │                        and by the next dashboard to start once <pid> is gone
  ├─ wake/<socket>.lock ─ held for one pass at waking supervisors at a time, by a
  │                        hook or a dashboard (an older Corgi's dashboard held it
  │                        for as long as it ran)
  ├─ wake/<socket>.seen.json ─ what the last pass saw of each agent it
  │                        follows (pane, run of state changes, transitions), for the next one;
  │                        replaced whole through a temporary file and a rename
  ├─ wake/<socket>.handovers.json ─ where each supervisor's handover stands, by
  │                        project root, for the next pass; written the same way
  ├─ wake/<socket>.dirty ─ set by a hook that found the wake lock taken, so the pass
  │                        holding it goes round once more; cleared as a pass begins
  ├─ wake/<socket>.open ── a share of it held by every open dashboard, so the
  │                        command-line tools can tell one is open
  ├─ markers/<socket>.json ─ every metadata token Corgi set in that Herdr session,
  │                        by pane and workspace, with the agent or checkout it
  │                        was set for; replaced whole through a temporary file
  │                        and a rename, under markers/<socket>.lock
  ├─ markers/<socket>.spawn-<hash>.lock ─ held by the `corgi spawn --request-id`
  │                        of one project, from its duplicate check until its
  │                        worker has started
  │   A dashboard that starts removes the wake/ and markers/ files, but not
  │   the spawn locks, of a Herdr session whose socket is gone, once they
  │   have gone untouched for a week (the core's `sweep.rs`).
  ├─ usage/<provider>.json ─ the last plan-usage reading every process shares,
  │                        when it was fetched, and the last fetch's error;
  │                        replaced whole through a temporary file and a rename
  ├─ usage/<provider>.lock ─ held by the one process fetching a new reading
  ├─ refresh.log ──────── the interactive dashboard's slow refreshes, errors and
  │                        changes in the number of agents (every refresh with
  │                        CORGI_REFRESH_LOG=all); refresh.log.old past 256 KB
  └─ supervisors/<project>/ ─ the supervisor's files, written by Corgi at launch
                          and by the supervisor: ROLE.md, launch.json,
                          decisions.md, ledger.jsonl, briefs/, handover.md
                          while one is under way, handovers/, notes/; at
                          launch Corgi moves superseded ledger lines to
                          ledger-archive.jsonl;
                          and by Corgi only (`inbox.rs`, under inbox.lock):
                          inbox.jsonl, inbox-reports/ for long reports, and
                          reports/, spawned workers' reports kept for good

Agent session files (per refresh, only the records appended since are parsed)
  ├─ {$CLAUDE_CONFIG_DIR,~/.claude}/projects/<slug>/<session>.jsonl ── model, token usage, cache lifetime
  └─ ~/.codex/sessions/<y>/<m>/<d>/rollout-*.jsonl ── model, usage, window, cache-hit estimate

Plan usage (once a minute per machine, only for CLIs Corgi can find)
  ├─ codex app-server ────── account/rateLimits/read over JSON-RPC
  └─ curl api.anthropic.com/api/oauth/usage ── with Claude Code's stored token

Optional desktop GUI: corgi-gui, a GPUI window (crates/corgi-gui)
  └─ corgi serve ──── its child, local or `ssh HOST corgi serve`: JSON lines
                      on stdio, a hello, a snapshot of the rows, cards, usage
                      and palette as data, then deltas; commands for the
                      dashboard's actions (docs/serve-protocol.md)

Optional Omarchy widget
  ├─ corgi --bar-stream <socket> ── newline-delimited dashboard rows and usage
  ├─ corgi --bar-transcript/--bar-new-*/--bar-action ── expand, create, merge, close
  ├─ herdr agent focus ─────── reveal a selected agent
  └─ herdr plugin pane open/focus ── launch or reveal the full dashboard

Herdr's plugin hooks (herdr-plugin.toml), one short process each
  └─ corgi hook ───── at the server's start or live handoff, and on pane.agent_detected,
                      pane.agent_status_changed, pane.exited, pane.closed and
                      workspace.closed: one pass of the refresh under the wake lock,
                      the same marks, wakes, deliveries and handovers a dashboard does

Headless launch and the supervisor
  ├─ corgi spawn ──── the new-agent form's launch sequence, task on stdin, result as JSON
  ├─ corgi supervisor the same launch into the project's root tab, as its supervisor
  ├─ corgi fleet ──── a project's agents as tab-separated rows
  ├─ corgi digest ─── the supervisor's memory, bounded, joined with the fleet
  ├─ corgi report ─── an agent's report: kept in an inbox, else its newest message
  ├─ corgi notify ─── add an item to a supervisor's inbox, text on stdin
  ├─ corgi inbox ──── a supervisor's undelivered inbox items, bounded
  ├─ corgi tag ────── tag an agent ready to merge (pane token), or clear it
  ├─ corgi role ───── the part of the supervisor's role it reads on demand (handover)
  └─ corgi edit ───── the plugin's `edit` action: open the focused or selected
                       agent's checkout in the user's editor

Setup (once, from a shell)
  ├─ corgi setup ──── append a key binding for the open action to Herdr's config.toml
  ├─ herdr config check / server reload-config ── validate the key, load the binding
  └─ herdr plugin action invoke open ── open or focus the dashboard
```

The dashboard refreshes from Herdr in the background, 850 ms after the last
refresh finished or at once when an action asks for one, never while a
launch runs and never two at a time, and redraws about every 80 ms, so
spinners move and keys answer however long a refresh takes (see
`refresh.rs` below); while a popup opens, closes or plays an outcome it draws
at about 60 frames a second, and drops back to 80 ms once nothing moves (see
"Popup motion" below). Plan usage is read once a minute per machine rather than
per process (see `crates/corgi-core/src/usage_cache.rs` below) and Codex
thread names every few seconds, each on a background thread. API calls open a
fresh newline-delimited JSON connection, have five-second I/O deadlines (ten
for a first prompt, which waits for the agent to react), and cap responses at
4 MiB. There is no daemon, database, HTTP listener, or state that needs to be
synchronized with Herdr: the always-on work runs in hooks Herdr starts, each
a process that exits within its time limit.

## Crates

The repository is a Cargo workspace of three crates. The first two are on
`rust-version` 1.89:

- `crates/corgi-core`, the library every frontend shares: Herdr's client,
  the agents' sessions, the rows and their order, the refresh pipeline, the
  supervisor, its inbox, memory and waker, the marks Corgi keeps in Herdr,
  the flows that change Herdr (launch, merge, close, tag, recovery), the
  model catalogs, and the color palette. It depends on neither ratatui nor
  crossterm, and nothing in it reads a key or draws. Its `test-support`
  feature exports the test helpers (scratch directories, a fake Herdr
  socket, the recovery fixture) to the other crates' tests.
- `corgi`, the root package, the `corgi` binary at `target/release/corgi`
  that the plugin, the installer, the release workflow and the Omarchy
  widget run: the ratatui dashboard (`src/app/`, `src/ui/`, `src/motion.rs`,
  `src/theme/`), the command-line tools and the Omarchy endpoints. Where a
  flow is split, the core holds the work and `src/app/` the form, the keys,
  the background job and the panel, re-exporting the core's items under the
  paths the dashboard always used (`app::launch::LaunchPlan`, …).

Both are the workspace's default members, so `cargo build`, `cargo test`
and `cargo clippy` at the root cover both. The third is out of the default
members:

- `crates/corgi-gui`, the desktop window (`corgi-gui`), drawn with GPUI
  (`gpui-pre =0.3.6`, the version Herdr UI's terminal painter pins, so the
  terminals of a later phase can use it). GPUI needs Rust 1.96, so the crate
  has its own `rust-version`; build and test it with `-p corgi-gui`. Only it
  depends on GPUI, and the `corgi` binary never builds it. It shares the
  workspace's `Cargo.lock`: adding GPUI moved no version the other two crates
  use, and they still build on Rust 1.89 with `--locked` (check that after
  changing the GUI's dependencies, as AGENTS.md says).

What a frontend other than the dashboard calls, with no `App`:

- the refresh: `refresh::run` with `refresh::Inputs::new(client)` and a
  `RefreshState` (with `SupervisorWaker::default()` in it to restore marks,
  wake supervisors and deliver their inboxes when it takes the per-socket
  wake lock), giving the sorted rows and workspaces;
- the marks and the waker on their own: `markers::restore_markers`,
  `markers::clear_stale_merge_tags`, `waker::wake_supervisors`, between a
  waker's `begin` and `end` (which take and give up the wake lock);
- a hook: `hook::run` with `hook::Event::from_env()`, or `hook::reconcile`
  with the passes of `hook::HookPasses`;
- the actions: `launch::run_launch` with a `LaunchPlan`
  (`launch::supervisor_plan` for a supervisor), `merge::merge_form`,
  `merge::run_merge`, `merge::clear_merged_tag` and
  `merge::tell_supervisor_about_merge`, `close::close_form` and
  `close::close`, `tag::tag_agent`, `recovery::{capture, plan, restore}`,
  and `HerdrClient`'s own `prompt_agent` and `go_to_agent`;
- the memory: `digest::render` and `digest::memory::digest_lookup`;
- the colors: `palette::resolve()`, and `palette::Theme::rgb_roles` for a
  frontend that paints its own pixels;
- the new-agent form's checks: `launch::new_agent_plan`, which the
  dashboard's form and `corgi serve` both call;
- the wire format of `corgi serve`, `protocol`: serde types a desktop
  frontend deserializes into rather than defining its own.

Every one of them takes a `HerdrClient`, which `HerdrClient::from_socket_path`
makes for any Herdr socket; only `HerdrClient::from_env` reads
`HERDR_SOCKET_PATH`.

## Components

### Reading Herdr and the agents

All of this is in the core, `crates/corgi-core/src/`.

- `herdr.rs` is the typed Herdr socket adapter. All actions are expressed
  in Herdr's public API rather than by driving terminal input directly. A
  refusal from Herdr is a typed `HerdrError`. The refusals a flow reacts to,
  such as `agent_pane_busy` or `not_git_worktree`, are classified by
  `HerdrError` itself (`HerdrError::is_pane_not_ready`,
  `HerdrError::is_not_git_worktree`, …), by its code rather than its text.
- `model.rs` holds Herdr's agents and workspaces as Corgi deserializes
  them, and `DashboardAgent`, one row of the dashboard with everything
  derived for it.
- `harness.rs` is the one place that knows a harness. `Harness` parses
  Herdr's kind string exactly (anything unknown stays `Other` and round-trips
  unchanged; text a user typed is trimmed and lower-cased first) and owns its
  title, the CLI lookup (`CORGI_CODEX_BIN` or `CORGI_CLAUDE_BIN`, else `PATH`,
  else the usual per-user and package-manager directories, because Herdr runs
  plugin panes with a minimal `PATH`), the model, effort, folder-trust and
  supervisor arguments, the offline model lists and model discovery, the exit
  command, the session file Corgi reads, and the form's order. It also reads
  Claude Code's settings from `CLAUDE_CONFIG_DIR`, then `~/.claude`, for
  everything that needs them. The rest of Corgi parses a kind once, where it
  arrives from Herdr, the command line, or the form's text, and passes a
  `Harness` from there on; it is a string again only where it leaves Corgi,
  in `agent.start` and the JSON Corgi writes.
- `activity.rs` reduces a bounded terminal read to display lines. It
  recognizes the command on screen separately from questions, assistant
  messages, and progress; strips terminal chrome; redacts common credentials;
  and caps output length. It is the fallback for what the session file lacks,
  and the only source of the permission prompts a blocked CLI shows on screen.
- `session/` reads each agent's session file, which its CLI already
  writes: Claude Code's transcript under `projects/` in its configuration
  directory, and Codex's rollout under `~/.codex/sessions`. From it come the
  model and context-window usage, the newest thing said in the conversation
  (the user's prompt, the assistant's reply, or its thinking, whichever is
  last), and the newest tool call with its whole argument, where the terminal
  only shows an abbreviation. The same reader also returns the newest turns as
  a list, which is what the expanded single-session view shows; it leaves the
  agent's own reasoning out so the turns the reader came for are not pushed
  off the view. `mod.rs` is `SessionReader` and its per-file cache;
  `locate.rs` finds a pane's file; `tail.rs` reads records back from a file's
  end, from where the last read stopped, and forward from its head;
  `claude.rs` and `codex.rs` are the one parser per harness, which reduces a
  record to the events of `text.rs`, where both views and the display helpers
  they share live.
  Reading is incremental. The first read of a file scans back from its end
  only as far as it must, and for the list only over a fixed tail; every later
  read parses just the records appended since and keeps what older records
  said until a newer one says otherwise, so a refresh costs what the agent
  wrote since the last one rather than a rescan of the file.
  Herdr publishes the session identity of a pane, so the right file is found
  without guessing. A Claude Code pane whose identity Herdr never learned is
  found by the `session` token a status-line bridge reports, which names the
  previous session until the bridge reports again after a `/clear`; failing
  that, by the newest transcript of its working directory, but only while no
  other live Claude Code pane works there, since that file may be the other
  pane's. A Codex pane without Herdr's identity has no file, for the same
  reason (Codex draws no status line). Nothing has to be installed
  into the agent CLIs, and a status-line bridge that reports `model` or `ctx`
  as pane metadata is used only when the file cannot be read. Claude Code's
  transcript records the model family but not its long-context variant, so the
  window comes from the model configured in Claude Code's settings and from
  any session that has already outgrown the default window.
  The same transcript gives the prompt-cache countdown: every record carries a
  timestamp, and every usage record says whether the request wrote its cache
  with the five-minute or the one-hour lifetime. Anthropic re-warms a cache on
  each request, so it lapses one lifetime after the newest record of the main
  conversation; the UI counts down to that moment against the wall clock and,
  past it, shows the context size the next turn re-reads. Codex rollouts do
  not record an exact lifetime, but GPT-5.6 records cached-input and
  cache-write token counts. A nonzero value shows a cache hit or write; Corgi
  adds the documented 30-minute minimum retention and labels the result an
  estimate. Past that minimum, its row says the cache may be cold rather than
  claiming a re-read is certain.
- `usage.rs` reads plan usage per provider and fetches Codex's
  account-aware picker catalog and thread names through a short-lived
  app-server. Claude Code usage comes from its stored OAuth token and
  Anthropic's usage endpoint. All of these run off the UI thread and fail
  soft. Its `Provider` is the view of the two harnesses that have a plan to
  read.
- `usage_cache.rs` shares one plan-usage reading per provider between
  every dashboard and Omarchy bar stream on the machine, so reloads and extra
  dashboards do not multiply requests (Anthropic answers them with HTTP 429).
  Each process checks `usage/<provider>.json` every few seconds, a started
  one at once, and uses it while its last fetch is under a minute old. Once
  it is older, the process that takes `usage/<provider>.lock` without waiting
  re-checks the cache, fetches, and writes it; the others keep what they have
  until a later check. A failed fetch is recorded with its time, beside the
  last good reading, so nobody retries it before the minute is up and the
  card keeps showing the reading. The card's first row shows its age.
- `defaults.rs` reads what each harness's own configuration names, so
  the form can show it in parentheses behind "Harness default": Claude Code's
  `model` and `effortLevel` (including per-model `modelSettings`) from its
  settings files, and Codex's top-level `model` and `model_reasoning_effort`.
  It never guesses a CLI's built-in default; an unread configuration shows
  nothing. Each harness is read once per dashboard run.
- `projects.rs` is where a new agent can be started: the projects with
  agents, the repositories with any primary workspace open in Herdr, the
  directories beside those, and the ones Corgi remembers in a plain text file
  under `$XDG_STATE_HOME/corgi` (`CORGI_PROJECTS_FILE` overrides it), turned
  into rows for the form's Project list along with directory-path completion.
  A name or path that does not exist yet becomes a new-project row. A bare
  name goes under the parent directory most known projects share. The launch
  creates the directory, records it in `created-projects`
  (`CORGI_CREATED_PROJECTS_FILE` overrides it), and starts the project's
  supervisor in the new project workspace's root tab; the directory stays plain
  until the supervisor makes it a repository. A picked project first becomes the
  `cwd` of `worktree.list`, which Herdr resolves to the repository itself.
- `git.rs` runs the few plain Git commands Corgi needs: a checkout's
  branch, whether it is clean, the commits a merge would bring, and the main
  checkout that folder trust keys on.

### The flows: `crates/corgi-core/src/`

The work of every flow that changes Herdr, and of the periodic refresh, is
in the core, as free functions over a `HerdrClient` and the rows, so the
dashboard, the command line and other frontends run the same code.

- `refresh.rs` is the periodic refresh: Herdr's snapshot, the marks put back
  and the stale merge tags cleared, the project workspaces relabelled, each
  agent's row from its session file, its screen and, for an unidentified
  scratch Codex, its pane's processes, the expanded transcript, the
  state-directory warnings, and the supervisors' wakes: `refresh::run`, from
  its `Inputs` and the `RefreshState` the last one handed back. The
  interactive dashboard runs all of it as a background `Job`
  (`src/app/refresh.rs`). What it carries from one
  refresh to the next (the session reader and its caches, the rows kept for
  failed reads, the waker) is moved into the job and handed back with its
  result, so the UI thread and the job share nothing behind a lock; the UI
  loop's next pass puts the rows in place, the selection on its row, and
  takes the state back. Keys during a refresh act on the rows last applied.
  At most one refresh is in flight: the next is due 850 ms after the last
  finished, or at once after an action asked for one, also one asked for
  while a refresh ran, so a slow Herdr means fewer refreshes, never
  overlapping ones. A panicked refresh shows a short status and takes its
  state with it; the next starts from a new one, a waker that takes the lock
  and the record up again included. Quitting waits up to two seconds for a
  refresh in flight, so a wake being typed is not cut off. The thread lives
  in the dashboard's process and never outlives it. A refresh worth
  reading appends how long it took, in all and per step (Herdr's snapshot,
  marks and relabelling, rows, wakes), and what it found to `refresh.log` in
  Corgi's state directory, which is moved to `refresh.log.old` past 256 KB:
  one that took 50 ms or more (`SLOW_REFRESH`), and one that found
  something else than the refresh before, another number of agents, Herdr
  gone or back, or another error; a panicked refresh is logged too. A
  refresh usually takes a few milliseconds, so the log holds days.
  `CORGI_REFRESH_LOG=all` logs every refresh, for debugging. The command-line tools
  and the Omarchy reader run the same refresh in place. The expanded
  transcript a key opens or moves to is read on the UI thread with a reader
  of its own, as before; the refresh rereads it with its own.
  `App::headless` is the same app for the command-line and Omarchy paths,
  which own no dashboard pane. `refresh::sort_agents` orders each project's rows:
  supervisor, then state, then the most recent `state_change_seq` first. That
  counter is one sequence for the whole Herdr session, not per agent, so it
  compares across panes and the dashboard keeps no ordering state of its own.
- `rows.rs` derives what a row shows from Herdr's metadata and the session
  facts: the project and its heading (`repo/checkout` for a linked worktree),
  the task summary, and the status-line bridge fallbacks.
- `catalog.rs` holds the per-harness model catalogs behind the Model row, and
  the harness, model, and effort rows' choices; `src/app/catalog.rs` hands a
  catalog that arrives to an open model list. Codex uses its App Server;
  OpenCode uses `opencode models`; static entries are an offline fallback. A
  discovered catalog is cached and re-fetched in the background once it is
  over an hour old, so a dashboard left open picks up a harness's new models
  without a restart; the cached list stays visible until the refresh lands,
  and a failed refresh keeps it.
- `launch.rs` is the checkout/create/start/prompt sequence. It is one
  function behind the dashboard form, the Omarchy popup, `corgi spawn`, and a
  supervisor's handover, so there is a single way to launch an agent. A selected
  model reaches the CLI as `--model` in the `agent.start` arguments, and a
  selected effort level as a `-c model_reasoning_effort` override for Codex
  or `--effort` for Claude Code; defaults send neither. New agents default to
  a fresh Git worktree: `worktree.list` resolves the selected agent's
  directory to its repository root (Herdr refuses to create worktrees from a
  linked worktree). Corgi creates or reuses the project workspace for that
  root, then passes that explicit workspace ID to `worktree.create`; Herdr
  picks a unique branch and path. This intentionally keeps workers
  independent of the Corgi dashboard's own workspace. The shared-directory
  option creates a separate tab in the same project workspace, and a supervisor
  starts in its root tab, or in a new tab of that workspace when an agent
  already runs in the root tab or it was split (a worker sent there by
  `corgi spawn --checkout root` is refused instead). The dashboard's `s` key
  builds the same `supervisor_plan` as `corgi supervisor` for the selected agent's
  project, taking the harness, model and effort from the supervisor's
  `launch.json`, else the form's presets, with no task; when the project's
  supervisor already runs, the key selects and focuses it as Enter does.
  The home directory itself (compared canonically, so `~` and `$HOME/` match;
  never its subdirectories) is never a project: `starts_supervisor` and
  `supervisor_plan` refuse it, and whatever the checkout, an agent there gets a
  plain workspace labelled `scratch` (`scratch 2` and on when that is taken),
  marked with the agent-workspace role so closing it closes the workspace. It
  never creates or looks up a project workspace, and is not remembered as a
  project. Its row has `scratch` set: it is grouped under Scratch after every
  project, left out of the form's project list, and ignored by the supervisor
  waker and handover. A launch with no task, which only a scratch agent can
  have from the form, skips the first prompt and leaves an interactive
  session. `run_launch` is the launch with the dashboard's focus put back
  after it. From the dashboard the launch runs as a background
  job, in `src/app/launch.rs`, whose progress replaces the form with a panel
  until the first prompt is confirmed. `progress.rs` is where a launch or
  merge reports its steps: the dashboard's job, stderr for the command line,
  or nowhere.
- `first_prompt.rs` delivers a new agent's first prompt, confirms it arrived,
  and waits out a question the agent opens on.
- `project_main.rs` is the Corgi-owned, agentless project workspace of each
  repository: found, created, relabelled `<project> supervisor`, and retired once
  unused. Corgi's project workspace is explicitly metadata-marked; it never
  claims an existing workspace based on a matching label. The mark names the
  project by a fixed-length FNV-1a digest of its root, because Herdr cuts long
  token values short; the root itself is written too when it fits, and a
  workspace marked before digests is still found by it.
  A supervisor launched beside agent sessions already running in its project
  (`corgi_project_main`, used only from `launch.rs`'s `Checkout::ProjectRoot`
  path for a `Role::Supervisor`) may instead adopt Herdr's own root workspace for
  the repository: the lowest-numbered workspace in `workspace.list`'s sidebar
  order whose checkout is the repository's primary one. `HerdrClient::list_workspaces`
  exists because that order is `workspace.list`'s alone — a session snapshot's
  `workspaces` happen to come back the same way, but nothing promises it, so
  finding the order itself always asks `workspace.list`. Adoption applies only
  when that root is not already Corgi's and holds no agent in any pane;
  otherwise the launch falls back to today's lookup and creation. A supervisor for
  a new project, or for one with no other sessions yet, never adopts. Once
  adopted, an older Corgi project-main of the same repository is closed if
  it is now unused, and both it and the adopted workspace can satisfy the
  ordinary lookup afterward: that lookup takes the first match in session
  snapshot order, so the adopted root, earlier in that order than an
  older duplicate, is what later launches keep finding.
  A project can end up with two Corgi project workspaces when a launch ran
  while Herdr had lost Corgi's marks (see "Herdr restarts" below) and created
  a second one. `project_main_workspace` then prefers the one the project's
  supervisor runs in, in any tab, and otherwise takes the first in snapshot
  order; every lookup (launch, close, merge, cleanup) goes through it.
- `close.rs` closes an agent: its worktree and checkout, its tab in a project
  workspace, or its whole workspace (`close_form` says which, `close` does it,
  and `src/app/close.rs` is the dialog), then retires the project workspace
  when nothing is left in it. Before that, any other Corgi project workspace
  of the same project that holds nothing — no agent in any pane, no dashboard,
  and only its recorded root tab with at most one pane — is closed as a
  duplicate, the way adopting Herdr's root workspace closes an idle older one.
  A duplicate with anything in it is left alone.
- `markers.rs` is Corgi's record of the metadata tokens it sets in Herdr,
  described under "Herdr restarts" below. Every Corgi token write goes
  through its `mark_pane` and `mark_workspace`; nothing else calls
  `pane.report_metadata` or `workspace.report_metadata`.
- `merge.rs` is the only Git command Corgi runs that writes: a confirmed
  merge of an agent's clean worktree branch into the branch already checked
  out in the repository's primary checkout, then a push through that branch's
  configured upstream, or, when the user hands a conflicted merge to the
  supervisor, the `git merge --abort` of that merge. Before the merge it
  fetches that upstream and, when the user asks, pulls it into the branch. The agent never
  performs the merge, and Corgi never changes the user's checked-out branch.
  The core has the Git steps and the supervisor's inbox lines; the dialog,
  its phases' keys and the job are `src/app/merge.rs`.
- `waker.rs` is `SupervisorWaker`, which wakes supervisors through their inboxes,
  puts the merge and merge-conflict lines in them, and hands them over, as
  described below; `draft.rs` tells it whether a supervisor's input box holds
  the user's draft.
- `setup.rs` is `corgi setup`. Herdr plugins cannot declare key bindings,
  so it appends one for the plugin's `open` action to the end of Herdr's
  `config.toml`, never rewriting what is there. It leaves a config that
  already binds the action alone, asks for the key on the terminal (none
  without one), refuses a key the config binds already, and lets
  `herdr config check` on a file of the binding alone judge the key's syntax.
  Opening the dashboard takes the user to its tab before they can read
  setup's lines, and `herdr plugin action invoke` passes no arguments, so
  when a key opens Corgi (bound already or bound now) setup first leaves
  `setup-notice.json` in Corgi's state directory, and removes it again if the
  dashboard did not open.
- `notice.rs` is that one-shot notice. The full dashboard's refresh job (not
  the compact Omarchy popup's) reads it at start and on every refresh and
  removes it; only the process whose removal succeeds gets the message, so
  two full dashboards never both show it, and one older than five minutes is
  removed unread. The UI thread shows it as `Overlay::Notice`, once no other
  popup is open, a success whose check stamp plays once it has opened.
- `sweep.rs` sweeps the state directory as the dashboard starts: the
  selection files of dashboards no longer running (`kill(pid, 0)`), and the
  `markers/` and `wake/` files of Herdr sessions whose socket is gone. A
  socket's file name cannot be turned back into its path, since every
  character but a letter or digit is spelled `_`, so the sweep walks the
  directories from `/` down to every entry that matches the spelling; a
  file goes only when none matches, it is not the dashboard's own session's,
  and it has gone untouched for a week.

### The dashboard: `src/app/`

`src/app/` owns the dashboard's state, its keys and its background jobs.
Its submodules are split by flow, each adding methods to the one `App`.

- `mod.rs` is `App`: selection, the expanded transcript, the plan
  usage and Codex thread-name jobs, key dispatch, and the terminal loop.
- `refresh.rs` decides when a refresh is due, runs the core's as a
  background `Job`, and puts its rows, the selection and the expanded
  transcript in place, as described above.
- `cards.rs` is the project cards: which runs of the sorted list are a card
  (led by a supervisor, not scratch), `CardMemory`, the expanded projects saved
  in `expanded-projects`, and the selection stops, which skip the workers
  folded into a collapsed card. `→` and `←` expand and collapse the selected
  card; `←` from a worker row selects its supervisor. A refresh keeps the
  selection on its stop, its row of the list, rather than on an agent index.
- `overlay.rs` is `Overlay`: nothing, or the one form or dialog open over the
  agent list (or a notice such as `corgi setup`'s), which takes the keys and is drawn on top. A launch or merge
  keeps its background job on `App`, since dropping a job would not stop its
  worker, and Esc never closes the overlay while that job runs. In the
  animated dashboard a launch that started keeps its panel up for 0.9 s,
  over its check stamp, then closes it by itself or at any key;
  without effects it closes at once, as the headless paths expect.
- `prompt.rs` is the prompt typed for an existing agent.
- `form.rs` is the new-agent form: one task row over a short list of
  selector rows, each with an inline filterable list. The default agent kind
  is `CORGI_DEFAULT_AGENT`, else the first installed CLI; the default project
  is the selected agent's repository, else the most recently used one. For a
  new project, or one with no agent session, the form launches the project's
  supervisor instead of a worker. It validates and builds a `LaunchPlan`, which
  `launch.rs` runs. The `t` key opens the same form for the home directory, a
  scratch agent, whose task may be left empty.
- `catalog.rs`, `launch.rs`, `merge.rs` and `close.rs` are the dashboard's
  side of the core's flows of the same names: fetching catalogs as the form
  asks, the launch job and its panel, the merge dialog and its job, and the
  close dialog.
- `cli.rs` is `corgi spawn`, `corgi supervisor`, `corgi fleet`,
  `corgi digest`, `corgi report`, `corgi notify`, `corgi inbox`,
  `corgi tag` and `corgi role`. The
  digest's parsing and caps are pure functions in
  `crates/corgi-core/src/digest.rs`.
- `edit.rs` is the dashboard's `e` key and `corgi edit`, which the plugin's
  `edit` action runs (as `./target/release/corgi edit`, with the
  `HERDR_SOCKET_PATH` Herdr gives every action). Both end in `open_checkout`:
  the top of the agent's Git work tree as it is on disk
  (`git rev-parse --show-toplevel`, else its working directory), a new tab
  there in the agent's own workspace labelled with its branch (else the
  directory's name), `pane.send_input` of
  `sh -c 'exec ${VISUAL:-${EDITOR:-vim}} "$1"' corgi-edit <root>` into the
  tab's shell, then the workspace and tab focused. The command is typed
  into the user's interactive shell rather than started by Corgi, so the
  `EDITOR` the shell's config sets applies though Herdr's own environment
  lacks it, and quitting the editor leaves that shell in the checkout;
  `sh -c` expands the variables the same way under bash, zsh or fish.
  The action asks `pane.current` with no caller pane, which is the pane
  focused in Herdr's UI. When that pane is a dashboard, the action uses the
  dashboard's selection instead. A dashboard's pane ID changes when the
  launcher moves it, and the action has no way to ask a running dashboard,
  so each interactive dashboard (full or compact) writes the pane ID of its
  selected agent to `selection/<socket>.<pid>` in the state directory,
  keyed by its own process ID: at its first frame, and whenever the
  selected agent changes, by a key or a refresh that moved the rows. The
  action looks for that file under the pane's foreground process IDs from
  `pane.process_info`, so it finds the right dashboard however many run. A
  foreground `corgi` without one is a dashboard started before this
  existed, which the action asks the user to reopen. Errors go to the
  status line from the key, and to a Herdr notification and stderr from
  the action.
- `bar.rs` is the `--bar-*` endpoints of the Omarchy widget. Their JSON is
  that widget's contract, pinned by snapshot tests, and they drive the same
  forms, key handlers, and launch as the dashboard.

### `corgi serve`: `src/serve/`

`corgi serve` is the data source of a frontend that draws its own window,
the desktop GUI, run locally or as `ssh HOST corgi serve`. It speaks the
protocol of `corgi_core::protocol` as JSON lines over stdin and stdout,
documented message by message in [docs/serve-protocol.md](docs/serve-protocol.md):
a hello with the protocol version and which commands touch Herdr's focus, a
snapshot of the whole state, then a delta whenever a refresh changed it.
Only protocol lines go to stdout; anything else goes to stderr. It exits
when stdin closes and no command is still running.

- It runs the dashboard's refresh pipeline (`refresh::run`) with a
  `SupervisorWaker`, so it contends for the per-socket wake lock like any
  dashboard and, holding it, restores marks, wakes supervisors and delivers
  their inboxes. It takes no part in deciding who holds it.
- `state.rs` turns the refresh's rows into the protocol's data: each row,
  the project cards with their worker counts and blocked workers (the
  dashboard's `project_runs` and `is_card`), each supervisor's open
  numbered questions from its ledger (`digest::all_open_questions`), the
  usage cards, the watched transcript and the palette, whose roles it sends
  in RGB for light and dark (the Omarchy theme, else Catppuccin). It never
  calls `palette::resolve`, which queries the terminal over stdin and stdout,
  where the protocol runs. It also computes
  a delta: rows and projects replaced whole by key, removals, and the order
  whenever it changed.
- `commands.rs` maps each command onto the core flow the dashboard's key
  runs: `prompt_agent` (never retried, never echoing the text), the
  new-agent form's presets and `launch::new_agent_plan` then
  `launch_agent`, `dashboard_supervisor_plan`, `merge_form`/`run_merge`
  with the dashboard's follow-ups (clear the tag, tell the supervisor, hand
  a conflict or failed push over), `close_form`/`close`, `tag_agent`,
  `go_to_agent`, `open_checkout` and the digest lookups. Quick commands run
  on the loop's thread; those that wait on Herdr, Git or a harness run on
  one of their own and reply from there, a panic becoming an error reply.
  Merges in progress are kept by pane between `merge_check`, `merge` and
  `merge_handoff`.
- `focus` and `edit` are the commands that call Herdr's focus methods,
  which move every attached client; the hello marks them so a GUI never
  sends them. `select` writes `selection/<socket>.<pid>` as a dashboard
  does, for `corgi edit` when `serve` runs in a pane.
- Its tests (`src/serve/tests.rs`) run the server against a fake Herdr that
  answers every request from a snapshot the test changes, with real Git
  repositories for the merges, and keep every file in a scratch directory.

### The desktop window: `crates/corgi-gui/`

`corgi-gui` is a GPUI window on `corgi serve`, which it runs as its child:
`corgi serve` beside its own binary (a release build's
`target/release/corgi`, or the one packaged with it), else on `PATH`, or
`ssh -T -o BatchMode=yes HOST corgi serve` with `--ssh`. It deserializes
into `corgi_core::protocol`'s types and keeps its copy of the state with
`State::apply`. Its layout is fixed, plain GPUI with no dock: the header
(the brand's pixel corgi, the HERD counts, a card per plan), a toolbar, the
overview of project cards and rows, the selected agent's session under it,
the side panel with the selected project's supervisor, its numbered
questions and its blocked workers, and a status bar. The session and the
side panel open and close, and the edges between them drag; their sizes, and
the window's, are kept in `$STATE/gui-layout.json`.

- `link.rs` starts the child and reads its stdout on a thread of its own,
  each line a `ServerMessage`; a line it cannot read (a newer server's) is
  skipped. When the child ends, the window keeps the last state, dims it
  under a banner with the reason (the end of the child's stderr) and
  connects again after 0.5 s, doubling to 15 s, back to 0.5 s after a hello.
  Requests still unanswered then are failed in their dialogs, and never
  sent again. Closing the window closes the child's stdin, so `serve` exits
  once its running commands have replied.
- `store.rs` keeps the state and its `seq`, asks for `resync` on a gap, and
  counts down from the server's `now`. Its `Outbox` numbers requests and
  refuses `focus`, `edit` and any command the hello marks `touches_focus`:
  Herdr's focus methods move every attached client, so the window never
  calls them.
- `dialogs.rs` is what every reply does: the prompt and answer boxes stay
  open with Corgi's error until a prompt is confirmed; the new-agent form
  (`form.rs`, the dashboard's rows and keys over the lists `serve` sends,
  with its own filter so a project typed over SSH is not looked up on the
  wrong machine) points at the row a refused `spawn` names; the merge dialog
  shows the steps, the outcome and the follow-ups the outcome allows; the
  close dialog asks again for a dirty worktree.
- `theme.rs` takes every color from the palette `serve` sends, light or dark
  as the system's appearance (Catppuccin Latte or Mocha, or the Omarchy theme
  for both), and the vendor and corgi colors from `palette::brand`.
- While the window is hidden (GPUI's `WindowVisibility`: macOS's occlusion,
  Wayland's suspended state), it stops drawing, stops its one-second tick
  and asks `serve` to stop reading the transcript; messages are still
  applied, so it is current when shown again.
- On Linux it enables `gpui-pre-platform`'s `wayland` feature and sets the
  Wayland app id `corgi-gui`, and picks a monospace font that exists there
  (Menlo does not).
- The `capture` feature drives the window from `CORGI_GUI_SCRIPT` (keys,
  waits) and writes screenshots with GPUI's `render_to_image`, for checking
  it without screen recording; `CORGI_GUI_BACKGROUND` opens it without
  taking focus.

### Herdr restarts

A Herdr restart or live handoff (`herdr update --handoff`) restores panes and
workspaces under the same IDs, but without the metadata tokens clients set.
Herdr v0.9.0 builds restored panes with empty tokens, and a handoff carries
none. Everything Corgi recognises by its tokens would silently stop: the
supervisor would count as a worker (no wakes, no handovers, `corgi fleet` says
`worker`), project workspaces would not be found (the next launch creates a
second `<project> supervisor` workspace), and agent workspaces would lose their
role. So Corgi keeps its own record and puts the tokens back.

- Every token Corgi sets is reported to Herdr and then written to
  `$XDG_STATE_HOME/corgi/markers/<socket>.json`, keyed by the Herdr socket
  like the wake lock: the supervisor marker, handover and baseline tokens, and
  a spawned worker's `corgi_request` id, by pane ID, with the Herdr name of the agent they were set for and its
  native session (Herdr's `agent_session`) once
  known; the project-main and agent-workspace marks by workspace ID, with
  the checkout Herdr reports for the workspace. Each change takes
  `markers/<socket>.lock`, reads the file, and writes it back only if it
  changed, through a temporary file renamed over it, so a dashboard, a launch
  on a background thread and `corgi spawn` never lose each other's entries
  and a reader never sees half a file. A record that cannot be written only
  costs the restore; the Herdr mark is what counts.
- The pass holding the wake lock, a hook's or a dashboard's, compares the
  record with each snapshot before anything reads it, so only one process
  reconciles at a time. It costs one
  small file read per refresh and a write only when something changed.
  Valid live session markers and workspace tokens update the record, since
  another process may have set them since. Legacy name markers migrate when
  the live name or recorded native session proves ownership; a recorded
  session mismatch rejects them. Stale pane tokens are cleared, retaining
  the old session record until clearing succeeds so a failed write cannot
  make a stale name trusted on the next refresh.
  Pane metadata reports are patches: Corgi sends JSON null for each omitted
  managed pane key to replace its own token set, leaving unrelated keys alone.
  Where Herdr has none, the recorded tokens are put back in one report only
  if the occupant is the same: for a pane, the recorded native session regardless of its name;
  for a workspace, the recorded checkout (none for a plain workspace). A
  pane or workspace that is gone, or holds another agent, session or
  checkout, has its entry dropped and nothing put back. A pane whose agent
  Herdr has not detected again yet, or whose session it does not know yet,
  is left for a later refresh. What is put back goes into the refresh's own
  snapshot too, so that same refresh already shows the supervisor and wakes it,
  and a status line says how many marks came back.
- A worker's `corgi_request` token has no identity in its value, unlike
  the supervisor's `session:` marker. While Herdr has it and no session is
  recorded for the pane, the agent in the pane is taken to be the one it was
  set for and its session is recorded; from then on the same rules apply as
  to the supervisor's marks, so a later session in the pane does not inherit
  the id. Workers never get a supervisor marker from reconciliation.
- A supervisor's merge tag (`corgi_merge`, see "Ready to merge") is recorded
  and put back like the request id. `corgi tag` and the clears report only
  that one key, so a worker's request id beside it is left alone; the
  launch's and the supervisor's own full mark sets clear it, as they clear every
  Corgi pane token they leave out.
- A supervisor launched and lost in a restart before any pass saw its
  session is not re-marked: without a recorded session there is nothing to
  confirm the pane still holds it. The hook Herdr runs when it detects the
  new agent (`pane.agent_detected`) records the session at once, and a
  dashboard's next refresh would too.
- Herdr's plugin-pane records are Herdr's own and not part of this; the
  `open` action copes with losing them (`scripts/herdr-corgi-dashboard`).
- The startup hook puts the marks back: Herdr runs it as its server starts
  and again after a live handoff, within about 50 ms, before any window is
  open. Panes whose agent Herdr detects only later are put back by the
  `pane.agent_detected` hook that follows.

### Herdr's hooks

`herdr-plugin.toml` declares a `[[startup]]` hook and `[[events]]` hooks on
`pane.agent_detected`, `pane.agent_status_changed`, `pane.exited`,
`pane.closed` and `workspace.closed`, all running `corgi hook`
(`src/app/hook.rs` around `crates/corgi-core/src/hook.rs`). Herdr starts it
in the plugin's root with its own environment, `HERDR_PLUGIN_EVENT` naming
the event and `HERDR_PLUGIN_EVENT_JSON` holding it, and keeps what it prints
in its plugin log (`herdr plugin log list --plugin io.github.zyrre.corgi`):
one line per run, saying what woke it, how many passes it made or that it
left the work to the pass holding the lock, how long it took, and the
status lines of its passes.

- **Trigger, then reconcile.** Herdr 0.9 runs at most 32 plugin commands
  at once across all plugins and drops the events beyond that, starts hooks
  in parallel and lets them finish in any order, says no previous status,
  state change counter or session in an event (an idle nobody looked at
  arrives as `done`), fires `pane.closed` only for an explicit close (a
  shell that exits gives `pane.exited`), loses the events of the time it is
  down, and runs no hook for token changes or for time passing. So the
  event decides nothing: every hook makes the same pass, `refresh::run`
  with a `SupervisorWaker`, which reconciles from Herdr's snapshot and
  Corgi's records (the agents followed, the handovers, the marks, the
  inboxes). A dropped event is caught by the next hook, whatever it is for,
  or by the startup hook. Exits and closes need no work of their own: the
  pass drops what it followed of agents, panes and workspaces that are gone.
- **One pass at a time.** A pass holds `wake/<socket>.lock` from before
  its snapshot to after its wakes. A hook first sets `wake/<socket>.dirty`,
  then tries the lock; finding it taken, it exits at once, and the process
  holding it goes round once more when its pass ends, since that pass's
  snapshot may predate the event. Because the flag is set before the lock is
  tried, it is never set after the holder last looked: a hook looks once
  more after giving the lock up, and goes round again if the flag is set
  and it can take the lock back. A pass that begins clears the flag. A
  burst of 16 hooks makes a few passes and delivers once.
- **Time limits.** No pass starts after five seconds; what is still dirty
  then is left to the next hook. A watchdog thread ends the process after 30
  seconds (exit code 124) whatever it is doing, since Herdr gives a hung
  hook no time limit and its slot is taken for good. The wake lock goes
  with the process. Every Herdr call has its own deadline, so the watchdog
  is for what has none, such as another process's lock held for good.
  `CORGI_HOOK_LIMIT_MS` lowers the hard limit, for the tests.
- **State between processes.** A hook is a new process, so everything a
  pass needs from the one before is on disk: what was seen of each agent
  (`seen.json`), where each handover stands (`handovers.json`), the marks
  record, and the inboxes, which record every delivery. A pass takes them
  up as it begins and writes them as it ends (`SupervisorWaker::begin` and
  `end`). An agent the record does not have counts as having appeared
  while nobody watched only at a process's first pass, as at a dashboard's
  start.
- **Replacements.** A supervisor's replacement (its exit, then its
  successor's launch) runs on a thread of the hook with the lock given up,
  for at most 90 seconds, with the watchdog's limit moved past that; then
  one more pass records how it went. A replacement whose process went
  before recording it (a dashboard that quit, a hook ended by its watchdog)
  counts as failed after `supervisor::REPLACEMENT_SECS` (three minutes), so
  the project's wakes do not wait for good.
- **Refused and unknown prompts.** Before typing a project's inbox in, a
  pass records the items' ids in the recovery journal, and clears them once
  Herdr confirms (`recovery::begin_delivery` and `finish_delivery`), so an
  item whose delivery nobody confirmed is never typed in twice. Herdr's
  `agent.prompt` refuses before writing anything with `agent_not_found`,
  `agent_target_ambiguous`, `agent_blocked`, `agent_not_ready` (the agent is
  not the pane's foreground process) or `empty_agent_prompt`: then the ids
  are cleared again (`recovery::abort_delivery`) and the items go out at a
  later pass, once the supervisor takes prompts (`waker::Delivery`). Any
  other failure (the connection, `agent_prompt_failed`, `timeout`) may have
  typed them, so they stay held, and with them everything later for that
  project, until they are read and recorded with
  `corgi inbox <project> --take`. A held inbox shows in every pass's status
  line (a dashboard's, a hook's log line), in `corgi fleet`, `digest`,
  `report` and `inbox` (`N inbox item(s) held: …`), and in `corgi serve`'s
  `held` count of each project, since a hook may hit it with no window
  open.
- **Dashboards.** A dashboard makes the same pass at each refresh, under
  the same lock, so it is the fallback where no hook runs: a Herdr whose
  plugin was linked before the manifest had hooks, or a hook that failed. A
  refresh that ends to find the flag set is due again at once. With hooks
  and dashboards taking turns, nothing is delivered twice: a delivery is
  made and recorded under the lock, and the next pass reads the inbox
  afresh. A dashboard of an older Corgi holds the lock for as long as it
  runs, so the hooks leave all the work to it until it quits. The idle
  prompt-cache handover stays with dashboards (hooks run only when
  something happens, so none could time a rest); the handover at 50% of the
  context is the hooks' as well.

### The supervisor

- `crates/corgi-core/src/supervisor.rs` holds what makes a session a project's
  supervisor: the role from `supervisor/ROLE.md`, embedded in the binary and
  written into the project's supervisor directory with the path of the Corgi
  binary that launches it; that directory's files; the first prompt; and the
  pure state machines behind waking and handover. The role is loaded into
  every turn, so what a supervisor needs only at one moment is not in it: how
  to hand over is `supervisor/HANDOVER.md`, which `corgi role handover` prints
  and the handover request names, and what to do after a merge, a conflicted
  merge or a failed push is in the `[Corgi]` line that reports it. A
  supervisor runs on Claude Code, the default, or Codex, which `Harness` gives
  the role arguments of. Claude Code appends the role file to its system prompt.
  Codex takes the role inline as `-c developer_instructions`, which adds to
  its own instructions (`model_instructions_file` would replace them), and
  runs the supervisor in its workspace-write sandbox with network access, which
  is what lets `herdr` and `corgi` reach Herdr's Unix socket, and the state
  directory as an extra writable root. The launch marks the supervisor's pane
  with a `corgi_handler` token holding `session:<native agent_session value>`,
  so presentation renames preserve its role and later sessions in the pane do
  not inherit it. Before Herdr exposes the session, the launch name marks
  the pane before the first prompt; the prompt acknowledgement and dashboard
  reconciliation upgrade it once native identity is known. Legacy name
  markers match the live name, or migrate after a rename when the recorded
  session proves ownership. A status-line `session` token cannot establish
  ownership because it can survive pane reuse.
  Herdr does not keep them across its own restart; Corgi's record puts the
  marker back on the same session (see "Herdr restarts").
  The role names the installed plugin's release build (`plugin.list` gives
  its root) unless `CORGI_BIN` chooses another.
- The role was called the Steward, then the Project handler, then the
  corgi, before it became the supervisor, and what an older Corgi left
  behind keeps working. The pane marker kept the key it had as the Project
  handler, `corgi_handler`, since every Corgi build in a Herdr session reads
  the marks on its panes, and a new key would hide the supervisor from an
  installed dashboard not yet rebuilt or another worktree's build; its
  other pane tokens (`corgi_handover`, `corgi_baseline`, `corgi_merge`) keep
  their keys for the same reason. A pane marked with the Steward's
  `corgi_steward` token is the supervisor's, and the next write of its marks
  moves the marker to `corgi_handler`, in Herdr and in Corgi's record. A
  project workspace still labelled `<project> corgi`, `<project> handler` or
  `<project> steward` is relabelled `<project> supervisor`;
  and `CORGI_HANDLER_BIN` and `CORGI_STEWARD_BIN` are still read. The
  dashboard's own lines start with
  `[Corgi]`, the app's name, so they are never taken for the supervisor's
  `[supervisor]`; `corgi notify` turns a leading `[corgi]`, as a script
  written for an older Corgi adds it, into `[Corgi]`.
- The supervisor's memory moves with it, and nothing in it is overwritten,
  merged or deleted. Until the next supervisor session starts, a project whose
  state is still in `corgis/<project>`, or `handler/<project>` or
  `steward/<project>` when it never moved from there, is read there. The launch of that session
  (`supervisor::prepare`, at a fresh start and at a handover alike) moves the
  directory to `supervisors/<project>` with a single rename and leaves a link to
  it at the old path, so a session an older Corgi started and an older
  Corgi binary keep reading and writing the same files; the links the
  previous renames left at `handler/<project>` and `steward/<project>` lead
  there through it. The
  move waits for a launch, rather than the dashboard's first look, because
  a handover asks the old session to exit before its successor is
  prepared: no running session, whose harness may have resolved the old
  path for its sandbox or its allowed directories, has its files moved
  from under it. A link at an old path is never moved or removed: one to
  the new directory means the move is done, and one to a directory
  elsewhere gets a second link to the same directory at the new path.
  When the new path and an old one hold directories of their own, Corgi
  uses `supervisors/<project>` and warns, naming both, so the older history is
  not overlooked: in the launch's progress (stderr for `corgi supervisor`), on
  stderr for a supervisor's `corgi spawn`, and once per project in the
  dashboard's status line.
- `corgi spawn` recognizes a supervisor caller by its pane: `HERDR_PANE_ID`
  must be the pane of an agent carrying the supervisor marker. Without
  `--harness`, such a spawn starts the worker on the kind Herdr detected in
  the supervisor's pane, rather than on the form's default.
- Herdr's hooks and every interactive dashboard wake supervisors, from the
  same pass of the refresh (`crates/corgi-core/src/waker.rs`; a dashboard's
  on its refresh's background thread), one pass at a time (see "Herdr's
  hooks"). Every agent of a project with a running supervisor
  counts, except the supervisor, whoever started it, and so does one the supervisor
  spawned (it carries a `corgi_request` token) while the project has no
  running supervisor, so the next supervisor hears of it. `supervisor::Transitions` reduces each agent's
  state from refresh to refresh to wakes: nothing until the agent has been
  seen working, so neither the states found at a first start nor a question
  a new agent opens on is news; after that, each time it rests (done or
  idle, which count as one) after working, and each change between blocked
  and resting. The same resting state with a newer `state_change_seq` means
  a turn passed between two refreshes, and wakes too. Only the pass
  holding an exclusive lock on `$XDG_STATE_HOME/corgi/wake/<socket>.lock`
  follows the agents, adds wakes and delivers, and it holds the lock only
  for that pass. Every hook and every interactive dashboard (the tab, the
  popup, a preview) tries for the lock; the Omarchy reader and the
  command-line tools never wake.
- Every wake goes through the supervisor's inbox
  (`crates/corgi-core/src/inbox.rs`), `inbox.jsonl` in its state directory, so
  none is lost or sent twice when a dashboard closes, crashes, restarts or
  hands the lock over. It is append-only JSON lines, written only through that
  module under `inbox.lock`: items (`id`, `ts`, `project`, `source`, `kind` of
  `wake`, `conflict`, `merge` or `note`, `agent`, `state`, for a wake the
  agent's `pane` and `change` (its `state_change_seq`), a coalescing `key`,
  the `[Corgi]` `text`, `own`, the spawn's `request` id for one of the
  supervisor's own, and the agent's report inline or as `report_file`), and
  records naming the ids delivered. An item no record names is
  undelivered, and an id is added only once. Readers take no lock and skip
  a last line still being written; a writer that finds the last line cut
  short (a killed writer, a full disk) ends it with a newline before its
  own. The dashboard reads each supervisor's inbox at each refresh in which
  its file changed: every write appends to it or replaces it by a rename, so
  the waker keeps the inbox it parsed with the file's device, inode,
  length, and modification and change times, and reads it again only when
  one of them differs (`InboxCache`). Past 256 KB,
  a write rewrites the file with every undelivered item and the newest 100
  delivered ones, and removes the dropped items' report files. The
  dashboard adds wakes, the merge-conflict handoff and the merge line in
  process (any interactive dashboard may add the last two, since the leader
  delivers them), and `corgi notify` adds anything else (a script's or a
  hook's line) through the same code. Projects of the same directory name
  share a state directory, so each item names its project root, and a
  supervisor gets only its own project's items.
- Inbox privacy lives in `crates/corgi-core/src/inbox_privacy.rs`. Before
  supervisor preparation writes content, its Corgi-owned project state
  directory is protected. New state/report directories request 0700; inbox,
  lock, spilled report and compaction files request 0600 at open, before
  content is written. Umask may remove further owner permissions. Repairs only
  remove bits (directory mode AND 0700, file mode AND 0600), never grant
  missing owner access; an access failure stops writes. Existing
  HOME/XDG/shared parents are not chmodded. Every inbox read (including the
  dashboard's, when the file changed, and CLI reads), append,
  delivery write and saved-report read checks the state directory, reports
  directory, inbox, lock and interrupted compaction file. All old report files,
  including orphans, are swept on first access in a process and again when the
  reports directory's identity or timestamps change. Successful sweeps are
  cached only in memory, keeping unchanged polling independent of report count;
  an in-place external chmod of an unselected old report is revisited by a new
  process or the next directory change. A selected report is always checked.
  State-directory migration aliases remain supported. Content symlinks,
  report-directory symlinks, nonregular files, multiply linked files and
  foreign-owned entries fail closed, without chmodding/truncating their targets.
  Opens use O_NOFOLLOW and validate the opened inode before chmod or truncation.
  This boundary protects against other local users, not concurrent filesystem
  replacement by the owning user. CLI inbox/report errors propagate; existing
  infallible dashboard readers log errors and return no private content.
- A wake's id names the agent, its pane, the run of state changes it is
  in, its `state_change_seq` and the state, and an item whose id the inbox
  already has is not added again. Herdr restores a pane's id and agent
  name after it restarts, but counts its state changes from the start
  again, so the run is part of the id: it is set when the dashboard first
  follows the agent, and again whenever its `state_change_seq` goes back.
- What the last pass saw of each agent (its pane, run, last
  `state_change_seq` and `Transitions`) is kept in
  `wake/<socket>.seen.json`, written after that pass's items, and only
  when it changed. Every pass starts from that record rather than from what
  it sees, so whatever changed while no pass ran is news: a worker that was working at the last save and is done now wakes
  its supervisor once. An agent of the same name in another pane is another
  agent. An agent the record does not have, and that rests now, wakes
  nobody, except one the supervisor spawned that is done, or idle with a report
  from its transcript (a screen alone may be one that never started): it
  came and finished while no dashboard ran, and is reported once. Without
  any record, as at the first start, everything found is a baseline. When
  a wake cannot be added (a full disk, a state directory that cannot be
  made), what was followed of the agent is put back before the record is
  saved, so the next refresh finds the same stop and adds it.
- When an agent rests, its report, what `corgi report` printed then (the
  newest thing said in its transcript, else its screen), goes into the item:
  inline up to 4 KB, else in `inbox-reports/<id>.md`. For a worker the
  supervisor spawned (its wake carries the `request` id) whose report comes
  from its transcript, the same report is also written, under the inbox
  lock, to its own new file in `reports/`, `YYYYMMDD-HHMMSS-<request id>.md`
  (a `-2`, `-3` … when a name of that second is taken), headed by the agent,
  its state, the time, the request id and the project. A screen, and any
  report of an agent the user started, is never kept there: the user's own
  sessions are not stored for good. Whether the report came from the
  transcript is a field of the item in memory only (`said`), not written to
  the inbox. Compaction never touches `reports/`, so a worker's report stays
  searchable after its item is dropped; the files are private like the
  inbox's (0700 directory, 0600 files). A wake's line names
  the agent, its state, and the command to run next. When it goes out for
  one of the supervisor's own workers with an inline report, and the prompt's
  reports stay within 12 KB, the line says the report follows instead,
  and the report, each line quoted with `> ` so that none passes for a
  `[Corgi]` line of the dashboard's, and a closing `[Corgi] End of
  <agent>'s report.` line go with it.
- Delivery reads the supervisor's undelivered items and sends them as one
  `agent.prompt` once the supervisor is neither working nor blocked, so they
  arrive after its turn, the newest item of each key only (a newer wake for
  an agent replaces an unsent one, though one without a report does not
  replace one with a report), then records all of them delivered. Items of
  one key stay apart in the inbox and are reduced only here
  (`inbox::coalesce`), so `Waker::pending_for` lists them all; a test of
  what the supervisor gets checks what `coalesce` keeps. When
  that record cannot be written, the dashboard does not type those items
  in again, and tries the record again at each refresh.
  They also wait while the supervisor's input box
  holds a draft, so they are never typed into what the user is writing.
  Just before sending, the dashboard reads the supervisor's visible screen with
  its styling and finds the box: for Claude Code, the line starting with `❯`
  directly below a horizontal rule, down to the next rule; for Codex, the
  last line starting with `›`, down to the next blank line. Any text in it
  that is not dim is a draft, a `[Pasted text #1]` marker included. Dim text
  counts as empty because it is the harness's own: Claude Code's prompt
  suggestions and Codex's placeholder, which look just like typed text once
  the escapes are stripped. Only the box's content counts, not focus or
  recent keystrokes. A screen that cannot be read, a box not on it, or a
  harness whose box Corgi does not know counts as no draft, so the items
  go out as before rather than waiting for good. The inbox belongs to the
  project rather than to one supervisor session, so the items a supervisor that is
  handing over did not get go to the supervisor that takes over from it, and
  those of a project without a running supervisor wait for the next one. The
  delivery uses no harness feature: typing into the box is the one way that
  starts a turn on every harness.
- `corgi inbox <project>` prints the project's undelivered items, oldest
  first, the newest 20 at most, each with its report while about 16 KB of
  reports last, else the command that prints it; `--delivered` adds the
  newest 10 delivered ones. Printing marks nothing delivered, since the
  dashboard's typing is what reaches the supervisor; `--take` records the
  printed items delivered, for a supervisor that acts on them itself, so they
  are not typed in again. The role tells a supervisor that is handing over to
  leave them to its successor.
- The same dashboard hands a supervisor over to a fresh session once its
  context is half full, since compaction would lose what only its
  conversation holds (`supervisor::Handover`, once per harness session). A
  supervisor that is idle or done, never working or blocked, at 50% or more
  gets one `[Corgi]` line asking it to run `corgi role handover <root>`,
  write `handover.md` in its state directory as that prints, and end its turn, held like a wake while its input box holds a
  draft and asked at a later refresh; the request is recorded as a
  `corgi_handover` pane token (`<unix seconds> <session>`), so a dashboard
  that takes over the wake lock during that turn adopts it instead of
  asking again. When the turn ends with a note written since the request,
  and its input box holds no draft (checked at each refresh, the same way
  as for wakes; once the note is found, a draft only delays the
  replacement, and neither the two-minute limit nor the note check can
  fail it), a background thread types the harness's own exit command
  (`/exit` for Claude Code, `/quit` for Codex), which leaves the old
  transcript on disk and the pane at its shell, waits for Herdr to clear
  the agent, and runs the usual supervisor launch in that same pane under the
  same name: the role is prepared again, the marker set again, and the
  first prompt has the new session do the start-of-session reading, then
  read the note and move it to `handovers/<YYYYMMDD-HHMMSS>.md`, and
  instead of a greeting say in one line that it took over, restate in full
  each open question the note hands on, and sum up what else is open. It starts
  on the harness, model, effort, and extra arguments the launch recorded in
  `launch.json`, or the harness's defaults without one. A supervisor seen at
  work after its note was found (a turn the user sent from the draft that
  held the replacement) has a stale note: once that turn is over it gets
  another `[Corgi]` line asking it to bring the note up to date and end its
  turn, held like any request while there is a draft, and it is replaced
  only once a note newer than that request exists. Wakes for the project
  wait from the request until the successor's first turn is over, and one
  queued while no supervisor was visible still reaches it. A turn that ends
  without a note (or without an updated one), or a request not taken up
  within two minutes, leaves the supervisor in place with a status line, and
  it is asked again only after a later turn of its own.
  `CORGI_DEBUG_HANDOVER_PERCENT` lowers the threshold for
  a debug run; it is not a setting. Where each supervisor's handover stands
  (`supervisor::Handover`) is kept in `wake/<socket>.handovers.json`, so a
  hook that asks and a dashboard that replaces, or the other way round, are
  one handover.
- A second trigger hands over a supervisor whose prompt cache is about to go
  cold, since its next turn would otherwise re-read the whole conversation
  uncached, while a fresh session is small enough to wake cold cheaply. It
  fires, with its own `[Corgi]` wording and status line and otherwise the
  same request, note check, exit, relaunch, and once-per-session rule, when
  the supervisor has rested (idle or done) for 50 minutes on Claude Code (one
  hour cache) or 25 on Codex (about thirty minutes), no other agent of its
  project is working or blocked (the dashboard cannot tell which of them
  the supervisor dispatched, so all count), and its context in tokens is at
  least twice its baseline. The baseline is the context at the first
  refresh that finds the session resting with one: the end of its first
  turn (the start-of-session reading, or the note read) when the dashboard
  saw it, else the first rest this dashboard saw, which only ever demands
  more growth. It is kept as a `corgi_baseline` pane token (`<tokens>
  <session>`), written with the marker and any `corgi_handover` token so
  none is lost whether Herdr merges or replaces tokens, and adopted by a
  dashboard that restarts or takes over the wake lock. The rest is timed
  from the first refresh that saw it, so such a dashboard starts that clock
  again and may ask late, never early. A session that only did its start-up
  never qualifies, and a supervisor on a harness whose session file Corgi
  cannot read is never handed over for idleness. The 50% trigger goes first
  when both hold. `CORGI_DEBUG_HANDOVER_IDLE_SECS` sets the idle time for
  a debug run on any harness; it is not a setting. Only a dashboard asks
  for this one: it is timed, and Herdr runs hooks only when something
  happens, so it waits for a Corgi window to be open.
- Claude Code's folder-trust question is the one question Corgi answers. It
  keys trust on a repository's main checkout, so the first worktree of a
  repository nobody trusted yet opens on it, even under a trusted folder. When
  the first prompt (`app/first_prompt.rs`) meets a blocked agent,
  `activity::folder_trust_dialog` must recognize exactly that dialog, naming
  the agent's own directory, and that directory's main checkout must be
  listed in `$XDG_STATE_HOME/corgi/created-projects`, which a new project's
  launch appends to. Only then does Corgi select "Yes" through
  `agent.send_keys`, confirm the highlighted answer, and press Enter.
  Otherwise it waits for the user.
  Codex asks the same question about a worktree's main checkout, and takes
  the answer on its command line instead: for a project in that list, a
  Codex launch gets `-c projects={"<main checkout>"={trust_level="trusted"}}`.
  It is one inline table because Codex splits a dotted `-c` key at every dot,
  and paths have dots. It stands in for the configured `projects` table for
  that session and writes nothing to Codex's configuration.

### Drawing and shared pieces

- `src/ui/` renders the Ratatui dashboard. A project led by its supervisor is
  a card, drawn as list items whose lines carry the card's border: collapsed,
  one item of the supervisor's rows, a divider, and the sum of its workers;
  expanded, the supervisor's item with the card top, then one item per worker,
  the last closing the card. Other projects' headings lead the item of their
  first session, and each project's gap closes its last item, so the list,
  which scrolls by whole items, never shows a first session with its heading
  or card top cut off, nor starts on a blank line. Each agent row takes an
  identity line, a message line, and a tool line. `space` grows the selected agent's
  message line, in the place it already occupies, into that session's latest
  turns down to the bottom of the box: every turn held to two rows except the
  newest, which an agent usually ends a task with and which is therefore drawn
  in full. Nothing above the selected identity line moves, and the sessions
  below it are the ones that give way, so expanding and collapsing is one row
  changing height. The expansion is a flag on the list rather than an overlay
  of its own, so prompting, merging, and closing keep working over it.
  Terminal text is always rendered as plain text. `mod.rs` lays out the
  screen, draws the footer, and holds the dialog helpers;
  `header.rs` is the brand and the plan-usage cards; `agents.rs` the agent
  list and the expanded transcript; `status.rs` the fields of an agent's
  status line;
  `new_agent.rs` and `choice_list.rs` the new-agent form and its lists;
  `dialogs.rs` the close and merge dialogs; `prompt.rs` the prompt dialog;
  `effects.rs` the popups' motion, below. Every color comes from
  `src/theme/`, below.
  The usage cards, status fields, and dialog lines are also what
  `app/bar.rs` sends the Omarchy widget, so both show the same thing; the
  widget gets a dialog's keys as a line of words, since it draws no keycaps.
  One function, `agent_status_line`, draws the identity line of worker rows,
  supervisor cards, and the expanded view alike. Its leading part, the state
  badge and the last-activity time, is never dropped; the `status_fields`
  after the task are dropped from the right on a narrow row, and are what the
  widget reads. A field meant for the dashboard alone therefore belongs in the
  leading part.
- Every popup shares one look inside its frame: a mark in the title (`◆`,
  `✓` or `✗` once it has an outcome), dimmed labels that line up beside
  bright values, the model and harness in their vendor's color, the keys as
  keycaps under a faint rule, a launch or merge as a step list (`✓` done, the
  spinner on the current step, `·` pending), and an outcome as a `✓`/`✗`
  headline over muted details. A launch's steps come from its `LaunchPlan`
  (`launch_steps`) and a merge's from its form; the worker says which one it
  is on with `Progress::step`, and its status lines still name the detail.
- Popup motion. `src/motion.rs` is the state and timing of every effect, and
  nothing in it draws. `Motion` lives on `App`: its clock (the wall clock in
  the interactive dashboard, a manual one tests set), where the one popup is
  in its life (hidden, shown since a time, or closing from the frame it was
  last drawn in), the outcome whose effect plays, and when the caret was last
  put back on by a key. `ui::draw` tells it once a frame whether an overlay is
  open and which outcome it shows (`Overlay::outcome`); an overlay appearing
  starts the frame growing, one going away leaves its frame shrinking, and an
  outcome that was not there last frame plays once. The renderers draw their
  content as before and report where their frame went (`Drawn`); the
  compositor in `ui/mod.rs` then works on the finished buffer with
  `effects.rs`: dimming the dashboard behind a popup, putting back what was
  under a frame still growing and drawing that frame alone, fading content
  in, recoloring the frame for a flash, and moving it sideways for a shake.
  Every popup that ends in success stamps a check where it stands: a launch
  or merge when its outcome arrives, and a prompt sent or a close confirmed
  when the confirm key's action succeeds (`Motion::succeeded`); Esc and
  failures never do. A popup that is a success from its first frame, such
  as a notice, would start its stamp while still growing, so `ui::draw`
  gives the notice its outcome only once `Motion::settled` says it has
  opened, rather than through `Overlay::outcome()`. The stamp keeps its own 880 ms clock, apart from the
  popup's: the frame turns green at once, a big four-row `✓` is drawn column
  by column centred on the popup with two clear columns either side, holds,
  and fades while the frame eases back to its color. It is drawn last, over
  the popup (whose content fades almost out under it and back), over the
  frame shrinking away, and over the dashboard once the popup is gone, so a
  popup that closes on success does not wait for it and keys go on to the
  list at once; it is clipped at the screen's edge, never skipped. A popup
  opening while it plays cuts it off. The loop stays at its fast rate until
  it ends, and since every frame is drawn afresh, it leaves nothing behind.
  Effects blend in the theme's RGB (`theme::mix`) and end every effect on
  the role's color itself. The spinner and the caret blink are pure
  functions of the clock, so a render at a set time is always the same
  frame. After each draw the loop asks `Motion::frame_interval` how long to
  wait for a key: 16 ms while anything moves, less the time the frame took to
  draw and write, since writing to the terminal takes a good part of that;
  otherwise the usual 80 ms after drawing, cut short only to land on the
  spinner's next frame or the caret's next turn, so an idle dashboard draws
  no more often than before. A snapshot refresh runs in the background and
  so never stalls a frame, and putting its rows in place is cheap enough not
  to wait for motion to stop. The caret is the real
  terminal cursor, shown and hidden on Corgi's clock; the dashboard asks the
  terminal for a steady bar cursor and gives back the user's shape on exit.
  A dashboard without an animated `Motion` (tests, the headless paths) shows
  every effect at its end.
- Colors. `crates/corgi-core/src/palette/` is the one place a color is
  written, and `src/theme/` the dashboard's view of it. A `Theme` is a
  `Palette` (the terminal's foreground, background and sixteen ANSI colors as
  RGB) and the `Roles` the UI draws with, worked out from it once: accent,
  muted, success, warning, danger, unknown, merge, text and so on, plus the
  derived shades (the popup fill, rule, keycaps, focus bar, the dimmed
  dashboard, what popup content fades in from, the success frame, the flash,
  and the context ramp). A role is the terminal's default, an ANSI color, one
  of the fixed 256 or an RGB (`palette::Color`), which `src/theme/` maps one
  to one onto ratatui's colors, and `Theme::rgb` turns into RGB for a frontend
  that is not a terminal. The UI reads them with `theme::roles()`.
  `palette::resolve` picks the palette once at startup, in raw mode before the
  first frame: `CORGI_THEME` when set; else the active Omarchy theme's
  `alacritty.toml` (`~/.local/state/omarchy/current/theme/`, or
  `~/.config/omarchy/current/theme/` before Omarchy 4), which Omarchy renders
  for every theme with the whole palette resolved; else the terminal's answer
  to OSC 10, 11 and 4 (0–15), followed by DA1, whose answer ends the wait,
  within 100 ms; else Catppuccin, Latte when the reported background or the
  system (macOS `AppleInterfaceStyle`, the freedesktop portal or GNOME's
  `color-scheme`) is light, Mocha otherwise. Herdr answers the queries itself,
  from the palette it read off the terminal it runs in, in well under a
  millisecond, also when that terminal is at the other end of SSH; a terminal
  that never answers costs the 100 ms. Whenever the palette is the terminal's
  own (Omarchy themes the terminal too, and Catppuccin only stands in for one
  it could not read), roles stay the terminal's ANSI colors (`Color::Cyan` in
  ratatui, and so on), so they match it exactly and keep following it when
  Omarchy switches theme under a running dashboard; only shades come from the
  palette's RGB, 24-bit when `COLORTERM` says so, else snapped to the
  256-color cube. `CORGI_THEME=catppuccin-mocha|catppuccin-latte` draws every
  role in Catppuccin's own RGB instead. A light palette swaps what depends on
  a dark background: body text becomes the terminal's foreground rather than
  bright white, the popup fill a step off the background rather than black,
  and the flash dark. Over xterm's defaults, the theme before one is resolved
  and in tests, the shades come out as the fixed greys and ramp Corgi drew
  before it had themes. The mascot's colors and the model vendors' are brand
  colors (`theme::brand`), fixed in every theme. The Omarchy reader
  (`app/bar.rs`) gives the widget each role's tone name and an RGB shade as
  its nearest 256-color index, so the widget is unchanged.
  `src/readme_shots.rs` draws the README images in a fixed Catppuccin Mocha,
  and its `theme_preview` test writes the dashboard and popups in several
  themes as ANSI text for `less -R`.
- `crates/corgi-core/src/choices.rs` is the pure model behind every selector
  list in the new-agent form: the rows, the word filter typed over them, and
  how typed text that names no row is offered (as a value of its own for
  harness and model, as a completed directory for the project, or not at all).
- `src/textfield.rs` wraps the form's task text and moves its caret along the
  rows the reader sees.
- `crates/corgi-core/src/job.rs` is work handed to a background thread: its
  progress messages and its outcome, polled from the UI loop so the UI thread
  never waits, and a panic reported rather than lost.
- `crates/corgi-core/src/time.rs` is the wall clock without a date crate: Unix
  seconds, RFC 3339 timestamps as the agent CLIs write them, and the UTC
  stamps handover archives are named with.
- `crates/corgi-core/src/paths.rs` is the home directory, Corgi's state
  directory under `$XDG_STATE_HOME` or `~/.local/state`, the `~` shorthand
  paths are typed and shown with, and `dir_name`, the last path component a
  project or checkout is known by.
- `crates/corgi-core/src/test_support.rs` is compiled for tests and the core's
  `test-support` feature: scratch directories and a fake Herdr socket that
  answers scripted replies. `src/test_support.rs` adds a dashboard that never
  reads the developer's own state, and rendered screens read back as text.
- `herdr-plugin.toml` exposes a persistent tab and a compact popup.
- `omarchy/` is an optional compact QML dashboard. Its owned reader process
  reuses the Rust dashboard data pipeline and emits snapshots every two seconds.
  The socket is explicit, since the desktop shell has no injected Herdr session.
  Its popup also creates agents (`--bar-new-options`, `--bar-new-agent`) and
  merges or closes them (`--bar-action`) with the TUI's own forms and launch
  sequence, and `--bar-navigate` (`src/app/navigate.rs`) shows a pane in
  Herdr and then focuses the Hyprland window of the terminal attached to that
  session. Each release publishes the folder to `zyrre/omarchy-corgi`, which
  `omarchy plugin add` installs from.

## Behaviour in detail

What the README leaves out, as the dashboard shows and does it.

### Rows and their order

Each agent takes three rows. The identity row holds its state, the local time
of its session's newest record (`HH:MM DD Mon`, absent for a harness whose
transcript Corgi does not read), the task its CLI summarizes, model and
effort, context used, the prompt cache, and its worktree checkout; a narrow
pane drops the fields after the task from the right before it clips the task,
and keeps the state and the time. The message row is the newest thing said, with the live
terminal filling in what the session file lacks, such as a permission prompt.
The tool row is the command or tool running or last run, with its whole
argument. Their marker says what they show:

| Marker | Row shows |
| --- | --- |
| `»` | Your prompt |
| `›` | The agent's reply |
| `…` | Its thinking, or a progress notice while it works |
| `?` | A question it is waiting on |
| `$` | A shell command |
| `●` | Any other tool, such as a file read or edit |
| `○` | Nothing yet |

The prompt-cache countdown (`⏱ 52m cache`) turns yellow in its last five
minutes and red in its last minute; once cold it reads, for example,
`⚠ 182k re-read`. A Codex estimate reads `≈ 18m cache`, then
`⚠ cache may be cold`.
`corgi fleet` shows the same reading in plain words in its CACHE column, for
a supervisor weighing a follow-up against a fresh worker: `warm 52m`,
`cold 182k`, `warm? 18m` and `cold? 182k` for a Codex estimate, `-` when
unknown. Both format through `cache_countdown` and `compact_tokens` in
`crates/corgi-core/src/model.rs`.

Every project with an agent session gets a heading, its supervisor first. Within
a state, agents Herdr reports no order between are alphabetical. Scratch
sessions are ordered the same way under Scratch. A row moves only when its own
agent changes state, and the selection stays on the same row. Agents in
linked worktrees are named `repo/checkout` (`webshop/order-pages-2d5a`);
other agents in the project's workspace, such as shared-checkout tabs, are
named after the repository.

### Project cards

A project whose supervisor runs is a card with the project as its title. The
supervisor stands apart from its workers in the same color as the pixel CORGI
wordmark at the top of the dashboard, so it follows the active theme the
way the wordmark does: the card's title, the supervisor's task, which is its
name, and a `▌` rail in the last column of the gutter on every supervisor row,
which runs on down the supervisor's turns when its session is expanded. Selecting
the supervisor's row turns its task bold, the same cue a worker's row gets. The
selection bar keeps its columns in front of the rail. The
dashboard opens every card collapsed: the supervisor's identity row, its message
and tool rows wrapped to two rows each and then cut with `…`, a divider, and
a footer such as `3 workers   ▲ 1 blocked   ● 1 working   ✓ 1 done` (only the
states present, in the list's state order, with workers tagged ready to merge
counted apart as `⇡ 1 merge` right after the blocked) with `→ expand` at the
right.
Each blocked worker gets a footer line of its own, `▲ blocked: <task> —
<what it waits on>`: the question on its screen, or `waiting for your
input`. A collapsed card is one stop for the selection and acts as its
supervisor for every key. `→` expands it: the footer goes, and each worker's
three rows follow the supervisor inside the card, which closes with
`← collapse`. `←` collapses it again from any of its rows. The choice is
kept per project heading in `expanded-projects` in the state directory.
Projects without a supervisor and scratch sessions keep plain rows under a
heading.

### Ready to merge

When a supervisor recommends the user merge a worker, after its clean-merge
check, it runs `corgi tag NAME merge --project <root>`. The command finds
NAME (a Herdr name or pane ID) among the project's agents and refuses one
that is not done or idle, or whose checkout (its linked worktree, else its
cwd) has no commits ahead of the branch checked out in the primary checkout,
the branch `m` merges into. It also refuses while the supervisor still has
questions open about the agent: when the newest line of the supervisor's
`ledger.jsonl` (newest per `id`, as the digest reads it) whose `agent` is
NAME has status `needs-answer` or a non-empty `questions` array
(`digest::open_questions`), it names those questions. No ledger, no entry
for NAME (an agent the user started), or a ledger it cannot read lets the
tag through; `--clear` is never refused. It sets the pane token `corgi_merge` to
`<state> <state_change_seq>`, the agent's state and Herdr's counter when it
was tagged, through `markers::tag_pane`, so the tag survives dashboard and
Herdr restarts like Corgi's other marks. `corgi tag NAME --clear` removes
it, and `corgi fleet` ends each row with a TAG column, `merge` or `-`.

A tag applies (`supervisor::merge_tag`) while its agent rests where it was
tagged: done or idle at the same state change, or idle at any after being
tagged done, which is how Herdr shows a done agent the user has looked at.
Working or blocked, done at another state change, or idle at another one
after being tagged idle, the agent has worked since, and the tag is stale:
every dashboard ignores it at once, and the next pass holding the wake lock
(a hook's or a dashboard's) clears it from Herdr and the record, also when the work
happened while no dashboard ran. The one case it cannot tell is a worker
tagged done that worked, finished and was looked at while no dashboard ran;
it still shows MERGE until it works again. A merge with `m` clears the
merged worker's tag too, also when only its push failed, since the branch
is merged by then.

A merge with `m` that lands locally also tells the project's supervisor
(`App::tell_supervisor_about_merge`): one inbox item of kind `merge`, keyed
`<agent> merged`, naming the agent, its task, branch, base branch and the
short hash of the base's HEAD after the merge, which the merge job reads
before pushing. When the push then fails, the line ends with Git's error
(`MergeLanded::push_error`) instead of leaving the supervisor unaware of the
merge. It goes in whether or not a supervisor runs
now, to be delivered when one does, but only for a project with a running
supervisor or a `ROLE.md` in its state directory (`Inboxes::has_supervisor`),
so a project that never had one gets no inbox. A failed, conflicted or
cancelled merge sends nothing, nor does a merge stopped before merging by
the upstream check, and neither does the Omarchy popup, which has no inbox.
A push that failed and is retried with Enter merges nothing new and sends
the line again, which replaces the failed push's line if that has not gone
out yet. The line itself tells the supervisor what to do, since the role
leaves it out: mark a worker in its ledger `merged`, stay quiet about an
agent it did not dispatch, re-check the other workers tagged MERGE against
the moved base, and, after a failed push, explain the error to the user
without pulling or pushing itself. The conflict line likewise carries how to
hand the conflict to the worker.

A worker whose tag applies shows a magenta ` MERGE ` badge instead of
` DONE ` or ` IDLE `; a busy one shows its real state. Magenta is the one
semantic color no state the user acts on uses (an unknown state shares it)
and is far from working's cyan in both Tokyo Night themes; its mark is `⇡`.
A collapsed card counts these workers apart, and the header's HERD card
reads WORKING, BLOCKED, MERGE, DONE and IDLE, the total making way for MERGE
so the card keeps its height. `m` still acts only on a selected worker.

### The expanded session

The newest turn, usually the closing summary, is shown in full and older
turns two rows each; the heading and the sessions above keep their place. A
session in a card grows inside it, and the card closes under its turns.
While expanded, `j` / `k` move to the next session and show it from its
newest turn, and every other key keeps working, so an agent can be prompted,
merged or closed without collapsing first. A session too far down for its
transcript to be worth reading pulls the sessions above it off the top. A
session with no readable file keeps its two rows and says why.

### The new-agent form

`n` presets the selected agent's repository, or the project used most
recently when no agent is selected, and a session name from the project
directory. The rows are Harness, Model, Effort (Codex and Claude Code only),
Project and Checkout.

| Key | In the form |
| --- | --- |
| `↑` / `↓`, `Tab` / `Shift+Tab` | Move between the task and the settings (`↓` on the task's last line steps into them; `Tab` wraps) |
| `←` / `→` | Next harness, model, effort level, or the other checkout |
| `Space`, or typing | Open the setting's list, filtered by what is typed |
| `Enter` | Create the agent |
| `Esc` | Cancel |

In an open list, typing filters, `↑` / `↓` move, `Enter` chooses, `Tab`
chooses and moves on, and `Esc` closes it unchanged. The harness and model
lists also offer the typed text as a value of its own, so an agent kind or
model Corgi does not know can still be used. When `Enter` cannot create the
agent yet, such as with no task or a missing project directory, the form
stays open, says why, and moves to the row that needs a value.

- **Harness** lists `codex`, `claude`, `gemini`, `copilot` and `opencode`,
  marking the CLIs found; any other kind Herdr supports can be typed.
  Switching harness clears a model the new one cannot run.
- **Model** starts on "Harness default", naming in parentheses what the CLI's
  configuration chooses where Corgi reads it (Claude Code's
  `settings.local.json` is read before `settings.json`). Choosing a model
  updates the effort hint, since effort can be set per model.
- **Effort** offers `low`, `medium`, `high`, `xhigh` and `max`.
- **The first agent of a project**, new or with no agent session in any of
  its workspaces, is its supervisor, with the task as its first request; the
  form says so in its title and above the keys. A supervisor runs only on Claude
  Code or Codex, so any other harness makes the form ask for one of them.

### Projects

The Project row's list is ordered by `crates/corgi-core/src/projects.rs`, and
opens by itself when no agent is selected and Corgi knows no project. Herdr
keeps a repository's primary workspace open while any of its worktree workspaces
exist, which is why an open workspace counts as a project. While an absolute
path is typed, matching directories are offered as completions, and a
complete directory path can be chosen even if Corgi has never seen it.

A new project's name goes on the `new project` row (`↑` wraps straight to it
when other projects match); a full path, `~` included, puts it elsewhere, and
with no known project it goes in the home directory. Its Checkout row is
hidden, since there is nothing to branch from yet. Corgi remembers a project
when it starts an agent there or sees it open in Herdr. The `projects` file
can be edited or deleted freely; missing directories are skipped, and
dropped the next time it is written. Directories inside Corgi's state
directory, such as a supervisor's, are never projects and never kept in it. To make a
repository known without running anything in it, open a workspace for it:
`herdr workspace create --cwd ~/repos/webshop --no-focus`.

### Worktrees, scratch sessions and closing

`n` on an agent already in a worktree creates a sibling worktree of the same
repository. Herdr picks a fresh `worktree/<name>` branch from `HEAD` and
checks it out under its `worktrees.directory`
(`~/.herdr/worktrees/<repo>/<branch-slug>` by default). Every worktree is
grouped below the project's `<project> supervisor` workspace, so closing or
restarting the dashboard cannot close workers. A label a user chose for that
workspace is kept; only the bare repository name an older Corgi used is
relabelled. With the Checkout row on "Project directory as is", the agent
gets its own tab in that workspace, and closing it leaves the root tab and
other agents alone. A directory outside a Git work tree always uses the plain
directory, and the status line says so.

`t` opens the form preset for `~`; a scratch session gets a plain `scratch`
workspace, never a worktree or a supervisor.

`x` on a worktree agent runs Herdr's `worktree remove`: it stops the panes,
closes the workspace, and deletes the checkout. A checkout with uncommitted
changes is refused once, then removed by force if asked again. The branch is
never deleted, so committed work survives; prune stale `worktree/*` branches
with `git branch -D`.

### Merging

The confirmation shows the task, the worktree, the source branch, the branch
checked out in the primary checkout, and the commits it brings in. Both
checkouts are checked clean before it asks and again after `Enter`, and then
`git merge --no-ff --no-edit` and `git push` run while the popup shows a
spinner and each step. An error leaves the popup open with `Enter` to retry.
Corgi never switches branches and never removes the worktree by itself: the
finished popup offers `x` to close it, or keeps it open.

A push the remote rejects because its branch moved on would leave the
merge done locally and the user to sort it out, so when the target branch
has an upstream (`git::git_upstream`, kept in `MergeWorktreeForm::upstream`)
the job first fetches it (`git fetch <remote> <merge ref>` from the
branch's config, with `GIT_TERMINAL_PROMPT=0` so no credential prompt lands
on the dashboard's terminal; an upstream in the same repository is not
fetched). A target branch without one merges as before, with no fetch step.
When the upstream has commits the target lacks, the job stops before
merging (`MergeError::Behind`), and the popup lists them, five at most,
with `Enter` to pull and `Esc` to stop. The pull runs as the fetch step of a
new job (`Upstream::Pull`): `git merge --ff-only <upstream>` when the target
has no commits of its own, else `git merge --no-edit <upstream>`, then the
worker's merge and the push as usual. A pull that conflicts is aborted,
the checkout checked clean, and the error tells the user to pull by hand.
A fetch that fails (offline, auth) stops the same way (`FetchFailed`), and
`Enter` merges without it (`Upstream::Skip`). Every retry checks the
upstream again.

A push that fails after the merge (`MergeError::PushFailed`, carrying the
merge commit and Git's error without its hints) clears the worker's merge
tag and sends the supervisor the merged line with the error, as above. The
popup shows the error, and offers `s` on the same terms as the conflict
handoff: it adds a second item of kind `merge` under the same `<agent>
merged` key (`supervisor::push_failed_message`), asking the supervisor to
tell the user what the error means and what to run, and standing in for
the merged line if that has not been delivered.

A merge that stops on conflicts leaves the primary checkout mid-merge, and
the popup lists the conflicted files (`git diff --name-only
--diff-filter=U`; the job's `MergeError::Conflicts`). Esc leaves them there
to resolve by hand. When the project has a running supervisor and this
dashboard is the one that wakes supervisors, `s` instead runs `git merge
--abort`, checks the checkout clean again (a failure stays in the popup as
an error), and queues one `[Corgi]` line for the supervisor on `SupervisorWaker`
(`supervisor::merge_conflict_message`) naming the worker, its task, branch and
worktree, the base branch and the files. It goes out with the wakes, between
the supervisor's turns, and the supervisor has the worker merge the base branch
into its own branch and resolve the conflict there; the user then merges
again. Without a supervisor the popup says so; a dashboard that does not wake
supervisors, and the Omarchy popup, do not offer the key, since only the waker
can hold the message for a supervisor mid-turn.

### `corgi spawn` and `corgi supervisor`

`corgi spawn` runs inside Herdr, on the session's injected socket, and reads
the task from stdin or `--task-file`, so a long brief needs no quoting:

```bash
corgi spawn --project ~/repos/webshop --harness claude --model opus --effort high <<'EOF'
Add a --json flag to ...
EOF
```

Every option defaults to the form's preset: the current directory, the
default harness with its own model and effort, and a fresh worktree
(`--checkout directory` for the shared checkout). `--name` picks an agent
name not already in use. Progress goes to stderr; on success stdout is one
JSON object with the agent's `name`, `pane_id`, `workspace_id`, `tab_id`,
`cwd` and `location`, ready for `herdr agent prompt`, `wait` and `read`.

`--request-id ID` makes a retried spawn safe. A supervisor passes its brief
id. The id is 1 to 80 (Herdr's metadata value limit) ASCII letters, digits,
`.`, `_` or `-`, kept verbatim as the worker pane's `corgi_request` token,
which the launch sets as soon as the pane exists and before the agent
starts, through `markers.rs` so that it survives a Herdr restart. Before
anything is created, a spawn with an id takes the project's spawn lock
(`markers/<socket>.spawn-<hash>.lock`, keyed by the project root's digest)
and holds it until its launch ends; then it looks for a running agent of
the project carrying the id. If there is one, it prints that agent as a
spawn would, with `"existing": true` and its state in `location`, and
starts nothing: no worktree, no pane, and `--name` is not checked against
the running agents. A retry made while the first spawn is still creating
the checkout or waiting for the first prompt to be accepted thus waits on
the lock and then finds that worker; one made after the first spawn exited
(also after it failed or timed out) finds the worker if it is running. An
agent that was closed does not block a retry, and a pane left behind with
the token but no agent in it does not count. A spawn without an id takes
no lock and checks nothing.

`corgi supervisor <project>` starts the supervisor of the project, which greets
with the project's state; with a request on stdin it takes that up instead,
and `--harness codex` starts it on Codex. The supervisor lists its workers with
`corgi fleet` and reads each final report with `corgi report`. For a
running agent, that prints the report the dashboard kept in its project's
inbox when the agent last stopped, if the item is of the agent's pane and
its current `state_change_seq`; otherwise (another agent of the same name,
or one that has said more since) the agent's newest message, or its
screen. For an agent whose pane is gone, it prints the newest report kept
for that name in any supervisor's inbox.
`corgi fleet`, `corgi digest` and `corgi report` end with one line on
stderr, `N undelivered inbox items: run corgi inbox <project>`, when the
supervisor of their project (for `report`, the current directory's) has missed
items: any undelivered while nothing delivers (no Corgi hook registered with
Herdr, no dashboard open, and no older dashboard holding the wake lock),
else only those undelivered for five minutes, and above that a line when some
are held because Herdr never confirmed their delivery (see "Herdr's hooks"), so
a supervisor notices a wake that did not reach it at its next command, but is
not drawn into the wakes that only wait for its turn to end.

A supervisor starts its session with `corgi digest <project>` instead of
reading its state files raw, since `decisions.md` and `ledger.jsonl` only
grow. The digest only reads, and prints, in order: the project, its base
branch and short HEAD; `handover.md` whole if it exists, else the "Open
threads" section of the newest note in `handovers/`, labelled with its date
as the previous session's; every ledger id whose
newest line is not `merged`, `abandoned` or `done` (finished work with
nothing to merge), joined by agent name with the running agents (a missing
one shows `**NOT RUNNING**`); the last 10 finished ids; the newest
decisions in full, whole entries up to 8 KB but at least 3; the titles of
the rest, newest first up to 3 KB, and a line counting any beyond; and a
footer with the number of superseded entries hidden, how to read one in full,
how to search, and warnings, among them one per ledger line that is not a
JSON object with an `id` (`file:line` and why), which every reader skips. A
decision entry's `Supersedes: <heading>` line hides the
earlier entry with exactly that heading (after `## `); one that matches none
is warned about. `corgi digest --decision "<words>"` prints every entry,
superseded or not, whose heading contains all the words; when no heading
does, those whose text has them all (as `--search` matches a word); when
none does, it says so and runs `--search` instead, since a question number
is often only in a handover note.

Open work is bounded like the decisions: a `reported` entry whose agent is
not running and whose newest line is two days old is only named, on one line
with the others like it, since a supervisor that forgot to mark finished
work `done` once made the digest 49 KB; the rest are shown newest first up to
4 KB, an entry of a running agent or one `needs-answer` or `blocked` always,
and one line names those left out. Summaries and outcomes are cut to 200
characters there and in the finished list; the ledger keeps them whole.

The ledger is append-only for the supervisor, so most of its lines are
superseded. `supervisor::prepare`, which runs before a session starts and
so while none of the project runs, compacts it: every line that a later line
of the same id supersedes moves, in order, to the end of
`ledger-archive.jsonl` beside it (`digest::compact_ledger`). The rest stays
where it was, unreadable and blank lines too, so the newest lines keep their
order and the digest, search and `corgi tag` read the same entries before
and after. The archive is written first and each file replaced through a
temporary file and a rename, so a stop halfway loses no line (at worst some
archive twice); a ledger with nothing superseded is not touched, and a
failure is a launch warning, not an error. Lookups and search read
`ledger-archive.jsonl` before `ledger.jsonl`, and compaction appends
superseded lines in their original order, so each id's history reads oldest
first.

`corgi digest --search "<words>"` (`crates/corgi-core/src/digest/search.rs`)
looks in all of the memory, since a test showed a supervisor answering "don't
know" to every question about older history with the digest alone: decision
entries (superseded ones marked); each ledger id, all its lines in
`ledger-archive.jsonl` and `ledger.jsonl` searched together and placed at
its newest; and the paragraphs and list items of briefs, `reports/`,
`notes/` (any UTF-8 file up to 1 MB, links not followed), `handover.md` and
archived handover notes. A word hits a block that has a word starting with
it, ignoring case. Blocks with the words as a phrase (in a row, only
punctuation between) come first, then those with the most words, then
those with the most on one line, then the newest; a file's hits past its
first two go behind every other file's. A block whose words (numbers made
alike) are at least four fifths in a better hit's, and which is at least a
third its size, folds into it and is named under it: an investigation of
63 supervisor transcripts found daily briefs repeating a paragraph filling
the top hits. Decisions and ledger ids never fold. At most six hits are
shown, each as a `file:line` handle, its date (a handover note's in full,
since a stale note misled the test's answers) and up to ten lines or
1.2 KB of the block around its best line, lines cut at 500 bytes; the
output stops near 7 KB with a line counting the hits left out.

`--show <file:line>`, `--ledger <id or agent>` and `--brief <id>`
(`crates/corgi-core/src/digest/lookup.rs`) print one item whole, since the same
investigation found a third of search hits followed by a raw read of the
file: the decision entry, the ledger line with its id's history, a brief
or report whole, or the section of any other Markdown file under its
nearest heading. A handle must stay inside the state directory and not
name the inbox. No index: the files are small enough
to read on each search. Like `fleet`, it
needs Herdr to resolve the project and its agents. `corgi` alone, or with
`--compact`, opens the dashboard; any other word it does not know prints the
usage to stderr and exits 2, so a mistyped command, or one this binary lacks,
says so instead of opening the dashboard (0.1.0 opened it for any word, so
`corgi setup` on that release silently did nothing). The old names of `corgi
supervisor`, `corgi start`, `corgi handler` and `corgi steward`, were dropped
in 0.2.0 and are unknown commands now.

### Plan usage

A card shows the plan, the reading's age, and the 5-hour and weekly usage
with the time until each resets; a percentage turns yellow from 60% and red
from 85%. Codex needs a signed-in `codex`. Claude Code needs a signed-in
`claude` and `curl`: Corgi reads the token from `~/.claude/.credentials.json`
or the macOS Keychain entry "Claude Code-credentials". This reuse of Claude
Code's login is unofficial and read-only, and the endpoint is undocumented. A
CLI with no reading at all gets no card, and the header's top right says why,
such as an expired login. To check the Claude reader by hand:
`cargo test -- --ignored live_claude_usage --nocapture`.

### Configuration

| Variable | Effect |
| --- | --- |
| `CORGI_DEFAULT_AGENT` | The harness the form presets, instead of the first of `codex` or `claude` found |
| `CORGI_CODEX_BIN`, `CORGI_CLAUDE_BIN` | Where that CLI is, when it is outside `PATH` and the usual install directories (`~/.local/bin`, `/opt/homebrew/bin`, npm, bun, nvm, cargo, volta) |
| `CORGI_PROJECTS_FILE` | Where the remembered projects are kept |
| `CORGI_THEME` | `catppuccin-mocha` or `catppuccin-latte` draws in Catppuccin's own colors; `terminal` skips the Omarchy theme and asks the terminal for its palette |
| `CORGI_BIN` | The Corgi binary a supervisor's commands call, such as a development build (`CORGI_HANDLER_BIN` and `CORGI_STEWARD_BIN`, its names before the renames, are still read) |
| `XDG_STATE_HOME` | Where Corgi keeps its state (`~/.local/state` by default) |

### Data and privacy

Prompt text goes over the Herdr socket, never in process arguments. Terminal
text is rendered as plain text after bounded cleanup and redaction of common
tokens. The Claude Code token is sent to `api.anthropic.com` only, passed to
`curl` on stdin rather than as an argument, and never stored.

### Installing

`scripts/install.sh`, the plugin's build step, downloads the release binary
matching `version` in `herdr-plugin.toml` for macOS or Linux on arm64 or
x86_64, checks it against the release's `SHA256SUMS` and its `--version`, and
puts it at `target/release/corgi`. It runs `cargo build --release --locked`
instead when the checkout has uncommitted changes to tracked files, there is
no binary for the platform, or the download or either check fails.

It then runs `scripts/link-command.sh`, which puts `corgi` on PATH. Herdr
builds in `<plugins>/.tmp-install-*/checkout` and moves the checkout to
`<plugins>/github/<id>-<first 12 hex digits of sha256(id)>` once the build
passes, so the script links `~/.local/bin/corgi` (or `$CORGI_BIN_DIR`, else
`$XDG_BIN_HOME`) to the binary's absolute final path there. Outside such a
build it does nothing. It replaces a link into the plugin folder or a broken
link, leaves a file, a folder or a working link elsewhere alone, and says
when the folder is not on PATH; `CORGI_LINK=0` turns it off. It only warns,
never fails the install. Herdr has no uninstall hook, so
`herdr plugin uninstall` leaves the link behind. Herdr
refuses to install over a linked plugin, so going back from a link needs
`herdr plugin unlink io.github.zyrre.corgi` first.

## Session and failure boundaries

Corgi operates only on the session represented by its injected socket. Losing
the socket marks the UI offline; reconnecting needs no recovery protocol because
the next snapshot is authoritative. Activity text is cached only in memory so a
brief read failure does not make every row blank.

A freshly created pane answers `agent.start` with `agent_pane_busy` until its
shell is available, and a just-started agent answers `agent.prompt` with
`agent_not_ready`; Corgi retries both for a few seconds rather than failing.
The first prompt waits for an observed state change, then Corgi checks the
agent screen or its session record for that task. Startup can acknowledge a
prompt before the new agent actually receives it; Corgi retries once when the
task is absent and reports an error if delivery still cannot be confirmed.

Creating an agent is a multi-step Herdr transaction without rollback. If a
later step fails, Corgi reports the exact failed stage and leaves the created
workspace (and its worktree checkout) visible so the user can inspect, reuse,
or remove it. Only the `not_git_worktree` error falls back to a plain
workspace; every other worktree failure is surfaced.

When Corgi closes one of its workers, it takes a fresh snapshot before
retiring that repository's agentless project parent. It keeps the parent if an
agent, a linked worktree workspace, or an extra tab remains; it also keeps the
parent when it sees a workspace it did not create. That conservative rule
means Corgi never closes a project resource it cannot prove is idle and owned.

### Confirmed conversation recovery

`S` opens `app/recovery.rs`'s cancel-first Sessions flow. Save writes immutable
versioned metadata in `recovery/` under the state home; `recovery/store.rs`
serializes mutations, uses owner-only files and fsync/rename, and keeps a
monotonic save sequence. Native conversation stores and supervisor memory stay
separate: the manifest references their current durable state, never copies
transcript/task text or rewinds memory. `recovery/native.rs` verifies exact UUIDs
without the display reader's newest-by-cwd fallback and builds prompt-free
Codex and Claude resume arguments. Codex permission records replace the whole
previous policy in transcript order: current `thread_settings_applied` profiles
supersede older `turn_context` policies, including empty roots and network/temp
restrictions. Capture replaces launch-profile permissions as a unit. The adapter
reconstructs Codex's built-in `.git`/`.agents`/`.codex`/`.aws` read guards and
blocks policies it cannot express faithfully (custom read restrictions, deny
rules and external enforcement); explicit review cannot override that block.
Read-only supervisors retain role instructions and state read access without
`--add-dir`, which requests writes. Observed writable roots are never expanded
for supervisor memory; registered launch profiles retain their intended state
root. Unknown other settings require explicit review.

Herdr owns layout restoration. Recovery validates saved workspace/tab/pane IDs,
checkout and `pane.process_info`'s foreground shell, then calls `agent.start`
in the original pane. The normal path creates no placement. Only an explicitly
selected missing/occupied-destination alternative creates a tab or opens an
existing directory/worktree. A durable native-identity intent precedes each
mutation. Unconfirmed launches are reconciled, not repeated; a confirmed native
identity permits metadata-only retry. Sequence-dependent merge and handover
claims are not restored. See [the recovery contract](docs/session-recovery-design.md).

Lifecycle tests must preserve a live native reference at disposable-server
shutdown. Quitting the harness first clears that reference and exercises a
different identity path. Record manual hook/API identity reports as a fixture
boundary: a hook browser's trusted/active status does not prove idle resume
emitted an identity report. Check native argv, history and task counts as well.

Recovery acknowledgment hints accept either letter case. Manual setup also
accepts Space to toggle its single acknowledgment; Confirm explains a missing
acknowledgment in the fixed footer so scrolling cannot hide the feedback.
Toggling, navigation and repeat events never substitute for explicit confirmation.

Manual setup has its own explicit confirmation and config check/backup/reload;
startup, Save and browsing never edit Herdr configuration. Herdr 0.9.1 cannot
report its loaded config path or an auto-resume barrier. Setup therefore needs
user acknowledgment of the displayed path and a subsequent normal server start
before new recovery launches; Corgi never restarts it. Layout restore remains on.

A committed Save arms next-boot supervisor holds; the waker checks them before
its first observation/delivery/handover. Same-boot Save does not pause normal
coordination. Every retained prior-boot manifest contributes unreleased holds,
independently of the newest Save and before any observation. Continue records
per-manifest acknowledgments for all matching prior-boot identities in selected
projects, leaving same-boot saves armed for next boot. Delivery checks use the
same retained manifests, including pending inbox quarantines. Explicit restore
also holds before launch. Observation baselines
advance while held, so Continue does not replay stops. Continue is separately
confirmed. Delivery IDs are persisted before inbox sends; uncertain sends and
saved pre-reboot pending IDs remain quarantined, with inbox bytes intact.
Old coordinating binaries cannot enforce these holds: use the updated build
for all coordinating dashboards after activation. Holds do not stop native
hooks, pending work, direct input or coordination already in flight.

## Future extensions

The polling boundary can later be replaced with Herdr event subscriptions while
retaining periodic snapshots for recovery. The activity extractor is isolated
so agent-specific parsers can be added without changing transport or UI code.
