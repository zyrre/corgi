# Development handoff

After making a change, ensure it is visible in the running environment before
handing off. Rebuild, relink, reload, or restart the affected component when
needed, then verify the change rather than assuming a source edit is live.

For Corgi, code changes normally require `cargo build --release`; plugin
metadata or link changes may also require `herdr plugin link "$PWD"`. Do not
restart Herdr or close panes unnecessarily—only do so when the affected change
cannot take effect otherwise.

## Testing worktree changes

Git worktrees are isolated: building in a worktree does not update the Corgi
plugin linked from the primary checkout, and an already running Corgi process
does not hot-reload. Before handing off Rust changes from any checkout, run:

```bash
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test
cargo build --release
```

These cover the whole workspace: the `corgi` binary at the root and the
`corgi-core` library in `crates/corgi-core` are both default members (see
"Crates" in ARCHITECTURE.md). The core must not depend on ratatui or
crossterm; after changing its dependencies, check that
`cargo tree -p corgi-core | grep -E 'ratatui|crossterm'` prints nothing.
Code that draws or reads keys goes in the root crate; what another frontend
would need (Herdr, the rows, the flows that change Herdr, the supervisor's
files) goes in the core, which takes a `HerdrClient` rather than an `App`.

Clippy lints code under `#[cfg(target_os = ...)]` only on that OS, so a lint
can pass on your machine and fail CI on the other (a `return` left at the end
of a macOS-only block did). CI runs on both Linux and macOS; check both jobs.

On macOS the default `$TMPDIR` makes the fake Herdr socket path in
`crates/corgi-core/src/test_support.rs` exceed `SUN_LEN`, so run
`TMPDIR=/tmp cargo test` there. The binary crate's tests reach the core's
test helpers through its `test-support` feature; a core item only tests use
is gated `#[cfg(any(test, feature = "test-support"))]`, not `#[cfg(test)]`,
when the binary's tests need it too.

A test that polls a non-blocking listener's `accept()` must set the accepted
stream back to blocking (`set_nonblocking(false)`) before relying on a read
timeout: macOS/BSD sockets inherit `O_NONBLOCK` from the listener, Linux
sockets do not, so the failure shows only on macOS CI.

In the root crate's tests, give every `HerdrClient` a scratch marker record
with `.with_marker_record(scratch.join("markers.json"))`: the core is built
there without `cfg(test)`, so `HerdrClient::from_socket_path` writes its
marker record into the real state directory.

A shell pane reported as an agent (below) shows in Corgi like one, but
Herdr refuses `agent.prompt` to it with `agent_not_ready`, since no agent is
its foreground process. Testing a prompt live needs a real agent CLI, which
tests must not drive, so test prompts against a fake Herdr socket.

A waker test's agents, built with `project_agent`, have no `agent` kind, so
their harness is `Other`, which has no idle time for a handover. A test that
needs Claude Code's behaviour sets `info.agent = Some("claude".into())`.

`Handover::observe` times a rest from the first refresh that sees it, so a
handover test must observe at second 0 before asserting at the idle time.

`App::new` loads the real `projects` list from Corgi's state directory;
`test_app()` keeps an in-memory one, so build test apps with it. Saving the
list drops directories that do not exist, so a test that saves it to a file
must remember real directories (made in its `ScratchDir`), not made-up paths
such as `/repos/a`.

A test's Git repositories name their branch on both sides: `git init -b main`,
and `git init --bare -b main` for a remote. A plain `git init` follows the
user's `init.defaultBranch`, and a clone of a bare remote whose `HEAD` names
`master` checks out an unborn `master`, so pushing `main` fails there.

The supervisor's role and the `[Corgi]` lines Corgi types into its box
(handover requests, merge, conflict and push lines) are pinned by tests in
`crates/corgi-core/src/supervisor.rs`, and the merge lines also by the
exact-inbox-text tests in `src/app/merge.rs`: change the wording, update
both. `Inboxes::pending_for` lists every line that waits; a newer line of
the same key replaces an older one only when they go out.

An error of `corgi merge` (`crates/corgi-core/src/merge.rs`, and what
`src/app/status.rs` makes of it) reaches three readers: the merge dialogs of
the dashboard and the desktop window, the card under a worker's row in the
Herdr app's Projects list, and the supervisor, whose line quotes it when the
user asks it to fix the merge (`--ask-supervisor`). Word it for all three:
what went wrong and what to do about it, without a key (`m`, `s`), which
only the dashboard has. Line breaks in `supervisor/ROLE.md` and `supervisor/HANDOVER.md`
matter, since those tests match their text with `contains`. Another test
there (`the_role_is_a_third_shorter_than_before_the_on_demand_parts_left_it`)
caps `ROLE.md` at three quarters of 34,516 bytes, 25,887, and only 39 bytes
are left: a role addition must trim something else.

On macOS, copying a rebuilt binary over an existing file (say, a copy of
`target/release/corgi` kept to compare before and after) gets it killed with
exit 137, because the kernel caches the old file's code signature. `rm` the
destination before copying.

macOS has no `timeout` command, so a command wrapped in it (`timeout 600
cargo build`) silently never runs; behind a `grep` filter even the
`command not found` is lost.

`corgi digest`'s lookups (`--search`, `--show`, `--ledger`, `--brief`,
`--decision`) need Herdr only to find the project's state directory. Test
them with a `ScratchDir` as the state directory through `digest_lookup` in
`crates/corgi-core/src/digest/memory.rs`. To try them on real data, copy the
state into a scratch directory and point `XDG_STATE_HOME` at it; never run a
change against the real `~/.local/state/corgi`:

```bash
mkdir -p "$scratch/corgi/supervisors"
cp -R ~/.local/state/corgi/supervisors/<project> "$scratch/corgi/supervisors/"
XDG_STATE_HOME="$scratch" ./target/release/corgi digest <project dir> --search "<words>"
```

