# Corgi architecture

Corgi does not run its own backend service. Herdr is the control plane and the
terminal multiplexer; each Corgi pane is a short-lived client of the Unix socket
that Herdr injects as `HERDR_SOCKET_PATH`.

## Runtime shape

```text
Herdr session
  ├─ coding-agent panes
  └─ Corgi plugin pane
       ├─ session.snapshot ── agent identity, state, project/task metadata
       ├─ agent.read ──────── visible terminal activity, and a Project handler's input box
       ├─ agent.prompt ────── prompt an existing agent, and wake a project's handler
       ├─ agent.focus ─────── focus an existing agent pane
       ├─ workspace.close ─── close a plain or unused Corgi project workspace
       ├─ tab.create/close ── open or close a shared-checkout agent's tab
       ├─ worktree.remove ─── delete a worktree checkout and close its workspace
       ├─ *.report_metadata ─ mark Corgi's project workspaces and the handler's pane,
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
  │                        written when a project is used or seen open
  ├─ created-projects ─── the projects Corgi created, whose folder trust it answers
  ├─ expanded-projects ── the projects whose dashboard card is expanded, one
  │                        heading per line; missing or unreadable is all collapsed
  ├─ wake/<socket>.lock ─ held by the one dashboard that wakes handlers
  ├─ markers/<socket>.json ─ every metadata token Corgi set in that Herdr session,
  │                        by pane and workspace, with the agent or checkout it
  │                        was set for; replaced whole through a temporary file
  │                        and a rename, under markers/<socket>.lock
  ├─ usage/<provider>.json ─ the last plan-usage reading every process shares,
  │                        when it was fetched, and the last fetch's error;
  │                        replaced whole through a temporary file and a rename
  ├─ usage/<provider>.lock ─ held by the one process fetching a new reading
  └─ handler/<project>/ ─ the handler's files, written by Corgi at launch and by
                          the handler: ROLE.md, launch.json, decisions.md, ledger.jsonl,
                          briefs/, handover.md while one is under way, handovers/

Agent session files (per refresh, only the records appended since are parsed)
  ├─ {$CLAUDE_CONFIG_DIR,~/.claude}/projects/<slug>/<session>.jsonl ── model, token usage, cache lifetime
  └─ ~/.codex/sessions/<y>/<m>/<d>/rollout-*.jsonl ── model, usage, window, cache-hit estimate

Plan usage (once a minute per machine, only for CLIs Corgi can find)
  ├─ codex app-server ────── account/rateLimits/read over JSON-RPC
  └─ curl api.anthropic.com/api/oauth/usage ── with Claude Code's stored token

Optional Omarchy widget
  ├─ corgi --bar-stream <socket> ── newline-delimited dashboard rows and usage
  ├─ corgi --bar-transcript/--bar-new-*/--bar-action ── expand, create, merge, close
  ├─ herdr agent focus ─────── reveal a selected agent
  └─ herdr plugin pane open/focus ── launch or reveal the full dashboard

Headless launch and the Project handler
  ├─ corgi spawn ──── the new-agent form's launch sequence, task on stdin, result as JSON
  ├─ corgi handler ── the same launch into the project's root tab, as its handler
  ├─ corgi fleet ──── a project's agents as tab-separated rows
  └─ corgi report ─── an agent's newest message, from its transcript
```

The dashboard refreshes from Herdr roughly once per second, except while a
launch runs, and redraws about every 80 ms, so spinners move and keys answer
between snapshots; while a popup opens, closes or plays an outcome it draws
at about 60 frames a second, and drops back to 80 ms once nothing moves (see
"Popup motion" below). Plan usage is read once a minute per machine rather than
per process (see `src/usage_cache.rs` below) and Codex thread names every few
seconds, each on a background thread. API calls open a fresh
newline-delimited JSON connection, have five-second I/O deadlines (ten for a
first prompt, which waits for the agent to react), and cap responses at
4 MiB. There is no daemon, database, HTTP listener, or state that needs to be
synchronized with Herdr.

## Components

### Reading Herdr and the agents

