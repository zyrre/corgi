# Save and restore agent sessions

Approved implementation, 21 October 2026. This replaces the initial Codex-first
proposal: **Codex and Claude Code are both supported**. Q13 A selected confirmed
recovery with Herdr layout restoration retained. Stable activation and installed
configuration changes remain subject to review; development changes neither.

## The user flow

Press **S** in the dashboard for Sessions. Nothing restores on startup.

```text
┌ ◆ Sessions ────────────────────────────────────┐
│ > Save sessions…                              │
│   Restore sessions…                           │
│   Manual recovery setup…                      │
│   Continue held coordination…                 │
│                          Enter open · Esc close│
└───────────────────────────────────────────────┘
```

Before the first reboot, open Manual recovery setup. It shows the selected
Herdr config path and configured auto-resume value. Herdr 0.9.1 does not expose
the running server's loaded config path, so **A**, **a** or **Space** toggles the
acknowledgment that this is the correct file. **Tab**, then **Enter**, separately
confirms `session.resume_agents_on_restore = false`. Corgi validates a staged file with
`herdr config check`, backs up the original, changes only that TOML value and
requires a fully applied config reload. Layout restoration remains enabled.
Opening this screen, Save and Restore never change configuration.

Setup records a private acknowledgment. Missing-agent launches remain blocked
until the next normal Herdr start or reboot, with unchanged acknowledged config.
The installed API cannot establish that an old auto-resume queue is empty, so a
reload alone is insufficient. Corgi never requests a restart. Prepare setup
before reboot; Save prominently reports whether it is prepared. If the config
is edited afterward, confirm setup again. Existing matching agents can always
be inspected and reused without another launch. Reconfirming unchanged setup
retains its original server-start boundary.

Save inventories supervisors, workers and scratch sessions in this Herdr scope.
It lists missing native identities, unsupported harnesses, busy states and
unknown settings, then defaults to Cancel:

```text
┌ ◆ Save sessions ───────────────────────────────┐
│ 5 sessions · 1 missing ID · 0 unsupported       │
│ 1 busy/unknown                                 │
│ weather / supervisor · idle                    │
│ weather / radar · working · tools not saved    │
│ Scratch / notes · idle                         │
│ Scratch / untracked · unknown · ID unavailable │
│ References only; agents keep running.          │
│ Manual recovery setup prepared.               │
│ [Cancel]  Save       Tab choose · Enter confirm│
│ R refresh · W observe idle (up to 30s)          │
└───────────────────────────────────────────────┘
```

Save does not stop, prompt or pause agents and does not make a busy reboot safe.
W observes for at most 30 seconds and returns a fresh inventory for review;
it never saves automatically. Save recaptures immediately before committing
and refuses changed session/setup details. Every save is a new dated manifest;
previous good saves are retained, so no overwrite dialog is needed. Coverage
gaps stay visible and are not counted as restored conversations.

After reboot Herdr restores its layout and shells. Open S → Restore sessions,
browse a dated snapshot, then select entries individually with Space. **Zero
entries start selected. Enter on a snapshot only browses.** An existing exact
native-ID match, including a renamed/moved agent, is listed as already running.
Missing, unsupported and unavailable entries show individual reasons.

The final screen identifies timestamp, age, projects, selected count, original
placement IDs, setup and all skipped entries. Cancel/Back is initially focused.
Tab deliberately selects Restore. A accepts listed unknown/changed setup; O
adds acknowledgment for snapshots over seven days old, from the future, or
previously restored. These are inline checks in the same final confirmation.

```text
┌ ◆ Confirm restore ────────────────────────────┐
│ Saved 2026-10-21 14:00Z · 18 minutes old        │
│ Restore 2 conversations · Project weather: 2  │
│ supervisor · existing w7/w7:t1/w7:p1           │
│ radar      · existing w8/w8:t1/w8:p1           │
│ Skipped notes: already running in w3:p2        │
│ Skipped untracked: native ID unavailable       │
│ [ ] A: accept listed unknown setup             │
│ Supervisor coordination stays held.           │
│ [Back / Cancel]  Restore · Tab / Enter · Esc    │
└───────────────────────────────────────────────┘
```

