// Corgi's data, read once for every copy of the mod: what `corgi status
// --json` says here and on each other machine the window shows, plan usage
// (`corgi usage --json`) and the machines (`hc machine ls`).
//
// The app runs a mod's modules once per source version and shares them by
// every copy (each block showing the mod), so the one `store` below is
// every copy's. Each copy asks it on its own timers, and takes turns
// (`Turns`): a copy's tick that finds a read begun on another copy's tick
// since its own last one leaves it at that, so `corgi status` runs once per
// refresh however many copies show. A read
// runs as a task of the copy that asked (`cx`), which is redrawn when it
// ends; the others see `version` change on their next tick. A copy that
// closes mid-read takes its task with it: a read that has run longer than
// any read may (`STUCK_MS`) counts as gone, so another one starts.
import { agents, exec, hc, home, appHome, readTextFile } from "herdr";

// How often Corgi's status is read again besides when the live agents
// change, and never more often.
export const POLL_MS = 5000;
// Another machine's status is read as often, but no sooner than three
// times its last read took: a slow machine is asked less often.
const SLOW_FACTOR = 3;
// A machine without corgi is looked at again this often (it may be installed).
const LOCATE_MS = 60000;
// How often plan usage is read: `corgi usage --json` reads the cache every
// Corgi process shares, which asks the provider at most once a minute.
export const USAGE_POLL_MS = 60000;
// How often the machines are listed.
export const MACHINES_MS = 15000;
// Longer than any read may take (exec's timeouts are 20 and 30 seconds): a
// read still marked running after this was taken away with its copy.
const STUCK_MS = 45000;

// Where corgi is on another machine, run there in `sh` (exec takes no
// login shell): the path that machine's own Corgi install recorded for its
// app (an older install there may have recorded it only for the Projects
// mod Corgi had before its one mod), `corgi` on PATH, on its login shell's
// PATH, then the usual places.
// Prints the first that is there, or exits 3.
const LOCATE_CORGI = [
  'try() { case "$1" in /*) if [ -x "$1" ]; then printf \'%s\\n\' "$1"; exit 0; fi ;; esac; }',
  'app="${HERDR_APP_HOME:-$HOME/.config/herdr-app}"',
  'try "$(sed -n \'s/.*"corgi"[[:space:]]*:[[:space:]]*"\\([^"]*\\)".*/\\1/p\' "$app/mods/corgi/config.json" "$app/mods/projects/config.json" 2>/dev/null | head -n 1)"',
  'try "$(command -v corgi)"',
  'try "$("${SHELL:-/bin/sh}" -lc \'command -v corgi\' </dev/null 2>/dev/null | tail -n 1)"',
  'for c in "$HOME/.local/bin/corgi" "$HOME/.local/share/corgi/target/release/corgi" "$HOME/.cargo/bin/corgi"; do try "$c"; done',
  "exit 3",
].join("\n");

// What the field says with no corgi to ask.
export const NO_CORGI = "The corgi command was not found. Install Corgi: run herdr-app/install.sh from its repository.";

// A Corgi project by its machine (null: this computer) and folder: two
// machines may each have one in the same folder.
export const projectKey = (machine, dir) => JSON.stringify([machine ?? null, dir]);

const oneLine = (text) => (text ?? "").replace(/\s+/g, " ").trim();

// Where corgi may be, in order: what the installer wrote for this mod, then
// the usual places.
async function corgiCandidates() {
  const list = [];
  try {
    const config = JSON.parse((await readTextFile(`${appHome()}/mods/corgi/config.json`)) ?? "{}");
    if (config.corgi) list.push(config.corgi);
  } catch (_) {
    // No config: the mod was added on its own.
  }
  const h = home();
  list.push("corgi", `${h}/.local/bin/corgi`, `${h}/.local/share/corgi/target/release/corgi`, `${h}/.cargo/bin/corgi`);
  return [...new Set(list)];
}

// A read of one thing: running or not, when the last began.
const reading = () => ({ loading: false, started: 0 });
const running = (r) => r.loading && Date.now() - r.started < STUCK_MS;