`corgi serve`'s forge listing (`src/serve/forge.rs`) is tested with a
canned `ForgeCli` runner; serve's other tests set `forge: None`, so no test
calls a real `glab` or `gh`. To check it against a real repository, read
only, run `CORGI_LIVE_FORGE_ROOT=<repo> TMPDIR=/tmp cargo test -p corgi --lib
-- --ignored live_forge --nocapture`. It lists and changes nothing on the forge.
Do not run a real `corgi serve` on the user's Herdr for this, since it wakes
supervisors. `glab` prints some notices (such as a deprecated setting) on
stdout ahead of its JSON.

`corgi usage` reads the plan usage through a cache shared by every Corgi
process (`usage/<provider>.json` in the state directory), which the running
dashboards and the Herdr app, on the old binary, refresh every minute. A
build of yours run against it takes their fresh entry and fetches nothing.
To check a usage change live, give it an empty scratch `XDG_STATE_HOME`:
`XDG_STATE_HOME="$scratch" ./target/release/corgi usage --json`.

`rust-version` in the root `Cargo.toml`'s `[workspace.package]`, which both
crates take, is the oldest Rust that builds Corgi: 1.89, set by std's
`File::lock` (ratatui 0.30 alone needs 1.88). With it set, clippy's
`incompatible_msrv` lint refuses a newer std API, so raise it deliberately. To
check an older toolchain, build in Docker with the worktree mounted read-only
and the target directory inside the container (a session scratchpad under
`/private/tmp` is not visible to Docker on macOS):

```bash
docker run --rm -v "$PWD":/src:ro -w /src -e CARGO_TARGET_DIR=/tmp/t \
  rust:1.89-slim cargo check --locked --all-targets
```

## The desktop window

`crates/corgi-gui` (GPUI, Rust 1.96) is out of the default members, so the
commands above never build it. Before handing off a change to it, also run:

```bash
cargo clippy -p corgi-gui --all-targets -- -D warnings
cargo clippy -p corgi-gui --all-targets --features capture -- -D warnings
cargo test -p corgi-gui
cargo build --release -p corgi-gui
```

It shares `Cargo.lock` with the other crates, so after changing its
dependencies repeat the Rust 1.89 check above: a newer pin of a crate the
core also uses would raise the core's oldest Rust. On Linux it needs the
Wayland, xkbcommon and fontconfig development packages; a Linux container
(`rust:latest` with `libxkbcommon-dev libwayland-dev libfontconfig-dev
libfreetype-dev pkg-config`) checks the Linux build from a Mac. Before
handing off `scripts/package-gui.sh`, run `bash scripts/test-package-gui`,
which packages fake binaries and runs `shellcheck`.

The root crate's tests that wait for a background job give it five
seconds, and fail while a GPUI build (or a container building one) takes
every core, so run the root tests without a GUI build alongside.

To see the window working, run it against a throwaway Herdr as for the
hooks below (private `HOME` and `XDG_*`, a shell wrapper that keeps
`/opt/homebrew/bin` off `PATH`, `CORGI_CLAUDE_BIN` and `CORGI_CODEX_BIN`
pointing at fakes so neither a real CLI nor the Keychain is touched, and
seeded `usage/<provider>.json` files with their `.lock` held so no plan
usage is fetched). A fake agent is a shell pane reported with a test
`--source` (Herdr 0.9.0 takes the states `idle`, `working`, `blocked` and
`unknown`; a done agent cannot be reported), named with
`herdr agent rename` after it is reported (a supervisor's name must match
its `corgi_handler` token), tagged with a
`session` token; a Claude session file under the private
`~/.claude/projects/<cwd with / and . as ->/<session>.jsonl` gives it a model,
context, cache and transcript (use the real path: `/tmp` is `/private/tmp`
on macOS). An agent's project is the repository
`git rev-parse --git-common-dir` names, so a plain workspace on a
`git worktree add` checkout is a linked worktree to Corgi as much as one
opened with `herdr worktree open`; only Herdr's own is removed with Herdr's
`worktree.remove`, a plain one with `git worktree remove`. The private home
needs a Git identity in its `.gitconfig` for merges, and a bare `origin`
for one to push.