Holding Enter cannot cross the confirmation: changing stages resets focus,
key-repeat events are discarded, and affirmative actions require a terminal
large enough to display the review. Up/down and PageUp/PageDown scroll long
summaries while action buttons stay visible.

```text
┌ ◆ Sessions — result ──────────────────────────┐
│ supervisor: same conversation ready in w7:p1   │
│             coordination held                 │
│ radar: start outcome unknown; do not retry     │
│ Read each outcome. Pending starts are not      │
│ automatically repeated.                       │
│ Continue coordination separately in Sessions.│
│                                     Esc close │
└───────────────────────────────────────────────┘
```

Escape during work stops scheduling additional entries; started harnesses and
created destinations remain available. Reopen Restore for a fresh validation
and confirmation of eligible retries. Continue held coordination is a separate,
cancel-first confirmation listing the affected projects. It releases only the
reviewed holds and does not itself send a prompt.

## Recovery contract and placement

“Exactly as it was” means the same saved native conversation at its **latest
durable history**, known recoverable settings, supervisor role/state access,
existing working files/worktrees, and original Herdr-restored terminals. This
is a reference to living conversation stores and memory, not a rollback to Save
time. RAM, running tools, shell jobs, unsaved editor content, unsent composer
input and arbitrary terminal processes cannot be checkpointed. Save reports
busy/unknown entries. Native pending-turn/queue metadata blocks launch where
detected. Detection is bounded and does not suspend processes or guarantee that
a native hook will not execute.

Normal restore validates the original workspace, tab and pane IDs and checkout.
`pane.process_info` must identify the foreground group as exactly the pane's
shell, at the expected directory, without another foreground process. Corgi
then calls **agent.start in that same pane**, with resume arguments and no task.
It never uses pane.run to inject a command into an unvalidated terminal.
Herdr performs its own launch-readiness check as well. Codex receives its saved
native cwd through --cd; its restored shell cwd is checked separately. Claude
requires the restored shell in its saved native cwd.

Corgi does not export/apply a competing layout, create replacement worktrees,
reset, switch branches, stash or change files. Changed branch/HEAD or native
config requires explicit acceptance and uses the existing state. Different
repository identity, missing checkout, missing CLI/store/conversation, changed
supervisor role or missing memory prevents resume. Fix the dependency and review
again. A missing/occupied original destination offers **F** for a visibly NEW
tab/workspace at an existing directory; Space cannot silently choose it. That
alternative appears in final confirmation. Otherwise it is skipped. If Herdr
has lost split geometry, Corgi cannot reconstruct it from these placement refs.

Native identities, not names, establish ownership. Matching duplicates block
that entry. Session-bound request metadata is restored only with existing marker
proof. Sequence-dependent merge readiness and handover tokens are historical
warnings, not replayed claims. A fresh supervisor using durable memory remains
the separate existing launch flow: recovery never calls supervisor preparation,
rewrites ROLE.md, or submits its greeting/digest prompt.

For both harnesses an idle resume supplies no prompt, fork or most-recent flag.
Codex uses `resume UUID --cd DIR`; Claude uses `--resume UUID` in DIR. Known raw
model/effort, permission policy and writable roots are reapplied. Supervisors
receive their unchanged role instructions and durable state directory access.
Unknown options are disclosed for acceptance or skip, not reconstructed from
arbitrary argv/environment. Native CLI startup/trust/permission screens are
left for the user; Corgi does not answer them. Startup hooks may run and are
called out when detected.

## Private metadata, not a transcript backup

The implementation is separated into [inventory/execution](../src/recovery/mod.rs),
[native adapters](../src/recovery/native.rs), [storage](../src/recovery/store.rs),
[setup](../src/recovery/setup.rs), [TUI state](../src/app/recovery.rs) and
[rendering](../src/ui/sessions.rs). It uses the existing Herdr client, theme
roles, background jobs and marker reconciliation.

Data lives in `$XDG_STATE_HOME/corgi/recovery` (default
`~/.local/state/corgi/recovery`):

```text
snapshots/<dated-unique-id>.json    immutable version-1 manifest
progress.json                     version-1 intents/outcomes/holds
profiles/<harness>-<UUID>.json     allowlisted session-bound launch settings
manual-setup.json                  confirmed config and server-start boundary
recovery.lock                     serializes mutations
```