- `src/herdr.rs` is the typed Herdr socket adapter. All actions are expressed
  in Herdr's public API rather than by driving terminal input directly. A
  refusal from Herdr is a typed `HerdrError`. The refusals a flow reacts to,
  such as `agent_pane_busy` or `not_git_worktree`, are classified by
  `HerdrError` itself (`HerdrError::is_pane_not_ready`,
  `HerdrError::is_not_git_worktree`, …), by its code rather than its text.
- `src/model.rs` holds Herdr's agents and workspaces as Corgi deserializes
  them, and `DashboardAgent`, one row of the dashboard with everything
  derived for it.
- `src/harness.rs` is the one place that knows a harness. `Harness` parses
  Herdr's kind string exactly (anything unknown stays `Other` and round-trips
  unchanged; text a user typed is trimmed and lower-cased first) and owns its
  title, the CLI lookup (`CORGI_CODEX_BIN` or `CORGI_CLAUDE_BIN`, else `PATH`,
  else the usual per-user and package-manager directories, because Herdr runs
  plugin panes with a minimal `PATH`), the model, effort, folder-trust and
  Project handler arguments, the offline model lists and model discovery, the exit
  command, the session file Corgi reads, and the form's order. It also reads
  Claude Code's settings from `CLAUDE_CONFIG_DIR`, then `~/.claude`, for
  everything that needs them. The rest of Corgi parses a kind once, where it
  arrives from Herdr, the command line, or the form's text, and passes a
  `Harness` from there on; it is a string again only where it leaves Corgi,
  in `agent.start` and the JSON Corgi writes.
- `src/activity.rs` reduces a bounded terminal read to display lines. It
  recognizes the command on screen separately from questions, assistant
  messages, and progress; strips terminal chrome; redacts common credentials;
  and caps output length. It is the fallback for what the session file lacks,
  and the only source of the permission prompts a blocked CLI shows on screen.
- `src/session/` reads each agent's session file, which its CLI already
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
  without guessing; a pane whose identity Herdr never learned falls back to the
  newest session file of its working directory. Nothing has to be installed
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
- `src/usage.rs` reads plan usage per provider and fetches Codex's
  account-aware picker catalog and thread names through a short-lived
  app-server. Claude Code usage comes from its stored OAuth token and
  Anthropic's usage endpoint. All of these run off the UI thread and fail
  soft. Its `Provider` is the view of the two harnesses that have a plan to
  read.
- `src/usage_cache.rs` shares one plan-usage reading per provider between
  every dashboard and Omarchy bar stream on the machine, so reloads and extra
  dashboards do not multiply requests (Anthropic answers them with HTTP 429).
  Each process checks `usage/<provider>.json` every few seconds, a started
  one at once, and uses it while its last fetch is under a minute old. Once
  it is older, the process that takes `usage/<provider>.lock` without waiting
  re-checks the cache, fetches, and writes it; the others keep what they have
  until a later check. A failed fetch is recorded with its time, beside the
  last good reading, so nobody retries it before the minute is up and the
  card keeps showing the reading. The card's first row shows its age.
- `src/defaults.rs` reads what each harness's own configuration names, so
  the form can show it in parentheses behind "Harness default": Claude Code's
  `model` and `effortLevel` (including per-model `modelSettings`) from its
  settings files, and Codex's top-level `model` and `model_reasoning_effort`.
  It never guesses a CLI's built-in default; an unread configuration shows
  nothing. Each harness is read once per dashboard run.
- `src/projects.rs` is where a new agent can be started: the projects with
  agents, the repositories with any primary workspace open in Herdr, the
  directories beside those, and the ones Corgi remembers in a plain text file
  under `$XDG_STATE_HOME/corgi` (`CORGI_PROJECTS_FILE` overrides it), turned
  into rows for the form's Project list along with directory-path completion.
  A name or path that does not exist yet becomes a new-project row. A bare
  name goes under the parent directory most known projects share. The launch
  creates the directory, records it in `created-projects`
  (`CORGI_CREATED_PROJECTS_FILE` overrides it), and starts the project's
  handler in the new project workspace's root tab; the directory stays plain
  until the handler makes it a repository. A picked project first becomes the
  `cwd` of `worktree.list`, which Herdr resolves to the repository itself.
- `src/git.rs` runs the few plain Git commands Corgi needs: a checkout's
  branch, whether it is clean, the commits a merge would bring, and the main
  checkout that folder trust keys on.

### The dashboard: `src/app/`

