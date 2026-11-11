# Development handoff

Corgi is a tool for the Herdr app: the `corgi` binary (the command-line
tools and the daemon), its mod in `herdr-app/mods/corgi`, and the supervisor's
role in `supervisor/`. ARCHITECTURE.md says how it works and why.

After making a change, ensure it is visible in the running environment before
handing off. Rebuild, reload, or restart the affected component when needed,
then verify the change rather than assuming a source edit is live. A worker
in a worktree changes nothing the user runs: its build is the worktree's own
`target/release/corgi`, and "After a merge" below is what makes merged work
live.

## Testing changes

Before handing off Rust changes from any checkout, run:

```bash
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
TMPDIR=/tmp cargo test
cargo build --release
```

The crate is a binary alone (`src/main.rs` declares every module), so
nothing is exported and rustc's dead-code lint sees every item: code that
no command reaches fails clippy. Delete it, or, when only tests use it,
give it `#[cfg(test)]`. A `pub` field of a struct that derives `PartialEq`
is the one thing the lint does not see; look for readers yourself when you
remove the last use of one.

`tests/open.rs` and `tests/version.rs` run the real binary. A binary crate
has no library for them to import, so `tests/open.rs` includes
`src/test_support.rs` as a module of its own (`#[path]`): that file must
use std alone.

Clippy lints code under `#[cfg(target_os = ...)]` only on that OS, so a lint
can pass on your machine and fail CI on the other (a `return` left at the end
of a macOS-only block did). CI runs on both Linux and macOS; check both jobs.
CI also runs `node scripts/test-app-mods.mjs`, the Jenkins mod's tests and
`bash scripts/test-install` on both (on Node 26: the tests use
`node:module`'s `registerHooks`); `herdr-app check` needs the app, which a
runner lacks, so it stays a local check.

On macOS the default `$TMPDIR` makes the fake `herdr-app` socket in
`src/app_layout/tests.rs` exceed `SUN_LEN`, so
run `TMPDIR=/tmp cargo test` there.

A test that polls a non-blocking listener's `accept()` must set the accepted
stream back to blocking (`set_nonblocking(false)`) before relying on a read
timeout: macOS/BSD sockets inherit `O_NONBLOCK` from the listener, Linux
sockets do not, so the failure shows only on macOS CI.

`hc::tests::the_trust_question_is_answered_yes_only_once_yes_shows_and_a_lost_key_is_pressed_again`
times a fake `hc` script and can fail when the machine is busy (another
build, the whole suite beside it). Run it alone before taking a failure of
it for yours.

A waker test's agents, built with `project_agent`, have no `agent` kind, so
their harness is `Other`, which has no idle time for a handover. A test that
needs Claude Code's behaviour sets `info.agent = Some("claude".into())`.

`Handover::observe` times a rest from the first pass that sees it, so a
handover test must observe at second 0 before asserting at the idle time.

Saving the projects list (`ProjectMemory`) drops directories that do not
exist, so a test that saves it to a file must remember real directories
(made in its `ScratchDir`), not made-up paths such as `/repos/a`.

A test's Git repositories name their branch on both sides: `git init -b main`,
and `git init --bare -b main` for a remote. A plain `git init` follows the
user's `init.defaultBranch`, and a clone of a bare remote whose `HEAD` names
`master` checks out an unborn `master`, so pushing `main` fails there.

The supervisor's role and the `[Corgi]` lines Corgi types into its box
(wakes, handover requests, merge, conflict and push lines) are pinned by
tests in `src/supervisor.rs`, and the merge lines also by the exact-text
tests in `src/merge.rs`: change the wording, update both. `Inboxes::pending_for`
lists every line that waits; a newer line of the same key replaces an older
one only when they go out.

An error of `corgi merge` (`src/merge.rs`, and what `src/commands/status.rs`
makes of it) reaches two readers: the card under a worker's row in the
Herdr app's Projects list, and the supervisor, whose line quotes it when
the user asks it to fix the merge (`--ask-supervisor`). Word it for both:
what went wrong and what to do about it, without a key or a flag. Line
breaks in `supervisor/ROLE.md` and `supervisor/HANDOVER.md` matter, since
those tests match their text with `contains`. Another test there
(`the_role_is_a_third_shorter_than_before_the_on_demand_parts_left_it`)
caps `ROLE.md` at three quarters of 34,516 bytes, 25,887, and under 100 bytes
are left: a role addition must trim something else.

The daemon's records are read by the next daemon, and the inbox by every
Corgi: `AgentState`, `supervisor::Transitions`, `supervisor::Handover`,
`waker::Followed` and `inbox::Item` are written to disk with serde, so a
field's name there (`pane`, `change`) is a file format, not a name to tidy.

On macOS, copying a rebuilt binary over an existing file (say, a copy of
`target/release/corgi` kept to compare before and after) gets it killed with
exit 137, because the kernel caches the old file's code signature. `rm` the
destination before copying.

macOS has no `timeout` command, so a command wrapped in it (`timeout 600
cargo build`) silently never runs; behind a `grep` filter even the
`command not found` is lost.

`corgi digest`'s lookups (`--search`, `--show`, `--ledger`, `--brief`,
`--decision`) read the project's state directory alone and need no core.
Test them with a `ScratchDir` as the state directory through
`digest_lookup` in `src/digest/memory.rs`. To try them on real data, copy
the state into a scratch directory and point `XDG_STATE_HOME` at it; never
run a change against the real `~/.local/state/corgi`:

```bash
mkdir -p "$scratch/corgi/supervisors"
cp -R ~/.local/state/corgi/supervisors/<project> "$scratch/corgi/supervisors/"
XDG_STATE_HOME="$scratch" ./target/release/corgi digest <project dir> --search "<words>"
```

The digest's summary, `corgi fleet`, `corgi tag` and `corgi report` of a
running agent ask `hc` for the agents. `hc` finds its core from
`XDG_STATE_HOME` (and `HC_SESSION`), so with a scratch `XDG_STATE_HOME`
they do not list the user's agents: `hc` starts a core of its own on the
scratch directory, with no terminals, and leaves it running. To see them
work, give that core fake agents ("Testing `corgi daemon`" below has the
environment; `hc run --bg --keep --name w-x --label corgi.role=worker
--label corgi.project=<dir> --cwd <dir> -- /bin/sleep 900` is an agent
whose state is unknown), and stop the core afterwards.

