// The Corgi mod's Projects list (its `projects` view, main.js): the
// sidebar's spaces, each repository with its agents under it, and "Open
// project" on its +. It stands where Herdr's Spaces and Agents lists stand in
// Herdr's own layout; `corgi app-layout` puts it there, in the layout itself.
//
// A project is a repository: the space on its primary checkout, with the
// spaces on its worktrees grouped under it by git. Under a project, one row
// per agent in it, its supervisor first: Corgi's agents with their task and
// role from `corgi status --json`, and agents started by hand with their own
// title. Each agent is a shaded card with its state's color down its left
// edge, on two lines: the task, then the harness, model and context
// fill with the state's badge at the right. A project's row counts its
// agents by state. A worktree's space with no agent keeps its plain row. Spaces outside
// any repository (~, other folders) show as spaces, with
// their agents under them. Click a project's row to fold it to its
// supervisor (or unfold it), another space to show it and an agent to show its
// terminal; right-click for the space's menu; drag a space to reorder. The
// command `corgi.open` does what the + does, for a key of the person's
// (keys.json: "cmd-o": "corgi.open").
//
// Each agent's row ends with its state's badge (WORKING, BLOCKED, DONE,
// IDLE, PAUSED, EXITED), the words `corgi fleet` uses for a state. A
// supervisor's first line ends with a blue "? N" when N of its questions
// have no answer yet: a click shows the Questions panel on its project. The
// list tells that panel which project is current (the active space's, or
// the one whose count was clicked) and how many of its questions are open.
// A worker Corgi tagged ready to merge has a MERGE badge in its place: a click
// asks under the row, then runs `corgi merge`, and shows how it went, with "Ask supervisor to fix" when it
// failed and the project's supervisor runs. Once merged and pushed, it asks
// whether to close the worker and remove its worktree (`corgi close`, the
// branch stays). A space's red ↓N (its checkout is N commits behind its
// upstream, as last fetched) pulls them with a click: `git pull --ff-only`
// in that checkout, nothing else; only a pull that fails leaves a card,
// under the row, saying why. At the top, above the header, a card counts the agents by
// state (HERD) and shows each plan's 5-hour and weekly use (`corgi usage
// --json`). Under the list, which Corgi
// answers (`Corgi 0.2.3 · 5b50895`), whether the checkout it was built in
// has moved on since, and whether the daemon is down or runs another build.
//
// Left of the +, "⏸ Agents" pauses the agents of Corgi's that are working,
// before the network goes (`corgi pause`): each ends its turn and exits,
// its conversation saved, and while any is paused Corgi's daemon types
// nothing into a supervisor. The button then reads "▶ Agents" and brings
// back exactly those (`corgi resume`). Paused agents' rows show a ⏸. With
// none working a click changes nothing and says so in a toast; agents that
// did not stop, or did not come back, are listed in a card under the header.
//
// Another machine's spaces (Herdr's "Add machine") are under its own
// header, with their agents' Corgi data from that machine's own corgi,
// asked there through herdr-core (`exec` with `on`); a merge, a close, a
// pull and a pause of its agents run there too. Plan usage and the last
// line stay this computer's. What Corgi says is read in data.js, once for
// every copy of the mod.
//
// It began as a copy of Herdr's `spaces` mod and is Corgi's own since.

import { div, image, svg } from "gpui-kit";
import { v_flex, h_flex, InputState, Input } from "gpui-base";
import {
  spaces, machines, panes, agents, focused, run, openOverlay, exec, hc, theme, shared, setShared, setAgentOrder,
  watchFolder, window, home, nextCommand, notify,
} from "herdr";
import { narrow, toggleSidebar, sidebarPx, oneLine, tildePaths, spinner, BADGES, badgeKey, badge, STATES, stateCounts, stateColor } from "./ui.js";
import { store, POLL_MS, USAGE_POLL_MS, MACHINES_MS, NO_CORGI, CURRENT, COUNT, PICK, QUESTIONS_BLOCK } from "./data.js";

// Where a space is now: its first terminal's live folder, else where it was made.
// `ps`: panes(), read once by the caller (each read copies every pane).
function place(s, ps) {
  const tabs = new Set((s.tabs ?? []).map((c) => c.id));
  return ps.find((p) => tabs.has(p.tab) && p.cwd)?.cwd || s.folder;
}
const SPACE_ROW = 30;
const TREE_ROW = 28;
const AGENT_ROW = 38;
// An agent's card (`agent`): its corners' rounding, and the bar in its
// state's color that stands just past them.
const CARD_RADIUS = 6;
const CARD_EDGE = 3;
// The narrowest sidebar whose cards keep their full margins.
const CARD_ROOMY = 230;
const RAIL_CELL = 32;
const railCell = (el) => h_flex().flex_none().justify_center().child(el);
const where = (s, ps) => JSON.stringify([s.machine ?? null, place(s, ps)]);
const base = (path) => path.replace(/\/+$/, "").split("/").pop();
// Changes made outside Herdr (an editor's save, a commit in another app): looked
// for this often, in repos the list shows, while the window is in front.
const FALLBACK = 60000;
// How many suggestions "Open project" shows at once.
const SUGGESTIONS = 8;
// Starting a supervisor waits for its shell, its agent and a folder-trust
// question: about a minute at the most.
const OPEN_TIMEOUT_MS = 120000;
// A merge fetches, merges and pushes; exec's longest wait is 10 minutes.
const MERGE_TIMEOUT_MS = 600000;
// What makes a Corgi built from a checkout the newest merged there again
// (AGENTS.md, "After a merge"): run in that checkout.
const INSTALL_COMMAND = "CORGI_BUILD=source herdr-app/install.sh";
// What starts the daemon, or restarts it, with the corgi that answers.
const DAEMON_COMMAND = "corgi daemon install";

// A pause interrupts each working agent and waits for it to exit; a resume
// waits for each to come back: some seconds an agent, a few minutes at most.
const PAUSE_TIMEOUT_MS = 300000;
// The pause button's icons: ⏸ and ▶, the mod's own.
const PAUSE_ICON = "mods/corgi/pause.svg";
const PLAY_ICON = "mods/corgi/play.svg";
// How long the toast of a pause that found no agent working stays.
const NOTE_MS = 3000;
// A pull fetches first: a slow remote, a big fetch.
const PULL_TIMEOUT_MS = 120000;
// The pull, never asking for a password or a passphrase: there is no
// terminal to ask in, so it would wait out PULL_TIMEOUT_MS. The ssh command
// the person uses (GIT_SSH_COMMAND, else core.sshCommand) is kept, in batch
// mode. `exec` takes no environment, hence the shell; it runs git alone.
const PULL_SCRIPT = 'ssh="${GIT_SSH_COMMAND:-$(git config --get core.sshCommand)}"; '
  + 'export GIT_TERMINAL_PROMPT=0 GIT_SSH_COMMAND="${ssh:-ssh} -o BatchMode=yes"; '
  + "exec git pull --ff-only";

// What a stored color name looks like on an avatar (the same in every mod
// that draws space avatars): one strong color per hue, light and dark alike.
const MARK = { ansi6: "#22a99a", ansi3: "#e3814f", ansi4: "#5b8def", ansi5: "#8b7cf6", ansi2: "#3fae6a", ansi1: "#d9649a" };

// The where() of a listed row: the checkout its git was read in.
const rowKey = (r) => JSON.stringify([r.w.machine ?? null, r.at]);
// " on lab" after an agent of another machine's, nothing for this computer's.
const onMachine = (machine) => (machine ? ` on ${machine}` : "");