// Whose tick reads, of the copies whose timers of one period tick out of
// step: a copy reads unless a read was begun on another copy's tick since
// its own last one. So the copies' timers together read once a period
// (one copy's, while the others' find it done), and when that copy goes,
// the next copy's tick reads. A copy keeps its last turn in `mine`.
class Turns {
  constructor(name) {
    this.name = name;
    this.taken = 0;
  }
  take(mine) {
    const last = mine[this.name] ?? -1;
    if (this.taken > 0 && this.taken > last) {
      mine[this.name] = this.taken;
      return false;
    }
    this.taken += 1;
    mine[this.name] = this.taken;
    return true;
  }
}

class CorgiData {
  constructor() {
    // Bumped whenever anything below changes: a copy that saw another
    // number draws again.
    this.version = 0;
    // Corgi's program here, kept until it stops answering.
    this.corgi = null;
    // What each machine's Corgi said last (null: this computer): { agents:
    // terminal -> agent, projects: dir -> project, paused }. A machine whose
    // Corgi did not answer has none: its rows show without Corgi's data.
    this.seen = new Map();
    // How many answers each machine's Corgi gave: a copy does what follows
    // an answer once per answer (`Projects.sync`).
    this.answers = new Map();
    // Corgi on the other machines, by name: { path (null: not found yet),
    // missing, loading, last, took, located }.
    this.remotes = {};
    // Every machine's, together. An agent of another machine's is under
    // its app address (`lab/t3`) and has its `machine`.
    this.agents = new Map(); // address -> { role, task, worktree, state, project, machine, ... }
    this.projects = new Map(); // projectKey(machine, dir) -> { base, supervisor, ... }
    // What `corgi status` says of a pause, every machine's together: { any,
    // agents }, null when no machine's corgi knows one.
    this.paused = null;
    this.build = null; // the corgi that answers: { version, commit, dirty, source, newer }
    this.daemon = null; // { running, pid, build, installed }
    // Plan usage, as `corgi usage --json` gives it: { now, providers }.
    this.usage = null;
    // The other machines, as `hc machine ls` lists them (not the temporary ones).
    this.machines = [];
    this.local = { ...reading(), again: false };
    this.usageRead = reading();
    this.machinesRead = reading();
    this.statusTurns = new Turns("status");
    this.usageTurns = new Turns("usage");
    this.machinesTurns = new Turns("machines");
    this.agentKey = "";
  }

  changed(cx) {
    this.version += 1;
    cx.notify();
  }

  // Whether the live agents changed since any copy last looked: a sign
  // Corgi's status did too. Once per change, for all copies.
  agentsChanged() {
    const key = agents().map((a) => `${a.address}:${a.status}`).join(",");
    if (key === this.agentKey) return false;
    this.agentKey = key;
    return true;
  }

  // --- this computer's Corgi ----------------------------------------------------

  // The first corgi that answers `status --json`, kept until it stops answering.
  async status() {
    const candidates = this.corgi ? [this.corgi] : await corgiCandidates();
    for (const path of candidates) {
      try {
        const r = await exec(path, ["status", "--json"], { timeout: 20000 });
        if (r.code !== 0) continue;
        const status = JSON.parse(r.stdout);
        this.corgi = path;
        return status;
      } catch (_) {
        // Not there, or not a corgi that knows status: the next one.
      }
    }
    this.corgi = null;
    return null;
  }

  // Reads the status now, for what was just done: a read already running
  // is followed by another, since it may have begun before the change.
  refresh(cx) {
    if (running(this.local)) {
      this.local.again = true;
      return;
    }
    this.local.loading = true;
    this.local.started = Date.now();
    cx.spawn(async (cx) => {
      const status = await this.status();
      // With no answer the pause stays as it was last said.
      this.learn(null, status ?? { projects: [], paused: this.seen.get(null)?.paused ?? null });
      this.build = status?.corgi ?? null;
      this.daemon = status?.daemon ?? null;
      if (status) this.answered(null);
      this.local.loading = false;
      // The first answer names the corgi to ask for usage too.
      if (status && !this.usage) this.refreshUsage(cx);
      this.changed(cx);
      if (this.local.again) {
        this.local.again = false;
        this.refresh(cx);
      }
    });
  }