The manifest has version, ID, monotonic save sequence, UTC creation time,
UID/machine/boot identity, Herdr socket scope, setup preparedness and entries:

| Entry data | Evidence and limit |
| --- | --- |
| Native UUID, harness, store namespace and exact file reference | Herdr native ID plus bounded exact-ID native-file verification. Missing IDs stay incomplete; no newest-by-directory fallback. |
| Raw model/effort, sandbox/approval or permission mode, extra directories, known profile/network policy | Session-bound Corgi launch profile where available, plus native metadata. Existing externally launched sessions often have unknown options. |
| Native and shell cwd, checkout, Git common directory, branch/HEAD, linked worktree flag | Herdr provenance, native metadata and read-only Git queries; never a worktree reconstruction recipe. |
| Workspace/tab/pane IDs and labels | Herdr snapshot; Herdr owns the layout, splits, ordering and shell restoration. |
| Project, supervisor/worker/scratch role, allowlisted ownership/request tokens | Live Corgi markers with native-session binding; merge/handover history is only a warning. |
| Supervisor ROLE path/fingerprint, memory path and known Corgi command | References existing durable assets; no role text or memory copy. Missing/unverifiable dependencies are reported. |
| Native config references/fingerprints and detected hooks; pending inbox IDs | Metadata/change detectors only, not hook bodies, settings contents, inbox messages or reports. |
| Agent state and coverage warnings | Save-time observations, not a process checkpoint. |

Directories are owner-only 0700, files 0600. Writes use a unique same-directory
temporary file, file fsync, atomic rename and parent-directory fsync. Readers
reject unsafe ownership, symlinks/hardlinks, malformed/newer records and bounded
size/count violations. Failed saves retain previous committed manifests;
interrupted temporary files are ignored. Save order uses sequence, even if the
wall clock goes backward. There is no automatic pruning or deletion UI in this
scope. Archive old private manifests if the 1,000-record bound is reached.

No private transcript text, task/prompt bodies, credentials, arbitrary argv or
environment dump goes into recovery data. Exact native files are read only for
identity/settings; unrelated conversations are not opened. The native stores,
Git worktrees/uncommitted files and supervisor memory already survive reboot
independently on durable storage. A snapshot cannot recover their deletion.
Volatile recovery state homes are rejected; volatile cwd/native/memory
references warn. Nothing required for recovery is deliberately stored in /tmp.

Scope is the same local account, machine and exact Herdr socket path. A recreated
socket at that path is supported. A different socket path, remote server or
machine has no automatic remapping in this focused feature and is rejected.
Native account/store changes are rejected too. Custom managed permission profiles
and arbitrary external launch options are not promised exact reconstruction;
unknown/default behavior must be accepted explicitly or the entry skipped.

## Deduplication, interruptions and coordination

One lock serializes restore/setup/save mutations. Durable per-native-identity
intent is written before placement creation and again before agent.start.
Outcomes record placement IDs, boot, snapshot and phase. Repeated restore and
cross-snapshot retries reconcile exact live native identity first. An accepted
but unconfirmed start is not sent again on that boot. Definitive rejection with
no accepted start may retain its known available placement for reviewed retry.
A confirmed native idle identity with failed marker writes permits metadata-only
retry. Successful prior restoration is acknowledged inline, never auto-rerun.
After a real boot change, old processes cannot still be running; fresh validation
may safely resume missing identities. Unknown placement creation leaves an
inspection requirement, not speculative duplicate placement creation.

A committed Save arms holds for its native supervisor identities on the next
boot, before the first waker refresh. Save itself leaves current coordination
running. Explicit restore also persists holds before launches. The waker retains
observation baselines but suppresses stop reports, inbox delivery, handover and
replacement for held projects. Malformed recovery state fails closed with a
visible status. Continue does not replay stops observed during a hold.

