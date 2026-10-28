# `corgi serve`: the frontend protocol

`corgi serve` is how a frontend that draws its own window, such as the
desktop GUI, sees and acts on Corgi. It speaks JSON lines over stdin and
stdout, so the frontend runs it locally or on another machine:

```sh
corgi serve                       # $HERDR_SOCKET_PATH, else the default session
corgi serve --session work        # ~/.config/herdr/sessions/work/herdr.sock
corgi serve --socket /path/to/herdr.sock
ssh host corgi serve              # the same, on another machine
```

The types are `corgi_core::protocol` (`crates/corgi-core/src/protocol.rs`);
a frontend in Rust depends on `corgi-core` and deserializes into them rather
than writing its own. This document describes the same messages for readers
and for frontends in other languages.

## Transport

- One JSON object per line, UTF-8, `\n`-terminated, in both directions.
  Text that holds newlines (a prompt, a transcript entry) has them escaped by
  JSON, so a message is always one line.
- stdout carries only protocol lines. Anything else Corgi has to say (a
  failed write, a refresh that panicked) goes to stderr.
- Corgi exits when stdin closes, once the commands still running (a merge,
  a launch) have replied. It also exits when stdout is gone. Closing stdin is
  how a frontend says goodbye; killing the process works too, at the cost of
  a merge or launch under way.
- A frontend ignores fields and message types it does not know.
  `protocol` in the hello rises only for a change an older frontend would
  misread.

## The session

```text
corgi → {"type":"hello", "protocol":1, …}
corgi → {"type":"snapshot", "seq":0, "now":…, "state":{…}}
corgi → {"type":"delta", "seq":1, "now":…, "changes":[…]}      whenever a refresh changed something
  gui → {"id":1, "command":"prompt", "pane_id":"w2:p1", "text":"go on"}
corgi → {"type":"reply", "id":1, "ok":true, "result":{"kind":"done", "message":"Prompt sent to w-fix"}}
```

Corgi refreshes as the dashboard does, about every 850 ms after the last
refresh ended and at once after a command that changed Herdr or Git, and
sends a delta only when something changed. It runs the dashboard's refresh
pipeline and contends for the same per-socket wake lock
(`$STATE/wake/<socket>.lock`), held for one pass at a time: whichever of
Herdr's hooks (`corgi hook`), the TUI dashboards and `corgi serve`
processes holds it restores Corgi's marks, wakes supervisors and delivers
their inboxes, and a refresh that a hook left work to runs again at once.
So a GUI, a TUI and the widget can run in any combination, or none.

## Server messages

Every message has `type`.

### `hello`

The first line.

```json
{"type":"hello","protocol":1,"corgi":"0.2.3","socket":"/Users/me/.config/herdr/herdr.sock","pid":4242,
 "commands":[{"name":"resync","touches_focus":false}, …, {"name":"focus","touches_focus":true},
             {"name":"edit","touches_focus":true}, {"name":"digest","touches_focus":false}]}
```

`commands` lists every command this server takes. **`touches_focus`** marks
the commands that call Herdr's focus methods (`workspace.focus`, `tab.focus`,
`agent.focus`), which move what *every* attached Herdr client shows. A GUI
never sends them; they are there for a TUI-style caller that runs inside
Herdr. Today they are `focus` and `edit`.

### `snapshot`

The whole state: right after the hello, and again after `resync`.

```json
{"type":"snapshot","seq":0,"now":1793204989,"state":{…State…}}
```

- `seq` numbers states. The next delta is `seq + 1`.
- `now` is the server's clock in Unix seconds. Every time in the state is a
  Unix time on that clock, so a frontend over `ssh` to a machine whose clock
  differs computes countdowns from `now`, not its own clock.

### `delta`

What changed since the state numbered `seq - 1`.

```json
{"type":"delta","seq":5,"now":1793204990,"changes":[
  {"change":"agent", …the whole Agent…},
  {"change":"agent_removed","pane_id":"w3:p1"},
  {"change":"agent_order","pane_ids":["w1:p1","w2:p1"]},
  {"change":"project", …the whole Project…},
  {"change":"project_removed","group":"Scratch"},
  {"change":"project_order","groups":["weather"]}
]}
```

A frontend applies the changes in order. If its last `seq` is not
`seq - 1`, it sends `resync` and starts over from the snapshot.