Build with `--features capture`, then run with `CORGI_GUI_BACKGROUND=1`
(no focus taken), `CORGI_GUI_SCRIPT` (steps such as
`wait:3000;key:n;type:Fix it;shot:form;quit`) and `CORGI_GUI_CAPTURE_DIR`;
script steps are trimmed, so a trailing space in `type:` is lost; the
window opens with no row selected, so a script reaches a row's actions
(`a`, `p`, `1`–`9`) after a `key:j`;
`CORGI_GUI_APPEARANCE=light` checks the light theme without changing the
system's (a scripted click on the header's toggle switches that run alone;
nothing is kept), and `CORGI_GUI_FONT_FILES` (font files separated by `:`) loads
fonts for that process alone on macOS, to check a font the machine lacks
without installing it. Ghostty draws Nerd Font icons from fonts built into
it, so icons in a user's Ghostty do not mean a Nerd Font is installed; the
step `glyphs:TEXT` logs which font draws each character in the terminal,
and works without a redraw. While the Mac's screen is locked
(`ioreg -n Root -d1 | grep CGSSessionScreenIsLocked`), the platform asks
for the window's first frame and no more, so `shot` and the click steps
draw a frame themselves (a click would otherwise hit-test that first
frame); other steps still see it, and AeroSpace does not list the window.
`screencapture -l` fails then too (and at any time without Screen
Recording permission; `CGPreflightScreenCaptureAccess()` says), but
`CGWindowListCopyWindowInfo` (filtered by owner pid) still gives the
window's id and frame, so a frame taller than its content shows a native
title bar. Move the window to AeroSpace workspace 4 the moment it
appears (see the user's instructions); `render_to_image` still captures it
there. Moved there, the window counts as occluded: it may be told it is
hidden, and it draws no frames until a `shot` step draws one, so the focus
view's terminal neither measures its grid nor connects. Repeat `show` after
the move and take a throwaway shot before the ones that count.
AeroSpace tiles the window to fill that workspace whatever size
`gui-layout.json` kept, so for a shot at a given width, float it
(`aerospace layout --window-id <id> floating`) and size it with a
`size:640x760` step; 640 wide is the window's minimum.
Script steps run through the untyped window handle (`cx.update_window`):
updating the typed `WindowHandle<CorgiWindow>` leases the root view, and a
key dispatched inside that lease panics when its listener updates the view.
A scripted click (`click:X,Y`, `right:X,Y`, `double:X,Y`) moves the mouse to
the point before pressing: without the move, the elements under it are not
hovered and the click reaches none of them.
The step `scroll` draws a frame and logs the overview's scroll offset, the
selected row and the focus view's agent: a click on a row must leave the
offset alone, and a key scroll it only as far as the row's edge
(`src/rows.rs`). A selection that scrolls moves the offset while the frame is
prepainted, so read it with `scroll`, not straight after a `key`.
Told it is hidden on workspace 4, the window also stops watching the
transcript, so a row expanded with Space shows "Reading the session…": put
a `show` after the key that expands it. The overview settles its offset
before it is prepainted (`Overview` in `src/rows.rs`), so a frame draws
its rows at the offset it chose; a shot may still be one frame behind a
key, so read offsets with `scroll`.
Kill the window's own process, not a wrapper shell function's subshell,
and check that its `corgi serve` child is gone afterwards.

To check layout alone, no Herdr is needed: `--corgi PATH` runs any program
as the server, so a script that `cat`s a file of two JSON lines (a `hello`
and a `snapshot`, written from a throwaway test with `serde_json` over
`ServerMessage`) and then `cat`s stdin, to stay open, gives the window any
state, a transcript included. Run it with a private `HOME` and
`XDG_*` so no layout or keymap of the user's is read or written. For exact
row geometry, put a temporary `eprintln!` of each row's top and bottom in
`Rows::record` (`src/rows.rs`), which gets every row's bounds each frame,
and remove it before committing; compare the numbers with and without a
row expanded, since a screenshot only shows a difference of a few pixels
to the eye.

In gpui-pre 0.3.6 `flex_shrink` takes a factor, and min-content sizing
wins: a row whose other children keep their width cannot shrink just one
text child, however it is styled. Fit such text in code from the
monospace cell width (`monospace_cell_width`), as the status bar's
machine names are (`fit_names` in `src/machine_bar.rs`) and its texts
(`fit_status` in `src/view.rs`). `whitespace_nowrap` does not stop a `\n`
in the text from starting a second line (`shape_text` splits on it), so
join a text's lines before showing it on one.

Run with `env -i` and a private `HOME`, a real `ssh` (a machine of the
status bar, or `--ssh`) still reads the real `~/.ssh`, since it takes the
home directory from the password database: the user's keys and
`known_hosts` work without copying them.

To try the focus view on another machine (`src/tunnel.rs`), run with
`--ssh HOST` and reach the scratch pane with the script steps
`open:PANE`, `leave` and `machine:N`, never with `key:j`/Enter, which can
open one of the user's agents. That machine may run a corgi-gui of its
own whose park workspace (`corgi-gui (parked terminal)`) this window finds
by its label and uses; it closes only a park it made itself. Check what a
run leaves with `pgrep -P <window pid> -lf` (the tunnel's `ssh -L`) and
`ls /tmp/corgi-gui-$(id -u)` (or under `$TMPDIR`), and list that Herdr's
workspaces before and after.

The focus view's terminal connects to the throwaway session's
`herdr-client.sock` itself. After changing `src/endpoint/`, run
`cargo test -p corgi-gui -- --ignored live_herdr`, which drives the adapter
against a Herdr of its own (private home, `/bin/sh` panes) and removes it.
To try the terminal by hand, run a program in the agent pane that turns on
bracketed paste and the kitty keyboard protocol, as Claude Code does, and
prints the bytes each key arrives as: a real Claude Code reads its
credentials from the Keychain on macOS, which these runs must not touch.
A real Claude Code runs without credentials or the Keychain as
`claude --bare` with a dummy `ANTHROPIC_API_KEY`, a private `HOME` and
`CLAUDE_CONFIG_DIR`, resuming (`--resume`) a hand-written transcript: it
needs no API then, but still downloads the public plugin marketplace on
its first start. Exit it with Ctrl+C Ctrl+C: killed twice in fullscreen,
it turns fullscreen off by itself (`"fullscreenAutoDisabled":{"strikes":2}`
in its `.claude.json`), and wheel tests then silently scroll 0 lines until
that key is deleted.
To see the context window Claude Code itself gives a model or setting, run the
real CLI that way with `--model <id>`, `ANTHROPIC_BASE_URL` at a small local
fake of `/v1/messages` (streaming; it answers a canned turn with a chosen
`usage`), and a `statusLine` command that saves its stdin: send one prompt, then
read `context_window.context_window_size` from the saved JSON and
`requestedModel` from the transcript. Do this when a new model ships:
`claude_natively_long` in `crates/corgi-core/src/session/claude.rs` follows
Claude Code's table by hand.
The script steps `drag`, `wheel`, `link`, `ime`, `hide`, `show`,
`deactivate` and `activate` drive the terminal (a scripted window is never
really active, so `activate` stands in for the user coming back to it);
`CORGI_GUI_LATENCY=1` logs key-to-surface and key-to-frame times, and for
three seconds after a Space that expands a row, every message, render,
layout of the turns (how long, and how many turns were new to the frame)
and overview move, timed from the key (`src/latency.rs`);
`CORGI_GUI_DEBUG_ENDPOINT=1` each snapshot, request and answer, and
`CORGI_GUI_URL_LOG=<file>` writes a clicked link there instead of opening a
browser. Copy and paste use the real clipboard: keep the user's text and put
it back. To see what a user's own Herdr window shows meanwhile, attach
`herdr --session NAME` in a pty of a fixed size (its first-run welcome and
integration questions take Esc; install nothing). A tab that a JSON focus
call (`herdr workspace focus`) moved that window to is owned by no client
until something is typed there, which is when a new connection could claim
its size. A client takes a tab's size from another by typing in it or by
reporting that its terminal gained focus; to see who has it, have the pane
print `stty size`, since every client lays the tab out at its own size.

For a screenshot that needs no real flow, `--corgi` can name a small
program instead: corgi-gui runs it with `serve …`, and any program that
writes a `Hello` and a `Snapshot` (built with `corgi_core::protocol`, one
JSON line each) and answers each `Request` line on stdin draws the window
with no Herdr at all, and can delay or refuse a reply to show a state. A
project with `card: false` draws its rows without the collapsed-by-default
card around them, so a script's `key:j` reaches a row at once.
To show a reconnect, kill that program alone with an anchored pattern
(`pkill -f "^<path>/fake-serve"`): the window's own command line holds
`--corgi <path>`, so an unanchored one kills the window too.
It is easiest as a scratch Cargo crate depending on `crates/corgi-core` by
path; build its palette with
`Theme::new(Palette::catppuccin(light), Source::Catppuccin, false, true).rgb_roles()`,
as `serve::state::theme_colors` does.
Built from a real serve's hello and snapshot lines, with the socket and the
agent's pane, tab and workspace changed, it can name a real throwaway Herdr:
the focus view then shows a live terminal of that session beside made-up
side-panel content.
When moving its window, look the id up after launch each time: AeroSpace
can still list the last run's closed window for a moment. Find it by its
process id, never by app name, since the user may have a corgi-gui window
of their own open:
`aerospace list-windows --all --json --format '%{window-id} %{app-name} %{app-pid}'`
lists `app-pid` only when `--format` asks for it.

Text on a fill other than the background (a badge, the selected row's tint,
a dialog) takes its color through `Colors::badge_text` or
`Colors::secondary`, which keep the role unless it all but vanishes on
that fill (Mocha's muted and black on its greys), so a theme keeps its
look wherever its roles read. A dialog's hints and other secondary text
take `colors.secondary(colors.roles.popup_fill)`: plain `roles.muted` all
but disappears on Mocha's popup fill.

`HERDR_CONFIG_PATH=<file> herdr config check` validates a Herdr config file
without a running server (and without touching the real config). Herdr's CLI
reports errors as JSON lines with an `error.code`, such as
`server_not_running` or `plugin_not_found`, so match on the code.

To show worktree changes interactively without replacing the stable
`prefix+d` plugin registration, launch that worktree's release binary in a
dedicated Herdr preview workspace. Every temporary preview must use the exact
visible prefix `🧪 TEMP CORGI PREVIEW` in its workspace, tab, and pane labels;
append the branch name so multiple previews remain distinguishable. Never name
a temporary preview only `Corgi` or `Corgi dashboard`.

```bash
corgi_preview_label="🧪 TEMP CORGI PREVIEW — $(git branch --show-current)"
corgi_preview_result="$(herdr workspace create --cwd "$PWD" --label "$corgi_preview_label" --no-focus)"
corgi_preview_workspace="$(jq -r '.result.workspace.workspace_id' <<<"$corgi_preview_result")"
corgi_preview_tab="$(jq -r '.result.tab.tab_id' <<<"$corgi_preview_result")"
corgi_preview_pane="$(jq -r '.result.root_pane.pane_id' <<<"$corgi_preview_result")"
herdr tab rename "$corgi_preview_tab" "$corgi_preview_label"
herdr pane rename "$corgi_preview_pane" "$corgi_preview_label"
herdr pane run "$corgi_preview_pane" "./target/release/corgi"
herdr workspace focus "$corgi_preview_workspace"
```

When driving the preview pane with `herdr pane run` or `herdr pane send-keys`,
wait until `herdr pane read` shows the dashboard or the shell prompt before
sending more: keys sent while Corgi starts or quits can reach the dashboard,
where letters such as `s`, `t` and `x` are actions.

The `herdr pane current` CLI sends the calling pane's ID from
`HERDR_PANE_ID`, so run inside a pane it returns that pane, not the one
focused in Herdr's UI; the socket request `pane.current` with `{}` returns
the focused pane.

`test_support::FakeHerdr` answers `pane.process_info` from what
`set_processes` gave it (no processes otherwise), for tests of what runs in
a pane: the orphans detector, the proven re-tag and Codex admission. A real
Herdr lists every process of the foreground group, an MCP server beside the
agent's CLI included, and may leave a process's `argv` out, so match on the
program's file name (`ForegroundProcess::program`) and never assume `argv`.
To try `corgi orphans` on the user's Herdr, run it as it is: it only reads
(snapshot, process info, screens), and changes nothing anywhere.

`test_support::FakeHerdr` answers `worktree.list` with a bare `ok` unless told
`set_repo_root`, and `corgi fleet` and `corgi digest` fail on that, so a test
running either against it must set one.

To test against an agent without starting one, report a shell pane in the
preview workspace as one. Corgi hides agent panes without a session, so tag
it with one too (the pane ID goes before the options), and release it
afterwards:

```bash
herdr pane report-agent --source corgi-test --agent claude --state idle "$pane"
herdr pane report-metadata "$pane" --source corgi-test --token session=test
herdr pane release-agent "$pane" --source corgi-test --agent claude
```

Report the temporary workspace label and ID in the handoff. Leave it available
when the user needs to inspect the result; otherwise close only the preview
workspace created for the test. Do not run `herdr plugin link "$PWD"` from a
disposable worktree during normal testing, because deleting that worktree would
leave `prefix+d` registered to a missing path.

Before handing off changes to `scripts/herdr-corgi-dashboard`, also run its
shell test, which drives the script against a fake `herdr` binary (no real
Herdr session needed) and checks `shellcheck` if it is installed:

```bash
bash scripts/test-herdr-corgi-dashboard
shellcheck scripts/herdr-corgi-dashboard scripts/test-herdr-corgi-dashboard
```

Before handing off changes to `scripts/install.sh`, `scripts/link-command.sh`
or `herdr-app/install.sh`, run their shell test, which serves a fake release
from a local directory and uses a fake `cargo` and a fake `herdr-app`. In a git
checkout the installers build from source unless HEAD is the tag `v<version>`
(Herdr's own `.tmp-install-*/checkout` and `CORGI_BUILD=release` download
regardless; `CORGI_BUILD=source` always builds). Never run the real install
against your own checkout or `~/.config/herdr-app`; the tests use temp homes.
When a brief does ask for a live install, never set `CORGI_OPENCODE_PLUGIN`:
it is a stored answer, not a way past the question, and `no` deletes the
user's OpenCode plugin and keeps that answer (with no terminal to ask in,
the question is skipped anyway):

```bash
bash scripts/test-install
shellcheck scripts/install.sh scripts/link-command.sh scripts/test-install herdr-app/install.sh
```

## The Herdr app mod

`herdr-app/mods/corgi/` is the Herdr app's Corgi mod,
`herdr-app/mods/projects/` its Projects mod, the list in the layout's
sidebar, and `herdr-app/mods/corgi-brand/` its brand row mod, the
"Herdr + Corgi" row at the sidebar's top; `herdr-app/RECIPE.md` describes
them for people and agents, and `herdr-app/install.sh` installs them with
the CLI and runs `corgi app-layout`
(`crates/corgi-core/src/app_layout.rs`). That changes one thing in the
layout the window shows, in place, the left sidebar: its `spaces` block
shows `projects`, its `agents` block goes, and its `brand` block shows
`corgi-brand` instead of `workspace-chrome` (the `sidebar-footer` block
stays `workspace-chrome`'s; a layout without a `brand` block keeps the
rest of the change). A shipped layout
(`main`) is first cloned to `corgi`, as `layout.clone` does, and switched
to; Corgi never rebuilds a layout from `main` and no mod touches the
layout, so other mods' changes to it (herdr-gitlab's `gitlab` block in
`panel-body`) stay. Test a change there against a scratch home holding
real copies of the app's layouts and its `installed.json` (which names the
shipped ones). The Corgi space, a space named
`Corgi` showing the Corgi mod, is retired (the user closed theirs on
purpose; Corgi's features in the app go into the Projects mod): neither the
install nor `corgi daemon` makes it, and nothing may bring it back by
itself. `corgi app-space` (`crates/corgi-core/src/app_space.rs`) still
makes it for a person who runs it, and `remove` closes it. Herdr's shipped
layouts and mods
(`~/.config/herdr-app/layouts/main.json`, `mods/spaces`, `mods/agents`, …)
are read-only: Herdr replaces them on every start and moves an edited one
aside, so never edit them, and never switch the user's window to another
layout without being asked to (the install's switch to the `corgi` clone
is what the user asks for by installing). The Projects mod began as a copy of Herdr's
`spaces`, and the brand row mod as a copy of the brand row of Herdr's
`workspace-chrome` (with its own copies of the two icons it draws); both
are Corgi's own and follow no later version of them.

Test `corgi app-space` against a fake `herdr-app` (its tests do), never
the user's app. The Corgi
mod reads `corgi status --json` and runs `corgi merge`; both live in
`src/app/status.rs` and see hc's agents alone, never the Herdr TUI's. The
Projects mod reads the same status, runs the same `corgi merge` from a
tagged worker's MERGE mark (and `corgi merge … --ask-supervisor` for
"Ask supervisor to fix"), and runs `corgi projects` and `corgi
open` (`src/app/open.rs`). Check every change with
`herdr-app check herdr-app/mods/corgi`, `herdr-app check
herdr-app/mods/projects` and `herdr-app check herdr-app/mods/corgi-brand`
(it writes `jsconfig.json` and
`gpui-kit.d.ts` there, which Git ignores). `check` renders only the first
frame (the "Reading Corgi's status…" line; the Projects list with its field
closed and no agents), so a mistake in another view passes it. Two things
cover the rest. The first must pass before a mod change is handed off:

