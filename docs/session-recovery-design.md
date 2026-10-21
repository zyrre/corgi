# Save and restore agent sessions

Proposal for review, 21 October 2026. Design only; no feature, installation,
configuration change, or runtime operation is authorized by this document.
Investigated Corgi `3d352cc` (also the primary checkout's HEAD) and installed
Herdr 0.9.1, protocol 22.

## Recommendation

Add **Sessions…** to Corgi's dashboard, containing **Save sessions…** and
**Restore sessions…**. Save creates an immutable, dated recovery manifest for
the current Herdr session, including supervisors, workers, and scratch agents.
Restore opens a selection and validation screen, then one explicit confirmation
before starting any missing conversations. Restore never runs automatically at
Corgi startup. Browsing, selecting, and validating are read-only.

Resume native conversations by exact ID, in their existing directories and
worktrees, with their recoverable launch settings and supervisor role. Do not
send a greeting, task, "continue", handover request, or reconstructed prompt.
Keep recovered supervisor coordination on hold until explicitly continued.
Reuse Herdr's restored layout and matching agents; reconstruct only missing
placements after showing the plan. Never replay an entire layout over live panes.

Ship an initial native-resume implementation for **Codex**, covering all three
roles, with strict identity discovery and an honest incomplete-session list.
Capture other harnesses in the same manifest. Claude has a plausible native
adapter, but enable its resume action only after the isolated compatibility
checks below establish idle resume and supervisor-role preservation for the
supported CLI versions. Other harnesses remain visibly unsupported. This makes
the initial scope useful on this machine without claiming that every detected
agent is already recoverable.

There is an important boundary: Herdr itself can automatically resume agents.
For a workflow in which **every launch waits for confirmation**, recommend a
separately approved setup step disabling Herdr's native automatic resume while
retaining its layout restoration. Corgi must never silently change that setting.
It must also work alongside Herdr auto-resume when enabled, counting matches as
already running and refusing uncertain competing launches. The product choice
at the end concerns this boundary, not permission to implement anything now.

## What is established

Repository references below use `file:line` and link to the file; line numbers
refer to the investigated revision. Installed schema observations are identified
separately from repository support. No lifecycle tests were performed.

| Evidence | Consequence for the feature |
| --- | --- |
| [src/app/launch.rs](../src/app/launch.rs):59, :333, :387 | `LaunchPlan` represents a **new** launch: checkout creation, CLI start, then first prompt. Recovery needs a separate plan and executor. |
| [src/app/launch.rs](../src/app/launch.rs):476, :534 | Supervisor preparation persists launch options and generates a first/takeover prompt; supervisor marking currently happens in the prompt path. Neither path can be reused unchanged for an idle resume. |
| [src/supervisor.rs](../src/supervisor.rs):34, :60, :234, :279 | Supervisor state is durable, outside Git. `launch.json` records harness, requested model/effort and arbitrary extra args **per project**, not per native session. `prepare` can migrate directories and rewrite `ROLE.md`. |
| [src/harness.rs](../src/harness.rs):207, :264 | Model/effort argument builders exist. Codex supervisor arguments include developer instructions, sandbox/approval policy, network access and a writable state root; Claude uses an appended role file and extra directory. |
| [src/model.rs](../src/model.rs):34, :47, :128 | Herdr provides native session reference source/kind/value, pane/tab/workspace IDs, cwd/foreground cwd and worktree provenance. Agent names are presentation. |
| [src/herdr.rs](../src/herdr.rs):103, :370, :389, :469 | Corgi wraps snapshots, unfocused workspace/tab creation and `agent.start` with argv. These are available primitives, not a transactional restore API. |
| [src/herdr.rs](../src/herdr.rs):673, :711, :730 | Corgi currently deserializes only part of Herdr's snapshot; split layout, launch readiness and some ordering fields need additional typed reads. |
| [ARCHITECTURE.md](../ARCHITECTURE.md):390; [src/app/markers.rs](../src/app/markers.rs):42, :322 | Existing marker reconciliation is bound to native sessions and checkout identity. It recovers Corgi metadata; it does not launch lost conversations. |
| [src/session/locate.rs](../src/session/locate.rs):34, :83; [src/session/mod.rs](../src/session/mod.rs):93 | Current display discovery can fall back to newest transcript/rollout by cwd. Its facts include private activity text. Neither fallback nor the whole facts object is suitable for recovery identity/storage. |
| [src/session/codex.rs](../src/session/codex.rs):103; [src/session/claude.rs](../src/session/claude.rs):95, :137 | Session metadata can supply model/effort, but displayed Claude model names are transformed. Recovery needs raw native model IDs and provenance, not dashboard labels. |
| [src/app/refresh.rs](../src/app/refresh.rs):153, :180; [src/app/waker.rs](../src/app/waker.rs):519 | Refresh can deliver inbox prompts and trigger handovers. Merely omitting the first prompt does not ensure a recovered supervisor stays idle. |
| [src/supervisor.rs](../src/supervisor.rs):490, :541 | Merge tags depend on Herdr state-change counters; supervisor ownership requires live native identity, not a stale `session` metadata token. |
| [src/paths.rs](../src/paths.rs):17, :85; [src/app/waker.rs](../src/app/waker.rs):250 | Marker and wake records are on durable storage, but keyed by socket path. A reboot recovery manifest needs a stable scope independent of an old socket filename. |

Read-only local checks established:

- `herdr --version`, `herdr api schema --json`, and help commands: installed
  0.9.1/protocol 22 supports `session.snapshot`, `pane.process_info`,
  `layout.export`, `layout.apply`, `worktree.open`, and `agent.start` with args.
  There is no exposed atomic, idempotent recovery operation or start request ID
  in this schema. JSON transport `id` is a response correlation ID.
- The schema's layout export tree contains split direction/ratio and pane
  cwd/label/ID, **also command and environment**. Capture only the structural
  allowlist. `worktree.open` accepts an existing absolute checkout `path`;
  Corgi does not yet wrap that method. Availability was checked; these mutations
  were not exercised.
- One live snapshot was reduced in memory to field names and aggregate identity
  coverage: nine detected Codex agents, seven with native IDs and two without;
  six `corgi_handler`-marked agents all had IDs. This is a point-in-time detected
  list, not proof of exhaustive discovery or resumability. No names, prompts,
  terminal text, transcripts, or raw snapshot were printed or saved. In
  particular, unidentified scratch sessions must remain visible in coverage.
- `herdr --default-config` exposes `session.resume_agents_on_restore = true`.
  A targeted read of this key found no explicit override in the selected config;
  that establishes the default-based configuration, not a live reload audit.
- Direct installed CLI help: Codex 0.163.0 accepts
  `codex resume [SESSION_ID] [PROMPT]`, with prompt optional, plus model, cwd,
  config, sandbox and writable-directory options. Claude Code 2.1.150 accepts
  `--resume`, model/effort, appended system-prompt-file and extra directories.
  The usual mise wrappers failed under read-only restrictions; the actual
  installed binaries answered help/version without launching conversations.

Official documentation confirms that Herdr restores layout and directories,
but not original processes. Native agent resume is enabled by default and can
start eligible panes after client attachment without focusing each pane.
Current integrations are required. Those facts explain why Corgi must reconcile
rather than duplicate restoration. [Herdr session state](https://herdr.dev/docs/session-state/)

The installed schema is the version authority. The current website describes
`resume_argv`; installed protocol 22 does **not** expose that parameter in
`PaneReportAgentSessionParams` or `PaneReportAgentParams`. Do not base v1 on
injecting supervisor resume commands into Herdr. The API documents
`worktree.open` as opening an existing checkout and warns that successful input
queue admission does not prove execution. [Herdr socket API](https://herdr.dev/docs/socket-api/)

Codex documents explicit-ID interactive resume and cwd/model/sandbox overrides.
Use `--cd` to specify the validated saved directory. Do not use `--last`, a
session name, or fork. The optional-prompt help supports the proposed idle
command shape; actual idle behavior, including hooks and pending work, still
needs an isolated test. [Official OpenAI documentation](https://learn.chatgpt.com/docs/developer-commands?surface=cli#codex-resume)

Claude documents ID resume and appending a role file, but current documentation
includes behavior newer than installed 2.1.150, including changes to system
prompt reuse. It does not establish that all saved supervisor settings are
reapplied on this installed version. Do not enable the adapter on help syntax
alone. [Claude CLI reference](https://code.claude.com/docs/en/cli-reference)

## User flow

Use an explicit dashboard footer entry **S sessions** (uppercase; lowercase `s`
already starts a supervisor and `r` refreshes:
[src/app/mod.rs](../src/app/mod.rs):557). The Sessions menu offers Save, Restore,
and saved-snapshot management. This phase proposes no Omarchy widget changes,
global hotkey, startup restore popup, reboot hook or unattended restore command.
At startup a quiet footer may say "Saved sessions available · S sessions".

### Save before reboot

Save opens a fresh inventory for the injected Herdr session, grouped by project
plus Scratch. Default scope is all detected agents, regardless of Corgi ownership;
unknown/unsupported rows are included rather than silently omitted. Users may
exclude rows. Show supervisors/workers/scratch counts, identity and setup
completeness, and a non-agent-pane coverage note. Corgi's own dashboard, ordinary
shells and arbitrary terminal processes are not agent sessions to resume.
The summary must state that other named/remote Herdr sessions are outside this
save's scope; it must not call this a machine-wide backup.

```text
+ Save sessions -------------------------------------+
| Current Herdr session: default                      |
| 9 agents: 6 supervisors, 1 worker, 2 scratch         |
| Conversations: 7 identified / 2 missing IDs          |
| Setup: 5 verified / 4 need review                   |
| Busy: 2 working, 1 waiting for input                |
|                                                    |
| Saves references and setup. Agents keep running.    |
| Does not finish work or make a reboot safe.         |
|                                                    |
| > Cancel    Wait for idle    Save now (incomplete)  |
+----------------------------------------------------+
```

Counts in wireframes are fictional examples, not a session listing. Use semantic
theme roles: success for verified, warning for incomplete, error for blocked;
always pair color with text. Narrow terminals scroll the body and keep action
buttons visible.

**Save now** makes a bounded observation, not a stop-the-world checkpoint. Busy,
blocked, unknown, or changing sessions can be saved as references to the latest
native durable history, with explicit warnings. No Ctrl-C, exit, checkpoint
prompt, supervisor handover or automatic stop. Save remains useful even if one
session has no ID; it must say "Saved incomplete", never "All sessions safe".

**Wait for idle** observes only; blocked and unknown states do not count as idle.
Show outstanding rows and elapsed time. It is cancellable and never times out
into an automatic Save now. Once eligible rows are idle, recheck identities and
return an updated summary; the user presses Save. An idle indicator still cannot
prove that the harness has flushed every byte or that no other process writes
the repository. Do not promise a checkpoint acknowledgment the harness lacks.

Before publishing, reread live identities and allowlisted metadata. Mark entries
that changed/exited during capture; do not substitute a new occupant. Record
capture start/end and per-entry observation times. New agents appearing after
the inventory produce a "not included; refresh to include" notice.

```text
+ Saving sessions -----------------------------------+
| [ok] Captured 9 agent records                       |
| [ok] Checked 7 native conversation references       |
| [..] Writing durable recovery manifest              |
| No agents stopped or prompted.                     |
+----------------------------------------------------+

+ Saved incomplete ----------------------------------+
| 21 Oct 2026 21:40 CEST · snapshot …8f2a              |
| 7 conversations identified; 2 scratch IDs missing   |
| 2 were working; their last turn may be interrupted. |
| Full details: 5 setups verified, 4 need review       |
|                                                    |
| > Close                    View saved details      |
+----------------------------------------------------+
```

Save never overwrites another snapshot: create a new timestamp + random ID.
Show "Previous save retained" after a subsequent save, avoiding an overwrite
dialog. Keep snapshots until explicitly deleted in management, with a single
delete confirmation; no silent pruning of an incomplete/unresolved recovery.
The browser may collapse older saves. No export/import or cloud sync in v1.

### Restore after reboot

Opening Sessions → Restore lists snapshots newest first with exact timestamp,
local timezone, age, origin Herdr scope and status: not restored, partial,
restored previously, or incompatible. It does not select a snapshot for execution.
Enter opens details only. Reading a snapshot never starts, stops, focuses or
relabels an agent, repairs directories, installs a CLI or rewrites configuration.

In details, compute a fresh preflight plan. **Nothing is selected initially**,
even in the newest snapshot. Space selects an eligible row or an explicitly
focused project's eligible rows. Provide a labeled "Select eligible in all
projects" control requiring deliberate activation, not an incidental keystroke.
Already running, unsupported, missing and uncertain rows cannot be launch
targets. Unknown settings require explicit acceptance of the displayed setup
difference before the row becomes eligible. Scratch is its own selectable group.

```text
+ Restore sessions — review -------------------------+
| Saved 21 Oct 21:40 CEST · 12 hours ago · …8f2a       |
| Project / session      Status                      |
| [ ] weather (3)        2 ready, 1 already running   |
| [ ] corgi (4)          3 ready, 1 missing worktree  |
| [-] Scratch (2)        2 missing native IDs         |
|                                                    |
| Selected: 0 sessions · 0 projects                  |
| Space select · Enter details · Esc cancel           |
| > Cancel                          Review restore   |
+----------------------------------------------------+
```

After selection, Review restore opens the only launch confirmation. It must
name the snapshot, timestamp/age, selected project names and per-project counts,
role totals, new-launch count, reused sessions, unselected/skipped categories,
placement changes and any accepted setup differences. For many projects, scroll
the project list; keep totals and Cancel visible. Do not hide errors in a generic
"some skipped" number. Explain whether Herdr auto-resume is known enabled,
disabled, or unknown. Disable confirmation while its launch state is uncertain.

```text
+ Confirm restore -----------------------------------+
| Snapshot …8f2a · 21 Oct 21:40 CEST · 12 hours old    |
| Start 5 conversations: weather 2, corgi 3            |
| Roles: 2 supervisors, 3 workers, 0 scratch          |
| Already running: 1 (reuse; no restart)              |
| Skipped: 1 missing worktree, 2 missing IDs          |
| Unsupported: 0 · Unselected eligible: 0             |
| Placement: 4 original panes, 1 new tab (occupied)   |
| Setup differences: none                            |
|                                                    |
| No prompt sent. Supervisor coordination stays held. |
| > Cancel                       Restore 5 sessions  |
+----------------------------------------------------+
```

Cancel is focused on entry; Esc always cancels before execution. Require a fresh
navigation and activation of **Restore N sessions**; ignore key repeats and the
Enter that opened the dialog. No `y` mass-restore shortcut. No action when N=0.
Changing selection invalidates this confirmation. Just before execution recheck
the plan: changed occupancy, launch eligibility or settings returns to review;
do not expand the selection or silently replace the approved destinations.

Mark saves older than seven days **Old snapshot**; mark future timestamps
**Clock differs**. For an old or previously restored snapshot require one
unchecked "I reviewed this older/prior restore" box **inside the same final
dialog**, alongside the timestamp, previous restore time and today's counts.
This adds friction only for exceptional restores. Never automatically choose
an older save when a newer save is invalid. A successful prior restore remains
inspectable; opening it again revalidates live state, not just its success badge.

```text
+ Restoring — snapshot …8f2a -------------------------+
| [ok] weather supervisor: same conversation, held    |
| [ok] weather worker: native ID verified             |
| [..] corgi supervisor: waiting for native identity  |
| 2 / 5 verified · no prompts sent                    |
| > Stop remaining                                   |
+----------------------------------------------------+

+ Restore partial -----------------------------------+
| 4 resumed · 1 already running · 1 failed            |
| 3 skipped: worktree missing (1), native ID (2)      |
| Failed: corgi worker — launch outcome unknown       |
| Will inspect again; will not launch it again.       |
| Supervisors held: 2 · pending inbox items retained  |
| > Close    Review unresolved    Continue coordination|
+----------------------------------------------------+
```

Totals distinguish attempted launches from the whole manifest. Progress/result
stays visible until dismissed; no success animation dismisses failures. Stop
remaining stops scheduling, never kills agents already started; an in-flight
start remains pending until reconciled. Closing Corgi leaves durable progress.
Retry means "review unresolved" → fresh validation → final confirmation for
only safely retryable rows, with zero initial selections. It never repeats an
uncertain launch or sends a prompt to prove recovery.

## Contract: what "as it was" means

The promise is the **same saved native conversations and recoverable setup** on
the same machine/account, not fresh agents loosely briefed from memory. A partial
result must identify every gap. Native history can advance after Save: restoring
an ID opens that conversation's latest durable state, not a historical rollback
to the snapshot timestamp. Tell the user this in saved details.

| Component | Recovery contract |
| --- | --- |
| Conversation | Exact native ID and native local store; no fresh conversation, fork, latest-by-directory fallback or transcript replay. Compaction/history retention remains the harness's responsibility. |
| Idle agent | Open interactive resume with **no prompt**. Require an adapter test that no new user turn or model/tool work starts. CLI startup hooks, plugins and MCP initialization may run; arbitrary user hooks cannot be frozen by Corgi. Hooks capable of submitting work make idle behavior unverified and require review/withholding eligibility. |
| Busy/blocked agent | Latest durable conversation survives if the harness saved it; an unfinished turn, approval dialog, stream, tool subprocess or queued input is not a resumable process checkpoint. Label "saved while working/waiting; inspect last turn". Never replay work automatically. Withhold automated resume if that harness/version can continue pending work on opening and no verified idle control exists. |
| Model and effort | Preserve explicit settings and the last confirmed native raw values when available. Record their provenance; unknown is not a default that Corgi invented. Changed/unavailable models require an explicit per-row choice or skip. |
| Supervisor | Same conversation, role instructions and state-directory access, plus Corgi ownership. Durable memory alone is not a substitute for conversation recovery. |
| Files and worktree | Reopen the existing saved cwd/checkout. Preserve working files, index and uncommitted changes by leaving them alone. Record saved branch/HEAD for comparison, never reset, clean, stash, switch, recreate or fetch automatically. |
| Layout | Prefer matching restored Herdr pane/tab/workspace and grouping. Recover missing Corgi agent placements using structural layout information where verified; unavailable split/zoom/order details are disclosed. A relocated session requires approval in the plan. Host window placement is outside this contract. |
| Volatile state | PTY state, terminal scrollback, unsent composer text, shells, servers, running commands, process environment mutations, credentials in RAM and temporary tool artifacts are not captured. Native prompt-cache warmth/cost cannot be preserved. |

Worktrees, uncommitted files, native Codex/Claude stores, Corgi supervisor
decisions/ledger/briefs/handovers/inbox, launch options, project lists and marker
records already live on disk independently of the snapshot. Recovery records
references to them; it is **not a backup** of those bytes. A lost transcript,
deleted worktree, changed role file or disk loss remains a real recovery failure.
Do not roll supervisor memory back to an older save.

No durable recovery dependency may live only in `/tmp`. Current Corgi state paths
are under the state home; socket endpoints are runtime addresses to rediscover,
not data to preserve. Mark saved cwd, harness store, role/settings file, Corgi
binary or writable root under a volatile directory as a reboot-recovery gap;
those dependencies are not verified merely because they exist before reboot.
Block exact-setup eligibility until repaired or the specific difference is
accepted. Move any new
recovery-owned launch/role asset to durable state storage at original launch.
Never copy arbitrary temporary files, private task files or tool output into a
snapshot to pretend to support their reconstruction. Existing user-owned
volatile dependencies need explicit repair, not silent migration.

## Coverage and capture rules

Use a metadata-only capture path rather than serializing `DashboardAgent` or
`SessionFacts`, both of which contain conversation-derived text. Persist only
the fields below. Do not read unrelated transcript bodies to improve discovery.

| Fields | Source and treatment |
| --- | --- |
| Harness and native reference | Live Herdr `agent_session` including source, agent, kind and exact value. Validate against the selected adapter; a path reference is not necessarily a UUID. Keep missing identity as null with a reason. |
| Scratch without identity | Inventory known Corgi scratch panes, including the foreground-process fallback in [src/herdr.rs](../src/herdr.rs):110. A foreground Codex process proves harness presence, not its conversation ID. An explicit resume ID in that exact process's allowlisted args can be supporting evidence; otherwise show "ID unavailable". Never bind by cwd/newest file, including when several scratch sessions share HOME. |
| Native metadata | Resolve **only the exact known ID** to its store record. Add a strict lookup separate from display lookup, which can currently fall back even after an ID lookup fails. Read bounded native metadata for raw model/effort/store compatibility; do not collect prompts, summaries or tool args. CLI/app-server metadata can be used only if it binds to that exact native ID and does not require a turn. |
| Requested versus effective launch | Today only supervisors have project-level `launch.json`; workers/scratch lack a durable session-bound launch profile. Last-turn model/effort are observations, not proof of current in-memory settings. Future launch capture should persist typed settings bound to native identity as soon as known, updating on confirmed changes. Older sessions get explicit provenance/unknown fields. |
| Other launch settings | Allowlisted profile name, permission/sandbox mode, extra writable roots and references/digests of settings/plugin files. Do not persist arbitrary argv, inline config strings, arbitrary environment, provider tokens or user/system prompts. Parse only known safe options; unknown extras become "custom setup not captured" and need review. Do not execute saved shell text. |
| Directory and Git | Native launch cwd and live foreground cwd separately; canonical repository/common Git directory and exact checkout path, linked-worktree flag, branch/HEAD as observations. Plain non-Git directories and Scratch need no repository. Conflicting cwd sources need review. |
| Placement | Herdr scope, workspace/tab/pane IDs, labels and order, worktree provenance, structural split tree/ratios and zoom/focus hints. `layout.export` can supply the tree; strip command/env, terminal titles, scrollback and unrelated non-agent pane contents. Retain only minimal non-agent leaf placeholders needed to describe placement, never replay their processes. |
| Corgi identity/ownership | Canonical project root, Corgi-owned versus external session, supervisor/worker/scratch role, validated request ID, workspace role and project-main association. Allowlist known Corgi token keys; no generic token dump. A name or workspace label cannot grant ownership. |
| Supervisor role/memory | Exact state-directory and role-file references, role digest/version, Corgi binary reference, memory-directory association and safe launch settings. Project `launch.json` may describe a different supervisor: treat as a candidate unless bound/confirmed against this session. Do not read/copy decisions, briefs, inbox contents or private custom role text into the manifest. |
| Merge/handover | Retain historical merge recommendation and baseline/handover provenance as session-bound observations. Herdr sequence counters are not portable across new processes. Restore a merge recommendation as "previously marked ready; recheck", not a newly valid MERGE flag. Never replay an outstanding handover instruction or completed request. |

For unknown exact settings, show a row-level choice such as "Resume conversation
with native saved/current defaults; model/effort unverified". The user can accept
that difference or leave the row out. Known explicit settings beat unbound
project defaults; a native latest-turn value and a saved launch option can differ
because the user changed settings mid-session. Surface the conflict rather than
silently pinning an older option. Do not claim all settings are verified merely
because the CLI accepts a resume command.

An ID missing at Save can be supplied later through the row's **Resolve ID**
detail using an exact native identifier the user obtains from that agent's own
status UI. Validate store and directory metadata; record this as a user-resolved
amendment in recovery state, preserving the original immutable manifest. A
different project's ID requires explicit correction, never automatic binding.
Installing/updating an integration is a separate user action, not a side effect
of opening Save or Restore. Unresolved rows prevent an "all recovered" result.

## Proposed storage and schema

Use `$XDG_STATE_HOME/corgi/recovery/`, falling back to
`~/.local/state/corgi/recovery/`, consistent with [src/paths.rs](../src/paths.rs):17.
Snapshots belong to a local installation/account and a logical Herdr scope
(machine plus named/default session), not a sanitized socket path. Allocate a
local random installation ID in private state; record UID and scope for checks.
Require an absolute, persistent state-home path; report a volatile or relative
override rather than quietly storing reboot recovery there.
Never use a label alone as identity or automatically restore onto another host,
account, remote server or Herdr session. If the API cannot prove scope, require
explicit scope mapping in review and do not use saved pane IDs as authoritative.

Proposed files:

```text
recovery/
  installation.json
  recovery.lock
  holds.json
  snapshots/<UTC timestamp>-<random ID>/manifest.json
  attempts/<snapshot ID>.json
  launch-profiles/<local session record ID>.json
```

An illustrative v1 manifest, with invented values and one session shown:

```json
{
  "schema_version": 1,
  "requires": ["native-resume", "coordination-hold"],
  "snapshot_id": "8f2a-example",
  "created_at": "2026-10-21T19:40:00Z",
  "capture_finished_at": "2026-10-21T19:40:02Z",
  "producer": {"corgi_version": "future", "herdr_version": "0.9.1", "protocol": 22},
  "origin": {
    "installation_id": "local-random-id",
    "uid": 1000,
    "herdr_scope": {"machine": "local", "session": "default"},
    "boot_token": "opaque-host-boot-identity",
    "socket_hint": "/run/user/1000/herdr-example.sock"
  },
  "coverage": {"detected": 1, "included": 1, "excluded": 0, "unresolved": 0},
  "projects": [{"id": "project-1", "root": "/home/example/projects/weather"}],
  "placements": [{
    "id": "placement-1",
    "workspace_id": "w7",
    "workspace_label": "weather supervisor",
    "workspace_number": 2,
    "tab_id": "t7",
    "tab_label": "Supervisor",
    "tab_number": 1,
    "pane_id": "w7:p1",
    "pane_label": "Supervisor",
    "structure": {"type": "pane", "session_record_id": "record-1"},
    "zoomed": false,
    "completeness": "verified"
  }],
  "sessions": [{
    "record_id": "record-1",
    "project_id": "project-1",
    "role": "supervisor",
    "corgi_owned": true,
    "display_name": "weather-supervisor",
    "harness": "codex",
    "native": {
      "kind": "id", "value": "00000000-0000-4000-8000-000000000001",
      "source": "herdr:codex", "agent": "codex",
      "confidence": "live-native-reference",
      "store_root": "/home/example/.codex",
      "cli_version": "0.163.0"
    },
    "directory": {
      "launch_cwd": "/home/example/projects/weather",
      "foreground_cwd": "/home/example/projects/weather",
      "checkout": "/home/example/projects/weather",
      "repo_common_dir": "/home/example/projects/weather/.git",
      "linked_worktree": false,
      "branch": "main", "head": "saved-commit-id"
    },
    "launch": {
      "profile_ref": "profile-record-1",
      "model": {"value": "gpt-6.1-sol", "source": "session-bound-profile"},
      "effort": {"value": "high", "source": "native-metadata"},
      "permission_mode": "on-request", "sandbox": "workspace-write",
      "extra_writable_roots": ["/home/example/.local/state/corgi/supervisors/weather"],
      "settings_refs": [], "unknown_options": false
    },
    "supervisor": {
      "state_dir": "/home/example/.local/state/corgi/supervisors/weather",
      "role_file": "/home/example/.local/state/corgi/supervisors/weather/ROLE.md",
      "role_digest": "sha256-of-existing-role-file",
      "role_revision": "known-corgi-role-revision",
      "corgi_bin": "/home/example/.local/bin/corgi"
    },
    "corgi": {"request_id": null, "workspace_role": "project-main", "merge_observation": null},
    "placement_ref": "placement-1",
    "observed": {"at": "2026-10-21T19:40:00Z", "state": "idle", "state_change_seq": 42},
    "completeness": {"identity": "verified", "setup": "verified", "warnings": []}
  }]
}
```

Schema notes: IDs and digests above are placeholders, not valid captured evidence.
`native`, project, Git, placement, supervisor and individual setting values are
nullable where unknown/inapplicable; every unknown has a reason/provenance. Counts
are derived and checked against records. A captured display name is presentation
only and may itself reveal a task; keep it private, bounded and optional. Do not
derive or persist a task title from prompt text. Internal record IDs keep missing
native IDs distinct, but cannot authorize native launches. Profiles contain only
typed allowlisted settings; raw historical `extra_args` are never copied wholesale.

The separate attempt journal records attempt ID, manifest digest, approved plan
digest, selected record IDs, target scope, per-row phase, destination mapping,
operation intent, response/error code and **observed native identity**, plus
previous result times. No raw error text that may echo argv/private data. Phases
are `planned`, `placement_pending`, `placement_ready`, `start_pending`,
`native_verified`, `metadata_pending`, `complete`, `failed_no_effect`, and
`outcome_unknown`. A success badge is evidence of a prior observation, not a
command to launch again. Journals and holds are mutable; manifests are immutable.

Snapshot and journal directories are owner-only (0700); files, assets and locks
0600 from creation. Check ownership and reject unexpected symlink/hardlink
content files; follow the existing private-state boundary pattern rather than
trusting umask ([src/inbox_privacy.rs](../src/inbox_privacy.rs):14, :34, :62).
Bound file sizes, entry counts, strings, nesting and path/reference formats;
render labels/errors as plain text with control characters removed. Recovery
data is private metadata even without transcripts.

Publish with an exclusive lock and same-directory exclusive temporary file:
write, flush, `sync_all`, rename, then fsync the containing directory. Sync new
parent-directory entries as needed before reporting **Saved**. No `/tmp` staging.
Use the same durable replacement for journal intents, results and hold changes.
The existing marker writer's atomic rename alone is not a durability guarantee
([src/app/markers.rs](../src/app/markers.rs):290). ENOSPC, fsync or permission
failure must not report success or destroy the previous save. A crash leaves
either a valid complete manifest or a disregarded temporary file. Do not
acknowledge Save until its recovery hold intent is also durable; a small
committed manifest reference can activate both without pretending two renames
are one transaction.

Reject unknown schema major versions and unknown required capabilities before
any mutations; offer inspect-only details and an update suggestion. Optional
additive fields can be ignored. Invalid/duplicate native identities, unsafe paths
or corrupt files block execution; do not silently reduce a malformed manifest to
an empty/partially trusted one. Future migration writes a new version while
retaining the original; no automatic destructive rewrite.

## Restore execution and identity

1. **Scope and validate.** Resolve the current injected Herdr socket, never
   reconnect using the saved socket hint. Validate local scope, file format,
   required CLI/version capabilities, exact conversation record, cwd, checkout,
   settings/role dependencies and ownership. Missing CLI/authentication is a
   blocked row with a repair instruction; no automatic install or login. Do not
   query a provider by generating a test turn. Missing native store or transcript
   never falls back to a new agent. An unavailable repository/worktree is skipped
   until the user repairs or explicitly maps it in details.
2. **Index today's occupants.** Index live agents by validated
   `(installation/store namespace, harness, native kind, native value)`. Native
   UUID alone is insufficient across accounts/store roots. Examine relevant
   restored panes as well as `agents`: a saved native reference on an otherwise
   empty pane may describe a pending Herdr restore, not a running agent.
   `launch_pending`, `interactive_ready` and process information are available
   in the installed schema but require Corgi client support. Stored references,
   stale pane tokens, names and cwd are not proof of a live process.
3. **Reconcile before selection.** A live exact match is **already running**,
   wherever it moved; do not refocus, restart or reconfigure it. Report any setup
   difference; "same conversation running" does not verify its role flags. Two
   live matches are a duplicate conflict, not permission to create a third.
   An unidentified relevant agent/pending launch blocks a competing restore until
   resolved; timeouts never prove absence. If the same project has a different
   live supervisor, block the old supervisor rather than replacing the new one
   or attaching a second supervisor to the same memory directory.
4. **Plan placement conservatively.** Reuse the exact restored pane only with
   matching scope/checkout and confirmed available shell, no pending launch and
   no input draft. A different foreground process, an error pane or unknown
   shell/input state is occupied, regardless of labels. Never send exit/Ctrl-C
   or a shell command into it. Offer a new tab in the appropriate existing
   workspace, explicitly shown in the plan, or skip. If safe shell emptiness
   cannot be established, a new pane is the fallback, not an assumption.
5. **Recover missing placement, not checkouts.** For an existing linked checkout
   with a lost workspace, use `worktree.open` with its validated absolute path,
   not `worktree.create`. Validate again after the response. Open a plain
   workspace for a plain directory, including Scratch; scratch remains outside
   supervisor grouping. Use saved grouping/order and filtered split structure
   only in newly created recovery-owned tabs. Never call `layout.apply` on a
   live tab or replay a saved command/env. Non-agent leaves remain out of scope;
   disclose a reduced layout. If original geometry cannot be safely reproduced,
   show the approved tab/pane fallback. Keep Corgi focused; create with
   `focus:false`. Do not create OS windows or change host workspaces 1–4.
6. **Reserve and persist before starting.** Serialize Corgi recovery operations
   with the recovery lock, scoped to this installation, including across
   snapshots. Hold session-key reservations to prevent two dashboards from
   restoring the same conversation. Record durable intent/destination before
   each mutation. Immediately revalidate against Herdr, then issue at most one
   start per intent. Saved request IDs are retained for worker provenance, not
   reused as if they were Herdr start idempotency keys.
7. **Resume only.** Build typed argv using a separate `ResumePlan` with no prompt
   field and no checkout-creation flag. Codex candidate shape is
   `codex resume <exact-uuid> --cd <validated-cwd>` plus reviewed model/effort,
   permissions and supervisor options. Claude candidate is
   `claude --resume <exact-id>` in the validated cwd, only once its adapter is
   verified. Never use `exec resume`, print mode, `--last`, `--continue`, fork,
   `--worktree`, or a positional task. Use Herdr's structured args, not stored
   shell command strings. An `agent.start` response by itself is not completion.
8. **Verify and rebind.** Observe the exact native ID and interactive readiness
   in the intended pane, then validate/rebind role and request/workspace marks
   through the existing marker writer. Do not grant supervisor status from a
   launch name. If native identity is delayed, show pending/unverified; no test
   prompt. Rebind workspace/root-tab IDs to new mappings; never replay old IDs
   into a different scope. Preserve unrelated tokens. Treat old sequence-bound
   merge/handover observations as historical, rechecking instead of copying
   stale validity. If mark writes fail after resume, retry metadata only.
9. **Record outcomes.** Complete means the native conversation is observed and
   recoverable setup/metadata either verified or explicitly accepted as different.
   Distinguish conversation-ready/setup-unverified from full success. Retain
   partial journal and any created empty panes; no automatic rollback that might
   kill a successful agent or delete a checkout. A retry reuses exact matches
   and journaled placements, not repeated workspace creation.

Git validation compares canonical cwd and Git common-directory/worktree
membership. A changed branch/HEAD is a visible difference, not evidence of a
missing directory. The user may accept using the existing changed checkout in
the same review or skip it. Path remapping requires explicit confirmation of
repository identity. Never infer identity from a basename or silently clone a
missing repository. Same-basename projects currently share a supervisor-state
path; detect conflicting project associations and block supervisor recovery
rather than merging their memory ([src/supervisor.rs](../src/supervisor.rs):60).

### Failure and concurrency limits

After any lost response, crash or disconnect, reload durable intent and inspect
Herdr. If a matching agent is observed, adopt it. If only a placement succeeded,
continue from that placement after review. If the API explicitly rejected a
request without side effects, the row can become retryable after repair. If
queueing/start might have happened but no live identity proves the outcome, keep
**outcome unknown** and block a second start. A closed shell/process alone does
not prove a queued command was never accepted. Require a supported definitive
negative acknowledgment or explicit user investigation of that pane before a
new reviewed attempt. Do not instruct the user to blindly retry.

Corgi can serialize its own launches but cannot atomically reserve a conversation
against Herdr's independent auto-resumer or a human starting the same CLI. The
installed API exposes neither a cross-owner conversation reservation nor a
documented "startup restore finished" barrier. Polling twice or waiting a fixed
delay does not close that race. In auto-resume mode, reuse verified matches;
withhold new starts into ambiguous restored panes. A confirmed-only setup with
Herdr native auto-resume disabled is the recommended way to permit predictable
manual launches. An upstream reservation/barrier would improve coexistence
later; it is not assumed to exist in v1.

No recovery path calls `agent.prompt`; therefore an uncertain operation must
never be resolved by resending a task. Native pending deliveries and Corgi inbox
items remain where they are. On later continuation of coordination, an old
delivery whose outcome was uncertain stays quarantined for review, not replayed
because its process disappeared. Retry safe reads and metadata writes; do not
turn all transport errors into launch retries as the new-launch shell readiness
loop does.

### Supervisor preservation and idle coordination

Do not call `supervisor::prepare` during resume: it can migrate state paths and
rewrite the role file. Use the saved/validated state directory and role asset
in place, preserving access at paths already embedded in the conversation.
Verify the Corgi executable referenced by that role still exists. Reapply the
reviewed Codex developer instructions plus state root and permission/sandbox
settings, or Claude's appended role file and state access, **only through a
version-tested resume adapter**. Do not assume a CLI restored custom instructions
from history, and do not assume reapplying flags cannot duplicate/change them.
Verify both in the isolated fixture before declaring setup complete.

If a role/settings file changed, a referenced binary disappeared, or the
project-level launch profile cannot be tied to this conversation, show the
specific gap. The user may approve an updated role/setup or skip; never silently
regenerate instructions, overwrite live memory or expand permissions. Future
Corgi launches should retain immutable Corgi-generated role assets/profile
versions in durable state, with no private prompt content. Legacy custom role
files remain references/digests, not copied prompt archives.

**Start a fresh supervisor using durable memory** is a separate new-agent action
when the native conversation is unavailable. It gets a new native ID and the
normal first prompt, with explicit labeling that conversation context is lost.
It must never be a fallback hidden inside Restore or count as successful resume.

Recovery must add a small durable coordination hold, checked by **every** Corgi
waker before delivery, handover or replacement. Save records a next-boot hold
intent for its saved supervisor identities; it does not pause current agents or
current coordination. On a later host boot, Corgi activates that hold before its
first refresh can wake those supervisors, including matches Herdr resumed before
the user opened Restore. Identify boot changes using a supported OS boot token
(Linux boot ID/macOS boot time), not socket name or elapsed wall-clock time. If
boot identity is unavailable, expose that limitation and withhold the idle
supervisor guarantee until a supported conservative hold can be established.
Only the newest committed Save arms this next-boot behavior; browsing archived
saves never activates a hold. Explicit restore on the same boot writes a hold
before starting its supervisors. Locking must make activation visible to another
dashboard already running the waker.

The hold suppresses Corgi coordination, not the native process or user input.
Preserve inbox/report/ledger bytes and request identities; do not rebase an old
state-change counter into a new completion event. Result UI offers **Continue
coordination** as a distinct deliberate action, listing projects and pending
item counts, with Cancel focused. Closing the result leaves the hold and a
visible dashboard badge. Reopening Corgi does not release it. A user typing a
message manually remains free to do so. Existing unsupported/older Corgi
binaries will not honor a new hold; mixed-version dashboards must be detected
or explicitly excluded from the guarantee before enabling this workflow.

This is a required, bounded integration with the waker, not a general job
pause/resume feature. If the adapter or waker cannot meet idle resume, report
that as unsupported rather than quietly allowing work on Restore.

## Initial implementation and acceptance plan

Approval should authorize the following bounded scope as a later phase, not this
document's merge or an immediate implementation:

1. **Strict inventory and private storage.** Native-ID-only metadata lookup;
   manifest/journal validation, durable writes, immutable snapshots and typed
   session-bound launch profiles. Inventory external workers and unidentified
   scratch panes, retaining unsupported/incomplete records. Capture/save tests
   use synthetic native metadata, not the user's conversations.
2. **Codex resume and Herdr reconciliation.** A separate prompt-free executor;
   existing-checkout opening, conservative occupancy, journal/reservations,
   metadata rebinding and durable supervisor hold. Do not refactor the general
   launcher, inbox pipeline or Git lifecycle beyond the necessary boundaries.
3. **TUI.** Sessions menu, Save summary/wait/progress, snapshot browser,
   selection, final confirmation and partial results/retry. Reuse existing
   overlay/job/theme patterns. Add small synthetic UI fixtures; refresh README
   screenshots with `cargo readme-shots` if this visible feature is implemented.
4. **Compatibility gate and focused review.** Verify Codex idle conversation
   resume and supervisor setup with temporary synthetic conversations in a
   supported isolated environment, including a busy/blocked saved fixture.
   No prompts to existing agents. Reboot is unnecessary for these tests: model
   boot transitions and fake Herdr responses, then do one controlled isolated
   restart exercise if needed. Any Herdr preview must follow AGENTS.md labels;
   no host windows on workspaces 1–4. Verify Claude independently before enabling
   its adapter; absence of that evidence leaves Claude captured/unsupported.

Concrete acceptance checks:

- A synthetic supervisor, worker and scratch inventory round-trips native IDs,
  raw model/effort provenance, existing directories, ownership/request fields
  and supervisor role/state references. Multiple HOME scratch panes with absent
  IDs remain distinct unresolved records; strict lookup never selects a newest
  unrelated rollout when the known ID is missing. No private user/task prompt,
  transcript content, environment dump or credential bytes appear in saved
  files, logs, command arguments or test snapshots. Corgi's generated supervisor
  role is a separate instruction asset; Codex's existing argument builder passes
  that role inline, so custom private role content needs a supported safe adapter
  or remains a setup gap rather than being copied into argv.
- Startup, menu opening, Enter-to-browse, selection and Cancel produce no starts
  or prompts. Zero selected by default; repeated Enter cannot cross confirmation.
  Final confirmation reports exact snapshot timestamp/age, projects/counts and
  all skipped categories. Old and previously restored saves get the inline
  acknowledgment. Revalidation rejects a changed plan.
- Matching Herdr agents are reused across repeated restore, a partial retry,
  renamed/moved panes and another snapshot containing the same ID. A replaced
  occupant, duplicate native identity, pending Herdr launch or missing native ID
  cannot authorize a start. Changed checkout/occupied-pane relocation requires
  visible approval; repositories and uncommitted files are untouched.
- Inject failure before/after manifest publish and before/after one placement,
  start and mark response. Restarting the executor never repeats an uncertain
  start, never duplicates a placement it can identify and never loses the last
  good save. Simultaneous Corgi restores serialize. Verify owner modes and
  durable-write error reporting; corrupt/newer schema executes nothing.
- The isolated native fixture opens the same conversation without a new turn,
  preserves reviewed model/effort and supervisor instructions/state access, and
  retains its idle state across the next Corgi refresh and dashboard reopen.
  Pending inbox items/handover events do not wake it while held; explicit
  Continue coordination is required. Unknown prior deliveries are not replayed.
  Missing CLI/conversation/worktree/role and a rejected model yield named partial
  results without fresh-session fallback. A native version that resumes work
  automatically fails eligibility for the idle contract.

Use fake socket/native metadata fixtures for most checks; do not introduce a
bespoke proof framework or a regression matrix for unchanged features. For later
Rust changes run the repository-required fmt, clippy, test and release build
checks, then a specifically labeled preview as appropriate. **For this docs-only
proposal**, verify references/links, `git diff --check`, and commit the document;
no Cargo suite, runtime preview, plugin relink or configuration change is needed.

Recommended work split: one implementer handles storage/inventory and resume
contracts; a second bounded task builds the TUI against those types after they
settle; a focused independent review examines confirmation, ambiguous starts,
supervisor role restoration and waker holds. Use **Codex GPT-6.1-Sol at high
effort**, as requested, for implementation and review. A narrow native/Herdr
compatibility check comes before UI polish; model escalation is unnecessary
unless that check reveals a deeper incompatibility. This is a suggested future
split, not delegation or implementation during the design phase.

Alternatives considered:

- **Herdr alone:** already valuable for layout/native IDs, but does not provide
  this dated Corgi save/review workflow or establish full supervisor launch-role
  recovery. Keep it as the layout owner rather than build a competing restore.
- **Only restart fresh supervisors from durable memory:** loses the conversation
  and excludes workers/scratch. Keep as an explicit fallback action outside
  Restore, never as the feature's claimed success.
- **Resume every harness in v1:** launch support is not evidence of idle native
  resume or exact settings. Add adapters individually after the same small
  compatibility check; preserve unsupported inventory in the meantime.
- **Backup all transcripts/environment or replay commands:** increases private
  data exposure and still cannot recover volatile execution. Native references,
  safe launch settings and clear missing-dependency reporting fit this feature.

## Review decision

Recommend approving the Codex-first, explicit Save/Restore product flow, with
immutable private manifests, incomplete discovery shown, Herdr-owned layout,
native-ID reconciliation, a prompt-free resume executor and supervisor holds.
Do not ship the claim "every agent restored exactly" until each selected row's
conversation and setup is either verified or its specific difference accepted.

One product choice remains: **must confirmation govern Herdr's existing startup
launches too?** Options are (a) retain Herdr auto-resume and confirm only missing
sessions launched by Corgi, or (b) recommend/offer a separately approved manual
recovery setup that disables Herdr native auto-resume while preserving layout.
Recommend **(b)** for the requested confirm-before-restoring experience; retain
safe coexistence with (a), reporting already running and pending/unknown rows.
Changing that installed setting is not authorized now. An unresolved compatibility
gate is an implementation prerequisite, not another product preference to guess.