`corgi usage` reads the plan usage through a cache shared by every Corgi
process (`usage/<provider>.json` in the state directory), which the Herdr
app's Projects list, on the installed binary, refreshes every minute. A
build of yours run against it takes that fresh entry and fetches nothing.
To check a usage change live, give it an empty scratch `XDG_STATE_HOME`:
`XDG_STATE_HOME="$scratch" ./target/release/corgi usage --json`. To check
the Claude reader by hand:
`TMPDIR=/tmp cargo test -- --ignored live_claude_usage --nocapture`.

`rust-version` in `Cargo.toml` is the oldest Rust that builds Corgi: 1.89,
set by std's `File::lock`. With it set, clippy's `incompatible_msrv` lint
refuses a newer std API, so raise it deliberately. To check an older
toolchain, build in Docker with the worktree mounted read-only and the
target directory inside the container (a session scratchpad under
`/private/tmp` is not visible to Docker on macOS):

```bash
docker run --rm -v "$PWD":/src:ro -w /src -e CARGO_TARGET_DIR=/tmp/t \
  rust:1.89-slim cargo check --locked --all-targets
```

`cargo brand-tile` draws the brand row's logo
(`herdr-app/mods/corgi/corgi.png`) again from `src/brand_tile.rs`; a
test fails while the committed file is not the one drawn there.

To see how Claude Code sizes a model's context window when a new model
ships, run the real CLI as `claude --bare` with a dummy
`ANTHROPIC_API_KEY`, a private `HOME` and `CLAUDE_CONFIG_DIR`, `--model
<id>`, `ANTHROPIC_BASE_URL` at a small local fake of `/v1/messages`
(streaming; it answers a canned turn with a chosen `usage`), and a
`statusLine` command that saves its stdin: send one prompt, then read
`context_window.context_window_size` from the saved JSON and
`requestedModel` from the transcript. `claude_natively_long` in
`src/session/claude.rs` follows Claude Code's table by hand.

**Never kill processes by name, pattern, process group or user**: no
`pkill`, `killall`, pgrep-and-kill, kill loops or watchdogs. One such loop
(`pgrep -g ""` selects unrelated processes on macOS) killed every agent on
the machine. Kill a process only by PID, once, after checking it is one you
started.

The repository is public: fixtures, examples, tests and docs name made-up
hosts, folders, jobs and users (`jenkins.example.com`, `team-prod/app_master`,
`jane`), also when what they hold was recorded from a real server, whose
ids, times and structure they keep. Before committing such data, `git grep`
for the real names.

## The installers

Before handing off changes to `scripts/install.sh` or
`herdr-app/install.sh`, run their shell test, which serves a fake release
from a local directory and uses a fake `cargo`, a fake `herdr-app` and a
fake release binary that logs its calls (so `corgi daemon install` never
reaches launchd or systemd). In a git checkout the installers build from
source unless HEAD is the tag `v<version>` (`CORGI_BUILD=release` downloads
regardless; `CORGI_BUILD=source` always builds). Never run the real install
against your own checkout or `~/.config/herdr-app`; the tests use temp
homes. When a brief does ask for a live install, never set
`CORGI_OPENCODE_PLUGIN`: it is a stored answer, not a way past the
question, and `no` deletes the user's OpenCode plugin and keeps that answer
(with no terminal to ask in, the question is skipped anyway):

```bash
bash scripts/test-install
shellcheck scripts/install.sh scripts/test-install herdr-app/install.sh
```

`herdr-app/install.sh --home <dir>` is no sandbox: it chooses the app home
alone, and without `--mod-only` the run still installs the user's daemon
and OpenCode plugin and links `~/.local/bin/corgi`. Use `--mod-only --home`
(below), or a private `HOME` with fakes as the shell test does.

## The Herdr app mod

`herdr-app/mods/corgi/` is Corgi's one mod for the Herdr app (decision
2026-11-11): each layout block that shows it shows the view its props name,
`{"mod": "corgi", "props": {"view": "projects"}}`. `main.js` picks the
view from `props()` (init's own argument is not the block's props) and
hands it init and render; the views are modules of their own:
`projects.js`, the Projects list in the layout's sidebar (the default, with
no view), and `brand.js`, the "Herdr + Corgi" row at the sidebar's top;
`ui.js` holds what they draw alike, and `data.js` what Corgi says (`corgi
status`, `corgi usage`, `hc machine ls`), read once for every copy: the
app runs a mod's modules once per source version and shares them by its
copies, so `data.js`'s `store` is every block's, and the copies' timers
take turns at reading. A later view (questions, in the right-hand
`panel-body` stack) goes beside them as a module and an entry in `main.js`'s
`VIEWS`. `herdr-app/RECIPE.md` describes the mod for people and agents,
and `herdr-app/install.sh` installs it with the CLI and runs `corgi
app-layout` (`src/app_layout.rs`). That changes one thing in the
layout the window shows, in place, the left sidebar: its `spaces` block
shows the mod's `projects` view, its `agents` block goes, and its `brand`
block shows the mod's `brand` view instead of `workspace-chrome` (the
`sidebar-footer` block stays `workspace-chrome`'s; a layout without a
`brand` block keeps the rest of the change). Corgi had a mod per view
before, `projects` and `corgi-brand`: `app-layout` makes their blocks the
one mod's in every personal layout (`upgrade_layouts`), `remove` restores
Herdr's from either form, and the install removes their folders only after
the layout was changed, and only when they are Corgi's. A shipped layout
(`main`) is first cloned to `corgi`, as `layout.clone` does, and switched
to; Corgi never rebuilds a layout from `main` and no mod touches the
layout, so other mods' changes to it (herdr-gitlab's `gitlab` block in
`panel-body`) stay. Test a change there against a scratch home holding
real copies of the app's layouts and its `installed.json` (which names the
shipped ones). The Corgi space and the Corgi mod it showed (an overview
of every project) are retired (decision 2026-11-10: the user closed theirs
on purpose; what Corgi needs in the app goes into the Corgi mod), and
`corgi app-space` with them. The overview lived in `mods/corgi` too, where
the one mod goes now: the install removes it (`retired` in the installer)
only when its `main.js` names `io.github.zyrre.corgi` and sets up its own
install of corgi (`const INSTALLER = `), which every version of the
overview did and the one mod never does (`scripts/test-app-mods.mjs`
checks that). Herdr's shipped
layouts and mods
(`~/.config/herdr-app/layouts/main.json`, `mods/spaces`, `mods/agents`, …)
are read-only: Herdr replaces them on every start and moves an edited one
aside, so never edit them, and never switch the user's window to another
layout without being asked to (the install's switch to the `corgi` clone
is what the user asks for by installing). The Projects list began as a copy of Herdr's
`spaces` mod, and the brand row as a copy of the brand row of Herdr's
`workspace-chrome` (with its own copies of the two icons it draws); both
are Corgi's own and follow no later version of them.

