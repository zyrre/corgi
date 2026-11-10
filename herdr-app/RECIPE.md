# Corgi

Two mods and a layout. The **Corgi** mod is an overview: how
each of your Corgi projects on herdr-core (`hc`), the Herdr app's backend,
is doing: one block per project with its supervisor's state, its workers by
state, the open questions waiting for you, the work ready to merge and the
inbox items not yet delivered; projects that need you stand out. Above them,
whether `corgi daemon`, which wakes the supervisors, is running. Click a
project for its supervisor and workers (state, task, model, context, prompt
cache, MERGE tag), its open questions with their full text, and a Merge
button on each worker tagged ready to merge; click an agent to see its
terminal.

The **Projects** mod is the sidebar of the **Corgi layout**, which is
Herdr's own layout with one list where Herdr has its Spaces and Agents
lists: your spaces, each repository with its agents under it, one line each
(the agent's task, its harness's icon, its state and the worktree it works
in, in that order), and a `+` that opens a project: type a name or a path, and Enter shows
its supervisor or starts it. A worker Corgi tagged ready to merge has a
`MERGE` mark in its row: click it, confirm, and its branch is merged into
the project's base branch and pushed, as `m` does in Corgi's dashboard. The
last line says which Corgi runs (`Corgi 0.2.3 · 5b50895`) and whether a
newer one has been merged since.

They need the `corgi` command-line tool; `herdr-app/install.sh` in the
Corgi repository installs the tool, both mods and the layout. The layout is
not switched to: `herdr-app do layout.switch '{"name":"corgi"}'` shows it,
and `"main"` shows Herdr's own again. The Corgi mod is installed but shown
nowhere: the space it used to have, the Corgi space, is retired, since
Corgi's place in the app is the Projects list. `corgi app-space`, run by
hand, still makes that space for whoever wants the overview.

<!-- preview.png is still to be made, from a look at the running window. -->

## What you see

### The Corgi mod's overview

The Corgi mod docks nowhere by itself (`mod.json`: `"place": "none"`) and no
layout of Corgi's shows it: it is drawn where a person puts it, which
`corgi app-space` does as the only tab of a space named `Corgi`, first in
the list of spaces (see "The Corgi space"). It fills the tab on the theme's
`surface`, text 13 px; the views' content is a column at most 960 px wide,
centred in a wider tab.

- **Header** (one line, a `border` rule below): `Corgi` in 14 px semibold,
  `herdr-core not running` in `blocked` when the core is not running (or
  `herdr-core not answering` when `hc` failed), and the time of the last
  reading (`HH:MM:SS`, UTC) in `dim` 11 px, or `…` while one is being read.
- **Overview** (the main view): a scrolling column, 10 px apart, that starts
  with the daemon: while it runs, a `dim` 12 px line `Corgi daemon running ·
  pid 4242`; when it does not, a card with a `blocked` outline: `Corgi daemon
  not running: supervisors on the Herdr app won't be woken or handed over`
  (semibold, `blocked`), then `To start it, run:` (or, when its service is
  installed, `Its service is installed but not running. To restart it, run:`)
  in `muted`, and `corgi daemon install` in a monospace box. It always shows.
  When `hc` failed, its error follows in `blocked`. Then the project blocks.
  Each block is a card (radius 8, padding 10, `background`, a `border`
  outline, or `blocked` when the project needs you):
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
  one line, and `model · ctx 18% · cache warm 55m · branch` in `dim`; a card
  takes `hover` under the pointer. Then
  `Open questions (N)`, each with its id in `blocked`, its title, and the
  worker it blocks; and a `dim` line with the ledger counts, inbox and
  backlog.
- **Setup**: while it installs or updates the `corgi` tool,
  `Installing Corgi 0.2.3…` or `Updating Corgi 0.2.2 → 0.2.3…`; if that
  fails, the error in `blocked`, the installer's last lines in a monospace
  box, the command to run by hand, and `Retry`.

### The Corgi layout and its Projects list