| `change` | Fields | Meaning |
| --- | --- | --- |
| `herdr` | `connected`, `error` | Herdr answered the last refresh, or why not. While Herdr is away the rows stay as they were; show them dimmed. |
| `status` | `status` | The dashboard's status line from the refresh (marks restored, a supervisor woken), or `null` once it has lapsed. |
| `usage` | `usage` | The whole usage list. |
| `agent` | an `Agent` | A row that is new or changed, whole. |
| `agent_removed` | `pane_id` | A row that went. |
| `agent_order` | `pane_ids` | The rows' order, sent whenever a row came, went or moved. |
| `project` | a `Project` | A project that is new or changed, whole. |
| `project_removed` | `group` | |
| `project_order` | `groups` | |
| `transcript` | `transcript` | The watched session's turns, or `null`. |
| `selection` | `pane_id` | |
| `palette` | a `PaletteSet` | The colors changed (an Omarchy theme switch). |

### `progress`

Progress of a long command (`spawn`, `start_supervisor`, `merge`) before its
reply, with the request's `id`.

```json
{"type":"progress","id":7,"steps":["Check both worktrees","Fetch origin/main","Merge worktree/fix into main","Push main"]}
{"type":"progress","id":7,"step":1}
{"type":"progress","id":7,"status":"Fetching origin/main"}
```

`steps` comes once, first; `step` is the index (from 0) of the step now
running; `status` is a line of what it is doing.

### `reply`

Exactly one per request, with its `id`.

```json
{"type":"reply","id":3,"ok":true,"result":{"kind":"done","message":"Cleared the merge tag of w-fix"}}
{"type":"reply","id":4,"ok":false,"error":"no agent in pane w9:p9"}
{"type":"reply","id":5,"ok":false,"error":"Describe the first task before creating the agent","field":"task"}
{"type":"reply","id":null,"ok":false,"error":"not a JSON object: expected value at line 1 column 1"}
```

- `ok: false` carries `error`, a sentence to show the user. Nothing a
  command can meet crashes the server; a command that panics replies
  `the command stopped unexpectedly`.
- `field`, on a refused `spawn`, is the new-agent form's row to point at:
  `task`, `harness` or `project`.
- `id` is `null` only for a line that could not be read far enough to find
  one.
- An error never repeats a prompt's text, not even in a parse error.

`result` has `kind`:

| `kind` | Fields | From |
| --- | --- | --- |
| `done` | `message` | most commands |
| `new_agent_form` | a `NewAgentForm` | `new_agent_form` |
| `supervisor` | `pane_id`, `started`, `message` | `start_supervisor` |
| `merge_form` | a `MergeForm` | `merge_check` |
| `merge_outcome` | a `MergeOutcome` | `merge` |
| `close_form` | a `CloseForm` | `close_check` |
| `close_dirty` | `message` | `close` on a worktree with uncommitted changes |
| `digest` | `text` | `digest` |

## The state

```json
{"herdr":{"connected":true,"error":null},
 "status":null,
 "usage":[…UsageCard…],
 "projects":[…Project…],
 "agents":[…Agent…],
 "transcript":null,
 "selection":null,
 "palette":{…PaletteSet…}}
```

### `Agent`: a row

```json
{"pane_id":"w2:p1","workspace_id":"w2","tab_id":"w2:t1",
 "name":"w-fix","herdr_name":"w-fix","harness":"codex",
 "state":"blocked","state_change_seq":5,"ready_to_merge":false,"focused":false,
 "supervisor":false,"scratch":false,
 "project_group":"weather","project":"weather/weather-fix","project_root":"/repos/weather",
 "worktree":"/repos/weather-fix","checkout":"weather-fix",
 "task":"Fix the forecast","model":"gpt-5-codex","effort":"high",
 "context_percent":42,"context_tokens":84000,
 "cache":{"expires_at":1793205289,"tokens":84000,"estimated":false},
 "last_activity":1793204900,
 "message":{"kind":"question","text":"Waiting for your input"},
 "tool":{"kind":"command","text":"cargo test"},
 "can_prompt":false,"can_merge":true}
```

- `pane_id` is the row's key and every command's handle on the agent.
- `state` is `idle`, `working`, `blocked`, `done` or `unknown`.
  `ready_to_merge` replaces it with MERGE in the dashboard.
- `name` is what Herdr shows; `herdr_name` is the name Corgi gave the agent,
  by which its supervisor knows it. A supervisor's `task` is `Supervisor`.
