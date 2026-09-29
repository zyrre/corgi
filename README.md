# Corgi

Corgi is a small, quick dashboard for the coding agents running in a Herdr
session. It lives inside Herdr as an interactive plugin pane and keeps the
agent's current terminal activity at the center of each row.

## What it does

- Works with any agent kind Herdr can start, including Codex and Claude Code.
- Lists the agents in the current Herdr session grouped by project, each row
  showing its state, the task the agent CLI is summarizing, the model and
  thinking effort that answer it, how full its context window is, and its
  worktree checkout.
- Counts down how long a Claude Code session's prompt cache stays warm, and
  once it has gone cold, warns with the size of the context the next turn will
  re-read at full price. The countdown turns yellow with a few minutes left
  and red in the last minute, so a session worth prompting before its cache
  lapses stands out. Claude Code records the cache lifetime of every request
  in its transcript. GPT-5.6 Codex sessions also show an `≈` countdown from
  the latest recorded cache hit or write: OpenAI guarantees that cache prefix
  for at least 30 minutes, but Corgi labels it as possibly cold afterwards
  because Codex does not report its exact lifetime.
- Shows under each agent the newest thing said in its session, whether that is
  your prompt, the agent's reply, or its thinking, and on the line below the
  command or tool it is running or last ran, in full. Both are read from the
  session file the agent CLI writes; the live terminal fills in what that
  lacks, such as a permission prompt waiting for you.
- Expands the selected session in place with `space`: its message row grows
  down to the bottom of the box into that session's latest turns, newest first,
  with what each side said and every command and tool call in between. Older
  turns are held to two rows each; the newest one, usually the agent's closing
  summary, is shown in full. The project heading and the sessions above keep
  their place, so opening and closing is the one row growing and shrinking.
- Sends prompts to idle, done, unknown, and working agents through Herdr's
  agent API.
- Gives every new agent its own Git worktree: creates or reuses an agentless
  Corgi project workspace, creates a fresh branch and checkout beneath it,
  starts a named agent in that workspace, and submits its first prompt.
- Shows the 5-hour and weekly plan usage for each installed CLI (Codex,
  Claude Code) in the header.
- Opens as either a persistent Herdr tab or a compact popup.

## Build and link locally

```bash
cargo build --release
herdr plugin link "$PWD"
herdr plugin pane open --plugin io.github.zyrre.corgi --entrypoint dashboard
```

To open or focus the dashboard with one key, bind the plugin's `open` action in
`~/.config/herdr/config.toml` and run `herdr server reload-config`:

```toml
[[keys.command]]
key = "prefix+d"
type = "plugin_action"
command = "io.github.zyrre.corgi.open"
description = "Corgi (open or focus)"
```

The compact Herdr popup is available as the `quick` entrypoint.

```bash
herdr plugin pane open --plugin io.github.zyrre.corgi --entrypoint quick
```

See [ARCHITECTURE.md](ARCHITECTURE.md) for the socket/API boundaries and why
Corgi does not need a separate backend.

## Dashboard keys

| Key | Action |
| --- | --- |
| `j` / `k`, arrows | Select an agent |
| `space` | Grow the selected session's message row into its transcript, or collapse it again |
| `u` / `d`, `PgUp` / `PgDn`, `Home` / `End` | Scroll the expanded transcript |
| `p` | Prompt the selected agent |
| `n` | Compose a prompt for a new agent in its own worktree; with no agent selected, the most recently used project is preset |
| `t` | Start a scratch agent in your home directory: the new-agent form preset for `~`, where the task may be left empty to type into the session yourself. It gets a plain `scratch` workspace, never a worktree or Steward, and is grouped under Scratch |
| `s` | Start the Steward of the selected agent's project, or focus it when one is already running |
| `m` | Merge and push an agent's worktree branch into the primary checkout's current branch (confirmation required) |
| `f` or `Enter` | Focus the selected agent pane |
| `x` | Close the selected agent; for a worktree agent this also deletes its checkout (confirmation required) |
| `r` | Refresh immediately |
| `Esc` | Collapse the expanded transcript, or close Corgi |
| `q` | Close Corgi |

