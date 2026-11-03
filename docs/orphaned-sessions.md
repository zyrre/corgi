# Find orphaned sessions

Design chosen by the user, 3 November 2026 (the supervisor's Q57–Q63).
Nothing here is built yet beyond the two plain fixes named under "Found on
the way". "Decisions" at the end records each choice and the options it
was chosen over.

An orphaned session is an agent in Herdr that Corgi gets wrong in one of
four ways:

- **hidden**: Herdr lists the pane as an agent, but Corgi shows no row;
- **not detected**: an agent CLI runs in a pane, but Herdr does not list it
  as an agent, so neither does Corgi;
- **wrong role**: a supervisor that shows as a worker, or a worker the
  supervisor has lost track of;
- **wrong project**: a row under another heading than the repository it
  works in, or outside the project its supervisor reads (`corgi fleet`
  and the digest filter on `project_root`, not on the heading).

Leftovers that hold no agent, such as an empty duplicate project
workspace, are not sessions, but the same pass finds them cheaply and the
dialog can offer to close them.

## How Corgi decides today

What Corgi shows comes from one `session.snapshot` per refresh
(`refresh::view`):

- A pane is a row only when `rows::has_real_agent_session` holds: Herdr
  gives it a non-empty `agent_session`, or it carries Corgi's
  `corgi_request` token, or a non-empty status-line `session` token
  (decision 2026-10-08). A Codex pane in the home directory, in a
  workspace Corgi marked as an agent workspace, is also admitted when
  `pane.process_info` shows `codex` in the foreground there.
- A row is the supervisor when its `corgi_handler` token is
  `session:<id>` and `<id>` is the pane's live `agent_session`, or, for a
  legacy marker, the pane's name (`supervisor::is_supervisor`).
- The project is the workspace's repository root when the workspace is a
  Herdr linked worktree and the agent's cwd is inside its checkout;
  otherwise the cwd. The heading is the repository name, the Corgi project
  workspace's root, the workspace label, or the cwd's last component, in
  that order (`rows::agent_project`, `rows::project_group`). The home
  directory is Scratch.
- The supervisor's ledger lists each worker it dispatched by name, branch
  and cwd, under the request id of its brief; the digest joins it to the
  running agents by name (and, since the fix below, by request id). Only
  `corgi spawn --request-id` tags a worker with `corgi_request`; a worker
  started from the new-agent form has none.

Markers are put back after a Herdr restart from Corgi's record, but only
on the same native session (see "Herdr restarts" in ARCHITECTURE.md).

## Causes

Each cause with what goes wrong, the evidence Corgi can read without
changing anything, and how sure an automatic tag could be. *Certain* means
the evidence was written by Corgi itself for this very process or session;
*likely* means a strong match that a user's own unusual setup could fake;
*guess* means a hint worth showing but not acting on.

| # | Cause | Symptom | Evidence | Confidence |
|---|-------|---------|----------|------------|
| 1 | Agent without an identity: a Codex pane outside Scratch (the Codex integration is not installed on this machine: `herdr integration status`), a harness Herdr has no session integration for, or a Claude pane before its first hook report | hidden | `pane.process_info`: foreground executable `codex`/`claude` (by `argv[0]`'s file name, as the scratch rule does) with the pane's cwd | *likely* an agent; no session file can be tied to it |
| 2 | Herdr's detector does not recognise the agent: a wrapper script, `npx`, a CLI Herdr has no manifest for, an agent inside `ssh` or `tmux` | not detected | `pane.process_info` of every non-agent pane: a known CLI in the foreground; `herdr agent explain` says why not | *likely* for `claude`/`codex`, *guess* for `node` |
| 3 | Supervisor marker cleared: the pane's native session changed (`/clear`, `/resume` of another session), so reconciliation treats the marker as stale and removes it (`markers::reconcile`, by design against pane reuse) | wrong role: shown as a worker, no wakes, no handovers | the foreground process's argv: `--append-system-prompt-file <state>/supervisors/<name>/ROLE.md` (Claude) or `--add-dir <state>/supervisors/<name>` (both harnesses); the pane is in the root tab its project workspace records (`corgi_project_main_tab`) | *certain* when the argv names the project's own state dir and no other live process does |
| 4 | Supervisor lost in a restart before any pass recorded its session, or a legacy name marker after a rename with no recorded session | wrong role | as 3, plus a name like `<project>-supervisor` or `steward-<project>` | as 3; the name alone is a *guess* |
| 5 | Worker renamed since its spawn (seen today, below) | the supervisor's digest said `NOT RUNNING`; `corgi tag` missed its open questions | `corgi_request` token = ledger `id` | *certain*; **fixed**, see below |
| 6 | Worker's request id gone: started from the form, by hand, or its token cleared as stale after `/clear` | wrong role: the supervisor treats it as the user's own session (decision 2026-09-24) | a ledger entry whose `cwd` and `branch` equal the pane's checkout and branch; its first transcript prompt equal to `briefs/<id>.md` | *certain* with the brief match, *likely* with cwd and branch, *guess* by name |
| 7 | Plain workspace on a `git worktree add` checkout, not opened with `herdr worktree open`: Corgi does not count it as a linked worktree (AGENTS.md), so its project is the checkout itself | wrong project: a heading of its own, outside `fleet`, no merge | `git rev-parse --git-common-dir` in the cwd gives the main repository | *certain* about the repository; whose worker it is as 6 |
| 8 | Agent's cwd moved out of its workspace's checkout, or a pane in a subdirectory of a workspace Herdr recorded no repository for (created before `git init`) | wrong project | `git rev-parse --show-toplevel` and `--git-common-dir` in the cwd | *certain* about the repository |
| 9 | Agent in a directory that is no repository and not home (`/tmp`, a downloads folder) | its own heading, `unknown project` at worst | none beyond the cwd | nothing to tag; offer Scratch-like "leave" or close |
| 10 | Agent in another Herdr session (`herdr --session X`) | not shown: Corgi reads one socket | `herdr session list` | out of scope |
| 11 | Duplicate empty Corgi project workspace, left when a launch ran while Herdr had lost Corgi's marks; only a later close in that project retires it | not a session; clutter, and the dashboard's own lookups pick by snapshot order | `corgi_workspace_role=project-main` for a root another workspace also has, no agent in any pane, one tab with at most one pane (the rule of `project_main::retire_duplicate_project_mains`, which close runs) | *certain* |

Not an orphan: an agent the user started by hand in a project's worktree
shows under its project and is the user's own; Scratch agents in `~` are
grouped under Scratch by design.

## Today's survey

Read-only, on the user's Herdr (`default`, Herdr 0.9.0), with
`herdr api snapshot`, `herdr pane process-info`, `herdr agent explain` and
the supervisors' ledgers. Names and cwds only.

At 2026-11-03, before the restore that stopped the machine's agents:

- 16 agent panes, all with a Herdr `agent_session`, so none hidden.
- 3 supervisors, each with a `session:` marker matching its pane:
  `steward-instat-backend` (`~/repos/instat-backend`), `steward-corgi`
  (`~/repos/corgi`), `corgi-project-maintainer` (`~/repos/project-maintainer`).
- 13 workers, each with a `corgi_request` token, each in a Herdr linked
  worktree under `~/.herdr/worktrees/<repo>/` of its supervisor's
  repository: 10 of instat-backend, 3 of corgi.
- 4 panes Herdr does not list as agents, none running an agent CLI:
  `wJ:p1` (zsh, `~/repos/instat-backend`, Herdr's own root workspace for
  the repository), `w9X:p1` (zsh, `~/repos/instat-backend`), `wK8:p1`
  (the Corgi dashboard, `~/repos/corgi`), `wKJ:p1` (zsh, `~`,
  "corgi-gui (parked terminal)").

So nothing was orphaned in the sense of the first two causes. What was
found:

1. After the restore every agent came back under another Herdr name: the
   workers as `restored-<pane>` (`restored-wht` for the worker in
   `worktree-calm-river-7741`, formerly `w-cpu-alarm-fix`, and so on), the
   supervisors as `<project>-supervisor`. The supervisors kept their role
   (their markers hold the session, not the name) and the workers their
   request ids, but the ledgers still name the old workers: cause 5. The
   rename did not come from Corgi's code.
2. `w9X` is a second, empty `instat-backend supervisor` workspace beside
   the supervisor's own `w45`: cause 11.
3. The Codex integration is not installed, so a Codex worker started from
   the form in a project would be hidden: cause 1. None runs today.
4. Line 32 of instat-backend's `ledger.jsonl` is not valid JSON (a `\c`
   escape). The digest skips it with a warning; the supervisor wrote it.
5. 100 `markers/_tmp_corgi_refresh_wake_*.json` files in the real state
   directory, left by a test; **fixed**, below. The sweep deletes them a
   week after they were last written.

## Found on the way

Two plain bugs, each its own commit with a test:

- The slow-Herdr refresh tests' clients had no marker record of their
  own, so every `cargo test` wrote one record of a made-up supervisor into
  `~/.local/state/corgi/markers/`.
- The digest joined ledger entries to the live fleet by name only, and
  `corgi tag`'s open-questions check found entries the same way. Both now
  also match by the worker's `corgi_request` id, and through the name the
  request's own entry gives, so a follow-up entry written under the old
  name still matches; the digest writes `agent w-x (now restored-w1)`.

## Detection rules

One read-only pass over the snapshot, `pane.process_info` for the panes
that need it, Git, and Corgi's state files. It writes nothing, so it can
run whenever the dialog opens and as a `corgi orphans` command that prints
the candidates with their evidence (the first thing to build, and what the
dialog lists).

1. **Hidden** (cause 1): every snapshot agent that `refresh::view` drops,
   other than a dashboard pane. With a known CLI in the foreground at the
   pane's cwd it is a candidate; without, Herdr's detector was wrong and it
   is left out.
2. **Not detected** (cause 2): every pane not in `agents`, not a
   dashboard, whose foreground processes include `claude` or `codex` by
   executable name. This is one `pane.process_info` per non-agent pane;
   today that is four calls.
3. **Supervisor** (causes 3, 4): an agent that is not the supervisor but
   whose foreground process's argv names `<state>/supervisors/<name>/`,
   where `<name>` is its cwd's repository's directory name (the state dir
   is keyed by that name). When two live processes name the same state
   dir, a handover is under way and nothing is suggested.
4. **Worker of a supervisor** (causes 5, 6): an agent of a project with a
   ledger that no unfinished ledger entry matches by name or request id,
   but one matches by `cwd` and `branch`, or by its first transcript
   prompt against `briefs/<id>.md`.
5. **Project** (causes 7, 8): an agent whose `project_root` differs from
   the main repository `git rev-parse --git-common-dir` gives for its cwd,
   or whose cwd is in no repository and not home (cause 9).
6. **Leftover workspaces** (cause 11): as for close's duplicate cleanup.

## The user flow

The action is **Find orphaned sessions**: a button in corgi-gui's header
and the key `o` in the dashboard, asked for over `corgi serve` like the
other actions. It runs only when asked: no count in the header and no
dialog that opens by itself. It opens a list, one row per candidate
(made-up rows):

```text
┌ ◆ Orphaned sessions ─────────────────────────────────────────────┐
│ ● claude  wQ:p1      ~/repos/webshop-pay      worker, no ledger   │
│     Worktree of ~/repos/webshop; ledger w-pay by name only        │
│     (guess)                         [w] worker of webshop  ✓      │
│ ○ codex   wR:p2      ~/repos/scratchpad       not shown           │
│     codex in the foreground; Herdr has no session for it.         │
│     Run `herdr integration install codex`.  [l] leave  [x] close  │
│ ○ (empty) w9X        instat-backend supervisor  duplicate          │
│                                                        [x] close  │
│                        Enter apply · x close · l leave · Esc done │
└───────────────────────────────────────────────────────────────────┘
```

Each row offers what its evidence allows, the suggested choice preselected:

- **Supervisor of P**: sets the `corgi_handler` marker with the pane's
  session through `markers::mark_pane`, so the record keeps it across
  restarts. Refused while P has a running supervisor, and for a pane
  without a native session (there is nothing to bind the marker to).
- **Worker of P**: with a ledger match, sets its `corgi_request` back to
  the entry's id, so the digest and `corgi tag` find it. Without one there
  is nothing to write: an untracked worker is the user's own, which the
  row says.
- **Close**: through `close::close`, which needs a `DashboardAgent`; a
  hidden or undetected pane gets one built from the snapshot. In a
  workspace Corgi created (marked `project-main` or `agent-workspace`) it
  is today's flow; in any other workspace it closes only the pane, with
  Herdr's `pane close`, leaving the user's other tabs and shells alone.
- **Leave**: nothing; the row is not offered again until its evidence
  changes (kept in the state directory by pane and session).

There is no project choice: the project follows from Git (below). A Codex
agent with no session gets a row only where it cannot be admitted (a
workspace Corgi did not create), and the row names `herdr integration
install codex`.

Nothing is ever prompted, renamed or restarted. A pane whose occupant
changed since the list was read (another session, another process) is
refused at apply time, as the markers' restore does.

### Tagged without asking

Two cases are proven by evidence Corgi wrote itself, and the hooks' pass
applies them as it puts back lost markers, naming what it did in its
status line (a hook's log line, a dashboard's status):

- **A supervisor**: an agent that is not the supervisor, whose foreground
  process's argv names its project's `<state>/supervisors/<name>/ROLE.md`
  or state directory, when no other live process names the same state
  directory (a handover under way) and the project has no running
  supervisor. It gets the `session:` marker of its native session.
- **A worker's request id**: an agent without `corgi_request` whose cwd
  and branch equal those of an unfinished ledger entry of its project, and
  no other live agent's do. It gets the entry's id as `corgi_request`.

Everything else waits in the dialog.

### What changes outside the dialog

- **Projects from Git.** An agent in any Git checkout belongs to the
  repository `git rev-parse --git-common-dir` names, whether Herdr opened
  it as a linked worktree or not. A plain workspace on a
  `git worktree add` checkout, or an agent whose cwd left its workspace's
  checkout, is then grouped under its repository, in `corgi fleet` and the
  digest, and is merged and closed as a worktree (causes 7 and 8). AGENTS.md's
  line that such workspaces are not linked worktrees to Corgi changes
  with it. There is no `corgi_project` token.
- **Codex without a session.** The process-verified admission that today
  holds only for scratch Codex extends to every workspace Corgi created:
  a Codex pane there, with `codex` in the foreground at the pane's cwd on
  this refresh, is a row without transcript facts (cause 1).

## Decisions

- **Where and when**: on demand only, a corgi-gui header button and
  the dashboard's `o`. Rejected for now: a header count of unlisted
  sessions (Herdr calls on every refresh), and a dialog that opens by
  itself after a restart (a popup nobody asked for).
- **Tagging without asking**: only the two proven cases above,
  applied by the hooks. Rejected: nothing without asking (the user would
  have to open the dialog after every restart that loses a supervisor),
  and likely matches too (a wrong role silently changes who wakes whom).
- **An agent's project**: the repository Git names, linked worktree
  or not; plain linked checkouts are worktrees for merge and close.
  Rejected: a `corgi_project` pane token set in the dialog (one more token
  to keep right), and no move at all.
- **Close in a workspace Corgi did not create**: the pane only.
  Rejected: the pane's tab, and today's whole-workspace close with a
  warning.
- **Agents without an identity**: the dialog names
  `herdr integration install codex`, and process-verified Codex agents
  are rows in every workspace Corgi created. Rejected: the hint alone
  (they would stay hidden), and admission in any workspace (whatever runs
  in the user's own shells).