- `message` is the newest thing said in the session; `tool` the call it runs
  or last ran. `kind` is one of `command`, `tool`, `prompt`, `message`,
  `thinking`, `question` or `ready`.
- `cache.expires_at` is when the prompt cache lapses (`estimated`: when its
  documented minimum retention ends). Count down from the snapshot's `now`.
- `can_prompt` is false while the agent is blocked on its own question,
  which is answered in its pane. `can_merge`: it works in a linked worktree.
- The rows are in the dashboard's order: by project, each led by its
  supervisor, the scratch sessions last, and within a project blocked,
  working, done, idle, unknown, the most recent first.

### `Project`: a heading or a card

```json
{"group":"weather","root":"/repos/weather","card":true,"scratch":false,
 "supervisor":"w1:p1","agents":["w1:p1","w2:p1"],
 "workers":{"total":1,"blocked":1,"merge":0,"working":0,"done":0,"idle":0,"unknown":0},
 "blocked":[{"pane_id":"w2:p1","task":"Fix the forecast","waiting_on":"Waiting for your input"}],
 "questions":[{"id":"Q4","title":"Keep the old flag?","agent":"w-fix"}]}
```

- `card` is true for a project led by its running supervisor, which the
  dashboard draws as a card; other projects and the scratch sessions
  (`scratch`, `root` empty) are a heading over their rows.
- `workers` is what a collapsed card's footer sums up, the supervisor not
  counted; a worker tagged ready to merge counts only under `merge`.
- `blocked` is each blocked worker and what it waits on: the question on its
  screen, else `waiting for your input`.
- `questions` are the numbered questions the supervisor has open with the
  user, from its ledger: the `questions` of each ledger id's newest line
  whose work is not finished. Answer one with `answer`. `id` is empty for a
  question the supervisor listed without a number.

### `UsageCard`: the plan-usage header

```json
{"provider":"claude","title":"CLAUDE",
 "usage":{"provider":"claude","plan_type":"max",
          "five_hour":{"used_percent":12,"resets_at":1793210000},
          "week":{"used_percent":40,"resets_at":1793600000},
          "banked_resets":null},
 "fetched_at":1793204950,"error":null}
```

One per provider whose CLI is installed on the server's machine. `usage` is
the last good reading, kept through failed fetches; `error` says why the
newest fetch failed. A `resets_at` of 0 means no reset time was reported.

### `Transcript`

```json
{"pane_id":"w2:p1","entries":[{"kind":"message","text":"Done: the forecast …"},
                              {"kind":"command","text":"cargo test"}]}
```

The newest turns of the session `watch` named, **newest first**, the
agent's reasoning left out: what the dashboard's expanded view shows. Empty
for a harness whose session file Corgi cannot read. Read again at every
refresh.

### `PaletteSet`: the theme's colors

```json
{"source":"catppuccin",
 "light":{"foreground":"#4c4f69","background":"#eff1f5","ansi":["#5c5f77", …16…],
          "roles":{"accent":"#…","muted":"#…","merge":"#…", …},
          "context_ramp":["#…", …7…]},
 "dark":{…}}
```

Colors are `#rrggbb`. `roles` holds each role of `palette::Roles` by its
field name (`accent`, `muted`, `success`, `warning`, `danger`, `unknown`,
`merge`, `thinking`, `text`, `quiet`, `badge_text`, `popup_fill`, `keycap`,
`rule`, `focus_bar`, `dimmed`, `dimmed_background`, `fade_from`,
`foreground`, `success_frame`, `stamp`, `flash`); `context_ramp` runs from
an empty context window (green) to a full one (red).

- `source: "omarchy"`: the active Omarchy theme on the server's machine;
  `light` and `dark` are the same theme.
- `source: "forced"`: `CORGI_THEME` names one; both are the same.
- `source: "catppuccin"`: Latte and Mocha, for the frontend to follow the
  system's light or dark scheme.

The terminal is never asked for its colors, since stdin and stdout carry
the protocol. Brand colors (`palette::brand`) are fixed and not sent.

## Requests

```json
{"id":7,"command":"merge","pane_id":"w2:p1","upstream":"check"}
```

`id` is any unsigned integer the frontend chooses; the reply and any
progress carry it. `command` names the command; its fields sit beside it.
Commands may run at the same time; replies come in the order commands
finish.

