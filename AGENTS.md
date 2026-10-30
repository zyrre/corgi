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
both. Line breaks in `supervisor/ROLE.md` and `supervisor/HANDOVER.md`
matter, since those tests match their text with `contains`.

On macOS, copying a rebuilt binary over an existing file (say, a copy of
`target/release/corgi` kept to compare before and after) gets it killed with
exit 137, because the kernel caches the old file's code signature. `rm` the
destination before copying.

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
on macOS). Worktrees opened with `herdr worktree open` are linked worktrees
to Corgi; plain workspaces on a `git worktree` are not. The private home
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
frame); other steps still see it, and AeroSpace does not list the window. Move the window to AeroSpace workspace 4 the moment it
appears (see the user's instructions); `render_to_image` still captures it
there.
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
Kill the window's own process, not a wrapper shell function's subshell,
and check that its `corgi serve` child is gone afterwards.

The focus view's terminal connects to the throwaway session's
`herdr-client.sock` itself. After changing `src/endpoint/`, run
`cargo test -p corgi-gui -- --ignored live_herdr`, which drives the adapter
against a Herdr of its own (private home, `/bin/sh` panes) and removes it.
To try the terminal by hand, run a program in the agent pane that turns on
bracketed paste and the kitty keyboard protocol, as Claude Code does, and
prints the bytes each key arrives as: a real Claude Code reads its
credentials from the Keychain on macOS, which these runs must not touch.
The script steps `drag`, `wheel`, `link`, `ime`, `hide` and `show` drive the
terminal; `CORGI_GUI_LATENCY=1` logs key-to-surface and key-to-frame times,
`CORGI_GUI_DEBUG_ENDPOINT=1` each snapshot, request and answer, and
`CORGI_GUI_URL_LOG=<file>` writes a clicked link there instead of opening a
browser. Copy and paste use the real clipboard: keep the user's text and put
it back. To see what a user's own Herdr window shows meanwhile, attach
`herdr --session NAME` in a pty of a fixed size (its first-run welcome and
integration questions take Esc; install nothing). A tab that a JSON focus
call (`herdr workspace focus`) moved that window to is owned by no client
until something is typed there, which is when a new connection could claim
its size.

For a screenshot that needs no real flow, `--corgi` can name a small
program instead: corgi-gui runs it with `serve …`, and any program that
writes a `Hello` and a `Snapshot` (built with `corgi_core::protocol`, one
JSON line each) and answers each `Request` line on stdin draws the window
with no Herdr at all, and can delay or refuse a reply to show a state.
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

Text on a fill other than the background (a badge, the selected row's bar,
a dialog) takes its color through `Colors::badge_text` or
`Colors::secondary`, which keep the role unless it all but vanishes on
that fill (Mocha's muted and black on its greys), so a theme keeps its
look wherever its roles read.

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

Before handing off changes to `scripts/install.sh` or `scripts/link-command.sh`,
run their shell test, which serves a fake release from a local directory and
uses a fake `cargo`:

```bash
bash scripts/test-install
shellcheck scripts/install.sh scripts/link-command.sh scripts/test-install
```

After reviewed changes are merged into the primary checkout, make them live
from that primary checkout with `cargo build --release` and
`herdr plugin link "$PWD"`. Reload Herdr configuration when actions or plugin
metadata changed. Quit and reopen only the Corgi dashboard when the running
process must pick up a rebuilt binary; do not restart Herdr.

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
- Keep the real agent CLIs unreachable. A login bash runs macOS's
  `path_helper`, which puts `/etc/paths.d/homebrew` (`/opt/homebrew/bin`,
  where `claude` lives) ahead of anything prepended to `PATH`, so set the
  server's `SHELL` to a wrapper that runs `bash --noprofile --norc -i` with
  `PATH` the fakes' directory and the system's, and keep the server's own
  `PATH` the same. Before typing into a pane, check with
  `herdr pane process-info --pane <id>` that each foreground process's
  executable (`lsof -a -p <pid> -d txt`, not `ps -o comm`, which gives only
  the name it was started by) is a fake, and run a loop that kills any
  other `claude`, `codex` or `node` of the private home.
- Report fake agents' states with a test `--source`. Only
  `--source herdr:claude` keeps an `--agent-session-id`, and Herdr then
  ignores the test source's states for that pane. **A pane with a
  `herdr:claude` session comes back after a Herdr restart or live handoff
  running the real `claude --resume <id>`** through the pane's shell, so
  the fakes must win on that shell's `PATH` (above) before any restart.
- `herdr plugin log list --plugin io.github.zyrre.corgi` shows each hook's
  line, its exit code and its time.

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

## README screenshots and animation

The images in `docs/images/` are drawn by Corgi's own UI code from made-up
agents in `src/readme_shots.rs`; nothing of the machine they are drawn on
may appear in them (no real home paths, work names or e-mails). After a UI
change, draw them again with:

```bash
cargo readme-shots
```

It writes the SVG screenshots and `merge-outcomes.gif`, a merge in the
merge popup that ends and rests on its success stamp.
The GIF plays the real effects of `src/motion.rs` on a manual clock at 25
frames a second, and
rasterizes each frame with resvg (a dev-dependency) and the system's
monospace fonts, so it needs no tool beyond Cargo. It takes about half a
minute; keep the GIF well under 2 MB. Delete an image the README stops
using.

Since the GIF's text comes from the system's fonts, `merge-outcomes.gif`
differs from machine to machine. To check that a change left it alone,
compare it with one drawn from a build of the base commit on the same
machine, not with the committed file (`git archive <base>` into a scratch
directory gives that build without touching a checkout).

## Releasing

Bump `version` in the root `Cargo.toml` (`[workspace.package]`, which both
crates take), `herdr-plugin.toml` and `omarchy/manifest.json` (and run
`cargo build` so `Cargo.lock` follows), commit, and push a `vX.Y.Z` tag
matching it.
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
