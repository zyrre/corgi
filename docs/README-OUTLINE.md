# README outline (for approval)

The proposed structure of the new `README.md`, written for people who
already run coding agents in Herdr. Each section gets one line on what it
says. 🖼 marks a visual: ✅ means a sample exists in `docs/images/`, ⏳ means
it is added in step 2. Every screenshot is drawn by `cargo readme-shots`
from Corgi's own UI code, using made-up agents (`webshop`, `weather`,
`notes-app`, `/home/jane.doe`).

## Top of the page

1. **Logo lockup**: 🖼 ✅ `logo.svg`, the pixel corgi and the CORGI
   wordmark cut from the dashboard header, centred.
2. **Pitch**, one or two lines: *Corgi is the dashboard for your Herdr agents:
   every session in one pane, grouped by project, showing what it is doing,
   what it waits on, and what its context and cache cost you. Press `n` for a
   new agent in its own worktree.*
3. **Hero screenshot**: 🖼 ✅ `dashboard.svg`. It shows three projects, a
   Steward, a blocked agent, working and done agents, cache countdowns
   (exact, estimated `≈` and cold), context %, and the HERD, Codex and Claude
   plan cards.
4. **Install as a Herdr plugin**: three commands (`cargo build --release`,
   `herdr plugin link "$PWD"`, `herdr plugin pane open …`), followed by the
   `prefix+d` key binding snippet. Also mentions the `quick` popup
   entrypoint.

## What you get

5. **At a glance**: six short bullets, each one sentence, with the details
   left to later sections: one row per agent, the newest message and tool
   call, the cache countdown, `space` to read the transcript, `n` for a
   worktree agent, `m` to merge, and the Steward.
6. **Reading a row**: 🖼 ⏳ a crop of one agent row with callouts (state
   badge, task, model and effort, ctx %, cache, worktree), and the markers
   `»` `›` `…` `$` `●` `?` in a small table.
7. **Read a session in place**: 🖼 ✅ `expanded-session.svg`. `space`
   grows the row into that session's latest turns, newest first, and every
   key keeps working while it is open.

## Everyday use

8. **Keys**: the dashboard cheat sheet as one table (the current table,
   tightened). The rules for the expanded view go below it in two lines.
9. **Start an agent**: 🖼 ✅ `new-agent-form.svg`. The form opens on the
   task with every other setting preset; the table of form keys follows,
   then one paragraph on the Harness/Model/Effort rows, including how
   "Harness default (…)" is read from each CLI's config.
10. **Choosing a project**: 🖼 ⏳ the form with the Project list open. It
    covers the four sources in order, typing a path, and `new project`.
11. **Worktrees, merging and closing**: each agent gets its own
    `worktree/<name>` branch, `m` merges and pushes after a confirmation, and
    `x` removes the checkout. 🖼 ⏳ the merge confirmation, and 🖼 ⏳ one
    frame of the success check stamp.
12. **Scratch sessions**: `t` starts an agent in `~` with no worktree and no
    Steward (two lines).

## The Steward

13. **What it is**: one long-lived Claude Code or Codex session per project
    that you talk to, which plans the work and dispatches it to worktree
    workers. 🖼 ⏳ a Mermaid diagram:
    `you ⇄ Steward → corgi spawn → workers → done/blocked → dashboard wakes Steward → corgi report`.
14. **Starting it**: `s` in the dashboard, `corgi steward <dir>` from a
    shell, and what a new project gets (`git init`, first commit).
15. **How it follows its workers**: `corgi fleet`, `corgi report`, and the
    one-line wake-up prompt. It stays out of your own sessions, and several
    open dashboards share a lock.
16. **Its memory**: `decisions.md`, `ledger.jsonl` and `briefs/` under
    `$XDG_STATE_HOME/corgi/steward/<project>/`.
17. **Trust prompts**: when Corgi answers Claude Code's and Codex's trust
    question itself, and when it leaves the question to you (kept short).

## For scripts and other agents

18. **`corgi spawn`**: the brief is read from stdin, every option has the
    form's default, and the result is printed as JSON. One example.

## Plan usage

19. **The plan cards**: 🖼 ⏳ a crop of the header cards. It explains where
    each number comes from (Codex app-server, Claude's OAuth usage endpoint,
    unofficial and read-only) and what "unavailable" means.

## Reference

20. **Configuration**: one table of environment variables
    (`CORGI_DEFAULT_AGENT`, `CORGI_CODEX_BIN`, `CORGI_CLAUDE_BIN`,
    `CORGI_PROJECTS_FILE`, `CORGI_STEWARD_BIN`) and the files Corgi keeps
    under `$XDG_STATE_HOME/corgi/`.
21. **Data and privacy**: everything goes over the Herdr socket, with no
    telemetry and no stored terminal output; the only network calls are the
    plan-usage reads (current text, tightened).
22. **Architecture**: one line with a link to `ARCHITECTURE.md`.
23. **Omarchy companion**: two lines with a link to `omarchy/README.md`.
24. **Regenerating the screenshots**: `cargo readme-shots`, one line.
25. **License**: MIT.

## Notes

- Section order puts what a Herdr user sees first (sections 1–9) and the
  mechanics (worktree layout, trust, state files) lower down. No current
  content is dropped; the long paragraphs about the project list, model
  catalogs and cleanup rules become short lists under their sections.
- The screenshots use the Tokyo Night palette, since Corgi follows the
  terminal theme and a screenshot has to pick one. The window's dark frame
  reads well on GitHub's light and dark themes alike.