`src/app/` owns the dashboard's state and every flow that changes Herdr. Its
submodules are split by flow, each adding methods to the one `App`.

- `mod.rs` is `App`: refresh, selection, the expanded transcript, the plan
  usage and Codex thread-name jobs, key dispatch, and the terminal loop.
  `App::headless` is the same app for the command-line and Omarchy paths,
  which own no dashboard pane. `sort_agents` orders each project's rows:
  Project handler, then state, then the most recent `state_change_seq` first. That
  counter is one sequence for the whole Herdr session, not per agent, so it
  compares across panes and the dashboard keeps no ordering state of its own.
- `cards.rs` is the project cards: which runs of the sorted list are a card
  (led by a handler, not scratch), `CardMemory`, the expanded projects saved
  in `expanded-projects`, and the selection stops, which skip the workers
  folded into a collapsed card. `→` and `←` expand and collapse the selected
  card; `←` from a worker row selects its handler. A refresh keeps the
  selection on its stop, its row of the list, rather than on an agent index.
- `rows.rs` derives what a row shows from Herdr's metadata and the session
  facts: the project and its heading (`repo/checkout` for a linked worktree),
  the task summary, and the status-line bridge fallbacks.
- `overlay.rs` is `Overlay`: nothing, or the one form or dialog open over the
  agent list, which takes the keys and is drawn on top. A launch or merge
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
  handler instead of a worker. It validates and builds a `LaunchPlan`, which
  `launch.rs` runs. The `t` key opens the same form for the home directory, a
  scratch agent, whose task may be left empty.
- `catalog.rs` holds the per-harness model catalogs behind the Model row, and
  the harness, model, and effort rows' choices. Codex uses its App Server;
  OpenCode uses `opencode models`; static entries are an offline fallback. A
  discovered catalog is cached and re-fetched in the background once it is
  over an hour old, so a dashboard left open picks up a harness's new models
  without a restart; the cached list stays visible until the refresh lands,
  and a failed refresh keeps it.
- `launch.rs` is the checkout/create/start/prompt sequence. It is one
  function behind the dashboard form, the Omarchy popup, `corgi spawn`, and a
  handler's handover, so there is a single way to launch an agent. A selected
  model reaches the CLI as `--model` in the `agent.start` arguments, and a
  selected effort level as a `-c model_reasoning_effort` override for Codex
  or `--effort` for Claude Code; defaults send neither. New agents default to
  a fresh Git worktree: `worktree.list` resolves the selected agent's
  directory to its repository root (Herdr refuses to create worktrees from a
  linked worktree). Corgi creates or reuses the project workspace for that
  root, then passes that explicit workspace ID to `worktree.create`; Herdr
  picks a unique branch and path. This intentionally keeps workers
  independent of the Corgi dashboard's own workspace. The shared-directory
  option creates a separate tab in the same project workspace, and a handler
  starts in its root tab, or in a new tab of that workspace when an agent
  already runs in the root tab or it was split (a worker sent there by
  `corgi spawn --checkout root` is refused instead). The dashboard's `h` key
  builds the same `handler_plan` as `corgi handler` for the selected agent's
  project, taking the harness, model and effort from the handler's
  `launch.json`, else the form's presets, with no task; when the project's
  handler already runs, the key selects and focuses it as Enter does.
  The home directory itself (compared canonically, so `~` and `$HOME/` match;
  never its subdirectories) is never a project: `starts_handler` and
  `handler_plan` refuse it, and whatever the checkout, an agent there gets a
  plain workspace labelled `scratch` (`scratch 2` and on when that is taken),
  marked with the agent-workspace role so closing it closes the workspace. It
  never creates or looks up a project workspace, and is not remembered as a
  project. Its row has `scratch` set: it is grouped under Scratch after every
  project, left out of the form's project list, and ignored by the handler
  waker and handover. A launch with no task, which only a scratch agent can
  have from the form, skips the first prompt and leaves an interactive
  session. From the dashboard the launch runs as a background
  job, also in `launch.rs`, whose progress replaces the form with a panel
  until the first prompt is confirmed. `progress.rs` is where a launch or
  merge reports its steps: the dashboard's job, stderr for the command line,
  or nowhere.
- `first_prompt.rs` delivers a new agent's first prompt, confirms it arrived,
  and waits out a question the agent opens on.