Inbox delivery intents contain item IDs only and are persisted before sends.
An uncertain delivery stays quarantined across dashboard/reboot; pre-reboot
undelivered item IDs in the saved manifest also require inspection with
`corgi inbox`. Continue does not blindly repeat them. Existing inbox/ledger/report
bytes remain intact. New explicit inbox items can be delivered after Continue.
All coordinating Corgi dashboards must run the updated build; an old binary cannot
honor these new holds. Corgi does not cancel coordination already in flight or
suspend native pending work, hooks or direct user input.

## Evidence, verification and review scope

Investigated installed Herdr **0.9.1 / protocol 22**, Codex **0.163.0** and
Claude Code **2.1.150**. Installed CLI help and API schema are authoritative for
this compatibility baseline. Read-only schema checks established session.snapshot,
pane.process_info, agent.start with native args and timeout, worktree.open and
config_reload's applied/partial/failed status. There is no native request
idempotency key, loaded config path or auto-resume queue barrier. Website-only
`resume_argv` support is not assumed.

Local source anchors at this revision: inventory
[src/recovery/mod.rs:185](../src/recovery/mod.rs), original-shell validation
[src/recovery/mod.rs:850](../src/recovery/mod.rs), holds
[src/recovery/mod.rs:871](../src/recovery/mod.rs), execution
[src/recovery/mod.rs:944](../src/recovery/mod.rs), delivery intents
[src/recovery/mod.rs:1257](../src/recovery/mod.rs), native metadata/arguments
[src/recovery/native.rs:151](../src/recovery/native.rs) and
[src/recovery/native.rs:322](../src/recovery/native.rs), durable saves
[src/recovery/store.rs:49](../src/recovery/store.rs), and explicit setup
[src/recovery/setup.rs:146](../src/recovery/setup.rs).

Official references: [Herdr session restoration](https://herdr.dev/docs/session-state/)
explains layout versus process/native restoration; [Codex resume](https://learn.chatgpt.com/docs/developer-commands?surface=cli#codex-resume)
documents existing-thread resumption; [Claude CLI reference](https://code.claude.com/docs/en/cli-reference)
documents --resume, model/effort, extra directories and appended system-prompt
file support. The adapters use these native paths without an initial task.

Codex compatibility used one disposable synthetic conversation. Interactive
resume showed the prior answer, idle state, selected model/effort and cwd; its
own native transcript retained exactly one task-start and task-complete across
resume, proving no automatic new turn. An initial trust screen was not counted
as success or answered. Role/state arguments were included; no real agent was
prompted. The intended primary checkout was already trusted and was not edited.

Claude verification is **documentation + installed parser/help + fixtures**,
not a live successful resume. The only synthetic live attempt returned 401
before a model turn. The user has no subscription and explicitly waived the
live-Claude completion gate. No login, auth modification, billing or retry is
required. Claude remains implemented. Independent live Codex verification after
this implementation is assigned by the supervisor and remains a separate review
step; no real reboot or active Herdr restart is authorized.

Focused tests in [recovery/tests.rs](../src/recovery/tests.rs) and the TUI/waker
modules cover both native adapters and role/memory access; original IDs with
zero placement creations/prompts; strict identity/private writes; missing and
occupied destinations; unknown starts and metadata-only retry; next-boot holds,
observation baselines and delivery quarantine; config preservation and explicit
confirmation. Full fmt, clippy, tests and release build are required by AGENTS,
plus synthetic README regeneration and a labeled isolated UI preview. Development
uses fake sockets/config/native stores for lifecycle and failure cases. The full
suite passed (485 library, one binary and two integration tests; five existing
live/asset tests ignored), as did fmt, clippy and the release build. README assets
were regenerated from made-up agents; the animation is 234,400 bytes. The release
UI was exercised through setup, Save, an isolated fake-server restart and Restore:
both synthetic conversations retained original placement IDs, with two starts,
zero placement creations, zero prompts and two durable supervisor holds. The
actual Herdr server and existing agents were not restarted or interrupted.

The recommended implementation scope is this local dual-harness workflow.
A second layout restorer would conflict with Herdr; a process checkpoint system
would promise unavailable guarantees; a fresh supervisor from memory loses the
requested conversation. Broader remote/remapping, arbitrary argv replay and
history backup remain separate features. Sol high is suitable for this bounded
implementation; keep independent Codex verification separate, as requested.
There are no unresolved product decisions for this approved scope.