The layout (`layouts/corgi.json`, listed as "Corgi") is Herdr's shipped
`main` as it is, except in the sidebar: the block that showed Herdr's
`spaces` mod shows the `projects` mod, and the block that showed its
`agents` mod is gone, so the one list takes the whole height between the
brand row and the footer. It is built, never hand-written (below).

The Projects mod fills its block on `chrome`, text 13 px:

- **Header** (31 px): `Projects` in `dim` 11.5 px and a 26 px `+` button
  (tooltip `Open project`), lit while its field is open.
- **Open project** (only while open, under the header): a card (`surface`,
  a `border` outline, radius 9) holding a text field (30 px, an `accent`
  outline, placeholder `Project name or path…`); up to eight suggestions,
  26 px each: the project's name, its folder from `~` in `dim` 11 px, cut
  with `…`, and on the right where it is known from in `dim` 10.5 px (`2
  agents`, `open in Herdr`, `nearby`, `recent`, `typed path`), `running` in
  `working` when its supervisor runs, or `new project` in `accent`; the
  highlighted one on `selected`; `N more: keep typing` when more match; then
  `Starting <name>'s supervisor…` with a spinner while it starts, or why it
  did not in `blocked`; and a `dim` 10.5 px line `Enter opens · Tab completes
  · Esc` with `New space` on its right.
- **The list** (scrolls): the spaces in order, under a header per machine
  once one is not this computer, as Herdr's Spaces list has them.
  - A space outside any repository (`~`, another folder, a Corgi space):
    30 px, a colored 18 px avatar with its initial, its name.
  - A project, a repository: the space on its primary checkout, drawn the
    same way, with its branch and ahead/behind after the name only when
    unusual. Under it, indented 22 px, one 26 px row per agent in it and in
    the spaces on its worktrees, which reads left to right: the task in
    12.5 px (medium weight for `Supervisor`); the harness's icon (14 px;
    Claude's now, Codex's or OpenCode's when it is one); the state, a
    spinning ring in `working`, a `!` tile in `blocked`, a dot in `done`, a
    ring in `idle`; on a worker tagged ready to merge, a `MERGE` mark
    (10 px semibold, `accent_foreground` on `accent`, radius 4; tooltip
    `Merge worktree/x into main and push`); and the worktree's folder name
    in `dim` 11 px
    (`worktree-calm-forest-4f96`; nothing on the primary checkout). They
    follow one another, 7 px apart, from the left; nothing is pushed to the
    right edge. The supervisor is first; the rest keep the app's order. One
    line each: a row too wide is cut from the right, the worktree's name
    first, with `…`, down to nothing if need be, and only then the task;
    the icon, the state and the mark stay whole.
  - Under a worker's row, while its merge is asked about, runs or has
    ended: a card indented like the row (22 px; `surface`, radius 7,
    padding 7, text 11.5 px, its text wrapping) with a `border` outline
    while it asks or runs, `working` for merged and for a supervisor that
    was asked, `done` for a failed push, `blocked` otherwise. It holds
    `Merge worktree/x into main and push?` with `Cancel` (`raised`) and
    `Merge + push` (`solid`); then a spinner and `Merging…`; then
    `<worker>: merged` (or `conflicts`, `push-failed`, `refused`) in the
    outline's color, semibold, Corgi's message, and its buttons, 22 px
    high: `Pull, merge + push` or `Merge + push without fetching` when
    Corgi offers it, `Ask supervisor to fix` when it failed and the
    project's supervisor runs, and `Dismiss`.
  - A worktree's space with no agent in it keeps a 28 px row of its own
    under its project: a branch glyph and the space's name.
  - A space outside a repository with agents in it has their rows under it
    the same way.
  - The shown space's row is on `selected`; so is the agent with the focus,
    and, for a shown worktree space that has no row, its first agent.