The Projects list reads `corgi status --json` and runs `corgi merge`
(both in `src/commands/status.rs`) from a tagged worker's MERGE mark (and `corgi merge … --ask-supervisor` for
"Ask supervisor to fix"), and runs `corgi projects` and `corgi
open` (`src/commands/open.rs`). Check every change with
`herdr-app check herdr-app/mods/corgi` (it writes `jsconfig.json` and
`gpui-kit.d.ts` there, which Git ignores). `check` renders only the first
frame of the default view (the Projects list with its field closed and no
agents; `props()` is empty there), so a mistake in another view, the brand
row included, passes it: check a scratch copy whose `main.js` defaults to
`brand` too. Two things
cover the rest. The first must pass before a mod change is handed off:

```bash
node scripts/test-app-mods.mjs
```

It runs the mod under Node with stand-ins for `gpui-kit`, `gpui-base` and
`herdr` (chainable records whose `child`, `when` and `on_*` calls are kept,
so a test reads each view's texts and calls its click and key handlers) and
canned answers for `git` and `corgi`; add a case there for what you change.
`start(w, props, module)` starts a copy with the block's `props`; copies
given the same `module` name share the mod's modules (and so `data.js`'s
store), as the copies of one mod do in the app, and any other start loads
them afresh.
`mod.tick()` and `mod.settle()` wait for every task still running, so a
test that checks a state while one is pending (a canned `corgi` whose
promise it resolves later) fires `mod.cx.timers` by hand instead.
It proves the logic, not that the app has a builder. For that, the second:
`herdr-app check` a scratch copy of the mod (outside the repository) whose
`init` puts it in the state to check (the field open, with suggestions, a
problem and progress) and whose `render` adds the rows in question, built
from made-up agents. `check` fails on a builder the app lacks, so a passing
copy means every element of those views is one the app can make. Neither
shows what the window really draws (widths, where text is cut, which keys
reach a field): only a look at the window does, and the window to look at
is the preview window below, never the user's.

### The preview window

`scripts/app-preview` opens a Herdr app window of the worktree's own, with
the worktree's mods and layout in it, on an app home, a copy of the app and
a herdr-core of its own, all under `target/app-preview/`. Check every
change to a mod or to the layout in it before handing off, after the two
fast checks above:

```bash
cargo build --release                  # the mod runs target/release/corgi
scripts/app-preview start              # again after an edit: installs the mod again
scripts/app-preview do corgi.open      # any command of `herdr-app commands`, or a mod's own
scripts/app-preview shot open-project  # prints target/app-preview/open-project.png
scripts/app-preview status             # layout, focus, and errors from mods and files
scripts/app-preview stop               # always, before the final report
```