  // A copy's timer (its turns in `mine`): reads when it is its turn.
  poll(cx, mine) {
    if (this.statusTurns.take(mine)) this.refresh(cx);
  }

  answered(machine) {
    this.answers.set(machine, (this.answers.get(machine) ?? 0) + 1);
  }

  // Keeps what `machine`'s Corgi said (`status`), or forgets it (null), and
  // puts every machine's together again.
  learn(machine, status) {
    if (!status) this.seen.delete(machine);
    else {
      const agents = new Map();
      for (const p of status.projects ?? []) {
        for (const a of p.agents ?? []) {
          if (!a.terminal) continue;
          // Corgi there names its own core's terminals; the app, `lab/t3`.
          if (machine) agents.set(`${machine}/${a.terminal}`, { ...a, project: p.dir, machine });
          else agents.set(a.terminal, { ...a, project: p.dir });
        }
      }
      const projects = new Map((status.projects ?? []).map((p) => [p.dir, p]));
      this.seen.set(machine, { agents, projects, paused: status.paused ?? null });
    }
    this.combine();
  }

  // Every machine's agents, projects and pause, together.
  combine() {
    this.agents = new Map();
    this.projects = new Map();
    let paused = null;
    // This computer's first: its agents, then each other machine's.
    for (const m of this.seenMachines()) {
      const s = this.seen.get(m);
      for (const [address, a] of s.agents) this.agents.set(address, a);
      for (const [dir, p] of s.projects) this.projects.set(projectKey(m, dir), p);
      if (s.paused) {
        paused ??= { any: false, agents: [] };
        paused.any ||= Boolean(s.paused.any);
        paused.agents.push(...(s.paused.agents ?? []).map((a) => (m ? { ...a, machine: m } : a)));
      }
    }
    this.paused = paused;
  }

  // The machines whose Corgi answered, this computer (null) first.
  seenMachines() {
    return [...this.seen.keys()].sort((a, b) => (a === null ? -1 : b === null ? 1 : a.localeCompare(b)));
  }

  // The project an agent of Corgi's is in, from its machine's status.
  projectOf(machine, dir) {
    return this.projects.get(projectKey(machine, dir));
  }

  // What a pause or resume (`verb`) did on each machine (`results`, [machine,
  // answer]), until the status read after it confirms it: the button
  // follows at once. A resume lets go of every mark, also of an agent that
  // did not come back.
  pauseDone(verb, results) {
    for (const [m, r] of results) {
      const seen = this.seen.get(m);
      if (!seen) continue;
      const stopped = r.paused ?? [];
      if (verb === "pause" && stopped.length) seen.paused = { any: true, agents: stopped.map((name) => ({ name })) };
      if (verb === "resume" && r.result !== "refused") seen.paused = { any: false, agents: [] };
    }
    this.combine();
    this.version += 1;
  }

  // --- the other machines ---------------------------------------------------------

  // The machines, on a copy's timer when it is its turn (`mine`), or now
  // (no `mine`: a reconnect).
  refreshMachines(cx, mine = null) {
    const r = this.machinesRead;
    if (mine && (running(r) || !this.machinesTurns.take(mine))) return;
    r.loading = true;
    r.started = Date.now();
    cx.spawn(async (cx) => {
      try {
        const res = await hc(["machine", "ls", "--json"]);
        if (res.code === 0) this.machines = (JSON.parse(res.stdout).machines ?? []).filter((m) => !m.temporary);
      } catch {}
      r.loading = false;
      // A machine just connected is read at once; one gone is forgotten.
      this.refreshRemotes(cx);
      this.changed(cx);
    });
  }

  // The other machines whose Corgi is read: those connected now. One that
  // went away or was removed is forgotten, and its rows show without data.
  remoteMachines() {
    const connected = this.machines.filter((m) => m.state === "connected").map((m) => m.name);
    for (const m of [...this.seen.keys()]) if (m !== null && !connected.includes(m)) this.learn(m, null);
    for (const m of Object.keys(this.remotes)) if (!this.machines.some((k) => k.name === m)) delete this.remotes[m];
    return connected;
  }

