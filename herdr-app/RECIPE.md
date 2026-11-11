# Corgi

One mod, `corgi`, and a change to your layout. Each block of the layout that shows the mod shows one of its
views, named in the block's props (`{"mod": "corgi", "props": {"view": "projects"}}`). The **Projects** list is a sidebar list that stands in your layout where
Herdr has its Spaces and Agents lists: your spaces, each repository with its agents under it, two lines each
(the agent's task; under it, smaller, its harness's icon, its model, how
full its context is and, at the right, its state as a badge: `WORKING`,
`BLOCKED`, `DONE`, `IDLE`, `PAUSED`, `EXITED`), and a `+` that opens a project: type a name or a path, and Enter shows
its supervisor or starts it. A worker Corgi tagged ready to merge has a
`MERGE` badge in its row, in place of its state's: click it, confirm, and its branch is merged into
the project's base branch and pushed (`corgi merge`);
then it offers to close the worker and remove its worktree, keeping the
branch (`corgi close`). A red `↓3` after a space's name says its
checkout is 3 commits behind its upstream: click it to pull them. At the
top, above the list's header, a card counts your agents by state (HERD) and shows how much of
each Claude and Codex plan's 5-hour and weekly windows is used. `⏸ Agents`, left of the `+`, is for leaving the
Wi-Fi or VPN: it stops the agents that are working at that moment, each
ending its turn and exiting with its conversation saved, and while any is
paused Corgi types nothing into a supervisor and hands none over; it then
reads `▶ Agents`, which brings exactly those back. The
last line says which Corgi runs (`Corgi 0.2.3 · 5b50895`) and whether a
newer one has been merged since.

The **brand row** (`"view": "brand"`) draws the row at the top of the sidebar: Herdr's logo
and "Herdr", a dim `+`, then Corgi's pixel corgi and "Corgi", so the window
says it is Herdr with Corgi. Its search and sidebar buttons are Herdr's.

A block with no view shows the Projects list; one naming a view this Corgi
lacks says so in its block. The views' copies share what Corgi says: `corgi
status` and `corgi usage` are read once for every block that shows the mod.
They need the `corgi` command-line tool; `herdr-app/install.sh` in the
Corgi repository installs the tool and the mod, and puts the Projects
list and the brand row into the layout the window shows. A layout of your own gets it in
place; Herdr's own `main` is cloned to `corgi` first, which the window
switches to (`"main"` shows Herdr's own again). Nothing else in the layout
changes, so another mod's changes to it (a panel, a tab bar) stay. Corgi
had a mod per view before (`projects` and `corgi-brand`): the install makes
their blocks the one mod's in each of your layouts, then removes their
folders. The overview mod an earlier Corgi showed in a space of its own,
also in `mods/corgi`, is retired, since Corgi's place in the app is the
Projects list: the install replaces one an earlier install left.

<!-- preview.png is still to be made, from a look at the running window. -->

## What you see

### The Projects list in the sidebar

In the layout (yours, or `layouts/corgi.json`, listed as "Corgi", cloned
from Herdr's `main`), the sidebar's block that showed Herdr's `spaces` mod
shows the `corgi` mod's `projects` view, and the block that showed its
`agents` mod is gone, so in `main`'s sidebar the one list takes the whole
height between the brand row and the footer. The brand row's block shows
its `brand` view; the footer stays Herdr's. Everything else is as it was
(see "The layout").

### The brand row

The `brand` view fills the sidebar's `brand` block (46 px, on
`chrome`, text 13.5 px), as Herdr's `workspace-chrome` did:

- From the left: Herdr's logo (`frame/welcome/herdr.png`, 20 px, radius
  5), "Herdr", a `+` in `dim`, Corgi's logo (`corgi.png`, 20 px, radius
  5: a finer pixel corgi, 32 by 32 cells of 5 px, alone on a transparent
  160 px canvas as Herdr's and centred, its jaw a smooth curve edged in
  warm grey `#b0aa9e` so the white face keeps the same outline on a light
  sidebar as on a dark one; `cargo brand-tile` draws it again from `src/brand_tile.rs`, and
  a test fails while it lags the corgi) and
  "Corgi", the two names semibold in `foreground`, 5 px apart; then the
  search button (opens the `picker` mod) and the sidebar toggle, 26 px
  each, `muted`, `hover` behind.
- On macOS, out of full screen, it starts 76 px in, past the traffic
  lights; elsewhere 14 px.