- **Which Corgi runs** (under the list, 16 px from the left, `dim`
  10.5 px): `Corgi 0.2.3 · 5b50895`, the release and the commit the `corgi`
  that answers was built from, with `· modified` for a build with
  uncommitted changes, and the release alone for a build without Git. When
  the checkout it was built in has commits the build lacks, `· main is
  newer` (the branch checked out there) in `done`, and under it, in
  monospace 10 px `muted`, wrapping, what to run:
  `cd ~/repos/corgi && CORGI_BUILD=source herdr-app/install.sh`. When the
  daemon does not run, `Corgi daemon not running: supervisors won't be
  woken or handed over` (`installed but not running` when its service is
  installed) in `blocked`, wrapping, and `corgi daemon install` under it
  the same way; when it runs another build, `The daemon runs another build:
  4563c06` (or `an older build`) in its place. Nothing until Corgi has
  answered; a `corgi` too old to say what it is shows no build, only a
  daemon that is down.
- **The rail** (the sidebar narrower than 160 px): no texts; the `+`, a
  32 px cell per space with its number and, on the right, the most urgent
  state in it (the spinner, the `!`, a dot for done), and `»` at the bottom.

## How it behaves

The overview:

- Agent states are Corgi's: `working`, `blocked`, `done`, `idle`, `unknown`
  and `exited`, each in the theme color of that name (`unknown` for the
  last two). A project needs you when its supervisor has open questions, an
  agent is blocked, or a worker is tagged MERGE.
- Agents are those Corgi started on herdr-core (`corgi supervisor`,
  `corgi spawn` there); `terminal` is the hc terminal's address. Agents on
  the old Herdr TUI are not shown. A project stays listed from its
  supervisor's state folder when none of its agents runs, and when the core
  is not running.
- It reads `corgi status --json` when it starts, every 5 seconds, when a file
  under the supervisors' state folder changes, and when the app's live
  agents change (compared once a second).
- Clicking a project opens its view; `← All projects` goes back. Clicking an
  agent's card runs `agent.focus` on its terminal, which shows it in its own
  space and tab; its buttons (`Merge`, `Cancel`, `Merge + push`) only do
  their own thing. Clicking a
  question with a saved text shows its whole text below it; clicking again
  hides it.
- `Merge` asks inline, `Merge worktree/x into main and push?` with `Cancel`
  and `Merge + push`, then shows `Merging…` and runs
  `corgi merge NAME --project DIR --json`: the merge Corgi's dashboard runs
  with `m`, on the worker's hc checkout and branch, which also tells the
  supervisor and clears the worker's MERGE tag. With the core not running it
  is refused, and nothing is started. The result shows in a card at
  the top of the project view, green-ish (`working`) for merged, `done` for
  a failed push, `blocked` otherwise. When the base branch's upstream has
  new commits it offers `Pull, merge + push` (`--pull`); after a failed
  fetch, `Merge + push without fetching` (`--skip-upstream`). A conflict is
  aborted and handed to the project's supervisor when one runs.
- When no `corgi` that knows `status --json` and is at least the mod's
  version is found, it installs that version at once, without asking, then
  shows the overview.
- It never starts, wakes or types into an agent. Waking
  supervisors, delivering their inboxes and handing them over is
  `corgi daemon`'s, a service `herdr-app/install.sh` installs, so it goes on
  with the window closed; `corgi status --json` says whether it runs
  (`daemon.running`, `daemon.pid`) and whether its service is installed
  (`daemon.installed`), and the overview shows it.
