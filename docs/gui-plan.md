# Corgi GUI: plan for phases 1 and 2

The brief w-gui-research left for the supervisor on 2026-10-28, kept here so
workers can read it. The background, measurements and decisions are in
[gui-research.md](gui-research.md).

## The user's instruction

The user asked me to report this to you so that you start workers on phases 1
and 2 below. Keep them working and testing until both phases are complete.
Stop only for a decision that really needs the user; everything else (design
details, splits, ordering, fixes) is yours and the workers' to settle.

## Where the material is

- **The study:** `docs/gui-research.md` on branch `worktree/brave-valley-36f8`
  (13 docs-only commits, the last `2defde5`). It is not merged. Merge it first
  (it is ready; I'd tag it `merge`), or let workers read it from that branch.
  Sections that matter most:
  - **"Decisions (2026-10-28)"**: what the user decided.
  - **§5:** the core/frontend split, with file and line references.
  - **§9–§10, §12:** GUI prototypes and measurements on macOS and Hyprland.
  - **§13:** Herdr's hooks as the backend.
- **Prototypes and spikes:** orphan branch `gui-prototypes` on origin, pushed;
  **never merge it into main**.
  - `egui/` and `gpui/`: the GUI prototypes (`gpui/` uses Herdr UI's terminal
    painter).
  - `hook-spike/`: the Herdr plugin hook test.
  - `results/omarchy.md`: the Hyprland results the user ran on their Omarchy
    machine.
  - A README with safety rules.

## Decisions the user made

1. **One GUI window with the terminal inside it.** Agent terminals are shown
   inside it; external terminal windows and embedding the user's own terminal
   app were rejected. Performance comes second only to that.
2. **Terminals come through Herdr's endpoint protocol (generation 1),** in
   phase 3, which is not now. They are painted natively with Herdr UI's
   painter (penso/herdr-gpui, Apache-2.0); there is no client-side terminal
   emulator.
3. **Toolkit: GPUI** (`gpui-pre =0.3.6`, the version Herdr UI's painter uses).
   It passed on macOS and on Hyprland (Omarchy), running natively on Wayland.
4. **No dock library.** Layouts are fixed:
   - an overview;
   - a focus view with one or two terminals (phase 3);
   - side panels such as the supervisor and open questions.

   Splits are resizable and sidebars can be opened and closed, with sizes
   remembered. Write this as plain GPUI layout.
5. **Herdr is the always-on backend.** Corgi runs no process of its own: no
   daemon. The plugin's `[[startup]]` and `[[events]]` hooks do the always-on
   work. Every window (GUI, TUI dashboard, Omarchy widget) can be closed at any
   time and reads the current state when opened.
6. **The idle prompt-cache handover runs only while a Corgi window is open.**
   Herdr has no timer hooks. Context handovers are event-driven and always on.
7. **Not decided:** automatic session saving. Saving stays manual, as the
   recovery design says, unless the user decides otherwise.

## Phase 1: core, `corgi hook`, `corgi serve`

The TUI dashboard must behave exactly as before throughout. Every worker runs
the AGENTS.md handoff: `cargo fmt --check`, clippy, `TMPDIR=/tmp cargo test`
on macOS, CI on Linux and macOS, and a release build.

**1a. Core split (§5).**
- Make a reusable core crate, or a clearly separated module tree, free of
  ratatui and crossterm.
- Move into it:
  - `waker`, `markers`, `rows`, `draft`, `first_prompt`, `sweep`;
  - `RefreshState::run`/`refresh` (`refresh.rs:143-212`);
  - `launch::launch_agent` (`launch.rs:387`);
  - the merge Git steps (`merge.rs:436-550`), close, and the `project_main`
    client functions.
- Invert `recovery/mod.rs`'s calls into `app/recovery.rs:649-710`
  (`recovery/mod.rs:460`, `:1147`, `:1240`).
- Split the theme into a palette with RGB roles plus a ratatui mapping.
  `Theme::rgb` already converts.
- Keep the core on `rust-version` 1.89. GPUI needs Rust 1.96, so the GUI
  crate gets its own `rust-version`.

**1b. `corgi hook`: Herdr's hooks as the always-on backend (§13).**
- **Manifest:** add `[[startup]]` and `[[events]]` to `herdr-plugin.toml`, at
  least for:
  - `pane.agent_detected`
  - `pane.agent_status_changed`
  - `pane.exited`
  - `pane.closed`
  - `workspace.closed`
- **The design rule: trigger, then reconcile.**
  - An event only says "something changed". The hook reconciles from Herdr's
    snapshot plus Corgi's records (`wake/<socket>.seen.json`, markers,
    inbox).
  - Only one reconciler runs at a time, under a lock. A hook that finds the
    lock taken marks the state as changed and exits; the running hook goes
    round once more.
  - Every hook enforces its own time limit and exits quickly.
- **Why that rule** (measured on Herdr 0.9.0):
  - Herdr runs at most 32 plugin commands at once, across all plugins, and
    **drops** events beyond that instead of queueing them.
  - A hung hook keeps its slot forever; there is no timeout.
  - Hooks start in parallel and finish out of order.
  - The event JSON is thin: pane, workspace, agent and new status, but no
    previous status, `state_change_seq` or session. An unseen idle arrives as
    `done`.
  - `pane.closed` fires only for an explicit close; a shell exit gives only
    `pane.exited`.
  - Events while Herdr is down are lost, and token changes trigger nothing.
- **What the hooks do:**
  - **Startup hook:** restore markers, which Herdr drops on every restart
    *and* every live handoff, and catch up on everything missed.
  - **`agent_detected`:** record the session at once. This fixes agents
    becoming invisible after restarts.
  - **`agent_status_changed`:** queue wakes to the supervisor's inbox and
    deliver them when it is idle or done. Start context handovers at the end
    of a turn.
  - **Exits and closes:** bookkeeping.
- **The wake lock:** decide how dashboards still holding it interact with the
  hooks. Nothing may be delivered twice.
- **Measured (§13):**
  - about 7 ms from a status change to the hook starting;
  - about 11 ms from a worker going idle to the text appearing in the
    supervisor;
  - the startup hook ran about 45 ms after the server started, and after a
    live handoff;
  - under 1 ms of Herdr CPU per hook.
- **Update ARCHITECTURE.md:** it currently says there is no daemon and that
  only the dashboard wakes supervisors. Update README.md too.

**1c. `corgi serve`.** JSON lines over stdio:
- a structured snapshot of rows and cards (data, not ratatui spans), then
  deltas;
- commands: prompt, spawn, merge, close, tag, focus, digest lookups,
  answering questions;
- it runs locally or as `ssh host corgi serve`;
- snapshot-tested.

It is the GUI's data source while a window is open. Moving the Omarchy widget
from `--bar-stream` onto it can wait.

**Done when:**
- The TUI works unchanged.
- The hooks do all the always-on work with no dashboard open.
- `corgi serve` drives the actions.
- Everything is tested with fake Herdr sockets, plus a real hook test in an
  isolated Herdr (see Safety).

## Phase 2: the first GPUI window (overview, no terminals)

A new GPUI app (`gpui-pre =0.3.6`) that spawns `corgi serve` locally or over
`ssh`.

**Shows:**
- project cards and agent rows;
- the plan-usage header;
- the expanded transcript tail;
- a side panel for the supervisor and open (numbered) questions.

**Actions:**
- prompt, answer numbered questions;
- new agent: preset defaults, inline lists rather than modal pickers,
  keyboard first, as in the TUI form;
- start or focus the supervisor;
- merge, with confirmation and outcome;
- close.

**Layout and look:**
- Fixed layouts: resizable splits and sidebars that open and close, with
  sizes remembered. No dock.
- Follows the system light/dark scheme through the theme's roles. Brand
  colours stay fixed.

**Behaviour:**
- Reconnects with backoff and shows the last state dimmed while
  disconnected.
- Never calls Herdr's JSON focus methods in GUI mode: they move every attached
  Herdr client.

**Lessons from the GPUI prototype on Linux:**
- enable `gpui-pre-platform`'s `wayland` feature;
- set the Wayland `app_id`;
- choose a monospace font per platform (no Menlo off macOS);
- pause or deactivate work while the window is hidden. On Hyprland, GPUI
  otherwise kept decoding updates for an invisible window.

**Packaging:** a Linux tarball and an unsigned macOS build are enough for now.

**Done when:** it is verified on macOS and on the user's Omarchy (Hyprland)
machine. The Omarchy run needs the user. Push a test branch and give them a
ready prompt, as was done with `gui-prototypes`; ask only for that run.

## Phase 3 (later, not now)

Terminals in the focus view through endpoint connections, each to the agent's
own single-pane tab, painted by Herdr UI's painter. The rules learned (§9):
- only the GUI's own connections navigate;
- park inactive connections on an empty tab;
- keep two connections warm;
- deactivate connections while the window is hidden;
- copy beyond the visible screen comes from a Herdr read.

## Safety rules for every worker

- **Never act on the user's live Herdr session** beyond reads. Tests use
  throwaway `herdr --session <name>` servers, with the inherited `HERDR_*`
  variables unset, deleted afterwards.
- **Plugin registrations are per user, not per session.** Hook tests must run
  Herdr under a private `HOME` and `XDG_*` directories, with a short path for
  macOS sockets (`gui-prototypes:hook-spike/penv.sh`). Never link a test plugin
  into the real home.
- **Windows opened on the Mac** must be moved to AeroSpace workspace 4 at
  once. On Hyprland, use runtime-only window rules; the user's Hyprland uses
  the Lua config, so `hyprctl eval`, not `hyprctl keyword`.
- **Never submit prompts to real agent CLIs** in tests.
- **Never send the user's email address** to external services, for example
  in a User-Agent header. A research subagent did once.

## Open items for the user (only if they come up)

- Automatic session saving: stays manual unless the user decides.
- Merging the study branch, if you do not tag it yourself.