Read each picture yourself (the Read tool shows a PNG) and say what looks
wrong in it, also when it is not your change's. While the Mac's screen is
locked, the window draws almost nothing and `shot` repeats an old frame
(the same bytes across a `stop` and `start`, its images never drawn):
check `ioreg -n Root -d1 | grep CGSSessionScreenIsLocked` before trusting
a shot. To see a light theme while the Mac is dark, set the dark one in
the preview's copy of the layout state (`"theme": {"light", "dark"}` in
its home's `layouts/.state/corgi.json`), then
`do layout.switch --name corgi`: the layout follows the system, so
`do app.theme` and a `theme` in `settings.json` leave the window dark.
A change that is visual
(what a view draws: a row, a width, a color, an element, where text is cut,
a state's mark) attaches screenshots to its report: each one's path under
`target/app-preview/` with a line on what it shows, taken at the sidebar's
own width and at a narrow one (`do layout.resize --id sidebar --px 190`)
when text can be cut. A change that is not visual (an installer step, a
command's arguments, a test, a doc) attaches none and says that it is not.
The window is also there to try a change while working on it: `do tab.new
--name x --argv '["sh"]'`, `do space.new`, and `hc` on its core (the
`hcp` function in the script shows the environment) make whatever state a
view needs, and `stop` removes all of it.

The window shows made-up projects: `alpha` with a supervisor and four
workers in worktrees (working, blocked, idle and done, one tagged to
merge, one with a long task, one with a short one and one with no model or
context fill in its status), `beta` with a
working supervisor and an agent started by hand, and `notes`, a folder in
no repository. Each project has an upstream of its own
(`fake/origins/*.git`): `alpha` is 3 commits behind it, so its `↓3`
pulls (once; `stop` and `start` make it behind again), and `beta` has
diverged (`↑1 ↓2`), so a pull fails. The agents are a script that reports a state to herdr-core
(OSC 7501) and sleeps; their tasks and roles come from a canned `corgi
status --json` (`target/app-preview/fake/corgi`), which passes every other
command to the worktree's `target/release/corgi` with the preview's state
directory. Its `corgi` and `daemon.build` are the worktree's own build
(`corgi --version --json`), so the list's last line shows; edit
`fake/status.json` for another (`"newer"`, a daemon that is down). A merge
is canned too: with `fake/merge.json` holding what `corgi merge --json`
would print (`fake/ask.json` for `--ask-supervisor`, `fake/merge.delay` the
seconds it takes), nothing is merged; without it the worktree's `corgi`
answers, on the preview's core. So is a close after a merge:
`fake/close.json` (`fake/forced.json` for `--force`, `fake/close.delay`). The window takes no clicks from a script,
so a state behind one (the question under a MERGE mark, a card's button)
is reached by saving a copy of the mod into the preview's home
(`/tmp/corgi-ap-…/home/mods/corgi/projects.js`; the app reloads it) whose
command loop calls the handler the click calls (`this.askMerge`,
`this.runMerge`, `this.closeWorker`), then `do corgi.<command>`; `start` again puts the
worktree's own mod back. Take a shot a second after a reload: until each
space's git is read, the worktrees' spaces show as rows of their own.
Change the made-up projects in `start_fakes` when a view needs
a state they lack. `scripts/test-app-preview` runs the script against a
fake app, hc, AeroSpace and corgi, opening nothing: run it, and
`shellcheck scripts/app-preview scripts/test-app-preview`, after changing
the script. What its choices rest on:

- The window is never put on the user's core. A second window there shows
  their real agents, but it is no onlooker: a terminal has one size, so the
  pane the second window shows is resized to its size in the user's window
  too (a supervisor's screen was redrawn at the test window's width for
  the seconds a test ran), and the spaces are one document that every window's mods
  write (the Projects list reorders them with `space.move`).
- The app is run from a copy of the installed `Herdr.app`: it stages its
  own update beside its executable and swaps it in when it quits, which a
  second copy of the user's app would do under their running one. The home
  is seeded with `update-notes.json` naming the app's version, since a
  first start after an update also updates herdr-core, and with
  `"frame.welcomed": "skipped"` in `shared.json`.
- The app serves its commands on `<home>/herdr-app.sock`, and a home under
  a worktree's `target/` is longer than a socket path may be (the app then
  runs with no socket: `no Herdr window … path must be shorter than
  SUN_LEN`). The home is given through a link in `/tmp`; so is the core's
  state (`XDG_STATE_HOME`).
- AeroSpace lists no window of an app started from a worker's shell under
  herdr-core, whether run directly, with `open`, or as a launchd job in the
  user's `gui/` domain: `NSWorkspace` does not count that process as a
  running app, although its window is on screen. So the window cannot be
  moved to workspace 4, as the user's rule for headed windows asks. It is
  kept off their screen by its place instead: the app opens where
  `window.json` says it was, so the script writes the bottom right corner
  of the screen there, one point of the window showing, which is where
  AeroSpace itself keeps a hidden workspace's windows. The app takes the
  place only together with the `display` id it wrote itself (read from the
  user's `window.json`, never written), and only inside the screen: a
  place outside it is ignored and the window opens in the middle. It
  takes no focus. `open -n` left the app stopped (`ps` state `T`), with a
  process to remove by pid.
- The app draws no frame while minimized, so `shot` then repeats the last
  one; in the corner it draws every frame. `do window.minimize` is refused
  for that reason, with the commands that reach outside the preview
  (`app.update`, `app.update_core`, `app.quit`, …).
- Launch the app with `env -i`: a worker's `HC_TERMINAL_ID` and
  `HC_TERMINAL_SOCKET` name the user's core. A core of one's own is
  `HC_SESSION=<name>` with `XDG_STATE_HOME`, started with `SSH_CONNECTION`
  set (see "Testing `corgi daemon`"); `hc server status` starts a stopped
  core too.
- A folder under `target/` is inside Corgi's own repository to `git`, so
  the made-up projects are repositories of their own there, and the folder
  in no repository is outside the checkout (`/tmp/corgi-ap-….files/notes`,
  with no folder beside it: `corgi projects` offers a space's neighbours as
  "nearby").
- `herdr-app/install.sh --mod-only` writes no `config.json`, and a mod
  without one finds the user's `corgi` on `PATH`: the script writes the
  file after each install, which reloads the mod once more.

A status from a private core's live test
(see "Testing on hc") shows agents, a MERGE tag and the daemon line; stop
that test's daemon, then its core, for the daemon warning and the
`herdr-core not running` badge. Never open the app window (`herdr-app` with no
command) from an agent; it takes over the user's screen
(`scripts/app-preview` is the one way to open one). In a mod, a
`.when(cond, fn)` callback must return the element it was given, and text
for one line goes through the mod's `oneLine`, since a `\n` starts a second
line even in a truncated element. A click goes to every element under the
pointer, up to the row: a button inside a clickable row calls
`cx.stop_propagation()` in its `on_click`, or the row's click runs too.
`herdr-app` exits 2 (`no Herdr window`) when the app is closed, and a closed
app can leave its `herdr-app.sock` behind: to tell whether the window is
open, test whether the socket accepts a connection, not whether it exists.

To try the installer and `corgi app-layout` for real without the user's app
home, give them a scratch one holding copies of the shipped `layouts/*.json`
and `installed.json`, and a scratch state directory, so that nothing of the
run can land in Corgi's real state:
`XDG_STATE_HOME=<scratch>/state sh herdr-app/install.sh --mod-only --home
<scratch>/app` (`--uninstall --home` removes only what is in that home;
an install without `--mod-only` still installs the user's daemon and
OpenCode plugin). It checks the mod with the real `herdr-app check` and
changes the layout with the worktree's `target/release/corgi`; with no
window on that home the shipped `main` is cloned to `corgi` by hand
(`layout.clone` needs the window). `herdr-app check` runs the Projects
mod's first frame for real, `hc machine ls` included, and with a scratch
`XDG_STATE_HOME` that starts a herdr-core of its own on it (a launchd job,
`<scratch>/state/herdr-core/default/`), which outlives the install: stop it
with `env -i HOME="$HOME" USER="$USER" PATH="$HOME/.local/bin:/usr/bin:/bin"
XDG_STATE_HOME=<scratch>/state hc server stop`, after `hc server status`
in the same environment named the scratch core's id (its `core.json`) and
a pid that is not the user's core's, and check that pid is gone. Without
`env -i` a worker's `HC_TERMINAL_SOCKET` names the user's core. The
preview window is the place to see
the real `layout.clone`: `do layout.switch --name main`, delete its home's
`layouts/corgi.json`, and `start` again. Putting a mod or a
layout into the user's real `~/.config/herdr-app` from a worktree, even to
look at it, is refused by the permission rules here: look at it in the
preview window instead.

A terminal started with `hc run --bg --keep --label herdr.space=<new id>
--cwd <dir>` shows in the app as a new space without the window switching
to it, which `herdr-app do space.new` would do: use it for a scratch shell
that `corgi open --terminal` or `corgi supervisor --terminal` should start
in. Killing its terminals closes the space, at once when the last one is
killed (a space with another terminal in it stays); the app keeps it in its
`recent` list, which is the app's own. `hc rm` right after `hc kill`
worked when tried (`corgi close` still waits for the terminal to finish).

`corgi status --json` only reads, and asks `hc` nothing unless its core's
socket accepts a connection, so it may be run on the user's real core and
state. Never run `corgi merge` against a real project; its tests use scratch
repositories and a fake `hc` script.

The Projects list's `⏸ Agents` runs `corgi pause --json` and `▶ Agents`
`corgi resume --json` (`src/commands/pause.rs`, `src/hc/pause.rs`).
Never run either on the user's core: a pause stops their working agents,
and while it holds any agent their daemon types nothing into a supervisor.
`hc/pause/tests.rs` drives both against a fake core that acts as hc does,
`tests/open.rs` the real binary against the fake `hc` script, and
`waker/hc/tests.rs` the daemon's holds; a live try uses a private core
(below) with fake agents: a program that is not a shell (hc's foreground
is the interpreter's name, and a `sh` script reads as a shell in front),
reports `state=…:app=claude-code` (OSC 7501), takes Esc in raw mode and
exits 0 on `/exit`, and terminals labelled as Corgi's launch labels them,
`corgi.resume` included. What hc does there: an agent that is its
terminal's program (a worker) finishes the terminal when it exits with a
code, `keep` or not, and hc keeps a finished terminal and its labels across
`hc server stop` and `start` until `hc rm`; a new terminal may take a
finished one's name. `hc server restart` keeps the programs running: to
make a kept terminal lose its program, `stop` the core and `start` it. In
the preview window, `fake/pause.json` and `fake/resume.json` (with
`fake/pause.delay`) are what the canned corgi answers, and `status.json`'s
`paused` and an agent's `"state": "paused", "paused": true` are what the
list shows.

The Projects list also reads the Corgi of each other machine the window
shows (`exec(…, { on })`, see RECIPE.md "How it behaves"): it runs merge,
close and pause there, so every such call passes the worker's machine, and
this computer's calls pass no `on` at all. In `scripts/test-app-mods.mjs`,
`w.machineList` is what `hc machine ls` lists, `w.calls` keeps each
program's options, and `labWindow` is a window with a second machine. To
see another machine live, give the preview's core one that is a private
core on this Mac, never the person's core or their machines list:

- A scratch home (a short `/tmp` link, for the socket path) whose
  `.local/bin` holds links to `hc` and `herdr-core` (`hc machine add`
  checks for `herdr-core` there, `command -v herdr-core` else
  `~/.local/bin/herdr-core`) and a canned `corgi` printing a status whose
  terminals are that core's.
- A `--via` wrapper that runs its last argument as `sh -c` under `env -i`
  with that `HOME`, a `PATH` starting at its `.local/bin`, `HC_SESSION=lab`,
  an `XDG_STATE_HOME` of its own and `SSH_CONNECTION` (as in "Testing
  `corgi daemon`"). Start that core and its fake agents (`--label
  herdr.space=…` makes the machine's spaces) in the same environment,
  then `hc machine add --via <wrapper> --name lab --session lab` with the
  preview's environment (the script's `hcp`). `exec --on lab` then runs in
  the scratch home, so the mod finds the canned `corgi` on `PATH` there.
- Afterwards `hc machine rm lab` on the preview's core, `hc kill` and `hc
  rm` lab's terminals by address, `hc server stop` lab's core and check its
  pid is gone: `hc server status` starts a stopped core again.
- Lab's rows are under "This computer"'s in the window, which takes no
  scroll from a script: a preview copy of the mod that adds `null` to
  `this.folded` folds this computer's.

### The Jenkins mod

`herdr-app/mods/jenkins/` is a mod of its own, not part of Corgi's layout:
it needs no `corgi`, `herdr-app/install.sh` does not install it (its own
`install.sh` beside it does, into a named app home), and it may move to a
repository of its own, so keep it free of anything of Corgi's. Its
`README.md` is written for its users. It reaches Jenkins with the app's
`fetch` and an API token alone (`api.js`); the token is kept in
`jenkins/settings.json` of the app's home, mode 600, and must never reach
the repository, a log, a screenshot or a report. Before handing off a
change to it:

```bash
herdr-app check herdr-app/mods/jenkins
node herdr-app/mods/jenkins/tests/test.mjs
shellcheck herdr-app/mods/jenkins/install.sh
```

The test runs the mod under Node, with stand-ins of its own as
`scripts/test-app-mods.mjs` has, against `tests/fake-jenkins.mjs`: a
made-up Jenkins on a local port with canned jobs, logs that grow, a crumb
issuer and two users (`jane:fake-token` may build, abort and answer an
input step, `viewer:viewer-token` only reads). Two `team-prod` builds wait
at an input step (`alarm-notify_master` #12 asks nothing, `exporter_master`
#41 asks three values), with `wfapi/pendingInputActions` and the step's
`proceedEmpty`, `submit` and `abort`; a step's id is as Jenkins makes one
for a step that names none, the MD5 of its message with a capital first. **Never trigger, abort or change
anything on a real Jenkins**; a real one is only ever read. To look at the
mod, use the preview window with the fake server, never a real token (a
screenshot would show its settings):

```bash
node herdr-app/mods/jenkins/tests/fake-jenkins.mjs 48765 &   # stop it by its pid afterwards
scripts/app-preview start
home=$(ls -d /tmp/corgi-ap-*/home)
HERDR_APP_BIN=target/app-preview/Herdr.app/Contents/MacOS/Herdr herdr-app/mods/jenkins/install.sh --home "$home" --panel right --layout corgi
mkdir -p "$home/jenkins"
printf '{"url":"http://127.0.0.1:48765","user":"jane","token":"fake-token","folders":"team-dev, team-prod"}\n' >"$home/jenkins/settings.json"
sleep 1                                                      # the app sees a new mod folder a moment later
scripts/app-preview shot jenkins                             # or, without --panel: do tab.new --name Jenkins --mod jenkins
```

`--panel right` puts the mod into the layout's right-hand panel (decision
2026-11-11): one more child of the stack `panel-body` inside the column
`panel` under the tab bar, after Files, Changes and herdr-gitlab's GitLab,
so opening it leaves the tab bar's width and buttons as they were (a block
of its own at the end of the root row, as before, narrowed the whole `work`
column). The child is `{"id": "jenkins-panel", "content": {"mod":
"jenkins", "props": {"panel": true}}}`: `props.panel` makes the mod draw the
card Files, Changes and GitLab draw there (`bg(background)`, `rounded(10)`,
a `border` border; its own 42 px header) and marks the block as the
installer's. Not the id `jenkins`: the app keeps a block's `hidden` and
`size` by id in `layouts/.state/<layout>.json` after the block left the
layout, and the old root-row block `jenkins`, hidden by its button, hid a
stack child of that id (seen in the preview: the button lit, Files shown).
A rerun moves an old root-row block (`props.panel`, outside the stack) into
the stack. The installer edits the layout with
`python3`, as it is a shell script with no JSON parser of its own
(`install-layout.py` beside it, not copied into the mod); its test
(`tests/test.mjs`) runs it on scratch layouts. `--panel right` also puts a
Jenkins button into the tab bar the layout's `tabs` block shows: Herdr's
`tabs` (read-only, no extension point) is cloned by hand to
`mods/tabs-jenkins` (asset paths rewritten, `cloned_from`, and a first line
with the hash of what was cloned, by which `--panel off` and `--uninstall`
tell a copy still as cloned, which they point the layout back from and
remove, from one another installer changed, which stays), and a copy
(herdr-gitlab's `tabs-gitlab`) is edited in place. Either way two places
change, each between marker lines: the tab bar's `this.syncPanel(…,
enabled);` call is replaced by one whose list adds `jenkins-panel` while
the stack has it (so the stock `syncPanel` remembers it per space), with
the call as it was kept after `// was: ` for `--panel off` to put back
byte for byte, and a `panelButton("jenkins-panel", …)` goes just before
`.when(w?.buttons, …)`, which gives the stock behaviour (select and show,
or hide when it shows; lit only while it shows). Not the `const enabled`
line: herdr-gitlab's `install.py` finds that line by its exact text and
`str.replace`s every copy of it, so one kept in a `was:` comment would be
rewritten too and the tab bar left with two `const enabled` (the test's
`herdrGitlab` does what it does, after the Jenkins install). An older install's button (a `layout.toggle` of
the root-row block) is replaced between its markers; an unmarked Jenkins
button is left alone and not doubled. Every edit is worked out and
`herdr-app check`ed before anything is written. The test's tab bar is a
stand-in (`STOCK_TABS`, with the real `syncPanel` and `panelButton`) that
the test runs as the app would (`tabBar`), as CI has no app home: when
Herdr's `tabs` changes, check a scratch home holding a copy of the real
`mods/tabs`. A narrow panel is `do layout.resize --id panel --px 280` (the
column's minimum); opening it from a script is `do layout.select --stack
panel-body --child jenkins-panel` and `do layout.show --id panel`, what the
button runs. The tab bar restores the panel a space had open when its copy
reloads, so take a "closed" shot after `do layout.hide --id panel`. The
preview's app copy shows its own "Herdr … are ready" update toast over the
bottom right corner, where the panel's footer is, and no command dismisses
it.

The window takes no clicks, so a view behind one (a job, a log, the build
form, the abort question) is reached as for the Projects list: in the
preview home's copy of `main.js`, add commands to the `nextCommand` loop
that call what the click calls (`this.show({ kind: "job", place })`,
`this.pick(place, n, cx)` (a build's stages), `this.show({ kind: "stage",
place, number, stage: { id, name, nth } }, cx)`, `this.wholeLog(cx)`,
`this.fold(place, cx)` (a heading), `this.askBuild(place, cx)`,
`this.build(cx)`, `this.askAbort(place, n, cx)`, `this.abort(cx)`; for an
input step, `p = this.pendingOf(place, n)` once the job view has read it,
then `this.proceed(place, n, p[0], cx)` (its form, when it asks for
values), `this.submitInput(cx)` and `this.askAbortInput(place, n, p[0],
cx)`; a place
is `{ path: "/job/team-dev/job/api_master", label: "team-dev/api_master" }`;
the fake's stage ids are `4`, `29`, `54`, `79`), then `do
jenkins.<command>`. Installing again replaces that copy. What the
app's script engine has and lacks, found there: `fetch` gives a response's
`status`, `ok`, `url`, `text()` and `json()` but no headers (so a console
log is followed by counting the bytes read, not by `X-Text-Size`), follows
a redirect after a POST, and rejects when the host does not answer; there
is no `TextEncoder` (`util.js` encodes UTF-8 itself) and no `process.env`;
`writeTextFile` replaces a file and keeps its mode. A block's `props`
(the panel's `{ "panel": true }`) reach the mod through `props()` from
`herdr`; `init`'s first argument is `null` (seen in the preview by drawing
both into the header), so the test calls `init(null, …)` and its stand-in
`props()` gives `w.props`. In the preview's dark theme `background`
(#17191d) and `chrome` (#101114) are hard to tell apart by eye: compare a
shot's pixels (`sips -s format bmp` gives bytes Python reads without PIL,
which is not installed) rather than trusting a look. Which headings are
folded is kept with `setShared("jenkins.collapsed", …)`; the test's
stand-in for `herdr` keeps `shared` per world.

What the mod reads goes through `data.js`'s `store`, one for every copy (the
app shares a mod's modules by its copies, as `mods/corgi/data.js` does):
each thing read (a list, a job, a build's stages, a job's runs, who the
token is, a build's log) is an `Entry` that knows when it was read and how
its reads failed, and is read when it is due by its own times, so copies
ticking on their own timers read it once. A copy's view (stack, folds,
whether it follows a log) stays its own. A failure backs that read off (10,
30, then 60 s, never sooner than its usual time); one that may pass (no
answer, a 5xx, 408, 429, `READ_LIMIT` without an answer) stays quiet, a dim
`Last read … ago` line, until `QUIET_FAILS` failures or `QUIET_MS` of
failing; 401, 403, 404 and a wrong URL show at once. The app's `fetch` has
no timeout and the mod no `setTimeout`, so a stalled read is given up on
when a tick finds it older than `READ_LIMIT`, and its late answer dropped
by its sequence number. In the test, `start(w, module)` with the same
`module` name shares the store, as the app's copies do; any other start
gets its own. `fake.fail({ mode: "refuse" | "stall" | "status", status,
count, path })`, `recover()` and `release()` make the fake fail (`POST
/__fake/fail?…` and `/__fake/recover` from outside); a test that stalls a
read cannot `settle()` while it is held, so it fires the timers with
`mod.poke()` instead.

What the real Jenkins (2.479, read on
2026-11-11) serves, for when the fake needs to follow it: Pipeline Stage
View (`<build>/wfapi/describe`, `<job>/wfapi/runs`, ten runs) but no Blue
Ocean (`/blue/rest/…` is 404). A stage's status is `SUCCESS`, `FAILED`,
`ABORTED`, `IN_PROGRESS`, `PAUSED_PENDING_INPUT` (the prod jobs' approval
stage) or `NOT_EXECUTED`, with `error.message` when it failed or was
aborted; stages not started yet are not listed, and a failed build lists
none after the one that failed. An input step aborted shows as its stage
`ABORTED` with `Rejected by <name>`. Whether a list's running builds wait
at an input is read from their `wfapi/describe` (every half minute), never
from the list, and `<build>/wfapi/pendingInputActions` (`[]` once
answered) only for a shown build that waits;
`herdr-app/mods/jenkins/tests/fixtures/real-input.json` holds a real
answered Release Notes input, read when none was waiting, so the waiting
shapes (and an input's parameters) follow the plugin's documented ones,
not a real server's. A stage node's own `wfapi/log` is empty and
a step's is HTML, so the mod cuts a stage's log out of the console. With
`curl`, pass `-g`: a `tree=…[…]` query is otherwise taken as a glob.

### After a merge

After changes are merged into the primary checkout, one command, run in that
checkout, makes them live. On Corgi's merge line, the Corgi project's
supervisor runs it there itself and then tells the user which build now
runs (`corgi --version`); the user can still run it by hand:

```bash
CORGI_BUILD=source herdr-app/install.sh
```

It builds `target/release/corgi` from what is checked out, installs the
daemon's service again, which restarts the daemon on the new binary, puts
the mod into the app's home when it differs, and makes sure the layout
has the Projects list (which, once it has, changes nothing). Nothing else
needs reloading: the app reloads a mod the moment its files change (each
shown copy starts over, so an open "Open project" field or a merge's result
is gone) and a layout the moment its file changes, so the window needs no
`layout.switch` again (seen in the preview window: an install's swap of
the mod's folder reloads it); a mod that failed to load keeps its old build
running and shows in `herdr-app status`. The mods and the daemon run that
checkout's `target/release/corgi`; `corgi` on `PATH`, which supervisors and
workers run, is the same binary only while `~/.local/bin/corgi` links it,
and the install says when it links another. A worker never runs this: it
writes into the user's app home and restarts their daemon.

The Projects list says when it is due: its last line names the build that
answers (`Corgi 0.2.3 · 5b50895`, from `corgi --version`, which `build.rs`
gives the commit), adds `· main is newer` with this command once the
checkout the binary was built in has commits the build lacks, and warns
when the daemon is not running or still runs another build (`corgi daemon
install` starts it again).

`build.rs` runs again when the checkout's `HEAD` or a file that goes into
the binary changes (its `INPUTS`: `src`, `supervisor`, the manifests), so
every commit rebuilds the crate once, and a build
with uncommitted changes to those says `modified`. A new directory whose
files are compiled into the binary must be added to `INPUTS`, or a change
there keeps the old commit and no `modified`.

## Testing on hc (herdr-core)

`src/hc/tests.rs` drives the hc client against a fake
`hc` script (canned output per command, or per call with `CMD.N.out`), and
`tests/open.rs` the real binary's `corgi open`, `projects`, `supervisor
--terminal`, `app-layout` and `close` against another (`CORGI_HC_BIN`), with a
private home and state. A command that asks `hc` only while its core runs
(`HcSeen::look` in `src/commands/status.rs`: `corgi merge`, `corgi close`) sees
a running core in a test when `HC_SOCKET` names a `UnixListener` the test
holds, at a short `/tmp` path (the socket path limit). A supervisor start prepares its state directory,
so a core test that starts one goes through `start_supervisor_with` and
`supervisor::prepare_at` with a scratch base, never the real one. A test
project inside this checkout (under `target/`) is Corgi's own repository to
`git rev-parse`, so `tests/open.rs` keeps its projects in a `ScratchDir`.
A worktree launch (`Checkout::Worktree`) in a test would make a real
checkout under `~/.herdr/worktrees`: use `create_worktree_in`. To
try the commands live, use the user's core, which must never be stopped or
restarted, and only terminals you start; remove each by address
(`hc kill t7; hc rm t7`), never by name pattern. Things that bit:

- hc wraps what one agent's terminal sends to a program that has reported
  a status: `hc send t7 /exit` run from a worker's terminal (yours) arrives
  as `<hc from="t2p w-name">/exit</hc>`, a message, not the command, and a
  shell line arrives unrunnable. `HcClient::send` removes `HC_TERMINAL_ID`
  for that reason; by hand, `env -u HC_TERMINAL_ID hc send …`. A first send
  to a shell that never reported is not wrapped, so the wrapping shows only
  on the second start in the same terminal.
- `hc ls --json` gives `status` as a bare word and no labels; `hc get` gives
  a record and the labels. hc's options after the program are the
  program's (`hc run … sleep 60 --json` passes `--json` to `sleep`), so put
  `--json` before `--`.
- A worker's shell is itself an hc terminal (`HC_TERMINAL_ID`), so a
  command you run there acts as from inside hc. To act as a plain terminal
  outside it, run with `env -u HC_TERMINAL_ID -u HC_TERMINAL_SOCKET`; to
  act inside another terminal, run the command as
  `hc run --name <probe> --cwd <dir> -- <corgi> …` and `hc rm` it after.
- A Claude Code started under hc answers in the user's own permission
  mode (auto mode here) and may draft a `/feedback` report about what it
  saw; such a draft goes away with its terminal, and is never sent unless
  someone presses its key.
- `--model haiku` keeps a test cheap. A scratch project's folder-trust
  question is answered by Corgi when `CORGI_CREATED_PROJECTS_FILE` names a
  file listing the project's real path (`/private/tmp/…` on macOS).
- Afterwards remove the worktree with `git worktree remove`, and the
  state directory under `~/.local/state/corgi/supervisors/<project>`, the
  transcripts under `~/.claude/projects/`, OpenCode's sessions
  (`opencode session delete`, run in the project) and the lock under
  `~/.local/state/corgi/hc/` that the test created.

### Testing `corgi daemon`

`src/waker/hc/tests.rs` drives the daemon's passes
against a fake core (`waker::hc::Core`); `hc/daemon/tests.rs` covers its
resume and space decisions, its lock, the event stream (a fake socket) and
`hc/tests.rs` the watch-only viewer (a fake `hc` that paints in a
pseudo-terminal) and the real paints in `fixtures/input_box/hc-observe-*`.
`scripts/test-install` covers `herdr-app/install.sh`'s daemon and OpenCode
parts with a fake release binary that logs its calls. What a daemon says of
itself (its lock, `corgi daemon status`, the `daemon` of `corgi status
--json`) needs no core: `corgi daemon` run with `env -i`, a private `HOME`
and `XDG_STATE_HOME`, `HC_SOCKET` naming a socket that does not exist and
`CORGI_HC_BIN` a script that logs its calls takes its lock and waits,
asking hc nothing; run the other commands in the same environment, and stop
it by its pid (start it as `env -i … corgi daemon &`, not through a shell
function, or `$!` is a subshell's). A live test of the
daemon's passes stops and starts a core, so it never uses the user's:

- Run a core of your own with a short state path (`HC_SESSION=x`,
  `XDG_STATE_HOME=~/.cdt/s`: the socket path limit is 103 bytes) and an
  `env -i` environment: `HOME`, `USER`, `SHELL`, `TERM`, `LANG`, a `PATH`
  with `~/.local/bin`, and the `SSH_CONNECTION`/`SSH_CLIENT` of an SSH
  session. Without those, `hc server start` starts the core through a
  launchd job, and every Claude Code in it says `Login expired`. The same
  `XDG_STATE_HOME` keeps Corgi's state (supervisors, inboxes, the daemon's
  log) out of the real one, and hc passes it, with `HC_SESSION`, to every
  terminal. Run every `hc` and `corgi` call of the test through that
  environment (a wrapper script), or it reaches the user's core.
- List the scratch project in `$XDG_STATE_HOME/corgi/created-projects` so
  Corgi answers its folder-trust question.
- Run `corgi daemon` in the background with that environment and stop it by
  its pid; `CORGI_DEBUG_HANDOVER_PERCENT=1` and
  `CORGI_DEBUG_HANDOVER_IDLE_SECS=30` force a handover in a minute or two,
  and hand every new session over again a minute later, so restart the
  daemon without them right after. A test LaunchAgent gets a label of its
  own (`corgi daemon install --label io.github.zyrre.corgi-daemon-test`
  in that environment) and `corgi daemon uninstall --label …` before you
  finish; never run the plain `corgi daemon install` for the user.
- `hc keys` types a draft into a supervisor's box (it does not count as a
  person typing for hc's send, but the box holds it), and `backspace` keys
  clear it.
- `hc attach --observe` waits half a second for answers to its questions
  before it paints; a read that stops at the first quiet moment gets no
  screen.
- Afterwards, besides the above: `hc kill` and `hc rm` each terminal by
  address, `hc server stop` the core, delete its state folder, the worktrees
  under `~/.herdr/worktrees/<project>`, and the transcripts of the project
  and of each worktree under `~/.claude/projects/`.

## Releasing

No release is cut until the user asks for one (decision 2026-11-09).

Bump `version` in `Cargo.toml` (and run `cargo build` so `Cargo.lock`
follows), commit, and push a `vX.Y.Z` tag matching it. The piped install
(`--version X.Y.Z`) runs that tag's `herdr-app/install.sh`, so the tag
must hold the installer. `.github/workflows/release.yml` then builds the
four binaries and publishes the GitHub release with them and their
`SHA256SUMS`; it refuses a tag that does not match `Cargo.toml`.

After the artifacts exist, independently download all four binaries and hash
those bytes; compare with the published `SHA256SUMS`, GitHub asset digests and
retained CI/publication evidence. Review and commit `checksums/vX.Y.Z.sha256`
in a follow-up source change. Each line is exactly `vX.Y.Z <lowercase 64-digit
SHA256> <asset filename>` with no header, comments, blank lines or duplicates;
the version in every row must match the filename and `Cargo.toml`.
Use the four exact platform asset names from the release workflow. Never
replace existing pins just because mutable release assets changed: investigate
the discrepancy. Keep verification evidence with the review. Until reviewed
pins reach the source checkout users install, that version builds from source.
The release tag need not contain its own future binary hashes, and the release
workflow must not automatically grant trust to its output by committing pins.