```bash
node scripts/test-app-mods.mjs
```

It runs the mods under Node with stand-ins for `gpui-kit`, `gpui-base` and
`herdr` (chainable records whose `child`, `when` and `on_*` calls are kept,
so a test reads each view's texts and calls its click and key handlers) and
canned answers for `git` and `corgi`; add a case there for what you change.
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
cargo build --release                  # the mods run target/release/corgi
scripts/app-preview start              # again after an edit: installs the mods again
scripts/app-preview do projects.open   # any command of `herdr-app commands`, or a mod's own
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
(`/tmp/corgi-ap-…/home/mods/projects/main.js`; the app reloads it) whose
command loop calls the handler the click calls (`this.askMerge`,
`this.runMerge`, `this.closeWorker`), then `do projects.<command>`; `start` again puts the
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
  write (the Projects mod reorders them with `space.move`).
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
(see "Testing on hc") shows hc agents, a MERGE tag and the daemon line; stop
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
OpenCode plugin). It checks the mods with the real `herdr-app check` and
changes the layout with the worktree's `target/release/corgi`; with no
window on that home the shipped `main` is cloned to `corgi` by hand
(`layout.clone` needs the window). The preview window is the place to see
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

The Projects mod's `⏸ Agents` runs `corgi pause --json` and `▶ Agents`
`corgi resume --json` (`src/app/hc_pause.rs`, `crates/corgi-core/src/hc/pause.rs`).
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