- Narrowed, only the words give way: "Corgi" is cut with `…` and then
  gone before "Herdr" is cut at all, then "Herdr" is cut with `…`; the two
  logos and the `+` are never cut. On macOS beside the traffic lights the
  whole row needs a sidebar of about 280 px (Herdr's own 268
  cuts "Corgi"); under about 220 px, where the logos and `+` would no
  longer fit whole, Herdr's logo shows alone. Without the traffic lights
  (full screen, other systems) every sidebar wider than the rail has room
  for both logos.
- The rail (a sidebar under 160 px) shows what Herdr's row shows: on macOS
  nothing (the traffic lights fill it; the Projects list's `»` unfolds
  it), elsewhere the sidebar toggle alone. The toggle folds the sidebar to
  the rail (76 px on macOS, 44 elsewhere) and unfolds it to the width it
  had (`sidebar.width`, else 268), and sets `sidebar.collapsed`.

The `projects` view fills its block on `chrome`, text 13 px:

- **Herd card** (the first thing in the mod, right under the brand row and
  above the header, 4 px above it and 2 px below; not in the rail, and not
  at all while there are neither agents nor plans): a card (`surface`, a
  `border` outline, radius 7, text 11 px), its outline and titles in the
  theme's `border` and `dim` whatever the agents' states, a 20 px line
  each, every title (`HERD`, `CODEX`, `CLAUDE`) 10 px semibold in a 50 px
  column:
  - `HERD`: per state with agents, the rows' badge with its count
    (`WORKING 3`, `BLOCKED 1`, `MERGE 1`, `DONE 1`, `IDLE 2`, `PAUSED 1`,
    `EXITED 1`, in that order, 4 px apart); a state with none left out,
    `no agents` in `dim` when none has any. Counted over the list's agents
    by the badge each row shows: a
    worker tagged ready to merge counts as MERGE alone (unless it is
    paused), an agent in no state as idle. The badges wrap whole onto more
    lines, 3 px apart, in the room right of the title, and the line grows
    with them: with all seven states two lines at 357 px and three at
    Herdr's own 268; no badge is ever cut. Each count's tooltip says it
    (`1 blocked`).
  - A line per harness with a plan (`corgi usage --json`): `5h` and `7d`,
    each in `dim`, a 34 px bar 4 px high on `border` filled to the use, the
    percentage semibold, both in `ansi2` under 60 %, `ansi3` under 85 %,
    `ansi1` above, and the time to the reset in `dim` 10 px (`2h 14m`, `3d
    14h`), left out whole in a sidebar under 330 px (Herdr's own 268), where
    the tooltip says them instead. The line's tooltip: the plan and how old
    the reading is (`Max · read 2m ago`), and why the last reading failed. With no reading, `loading…`, or `unavailable` once one failed.
- **Header** (31 px): `Projects` in `dim` 11.5 px and, 4 px apart on the
  right, the pause button and a 26 px `+` button (tooltip `Open project`),
  lit while its field is open. The pause button is drawn as the `+`: 26 px
  high, radius 6, no fill and the header's `dim`, `hover` behind it and
  `foreground` on hover; a 14 px icon (the mod's `pause.svg`, two bars,
  stroked as the `+` is) and `Agents` in the header's 11.5 px. While agents
  are paused only its icon changes, to a play triangle (`play.svg`); at
  half opacity while a pause or resume runs. Its tooltip says what it does.
  Not drawn on the rail, nor from a `corgi` whose status has no `paused`.
- **Spinners**: what is running (a pause, a merge, a close, a supervisor
  starting, a pull, a working space in the rail) shows braille frames
  (`⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏`, one every 90 ms, `.frames`, natively and only while
  shown) in `working`, the glyph 18 px in a fixed 12 px box (10 px with a
  16 px glyph for the pull), so nothing beside it moves. Never a turning
  svg (`.spin`): the app turns one at about 10 fps while the window is
  quiet, which reads as lagging. The rows and the herd card have none.
- **Pause progress and result** (under the header, 8 px in from each side,
  a card on `surface`, radius 7, padding 7, 11.5 px): a spinner and
  `Pausing 3 agents…` (or `Resuming 2 agents…`) with a `border` outline
  while it runs; nothing once it went through. When agents were left
  behind, a `blocked` outline, `Not paused:` (or `Did not come back:`) in
  `blocked` semibold, one wrapping line per agent, `<name>: <why>`, and
  `Dismiss` (`raised`). A refusal (`hc` not running) shows `Nothing was
  paused` and Corgi's message.
- **Open project** (only while open, under the header): a card (`surface`,
  a `border` outline, radius 9) holding a text field (30 px, an `accent`
  outline, placeholder `Project name or path…`); up to eight suggestions,
  26 px each: the project's name, its folder from `~` in `dim` 11 px, cut
  with `…`, and on the right where it is known from in `dim` 10.5 px (`2
  agents`, `open in Herdr`, `nearby`, `recent`, `typed path`), `running` in
  `working` when its supervisor runs, or `new project` in `accent`; the
  highlighted one on `selected`; `N more: keep typing` when more match; then
  `Starting <name>'s supervisor…` with a spinner while it starts, or why it
  did not in `blocked`; and the keys in `dim` 10.5 px, `Enter opens`, `Tab
  completes`, `Esc closes`, each kept whole and wrapping to a second line
  when the sidebar is narrow (at its own 268 px `Esc closes` goes there),
  with `New space` on their right.
- **The list** (scrolls): the spaces in order, under a header per machine
  once one is not this computer, as Herdr's Spaces list has them. Under
  the header of a connected machine with no `corgi`, one line, 10.5 px in
  `dim`, cut with `…`: `Corgi isn't installed on <machine>`; its rows show
  as an agent started by hand shows, with no second-line data or `MERGE`.
  - A space outside any repository (`~`, another folder):
    30 px, a colored 18 px avatar with its initial, its name.
  - A project, a repository: the space on its primary checkout, drawn the
    same way, with its branch and ahead/behind after the name only when
    unusual: `↑N` in `ansi2`, `↓N` in `ansi1`. The `↓N` is a button
    (pointer, a faint `ansi1` tint on hover, radius 4, tooltip `Pull 3
    commits from origin/main`); while it pulls it is dimmed with a 10 px
    spinner before it, and so is every other `↓N`. A pull that failed
    puts a card under the row, indented 22 px (`surface`, a `blocked`
    border, radius 7): git's reason in plain words and `Dismiss`. Under it, indented 22 px, one 38 px row per agent in it and in
    the spaces on its worktrees, on two lines. The first (18 px): the task
    in 12.5 px (medium weight for `Supervisor`), the line's whole width,
    cut with `…`. The second (16 px), in `dim` 11 px, 5 px apart from
    the left: the harness's icon (12 px; Claude's now, Codex's or
    OpenCode's when it is one), the model (`Opus 5.5`, cut with `…` if the
    line runs out), a `·`, and the context fill (`33%`), from `model` and
    `ctx_pct` in `corgi status --json`. What is not known is left out with
    nothing in its place (no `·` without both): an agent started by hand
    shows its icon alone. At the right end of the second line, the state's
    badge, still (no animation), 10 px semibold, 5 px padding, radius 4:
    `WORKING` on `working`, `BLOCKED` on `blocked`, `DONE` on `done`,
    `PAUSED` on `muted` (an agent paused by `⏸ Agents`, tagged or not),
    each with near-black text on a light fill and white on a dark one;
    `IDLE` in `muted` on a faint `muted` tint, the state at rest; `EXITED`
    (an agent Corgi says exited) a `blocked` outline with `blocked` text.
    An agent in no state shows none. On a worker tagged ready to merge, the
    `MERGE` mark is its badge, in place of its state's (`accent_foreground`
    on `accent`; tooltip `Merge worktree/x into main and push`). A paused
    worker whose terminal ended with its agent shows its `PAUSED` badge
    alone. The worktree's folder name
    (`worktree-calm-forest-4f96`; nothing on the primary checkout) is not
    drawn; it is the row's tooltip. The supervisor is first; the rest keep
    the app's order. The badge, the icon and the fill stay whole.
  - Under a worker's row, while its merge is asked about, runs or has
    ended: a card indented like the row (22 px; `surface`, radius 7,
    padding 7, text 11.5 px, its text wrapping) with a `border` outline
    while it asks or runs, `ansi2` (green) for merged and for a supervisor
    that was asked, `ansi4` (blue) for a conflict handed to the supervisor
    (nothing left for the person), `ansi3` (yellow) for a failed push,
    `blocked` otherwise.
    It holds
    `Merge worktree/x into main and push?` with `Cancel` (`raised`) and
    `Merge + push` (`solid`); then a spinner and `Merging…`; then
    `<worker>: merged` (or `conflicts`, `conflicts, handed to
    <supervisor>`, `push-failed`, `refused`) in the
    outline's color, semibold, Corgi's message, and its buttons, 22 px
    high: `Pull, merge + push` or `Merge + push without fetching` when
    Corgi offers it, `Ask supervisor to fix` when it failed and the
    project's supervisor runs, and `Dismiss`.
    After `merged` it asks, under Corgi's message, `Close <worker> and
    remove its worktree?` with `Keep` (`raised`) and `Close` (`solid`, with
    a 1 px outline in `solid` that turns `foreground` while it holds the
    keyboard); then a spinner and `Closing…`. A close that went through,
    and `Keep` (or Escape), put the card away at once, with nothing to
    dismiss. A close refused
    for changes in the worktree turns the outline `blocked` and shows
    Corgi's message (which names them, paths under the home folder as
    `~/…`) in `blocked`, then `Close <worker> anyway and discard them?`
    with `Keep` (which puts the card away too) and `Close anyway`
    (`solid`); a close that failed shows its
    message in `blocked` on a `blocked` outline, and `Dismiss`. If the
    worker's row goes with its space while the card is still up, the card
    stands after the project's last agent row instead.
  - A worktree's space with no agent in it keeps a 28 px row of its own
    under its project: a branch glyph and the space's name.
  - A space outside a repository with agents in it has their rows under it
    the same way.
  - One row at most is on `selected`: the agent with the focus, alone. A
    project's own row (a repository's space) is never lit, not even with
    the focus on its supervisor or on a shell in its space (then nothing
    is). A space outside any repository is lit while it is shown and none
    of its agents has the focus; a worktree's space with no agent row is
    lit while shown; for a shown worktree space whose focus is on no agent,
    its first agent is.
- **Which Corgi runs** (under the list, 16 px from the left, `dim`
  10.5 px): `Corgi 0.2.3 · 5b50895`, the release and the commit the `corgi`
  that answers was built from, with `· modified` for a build with
  uncommitted changes, and the release alone for a build without Git. When
  the checkout it was built in has commits the build lacks, `· main is
  newer` (the branch checked out there) in `ansi3`, and under it, in
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
  state in it (the braille spinner, the `!`, a dot for done), and `»` at the bottom.

## How it behaves

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
  the app's live agents change (compared once a second), never more often.
  Every copy of the mod (each block showing it) shares that read, and plan
  usage and `hc machine ls` with it (`data.js`): the copies' timers take
  turns, so a tick that finds a read begun on another copy's tick since its
  own last one reads nothing, and `corgi status` runs once a period however
  many blocks show the mod. The other copies draw the answer on their next
  second's tick. A read whose copy closed under it counts as gone after 45
  seconds.
  Each space's git is read as Herdr's Spaces list
  reads it. Without a `corgi` it still lists every space and agent, by
  their own titles.
- Another machine (added with Herdr's `Add machine`, reached by herdr-core
  over ssh) has its own Corgi, which the list asks there with `exec(…, {
  on: <machine> })`: each machine that `hc machine ls` says is
  `connected`, every 5 seconds at most, and never sooner than three times
  its last read took, so a slow machine is asked less often; a change in
  the live agents reads it again only once that time has come. Its
  `corgi` is found once, by a `sh -c` there: the path that machine's own
  install wrote in its app's `mods/corgi/config.json` (or, by an older
  install, `mods/projects/config.json`), `corgi` on the `PATH` exec has there, on its
  login shell's `PATH` (`$SHELL -lc`), then `~/.local/bin/corgi`,
  `~/.local/share/corgi/target/release/corgi` and `~/.cargo/bin/corgi`.
  Found, it is kept until a read fails; with none, the header says so (see
  above) and it is looked for again a minute later. Corgi there names its
  own core's terminals (`t3`); the list puts each under the app's address
  for it (`lab/t3`), and keeps each project by machine and folder, so two
  machines' projects in the same folder stay apart. A machine that is away,
  not connected or removed has no data: its rows show without it until it
  answers again. Its `daemon` and `corgi` fields are not read: the last
  line is this computer's, and so are the plan lines (plans are per
  account, not per machine). The herd card counts every agent the list
  shows, on every machine, by the app's live state; another machine's
  tagged worker counts as `MERGE` and its paused agent as paused once its
  Corgi has answered. Each machine's own `corgi daemon` wakes its own
  supervisors.
- `+` (or the command `corgi.open`, for a key such as `"cmd-o":
  "corgi.open"` in `keys.json`; it was `projects.open` while the list was a
  mod of its own) opens the field with the keyboard in it,
  and again puts it away; on the rail it unfolds the sidebar first. Opened,
  and after every key, it runs `corgi projects --json --query <text> --open
  <folder>…` with the folders its spaces on this computer are in: Corgi's
  known projects (those with agents, those open, the Git repositories
  beside them, those it remembers; a plain folder beside them is offered
  only when typed) narrowed by every word typed; for an absolute
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
- `⏸ Agents` is for leaving the network (a Wi-Fi or a VPN): an agent in the
  middle of a turn without one fails. A click pauses at once, with no
  question, the agents of Corgi's that are working now (by the app's live
  status, else Corgi's), on every machine whose Corgi answered and knows a
  pause: on each machine with one working, at the same time, it runs
  `corgi pause --json` there, which presses Esc in
  each (ending its turn), then types its harness's exit (`/exit`), so its
  conversation is saved; a supervisor's kept shell stays, a worker's
  terminal ends. Idle, done and blocked agents are left as they are. With
  none working nothing changes: no `corgi pause` runs, the button stays,
  and the app's toast says `No agents are working` and goes by itself.
  Nothing is killed: an agent that keeps working, has text in its input
  box, or does not exit is left running and listed in the card, as is one
  Corgi could not resume (no resume line: Codex and OpenCode on hc).
- While any agent is paused, Corgi's daemon types nothing into any
  supervisor (the paused ones and the others alike) and moves no handover
  on, so nothing starts a turn while you are offline; a worker's stop still
  adds its wake to the inbox, and those wakes go out after Resume. A
  paused agent's own stop wakes no one, and the daemon does not resume it
  after a core restart. The pause is kept in Corgi's state
  (`pause/<socket>.json`), so it outlasts a restart of the app, the core or
  the daemon.
- In the card, an agent of another machine's is named with it (`w-api on
  lab: did not exit; left running`); a machine whose `corgi` refused while
  another paused is a line of its own (`Agents on lab: <why>`). The button
  shows `▶ Agents` while an agent is paused on any machine.
- `▶ Agents` runs `corgi resume --json`, on each machine with an agent
  paused: each paused agent comes back on its
  own conversation, a supervisor in its shell, a worker in a new terminal
  with its name, labels and space (its ended one removed). One that does
  not come back is listed in the card, and holds nothing back any more.
  The status is read again after either.
- `MERGE` is on a worker whose `tag` in `corgi status --json` is `merge`:
  Corgi's supervisor tags a worker whose branch is ready to merge straight
  into the base branch. A project that merges through merge or pull
  requests is not tagged, so it has no mark. A click on the mark asks under
  the row (and is not also the row's click); `Cancel`, or the mark again,
  puts the question away. `Merge + push` runs `corgi merge <worker>
  --project <dir> --json`, on the worker's machine (its project and base
  from that machine's status): the merge, with
  its checks, in the project's primary checkout, pushed, which also clears
  the tag and tells the supervisor in its inbox. One merge at a time: while
  one runs the marks are dimmed and do nothing. Then the status is read
  again.
- The result stays under the row until `Dismiss`: `merged`; `conflicts`,
  which Corgi has aborted and handed to the project's supervisor when one
  runs (`supervisor` in its answer names it, and the card says it was
  handed over), and left in the primary checkout to resolve when none does; `push-failed`, the branch merged but not pushed;
  `refused`, with why. Refused because the base branch's upstream has new
  commits, it offers `Pull, merge + push` (`--pull`); after a failed fetch,
  `Merge + push without fetching` (`--skip-upstream`).
- After `merged` (merged and pushed; never after conflicts, a refusal or a
  failed push) the card asks whether to close the worker. `Close` is the
  default: it takes the keyboard as the question shows, so Enter closes and
  Escape keeps. `Close` runs `corgi close <worker> --project <dir> --json`
  on the worker's machine: it ends the worker's hc
  terminal and removes it, by its address, then removes its worktree with
  `git worktree remove`; the branch stays. The app closes the worker's
  space once its last terminal is gone (a terminal of the person's in it
  keeps it). A worktree with uncommitted or untracked changes is refused
  (`reason` `dirty`, `dirty` listing them) and nothing is closed: `Close
  anyway` runs it again with `--force`, which discards them. A close that
  went through (`closed`) ends the flow: the card goes at once and only a
  refusal or a failure is shown. `Keep` (Escape, or `Keep` after a dirty
  refusal) ends it too: the card goes at once and nothing is closed. After
  a close the status is read again. `corgi close` refuses a supervisor and
  an agent Corgi did not start.
- `Ask supervisor to fix` is offered on a failed push, a failed fetch, and
  a merge that failed or was refused before it started (uncommitted changes
  in the primary checkout, say): whenever Corgi's answer has an `error`,
  and the project's supervisor is running (`supervisor` in the status of
  the worker's machine, not `exited`). It runs `corgi merge <worker>
  --project <dir> --json --ask-supervisor <error>` on that machine, with
  `--commit <commit>` after a failed push: a line in the supervisor's
  inbox, which that
  machine's `corgi daemon` types in. The card then says who was asked;
  if nobody could be, why, with the button still there.
- The herd card's counts come from the list's own agents; its plan lines
  from `corgi usage --json`, read once the first status answers and then
  every minute. That command reads the plan-usage cache every Corgi process
  shares (`usage/<provider>.json` in Corgi's state directory), so the
  providers are asked at most once a minute on the machine however many
  windows and lists read it; a corgi too old to know `usage`
  leaves the plan lines out.
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
- `↓N` comes from `git status --porcelain=v2 --branch` (`# branch.ab`,
  `# branch.upstream`), against the upstream as last fetched: the list
  never fetches by itself. A click on it runs `git pull --ff-only` in that
  row's checkout (the space's folder: a project's primary checkout, or a
  worktree's own for a worktree's row), with no question first, and
  nothing else: no merge, rebase, stash or reset. It never asks for a
  login, which it has no terminal to ask in: it runs from `sh -c` with
  `GIT_TERMINAL_PROMPT=0` and `GIT_SSH_COMMAND` set to the person's own
  (`GIT_SSH_COMMAND`, else `core.sshCommand`, else `ssh`) plus `-o
  BatchMode=yes` (`exec` takes no environment), so a pull that needs one
  fails at once. One pull at a time,
  since worktrees of a repository share its remote branches; a click while
  one runs does nothing. Then the checkout's git is read again. Pulled: the
  `↓N` goes and nothing more is shown. Failed: the card says why, from
  git's own words: `Can't fast-forward: main and origin/main have
  diverged`, `Local changes would be overwritten: <files>`, `Untracked
  files would be overwritten: <files>`, `Couldn't reach origin: <git's
  line>`, `Needs a login: run git pull in a terminal once` (prompts
  disabled, `Permission denied (publickey)`, an unknown host key), else
  git's first `fatal:`/`error:` line. It stays until
  `Dismiss`, or until the checkout is no longer behind. `↑N` is only shown:
  nothing pushes. A worktree's space with agents in it has no row, so no
  `↓N`.
- `»` on the rail unfolds the sidebar (Herdr's own layout has it in its
  Agents list).

## Built from

- The mod, `mods/corgi/` (`mod.json`: `"place": "none"`, `"title":
  "Corgi"`). `main.js` reads the block's `props()` and hands its init and
  render to the view it names (`projects`, the default, or `brand`; any
  other draws one line saying this Corgi lacks it). The views' modules:
  - `projects.js`, the Projects list. It began as a copy of Herdr's
    `spaces` mod (herdr-app 0.1.14) and is Corgi's own since: it follows no
    later version of it. Imports: `div`, `image`, `svg` from `gpui-kit`;
    `v_flex`, `h_flex`, `InputState`, `Input` from `gpui-base`; `spaces`,
    `machines`, `panes`, `agents`, `focused`, `run`, `openOverlay`,
    `exec`, `hc`, `theme`, `shared`, `setShared`, `setAgentOrder`,
    `watchFolder`, `window`, `home`, `nextCommand`, `notify` from `herdr`.
    App commands: `space.select`, `space.move`, `space.new`,
    `agent.focus`, `layout.resize`; its own command `open` (`corgi.open`).
    It opens Herdr's `menu` mod as a popup and draws Herdr's icons
    (`mods/workspace-chrome/*.svg`, the programs' `icons/`) and its own
    `pause.svg` and `play.svg`. Programs: `git` (status and `rev-parse`,
    per space), `corgi projects --json …`, `corgi open … --json`, `corgi
    merge … --json` (with `--pull`, `--skip-upstream`, or
    `--ask-supervisor <error> [--commit <commit>]`), `corgi close`, `corgi
    pause`, `corgi resume`, and `sh -c` for a pull. On another machine,
    through `exec`'s `on`: `git`, the pull's `sh -c`, and that machine's
    `corgi`'s `merge`, `close`, `pause` and `resume`; `corgi projects`,
    `open` and `usage` run on this computer only.
  - `brand.js`, the brand row (with `corgi.png`, and `search.svg` and
    `sidebar.svg`, copies of Herdr's). It began as a copy of the brand row
    of Herdr's `workspace-chrome` mod (herdr-app 0.1.14) and is Corgi's own
    since: it follows no later version of it. Imports: `div`, `image`,
    `svg` from `gpui-kit`; `h_flex` from `gpui-base`; `theme`, `run`,
    `window` from `herdr`. App commands: `mod.open` (`picker`),
    `layout.resize`. No programs or files.
  - `data.js`, what Corgi says, read once for every copy: `corgi status
    --json` and `corgi usage --json` here, `hc machine ls`, and on another
    machine the `sh -c` that finds `corgi` there and its `status`. Files:
    `mods/corgi/config.json` (`{ "corgi": "<path>" }`, written by
    `herdr-app/install.sh`).
  - `ui.js`, what the views draw alike: the sidebar's width and rail, the
    spinner, a state's badge, one-line text.
- The layout: no file of this folder, but the one change `corgi
  app-layout` makes to the app's own (see "The layout").
- `corgi projects --json` prints `{query, parent, projects: [{dir, name,
  badge, new, supervisor}]}`; `corgi open … --json` prints `{result: started
  | running | failed, message, project, name, terminal, created, place: new
  | own | shell}`; `corgi app-layout --json` prints `{result: edited |
  cloned | unchanged | refused, layout, path, upgraded, message}`
  (`upgraded`: the other layouts whose blocks of Corgi's old mods were made
  the one mod's).
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

## The layout

- `herdr-app/install.sh` runs `corgi app-layout --app <herdr-app> --home
  <app home>` after the mod is in place. It takes the layout the
  window shows (`herdr-app status`; with the window closed the one it
  showed last, `layout` in `shared.json`, else `main`), or the one named
  with `--layout NAME`.
- The change: in the node `sidebar` (or the whole layout, when it has no
  such node), the one block whose `content.mod` is `spaces` shows the
  `corgi` mod's Projects list (`content` `{"mod": "corgi", "props":
  {"view": "projects"}}`; it keeps its id `spaces`, its size and
  everything else), the blocks whose `content.mod` is `agents` go, and the
  block `brand` shows the mod's brand row (`"view": "brand"`, beside any
  props it had) when it shows `workspace-chrome` (a layout with no
  `brand` block, or one showing another mod, keeps it as it is; the
  footer, `sidebar-footer`, stays `workspace-chrome`). Nothing else is touched:
  herdr-gitlab's `gitlab` block in `panel-body` and its `tabs-gitlab` tab
  bar, say, stay as they are, whichever was installed first. The file is
  written in one rename, and only when it changes.
- A layout of the person's own (not `layout:<name>` in the app's
  `installed.json`) is changed in place. A shipped one (`main`), which
  Herdr replaces whole on every start, is cloned to `corgi` first: with
  the window open by `herdr-app do layout.clone '{"name":"main","as":
  "corgi"}'`, which keeps its clicks and switches to the clone; with it
  closed by a copy recording `cloned_from: {name, version}` as the app's
  clone does, and the command says how to switch
  (`herdr-app do layout.switch '{"name":"corgi"}'`). A `corgi` layout that
  is there already is used, and switched to, instead.
- Corgi's old mods: a block anywhere in the layout that shows `projects`
  or `corgi-brand` shows the `corgi` mod's view of the same (`projects`,
  `brand`) instead, keeping the rest of the block, and so does every other
  layout of the person's own that names them (`upgraded`), with nothing
  else changed in it; the installer removes their folders only after.
- Run again, it changes nothing: a sidebar that shows the Projects list
  (the `corgi` mod with view `projects`, or no view) keeps its lists as
  they are, even with an Agents list put back by hand; a layout Corgi
  changed before it had a brand row gets the brand row then. Corgi never builds
  a layout from `main` again and does not follow Herdr's later changes to
  it.
- A sidebar with no `spaces` block and no Projects list, or with more than
  one `spaces` block, is not guessed at: nothing is changed or cloned, and
  the command says why (`--json`: `refused`, exit 0).
- `corgi app-layout remove` (run by `herdr-app/install.sh --uninstall`,
  before the mod goes) puts Spaces and Agents back in each of the
  person's layouts that shows the Projects list or the brand row, of the
  `corgi` mod or of its old `projects` and `corgi-brand`: the Projects
  list's block shows `spaces` again, an `agents` block (the one in Herdr's
  `main`) follows it when the layout has none, and the brand row's block
  shows `workspace-chrome` again (without the `view` prop, and without
  `props` when that was all). No layout is deleted (`corgi` stays, as the
  person's own) and the window is not switched.

## Watch out

- `corgi status` reads herdr-core for running agents, and only when its
  socket accepts a connection: any `hc` call starts a stopped core, with the
  caller's environment, and the mod must never be the one that starts it.
  Without it the projects still show from the state folders, with no
  agents.
- A `.when()` callback must return the element it got, and text for one
  line must have its newlines joined (tasks can hold them).
- The mods run only while the window is open, so they deliver nothing to
  supervisors: that stays with Corgi.
- Never edit Herdr's shipped `layouts/main.json` or its `spaces` and
  `agents` mods: Herdr replaces them whole on every start and moves an
  edited one aside. That is why a shipped layout is cloned to `corgi`
  before Corgi changes it.
- The Projects list is one list standing where two stood, so what only
  Herdr's Agents list did, it must: `setAgentOrder` on every render (or
  `agent.next` and `agent.prev` follow no list), and the `»` that unfolds
  the rail (on macOS the brand row has no room for its toggle there).
- An agent's row is found by its terminal's address in `corgi status
  --json`. Before the first read, and with no `corgi`, every agent shows by
  its own title; a row must never wait for Corgi.
- A row is two `h_flex` lines in a `v_flex` of a fixed height, laid out
  by flex alone, nothing measured. On the first, the task is `flex_1` with
  a minimum width of 0 and truncates, so it fills the line and the state
  (`flex_none`, 14 px wide) stands at the right edge whatever the task's
  length. On the second, the model truncates with a minimum width of 0;
  the icon, the `·`, the fill and the mark are `flex_none`, and a `flex_1`
  spacer before the mark pushes it to the right. A `\n` in a task starts a
  second line even when truncated: join the lines first.
- The `MERGE` mark and the card's buttons sit inside or beside a clickable
  row: each calls `cx.stop_propagation()` in its `on_click`, or the row's
  click shows the agent's terminal too. The card is an element of the list
  after its row, not a child of the row, which stays 38 px high.
- The `↓N` sits in a row that selects its space on mouse up and drags on
  mouse down, not on click: so the mark takes both itself
  (`on_mouse_down` and `on_mouse_up`, each with `cx.stop_propagation()`),
  and pulls on a release that follows its own press. A drag let go over it
  is still the row's.
- Never run `corgi merge` from a test or a look at the mod: it merges and
  pushes for real. `node scripts/test-app-mods.mjs` gives the mod canned
  answers for it; Corgi's own tests merge in scratch repositories.
- `Ask supervisor to fix` passes Corgi's `error`, not its `message`: the
  message is written for the person (`Merge unavailable: …`, `merge again
  with --skip-upstream`), the error is what the supervisor's line quotes. A
  conflict needs no button: `corgi merge` hands it over itself.
- The card's and the footer's colors are not the agent states': a theme's
  `working` is amber and its `done` is whatever a finished agent's dot is,
  so "merged" in `working` read as a warning. What went well is `ansi2`,
  what landed but needs a look `ansi3`, what needs the person `blocked`.
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
  adds rows made from made-up agents (for the brand row, a copy whose
  `main.js` defaults to `brand`); `node scripts/test-app-mods.mjs` in
  the Corgi repository runs every view with stand-ins.
- A view reads its block's props with `props()` from `herdr`: `init`'s own
  argument is not them. Module scope is shared by every copy of the mod
  (each block showing it), which is where `data.js` keeps its one store;
  a copy's own state stays on its view.
- None of those shows what the window draws. In the Corgi repository,
  `scripts/app-preview start` opens a window of the checkout's own, with
  this mod and made-up projects on a herdr-core of its own, and
  `scripts/app-preview shot <name>` saves a picture of it (macOS).

## Done when

- `herdr-app check mods/corgi` passes, and so does a copy defaulting to
  the `brand` view.
- After `herdr-app/install.sh` on Herdr's `main`, `layouts/corgi.json` is
  there, records `cloned_from` and differs from `layouts/main.json` only in
  the sidebar's three blocks (`brand`, `spaces`, `agents`), and the window shows it: the sidebar is one
  list titled Projects, with no Agents list under it. On a layout of your
  own, that layout has the same change and no `corgi.json` is made. A
  second run changes no file.
- The sidebar's top row reads Herdr's logo, "Herdr", a dim `+`, the pixel
  corgi and "Corgi", with Herdr's search and sidebar buttons at its right;
  narrowed, the word "Corgi" goes first, then "Herdr", and the logos and
  `+` stay whole; the rail shows what Herdr's row shows
  there; the footer is Herdr's.
- Projects shows each repository with its agents in two-line rows (the
  task with its state at the right edge; then the icon, model and context
  fill, dim), the supervisor first; in a narrow sidebar the task is cut
  with `…` and the state keeps its place, and an agent without a model or
  fill shows neither, with no placeholder; a click on a row shows that
  agent's terminal and lights that row alone, never the project's row with
  it; `~`, other folders and a worktree with no agent still
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
- A project behind its upstream shows `↓N`; a click shows it dimmed
  with a spinner, then, fast-forwarded, the mark is gone and nothing else
  shows; a checkout that has diverged, or whose changes the pull would
  overwrite, shows a card saying so under its row, with `Dismiss`, and is
  left as it was.
- With herdr-gitlab installed before or after Corgi (in a scratch app
  home), the layout shows both the Projects list and the GitLab panel;
  `--uninstall` brings back Spaces and Agents and leaves the panel.
- `herdr-app/install.sh` makes no space, and after it `corgi daemon` makes
  none when the window opens. Run over an earlier install, it removes the
  retired Corgi mod (`mods/corgi`, when its `main.js` is Corgi's) and
  leaves every other mod.