- When it starts it runs `corgi app-layout --home <app home> --json` once:
  the Corgi layout is built again if Herdr's `main` changed. `Corgi layout
  rebuilt` is a toast; a `main` it could not build from is a line in the
  overview, `Corgi layout: kept the Corgi layout as it was built last: …` in
  `blocked` (or in `dim` when there is no layout at all, such as without the
  Projects mod). It never switches the window's layout.

The Projects list:

- A project's agents are those the app lists in its spaces (`agents()`), and
  Corgi's that have reported no status yet or whose agent exited into its
  shell, found by the pane their terminal is in. An agent Corgi runs
  (`corgi status --json`, by its terminal) shows Corgi's task for it, and
  `Supervisor` for a supervisor; one started by hand shows the title it gave
  its terminal when that says something (not a folder, not `user@host`),
  else its tab's name, else its program's.
- Clicking a space shows it (`space.select`); dragging one reorders it
  within its machine; clicking an agent shows its terminal (`agent.focus`);
  a right-click on either opens the space's menu (Herdr's `menu` mod).
  `agent.next` and `agent.prev` follow the list's order.
- It reads `corgi status --json` when it starts, every 5 seconds, and when
  the app's live agents change (compared once a second): as often as the
  Corgi mod, never more. Each space's git is read as Herdr's Spaces list
  reads it. Without a `corgi` it still lists every space and agent, by
  their own titles.
- `+` (or the command `projects.open`, for a key such as `"cmd-o":
  "projects.open"` in `keys.json`) opens the field with the keyboard in it,
  and again puts it away; on the rail it unfolds the sidebar first. Opened,
  and after every key, it runs `corgi projects --json --query <text> --open
  <folder>…` with the folders its spaces on this computer are in: Corgi's
  known projects (those with agents, those open, the directories beside
  them, those it remembers) narrowed by every word typed; for an absolute
  path or one from `~`, the directories that complete it; and a name alone,
  or a path that does not exist, as a new project (a name goes beside most
  known projects). The first suggestion is preset. ↑/↓ move, Tab puts the
  highlighted folder in the field with a `/` after it, Esc closes.
- Enter (or a click on a suggestion) opens it. A project whose supervisor
  runs, in a terminal the app shows, is shown at once (`agent.focus`).
  Otherwise it runs `corgi open <dir> --json`, with `--create` for a new
  project and a `--terminal <address>` for every shell the supervisor could
  start in: panes whose program is a shell with no agent status, those of
  the focused space first, then any other already in that folder. Corgi
  then names the running supervisor's terminal; or starts the supervisor in
  the first offered shell that waits at its prompt in the project's primary
  checkout, which makes that terminal the supervisor's and its space the
  project's, where its workers nest; or in a new terminal and space, as
  `corgi supervisor <dir> --backend hc` does. A new project's directory is
  created and recorded as Corgi's, so Claude Code's folder-trust question is
  answered. On success the field closes and the supervisor's terminal is
  shown; on a failure Corgi's message shows in the field, which stays open
  with what was typed. A second Enter while one starts does nothing.
- `New space` in the field makes a plain space (`space.new`), as `cmd-n`,
  the tab bar's `+` menu and the space menu still do; a machine's header
  keeps its own `+`.
- `MERGE` is on a worker whose `tag` in `corgi status --json` is `merge`:
  Corgi's supervisor tags a worker whose branch is ready to merge straight
  into the base branch. A project that merges through merge or pull
  requests is not tagged, so it has no mark. A click on the mark asks under
  the row (and is not also the row's click); `Cancel`, or the mark again,
  puts the question away. `Merge + push` runs `corgi merge <worker>
  --project <dir> --json`: the merge Corgi's dashboard runs with `m`, with
  its checks, in the project's primary checkout, pushed, which also clears
  the tag and tells the supervisor in its inbox. One merge at a time: while
  one runs the marks are dimmed and do nothing. Then the status is read
  again.
- The result stays under the row until `Dismiss`: `merged`; `conflicts`,
  which Corgi has aborted and handed to the project's supervisor when one
  runs (its message says so), and left in the primary checkout to resolve
  when none does; `push-failed`, the branch merged but not pushed;
  `refused`, with why. Refused because the base branch's upstream has new
  commits, it offers `Pull, merge + push` (`--pull`); after a failed fetch,
  `Merge + push without fetching` (`--skip-upstream`).
- `Ask supervisor to fix` is offered on a failed push, a failed fetch, and
  a merge that failed or was refused before it started (uncommitted changes
  in the primary checkout, say): whenever Corgi's answer has an `error`,
  and the project's supervisor is running (`supervisor` in the status, not
  `exited`). It runs `corgi merge <worker> --project <dir> --json
  --ask-supervisor <error>`, with `--commit <commit>` after a failed push:
  a line in the supervisor's inbox, as `s` sends from the dashboard's merge
  dialog, which `corgi daemon` types in. The card then says who was asked;
  if nobody could be, why, with the button still there.
- The last line is read from the same status: `corgi` (`version`, `commit`,
  `dirty`, `source`, `newer`) and `daemon.build`. `newer` is Corgi's own
  finding: the checkout the binary was built in (`source`) has commits its
  `commit` lacks, which is the case after a branch was merged there. A
  downloaded release was built elsewhere, so it is never "newer". A running
  daemon whose `build` is not the answering `corgi`'s (version, commit and
  `dirty` compared) is another build: the binary was rebuilt and the daemon
  not restarted. A daemon that is not running (`daemon.running` false) wakes
  no supervisor and delivers no inbox, a merge's line included, so it is
  said first; `daemon.installed` only changes the words. `corgi daemon
  install` is the answer to both: it installs the service again, which
  starts the daemon on the `corgi` that answers.
- `»` on the rail unfolds the sidebar (Herdr's own layout has it in its
  Agents list).

## Built from

- The Corgi mod, `mods/corgi/` (`main.js`, `mod.json`: `"place": "none"`).
  Imports: `View`, `div` from `gpui-kit`; `v_flex`, `h_flex` from
  `gpui-base`; `agents`, `exec`, `home`, `appHome`, `readTextFile`,
  `watchFolder`, `theme`, `run`, `notify` from `herdr`. App commands:
  `agent.focus`. No keys or overlays.
- Programs: `corgi --version`, `corgi status --help`,
  `corgi status --json`, `corgi merge … --json`, `corgi app-layout --home
  <app home> --json`, and, to install, `sh -c` running `curl` on the
  release's `herdr-app/install.sh` with `--version <VERSION> --cli-only`.
- The Projects mod, `mods/projects/` (`main.js`, `mod.json`: `"place":
  "none"`, `"title": "Projects"`). It began as a copy of Herdr's `spaces`
  mod (herdr-app 0.1.14) and is Corgi's own since: it follows no later
  version of it. Imports: `View`, `div`, `image`, `svg` from `gpui-kit`;
  `v_flex`, `h_flex`, `InputState`, `Input` from `gpui-base`; `spaces`,
  `machines`, `panes`, `agents`, `focused`, `run`, `openOverlay`, `exec`,
  `hc`, `theme`, `shared`, `setShared`, `setAgentOrder`, `watchFolder`,
  `window`, `layout`, `notify`, `home`, `appHome`, `readTextFile`,
  `nextCommand` from `herdr`. App commands: `space.select`, `space.move`,
  `space.new`, `agent.focus`, `layout.resize`; its own command `open`
  (`projects.open`). It opens Herdr's `menu` mod as a popup and draws
  Herdr's icons (`mods/workspace-chrome/*.svg`, the programs' `icons/`).
  Programs: `git` (status and `rev-parse`, per space), `hc machine ls`,
  `corgi status --json`, `corgi projects --json …`, `corgi open … --json`,
  `corgi merge … --json` (with `--pull`, `--skip-upstream`, or
  `--ask-supervisor <error> [--commit <commit>]`),
  `corgi app-layout --home <app home> --json`. Files: `mods/projects/
  config.json` or `mods/corgi/config.json` (`{ "corgi": "<path>" }`).
- The layout, `layouts/corgi.json`: no file of this folder, but what `corgi
  app-layout` builds from the app's own `layouts/main.json` (see "The Corgi
  layout").
- `corgi projects --json` prints `{query, parent, projects: [{dir, name,
  badge, new, supervisor}]}`; `corgi open … --json` prints `{result: started
  | running | failed, message, project, name, terminal, created, place: new
  | own | shell}`; `corgi app-layout --json` prints `{result: made | rebuilt
  | unchanged | kept | missing, message, layout}`.
- Files: `mods/corgi/config.json` (`{ "corgi": "<path>" }`, written by
  `herdr-app/install.sh`), each question's file (`questions/Q<n>.md` in the
  supervisor's state folder, named by `corgi status`), and a watch on the
  folder holding the supervisors' state folders.
- `corgi status --json` prints `{version: 2, generated, hc: {running,
  error}, daemon: {running, pid, build: {version, commit, dirty} | null,
  installed}, corgi: {version, commit, dirty, source, newer: {branch,
  commits, commit} | null}, projects: [{name, dir,
  state_dir, base, supervisor, agents:
  [{name, role, state, task, model, harness, ctx_pct, cache: {state,
  minutes_left, tokens, estimated}, cwd, branch, worktree, tag, request_id,
  terminal}], questions: [{id, title, blocks, file}], inbox_pending,
  ledger: {needs_answer, ready_to_merge, dispatched}, backlog}]}`.
  `corgi merge` prints `{result: merged | conflicts | push-failed | refused
  | asked, message, commit, reason, error}`: `reason` is why it was refused
  (`unavailable`, `behind`, `fetch-failed`, `failed`, `no-agent`,
  `no-supervisor`), `error` the failure alone when a supervisor could fix
  it, and `asked` what `--ask-supervisor` answers. `corgi --version` prints
  `corgi 0.2.3 (5b50895)`, and `--version --json` `{version, commit, dirty,
  source}`.

## The Corgi layout

- `corgi app-layout [--home DIR]` builds it, and `herdr-app/install.sh`
  runs that after the Projects mod is in place: it reads the app home's
  `layouts/main.json`, finds the node `sidebar`, and in it changes the one
  block whose `content.mod` is `spaces` to show `projects` (the block keeps
  its id `spaces`, its size and everything else) and removes the one block
  whose `content.mod` is `agents`. Nothing else differs from `main`. The
  file is written in one rename, and only when it differs from the one
  there, since the app reloads a layout the moment its file changes.
- Herdr replaces its shipped `main` whole on every start, so the layout is
  built again from it: by each install, and by each of Corgi's mods once
  when it starts. The build is a function of `main` alone, so comparing its
  result with the file in place says whether `main` changed.
- A `main` that is missing, is not JSON, has no `sidebar`, or has another
  number of `spaces` and `agents` blocks in it than one each, is not
  guessed at: nothing is written, the last build stays in use, and the
  command, the Corgi mod's overview and a toast of the Projects mod say
  why. The same holds for a home with no `mods/projects/main.js`: the
  layout would show a mod the app does not have.
- It never switches the window to the layout, and neither does the install:
  it prints `herdr-app do layout.switch '{"name":"corgi"}'`. A layout is
  listed under its file's name, so `corgi.json` is "Corgi" in Settings and
  the palette. The sidebar's width and the other things a person changes by
  hand are the app's own per layout, so the Corgi layout starts with
  Herdr's default width, not the one `main` was dragged to.
- `corgi app-layout remove` (run by `herdr-app/install.sh --uninstall`)
  deletes the file when it is one Corgi built (it shows the `projects`
  mod), after switching a window that shows it to `main`.

## The Corgi space

Retired: the install does not make it, and neither does `corgi daemon`. It
is a space named `Corgi` whose only tab shows the Corgi mod, for whoever
wants the overview on screen.

- `corgi app-space`, run by hand with the app's window open, makes it:
  `space.new {name: "Corgi", folder: ~, machine: ""}` (the app has no
  command that makes a space around a mod, so it starts with a shell),
  `tab.new {space, mod: "corgi", name: "Corgi"}`, `tab.close` on the
  shell's tab, and `space.move {index: 1}`. If the mod's tab cannot be
  made, the new space is closed again. With the window closed it makes
  nothing and says so.
- A space named `Corgi` on this computer is the Corgi space (`herdr-app
  ls`); with one there, nothing is made. A space nobody named is called
  after its first terminal's repo, and Corgi's own repo space is `corgi`,
  in lower case. A Corgi space that is there is left where it is, by the
  install too.
- An earlier Corgi noted a space still to be made (`app-space.json` in
  Corgi's state folder) for its daemon to make once the window opened. The
  daemon now deletes that note when it starts and makes nothing; so does
  `corgi app-space`.
- `herdr-app/install.sh --uninstall` runs `corgi app-space remove`, since
  the mod the space shows is going: it closes the tabs named `Corgi` with
  no terminals in Corgi spaces (the space goes with its last tab; terminals
  the person opened there stay).

## Watch out

- The Corgi mod is nothing without `corgi`, and copying the mod folder in does not
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
  and it leaves the mod's folder alone (`--cli-only` does put the Projects
  mod and the Corgi layout in place). A failed install shows the error,
  the installer's last lines and the command to run by hand, and is tried
  once per app start; `Retry` tries again.
- `VERSION` in `main.js` is set by each Corgi release (the release refuses a
  tag that does not match it); a tag older than the mod's first release has
  no `herdr-app/install.sh`, so an unreleased mod's own install fails.
- A `corgi` newer than the mod is used as it is, never downgraded.
- `corgi status` reads herdr-core for running agents, and only when its
  socket accepts a connection: any `hc` call starts a stopped core, with the
  caller's environment, and the mod must never be the one that starts it.
  Without it the projects still show from the state folders, with no
  agents. A `corgi` older than the mod's own version prints version 1, with
  `herdr` in place of `hc` and no daemon fields; the mod reads both.
- `herdr-app check` renders only the first frame ("Reading Corgi's
  status…"), so it does not prove the overview or project view work. A
  `.when()` callback must return the element it got, and text for one line
  must have its newlines joined (tasks and question titles can hold them).
- The mod runs only while the window is open, so it delivers nothing to
  supervisors: that stays with Corgi.
- Never edit Herdr's shipped `layouts/main.json` or its `spaces` and
  `agents` mods: Herdr replaces them whole on every start and moves an
  edited one aside. The Corgi layout is a file of its own for that reason.
- The Projects mod is one mod standing where two stood, so what only
  Herdr's Agents list did, it must: `setAgentOrder` on every render (or
  `agent.next` and `agent.prev` follow no list), and the `»` that unfolds
  the rail (on macOS the brand row has no room for its toggle there).
- An agent's row is found by its terminal's address in `corgi status
  --json`. Before the first read, and with no `corgi`, every agent shows by
  its own title; a row must never wait for Corgi.
- A row is cut from the right by flex alone, nothing is measured: the task
  and the worktree's name both truncate with a minimum width of 0 and
  neither grows; the task shrinks by a factor of 1 and the name by 1000,
  and flex takes the overflow from each in proportion to its factor times
  its width, so the name is gone before the task loses a letter. The icon
  and the state do not shrink (`flex_none`). Giving the task `flex_1`
  instead would make it the first to give way. A `\n` in a task starts a
  second line even when truncated: join the lines first.
- The `MERGE` mark and the card's buttons sit inside or beside a clickable
  row: each calls `cx.stop_propagation()` in its `on_click`, or the row's
  click shows the agent's terminal too. The mark is `flex_none` like the
  icon and the state, and stands before the worktree's name, so the name
  still gives way first. The card is an element of the list after its row,
  not a child of the row, which stays 26 px high.
- Never run `corgi merge` from a test or a look at the mod: it merges and
  pushes for real. `node scripts/test-app-mods.mjs` gives the mod canned
  answers for it; Corgi's own tests merge in scratch repositories.
- `Ask supervisor to fix` passes Corgi's `error`, not its `message`: the
  message is written for the person (`Merge unavailable: …`, `merge again
  with --skip-upstream`), the error is what the supervisor's line quotes. A
  conflict needs no button: `corgi merge` hands it over itself.
- The version line shows what the answering `corgi` says of itself, never
  what the mod's files are: after an install the mod is reloaded and reads
  the new binary's status within 5 seconds. A `corgi` from before `status`
  had `corgi` shows no line, and a daemon from before its lock held a
  build is `an older build`.
- The field's keys (↑, ↓, Tab, Esc) are handled on the card around the
  input, which tracks a focus handle of its own; Enter is the input's
  `submit`. The input is focused when the field opens and once more a
  moment later, since it is drawn only after the click that opened it.
- `corgi projects` is run after every key, one read at a time with the
  newest text last; an answer for text that has since changed is dropped.
  A `corgi` older than the command answers `unknown command`: say so.
- A shell is offered to `corgi open` only by what the app knows of its pane
  (a shell program, no agent status); Corgi decides, on hc's word, whether
  that shell really waits at its prompt in the project's primary checkout.
  Never start a supervisor by typing into a pane from the mod.
- `herdr-app check` renders the first frame, where the field is closed and
  the window has no agents. To check the field and the agent rows, check a
  scratch copy of the mod whose `init` opens the field and whose `render`
  adds rows made from made-up agents; `node scripts/test-app-mods.mjs` in
  the Corgi repository runs both mods' other views with stand-ins.
- None of those shows what the window draws. In the Corgi repository,
  `scripts/app-preview start` opens a window of the checkout's own, with
  these mods and made-up projects on a herdr-core of its own, and
  `scripts/app-preview shot <name>` saves a picture of it (macOS).

## Done when

- `herdr-app check mods/corgi` and `herdr-app check mods/projects` pass.
- After `herdr-app/install.sh`, `layouts/corgi.json` is there and differs
  from `layouts/main.json` only in the sidebar's two blocks; the window
  still shows the layout it showed; after `herdr-app do layout.switch
  '{"name":"corgi"}'` the sidebar is one list titled Projects, with no
  Agents list under it.
- Projects shows each repository with its agents in one-line rows (task,
  icon, state, worktree), the supervisor first; in a sidebar too narrow for
  a row, its worktree's name is cut with `…` and then gone before its task
  is cut, and the icon and the state are whole; a click on a row shows that
  agent's terminal; `~`, other folders and a worktree with no agent still
  have their rows; the narrow rail shows a cell per space and `»` unfolds
  it.
- A worker tagged MERGE has the mark in its row and no other row has one;
  a click on it asks under the row without showing the agent's terminal;
  `Cancel` puts the question away; `Merge + push` shows `Merging…`, then
  `<worker>: merged` with Corgi's message, the commit is on the base
  branch's remote, the mark is gone, and the supervisor's inbox holds the
  merge's line. A merge that conflicts shows `conflicts` and that the
  supervisor was asked; a refused or failed one shows why, and `Ask
  supervisor to fix` only while the project's supervisor runs, which puts
  one line in its inbox.
- The last line of the list reads `Corgi <version> · <commit>`, the commit
  `corgi --version` prints; after a branch is merged into the checkout that
  `corgi` was built in, it adds `· main is newer` and `cd <checkout> &&
  CORGI_BUILD=source herdr-app/install.sh`, and after that command has run
  it shows the new commit alone; with the binary rebuilt and the daemon
  not restarted, it warns that the daemon runs another build, and with the
  daemon stopped, that it is not running, each with `corgi daemon install`.
- `+`, then typing, narrows the known projects and completes a path; Enter
  on a project whose supervisor runs shows it; Enter on one without starts
  its supervisor, in the shell that waits in its primary checkout when
  there is one (a worker spawned afterwards is a tab of that space or nests
  under it, and `corgi fleet` lists both), else in a new space; a new name
  creates the directory beside the other projects and starts its supervisor
  with no folder-trust question left open.
- With `layouts/main.json` changed (in a scratch app home), the next start
  of either mod rebuilds `corgi.json` from it; with its sidebar's blocks
  gone, `corgi.json` stays as it was and the overview says why.
- `herdr-app/install.sh` makes no space, and after it `corgi daemon` makes
  none when the window opens, even with an old `app-space.json` in Corgi's
  state folder, which is gone once the daemon has started. `corgi
  app-space`, run by hand, puts a `Corgi` space first in the list with the
  overview as its only tab, and run again leaves one.
- Clicking an agent's card in a project's view shows that agent's
  terminal.
- The overview shows `Corgi daemon running · pid N` while `corgi daemon`
  runs, and the warning with `corgi daemon install` when it does not.
- With `corgi` installed and supervisors running on herdr-core, the overview
  lists each project with its supervisor's state, worker counts by state, open
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