- `project_main.rs` is the Corgi-owned, agentless project workspace of each
  repository: found, created, relabelled `<project> handler`, and retired once
  unused. Corgi's project workspace is explicitly metadata-marked; it never
  claims an existing workspace based on a matching label. The mark names the
  project by a fixed-length FNV-1a digest of its root, because Herdr cuts long
  token values short; the root itself is written too when it fits, and a
  workspace marked before digests is still found by it.
  A handler launched beside agent sessions already running in its project
  (`handler_project_main`, used only from `launch.rs`'s `Checkout::ProjectRoot`
  path for a `Role::Handler`) may instead adopt Herdr's own root workspace for
  the repository: the lowest-numbered workspace in `workspace.list`'s sidebar
  order whose checkout is the repository's primary one. `HerdrClient::list_workspaces`
  exists because that order is `workspace.list`'s alone — a session snapshot's
  `workspaces` happen to come back the same way, but nothing promises it, so
  finding the order itself always asks `workspace.list`. Adoption applies only
  when that root is not already Corgi's and holds no agent in any pane;
  otherwise the launch falls back to today's lookup and creation. A handler for
  a new project, or for one with no other sessions yet, never adopts. Once
  adopted, an older Corgi project-main of the same repository is closed if
  it is now unused, and both it and the adopted workspace can satisfy the
  ordinary lookup afterward: that lookup takes the first match in session
  snapshot order, so the adopted root, earlier in that order than an
  older duplicate, is what later launches keep finding.
  A project can end up with two Corgi project workspaces when a launch ran
  while Herdr had lost Corgi's marks (see "Herdr restarts" below) and created
  a second one. `project_main_workspace` then prefers the one the project's
  handler runs in, in any tab, and otherwise takes the first in snapshot
  order; every lookup (launch, close, merge, cleanup) goes through it.
- `close.rs` closes an agent: its worktree and checkout, its tab in a project
  workspace, or its whole workspace, then retires the project workspace when
  nothing is left in it. Before that, any other Corgi project workspace of
  the same project that holds nothing — no agent in any pane, no dashboard,
  and only its recorded root tab with at most one pane — is closed as a
  duplicate, the way adopting Herdr's root workspace closes an idle older
  one. A duplicate with anything in it is left alone.
- `markers.rs` is Corgi's record of the metadata tokens it sets in Herdr,
  described under "Herdr restarts" below. Every Corgi token write goes
  through its `mark_pane` and `mark_workspace`; nothing else calls
  `pane.report_metadata` or `workspace.report_metadata`.
- `merge.rs` is the only Git command Corgi runs that writes: a confirmed
  merge of an agent's clean worktree branch into the branch already checked
  out in the repository's primary checkout, then a push through that branch's
  configured upstream. The agent never performs the merge, and Corgi never changes the
  user's checked-out branch.
- `waker.rs` is `HandlerWaker`, which wakes handlers and hands them over, as
  described below; `draft.rs` tells it whether a handler's input box holds
  the user's draft.
- `cli.rs` is `corgi spawn`, `corgi handler`, `corgi fleet`, and
  `corgi report`.
- `bar.rs` is the `--bar-*` endpoints of the Omarchy widget. Their JSON is
  that widget's contract, pinned by snapshot tests, and they drive the same
  forms, key handlers, and launch as the dashboard.

### Herdr restarts

A Herdr restart or live handoff (`herdr update --handoff`) restores panes and
workspaces under the same IDs, but without the metadata tokens clients set.
Herdr v0.9.0 builds restored panes with empty tokens, and a handoff carries
none. Everything Corgi recognises by its tokens would silently stop: the
Project handler would count as a worker (no wakes, no handovers, `corgi fleet` says
`worker`), project workspaces would not be found (the next launch creates a
second `<project> handler` workspace), and agent workspaces would lose their
role. So Corgi keeps its own record and puts the tokens back.