  refreshRemotes(cx) {
    for (const m of this.remoteMachines()) this.refreshRemote(m, cx);
  }

  // Where corgi is on `machine`: its path, or null when it has none.
  // Rejects when the machine cannot be asked.
  async locate(machine) {
    const r = await exec("sh", ["-c", LOCATE_CORGI], { on: machine, timeout: 20000 });
    const path = r.code === 0 ? r.stdout.trim().split("\n").pop() : "";
    return path.startsWith("/") ? path : null;
  }

  // `corgi status --json` on another machine, through herdr-core: at most
  // every POLL_MS, less often when the machine is slow to answer, and
  // `now` (after a merge, close or pause there) whenever none is running.
  refreshRemote(machine, cx, now = false) {
    const r = (this.remotes[machine] ??= { path: null, missing: false, loading: false, last: 0, took: 0, located: 0 });
    if (r.loading && Date.now() - r.last < STUCK_MS) return;
    const at = Date.now();
    if (!now && at - r.last < Math.max(POLL_MS, r.took * SLOW_FACTOR)) return;
    if (!r.path && r.missing && at - r.located < LOCATE_MS) return;
    r.loading = true;
    r.last = at;
    cx.spawn(async (cx) => {
      let status = null;
      try {
        if (!r.path) {
          r.located = Date.now();
          r.path = await this.locate(machine);
          r.missing = !r.path;
        }
        if (r.path) {
          const res = await exec(r.path, ["status", "--json"], { on: machine, timeout: 20000 });
          // Gone, or too old to know status: looked for again next time.
          if (res.code === 0) status = JSON.parse(res.stdout);
          else r.path = null;
        }
      } catch (_) {
        // Away, corgi gone from there, or an answer that is not JSON: no
        // data until it answers, and corgi is looked for again first.
        r.path = null;
      }
      r.took = Date.now() - at;
      r.loading = false;
      // Gone meanwhile: nothing to keep.
      if (this.machines.some((k) => k.name === machine && k.state === "connected")) {
        this.learn(machine, status);
        if (status) this.answered(machine);
      }
      this.changed(cx);
    });
  }

  // Reads `machine`'s Corgi again after acting there.
  refreshOn(machine, cx) {
    if (machine) this.refreshRemote(machine, cx, true);
    else this.refresh(cx);
  }

  // --- plan usage -------------------------------------------------------------------

  // Plan usage from the corgi that answers status, on a copy's timer when
  // it is its turn (`mine`), or now (no `mine`); a corgi too old to know
  // `usage` leaves the boxes out.
  refreshUsage(cx, mine = null) {
    const r = this.usageRead;
    if (!this.corgi || running(r) || (mine && !this.usageTurns.take(mine))) return;
    r.loading = true;
    r.started = Date.now();
    const path = this.corgi;
    cx.spawn(async (cx) => {
      try {
        const res = await exec(path, ["usage", "--json"], { timeout: 30000 });
        if (res.code === 0) this.usage = JSON.parse(res.stdout);
      } catch (_) {
        // Kept as it was: the next minute asks again.
      }
      r.loading = false;
      this.changed(cx);
    });
  }

  // --- acting -------------------------------------------------------------------------

  // What `corgi COMMAND ARGS` (merge, close, pause, resume) answered on
  // `machine` (null: this computer): its JSON, or a refusal saying why
  // there is none.
  async corgiJson(command, args, what, timeout, machine = null) {
    const path = machine ? this.remotes[machine]?.path : this.corgi;
    if (!path) return { result: "refused", message: machine ? `Corgi isn't installed on ${machine}` : NO_CORGI };
    try {
      const r = await exec(path, [command, ...args], machine ? { timeout, on: machine } : { timeout });
      try {
        return JSON.parse(r.stdout);
      } catch (_) {
        return { result: "refused", message: oneLine(r.stderr) || `corgi ${command} exited with ${r.code}` };
      }
    } catch (e) {
      return { result: "refused", message: `${what} did not answer: ${e}` };
    }
  }
}

// The one every copy shares.
export const store = new CorgiData();
