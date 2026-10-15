<p align="center">
  <img src="docs/images/logo.svg" alt="Corgi" width="504">
</p>

<p align="center">
  <b>The dashboard for your Herdr agents, and a supervisor that runs them.</b><br>
  Every coding agent in your Herdr session in one pane, grouped by project:
  what it is doing, what it waits on, and what its context and prompt cache
  cost you. Give each project a supervisor, a long-lived agent you talk to
  about what to build, and it briefs worker agents in their own worktrees,
  reviews what they report, and tags their work for you to merge.
</p>

## The dashboard

![The Corgi dashboard: the header's herd and plan-usage cards over the webshop and weather projects, each a card of its supervisor's latest message and command over a count of its workers by state, with webshop's blocked worker named](docs/images/dashboard.svg)

- **One card per project with a supervisor**: the supervisor's latest message and command, and its workers summed up by state, with every blocked worker named. `→` expands a card into each worker's own rows, and Corgi remembers the choice per project.
- **One row per agent** of any kind Herdr can start, such as Codex and Claude Code, for expanded cards, projects without a supervisor, and scratch sessions.
- **What needs you first**: the supervisor, then blocked, working, done and idle; within a state, the agent that arrived there last comes first. A worker its supervisor recommends merging shows a magenta `MERGE` badge.
- **What each agent is doing**: the newest thing said in its session and the command or tool it runs.
- **What it costs**: its model and effort, how full its context is, how long its prompt cache stays warm, and in the header, each CLI's 5-hour and weekly plan usage.
- **New agents in their own worktree** with `n`, and their work merged back with `m`, stamped with a check when it lands:

<img src="docs/images/merge-outcomes.gif" width="757" alt="The merge popup confirming the merge of the order-history worker, its steps running, and the success stamped with a big green check">

**Install:** `herdr plugin install zyrre/corgi` (key binding and updates under [Install](#install)).

### Keys

| Key | Action |
| --- | --- |
| `j` / `k`, `↑` / `↓` | Select an agent, or a collapsed project card as its supervisor |
| `→` / `←` | Expand the selected project's card into its workers' rows, or collapse it again |
| `space`, then `u` / `d`, `PgUp` / `PgDn` | Expand the selected session into its transcript and scroll it; `space` collapses it |
| `p` | Prompt the selected agent |
| `n` | Start a new agent in its own worktree |
| `t` | Start a scratch session in your home directory |
| `s` | Start or focus the supervisor of the selected agent's project |
| `m` | Merge and push an agent's worktree branch (asks first) |
| `f` or `Enter` | Focus the selected agent's pane |
| `x` | Close the selected agent, and delete its worktree checkout (asks first) |
| `r` | Refresh now |
| `Esc`, `q` | Collapse the transcript, or close Corgi; `q` always closes |

For a compact popup instead of a tab, open the `quick` pane: `herdr plugin pane open --plugin io.github.zyrre.corgi --entrypoint quick`.

### Reading a row

In an expanded card, a project without a supervisor, and the scratch sessions, each agent has three rows:

![One agent row, labelled: selection bar, state, task, model, effort, context used, prompt cache left, worktree, the newest thing said, and the command it runs](docs/images/row-anatomy.svg)

### Read a session in place

`space` grows the selected row into the session's latest turns, newest first, without leaving the list.

![The finished order-history agent expanded: its closing summary in full, then the commands, edits, thinking and prompt before it](docs/images/expanded-session.svg)

### Start an agent

![The new-agent form: the task typed, and the harness, model, effort, project and checkout on their presets](docs/images/new-agent-form.svg)

`n` opens the new-agent form with everything but the task preset: the selected agent's project, the default harness with its own model and effort, and a fresh worktree. Type the task and press `Enter`; `←` / `→` change a setting in place, and `Space` or typing opens its list. In a project with no agents yet, the first one is its supervisor. Each worker gets its own Git worktree on a fresh `worktree/<name>` branch; switch the Checkout row to work in the project directory as it is instead.

### Merging

`m` shows what the merge brings in, checks that both checkouts are clean, then runs `git merge --no-ff` and `git push` in the primary checkout, and offers to close the worktree. If the merge stops on conflicts, the popup lists the files; with a supervisor running, `s` aborts the merge and asks the supervisor to have the agent merge the base branch into its own branch and resolve them there, after which you merge again. Esc leaves the primary checkout mid-merge to resolve by hand.

## The supervisor

Each project can have one supervisor: a long-lived Claude Code or Codex session in the project's root tab, which you talk to about *what* to build and *why*. It turns that into briefs, runs worker agents in their own worktrees, reviews what they report, and keeps the project's decisions and history, so you deal with one agent instead of many. It never edits the checkout, merges or pushes itself. Its full role is [`supervisor/ROLE.md`](supervisor/ROLE.md).

- **Start it** with `s` on any agent of the project, or `corgi supervisor ~/repos/webshop` from a shell. It greets you with the open work, anything blocked or ready to merge, and the questions it still has for you.
- **Plans, then briefs**: it proposes how the work splits and which model and effort each part gets, waits for your go, then saves a self-contained brief per worker and starts each with `corgi spawn` in a fresh worktree. Spawn refuses a project outside Git or in Corgi's state directory, so a worker never lands in the wrong place.
- **Woken by its workers**: when a worker stops, the dashboard puts a `[Corgi]` line in the supervisor's inbox, with the worker's report quoted when it is short, and types it in once the supervisor is between turns. Wakes survive the dashboard closing; `corgi inbox` shows what has not been typed in yet, and scripts add lines with `corgi notify`.
- **Questions you can answer at a glance**: anything that needs your judgement comes as a numbered question with options and a recommendation (`Q3: B` answers it), and a worker with an open question is not tagged for merge.
- **MERGE when it is ready**: once a worker's report meets its brief and its branch merges cleanly, the supervisor runs `corgi tag NAME merge` and the dashboard shows it as `MERGE`. Your `m` merge tells the supervisor, which records it and re-checks its other tagged workers against the moved base branch; a conflicted merge is handed to it from the merge popup.
- **Memory that outlives its context**: a decision log, a ledger of every worker, and every brief, kept outside the repository. `corgi digest` gives it a bounded summary at the start of each session, and `--search` and `--decision` find older history.
- **Handover to a fresh session**: when its context passes 50%, or it has idled nearly until its prompt cache expires, the dashboard has it write a handover note and starts a fresh supervisor that picks up from it.
- **Lessons kept in the repo**: what a worker learned the hard way goes into `AGENTS.md` or `ARCHITECTURE.md` as a commit on that worker's branch, so the next worker knows it.
- **Cost-aware**: `corgi fleet` lists its agents with their context use and prompt cache, warm with the minutes left or cold with the tokens a follow-up would re-read, so it can start a fresh worker instead.
- **Out of your way**: agents you start or steer yourself are yours; it only notes significant decisions made there, silently.

```mermaid
flowchart TD
    you(["You"]) <-->|"what to build,<br/>numbered questions"| sup["Supervisor<br/>project root tab"]
    sup <-->|"corgi digest"| memory[("Memory<br/>decisions, ledger,<br/>briefs, handover")]
    sup -->|"corgi spawn + brief,<br/>[supervisor] prompts"| workers["Workers<br/>one worktree each"]
    workers -.->|"stop: done, idle, blocked"| dash["Corgi dashboard"]
    dash -.-> inbox[("Inbox")]
    inbox -.->|"[Corgi] wake with<br/>the quoted report"| sup
    sup -->|"corgi tag NAME merge"| badge["MERGE badge"]
    badge --> merge(["Your merge with m"])
    merge -.->|"[Corgi] merged line"| inbox
```

## Install

```bash
herdr plugin install zyrre/corgi
```

It also links the `corgi` command into `~/.local/bin`. Run it again to update; `herdr plugin uninstall io.github.zyrre.corgi` removes it but leaves the link (`rm ~/.local/bin/corgi`). To open the dashboard with `prefix+d`, or focus it when it is already open, add this to `~/.config/herdr/config.toml` and run `herdr server reload-config`:

```toml
[[keys.command]]
key = "prefix+d"
type = "plugin_action"
command = "io.github.zyrre.corgi.open"
description = "Corgi (open or focus)"
```

## Data and privacy

Corgi talks only to the Herdr socket and sends no telemetry. It keeps no terminal output or prompt history; what it stores, in `~/.local/state/corgi` (or `$XDG_STATE_HOME/corgi`), is its own bookkeeping and each supervisor's memory and inbox, which holds the reports of workers that stopped. The only network use is the plan usage: `codex app-server` for Codex, and Claude Code's stored login sent to `api.anthropic.com` for Claude. That Claude reading is unofficial and read-only, and may break if Anthropic changes the endpoint.

## More

- Architecture: [ARCHITECTURE.md](ARCHITECTURE.md) has the socket and API boundaries, and every behaviour in detail.
- Supervisor: [`supervisor/ROLE.md`](supervisor/ROLE.md) is the role every supervisor runs by, and `corgi <command> --help` documents each command.
- Omarchy companion: [`omarchy/`](omarchy/README.md) is an optional bar widget with a compact dashboard of its own.

## Build from source

```bash
git clone https://github.com/zyrre/corgi && cd corgi
cargo build --release && herdr plugin link "$PWD"
```

Uninstall the plugin before you link, since both use the same id. The installer builds from source when `CORGI_BUILD=source` is set, and `CORGI_DOWNLOAD_URL=<url>` downloads from `<url>/v<version>/` instead of the GitHub release. `CORGI_LINK=0` skips the `~/.local/bin` link, and `CORGI_BIN_DIR=<dir>` links into `<dir>` instead.

## License

MIT