- Every token Corgi sets is reported to Herdr and then written to
  `$XDG_STATE_HOME/corgi/markers/<socket>.json`, keyed by the Herdr socket
  like the wake lock: the handler marker, handover and baseline tokens by
  pane ID, with the Herdr name of the agent they were set for and its
  native session (Herdr's `agent_session`) once
  known; the project-main and agent-workspace marks by workspace ID, with
  the checkout Herdr reports for the workspace. Each change takes
  `markers/<socket>.lock`, reads the file, and writes it back only if it
  changed, through a temporary file renamed over it, so a dashboard, a launch
  on a background thread and `corgi spawn` never lose each other's entries
  and a reader never sees half a file. A record that cannot be written only
  costs the restore; the Herdr mark is what counts.
- The dashboard holding the wake lock compares the record with each snapshot
  before anything reads it, so only one process reconciles. It costs one
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
  snapshot too, so that same refresh already shows the handler and wakes it,
  and a status line says how many marks came back.
- A handler launched and lost in a restart before any dashboard saw its
  session is not re-marked: without a recorded session there is nothing to
  confirm the pane still holds it. The dashboard records the session at the
  first refresh after the launch.
- Herdr's plugin-pane records are Herdr's own and not part of this; the
  `open` action copes with losing them (`scripts/herdr-corgi-dashboard`).

### The Project handler

- `src/handler.rs` holds what makes a session a project's handler: the role
  from `handler/ROLE.md`, embedded in the binary and written into the
  project's handler directory with the path of the Corgi binary that launches
  it; that directory's files; the first prompt; and the pure state machines
  behind waking and handover. A handler runs on Claude Code, the default, or
  Codex, which `Harness` gives the role arguments of.
  Claude Code appends the role file to its system prompt.
  Codex takes the role inline as `-c developer_instructions`, which adds to
  its own instructions (`model_instructions_file` would replace them), and
  runs the handler in its workspace-write sandbox with network access, which
  is what lets `herdr` and `corgi` reach Herdr's Unix socket, and the state
  directory as an extra writable root. The launch marks the handler's pane with a
  `corgi_handler` token holding `session:<native agent_session value>`, so
  presentation renames preserve its role and later sessions in the pane do
  not inherit it. Before Herdr exposes the session, the launch name marks
  the pane before the first prompt; the prompt acknowledgement and dashboard
  reconciliation upgrade it once native identity is known. Legacy name
  markers match the live name, or migrate after a rename when the recorded
  session proves ownership. A status-line `session` token cannot establish
  ownership because it can survive pane reuse.
  Herdr does not keep them across its own restart; Corgi's record puts the
  marker back on the same session (see "Herdr restarts").
  The role names the installed plugin's release build (`plugin.list` gives
  its root) unless `CORGI_HANDLER_BIN` chooses another.
- The role was called the Steward until it was renamed Project handler, and
  what an older Corgi left behind keeps working. A pane marked with the old
  `corgi_steward` token is the handler's, and the next write of its marks
  moves the marker to `corgi_handler`, in Herdr and in Corgi's record. A
  project workspace still labelled `<project> steward` is relabelled, and
  `corgi steward` and `CORGI_STEWARD_BIN` are still accepted.
- The handler's memory moves with it, and nothing in it is overwritten,
  merged or deleted. Until the next handler session starts, a project whose
  state is still in `steward/<project>` is read there. The launch of that
  session (`handler::prepare`, at a fresh start and at a handover alike)
  moves the directory to `handler/<project>` with a single rename and
  leaves a link to it at the old path, so a session an older Corgi started
  and an older Corgi binary keep reading and writing the same files. The
  move waits for a launch, rather than the dashboard's first look, because
  a handover asks the old session to exit before its successor is
  prepared: no running session, whose harness may have resolved the old
  path for its sandbox or its allowed directories, has its files moved
  from under it. A link at the old path is never moved or removed: one to
  the new directory means the move is done, and one to a directory
  elsewhere gets a second link to the same directory at the new path.
  When both paths hold directories of their own, Corgi uses
  `handler/<project>` and warns, naming both, so the older history is not
  overlooked: in the launch's progress (stderr for `corgi handler`), on
  stderr for a handler's `corgi spawn`, and once per project in the
  dashboard's status line.
- `corgi spawn` recognizes a handler caller by its pane: `HERDR_PANE_ID`
  must be the pane of an agent carrying the handler marker. Without
  `--harness`, such a spawn starts the worker on the kind Herdr detected in
  the handler's pane, rather than on the form's default.