| Command | Fields | Touches focus | Does |
| --- | --- | --- | --- |
| `resync` | | | Sends a snapshot, the next `seq`, then replies. |
| `refresh` | | | Refreshes now. |
| `select` | `pane_id` or `null` | | The agent the frontend selected (below). |
| `watch` | `pane_id` or `null` | | The session whose transcript the state carries. |
| `prompt` | `pane_id`, `text` | | Types `text` into the agent and submits it. |
| `answer` | `project_root`, `question`, `text` | | Answers a numbered question. |
| `new_agent_form` | `pane_id`?, `project`?, `harness`?, `model`? | | The new-agent form, preset. |
| `spawn` | `project`, `new_project`, `harness`, `model`, `effort`, `task`, `checkout` | | Starts the agent. |
| `start_supervisor` | `project_root` | | Starts the project's supervisor, or names the running one. |
| `merge_check` | `pane_id` | | The merge confirmation. |
| `merge` | `pane_id`, `upstream` | | Runs the confirmed merge. |
| `merge_handoff` | `pane_id` | | Hands a conflict or failed push to the supervisor. |
| `merge_dismiss` | `pane_id` | | Forgets the merge. |
| `close_check` | `pane_id` | | The close confirmation. |
| `close` | `pane_id`, `force` | | Closes the agent's worktree, tab or workspace. |
| `tag` | `pane_id`, `merge` | | Tags the agent ready to merge, or clears the tag. |
| `focus` | `pane_id` | **yes** | Brings the agent's pane into view in Herdr. |
| `edit` | `pane_id` | **yes** | Opens the agent's checkout in the editor, in a new Herdr tab it goes to. |
| `digest` | `project_root`, `lookup` | | Reads the supervisor's memory. |

### `prompt`

```json
{"id":1,"command":"prompt","pane_id":"w2:p1","text":"Run the tests again"}
```

Delivered once and never retried, since a failure can follow delivery. A
blocked agent is refused (`Blocked agents must be handled in their own
pane`), as are empty text and an unknown pane. Any other failure says
`Could not confirm prompt delivery (<Herdr's error code>). Check the agent
before retrying.` and never includes the text.

### `answer`

```json
{"id":2,"command":"answer","project_root":"/repos/weather","question":"Q4","text":"B, keep it"}
```

Prompts the project's running supervisor with `Q4: B, keep it`, the form its
role asks the user to answer in. `question` is `Q4`, `q4` or `4`. Refused
when no supervisor runs for the project.

### `new_agent_form` and `spawn`

```json
{"id":3,"command":"new_agent_form","pane_id":"w2:p1"}
```