While a session is expanded, `j` / `k` move to the next session and show it
from its newest turn. Every other key keeps working, so you can read the
transcript and then prompt, merge, or close that agent without collapsing it
first. A session sitting too far down the list for its transcript to be worth
reading pulls the sessions above it off the top to make room. Transcripts come
from the session files Claude Code and Codex write; a session with no readable
file keeps its two normal rows and says why there is nothing more.

The new-agent form opens in its task row with everything else preset: the
selected agent's repository (or the project you used most recently), the
default harness, that harness's own model and effort, a fresh worktree, and a
session name taken from the project. Type the task and press `Enter`.

For a new project, or a project with no agent session in any of its
workspaces, `Enter` starts the project's [Steward](#the-steward) instead of
a worker, with your task as its first request. The form says so in its title
and in a line above the keys. The Steward starts on the harness, model, and
effort chosen in the form, as a worker would, when that harness is Claude Code
or Codex; for any other harness the form asks you to pick one of those. Once
any agent works in the project, the form starts workers as usual.

Below the task sits one short list of settings: Harness, Model, Effort (Codex
and Claude Code), Project, and Checkout. The same keys work on every row:

| Key | In the form |
| --- | --- |
| `↑` / `↓`, `Tab` / `Shift+Tab` | Move between the task and the settings (`↓` on the task's last line steps into the settings; `Tab` wraps) |
| `←` / `→` | Change the setting in place: next harness, model, effort level, or the other checkout |
| `Space`, or just start typing | Open the setting's list, filtered by what you type |
| `Enter` | Create the agent |
| `Esc` | Cancel |

In an open list, typing filters, `↑` / `↓` move, `Enter` chooses, `Tab`
chooses and moves to the next setting, and `Esc` closes the list with the
value unchanged. The harness and model lists also offer whatever you typed as
a value of its own, so a model or agent kind Corgi does not know can still be
used. The session name is always taken from the project directory.

If `Enter` cannot create the agent yet (no task, or a project directory that
does not exist), the form stays open, says why, and moves the cursor to the
row that needs a value.

## Projects

The dashboard lists a heading for every project that has an agent session.
The agent Corgi launched as a project's Steward is marked on its pane, so it
is recognized wherever the pane is moved: it is shown as `Steward`, in place
of a task summary, and always comes first under its project heading. The
other agents follow by state, so what needs attention is on top: blocked,
working, done, idle, then those whose state is unknown. Agents in the same
state are listed with the one that entered it most recently first, then
alphabetically when Herdr reports no order between them. Scratch sessions are
ordered the same way. A row moves only when its own agent changes state; the
selection stays on the same row. Other agents in the project's
workspace, such as shared-checkout tabs or a worker in its root tab, are named
after the repository.

Starting an agent in any other project goes through the Project row of the
new-agent form: press `Space` or start typing on it to open its list. When no
agent is selected and Corgi knows no project at all, the form opens with that
list already open. It lists every project Corgi knows, in this order:

1. projects with running agents;
2. repositories with a workspace open in Herdr, agent or not (Herdr keeps a
   repository's primary workspace open while any of its worktree workspaces
   exist, so this is Herdr's own idea of a project);
3. sibling directories beside currently open projects (for example, other
   repositories in your `projects` directory);
4. projects Corgi remembers from before.

Type to filter by name or path; `↑` / `↓` move, `Enter` chooses, `Esc` goes
back. While typing an absolute path, matching directories are offered as
completion rows; choose one with `Enter`. A complete directory path is offered
too, so a project Corgi has never seen can be used without leaving the form.

To start a new project, type its name and choose the `new project` row (↑
wraps straight to it when other projects match). The new project goes in the
directory most known projects share, such as `~/repos`, or in your home
directory when Corgi knows no project yet. To put it somewhere else, type the
full path instead; `~` works too. Corgi creates the directory and starts the
project's Steward there with your task (see below); the Checkout row is
hidden, since there is nothing to branch a worktree from yet. The Steward
makes the directory a repository with a first commit, shaped by your task,
and can then start worktree agents.

Corgi remembers a project when it starts an agent there and when it sees the
repository open in Herdr. The list is a plain text file, one absolute path per
line, most recently used first, at `$XDG_STATE_HOME/corgi/projects`
(`~/.local/state/corgi/projects` by default; `CORGI_PROJECTS_FILE` overrides
the location). Edit or delete it freely. Directories that no longer exist are
skipped. To make a repository known without running anything in it, open a
workspace for it in Herdr:

```bash
herdr workspace create --cwd ~/repos/webshop --no-focus
```

The Model row starts on "Harness default", which leaves the choice to the
CLI's own configuration. Where Corgi can read that configuration it names the
value in parentheses, so the row says what you will actually get: Claude
Code's `model`, `effortLevel`, and per-model `modelSettings` overrides from
`~/.claude/settings.json` (and `settings.local.json` first, when present), and
Codex's top-level `model` and `model_reasoning_effort` from
`~/.codex/config.toml`. Choosing a model updates the effort hint, because the
level can be configured per model. A harness whose configuration Corgi does
not read simply shows "Harness default" with nothing in parentheses; Corgi
never guesses a CLI's built-in default. Corgi refreshes the Codex list from its locally
authenticated App Server and OpenCode from `opencode models`, without blocking
the UI; the result is cached while the dashboard runs and reaches a list that
is already open. Until a refresh succeeds (or when a harness provides no
catalog), Corgi falls back to its curated model list. Any other model its CLI
accepts can be typed into the list and chosen as typed; Corgi passes the
selection to Herdr as `--model`. The Effort row, shown for Codex and Claude
Code, offers `low`, `medium`, `high`, `xhigh`, and `max`; the default leaves
the CLI's own setting in charge. A chosen level reaches Codex as
`-c model_reasoning_effort=<level>` and Claude Code as `--effort <level>`.
Other harnesses have no effort control, so the row is left out for them.
Switching harness clears a model the new
one cannot run. The Harness list marks the CLIs found on this machine and
offers `codex`, `claude`, `gemini`, `copilot`, and `opencode`; any other kind
Herdr supports can be typed. `Esc` cancels either form.

By default each new agent gets its own Git worktree. Corgi asks Herdr for the
repository behind the project directory, so pressing `n` on an agent that is
already working in a worktree creates a sibling worktree of the same
repository rather than a worktree of a worktree. Herdr picks a fresh
`worktree/<name>` branch from `HEAD` and checks it out under its
`worktrees.directory` (`~/.herdr/worktrees/<repo>/<branch-slug>` by default).

On its first launch for a Git repository, Corgi also creates one agentless
project workspace, labelled `<project> steward` (for example `corgi steward`)
because its root tab is where the project's Steward can run, and marks it as
Corgi-managed in Herdr metadata. A project workspace that still has the bare
repository name from an older Corgi is relabelled the first time Corgi sees
it; a label you chose yourself is kept. Every Corgi-created worktree is explicitly
grouped below that workspace—not below the Corgi dashboard or an unrelated
existing workspace. Closing or restarting the dashboard therefore cannot
close those workers. Existing Herdr workspaces are never adopted merely
because they have the same name. Agents in linked worktrees are listed as
`repo/checkout`, for example `corgi/calm-otter-1f2e`.

Switch the checkout field to "Project directory as is" with `←` / `→` or
`Space` to start the agent in the shared directory instead. For Git projects,
the agent gets its own tab under the same Corgi project workspace; closing it
does not affect the root tab or other agents. Directories outside a Git work
tree always use the plain directory, and the status line says so.

Pressing `x` on a worktree agent runs Herdr's `worktree remove`: it stops the
panes, closes the workspace, and deletes the checkout from disk. If the checkout
still has uncommitted changes, Corgi refuses once and asks again before forcing
the removal. The branch is never deleted, so committed work survives; prune
stale `worktree/*` branches with `git branch -D` when you no longer need them.
After a Corgi agent is closed, Corgi checks its project workspace: it retires
that workspace only when no project agent, linked worktree workspace, or extra
tab remains. Any unowned workspace makes cleanup stop rather than risking
another user's session.

When you have reviewed a worktree agent's work, press `m` to merge it. The
confirmation shows the worktree name, the source branch, and the branch
currently checked out in the repository's primary checkout. Corgi checks that
both checkouts are clean, then runs `git merge --no-ff --no-edit` followed by
`git push` only after you press `Enter`. The popup stays open with a spinner
while those commands run, and reports any merge or push error. It never switches
branches or removes the worktree; resolve any reported conflicts in the primary
checkout, then use `x` separately when you are ready to remove the agent
workspace.

The preselected kind is the first of `codex` or `claude` that Corgi can find.
Set `CORGI_DEFAULT_AGENT` (for example `CORGI_DEFAULT_AGENT=claude`) to override
it.

Herdr runs plugin panes with a minimal `PATH`, so Corgi also looks in the usual
install directories (`~/.local/bin`, `/opt/homebrew/bin`, npm, bun, nvm, cargo,
volta). If a CLI lives somewhere unusual, point `CORGI_CODEX_BIN` or
`CORGI_CLAUDE_BIN` at it.

## Starting agents from another agent

`corgi spawn` starts an agent exactly as the new-agent form does, for callers
without a dashboard, such as a coordinating agent running in the project's
root tab. Run it from inside Herdr; it uses the session's injected socket.
The task is read from stdin (or `--task-file`), so a long brief needs no
shell quoting:

```bash
corgi spawn --project ~/repos/corgi --harness claude --model opus --effort high <<'EOF'
Add a --json flag to ...
EOF
```

Every option defaults to what the form would preset: the current directory,
the default harness, the harness's own model and effort, and a fresh worktree
(`--checkout directory` for the shared checkout). Run by a Steward, a spawn
without `--harness` starts the worker on the Steward's own harness instead, so
a Codex Steward's workers are Codex sessions and a Claude Code Steward's are
Claude Code sessions. `--name` picks the agent
name, which must not be in use. Progress goes to stderr; on success the
started agent is printed to stdout as one JSON object with its `name`,
`pane_id`, `workspace_id`, `tab_id`, `cwd`, and `location`, ready for
`herdr agent prompt`, `wait`, and `read`. Run `corgi spawn --help` for the
full list.

## The Steward

Each project can have one Steward: a long-lived Claude Code or Codex session
in the root tab of the project's Corgi workspace, which you talk to about what
to build and which dispatches the work to worktree agents. Corgi launches it
with its role, `steward/ROLE.md` in this repository, written into the
project's Steward directory. Claude Code gets the role appended to its system
prompt; Codex gets it as developer instructions (`-c developer_instructions`),
in its workspace-write sandbox with network access so that `herdr` and
`corgi` reach Herdr's socket, and with the Steward directory as an extra
writable root. Codex still asks you to approve anything else its sandbox
refuses, such as the `git init` of a new project.

```bash
corgi steward ~/repos/corgi                    # greets with the project's state
corgi steward ~/repos/corgi < first-request.md # takes that request up instead
corgi steward ~/repos/corgi --harness codex    # on Codex instead of Claude Code
```

In the dashboard, `s` does the same for the selected agent's project when no
Steward runs there, even while other sessions of the project do; when one
does, `s` focuses it instead, switching to its workspace and tab. It asks
nothing: the Steward starts on the harness, model, and effort it was last
launched with, else on the new-agent form's presets, and greets with the
project's state. It takes the root tab of the project's Corgi workspace when
no agent runs there; if one does, such as your own session, the Steward gets
a new tab of that workspace instead, and the root tab is left alone. Without
a Corgi workspace for the project, one is created.

The Steward keeps its memory outside the repository, in
`$XDG_STATE_HOME/corgi/steward/<project>/` (`~/.local/state/…` by default):
`decisions.md`, a `ledger.jsonl` of dispatched work, and every brief it sent
under `briefs/`. It starts workers with `corgi spawn` and follows them with
two more commands:

- `corgi fleet [PROJECT]` lists the project's agents, one tab-separated row
  each: name, role (`steward` or `worker`), state, task, model, context, and
  working directory.
- `corgi report NAME` prints the newest thing an agent said, which for a
  worker is the report its brief asks it to end with; an agent without a
  readable transcript shows its screen instead.

The Steward does not wait for its workers: the Corgi dashboard wakes it.
Whenever an agent of a project with a running Steward, other than the
Steward itself, becomes done, idle, or blocked after working, including
after it was steered, the dashboard prompts that Steward with one line, such
as `[corgi] w-x is done. Run: corgi report w-x`. Every agent of the project
counts, whether the Steward or you started it. The prompt waits until the
Steward has finished its current turn. The Steward stays out of your own
sessions: when the turn that ended was yours, in an agent it did not
dispatch or in one of its workers you steered, it only checks with `git
status` and `git log` whether anything changed. It says nothing, and it
records only a significant decision or architectural change, silently, in
`decisions.md` with where it was observed. For a worker you steered it
also notes your change in its ledger and judges the work against the brief
plus that change. Only changes the dashboard sees while
it runs count, so opening it never replays old states, and with the
dashboard closed nobody is woken: the Steward then checks `corgi fleet` when
you next talk to it. With several dashboards open, such as the tab and the
popup, the first to take a lock under `$XDG_STATE_HOME/corgi/wake/` for its
Herdr socket sends the prompts, and another takes over when it quits.

Corgi marks the Steward's pane when it launches it, which is how the dashboard
recognizes it. The Steward also starts in a directory that is not a Git
repository yet: until the project has a first commit it may run `git init`
and commit a `README.md`, `.gitignore`, and `AGENTS.md` shaped by your first
request, and nothing else in the checkout.

Claude Code asks whether to trust a directory the first time it runs in one.
A Git repository does not inherit the trust of the folder it sits in, and a
worktree's trust is its main repository's, so the first worker of a new
repository is asked even when the Steward was not. For a project Corgi
created itself, Corgi answers that one question with Yes when it starts an
agent there: only when exactly that question is on screen, about the agent's
own directory. Codex asks the same question, keyed the same way; for a Codex
agent in a project Corgi created, Corgi passes that project as trusted with
`-c projects={…}` on the command line, so Codex does not ask and nothing is
written to `~/.codex/config.toml`. Corgi records the projects it created in
`$XDG_STATE_HOME/corgi/created-projects`. Every other question, and the
trust question in any other project, waits for you in the agent's pane; Corgi
waits up to ten minutes for your answer before it hands over the first
prompt.

The Steward's commands call the installed Corgi plugin's release build. To
point a Steward at a development build instead, set `CORGI_STEWARD_BIN` to
that binary when launching it.

## Plan usage

The header shows one row per CLI Corgi can find (see above for how it looks).

- **Codex**: Corgi starts a short-lived `codex app-server` and asks it for the
  account's rate limits. Requires a signed-in `codex`.
- **Claude Code**: Corgi reads the OAuth token that Claude Code already stores
  (`~/.claude/.credentials.json`, or the macOS Keychain entry
  "Claude Code-credentials") and calls Anthropic's usage endpoint with `curl`.
  Requires a signed-in `claude` and `curl`. This reuse of Claude Code's stored
  login is unofficial and read-only: Corgi only reads usage and never changes
  the account. The endpoint is not documented, so the reading may break if it
  changes, and the row then falls back to "unavailable".

Rows that cannot be read show "unavailable" and never block the dashboard. To
check the Claude reader by hand:

```bash
cargo test -- --ignored live_claude_usage --nocapture
```

## Data and privacy

All agent control goes through the Herdr socket supplied in
`HERDR_SOCKET_PATH`. Prompt text is sent over that socket and is not placed in
process arguments. Corgi does not transmit telemetry, persist terminal output,
or write prompt history. Terminal text is rendered as plain text after bounded
cleanup and common-token redaction.

The only other network activity is the optional plan-usage row: `codex
app-server` talks to OpenAI on Codex's behalf, and the Claude row sends the
locally stored Claude Code token to `api.anthropic.com` only. The token is
passed to `curl` on stdin, never as an argument, and is not stored by Corgi.

## Optional Omarchy companion

The `omarchy/` directory contains an independently installable bar widget. Its
corgi icon opens a compact dashboard with project groups, agent activity,
model/context information, cache estimates, and plan usage. It uses Corgi's
`--bar-stream <socket-path>` mode to share the TUI's data readers. See
[the companion README](omarchy/README.md) for setup and controls.

## License

MIT