- The dashboard wakes handlers from its own refresh (`app/waker.rs`);
  nothing else runs for it. Every agent of a project with a running handler
  counts, except the handler, whoever started it. `handler::Transitions` reduces each agent's
  state from refresh to refresh to wakes: nothing until the agent has been
  seen working, so neither the states found at startup nor a question a new
  agent opens on is news; after that, each time it rests (done or idle,
  which count as one) after working, and each change between blocked and
  resting. The same resting state with a newer `state_change_seq` means a
  turn passed between two refreshes, and wakes too. A wake is one
  `[corgi]` line naming the agent, its state, and the command to run next;
  a handler's lines go out as one `agent.prompt` once it is neither working
  nor blocked, so they arrive after its turn, and a newer line for the same
  agent replaces an unsent one. They also wait while the handler's input box
  holds a draft, so they are never typed into what the user is writing.
  Just before sending, the dashboard reads the handler's visible screen with
  its styling and finds the box: for Claude Code, the line starting with `❯`
  directly below a horizontal rule, down to the next rule; for Codex, the
  last line starting with `›`, down to the next blank line. Any text in it
  that is not dim is a draft, a `[Pasted text #1]` marker included. Dim text
  counts as empty because it is the harness's own: Claude Code's prompt
  suggestions and Codex's placeholder, which look just like typed text once
  the escapes are stripped. Only the box's content counts, not focus or
  recent keystrokes. A screen that cannot be read, a box not on it, or a
  harness whose box Corgi does not know counts as no draft, so the lines
  go out as before rather than waiting for good. Every interactive
  dashboard (the tab, the popup, a preview) observes, but only the one
  holding an exclusive lock on `$XDG_STATE_HOME/corgi/wake/<socket>.lock`
  sends; the lock goes with its process, and the next dashboard to refresh
  takes it over, with a baseline already in hand. The Omarchy reader and
  the command-line tools never wake. Queued lines are kept per project
  rather than per handler, so the ones a handler that is handing over did
  not get go to the handler that takes over from it.
- The same dashboard hands a handler over to a fresh session once its
  context is half full, since compaction would lose what only its
  conversation holds (`handler::Handover`, once per harness session). A
  handler that is idle or done, never working or blocked, at 50% or more
  gets one `[corgi]` line asking it to write `handover.md` in its state
  directory and end its turn, held like a wake while its input box holds a
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
  the agent, and runs the usual handler launch in that same pane under the
  same name: the role is prepared again, the marker set again, and the
  first prompt has the new session do the start-of-session reading, then
  read the note and move it to `handovers/<YYYYMMDD-HHMMSS>.md`. It starts
  on the harness, model, effort, and extra arguments the launch recorded in
  `launch.json`, or the harness's defaults without one. A handler seen at
  work after its note was found (a turn the user sent from the draft that
  held the replacement) has a stale note: once that turn is over it gets
  another `[corgi]` line asking it to bring the note up to date and end its
  turn, held like any request while there is a draft, and it is replaced
  only once a note newer than that request exists. Wakes for the project
  wait from the request until the successor's first turn is over, and one
  queued while no handler was visible still reaches it. A turn that ends
  without a note (or without an updated one), or a request not taken up
  within two minutes, leaves the handler in place with a status line, and
  it is asked again only after a later turn of its own.
  `CORGI_DEBUG_HANDOVER_PERCENT` lowers the threshold for
  a debug run; it is not a setting.
- A second trigger hands over a handler whose prompt cache is about to go
  cold, since its next turn would otherwise re-read the whole conversation
  uncached, while a fresh session is small enough to wake cold cheaply. It
  fires, with its own `[corgi]` wording and status line and otherwise the
  same request, note check, exit, relaunch, and once-per-session rule, when
  the handler has rested (idle or done) for 50 minutes on Claude Code (one
  hour cache) or 25 on Codex (about thirty minutes), no other agent of its
  project is working or blocked (the dashboard cannot tell which of them
  the handler dispatched, so all count), and its context in tokens is at
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
  never qualifies, and a handler on a harness whose session file Corgi
  cannot read is never handed over for idleness. The 50% trigger goes first
  when both hold. `CORGI_DEBUG_HANDOVER_IDLE_SECS` sets the idle time for
  a debug run on any harness; it is not a setting.
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

