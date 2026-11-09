# Corgi

A panel that shows how each of your Corgi projects is doing: one block per
project with its supervisor's state, its workers by state, the open
questions waiting for you, the work ready to merge and the inbox items not
yet delivered; projects that need you stand out. Click a project for its
supervisor and workers (state, task, model, context, prompt cache, MERGE
tag), its open questions with their full text, and a Merge button on each
worker tagged ready to merge. It needs the `corgi` command-line tool, which
it installs by itself; `herdr-app/install.sh` in the Corgi repository
installs both the tool and the mod.

<!-- preview.png is still to be made, from a look at the running window. -->

## What you see

The panel docks on the right, 420 px wide (`mod.json`: `"place": "right"`),
on the theme's `surface`, text 13 px.

- **Header** (one line, a `border` rule below): `Corgi` in 14 px semibold,
  `Herdr not reachable` in `blocked` when Corgi cannot reach the Herdr TUI,
  and the time of the last reading (`HH:MM:SS`, UTC) in `dim` 11 px, or `…`
  while one is being read.
- **Overview** (the main view): a scrolling column of project blocks, 10 px
  apart. Each block is a card (radius 8, padding 10, `background`, a
  `border` outline, or `blocked` when the project needs you):
  - the project's name (15 px semibold), its base branch (`dim`), and on the
    right, when it needs you, a `blocked` badge such as
    `needs you · 2 questions, 1 blocked, 1 to merge`;
  - `Supervisor` and its state: `working`, `resting` (done or idle),
    `needs you` (blocked), or `not running` in `dim`; then its context
    percentage and prompt cache (`warm 55m`, `cold 120k`);
  - the number of workers, then a count per state, most urgent first, each
    in its state's color (`working`, `blocked`, `done`, `idle`);
  - up to three open questions, `Q71  How should…`, one line each, then
    `+N more`;
  - a `dim` 12 px line: ready to merge in the ledger, inbox items waiting,
    dispatched work, backlog entries (only those that are not zero).
- **Project view**: `← All projects` in the header, the project's folder and
  base branch, then a card per agent, the supervisor first: a dot in the
  state's color, the name (semibold), its role, its state, a `MERGE` badge
  (`accent`) when tagged, and a `Merge` button (`solid`); below, the task on
  one line, and `model · ctx 18% · cache warm 55m · branch` in `dim`. Then
  `Open questions (N)`, each with its id in `blocked`, its title, and the
  worker it blocks; and a `dim` line with the ledger counts, inbox and
  backlog.
- **Setup**: while it installs or updates the `corgi` tool,
  `Installing Corgi 0.2.3…` or `Updating Corgi 0.2.2 → 0.2.3…`; if that
  fails, the error in `blocked`, the installer's last lines in a monospace
  box, the command to run by hand, and `Retry`.

## How it behaves

- Agent states are Corgi's: `working`, `blocked`, `done`, `idle`, `unknown`
  and `exited`, each in the theme color of that name (`unknown` for the
  last two). A project needs you when its supervisor has open questions, an
  agent is blocked, or a worker is tagged MERGE.
- It reads `corgi status --json` when it starts, every 5 seconds, when a file
  under the supervisors' state folder changes, and when the app's live
  agents change (compared once a second).
- Clicking a project opens its view; `← All projects` goes back. Clicking a
  question with a saved text shows its whole text below it; clicking again
  hides it.
- `Merge` asks inline, `Merge worktree/x into main and push?` with `Cancel`
  and `Merge + push`, then shows `Merging…` and runs
  `corgi merge NAME --project DIR --json`: the merge Corgi's dashboard runs
  with `m`, which also tells the supervisor. The result shows in a card at
  the top of the project view, green-ish (`working`) for merged, `done` for
  a failed push, `blocked` otherwise. When the base branch's upstream has
  new commits it offers `Pull, merge + push` (`--pull`); after a failed
  fetch, `Merge + push without fetching` (`--skip-upstream`). A conflict is
  aborted and handed to the project's supervisor when one runs.
- When no `corgi` that knows `status --json` and is at least the mod's
  version is found, it installs that version at once, without asking, then
  shows the overview.