The Projects mod also reads the Corgi of each other machine the window
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
issuer and two users (`jane:fake-token` may build and abort,
`viewer:viewer-token` only reads). **Never trigger, abort or change
anything on a real Jenkins**; a real one is only ever read. To look at the
mod, use the preview window with the fake server, never a real token (a
screenshot would show its settings):

```bash
node herdr-app/mods/jenkins/tests/fake-jenkins.mjs 48765 &   # stop it by its pid afterwards
scripts/app-preview start
home=$(ls -d /tmp/corgi-ap-*/home)
HERDR_APP_BIN=target/app-preview/Herdr.app/Contents/MacOS/Herdr herdr-app/mods/jenkins/install.sh --home "$home"
mkdir -p "$home/jenkins"
printf '{"url":"http://127.0.0.1:48765","user":"jane","token":"fake-token","folders":"team-dev, team-prod"}\n' >"$home/jenkins/settings.json"
sleep 1                                                      # the app sees a new mod folder a moment later
scripts/app-preview do tab.new --name Jenkins --mod jenkins  # or: do mod.open --name jenkins
```

The window takes no clicks, so a view behind one (a job, a log, the build
form, the abort question) is reached as for the Projects mod: in the
preview home's copy of `main.js`, add commands to the `nextCommand` loop
that call what the click calls (`this.show({ kind: "job", place })`,
`this.askBuild(place, cx)`, `this.build(cx)`, `this.askAbort(place, n,
cx)`, `this.abort(cx)`; a place is `{ path: "/job/team-dev/job/api_master",
label: "team-dev/api_master" }`), then `do jenkins.<command>`. What the
app's script engine has and lacks, found there: `fetch` gives a response's
`status`, `ok`, `url`, `text()` and `json()` but no headers (so a console
log is followed by counting the bytes read, not by `X-Text-Size`), follows
a redirect after a POST, and rejects when the host does not answer; there
is no `TextEncoder` (`util.js` encodes UTF-8 itself) and no `process.env`;
`writeTextFile` replaces a file and keeps its mode.

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
the mods into the app's home when they differ, and makes sure the layout
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
install` starts it again). The dashboard in the Herdr TUI is a process of its own: quit and reopen
it to run a rebuilt binary (and `herdr plugin link "$PWD"` after a change to
`herdr-plugin.toml`); do not restart Herdr.

`build.rs` runs again when the checkout's `HEAD` or a file that goes into
the binary changes (its `INPUTS`: `src`, `crates/corgi-core`, `supervisor`,
the manifests), so every commit rebuilds the root crate once, and a build
with uncommitted changes to those says `modified`. A new directory whose
files are compiled into the binary must be added to `INPUTS`, or a change
there keeps the old commit and no `modified`.

## Testing Herdr's hooks

`cargo test hook` covers `corgi hook`: the core's tests drive the lock and
dirty-flag protocol, the event parsing and the watchdog, and `tests/hook.rs`
runs the real binary, sixteen at once, against `test_support::FakeHerdr`.
To try the hooks in a real Herdr, register the plugin only in a private
home: plugin registrations are per user, not per session. Run Herdr with
`env -i` and a private `HOME` and `XDG_*` directories (a short
`XDG_CONFIG_HOME` such as `/tmp/<name>`, since macOS limits socket paths),
a throwaway `--session`, and link a copy of the manifest without its
`[[build]]` whose `./target/release/corgi` is a symlink to the worktree's
build; delete the session afterwards and check `herdr session list`. Then:

- Keep `XDG_STATE_HOME` outside `/tmp` and `$TMPDIR` (say under
  `target/`): Corgi's recovery refuses to coordinate deliveries from a
  volatile state directory, so no wake is typed in. `tests/hook.rs` keeps
  its state under `CARGO_TARGET_TMPDIR` for the same reason.
- Herdr types into an agent (`agent.prompt`) only while the agent is the
  pane's foreground process. A fake agent is a small program compiled with
  `rustc` and installed as `claude` (and `codex`) in a directory of its own:
  it ignores its arguments, echoes each line, prints Claude Code's working
  line (`⏵⏵ working · esc to interrupt`: Herdr's pattern needs whitespace,
  `·` or the line's end after "interrupt") for a few seconds per prompt so
  Herdr sees a turn, writes the handover note when a line asks for one, and
  exits on `/exit`. A copied system binary must be signed with
  `codesign -s - -f`, or macOS kills it.
  It must clear its screen when a turn ends, or Herdr keeps reading the
  leftover "esc to interrupt" line as working. Herdr 0.9.5's
  `pane report-agent` CLI has no `--session-start-source`, so the note
  on it below cannot be followed through the CLI. Herdr judges a Codex pane's state from its
  screen, not from `herdr:codex` reports, so a fake `codex` that does not
  draw Codex's own working line always reads idle.
- Keep the real agent CLIs unreachable. A login bash runs macOS's
  `path_helper`, which puts `/etc/paths.d/homebrew` (`/opt/homebrew/bin`,
  where `claude` lives) ahead of anything prepended to `PATH`, so set the
  server's `SHELL` to a wrapper that runs `bash --noprofile --norc -i` with
  `PATH` the fakes' directory and the system's, and keep the server's own
  `PATH` the same. Before typing into a pane, check with
  `herdr pane process-info --pane <id>` that each foreground process's
  executable (`lsof -a -p <pid> -d txt`, not `ps -o comm`, which gives only
  the name it was started by) is a fake. If a real `claude`, `codex` or
  `node` shows up under the private server, stop that server
  (`herdr session stop <name>` with the private environment).
- **Never kill processes by name, pattern, process group or user**: no
  `pkill`, `killall`, pgrep-and-kill, kill loops or watchdogs. One such
  loop (`pgrep -g ""` selects unrelated processes on macOS) killed every
  agent on the machine. Kill a process only by PID, once, after walking its
  ppid chain to your own private server's PID and checking with `lsof` that
  its executable is one of your fakes. After a live handoff the panes'
  processes no longer descend from the new server, so that walk fails:
  tell a fake by the pane's `herdr pane process-info` and its executable's
  `lsof` line instead, and still kill nothing by name, pattern or group.
- `herdr update --handoff` is disabled for Homebrew installs. To hand off a
  private server anyway, send it `server.live_handoff` on its socket, with
  `import_exe` the Herdr binary to take over (the same one will do):
  `{"id":"h","method":"server.live_handoff","params":{"import_exe":"<herdr>"}}`.
- Report fake agents' states with a test `--source`. Only
  `--source herdr:claude` keeps an `--agent-session-id`, and Herdr then
  ignores the test source's states for that pane. **A pane with a
  `herdr:claude` session comes back after a Herdr restart or live handoff
  running the real `claude --resume <id>`** through the pane's shell, so
  the fakes must win on that shell's `PATH` (above) before any restart.
- `herdr plugin log list --plugin io.github.zyrre.corgi` shows each hook's
  line, its exit code and its time.
- A fake that reports a new native session (as Claude does after `/clear`)
  must pass `--session-start-source` (`startup`, `resume` or `clear`), as
  Herdr's integration does: without it Herdr keeps the old session. Herdr
  0.9.2 and later run the resume command Corgi reported for each pane after
  a restart, so fakes that log their argv show what every agent came back
  with; `markers/<socket>.launches.json` under the private state shows what
  Corgi reported. The Herdr 0.9.5 release binary may be downloaded into the
  worktree's `target/` (check its SHA-256 against GitHub's) and run only as
  a private server.

## Testing on hc (herdr-core)

`crates/corgi-core/src/hc/tests.rs` drives the hc client against a fake
`hc` script (canned output per command, or per call with `CMD.N.out`), and
`tests/open.rs` the real binary's `corgi open`, `projects`, `supervisor
--terminal`, `app-layout` and `close` against another (`CORGI_HC_BIN`), with a
private home and state. A command that asks `hc` only while its core runs
(`HcSeen::look` in `src/app/status.rs`: `corgi merge`, `corgi close`) sees
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
- A Herdr TUI pane's variables reach this shell, so to act as a plain
  terminal outside Herdr unset `HERDR_*` and `CLAUDE_CODE_*` (with `env -u`)
  and pass `--backend hc`; to act inside hc, run the command as
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

`crates/corgi-core/src/waker/hc/tests.rs` drives the daemon's passes
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

## Session recovery checks

`cargo test recovery` uses isolated native-store/config fixtures and fake Herdr
sockets, including both Codex and Claude resume adapters. Keep the normal-path
assertions: unchanged original workspace/tab/pane IDs, zero placement creation,
and no prompt. Never substitute newest-by-cwd display discovery for recovery
identity, and never use installed configuration or real conversations as failure
fixtures. `CORGI_RECOVERY_PREVIEW=1` disables the preview dashboard's waker;
combine it with a fake socket, private synthetic HOME/state/native stores and
CLI overrides for a safe labeled UI preview. It is not a production hold switch.
The [recovery contract](docs/session-recovery-design.md) records compatibility
limits and the user-waived live-Claude check; do not retry authentication errors.

`recovery::begin_delivery` writes the journal even when it only checks, so try
it on a copy of the state, never the real one. Keep the copy outside `/tmp` and
`$TMPDIR` (say under `target/`): outside `cfg(test)`, `Store::lock` refuses a
volatile directory.

A `held` note (`N inbox item(s) held`) can be a delivery in flight, since the
ids are recorded before the prompt and cleared once Herdr confirms it. Read
`deliveries` in `recovery/progress.json` again before calling it a quarantine.

## README screenshots and animation

Do not run `cargo readme-shots` or update anything in `docs/images/` for now;
the README and its images will be redone after the GUI work. Leave
`docs/images/` untouched, and discard any change a tool makes there.

## Releasing

Bump `version` in the root `Cargo.toml` (`[workspace.package]`, which both
crates take), `herdr-plugin.toml`, `omarchy/manifest.json` and `VERSION` in
`herdr-app/mods/corgi/main.js` (and run
`cargo build` so `Cargo.lock` follows), commit, and push a `vX.Y.Z` tag
matching it. The Herdr app mod installs exactly its `VERSION`, from that
tag's `herdr-app/install.sh`, so the tag must hold the installer.
`.github/workflows/release.yml` then builds the four binaries and publishes
the GitHub release with them and their `SHA256SUMS`; it refuses a tag that
does not match all three versions. Its last job, `omarchy`, then publishes the
Omarchy bar widget (below).

After the artifacts exist, independently download all four binaries and hash
those bytes; compare with the published `SHA256SUMS`, GitHub asset digests and
retained CI/publication evidence. Review and commit `checksums/vX.Y.Z.sha256`
in a follow-up source change. Each line is exactly `vX.Y.Z <lowercase 64-digit
SHA256> <asset filename>` with no header, comments, blank lines or duplicates;
the version in every row must match the filename and `herdr-plugin.toml`.
Use the four exact platform asset names from the release workflow. Never
replace existing pins just because mutable release assets changed: investigate
the discrepancy. Keep verification evidence with the review. Until reviewed
pins reach the source checkout users install, that version builds from source.
The release tag need not contain its own future binary hashes, and the release
workflow must not automatically grant trust to its output by committing pins.

## The Omarchy bar widget

`omarchy/` is the widget's source, and is published as it stands, so
`omarchy/README.md` is written for its users; developer notes stay here. The
widget has no program of its own: every `--bar-*` command it runs is in the
`corgi` binary, which it finds on `PATH` (`~/.local/bin/corgi`, linked by
`herdr plugin install`). Keep `omarchy/` free of symlinks and of anything
that needs building, since Omarchy installs by `git clone` and runs nothing.
Text that shows agent output, commands or names uses
`textFormat: Text.PlainText`; the one rich text, `ActionDialog`'s spans, goes
through `Model.escapeHtml`.

Before handing off a change to `omarchy/`, run both validators the plugin
must pass. Omarchy's is a bash script that runs on macOS (it needs `jq`); the
marketplace's is `validateManifest` in `scripts/build-catalog.mjs` of
omacom/omarchy-plugin-marketplace, after `npm ci` there:

```bash
bash <omarchy>/bin/omarchy-plugin-validate "$PWD/omarchy"
CATALOG=<marketplace>/scripts/build-catalog.mjs node -e 'import(process.env.CATALOG).then(m => m.validateManifest(JSON.parse(require("fs").readFileSync("omarchy/manifest.json", "utf8")), "manifest.json", {community: true}))'
```

Pass the module's path in a variable, not as an argument: as `argv[1]` it
takes itself for the main script and starts a catalog build.

The marketplace accepts `barWidget.defaultSection` only as `left`, `center`
or `right`, though Omarchy knows more sections. Omarchy's shell reads no
`activation` and no bar-widget `aliases`, and hands a widget its settings
without filling in `defaults`, so `Panel.qml` keeps its own fallbacks and
treats an empty setting as unset.

For changes to the widget's existing-agent prompt path, `cargo test` exercises
the real `--bar-prompt SOCKET PANE` binary against a fake Unix socket. The
endpoint accepts one newline-terminated JSON object with a nonempty `text`
string on stdin; embedded newlines must be JSON-escaped. Keep prompt text out
of argv/environment and discard raw parser/Herdr errors, which can echo it.
Never automatically retry an unconfirmed delivery.

On an Omarchy machine, after `cargo build --release`, also run the prompt test.
Callers must first provide a supported isolated Wayland compositor and set
`CORGI_PROMPT_TEST_DISPLAY` to its absolute socket path. The test refuses the
inherited desktop display; keep the isolated compositor's host windows off
workspaces 1–4. With `isolated_wayland_socket` set to that already-started
compositor's socket:

```bash
CORGI_PROMPT_TEST_DISPLAY="$isolated_wayland_socket" python3 scripts/test-omarchy-prompt.py
```

It opens a Quickshell preview on that display with the actual widget and a
controlled Herdr socket, checks argv/environment while delivery is held, and
verifies success, failure, explicit retry and dismissal during send. Startup
assertions follow `Process.started`: the `running` getter can become true before
its notifications update QML bindings. It needs Linux `/proc` and the Omarchy
shell QML (`OMARCHY_PATH` defaults to `/usr/share/omarchy`). It changes no installed
files or registrations and closes its own preview; callers manage the isolated
compositor's lifecycle.

### Trying a change on an Omarchy machine

The running Omarchy bar loads `~/.config/omarchy/plugins/io.github.zyrre.corgi/`,
not the worktree's `omarchy/` directory. Copy the changed files there (a
symlinked folder fails Omarchy's validation); `--exclude .git` keeps the
checkout `omarchy plugin add` made:

```bash
rsync -a --delete --exclude .git omarchy/ ~/.config/omarchy/plugins/io.github.zyrre.corgi/
```

That widget runs `corgi` from `PATH`, the installed plugin's binary, not a
worktree build. To try a build, set the widget's **Corgi command** setting to
that worktree's `target/release/corgi`, and clear it afterwards. Avoid
overwriting unrelated changes from other agents.

For QML changes, plugin rescans can retain cached components even when
`omarchy-shell shell rescanPlugins` succeeds. Once the installed files are
updated, use these two commands to load fresh QML and open the popup:

```bash
omarchy restart shell
omarchy-shell shell summon io.github.zyrre.corgi
```

This restarts only the Omarchy shell, not Herdr. Wait for the shell to become
ready before summoning, then verify the visible popup after its opening
animation finishes. Do not assume that a successful rescan made QML edits live.
If sandbox restrictions block shell IPC, run these commands with the required
sandbox escalation rather than interpreting the failure as a stopped shell.

### Publishing

Each release's `omarchy` job runs `scripts/publish-omarchy.sh`, which
commits the tag's `omarchy/` files and `LICENSE` as the whole root of
`zyrre/omarchy-corgi` ("Corgi vX.Y.Z") on top of its history and pushes with
a deploy key. `omarchy plugin update` only fast-forwards, so that history
must never be rewritten or force-pushed. To publish again after a failure,
re-run the job. Before handing off changes to the script, run its test, which
publishes into scratch repositories:

```bash
bash scripts/test-publish-omarchy
shellcheck scripts/publish-omarchy.sh scripts/test-publish-omarchy
```

The job is skipped, with a notice, until this one-time setup is done:

1. Create `zyrre/omarchy-corgi` on GitHub: public and empty (no README,
   license or `.gitignore`).
2. Make a key pair with no passphrase, used for nothing else:
   `ssh-keygen -t ed25519 -N "" -C "corgi release" -f omarchy-deploy-key`.
3. In `zyrre/omarchy-corgi`, Settings → Deploy keys → Add deploy key: paste
   `omarchy-deploy-key.pub` and tick **Allow write access**.
4. In `zyrre/corgi`, Settings → Environments → New environment
   `omarchy-plugin`; under Deployment branches and tags choose **Selected
   branches and tags** and add the tag rule `v*`. In that environment add the
   secret `OMARCHY_DEPLOY_KEY` with the whole of `omarchy-deploy-key` (the
   private key). Then delete both key files.

The job names that environment, so GitHub creates it on the first release if
step 4 was skipped, and a repository secret of the same name works too, but
without the tag rule.

Although Corgi is a space-constrained TUI, use clear icons and small ASCII art
where they improve scanning or personality. Keep text labels and contrast
strong enough that decoration never interferes with readability.

Use color often to clarify state, hierarchy, and actions. Prefer semantic ANSI
terminal colors over fixed RGB values so Corgi automatically follows the active
Omarchy theme palette; reserve high-contrast colors for important information.
Take every color from a role (`theme::roles()`), adding a role in
`crates/corgi-core/src/palette/` when none fits, so it follows the terminal's
palette, light or dark; only the palette and theme modules and tests write
color literals; `src/theme/` maps the core's colors onto ratatui's. To check
a color change without a light terminal or an Omarchy machine, write the
theme preview with
`CORGI_PREVIEW_DIR=<dir> TMPDIR=/tmp cargo test --lib theme_preview -- --ignored`
and read its files with `less -R`.