- `src/ui/` renders the Ratatui dashboard. A project led by its Project handler is
  a card, drawn as list items whose lines carry the card's border: collapsed,
  one item of the handler's rows, a divider, and the sum of its workers;
  expanded, the handler's item with the card top, then one item per worker,
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
  screen, draws the footer, and holds the palette and dialog helpers;
  `header.rs` is the brand and the plan-usage cards; `agents.rs` the agent
  list and the expanded transcript; `status.rs` the fields of an agent's
  status line;
  `new_agent.rs` and `choice_list.rs` the new-agent form and its lists;
  `dialogs.rs` the close and merge dialogs; `prompt.rs` the prompt dialog;
  `effects.rs` and `blend.rs` the popups' motion, below.
  The usage cards, status fields, and dialog lines are also what
  `app/bar.rs` sends the Omarchy widget, so both show the same thing; the
  widget gets a dialog's keys as a line of words, since it draws no keycaps.
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
  failures never do. The stamp keeps its own 880 ms clock, apart from the
  popup's: the frame turns green at once, a big four-row `✓` is drawn column
  by column centred on the popup with two clear columns either side, holds,
  and fades while the frame eases back to its color. It is drawn last, over
  the popup (whose content fades almost out under it and back), over the
  frame shrinking away, and over the dashboard once the popup is gone, so a
  popup that closes on success does not wait for it and keys go on to the
  list at once; it is clipped at the screen's edge, never skipped. A popup
  opening while it plays cuts it off. The loop stays at its fast rate until
  it ends, and since every frame is drawn afresh, it leaves nothing behind.
  Colors beyond the theme's come from the fixed 256-color cube and grey ramp;
  `blend.rs` mixes them in RGB and snaps back to that palette, stands a theme
  color in by its xterm default while an effect runs, and ends every effect
  on the theme color itself. The spinner and the caret blink are pure
  functions of the clock, so a render at a set time is always the same
  frame. After each draw the loop asks `Motion::frame_interval` how long to
  wait for a key: 16 ms while anything moves, less the time the frame took to
  draw and write, since writing to the terminal takes a good part of that;
  otherwise the usual 80 ms after drawing, cut short only to land on the
  spinner's next frame or the caret's next turn, so an idle dashboard draws
  no more often than before. A snapshot refresh waits for motion to stop, at
  most half a second, so its round trips to Herdr never stall a frame. The caret is the real
  terminal cursor, shown and hidden on Corgi's clock; the dashboard asks the
  terminal for a steady bar cursor and gives back the user's shape on exit.
  A dashboard without an animated `Motion` (tests, the headless paths) shows
  every effect at its end.
- `src/choices.rs` is the pure model behind every selector list in the
  new-agent form: the rows, the word filter typed over them, and how typed
  text that names no row is offered (as a value of its own for harness and
  model, as a completed directory for the project, or not at all).
- `src/textfield.rs` wraps the form's task text and moves its caret along the
  rows the reader sees.
- `src/job.rs` is work handed to a background thread: its progress messages
  and its outcome, polled from the UI loop so the UI thread never waits, and
  a panic reported rather than lost.
- `src/time.rs` is the wall clock without a date crate: Unix seconds, RFC 3339
  timestamps as the agent CLIs write them, and the UTC stamps handover
  archives are named with.
- `src/paths.rs` is the home directory, Corgi's state directory under
  `$XDG_STATE_HOME` or `~/.local/state`, the `~` shorthand paths are typed
  and shown with, and `dir_name`, the last path component a project or
  checkout is known by.
- `src/test_support.rs` is compiled only for tests: scratch directories, a
  fake Herdr socket that answers scripted replies, a dashboard that never
  reads the developer's own state, and rendered screens read back as text.
- `herdr-plugin.toml` exposes a persistent tab and a compact popup.
- `omarchy/` is an optional compact QML dashboard. Its owned reader process
  reuses the Rust dashboard data pipeline and emits snapshots every two seconds.
  The socket is explicit, since the desktop shell has no injected Herdr session.
  Its popup also creates agents (`--bar-new-options`, `--bar-new-agent`) and
  merges or closes them (`--bar-action`) with the TUI's own forms and launch
  sequence; prompting an existing agent remains in the TUI.

## Behaviour in detail

What the README leaves out, as the dashboard shows and does it.

### Rows and their order