- It never starts, wakes, types into or focuses an agent.

## Built from

- One mod, `mods/corgi/` (`main.js`, `mod.json`). Imports: `View`, `div`
  from `gpui-kit`; `v_flex`, `h_flex` from `gpui-base`; `agents`, `exec`,
  `home`, `appHome`, `readTextFile`, `watchFolder`, `theme` from `herdr`.
  No commands, keys or overlays.
- Programs: `corgi --version`, `corgi status --help`,
  `corgi status --json`, `corgi merge … --json`, and, to install,
  `sh -c` running `curl` on the release's `herdr-app/install.sh` with
  `--version <VERSION> --cli-only`.
- Files: `mods/corgi/config.json` (`{ "corgi": "<path>" }`, written by
  `herdr-app/install.sh`), each question's file (`questions/Q<n>.md` in the
  supervisor's state folder, named by `corgi status`), and a watch on the
  folder holding the supervisors' state folders.
- `corgi status --json` prints `{version: 1, generated, herdr: {connected,
  status}, projects: [{name, dir, state_dir, base, supervisor, agents:
  [{name, role, state, task, model, harness, ctx_pct, cache: {state,
  minutes_left, tokens, estimated}, cwd, branch, worktree, tag, request_id,
  terminal}], questions: [{id, title, blocks, file}], inbox_pending,
  ledger: {needs_answer, ready_to_merge, dispatched}, backlog}]}`.
  `corgi merge` prints `{result: merged | conflicts | push-failed | refused,
  message, commit, reason}`.

## Watch out

- The mod is nothing without `corgi`, and copying the mod folder in does not
  bring it. On start, and whenever no usable `corgi` is found, the mod looks
  in `config.json`'s path, `corgi` on `PATH`, `~/.local/bin/corgi`,
  `~/.local/share/corgi/target/release/corgi` and `~/.cargo/bin/corgi`, and
  takes the first that knows `status --json` and is not older than its own
  `VERSION`. If none is, it installs `VERSION` (or updates to it) by itself,
  at once, from a fixed source: the `herdr-app/install.sh` of the release
  tag `v<VERSION>` on GitHub, run with `--version <VERSION> --cli-only`.
  That installer clones the repository into `~/.local/share/corgi`, checks
  out the tag, takes the tag's reviewed checksum pins from the main branch,
  and downloads the release binary only when it matches its pin, else
  builds it with `cargo build --release --locked` (minutes; `exec` waits at
  most 10 minutes). It never installs another version than the mod's own,
  and it leaves the mod's folder alone. A failed install shows the error,
  the installer's last lines and the command to run by hand, and is tried
  once per app start; `Retry` tries again.
- `VERSION` in `main.js` is set by each Corgi release (the release refuses a
  tag that does not match it); a tag older than the mod's first release has
  no `herdr-app/install.sh`, so an unreleased mod's own install fails.
- A `corgi` newer than the mod is used as it is, never downgraded.
- `corgi status` reads the Herdr TUI's socket (`$HERDR_SOCKET_PATH`, else
  `~/.config/herdr/herdr.sock`) for running agents; without it the projects
  still show from the state folders, with no agents.
- `herdr-app check` renders only the first frame ("Reading Corgi's
  status…"), so it does not prove the overview or project view work. A
  `.when()` callback must return the element it got, and text for one line
  must have its newlines joined (tasks and question titles can hold them).
- The mod runs only while the window is open, so it delivers nothing to
  supervisors: that stays with Corgi.

## Done when

- `herdr-app check mods/corgi` passes.
- With `corgi` installed and supervisors running, the overview lists each
  project with its supervisor's state, worker counts by state, open
  questions and counts, and a project with a question, a blocked agent or a
  MERGE tag has the `needs you` badge and border.
- Clicking a project shows its agents with state, task, model, ctx, cache
  and branch; `← All projects` returns.
- A tagged worker's `Merge` asks first, and `Merge + push` shows `merged`
  with the message and the commit landing on the base branch's remote.
- With no `corgi` on the machine, opening the mod shows
  `Installing Corgi <VERSION>…`, then the overview; with the network off it
  shows the error, the command to run by hand and `Retry`, and does not try
  again on its own.