The form preset as the dashboard's `n` presets it: the project of `pane_id`
(its repository's primary checkout, so the new agent gets a sibling
worktree; never a scratch agent's home directory), else `project`, else the
project used most recently, else the first known one. `harness` defaults to
`CORGI_DEFAULT_AGENT`, else the first installed harness; send the form's
`harness` and `model` again to get the lists for them.

```json
{"type":"reply","id":3,"ok":true,"result":{"kind":"new_agent_form",
 "project":"/repos/weather",
 "projects":[{"value":"/repos/weather","label":"weather","detail":"/repos/weather","badge":"2 agents"}, …],
 "new_project_parent":"/repos",
 "harness":"codex","harnesses":[{"value":"codex","label":"codex","detail":"","badge":"installed"}, …],
 "model":"","models":[{"value":"","label":"Harness default (gpt-5-codex)", …}, …],
 "effort":"","efforts":[…],"supports_effort":true,
 "checkout":"worktree","checkouts":[{"value":"worktree","label":"New Git worktree","detail":"own branch and checkout","badge":""},
                                   {"value":"directory","label":"Project directory as is", …}],
 "default_model":"gpt-5-codex","default_effort":"high",
 "starts_supervisor":false}}
```

Lists are the dashboard's inline lists, in order, `value` being what the
form stores (`""` is "harness default"). Leave the effort row out unless
`supports_effort`, and the checkout row out for a new project.
`starts_supervisor` says the project has no agent yet, so `spawn` starts its
supervisor on the chosen harness.

```json
{"id":4,"command":"spawn","project":"/repos/weather","harness":"codex","model":"","effort":"high",
 "task":"Fix the forecast for leap days","checkout":"worktree","new_project":false}
```

The form's own checks come first and point at a row with `field`: a task is
required except for a scratch agent (the home directory), a harness, and an
existing absolute directory (or `new_project` for one chosen as new from
the project list). Then `progress` with the launch's steps, and a `done`
reply such as `weather started in weather worktree worktree/fix`. Every field may be
left out, as empty; `checkout` empty is `worktree`.

### `start_supervisor`

```json
{"id":5,"command":"start_supervisor","project_root":"/repos/weather"}
```

The dashboard's supervisor key, without the focus. When it runs, replies at
once with `{"kind":"supervisor","pane_id":"w1:p1","started":false,…}`, for
the frontend to show its panel. Otherwise starts it on the harness, model
and effort it last ran with (else the form's preset harness), with progress,
and replies `started: true` with its new pane.

### Merging

A merge is the dashboard's `m` dialog as three commands.

1. `merge_check` checks both checkouts and returns what the confirmation
   shows:

   ```json
   {"kind":"merge_form","pane_id":"w2:p1","agent":"w-fix","task":"Fix the forecast",
    "project_root":"/repos/weather","worktree":"/repos/weather-fix",
    "source_branch":"worktree/fix","target_branch":"main",
    "commits":["1a2b3c4 Fix leap days"],"upstream":"origin/main",
    "steps":["Check both worktrees","Fetch origin/main","Merge worktree/fix into main","Push main"]}
   ```

   It is refused for an agent outside a linked worktree, for a dirty
   checkout, and when both are on the same branch.

2. `merge` with `upstream` `check` (the default) runs it, with progress, and
   replies `merge_outcome`, `ok: true` whatever the outcome:

   | `outcome` | Fields | Then |
   | --- | --- | --- |
   | `succeeded` | `commit`, `message` | Done. The agent's merge tag is cleared and its supervisor is told. Offer `close_check`. |
   | `behind` | `commits` | The upstream has commits the target lacks; nothing merged. `merge` with `pull` brings them in first. |
   | `fetch_failed` | `error` | Nothing merged. `merge` with `skip` merges without the upstream. |
   | `conflicted` | `files`, `supervisor` | The primary checkout is mid-merge. `merge_handoff` aborts it and asks `supervisor` (when not `null`) to have the worker merge the base into its branch; `merge_dismiss` leaves it to resolve by hand. |
   | `push_failed` | `commit`, `error`, `supervisor` | Merged locally; the tag is cleared and the supervisor told. `merge_handoff` asks it to explain the error. |
   | `failed` | `error` | |

3. `merge_handoff` or `merge_dismiss` ends a conflicted or push-failed
   merge. A succeeded or failed merge is forgotten by itself; `merge_check`
   starts a new one.

### Closing

`close_check` says what closing the agent takes down:

```json
{"kind":"close_form","pane_id":"w2:p1","label":"weather/weather-fix",
 "target":"worktree","checkout":"/repos/weather-fix","confirm":"Remove worktree"}
```

`target` is `worktree` (removes the checkout from disk; the branch stays),
`tab` (its tab in the project's workspace) or `workspace`. `close` does it.
A worktree with uncommitted changes replies `close_dirty`; confirm again
with `force: true` to discard them.

### `tag`

`{"command":"tag","pane_id":"w2:p1","merge":true}` tags the agent ready to
merge as `corgi tag` does, with the same refusals (it works, has no commits
ahead of the base, or its supervisor has questions open about it);
`merge: false` clears the tag.

### `focus` and `edit` (touch focus)

`focus` walks Herdr to the agent's workspace, tab and pane. `edit` opens the
agent's checkout in `$VISUAL`/`$EDITOR` in a new tab of its workspace named
after its branch, and goes to that tab. Both move every attached Herdr
client's view, so a GUI never sends them.

### `select`

`{"command":"select","pane_id":"w2:p1"}` records the frontend's selection in
the state and in `$STATE/selection/<socket>.<pid>`, the file `corgi edit`
reads to open the agent selected in the dashboard whose process is in the
focused pane. That only applies to a `corgi serve` running in a Herdr pane;
a GUI opens a checkout with `edit` instead, or not at all. The file is
removed when the server exits.

### `watch`

`{"command":"watch","pane_id":"w2:p1"}` puts that session's transcript in
the state from the next refresh (asked for at once), and keeps it read at
every refresh; `null` stops.

### `digest`

```json
{"id":9,"command":"digest","project_root":"/repos/weather","lookup":{"kind":"search","words":"leap day"}}
```

What `corgi digest` prints, as `text`. `lookup.kind` is `summary` (the
default; the whole digest), `search` and `decision` (with `words`), `show`
(with `handle`, such as `decisions.md:12`), `ledger` and `brief` (with
`id`).