// Why `git pull --ff-only` (result `r`) pulled nothing, in a line, for the
// checkout whose git was `g`.
function pullProblem(r, g) {
  const lines = `${r.stderr ?? ""}\n${r.stdout ?? ""}`.split("\n").map((l) => l.trim()).filter(Boolean);
  const said = (l) => l.replace(/^(fatal|error|ssh|hint): */i, "");
  const branch = g.branch ?? "this branch";
  const upstream = g.upstream ?? "its upstream";
  const remote = g.upstream ? g.upstream.split("/")[0] : "the remote";
  // The files git lists after its line, one per line, up to its advice.
  const files = (at) => {
    const out = [];
    for (const l of lines.slice(at + 1)) {
      if (/^(Please|Aborting|Merge with strategy|error:|fatal:|hint:)/.test(l)) break;
      out.push(l);
    }
    return out.length > 3 ? `${out.slice(0, 3).join(", ")} and ${out.length - 3} more` : out.join(", ");
  };
  const local = lines.findIndex((l) => /local changes to the following files would be overwritten/i.test(l));
  if (local !== -1) return `Local changes would be overwritten: ${files(local)}`;
  const untracked = lines.findIndex((l) => /untracked working tree files would be (overwritten|removed)/i.test(l));
  if (untracked !== -1) return `Untracked files would be overwritten: ${files(untracked)}`;
  // A pull that would have asked for a login, refused in batch mode.
  if (lines.some((l) => /terminal prompts disabled|could not read (username|password)|authentication failed|permission denied \(|host key verification failed/i.test(l))) {
    return "Needs a login: run git pull in a terminal once";
  }
  const away = lines.find((l) => /could not read from remote repository|unable to access|could not resolve|connection (refused|timed out|closed)|operation timed out/i.test(l));
  if (away) return `Couldn't reach ${remote}: ${oneLine(said(away))}`;
  if (lines.some((l) => /not possible to fast-forward|diverging branches|have diverged/i.test(l))) {
    return `Can't fast-forward: ${branch} and ${upstream} have diverged`;
  }
  return oneLine(said(lines.find((l) => /^(fatal|error):/i.test(l)) ?? lines[0] ?? "")) || `git pull exited with ${r.code}`;
}
const clean = (s) => (s ?? "").replace(/^[^\p{L}\p{N}]+/u, "").trim();

// The agent's icon from the app's program table: a one-color glyph or a picture.
function icon(p, size, color) {
  if (!p?.icon) return null;
  return p.tint ? svg(p.icon).size(size).flex_none().text_color(color) : image(p.icon).w(size).h(size).flex_none();
}

// A space's state in the rail, where a badge has no room: a spinner while an
// agent in it works, a ! when one needs you.
function stateMark(t, status) {
  if (status === "working") return spinner(t).mx(1);
  return div().size(14).flex_none().rounded(4).bg(t.blocked + "26").text_size(10.5).font_bold().text_color(t.blocked).flex().items_center().justify_center().child("!");
}

// The narrowest sidebar whose herd card shows the plans' reset times.
const RESETS_FIT = 330;

// A plan window's use in color: green, then yellow from 60%, red from 85%.
const usageColor = (t, pct) => (pct >= 85 ? t.ansi1 : pct >= 60 ? t.ansi3 : t.ansi2);
// How long until a window resets: `2h 05m`, `3d`.
function countdown(resetsAt, now) {
  if (!resetsAt) return "—";
  const left = Math.max(0, resetsAt - now);
  if (!left) return "now";
  const m = Math.floor(left / 60);
  if (m < 60) return m ? `${m}m` : "<1m";
  const h = Math.floor(m / 60);
  if (h < 24) return `${h}h ${String(m % 60).padStart(2, "0")}m`;
  return `${Math.floor(h / 24)}d ${String(h % 24).padStart(2, "0")}h`;
}
// How old a reading is, in its largest whole unit.
function readingAge(elapsed) {
  const m = Math.floor(Math.max(0, elapsed) / 60);
  return m < 1 ? "<1m" : m < 60 ? `${m}m` : m < 1440 ? `${Math.floor(m / 60)}h` : `${Math.floor(m / 1440)}d`;
}

// What the field says with a corgi too old to answer.
const OLD_CORGI = "This corgi is older than the Projects list. Update Corgi: run herdr-app/install.sh from its repository again.";

// The Open project field's keys, each kept whole when the line wraps.
const KEYS = ["Enter opens", "Tab completes", "Esc closes"];

// Corgi's word for an agent's state as one of the app's, or null.
const STATUS = { working: "working", blocked: "blocked", done: "done", idle: "idle" };

// A build as `corgi status --json` names one ({ version, commit, dirty }),
// in a few letters: its commit, else its release.
const buildName = (b) => [b.commit ? b.commit.slice(0, 7) : `Corgi ${b.version}`, ...(b.dirty ? ["modified"] : [])].join(" · ");
const sameBuild = (a, b) => a.version === b.version && (a.commit ?? null) === (b.commit ?? null) && Boolean(a.dirty) === Boolean(b.dirty);

// What an agent with no task of Corgi's is called: what it titled its
// terminal, when that says something, else its tab's name, else its program.
function ownTitle(row, tab) {
  const prog = row.program?.name;
  // A shell's title is its folder or user@host: that says nothing of the
  // work; nor does another machine's untitled terminal's, its address.
  const raw = (row.title ?? "").trim();
  const live = clean(raw);
  const telling = live && live !== prog && raw !== row.address && !/^[~/]|^[A-Za-z]:\\/.test(raw) && !live.includes("@") && live.toLowerCase() !== row.program?.key;
  if (telling) return oneLine(live);
  return tab?.named ? tab.name : prog || "agent";
}

export default class Projects {
  // What Corgi says, from the store every copy shares (data.js).
  get corgi() { return store.corgi; }
  get seen() { return store.seen; }
  get remotes() { return store.remotes; }
  get corgiAgents() { return store.agents; }
  get corgiProjects() { return store.projects; }
  get paused() { return store.paused; }
  get build() { return store.build; }
  get daemon() { return store.daemon; }
  get usage() { return store.usage; }
  get machines() { return store.machines; }

  init(_props, cx) {
    this.git = {}; // where() -> { name, branch, upstream, ahead, behind, repo, linked }, null: not a repo
    // Pulls from a ↓N, by where(): { phase: "running" }, or "failed" with
    // the `message` its card shows. One runs at a time: worktrees of a repo
    // share its remote branches, which two fetches at once would both lock.
    this.pulls = {};
    this.pressedPull = null; // the where() whose ↓N the pointer went down on
    this.checked = {}; // where() -> when it was last looked at
    this.unsure = new Set(); // where()s git couldn't answer for (a machine away)
    this.pending = new Set(); // where()s to look at next
    this.busy = {}; // pane -> an agent working or a program in front of its shell
    this.ready = {}; // machine -> its layout was readable at the last look
    this.at = {}; // space -> its where() at the last look
    this.watching = new Set(); // local repos' git folders
    // Spaces made while this runs wait (at most a moment) for their git, so a
    // worktree appears under its repo instead of jumping there.
    this.shown = new Set(spaces().map((s) => s.id));
    this.born = {};
    this.pressed = null;
    this.over = null;
    this.reconnecting = JSON.parse(localStorage.getItem("machines.reconnecting") ?? "{}");
    for (const state of Object.values(this.reconnecting)) state.busy = false;
    this.folded = new Set(JSON.parse(localStorage.getItem("machines.folded") ?? "[]"));
    // The projects folded to their supervisor, by their repository (a
    // project's row's `g.repo`), kept across restarts as the machines are.
    this.foldedProjects = new Set(JSON.parse(localStorage.getItem("projects.folded") ?? "[]"));

    // The store's version this copy drew last, and how many answers each
    // machine's Corgi had given then (see `sync`).
    this.version = -1;
    this.answers = new Map(store.answers);
    // This copy's turns at reading for all (data.js, `Turns`).
    this.turns = {};
    // The one merge under way, shown under its worker's row: { terminal,
    // name, dir, branch, base, phase, outcome, problem, closed }. Its phases:
    // confirm, running, result, asking (the supervisor), asked; once merged
    // and pushed, offer (close the worker?), closing, dirty (refused: the
    // worktree has changes) and closed (`closed`: a close that did not go
    // through), shown under the project once the worker's row is gone. A
    // close that went through puts the card away.
    this.merge = null;
    // A pause or resume under way, { verb, phase: "running", count }, or
    // one that left agents behind, { verb, phase: "failed", failed, message }.
    this.pausing = null;
    // The offer's Close, which holds the keyboard so that Enter takes it.
    this.closeFocus = cx.focus_handle();

    // Open project: the field, what it suggests, and what Enter is doing.
    this.open = false;
    this.query = "";
    this.suggestions = []; // [{ dir, name, badge, new, supervisor }]
    this.pick = 0;
    this.asking = false; // a suggestions read is running
    this.askAgain = false;
    this.starting = null; // the name of the project being opened
    this.problem = null; // why the last Enter opened nothing
    this.input = InputState.new({ placeholder: "Project name or path…" });
    this.input.on("change", (_e, cx) => {
      this.query = this.input.value();
      this.pick = 0;
      this.problem = null;
      this.suggest(cx);
      cx.notify();
    });
    this.input.on("submit", (_e, cx) => this.submit(cx));
    this.handle = cx.focus_handle();

    store.refreshMachines(cx, this.turns);
    cx.timer.every(MACHINES_MS, (cx) => store.refreshMachines(cx, this.turns));
    // `git status` walks the whole tree, on another machine a process there:
    // only when its answer could have changed (see `due`).
    cx.timer.every(250, (cx) => this.checkNew(cx));
    // Every copy polls; the store reads for all of them at once.
    store.poll(cx, this.turns);
    cx.timer.every(POLL_MS, (cx) => {
      store.poll(cx, this.turns);
      store.refreshRemotes(cx);
    });
    cx.timer.every(USAGE_POLL_MS, (cx) => store.refreshUsage(cx, this.turns));
    // Live agents changing is a sign Corgi's status did too; comparing is
    // cheap. Another machine's is read again only once its time has come.
    // A read another copy began shows here on this tick.
    cx.timer.every(1000, (cx) => {
      if (store.agentsChanged()) {
        this.refreshCorgi(cx);
        store.refreshRemotes(cx);
      }
      if (this.sync()) cx.notify();
      this.publishQuestions();
    });
    // `corgi.open`, from a key or `herdr-app do`.
    cx.spawn(async (cx) => {
      for (;;) {
        let command;
        try {
          command = await nextCommand();
        } catch (_) {
          return;
        }
        if (command === "open") this.plus(cx);
      }
    });
  }

  // --- Corgi -------------------------------------------------------------------

  // Reads this computer's Corgi again, now.
  refreshCorgi(cx) {
    store.refresh(cx);
  }

  // Whether the store changed since this copy last drew, and what follows
  // each machine's new answer here: a merge card with nothing left to stand
  // for goes.
  sync() {
    if (this.version === store.version) return false;
    this.version = store.version;
    for (const [machine, n] of store.answers) {
      if (this.answers.get(machine) === n) continue;
      this.answers.set(machine, n);
      this.dropGoneMerge(machine);
    }
    return true;
  }

  // A question about a worker that is gone, or no longer tagged, and a
  // result with no row left to stand under, are put away once its
  // machine's Corgi answers; not a close, whose worker is meant to go.
  dropGoneMerge(machine) {
    const m = this.merge;
    if (!m || (m.machine ?? null) !== machine || ["running", "asking", "closing", "closed"].includes(m.phase)) return;
    const worker = this.corgiAgents.get(m.terminal);
    if (!worker || (m.phase === "confirm" && worker.tag !== "merge")) this.merge = null;
  }

  // The project an agent of Corgi's is in, from its machine's status.
  projectOf(machine, dir) {
    return store.projectOf(machine, dir);
  }

  refreshRemote(machine, cx, now = false) {
    store.refreshRemote(machine, cx, now);
  }

  refreshOn(machine, cx) {
    store.refreshOn(machine, cx);
  }

  // --- Merge --------------------------------------------------------------------

  // A click on a worker's MERGE mark: asks under its row, or puts the
  // question away again. One merge at a time: nothing while one runs, or
  // while the merged worker closes.
  askMerge(cx, a) {
    const busy = ["running", "asking", "closing"].includes(this.merge?.phase);
    if (busy) return;
    if (this.merge?.terminal === a.address && this.merge.phase === "confirm") this.merge = null;
    else {
      const c = a.corgi;
      const machine = c.machine ?? null;
      this.merge = { terminal: a.address, machine, name: c.name, dir: c.project, branch: c.branch ?? c.name, base: this.projectOf(machine, c.project)?.base ?? "the base branch", phase: "confirm" };
    }
    cx.notify();
  }

  corgiJson(command, args, what, timeout = MERGE_TIMEOUT_MS, machine = null) {
    return store.corgiJson(command, args, what, timeout, machine);
  }

  corgiMerge(m, args, what) {
    return this.corgiJson("merge", args, what, MERGE_TIMEOUT_MS, m.machine ?? null);
  }

  // The confirmed merge (`corgi merge`), with its checks.
  // `extra`: --pull, or --skip-upstream, after a result that offered it.
  runMerge(cx, extra) {
    const m = this.merge;
    if (!m || (m.phase !== "confirm" && m.phase !== "result")) return;
    this.merge = { ...m, phase: "running", outcome: null, problem: null };
    cx.notify();
    cx.spawn(async (cx) => {
      const outcome = await this.corgiMerge(m, [m.name, "--project", m.dir, "--json", ...extra], "The merge");
      // Merged and pushed: the worker is done, so its close is offered.
      // Not after conflicts, a refusal or a failed push.
      const offer = outcome.result === "merged";
      this.merge = { ...m, phase: offer ? "offer" : "result", outcome, problem: null, closed: null };
      cx.notify();
      if (offer) {
        this.closeFocus.focus();
        // The question is drawn after this: focus Close again once it is there.
        cx.timer.after(120, () => this.merge?.phase === "offer" && this.closeFocus.focus());
      }
      this.refreshOn(m.machine ?? null, cx);
    });
  }

  // Closes the merged worker and removes its worktree (`corgi close`; the
  // branch stays). `force`: after a refusal
  // for changes in the worktree, discarding them.
  closeWorker(cx, force) {
    const m = this.merge;
    if (m?.phase !== "offer" && m?.phase !== "dirty") return;
    this.merge = { ...m, phase: "closing", closed: null };
    cx.notify();
    cx.spawn(async (cx) => {
      const closed = await this.corgiJson("close", [m.name, "--project", m.dir, "--json", ...(force ? ["--force"] : [])], "The close", MERGE_TIMEOUT_MS, m.machine ?? null);
      // Closed: that ends it, and the card goes with the worker's row.
      // Only a refusal or a failure is left for the person.
      const dirty = closed.result === "refused" && closed.reason === "dirty";
      this.merge = closed.result === "closed" ? null : { ...m, phase: dirty ? "dirty" : "closed", closed };
      cx.notify();
      this.refreshOn(m.machine ?? null, cx);
    });
  }

  // Keep: the worker and its worktree stay, and that ends it: the card
  // goes, with nothing to dismiss.
  keepWorker(cx) {
    const m = this.merge;
    if (m?.phase !== "offer" && m?.phase !== "dirty") return;
    this.merge = null;
    cx.notify();
  }

  // Whether the project's supervisor runs, to be asked to fix a merge: an
  // inbox line would wait for one that does not.
  supervised(machine, dir) {
    const supervisor = this.projectOf(machine, dir)?.supervisor;
    return Boolean(supervisor) && supervisor.state !== "exited";
  }

  // Hands what failed the merge to the project's supervisor: a line in its
  // inbox, which Corgi's daemon types in.
  askSupervisor(cx) {
    const m = this.merge;
    const o = m?.outcome;
    if (m?.phase !== "result" || !o?.error) return;
    this.merge = { ...m, phase: "asking", problem: null };
    cx.notify();
    cx.spawn(async (cx) => {
      // A push that failed left the branch merged, at this commit.
      const landed = o.result === "push-failed" && o.commit ? ["--commit", o.commit] : [];
      const answer = await this.corgiMerge(m, [m.name, "--project", m.dir, "--json", "--ask-supervisor", o.error, ...landed], "Corgi");
      if (answer.result === "asked") this.merge = { ...m, phase: "asked", answer };
      // Not asked: the result stays, with why, and the button to try again.
      else this.merge = { ...m, phase: "result", problem: oneLine(answer.message) || "The supervisor could not be asked." };
      cx.notify();
      this.refreshOn(m.machine ?? null, cx);
    });
  }

  // The MERGE mark of a worker tagged ready to merge: its row's way to the
  // merge, and its badge (`badge`'s look), in place of its state's at the
  // right of the row's second line. It keeps its width.
  mergeMark(a, t) {
    const busy = ["running", "asking", "closing"].includes(this.merge?.phase);
    const base = this.projectOf(a.corgi.machine ?? null, a.corgi.project)?.base ?? "the base branch";
    return div()
      .id(`merge-${a.address}`)
      .flex_none()
      .px(5)
      .rounded(4)
      .bg(t.accent)
      .text_color(t.accent_foreground)
      .text_size(10)
      .font_semibold()
      .cursor_pointer()
      .when(busy, (el) => el.opacity(0.5))
      .hover((s) => s.opacity(0.88))
      .tooltip(`Merge ${a.corgi.branch ?? a.corgi.name} into ${base} and push`)
      // The mark's click is its own, not also the row's.
      .on_click((_e, cx) => {
        cx.stop_propagation();
        this.askMerge(cx, a);
      })
      .child("MERGE");
  }

  // Under the worker's row: the question, the merge running, how it went.
  mergePanel(m, t) {
    const o = m.outcome;
    const button = (id, label, solid, onClick) =>
      div()
        .id(`${id}-${m.terminal}`)
        .px(8)
        .h(22)
        .flex()
        .flex_none()
        .items_center()
        .rounded(5)
        .bg(solid ? t.solid : t.raised)
        .text_color(solid ? t.solid_foreground : t.foreground)
        .cursor_pointer()
        .hover((s) => s.opacity(0.88))
        .on_click((_e, cx) => {
          cx.stop_propagation();
          onClick(cx);
        })
        .child(label);
    const dismiss = button("merge-dismiss", "Dismiss", false, (cx) => {
      this.merge = null;
      cx.notify();
    });
    const waiting = (text) =>
      h_flex().gap(7).items_center().text_color(t.muted)
        .child(spinner(t))
        .child(div().flex_1().min_w(0).truncate().child(text));
    // Green for what went well, yellow for a merge that landed but was not
    // pushed, blue for a conflict Corgi handed to the supervisor (not
    // merged, but nothing is left to the person), the color of what needs
    // the person otherwise. Not the states' colors: a theme's `working` is
    // amber, which would read as a warning.
    const handed = o?.result === "conflicts" && Boolean(o.supervisor);
    const color = m.phase === "confirm" || m.phase === "running" ? t.border
      : m.phase === "asked" || o.result === "merged" ? t.ansi2
      : handed ? t.ansi4
      : o.result === "push-failed" ? t.ansi3 : t.blocked;
    // The close after it: a worktree with changes, or a close that did not
    // go through, needs the person.
    const c = m.closed;
    const border = m.phase === "dirty" || m.phase === "closed" ? t.blocked : color;
    const panel = v_flex()
      .id(`merge-panel-${m.terminal}`)
      .flex_none()
      .ml(22)
      .mb(4)
      .p(7)
      .gap(5)
      .rounded(7)
      .border_1()
      .border_color(border)
      .bg(t.surface)
      .text_size(11.5)
      .text_color(t.foreground);
    const buttons = (...list) => h_flex().flex_wrap().gap(5).children(list.filter(Boolean));
    if (m.phase === "confirm") {
      return panel
        .child(div().whitespace_normal().child(`Merge ${m.branch} into ${m.base} and push?`))
        .child(buttons(
          button("merge-cancel", "Cancel", false, (cx) => {
            this.merge = null;
            cx.notify();
          }),
          button("merge-confirm", "Merge + push", true, (cx) => this.runMerge(cx, [])),
        ));
    }
    if (m.phase === "running") return panel.child(waiting("Merging…"));
    if (m.phase === "asked") {
      return panel
        .child(div().whitespace_normal().child(oneLine(m.answer.message) || "Asked the supervisor to fix it."))
        .child(buttons(dismiss));
    }
    const title = div().text_color(color).font_semibold().whitespace_normal().child(handed ? `${m.name}: conflicts, handed to ${o.supervisor}` : `${m.name}: ${o.result}`);
    const said = div().whitespace_normal().child(oneLine(o.message));
    const keep = button("close-keep", "Keep", false, (cx) => this.keepWorker(cx));
    if (m.phase === "offer") {
      // Close is the default: it holds the keyboard, so Enter takes it.
      const close = button("close-confirm", "Close", true, (cx) => this.closeWorker(cx, false))
        .track_focus(this.closeFocus)
        .border_1()
        .border_color(t.solid)
        .focus((s) => s.border_color(t.foreground))
        .on_key_down((e, cx) => {
          if (e.key === "enter") this.closeWorker(cx, false);
          else if (e.key === "escape") this.keepWorker(cx);
          else return;
          cx.stop_propagation();
        });
      return panel
        .child(title)
        .child(said)
        .child(div().whitespace_normal().child(`Close ${m.name} and remove its worktree?`))
        .child(buttons(keep, close));
    }
    if (m.phase === "closing") return panel.child(title).child(said).child(waiting("Closing…"));
    if (m.phase === "dirty") {
      return panel
        .child(title)
        .child(said)
        .child(div().text_color(t.blocked).whitespace_normal().child(tildePaths(oneLine(c.message))))
        .child(div().whitespace_normal().child(`Close ${m.name} anyway and discard them?`))
        .child(buttons(keep, button("close-force", "Close anyway", true, (cx) => this.closeWorker(cx, true))));
    }
    if (m.phase === "closed") {
      return panel
        .child(title)
        .child(said)
        .child(div().text_color(t.blocked).whitespace_normal().child(tildePaths(oneLine(c.message)) || `The close ${c.result}.`))
        .child(buttons(dismiss));
    }
    const retry = o.reason === "behind" ? ["Pull, merge + push", ["--pull"]] : o.reason === "fetch-failed" ? ["Merge + push without fetching", ["--skip-upstream"]] : null;
    // What failed, for a supervisor that runs to fix. A conflict went to it
    // already: `corgi merge` aborts one and hands it over by itself.
    const askable = Boolean(o.error) && this.supervised(m.machine ?? null, m.dir);
    return panel
      .child(title)
      .child(said)
      .when(Boolean(m.problem), (el) => el.child(div().text_color(t.blocked).whitespace_normal().child(m.problem)))
      .when(m.phase === "asking", (el) => el.child(waiting("Asking the supervisor…")))
      .when(m.phase === "result", (el) =>
        el.child(buttons(
          retry && button("merge-retry", retry[0], true, (cx) => this.runMerge(cx, retry[1])),
          askable && button("merge-ask", "Ask supervisor to fix", !retry, (cx) => this.askSupervisor(cx)),
          dismiss,
        )),
      );
  }

  // --- Pause and resume ----------------------------------------------------------

  // How many of Corgi's agents work now, on `machine` (null: this
  // computer; left out: on every machine): as the window sees them, else as
  // Corgi's status last said.
  workingCount(machine) {
    const live = new Map(agents().map((a) => [a.address, a.status]));
    let n = 0;
    for (const [address, c] of this.corgiAgents) {
      if (machine !== undefined && (c.machine ?? null) !== machine) continue;
      if ((live.get(address) ?? c.state) === "working" && !c.paused) n++;
    }
    return n;
  }

  seenMachines() {
    return store.seenMachines();
  }

  // The button: resumes while any agent is paused, on each machine that has
  // one, else pauses the working ones at once, on each machine where one
  // works whose Corgi knows a pause. With none working nothing changes: a
  // toast says so.
  pauseAgents(cx) {
    if (this.pausing?.phase === "running") return;
    if (this.paused?.any) {
      const on = this.seenMachines().filter((m) => this.seen.get(m).paused?.any);
      this.runPause(cx, "resume", this.paused.agents?.length ?? 0, on);
      return;
    }
    const on = this.seenMachines().filter((m) => this.seen.get(m).paused && this.workingCount(m) > 0);
    const working = on.reduce((n, m) => n + this.workingCount(m), 0);
    if (!working) {
      this.nothingWorking();
      return;
    }
    this.runPause(cx, "pause", working, on);
  }

  nothingWorking() {
    notify({ title: "No agents are working", body: "Nothing was paused.", level: "info", timeout: NOTE_MS });
  }

  // Runs `corgi pause` or `corgi resume` on each machine of `on` (null:
  // this computer) at once, with `count` agents in its progress line; a
  // card lists any agent it left behind, with the machine it is on.
  runPause(cx, verb, count, on) {
    this.pausing = { verb, phase: "running", count };
    cx.notify();
    cx.spawn(async (cx) => {
      const what = verb === "pause" ? "The pause" : "The resume";
      const results = await Promise.all(on.map(async (m) => [m, await this.corgiJson(verb, ["--json"], what, PAUSE_TIMEOUT_MS, m)]));
      const failed = results.flatMap(([m, r]) => [...(r.failed ?? []), ...(r.left ?? [])].map((f) => (m ? { ...f, machine: m } : f)));
      const refused = results.filter(([, r]) => r.result === "refused");
      // Of several machines, one that refused is a line of the card's.
      const several = results.length > 1;
      if (several) for (const [m, r] of refused) failed.push({ name: `Agents ${m ? `on ${m}` : "on this computer"}`, why: r.message ?? "" });
      if (verb === "pause" && !failed.length && results.every(([, r]) => r.result === "nothing")) {
        this.pausing = null;
        this.nothingWorking();
      } else if (failed.length || refused.length) {
        this.pausing = { verb, phase: "failed", failed, message: several ? "" : oneLine(results[0][1].message) };
      } else this.pausing = null;
      // The button follows at once; the status read after confirms it.
      store.pauseDone(verb, results);
      cx.notify();
      for (const m of on) this.refreshOn(m, cx);
    });
  }

  // The header's button: "⏸ Agents", or "▶ Agents" while agents are paused.
  pauseButton(t) {
    const any = Boolean(this.paused?.any);
    const busy = this.pausing?.phase === "running";
    const n = this.paused?.agents?.length ?? 0;
    const tip = any
      ? `Resume the ${n === 1 ? "paused agent" : `${n} paused agents`} on ${n === 1 ? "its" : "their"} own conversation`
      : "Pause the agents that are working, before you go offline. Each ends its turn and exits, its conversation saved; nothing is typed into a supervisor until you resume.";
    // Drawn as the + beside it: its height, radius, hover and colours, the
    // header's text; paused is told apart by the icon alone (▶, not ⏸).
    return div()
      .id("agents-pause")
      .h(26)
      .px(6)
      .gap(4)
      .flex()
      .flex_none()
      .items_center()
      .rounded(6)
      .cursor_pointer()
      .hover((s) => s.bg(t.hover).text_color(t.foreground))
      .when(busy, (el) => el.opacity(0.5))
      .tooltip(tip)
      .on_click((_e, cx) => this.pauseAgents(cx))
      .child(svg(any ? PLAY_ICON : PAUSE_ICON).size(14).flex_none())
      .child("Agents");
  }

  // Under the header: the pause running, or the agents it left behind.
  pausePanel(t) {
    const p = this.pausing;
    if (!p) return null;
    const box = v_flex().id("agents-pause-panel").flex_none().mx(8).mb(6).p(7).gap(5).rounded(7).border_1().bg(t.surface).text_size(11.5).text_color(t.foreground);
    if (p.phase === "running") {
      const what = `${p.verb === "pause" ? "Pausing" : "Resuming"} ${p.count === 1 ? "1 agent" : `${p.count} agents`}…`;
      return box.border_color(t.border).child(
        h_flex().gap(7).items_center().text_color(t.muted)
          .child(spinner(t))
          .child(div().flex_1().min_w(0).truncate().child(what)),
      );
    }
    const title = p.verb === "pause" ? "Not paused:" : "Did not come back:";
    return box.border_color(t.blocked)
      .child(div().text_color(t.blocked).font_semibold().whitespace_normal().child(p.failed.length ? title : p.verb === "pause" ? "Nothing was paused" : "Nothing was resumed"))
      .when(!p.failed.length && Boolean(p.message), (el) => el.child(div().whitespace_normal().child(tildePaths(p.message))))
      .children(p.failed.map((f, i) => div().id(`agents-pause-failed-${i}`).whitespace_normal().child(`${f.name}${onMachine(f.machine)}: ${tildePaths(oneLine(f.why))}`)))
      .child(h_flex().gap(5).child(
        div().id("agents-pause-dismiss").px(8).h(22).flex().flex_none().items_center().rounded(5).bg(t.raised).cursor_pointer()
          .hover((s) => s.opacity(0.88))
          .on_click((_e, cx) => {
            this.pausing = null;
            cx.notify();
          })
          .child("Dismiss"),
      ));
  }

  // --- The herd and plan usage ---------------------------------------------------

  // At the top of the list, above its header, one card of a line each, in the theme's
  // border and dim alone, whatever the agents' states: HERD, the agents by
  // state as the rows' badges with a count (a state with none left out,
  // wrapping whole badges onto more lines beside the label), then a
  // line per harness with a plan: its 5-hour and 7-day windows, each a thin
  // bar, the use in color and the time to its reset. The
  // plan and how old the reading is are the line's tooltip. Nothing while
  // there are neither agents nor plans.
  herdCard(by, t) {
    const n = stateCounts(Object.values(by).flat());
    const providers = this.usage?.providers ?? [];
    const now = Date.now() / 1000;
    const states = STATES.filter(([k]) => n[k] > 0);
    if (!states.length && !providers.length) return null;
    const label = (text) => div().w(50).flex_none().text_size(10).font_semibold().text_color(t.dim).child(text);
    // At least a line high, and taller as the badges wrap.
    const herd = h_flex().id("herd").min_h(20).py(2).gap(8).items_center()
      .child(label("HERD"))
      .when(!states.length, (el) => el.child(div().text_color(t.dim).child("no agents")))
      .when(states.length > 0, (el) => el.child(
        h_flex().id("herd-badges").flex_1().min_w(0).flex_wrap().gap_x(4).gap_y(3).items_center()
          .children(states.map(([k, word]) => badge(t, k, `${BADGES[k]} ${n[k]}`).id(`herd-${k}`).tooltip(`${n[k]} ${word}`))),
      ));
    const BAR = 34;
    // The reset times need a sidebar of about 330 px; narrower they go whole,
    // rather than leave a lone `…`, and stay in the line's tooltip.
    const roomy = sidebarPx() >= RESETS_FIT;
    const win = (name, w) =>
      h_flex().flex_1().min_w(0).gap(4).items_center()
        .child(div().flex_none().text_color(t.dim).child(name))
        .child(div().w(BAR).h(4).flex_none().rounded(2).bg(t.border)
          .child(div().h(4).rounded(2).w(w ? Math.max(2, Math.round((BAR * Math.min(100, w.used_percent)) / 100)) : 0).bg(w ? usageColor(t, w.used_percent) : t.dim)))
        .child(div().flex_none().font_semibold().text_color(w ? usageColor(t, w.used_percent) : t.dim).child(w ? `${w.used_percent}%` : "—"))
        .when(roomy, (el) => el.child(div().min_w(0).truncate().text_size(10).text_color(t.dim).child(w ? countdown(w.resets_at, now) : "")));
    const plan = (p) => {
      const u = p.usage;
      const read = u && p.fetched_at ? `read ${readingAge(now - p.fetched_at)} ago` : null;
      const resets = u && !roomy ? [["5h", u.five_hour], ["7d", u.week]].filter(([, w]) => w).map(([name, w]) => `${name} resets in ${countdown(w.resets_at, now)}`) : [];
      const tip = [u ? [u.plan_type || null, read].filter(Boolean).join(" · ") : null, ...resets, p.error ? oneLine(p.error) : null].filter(Boolean).join(" · ");
      return h_flex().id(`usage-${p.provider}`).h(20).gap(8).items_center()
        .when(Boolean(tip), (el) => el.tooltip(tip))
        .child(label(p.title))
        .when(!u, (el) => el.child(div().text_color(t.dim).child(p.error ? "unavailable" : "loading…")))
        .when(Boolean(u), (el) => el.child(win("5h", u.five_hour)).child(win("7d", u.week)));
    };
    return div().flex_none().px(8).pt(4).pb(2).child(
      v_flex().id("herd-card").px(8).py(4).rounded(7).border_1().border_color(t.border).bg(t.surface).text_size(11)
        .child(herd).children(providers.map(plan)));
  }

  // --- Which Corgi runs --------------------------------------------------------

  // Under the list: the corgi that answers, and what is not as it should be:
  // its checkout has commits it lacks (a branch merged since it was built),
  // with what to run there; a daemon that does not run, or runs another
  // build, with what starts it again. Nothing before Corgi answered; a
  // corgi too old to say what it is still says whether its daemon runs.
  versionLine(t) {
    const b = this.build;
    const d = this.daemon;
    // Without the daemon no supervisor on the app is woken or handed over.
    const stopped = d && !d.running ? `Corgi daemon ${d.installed ? "installed but not running" : "not running"}: supervisors won't be woken or handed over` : null;
    if (!b && !stopped) return null;
    const h = home();
    const tilde = (dir) => (dir === h ? "~" : dir.startsWith(`${h}/`) ? `~${dir.slice(h.length)}` : dir);
    const what = b && [`Corgi ${b.version}`, ...(b.commit ? [b.commit.slice(0, 7)] : []), ...(b.dirty ? ["modified"] : [])].join(" · ");
    const newer = b?.newer ? `${b.newer.branch ?? "its checkout"} is newer` : null;
    const other = b && d?.running && !(d.build && sameBuild(d.build, b)) ? `The daemon runs ${d.build ? `another build: ${buildName(d.build)}` : "an older build"}` : null;
    const command = (text) => div().whitespace_normal().font_family("JetBrains Mono").text_size(10).text_color(t.muted).child(text);
    return v_flex()
      .id("corgi-version")
      .flex_none()
      .px(16)
      .pt(4)
      .pb(8)
      .gap(2)
      .text_size(10.5)
      .text_color(t.dim)
      .when(Boolean(b), (el) =>
        el.child(
          h_flex().gap(4).items_center()
            .child(div().flex_shrink(1).min_w(0).truncate().child(what))
            .when(Boolean(newer), (el) => el.child(div().flex_none().text_color(t.ansi3).child(`· ${newer}`))),
        ),
      )
      .when(Boolean(newer), (el) => el.child(command(`${b.source ? `cd ${tilde(b.source)} && ` : ""}${INSTALL_COMMAND}`)))
      // Either starts with the same command: it installs the service again,
      // which starts the daemon on the corgi that answers.
      .when(Boolean(stopped ?? other), (el) => el.child(div().text_color(t.blocked).whitespace_normal().child(stopped ?? other)).child(command(DAEMON_COMMAND)));
  }

  // --- Open project ------------------------------------------------------------

  // The +: opens the field, or puts it away. The rail has no room for it:
  // there it unfolds the sidebar first.
  plus(cx) {
    if (narrow()) {
      toggleSidebar();
      if (this.open) {
        cx.notify();
        return;
      }
    }
    this.toggleOpen(cx);
  }

  toggleOpen(cx) {
    this.open = !this.open;
    this.problem = null;
    if (this.open) {
      this.query = "";
      this.pick = 0;
      this.input.set_value("");
      this.input.focus();
      // The field is drawn after this click: focus it again once it is there.
      cx.timer.after(120, () => this.open && this.input.focus());
      this.suggest(cx);
    }
    cx.notify();
  }

  // The folders the list's spaces on this computer are in, for Corgi to
  // offer as open projects and the directories beside them.
  openFolders() {
    const ps = panes();
    return [...new Set(spaces().filter((s) => !s.machine).map((s) => place(s, ps)).filter(Boolean))];
  }

  // Reads the suggestions for what is typed: one read at a time, the newest
  // text last.
  suggest(cx) {
    if (this.asking) {
      this.askAgain = true;
      return;
    }
    this.asking = true;
    cx.spawn(async (cx) => {
      const query = this.query;
      let rows = null;
      let failure = null;
      if (!this.corgi) await store.status();
      if (!this.corgi) failure = NO_CORGI;
      else {
        try {
          const args = ["projects", "--json", "--query", query, ...this.openFolders().flatMap((dir) => ["--open", dir])];
          const r = await exec(this.corgi, args, { timeout: 20000 });
          if (r.code === 0) rows = JSON.parse(r.stdout).projects ?? [];
          else failure = /unknown command/.test(r.stderr) ? OLD_CORGI : oneLine(r.stderr) || `corgi projects exited with ${r.code}`;
        } catch (e) {
          failure = `corgi projects did not answer: ${e}`;
        }
      }
      this.asking = false;
      // Text typed meanwhile has a read of its own coming: keep what shows.
      if (query === this.query) {
        if (rows) this.suggestions = rows;
        else this.problem = failure;
        this.pick = Math.min(this.pick, Math.max(0, this.suggestions.length - 1));
      }
      cx.notify();
      if (this.askAgain) {
        this.askAgain = false;
        this.suggest(cx);
      }
    });
  }

  // Tab: the highlighted directory with a / after it, so its own
  // directories come next.
  complete(cx) {
    const row = this.suggestions[this.pick];
    if (!row) return;
    const h = home();
    const typed = row.dir.startsWith(`${h}/`) && this.query.startsWith("~") ? `~${row.dir.slice(h.length)}` : row.dir;
    this.input.set_value(`${typed}/`);
    this.query = this.input.value();
    this.pick = 0;
    this.suggest(cx);
    cx.notify();
  }

  // Shell terminals a supervisor could start in, most likely first: those
  // of the focused space, then every other one already in `dir`. Corgi takes
  // the first whose shell waits at its prompt in the project's primary
  // checkout, and none otherwise.
  shells(dir) {
    const here = focused()?.space;
    const spaceOf = {};
    for (const s of spaces()) if (!s.machine) for (const tab of s.tabs ?? []) spaceOf[tab.id] = s.id;
    const idle = panes().filter((p) => p.terminal && spaceOf[p.tab] && p.program?.shell && !p.status);
    const mine = idle.filter((p) => spaceOf[p.tab] === here);
    const others = idle.filter((p) => spaceOf[p.tab] !== here && p.cwd === dir);
    return [...mine, ...others].map((p) => p.terminal);
  }

  submit(cx) {
    if (this.starting) return;
    const row = this.suggestions[this.pick];
    if (!row) {
      this.problem = this.query.trim() ? "Nothing to open by that name: type a project's name or a folder's path." : "Type a project's name or a folder's path.";
      cx.notify();
      return;
    }
    // Its supervisor runs, and the app shows it: there at once.
    if (row.supervisor && panes().some((p) => p.terminal === row.supervisor)) {
      this.done(cx, row.supervisor);
      return;
    }
    if (!this.corgi) {
      this.problem = NO_CORGI;
      cx.notify();
      return;
    }
    this.starting = row.name;
    this.problem = null;
    cx.notify();
    cx.spawn(async (cx) => {
      let opened;
      try {
        const args = ["open", row.dir, "--json", ...(row.new ? ["--create"] : []), ...this.shells(row.dir).flatMap((t) => ["--terminal", t])];
        const r = await exec(this.corgi, args, { timeout: OPEN_TIMEOUT_MS });
        try {
          opened = JSON.parse(r.stdout);
        } catch (_) {
          opened = { result: "failed", message: oneLine(r.stderr) || `corgi open exited with ${r.code}` };
        }
      } catch (e) {
        opened = { result: "failed", message: `corgi open did not answer: ${e}` };
      }
      this.starting = null;
      if (opened.terminal && (opened.result === "started" || opened.result === "running")) {
        this.done(cx, opened.terminal);
        this.refreshCorgi(cx);
      } else {
        // The field stays open with what was typed.
        this.problem = oneLine(opened.message) || "Corgi could not open the project.";
        cx.notify();
      }
    });
  }

  // Shows the supervisor's terminal and puts the field away.
  done(cx, terminal) {
    this.open = false;
    this.problem = null;
    run("agent.focus", { terminal });
    cx.notify();
  }

  openPanel(t) {
    const rows = this.suggestions;
    const first = Math.max(0, Math.min(this.pick - Math.floor(SUGGESTIONS / 2), rows.length - SUGGESTIONS));
    const shown = rows.slice(first, first + SUGGESTIONS);
    const h = home();
    const tilde = (dir) => (dir === h ? "~" : dir.startsWith(`${h}/`) ? `~${dir.slice(h.length)}` : dir);
    const suggestion = (row, n) => {
      const on = first + n === this.pick;
      return h_flex()
        .id(`open-${first + n}`)
        .h(26)
        .flex_none()
        .px(8)
        .gap(6)
        .items_center()
        .rounded(6)
        .cursor_pointer()
        .when(on, (el) => el.bg(t.selected))
        .when(!on, (el) => el.hover((s) => s.bg(t.hover)))
        .on_click((_e, cx) => {
          this.pick = first + n;
          this.submit(cx);
        })
        .child(div().flex_none().max_w(130).truncate().text_color(t.foreground).child(row.name))
        .child(div().flex_1().min_w(0).truncate().text_size(11).text_color(t.dim).child(tilde(row.dir)))
        .child(div().flex_none().text_size(10.5).text_color(row.new ? t.accent : row.supervisor ? t.working : t.dim).child(row.supervisor ? "running" : row.badge));
    };
    return v_flex()
      .id("open-project")
      .track_focus(this.handle)
      .flex_none()
      .mx(8)
      .mb(8)
      .p(6)
      .gap(4)
      .rounded(9)
      .border_1()
      .border_color(t.border)
      .bg(t.surface)
      .on_key_down((e, cx) => {
        const k = e.key;
        if (k === "escape") this.open = false;
        else if (k === "down") this.pick = rows.length ? (this.pick + 1) % rows.length : 0;
        else if (k === "up") this.pick = rows.length ? (this.pick + rows.length - 1) % rows.length : 0;
        else if (k === "tab") this.complete(cx);
        else return;
        cx.stop_propagation();
        cx.notify();
      })
      .child(Input.new(this.input).h(30).px(8).rounded(7).border_1().border_color(t.accent).bg(t.background).text_size(12.8).text_color(t.foreground))
      .children(shown.map(suggestion))
      .when(rows.length > first + SUGGESTIONS, (el) => el.child(div().px(8).text_size(11).text_color(t.dim).child(`${rows.length - first - SUGGESTIONS} more: keep typing`)))
      .when(rows.length === 0 && !this.problem && !this.asking, (el) => el.child(div().px(8).py(3).text_size(11.5).text_color(t.dim).child(this.query.trim() ? "No project or folder by that name." : "No projects yet: type a folder's path, or a name for a new one.")))
      .when(Boolean(this.starting), (el) =>
        el.child(
          h_flex().px(8).py(3).gap(7).items_center().text_size(11.5).text_color(t.muted)
            .child(spinner(t))
            .child(div().flex_1().min_w(0).truncate().child(`Starting ${this.starting}'s supervisor…`)),
        ),
      )
      .when(Boolean(this.problem), (el) => el.child(div().px(8).py(3).text_size(11.5).text_color(t.blocked).whitespace_normal().child(this.problem)))
      .child(
        // The keys wrap whole, a key with what it does, rather than be cut:
        // at the sidebar's own width the last goes to a second line.
        h_flex().px(8).pt(2).gap(6).items_start().text_size(10.5).text_color(t.dim)
          .child(
            h_flex().id("open-keys").flex_1().min_w(0).flex_wrap().gap_x(8)
              .children(KEYS.map((k) => div().flex_none().whitespace_nowrap().child(k))),
          )
          .child(
            div().id("open-plain-space").flex_none().px(5).rounded(4).cursor_pointer().hover((s) => s.bg(t.hover).text_color(t.foreground))
              .tooltip("A space with a shell and no project")
              .on_click((_e, cx) => {
                this.open = false;
                run("space.new");
                cx.notify();
              })
              .child("New space"),
          ),
      );
  }

  // --- spaces and their git ------------------------------------------------------

  checkNew(cx) {
    const now = Date.now();
    const sp = spaces();
    for (const s of sp) if (!this.shown.has(s.id)) this.born[s.id] ??= now;
    const due = this.due(now, sp, panes());
    if (due.size) this.refreshGit(due, cx);
    if (sp.some((s) => !this.shown.has(s.id)) || this.orderingDue(now)) cx.notify();
  }

  // Places whose git could have changed: new ones (a new space, a cd), where a
  // command or an agent's turn just ended (a checkout, a commit, edits), those on
  // a machine that is back, and repos in view whose fallback is due.
  due(now, sp, ps) {
    // Moved there: what it knew of that place may be old.
    const at = {};
    for (const s of sp) at[s.id] = where(s, ps);
    const keys = new Set(Object.values(at).filter((k) => JSON.parse(k)[1]));
    const due = new Set([...keys].filter((k) => !(k in this.git)));
    for (const [id, k] of Object.entries(at)) if (this.at[id] && this.at[id] !== k && keys.has(k)) due.add(k);
    this.at = at;
    const spaceOf = {};
    for (const s of sp) for (const t of s.tabs ?? []) spaceOf[t.id] = s;
    const busy = {};
    for (const p of ps) {
      if (!p.terminal || !spaceOf[p.tab]) continue;
      busy[p.pane] = p.status === "working" || !(p.program?.shell || p.program?.agent);
      if (this.busy[p.pane] && !busy[p.pane]) this.sameRepo(at[spaceOf[p.tab].id], keys).forEach((k) => due.add(k));
    }
    this.busy = busy;
    for (const m of machines()) {
      const ready = m.layout === "ready";
      if (ready && this.ready[m.name] === false) for (const k of keys) if (JSON.parse(k)[0] === m.name) due.add(k);
      this.ready[m.name] = ready;
    }
    if (window().active) {
      for (const k of keys) {
        const asks = this.git[k] || this.unsure.has(k);
        if (asks && !this.folded.has(JSON.parse(k)[0]) && now - (this.checked[k] ?? 0) > FALLBACK) due.add(k);
      }
    }
    return due;
  }

  // `key` and the other places in its repo (a commit there moves their ahead/behind).
  sameRepo(key, keys) {
    const repo = this.git[key]?.repo;
    return [key, ...(repo ? [...keys].filter((k) => this.git[k]?.repo === repo) : [])];
  }

  // The machines, read again now for every copy: after a reconnect.
  refreshMachines(cx) {
    store.refreshMachines(cx);
  }

  rebuilt(machine) {
    return machine?.state === "unreachable" && /^a different core answers there \("[^"]+"\)$/.test(machine.why ?? "");
  }

  reconnect(machine, cx) {
    const name = machine.name;
    if (this.reconnecting[name]?.busy) return;
    const state = (this.reconnecting[name] ??= { machine, removed: false });
    state.busy = true;
    state.error = null;
    localStorage.setItem("machines.reconnecting", JSON.stringify(this.reconnecting));
    cx.notify();
    cx.spawn(async (cx) => {
      try {
        const ask = async (args) => {
          const r = await hc([...args, "--json"]);
          if (r.code !== 0) {
            let error;
            try {
              error = JSON.parse(r.stdout || r.stderr).error?.message;
            } catch {}
            throw new Error(error || r.stderr || r.stdout || "Couldn't connect.");
          }
        };
        if (!state.removed) {
          await ask(["machine", "rm", name]);
          state.removed = true;
          localStorage.setItem("machines.reconnecting", JSON.stringify(this.reconnecting));
        }
        const args = ["machine", "add"];
        if (machine.via) args.push("--via", machine.target);
        else args.push(machine.target);
        args.push("--name", name);
        if (machine.session) args.push("--session", machine.session);
        if (machine.temporary) args.push("--temp");
        await ask(args);
        this.folded.delete(name);
        localStorage.setItem("machines.folded", JSON.stringify([...this.folded]));
        delete this.reconnecting[name];
      } catch (e) {
        state.error = String(e.message ?? e);
        state.busy = false;
      }
      localStorage.setItem("machines.reconnecting", JSON.stringify(this.reconnecting));
      this.refreshMachines(cx);
      cx.notify();
    });
  }

  // One look at a time, each place once however often it was asked for meanwhile.
  refreshGit(keys, cx) {
    for (const k of keys) this.pending.add(k);
    if (this.refreshing) return;
    this.refreshing = true;
    cx.spawn(async (cx) => {
      while (this.pending.size) {
        const keys = [...this.pending];
        this.pending.clear();
        for (const key of keys) await this.check(key, cx);
      }
      const used = new Set(Object.values(this.git).filter(Boolean).map((g) => JSON.parse(g.repo)).filter(([on]) => !on).map(([, repo]) => repo));
      for (const repo of this.watching) if (!used.has(repo)) this.watching.delete(repo);
      this.refreshing = false;
      cx.notify();
    });
  }

  // One place's git: branch, ahead/behind, its repo.
  async check(key, cx) {
    const [on, cwd] = JSON.parse(key);
    this.checked[key] = Date.now();
    this.unsure.delete(key);
    try {
      // No optional locks: a status that touched the index would wake the watch again.
      // Writes nothing under .git (index lock, fsmonitor cookie): watch() would wake on it.
      const r = await exec("git", ["--no-optional-locks", "-c", "core.fsmonitor=false", "status", "--porcelain=v2", "--branch"], { cwd, on });
      if (r.code !== 0) {
        // Only git's own "no" is remembered as no repo; anything else is asked again.
        if (!/not a git repository/i.test(r.stderr)) this.unsure.add(key);
        this.git[key] = null;
        return;
      }
      // The repo's own .git folder names it; a worktree's top differs from it.
      const top = await exec("git", ["rev-parse", "--show-toplevel"], { cwd, on });
      const common = await exec("git", ["rev-parse", "--path-format=absolute", "--git-common-dir"], { cwd, on });
      const repo = common.stdout.trim();
      const name = repo.endsWith("/.git") ? base(repo.slice(0, -5)) : base(repo).replace(/\.git$/, "");
      const info = { name, branch: null, upstream: null, ahead: 0, behind: 0, repo: JSON.stringify([on, repo]), linked: repo !== `${top.stdout.trim()}/.git` };
      for (const line of r.stdout.split("\n")) {
        if (line.startsWith("# branch.head ")) info.branch = line.slice(14);
        else if (line.startsWith("# branch.upstream ")) info.upstream = line.slice(18);
        else if (line.startsWith("# branch.ab ")) {
          const [a, b] = line.slice(12).split(" ");
          info.ahead = Math.abs(parseInt(a, 10)) || 0;
          info.behind = Math.abs(parseInt(b, 10)) || 0;
        }
      }
      this.git[key] = info;
      // Brought up to date some other way: a failed pull's card has nothing left to say.
      if (!info.behind && this.pulls[key]?.phase === "failed") delete this.pulls[key];
      if (!on) this.watch(repo, cx);
    } catch {
      // Couldn't ask (the machine is away): keep what it showed, look again later.
      this.git[key] ??= null;
      this.unsure.add(key);
    }
  }

  watch(repo, cx) {
    if (this.watching.has(repo)) return;
    this.watching.add(repo);
    cx.spawn(async (cx) => {
      try {
        while (this.watching.has(repo)) {
          await watchFolder(repo);
          const mine = JSON.stringify([null, repo]);
          if (this.watching.has(repo)) this.refreshGit(Object.keys(this.git).filter((k) => this.git[k]?.repo === mine), cx);
        }
      } catch {
        this.watching.delete(repo);
      }
    });
  }

  menu(w) {
    // A popup is as tall as its items; height is only the most.
    openOverlay({ mod: "menu", kind: "popup", width: 200, height: 260, props: { tab: "space", target: { id: w.id } } });
  }

  orderingDue(now) {
    return this.ordering && now - this.ordering.at >= 1000
      && this.ordering.machines.every((name) => machines().some((m) => m.name === name && m.layout === "ready"));
  }

  // Spaces in order, each worktree space moved under its main checkout's.
  // `sp`, `ps`: spaces() and panes(), read once by the caller.
  ordered(sp, ps) {
    const ready = (w) => this.shown.has(w.id) || where(w, ps) in this.git || Date.now() - (this.born[w.id] ??= Date.now()) > 1500;
    for (const w of sp) if (!this.shown.has(w.id) && ready(w)) this.shown.add(w.id);
    const list = sp.map((w, i) => ({ w, i, g: this.git[where(w, ps)] ?? null, at: place(w, ps) }));
    const mains = new Set(list.filter((r) => r.g && !r.g.linked).map((r) => r.g.repo));
    const nested = (r) => r.g?.linked && mains.has(r.g.repo);
    const out = [];
    const placed = new Set(); // repos whose worktrees are listed (under the first of its spaces)
    for (const r of list) {
      if (nested(r)) continue;
      out.push({ ...r, tree: false });
      if (!r.g || r.g.linked || placed.has(r.g.repo)) continue;
      placed.add(r.g.repo);
      for (const k of list.filter((k) => nested(k) && k.g.repo === r.g.repo)) out.push({ ...k, tree: true });
    }
    // Simulate the batch so later indexes include earlier moves. An attempted
    // write can lose a sync conflict: allow the echo to settle before retrying.
    const order = sp.map((w) => w.id);
    const target = out.map((r) => r.w.id);
    const key = JSON.stringify([order, target, machines()]);
    if (order.every((id, i) => id === target[i])) {
      this.ordering = null;
    } else if (this.ordering?.key !== key || this.orderingDue(Date.now())) {
      const moved = new Set();
      this.ordering = { key, at: Date.now(), machines: [] };
      for (let i = 0; i < out.length; i++) {
        const from = order.indexOf(out[i].w.id);
        if (from === i) continue;
        run("space.move", { id: out[i].w.id, index: i + 1 });
        moved.add(out[i].w.machine ?? null);
        order.splice(i, 0, order.splice(from, 1)[0]);
      }
      this.ordering.machines = [...moved];
    }
    return out.filter((r) => this.shown.has(r.w.id));
  }

  // Every agent the list shows, by the space it is in: the app's agents, and
  // those of Corgi's the app does not count as agents yet (one that has
  // reported no status, a supervisor's shell after its agent exited), found
  // by the pane their terminal is in.
  agentsBySpace(sp, ps) {
    const spaceOf = {};
    const tabs = {};
    for (const s of sp) for (const tab of s.tabs ?? []) { spaceOf[tab.id] = s.id; tabs[tab.id] = tab; }
    const rows = agents().map((a) => ({ address: a.address, space: a.space, tab: a.tab, status: a.status, program: a.program, title: a.title, focused: a.focused }));
    const listed = new Set(rows.map((r) => r.address));
    for (const [address, c] of this.corgiAgents) {
      if (listed.has(address)) continue;
      const p = ps.find((p) => p.terminal === address);
      if (p && spaceOf[p.tab]) {
        rows.push({ address, space: spaceOf[p.tab], tab: p.tab, status: null, program: p.program, title: p.title, focused: p.focused });
        continue;
      }
      // A paused worker's terminal ended with its agent: its row stands
      // under its project's space until it is resumed.
      const home = c.paused && sp.find((s) => (s.machine ?? null) === (c.machine ?? null) && place(s, ps) === c.project);
      if (home) rows.push({ address, space: home.id, tab: null, status: null, program: null, title: "", focused: false });
    }
    const by = {};
    for (const row of rows) {
      const corgi = this.corgiAgents.get(row.address) ?? null;
      (by[row.space] ??= []).push({
        ...row,
        corgi,
        status: corgi?.paused ? "paused" : row.status ?? STATUS[corgi?.state] ?? null,
        supervisor: corgi?.role === "supervisor",
        task: corgi ? (corgi.role === "supervisor" ? "Supervisor" : oneLine(corgi.task) || corgi.name) : ownTitle(row, tabs[row.tab]),
      });
    }
    return by;
  }

  // What the list shows for `rows`, the spaces of one machine in order: each
  // space outside a worktree, then the agents of it and of its worktrees'
  // spaces, the supervisor first, then the worktrees' spaces with no agent,
  // and some room before the next. A project folded (a click on its row,
  // `foldProject`) shows its supervisor alone: its other agents, its
  // worktrees' rows and their cards are not drawn, and agent.next and
  // agent.prev pass them by.
  listed(rows, by, t) {
    const out = [];
    const order = [];
    for (let n = 0; n < rows.length; n++) {
      const head = rows[n];
      const kids = [];
      while (rows[n + 1]?.tree) kids.push(rows[++n]);
      const members = [head, ...kids].flatMap((r) => (by[r.w.id] ?? []).map((a) => ({ a, r })));
      // The supervisor first; the rest stay in the app's order.
      members.sort((x, y) => Number(y.a.supervisor) - Number(x.a.supervisor));
      // A project (a repository's space) folds; a space outside any does not.
      const folded = Boolean(head.g) && this.foldedProjects.has(head.g.repo);
      const shown = folded ? members.filter(({ a }) => a.supervisor) : members;
      const fold = head.g && {
        folded,
        // Something to fold away: a chevron says which way it is.
        more: folded || kids.length > 0 || members.some(({ a }) => !a.supervisor),
        counts: stateCounts(members.map(({ a }) => a)),
      };
      // Only the agent with the focus is lit, never a project's row with it:
      // a project's row (a repository's space) is not lit at all, a click on
      // it folds it; a space outside any repository is lit while none of its
      // agents has the focus.
      out.push(this.row(head, t, !head.g && head.w.active && !(by[head.w.id] ?? []).some((a) => a.focused), fold));
      const card = this.pullCard(rowKey(head), t);
      if (card) out.push(card);
      let panel = false;
      for (const [i, { a, r }] of shown.entries()) {
        // A worktree's space has no row of its own while agents stand for it:
        // shown, its first agent is lit unless another of its agents has the focus.
        const mine = by[r.w.id] ?? [];
        const lit = a.focused || (r.tree && r.w.active && mine[0] === a && !mine.some((x) => x.focused));
        out.push(this.agent(a, r, lit, t, i === 0));
        if (this.merge?.terminal === a.address) {
          out.push(this.mergePanel(this.merge, t));
          panel = true;
        }
        order.push(a.address);
      }
      // A closing worker's row may go with its space before the close
      // answers: the card, and a close that did not go through, stand under
      // its project's agents instead.
      const m = this.merge;
      const closing = m?.phase === "closing" || m?.phase === "closed";
      const here = (head.w.machine ?? null) === (m?.machine ?? null);
      if (!folded && !panel && closing && here && (head.at === m.dir || members.some(({ a }) => a.corgi?.project === m.dir))) out.push(this.mergePanel(m, t));
      for (const k of folded ? [] : kids) {
        if ((by[k.w.id] ?? []).length) continue;
        out.push(this.row(k, t));
        const card = this.pullCard(rowKey(k), t);
        if (card) out.push(card);
      }
      // Room after a group of agents, before the next project.
      if (shown.length) out.push(div().flex_none().h(8));
    }
    return { out, order };
  }

  // Folds the project whose repository is `repo` to its supervisor, or
  // unfolds it, and remembers which are folded.
  foldProject(repo, cx) {
    if (this.foldedProjects.has(repo)) this.foldedProjects.delete(repo);
    else this.foldedProjects.add(repo);
    localStorage.setItem("projects.folded", JSON.stringify([...this.foldedProjects]));
    cx.notify();
  }

  // The rows, under a header per machine once one isn't this computer.
  grouped(t, collapsed, sp, ps) {
    const rows = this.ordered(sp, ps);
    const by = collapsed ? {} : this.agentsBySpace(sp, ps);
    this.current = this.currentOf(rows, by);
    const order = [];
    const draw = (mine) => {
      if (collapsed) return mine.map((r) => this.compact(r, t));
      const listed = this.listed(mine, by, t);
      order.push(...listed.order);
      return listed.out;
    };
    const done = (els) => {
      // agent.next and agent.prev follow the list.
      if (!collapsed) setAgentOrder(order);
      return els;
    };
    const remote = [...new Set([...rows.map((r) => r.w.machine).filter(Boolean), ...this.machines.map((m) => m.name), ...Object.keys(this.reconnecting)])].sort();
    const loading = (m) => {
      const layout = machines().find((k) => k.name === m)?.layout;
      return layout === "loading" || (!layout && this.machines.some((k) => k.name === m && k.state === "connected"))
        || sp.some((s) => (s.machine ?? null) === m && !this.shown.has(s.id));
    };
    const loadingRow = (m) => h_flex().id(`machine-loading-${m ?? "local"}`).h(28).flex_none().px(collapsed ? 0 : 8).items_center().when(collapsed, (el) => el.justify_center()).text_size(12).text_color(t.dim).child(collapsed ? "…" : "Loading…");
    if (!remote.length) return done(loading(null) && !rows.length ? [loadingRow(null)] : draw(rows));
    const machineNames = [null, ...remote];
    return done(machineNames.flatMap((m) => {
      const mine = rows.filter((r) => (r.w.machine ?? null) === m);
      const repair = this.reconnecting[m];
      const machine = this.machines.find((k) => k.name === m) ?? repair?.machine;
      const connected = !m || machine?.state === "connected";
      const rebuilt = this.rebuilt(machine) || !!repair;
      const folded = this.folded.has(m);
      const icon = svg(`mods/workspace-chrome/${m ? "server" : "monitor"}.svg`).size(13).flex_none();
      const badge = div().absolute().right(-3).bottom(-2).size(6).rounded(3).border_1().border_color(t.chrome).bg(connected ? t.ansi2 : t.dim);
      const head = h_flex().id(`machine-${m ?? "local"}`).h(26).flex_none().px(collapsed ? 0 : 8).gap(7).items_center().text_size(11).text_color(t.dim)
        .when(collapsed, (el) => el.mt(4).w(RAIL_CELL).justify_center().rounded(6).tooltip(m ? `${m} · ${connected ? "connected" : (machine?.state ?? "asleep").replace(/_/g, " ")}` : "This computer"))
        .cursor_pointer().hover((s) => s.bg(t.hover))
        .on_click((_e, cx) => {
          folded ? this.folded.delete(m) : this.folded.add(m);
          localStorage.setItem("machines.folded", JSON.stringify([...this.folded]));
          cx.notify();
        })
        .when(!collapsed, (el) => el.child(icon))
        .when(collapsed, (el) => el.child(div().relative().flex_none().child(icon).when(Boolean(m), (el) => el.child(badge))))
        .when(!collapsed, (el) => el.child(div().flex_1().truncate().child(m ?? "This computer"))
          .when(!connected && !rebuilt, (el) => el.child(div().flex_none().child((machine?.state ?? "asleep").replace(/_/g, " "))))
          .when(rebuilt, (el) => el.child(h_flex().flex_none().gap(3).items_center()
            .child(div().max_w(140).truncate().child(repair?.error || "rebuilt ·"))
            .child(div().id(`machine-connect-${m}`).text_color(repair?.error ? t.blocked : t.muted)
              .hover((s) => s.text_color(t.foreground))
              .tooltip(repair?.error || "This machine was rebuilt or replaced. Click to connect to the new one.")
              .on_click((_e, cx) => { cx.stop_propagation(); this.reconnect(machine, cx); })
              .child(repair?.busy ? "Connecting…" : "Connect"))))
          .when(connected, (el) => el.child(div().id(`machine-add-${m ?? "local"}`).flex_none().px(4).rounded(4).hover((s) => s.bg(t.hover).text_color(t.foreground))
            .tooltip(`New space on ${m ?? "this computer"}`)
            .on_click((_e, cx) => { cx.stop_propagation(); run("space.new", { machine: m ?? "" }); }).child("+")))
          .child(div().size(6).rounded(3).flex_none().bg(connected ? t.ansi2 : t.dim))
          .child(svg(`mods/workspace-chrome/chevron-${folded ? "right" : "down"}.svg`).size(11).flex_none()));
      const empty = h_flex().id(`machine-new-${m ?? "local"}`).h(28).flex_none().px(collapsed ? 0 : 8).gap(7).items_center().rounded(7)
        .text_size(12).text_color(t.dim).hover((s) => s.bg(t.hover).text_color(t.foreground))
        .on_click(() => run("space.new", { machine: m ?? "" }))
        .when(collapsed, (el) => el.w(RAIL_CELL).rounded(6).justify_center().tooltip(`New space on ${m ?? "this computer"}`)
          .child(h_flex().size(18).rounded(5).border_1().border_color(t.dim).items_center().justify_center()
            .child(svg("mods/workspace-chrome/plus.svg").size(10))))
        .when(!collapsed, (el) => el.child(svg("mods/workspace-chrome/plus.svg").size(12)).child("New space there"));
      if (!collapsed && rebuilt) return [head];
      const cell = collapsed ? railCell : (el) => el;
      // Its rows show without Corgi's data: said once, under the header.
      const noCorgi = !collapsed && m && connected && this.remotes[m]?.missing
        ? [div().id(`machine-no-corgi-${m}`).flex_none().px(8).pb(4).text_size(10.5).text_color(t.dim).truncate().child(`Corgi isn't installed on ${m}`)]
        : [];
      return [cell(head), ...noCorgi, ...(folded ? [] : mine.length ? draw(mine) : [loading(m) ? loadingRow(m) : cell(empty)])];
    }));
  }

  // Only what's unusual, after the name: "herdr · spike ↑2 ↓1". The repo or
  // branch goes when the name already says it (a worktree is named after its branch),
  // and master/main go: nearly every space is on one. A project's agents
  // by state (`counts`, its swatches) stand before the arrows.
  gitTail(g, tree, name, t, key, counts = null) {
    const raw = g.branch ?? "detached";
    const same = (a) => a.toLowerCase() === name.toLowerCase();
    const branch = same(raw) || raw === "master" || raw === "main" ? "" : raw;
    const repo = tree || same(g.name) ? "" : g.name;
    const where = [repo, branch].filter(Boolean).join(" · ");
    return h_flex()
      .flex_1()
      .gap(5)
      .justify_end()
      .items_center()
      .text_size(11.5)
      .text_color(t.dim)
      .when(Boolean(where), (el) => el.child(div().min_w(0).truncate().child(where)))
      .when(Boolean(counts), (el) => el.child(counts))
      .when(g.ahead > 0, (el) => el.child(div().flex_none().text_color(t.ansi2).child(`↑${g.ahead}`)))
      .when(g.behind > 0, (el) => el.child(this.pullMark(g, key, t)));
  }

  // --- Pull -------------------------------------------------------------------------

  // The red ↓N of a checkout behind its upstream: a click pulls. It takes
  // the press and the release itself, so the row under it neither shows its
  // space nor starts a drag. While it pulls: dimmed, with a spinner.
  pullMark(g, key, t) {
    const running = this.pulls[key]?.phase === "running";
    const other = !running && this.pulling();
    const n = g.behind;
    return h_flex()
      .id(`pull-${key}`)
      .flex_none()
      .gap(3)
      .px(3)
      .rounded(4)
      .items_center()
      .text_color(t.ansi1)
      .when(running || other, (el) => el.opacity(0.5))
      .when(!running && !other, (el) => el.cursor_pointer().hover((s) => s.bg(t.ansi1 + "26")))
      .tooltip(running ? "Pulling…" : other ? "Another pull is running" : `Pull ${n} commit${n === 1 ? "" : "s"} from ${g.upstream ?? "its upstream"}`)
      .on_mouse_down("left", (_e, cx) => {
        cx.stop_propagation();
        this.pressedPull = key;
      })
      .on_mouse_up("left", (_e, cx) => {
        // A drag let go here is the row's.
        if (this.pressedPull !== key) return;
        cx.stop_propagation();
        this.pressedPull = null;
        this.pull(key, cx);
      })
      .when(running, (el) => el.child(spinner(t, 10)))
      .child(`↓${n}`);
  }

  pulling() {
    return Object.values(this.pulls).some((p) => p.phase === "running");
  }

  // `git pull --ff-only` in the checkout `key` names (PULL_SCRIPT), and
  // nothing else: no merge, rebase, stash or reset. Done, its git is read again and the ↓N
  // goes with nothing more; failed, a card says why.
  pull(key, cx) {
    const g = this.git[key];
    if (!g?.behind || this.pulling()) return;
    const [on, cwd] = JSON.parse(key);
    this.pulls[key] = { phase: "running" };
    cx.notify();
    cx.spawn(async (cx) => {
      let r;
      try {
        r = await exec("sh", ["-c", PULL_SCRIPT], { cwd, on, timeout: PULL_TIMEOUT_MS });
      } catch (e) {
        r = { code: -1, stdout: "", stderr: `git pull did not finish: ${e?.message ?? e}` };
      }
      const failed = r.code === 0 ? null : { phase: "failed", message: pullProblem(r, g) };
      // The mark stays dimmed until the new count is read.
      await this.check(key, cx);
      if (failed) this.pulls[key] = failed;
      else delete this.pulls[key];
      cx.notify();
    });
  }

  // Under a row whose pull failed: why, and Dismiss.
  pullCard(key, t) {
    const p = this.pulls[key];
    if (p?.phase !== "failed") return null;
    return v_flex()
      .id(`pull-card-${key}`)
      .flex_none()
      .ml(22)
      .mb(4)
      .p(7)
      .gap(5)
      .rounded(7)
      .border_1()
      .border_color(t.blocked)
      .bg(t.surface)
      .text_size(11.5)
      .text_color(t.foreground)
      .child(div().whitespace_normal().child(tildePaths(p.message)))
      .child(h_flex().child(div()
        .id(`pull-dismiss-${key}`)
        .px(8)
        .h(22)
        .flex()
        .flex_none()
        .items_center()
        .rounded(5)
        .bg(t.raised)
        .cursor_pointer()
        .hover((s) => s.opacity(0.88))
        .on_click((_e, cx) => {
          cx.stop_propagation();
          delete this.pulls[key];
          cx.notify();
        })
        .child("Dismiss")));
  }

  // A space: a project's own, one outside any repository, or a worktree's
  // with no agent in it. `on`: lit (by default while it is shown). A
  // click shows the space, but on a project's row (`fold`: whether it is
  // `folded`, has `more` than its supervisor to fold, and its agents'
  // `counts` by state) it folds the project or unfolds it, and the row
  // says how many of its agents are in each state, folded or not.
  row({ w, i, g, tree, at }, t, on = w.active, fold = null) {
    const fromIndex = this.pressed === null ? -1 : spaces().findIndex((s) => s.id === this.pressed);
    const colors = shared("space.colors") ?? {};
    const color = colors[w.id] ?? "ansi6";
    const fill = color.startsWith("#") ? color : (MARK[color] ?? t[color]);
    return v_flex()
      .relative()
      .id(`space-${w.id}`)
      .px(8)
      .h(tree ? TREE_ROW : SPACE_ROW)
      .flex_none()
      .justify_center()
      .rounded(8)
      .when(on, (el) => el.bg(t.selected))
      .when(!on, (el) => el.hover((s) => s.bg(t.hover)))
      .when(this.pressed === w.id && this.over !== w.id, (el) => el.opacity(0.5))
      .when(this.pressed !== null && this.over === w.id && this.pressed !== w.id, (el) => el.child(
        div().absolute().left(0).right(0).h(2).bg(t.accent)
          .when(fromIndex < i, (line) => line.bottom(0))
          .when(fromIndex > i, (line) => line.top(0)),
      ))
      .on_mouse_down("left", () => {
        this.pressed = w.id;
        this.over = w.id;
      })
      .on_mouse_move((_e, cx) => {
        const source = spaces().find((s) => s.id === this.pressed);
        const over = source && (source.machine ?? null) === (w.machine ?? null) ? w.id : null;
        if (this.pressed !== null && this.over !== over) {
          this.over = over;
          cx.notify();
        }
      })
      .on_mouse_up("left", (_e, cx) => {
        const from = this.pressed;
        const over = this.over;
        this.pressed = null;
        this.over = null;
        if (from !== null && over === w.id && from !== w.id) {
          const index = spaces().findIndex((s) => s.id === w.id);
          if (index !== -1) run("space.move", { id: from, index: index + 1 });
        } else if (from === w.id) {
          if (fold) this.foldProject(g.repo, cx);
          else run("space.select", { id: w.id });
        }
        cx.notify();
      })
      .on_mouse_down("right", () => this.menu(w))
      .child(
        h_flex()
          .gap(9)
          .items_center()
          .when(tree, (el) => el.pl(22).child(svg("mods/workspace-chrome/branch.svg").size(13).text_color(t.dim)))
          .when(!tree, (el) => el.child(h_flex().size(18).flex_none().rounded(5).bg(fill).items_center().justify_center().text_size(10.5).font_bold().text_color("#ffffff").child([...w.name][0]?.toUpperCase() ?? "?")))
          // The name gives way first: the counts and arrows after it stay whole.
          .child(h_flex().flex_shrink(1).max_w(150).gap(3).items_center()
            .child(div().min_w(0).truncate().text_color(t.foreground).child(w.name))
            .when(Boolean(fold?.more), (el) => el.child(svg(`mods/workspace-chrome/chevron-${fold.folded ? "right" : "down"}.svg`)
              .id(`fold-${w.id}`).size(10).flex_none().text_color(t.dim))))
          .child(g ? this.gitTail(g, tree, w.name, t, rowKey({ w, at }), fold && this.stateSwatches(w, fold.counts, t)) : div().flex_1()),
      )
      .when(Boolean(fold), (el) => el.cursor_pointer().tooltip(fold.folded ? "Show its agents" : "Fold to its supervisor"));
  }

  // On a project's row, its agents by state: a count in the color of each
  // state's badge, in the HERD card's order, a state with none left out.
  stateSwatches(w, n, t) {
    const states = STATES.filter(([k]) => n[k] > 0);
    if (!states.length) return null;
    // Narrower than a badge, so that the name keeps its room in a narrow sidebar.
    return h_flex().id(`counts-${w.id}`).flex_none().gap(2).items_center()
      .children(states.map(([k, word]) => badge(t, k, String(n[k])).px(3).min_w(13).flex().justify_center()
        .id(`count-${w.id}-${k}`).tooltip(`${n[k]} ${word}`)));
  }

  // One agent, on two lines. The first: its task, the whole width, cut with
  // `…`. The second, dim and smaller: its harness's icon, its model and how
  // full its context is, each only when known, and at the right edge its
  // state's badge (`badge`; for a worker tagged ready to merge, the MERGE
  // mark that opens the merge question). The worktree it works in is the
  // row's tooltip.
  // Each is a faintly shaded card with its state's color as a full-height
  // bar at its left: the card reaches past the bar by its rounding, so its
  // corners round beside the bar and never cut into it. Cards stand 4 px
  // apart (2 under the project's row: `first`); a narrow sidebar takes in
  // the card's margins, so the second line keeps its room.
  // `on`: lit, as the agent with the focus or for its space, which is shown:
  // the selection's fill, with an outline every card has, unseen until lit,
  // so that lighting it moves nothing.
  agent(a, { w, g, at, tree }, on, t, first = false) {
    const worktree = a.corgi?.worktree ? base(a.corgi.worktree) : tree && g?.linked && at ? base(at) : "";
    const pic = icon(a.program, 12, a.supervisor ? t.foreground : t.muted);
    const model = oneLine(a.corgi?.model);
    const ctx = Number.isFinite(a.corgi?.ctx_pct) ? `${Math.round(a.corgi.ctx_pct)}%` : "";
    const key = badgeKey(a);
    // A supervisor's open questions, as a pill at the right of its first line.
    const asked = a.supervisor ? this.openQuestions(a) : 0;
    const tight = sidebarPx() < CARD_ROOMY;
    return v_flex()
      .id(`agent-${a.address}`)
      .relative()
      .ml(tight ? 2 : 10)
      .mt(first ? 2 : 4)
      .pl(CARD_RADIUS + CARD_EDGE + (tight ? 6 : 8))
      .pr(tight ? 6 : 8)
      .h(AGENT_ROW + 2)
      .flex_none()
      .justify_center()
      .rounded(CARD_RADIUS)
      .border_1()
      .border_color(on ? t.muted : t.foreground + "00")
      .bg(on ? t.selected : t.foreground + "0d")
      .text_size(12.5)
      .cursor_pointer()
      .when(Boolean(worktree), (el) => el.tooltip(worktree))
      .when(!on, (el) => el.hover((s) => s.bg(t.foreground + "1a")))
      .on_click(() => run("agent.focus", { terminal: a.address }))
      .on_mouse_down("right", () => this.menu(w))
      .child(h_flex().h(18).items_center()
        .child(div().flex_1().min_w(0).truncate().text_color(a.status === "idle" && !on ? t.muted : t.foreground).when(a.supervisor, (el) => el.font_medium()).child(a.task))
        .when(asked > 0, (el) => el.child(this.questionPill(a, asked, t))))
      .child(h_flex().h(16).gap(5).items_center().text_size(11).text_color(t.dim)
        .when(Boolean(pic), (el) => el.child(pic))
        .when(Boolean(model), (el) => el.child(div().flex_shrink(1).min_w(0).truncate().child(model)))
        .when(Boolean(model && ctx), (el) => el.child(div().flex_none().child("·")))
        .when(Boolean(ctx), (el) => el.child(div().flex_none().child(ctx)))
        .when(Boolean(key), (el) => el.child(div().flex_1()).child(key === "merge" ? this.mergeMark(a, t) : badge(t, key).id(`badge-${a.address}`))))
      // Inside the border, past the corners' rounding: whole from top to bottom.
      .child(div().id(`edge-${a.address}`).absolute().left(CARD_RADIUS - 1).top(0).bottom(0).w(CARD_EDGE).bg(stateColor(t, key ?? "idle")));
  }

  // --- the Questions panel -----------------------------------------------------------

  // How many of the questions of `a`'s project (a supervisor) have no
  // answer from the app yet.
  openQuestions(a) {
    const p = a.corgi?.project && this.projectOf(a.corgi.machine ?? null, a.corgi.project);
    return p?.open_questions ?? 0;
  }

  // The count on a supervisor's row: a click shows the Questions panel on
  // its project, until another space is shown.
  questionPill(a, n, t) {
    const machine = a.corgi.machine ?? null;
    const dir = a.corgi.project;
    const show = (_e, cx) => {
      cx.stop_propagation();
      setShared(PICK, { machine, dir, space: spaces().find((s) => s.active)?.id ?? null });
      this.publishQuestions();
      run("layout.select", { stack: "panel-body", child: QUESTIONS_BLOCK });
      run("layout.show", { id: "panel" });
    };
    return div().id(`questions-${a.address}`).ml(6).px(5).h(16).flex_none().flex().items_center().gap(3).rounded(4)
      .text_size(10.5).font_semibold().bg(t.accent).text_color(t.accent_foreground).cursor_pointer()
      .tooltip(`${n} open question${n === 1 ? "" : "s"}: show ${n === 1 ? "it" : "them"}`)
      .on_mouse_down("left", (_e, cx) => cx.stop_propagation()).on_click(show)
      .child(svg("mods/corgi/question.svg").size(11)).child(String(n));
  }

  // The project the active space is in: the project a Corgi agent in it
  // works for, else the one whose folder holds the space's group's head
  // (a project's own space, with its worktrees' spaces under it).
  currentOf(rows, by) {
    let i = rows.findIndex((r) => r.w.active);
    if (i < 0) return null;
    const active = rows[i].w;
    const machine = active.machine ?? null;
    const agent = (by?.[active.id] ?? []).find((a) => a.corgi?.project);
    if (agent) return { machine, dir: agent.corgi.project };
    while (i > 0 && rows[i].tree) i--;
    const at = rows[i].at;
    if (!at) return null;
    for (const [, p] of store.projects) {
      if (!this.projectOf(machine, p.dir)) continue;
      if (at === p.dir || at.startsWith(`${p.dir}/`)) return { machine, dir: p.dir };
    }
    return null;
  }

  // Tells the Questions panel and the tab bar's button which project is
  // current and how many of its questions are open (shared state, which
  // every mod sees): the project whose count was clicked while the space
  // shown then is, else the active space's.
  publishQuestions() {
    const pick = shared(PICK);
    const active = spaces().find((s) => s.active)?.id ?? null;
    const at = pick && pick.space === active ? pick : this.current;
    const p = at && this.projectOf(at.machine ?? null, at.dir);
    const current = p
      ? { machine: at.machine ?? null, dir: at.dir, name: p.name, open: p.open_questions ?? 0,
          key: JSON.stringify([p.open_questions ?? 0, (p.questions ?? []).map((q) => q.id), p.inbox_pending ?? 0, p.backlog ?? 0]) }
      : null;
    if (JSON.stringify(current) !== JSON.stringify(shared(CURRENT) ?? null)) setShared(CURRENT, current);
    const count = current?.open ?? 0;
    if (shared(COUNT) !== count) setShared(COUNT, count);
  }

  // A space in the rail: its number, and the most urgent state in it.
  compact({ w, i }, t) {
    const mark = w.status === "working" || w.status === "blocked" ? stateMark(t, w.status) : div().text_color(t[w.status] ?? t.unknown).child(w.status === "done" ? "●" : "·");
    return railCell(h_flex()
      .id(`space-${w.id}`)
      .relative()
      .w(RAIL_CELL)
      .h(26)
      .rounded(6)
      .justify_center()
      .items_center()
      .tooltip(w.name)
      .when(w.active, (el) => el.bg(t.selected))
      .when(!w.active, (el) => el.hover((s) => s.bg(t.hover)))
      .on_click(() => run("space.select", { id: w.id }))
      .on_mouse_down("right", () => this.menu(w))
      .child(div().text_color(w.active ? t.foreground : t.muted).child(String(i + 1)))
      .child(div().absolute().right(w.status === "working" || w.status === "blocked" ? 1 : 5).child(mark)));
  }

  render() {
    this.sync();
    const t = theme();
    const collapsed = narrow();
    const version = collapsed ? null : this.versionLine(t);
    const palette = ["ansi6", "ansi3", "ansi4", "ansi5", "ansi2", "ansi1"];
    const sp = spaces();
    const ps = panes();
    const ids = new Set(sp.map((p) => p.id));
    const kept = Object.fromEntries(Object.entries(shared("space.colors") ?? {}).filter(([id]) => ids.has(id)));
    let changed = Object.keys(kept).length !== Object.keys(shared("space.colors") ?? {}).length;
    for (const p of sp) {
      if (kept[p.id] || p.name === "~") continue;
      const uses = (c) => Object.values(kept).filter((v) => v === c).length;
      kept[p.id] = palette.reduce((best, c) => (uses(c) < uses(best) ? c : best));
      changed = true;
    }
    if (changed) setShared("space.colors", kept);
    // The rail has no room for the herd and usage card.
    const by = collapsed ? null : this.agentsBySpace(sp, ps);
    const herd = by && this.herdCard(by, t);
    return v_flex().size_full().min_h(0).bg(t.chrome).text_size(13)
      .when(Boolean(herd), (el) => el.child(herd))
      .child(h_flex().h(31).flex_none().pl(16).pr(10).items_center().justify_between().text_size(11.5).text_color(t.dim)
        .when(collapsed, (el) => el.pl(0).pr(0).justify_center())
        .when(!collapsed, (el) => el.child("Projects"))
        .child(h_flex().flex_none().gap(4).items_center()
          // No button from a corgi that knows no pause, nor in the rail.
          .when(Boolean(this.paused) && !collapsed, (el) => el.child(this.pauseButton(t)))
          .child(div().id("project-open").size(26).flex_none().flex().items_center().justify_center().rounded(6)
            .when(this.open && !collapsed, (el) => el.bg(t.hover).text_color(t.foreground))
            .hover((s) => s.bg(t.hover).text_color(t.foreground))
            .tooltip("Open project")
            .on_click((_e, cx) => this.plus(cx))
            .child(svg("mods/workspace-chrome/plus.svg").size(14)))))
      .when(Boolean(this.pausing) && !collapsed, (el) => el.child(this.pausePanel(t)))
      .when(this.open && !collapsed, (el) => el.child(this.openPanel(t)))
      .child(v_flex().flex_1().min_h(0).px(collapsed ? 0 : 8).pb(14).overflow_y_scrollbar().mode("always")
        .on_mouse_up("left", (_e, cx) => { this.pressed = null; this.pressedPull = null; cx.notify(); })
        .children(this.grouped(t, collapsed, sp, ps)))
      .when(Boolean(version), (el) => el.child(version))
      // Herdr's own layout unfolds the rail from its agents list, which Corgi's has not.
      .when(collapsed, (el) =>
        el.child(
          h_flex().h(28).flex_none().items_center().justify_center()
            .child(div().id("rail-unfold").text_size(13).text_color(t.dim).hover((s) => s.text_color(t.foreground)).tooltip("Show the sidebar").on_click(toggleSidebar).child("»")),
        ),
      );
  }
}