Each agent takes three rows. The identity row holds its state, the task its
CLI summarizes, model and effort, context used, the prompt cache, and its
worktree checkout; a narrow pane drops these fields from the right before it
clips the task. The message row is the newest thing said, with the live
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

Every project with an agent session gets a heading, its Project handler first. Within
a state, agents Herdr reports no order between are alphabetical. Scratch
sessions are ordered the same way under Scratch. A row moves only when its own
agent changes state, and the selection stays on the same row. Agents in
linked worktrees are named `repo/checkout` (`webshop/order-pages-2d5a`);
other agents in the project's workspace, such as shared-checkout tabs, are
named after the repository.

### Project cards

A project whose Project handler runs is a card with the project as its title. The
dashboard opens every card collapsed: the handler's identity row, its message
and tool rows wrapped to two rows each and then cut with `…`, a divider, and
a footer such as `3 workers   ▲ 1 blocked   ● 1 working   ✓ 1 done` (only the
states present, in the list's state order) with `→ expand` at the right.
Each blocked worker gets a footer line of its own, `▲ blocked: <task> —
<what it waits on>`: the question on its screen, or `waiting for your
input`. A collapsed card is one stop for the selection and acts as its
handler for every key. `→` expands it: the footer goes, and each worker's
three rows follow the handler inside the card, which closes with
`← collapse`. `←` collapses it again from any of its rows. The choice is
kept per project heading in `expanded-projects` in the state directory.
Projects without a handler and scratch sessions keep plain rows under a
heading.

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
  its workspaces, is its Project handler, with the task as its first request; the
  form says so in its title and above the keys. A handler runs only on Claude
  Code or Codex, so any other harness makes the form ask for one of them.

### Projects

The Project row's list is ordered by `src/projects.rs`, and opens by itself
when no agent is selected and Corgi knows no project. Herdr keeps a
repository's primary workspace open while any of its worktree workspaces
exist, which is why an open workspace counts as a project. While an absolute
path is typed, matching directories are offered as completions, and a
complete directory path can be chosen even if Corgi has never seen it.

A new project's name goes on the `new project` row (`↑` wraps straight to it
when other projects match); a full path, `~` included, puts it elsewhere, and
with no known project it goes in the home directory. Its Checkout row is
hidden, since there is nothing to branch from yet. Corgi remembers a project
when it starts an agent there or sees it open in Herdr. The `projects` file
can be edited or deleted freely; missing directories are skipped. To make a
repository known without running anything in it, open a workspace for it:
`herdr workspace create --cwd ~/repos/webshop --no-focus`.

### Worktrees, scratch sessions and closing

`n` on an agent already in a worktree creates a sibling worktree of the same
repository. Herdr picks a fresh `worktree/<name>` branch from `HEAD` and
checks it out under its `worktrees.directory`
(`~/.herdr/worktrees/<repo>/<branch-slug>` by default). Every worktree is
grouped below the project's `<project> handler` workspace, so closing or
restarting the dashboard cannot close workers. A label a user chose for that
workspace is kept; only the bare repository name an older Corgi used is
relabelled. With the Checkout row on "Project directory as is", the agent
gets its own tab in that workspace, and closing it leaves the root tab and
other agents alone. A directory outside a Git work tree always uses the plain
directory, and the status line says so.

`t` opens the form preset for `~`; a scratch session gets a plain `scratch`
workspace, never a worktree or a Project handler.

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
finished popup offers `x` to close it, or keeps it open. Conflicts are
resolved in the primary checkout.

### `corgi spawn` and `corgi handler`

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

`corgi handler <project>` starts the Project handler of the project, which greets
with the project's state; with a request on stdin it takes that up instead,
and `--harness codex` starts it on Codex. The handler lists its workers with
`corgi fleet` and reads each final report with `corgi report`. `corgi
steward`, the command's name before the rename, still works but is not
listed.

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
| `CORGI_HANDLER_BIN` | The Corgi binary a Project handler's commands call, such as a development build (`CORGI_STEWARD_BIN`, its name before the rename, is still read) |
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

## Future extensions

The polling boundary can later be replaced with Herdr event subscriptions while
retaining periodic snapshots for recovery. The activity extractor is isolated
so agent-specific parsers can be added without changing transport or UI code.
