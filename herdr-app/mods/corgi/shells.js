// The terminal panel's shells (decision 2026-11-16, "A terminal in the
// right-hand panel follows the focused agent's folder; corgi close ends its
// shells"): what the panel's views (terminal.js) and the Projects list's
// `>_` marks share.
//
// One shell per folder, kept running while another shows: a dev server in
// one agent's worktree is still running when the panel comes back to it.
// A shell starts only when the person asks for one (decision 2026-11-17,
// "The terminal panel stays in the space it was opened in and never starts
// shells by itself"): see `ask` and `watch` below. Focusing an agent shows
// its folder's shell when it has one, else says there is none, with Open
// shell.
// A shell is a hidden terminal on herdr-core, `hc run --bg --keep` in the
// folder with the person's login shell, labelled `corgi.shell=panel` and
// `corgi.shell.dir=<folder>`. Those labels are the tracking: the mod reads
// them with `terminals()`, `corgi close` with `hc ls --label`, both from the
// one place that knows whether the shell still runs, so a shell the person
// ended (`exit`) is simply no longer listed, and nothing else needs to be
// kept in step. End shell in the header ends one (`endShell`: killed, then
// removed, as `corgi close` does). A shell moved into a tab is relabelled `corgi.shell=tab`:
// no longer the panel's, still ended by `corgi close` with its worktree.
//
// The panel is a column `corgi-terminal` in the layout's `panel-body` stack
// (`corgi app-layout` puts it there): the `terminal` view as its header
// over a block `corgi-terminal-body`, which shows `{ "terminal": "t9" }`,
// or the `terminal-hint` view while no shell shows. A block's terminal is
// part of the layout, so showing another shell is a change of the layout's
// file. Only that block is changed: the file is read just before and
// written with `writeTextFile`'s expected text, which refuses the write
// when anything (another mod, an installer, the app) changed the file in
// between; the next tick tries again on what is there then. It is written
// only when the block does not hold the shell already, and only while the
// panel shows. The app reloads the layout from the file and keeps which
// panel shows and its size (those are kept apart, by id). The app takes a
// terminal's block out of the file when its terminal ends; the next write
// puts it back.
import { hc, exec, run, layout, terminals, agents, panes, focused, spaces, shared, setShared, appHome, readTextFile, writeTextFile } from "herdr";
import { find, oneLine } from "./ui.js";
import { store } from "./data.js";

export const COLUMN = "corgi-terminal";
export const BODY = "corgi-terminal-body";
const PANEL = "panel";
const STACK = "panel-body";
// Shared: what the panel shows (see `panelState`), kept across restarts.
export const STATE = "corgi.terminal";
export const SHELL_LABEL = "corgi.shell";
export const DIR_LABEL = "corgi.shell.dir";
// The body while no shell shows.
export const HINT = { mod: "corgi", props: { view: "terminal-hint" } };
const BODY_PADDING = [6, 10];

// What the panel shows, shared by every copy:
//   kind      "worktree" (the agent's folder) or "root" (its project's primary
//             checkout), for the agent shown: its folder's choice in `rooted`
//   rooted    the agents' folders (`home`) whose panel shows the project root,
//             the switch's choice for each. The switch is the shown agent's
//             alone: a choice kept for every agent showed one root shell for
//             all of a project's workers, in every space (2026-11-17)
//   follow    true: the focused agent's; false: pinned to `agent`
//   agent     the agent's address
//   home      its folder: a worker's worktree, a supervisor's project root
//   dir       the folder shown: `home`, or its project's root
//   address   the shell shown there
//   ended     a folder whose shell the person ended or moved to a tab (`why`:
//             "ended", "moved"): no new one starts there until asked, or until
//             the panel goes to another folder
//   note      while no shell shows, why: "none" (no agent, or no shell in
//             its folder: `dir` says which), "remote", "starting", "ended",
//             "moved", "problem" (with `problem`)
export function panelState() {
  const st = shared(STATE);
  const was = st && typeof st === "object" ? st : {};
  // A state from before `rooted`: its root choice was made for the agent it showed.
  const rooted = Array.isArray(was.rooted) ? was.rooted : was.kind === "root" && was.home ? [was.home] : [];
  return { kind: "worktree", follow: true, ...was, rooted };
}

// The folders `rooted` keeps, the latest choices.
const ROOTED_MAX = 32;
// `rooted` with the choice for the folder `home`.
function choose(rooted, home, kind) {
  const out = rooted.filter((h) => h !== home);
  if (kind === "root" && home) out.push(home);
  return out.slice(-ROOTED_MAX);
}

const same = (a, b) => JSON.stringify(a ?? null) === JSON.stringify(b ?? null);

// Changes what the panel shows; every copy draws again only when it changed.
export function setPanelState(change) {
  const before = panelState();
  const next = { ...before, ...change };
  if (!same(before, next)) setShared(STATE, next);
}

// This computer's terminals; another machine's are `lab/t3`.
export const local = (address) => typeof address === "string" && address !== "" && !address.includes("/");

// --- the shells -------------------------------------------------------------------------

// Shells started here that `terminals()` does not list yet, by folder.
const started = new Map();
// Shells the person ended with End shell (`endShell`), no longer the
// panel's from the click on, though `terminals()` may list them live for a
// moment: address -> whether hc removed it (then dropped once unlisted).
const ending = new Map();

// The panel's live shells on this computer, by folder.
export function panelShells(list = terminals()) {
  const out = new Map();
  const all = Array.isArray(list) ? list : [];
  for (const [address, removed] of ending) if (removed && !all.some((t) => t?.address === address)) ending.delete(address);
  for (const t of all) {
    const labels = t?.labels ?? {};
    if (labels[SHELL_LABEL] !== "panel" || (t.state ?? "live") !== "live" || !local(t.address) || ending.has(t.address)) continue;
    const dir = labels[DIR_LABEL];
    if (dir && !out.has(dir)) out.set(dir, t.address);
  }
  for (const [dir, address] of started) {
    if (all.some((t) => t?.address === address)) started.delete(dir);
    else if (!out.has(dir)) out.set(dir, address);
  }
  return out;
}

// The panel's shell in `dir`, if one runs.
export const shellIn = (dir) => (dir ? panelShells().get(dir) ?? null : null);

// Finished shells of Corgi's are removed from herdr-core (it keeps a
// finished terminal until `hc rm`), each once.
const removing = new Set();
export function sweep(list = terminals()) {
  for (const t of Array.isArray(list) ? list : []) {
    if (!t?.labels?.[SHELL_LABEL] || t.state !== "finished" || !local(t.address) || removing.has(t.address) || ending.get(t.address) === false) continue;
    forget(t.address);
  }
}
function forget(address) {
  for (const [dir, a] of started) if (a === address) started.delete(dir);
  if (!local(address) || removing.has(address)) return;
  removing.add(address);
  Promise.resolve(hc(["rm", address])).catch(() => {});
}

// The person's login shell: $SHELL, else what the user database says.
const LOGIN_SHELL = 's=${SHELL:-}; [ -n "$s" ] || s=$(dscl . -read "/Users/$(id -un)" UserShell 2>/dev/null | sed -n "s/^UserShell: //p"); '
  + '[ -n "$s" ] || s=$(getent passwd "$(id -un)" 2>/dev/null | cut -d: -f7); printf %s "${s:-/bin/sh}"';
let login = null;
async function loginShell() {
  if (login) return login;
  try {
    const r = await exec("/bin/sh", ["-c", LOGIN_SHELL], { timeout: 5000 });
    login = (r.code === 0 && r.stdout.trim()) || "/bin/sh";
  } catch (_) {
    login = "/bin/sh";
  }
  return login;
}

// A shell's name in `hc ls` and on its tab once moved: `sh-<folder>`.
export const shellName = (dir) => `sh-${dir.replace(/\/+$/, "").split("/").pop()}`.toLowerCase().replace(/[^a-z0-9_-]+/g, "-").slice(0, 32);

// What hc said went wrong, in a line.
function hcProblem(r, command = "run") {
  try {
    return oneLine(JSON.parse(r.stderr).error.message);
  } catch (_) {
    return oneLine(r.stderr) || `hc ${command} exited ${r.code}`;
  }
}

// The kind of hc's error ("live", "not_found", …), if it said one.
function hcKind(r) {
  try {
    return JSON.parse(r.stderr).error.kind ?? null;
  } catch (_) {
    return null;
  }
}

const starting = new Map(); // dir -> the start under way
// The panel's shell in `dir`: the one there, else a new one. Its address.
export function openShell(dir) {
  const had = shellIn(dir);
  if (had) return Promise.resolve(had);
  if (!starting.has(dir)) starting.set(dir, startShell(dir).finally(() => starting.delete(dir)));
  return starting.get(dir);
}

async function startShell(dir) {
  const shell = await loginShell();
  const args = (name) => [
    "run", "--bg", "--keep", "--json", "--no-parent", "--no-notify", ...(name ? ["--name", name] : []),
    "--cwd", dir, "--label", `${SHELL_LABEL}=panel`, "--label", `${DIR_LABEL}=${dir}`, "--", shell, "-l",
  ];
  let r = await hc(args(shellName(dir)));
  // A name is unique among live terminals: another folder of that name has it.
  if (r.code !== 0) r = await hc(args(null));
  if (r.code !== 0) throw new Error(hcProblem(r));
  const address = JSON.parse(r.stdout).address;
  started.set(dir, address);
  return address;
}

// --- an agent's folders -------------------------------------------------------------

const tops = new Map(); // a live folder -> the top of its checkout (itself outside one)
const roots = new Map(); // a folder -> its project's primary checkout (itself outside one)

async function git(dir, args) {
  try {
    const r = await exec("git", args, { cwd: dir, timeout: 10000 });
    return r.code === 0 ? r.stdout.trim() : null;
  } catch (_) {
    return null;
  }
}

// What Corgi says the agent's folder is: a worker's worktree, a
// supervisor's project, else where it runs.
function corgiHome(address) {
  const c = store.agents.get(address);
  if (!c) return null;
  if (c.worktree) return c.worktree;
  if (c.role === "supervisor" && c.project) return c.project;
  return c.cwd || null;
}

// The agent's folder from what is known now, without asking git: Corgi's,
// else the checkout its live folder is in (once read), else that folder.
// null for another machine's agent: its shells would be that machine's.
export function homeNow(address) {
  if (!local(address)) return null;
  const known = corgiHome(address);
  if (known) return known;
  const cwd = panes().find((p) => p.terminal === address)?.cwd ?? null;
  return cwd ? tops.get(cwd) ?? cwd : null;
}

// The agent's folder, asking git for the checkout of one Corgi does not know.
export async function homeOf(address) {
  if (!local(address)) return null;
  if (corgiHome(address)) return homeNow(address);
  const cwd = panes().find((p) => p.terminal === address)?.cwd ?? null;
  if (!cwd) return null;
  if (!tops.has(cwd)) tops.set(cwd, (await git(cwd, ["rev-parse", "--show-toplevel"])) || cwd);
  return tops.get(cwd);
}

// The project root (the primary checkout) of `dir`, the folder of the agent
// at `address`: Corgi's project, else git's common directory's folder.
export async function rootOf(dir, address = null) {
  const c = address ? store.agents.get(address) : null;
  if (c?.project && corgiHome(address) === dir) return c.project;
  if (!roots.has(dir)) {
    const common = await git(dir, ["rev-parse", "--path-format=absolute", "--git-common-dir"]);
    roots.set(dir, common && /\/\.git\/?$/.test(common) ? common.replace(/\/\.git\/?$/, "") : dir);
  }
  return roots.get(dir);
}

// --- the layout ---------------------------------------------------------------------------

// Whether the layout shows the terminal panel: its column is the stack's
// shown child and the panel's column is not hidden.
export function panelShows(tree = layout()) {
  const panel = find(tree, PANEL);
  const stack = find(tree, STACK);
  if (!panel || !stack || panel.hidden || !find(stack, COLUMN)) return false;
  return (stack.active ?? stack.children?.[0]?.id) === COLUMN;
}

// What the panel's body shows in `tree`: its block's content, null when
// there is no block (the app took it out when its terminal ended).
export const bodyOf = (tree) => find(find(tree, COLUMN), BODY)?.content ?? null;

// Puts `content` into the panel's body in `tree`, the block put back when
// it is gone: whether `tree` changed. A tree without the panel is left alone.
export function placeBody(tree, content) {
  const column = find(tree, COLUMN);
  if (!column) return false;
  column.children ??= [];
  const body = column.children.find((c) => c?.id === BODY);
  if (!body) {
    column.children.push({ id: BODY, type: "block", padding: BODY_PADDING, content });
    return true;
  }
  if (same(body.content, content)) return false;
  body.content = content;
  return true;
}

// Writes `content` into the body block of the shown layout's file, and
// nothing else (see the top). One write at a time. Whether the file holds
// it now: false when it was changed meanwhile, or is not a layout of the
// panel's, and the next tick tries again.
let writes = Promise.resolve();
export function putBody(content) {
  const write = writes.then(() => writeBody(content));
  writes = write.catch(() => false);
  return write;
}

async function writeBody(content) {
  const name = shared("layout");
  if (typeof name !== "string" || !name || name.includes("/")) return false;
  const path = `${appHome()}/layouts/${name}.json`;
  try {
    const text = await readTextFile(path);
    if (!text) return false;
    const tree = JSON.parse(text);
    if (!find(tree, COLUMN)) return false;
    if (!placeBody(tree, content)) return true;
    await writeTextFile(path, `${JSON.stringify(tree, null, 2)}\n`, text);
    return true;
  } catch (_) {
    // Changed since it was read (or not JSON while another writer was at it).
    return false;
  }
}

// --- what the person asks for ---------------------------------------------------------

// A shell starts in the shown folder only on the person's word: the
// Terminal button opening the panel, Open shell or New shell in its body,
// the Worktree | Project root switch. Focusing an agent starts none, nor
// does a space the window moves to whose panel the tab bar restores.
//
// `asked`: the next look starts a shell in its folder when it has none
// (and the folder's last shell was not ended or moved away), in the space
// it was asked in. Taken by that look, dropped when the space changes or
// the panel hides.
let asked = null;
// The panel as last watched: the space, and the times it was first and
// last seen hidden there.
let seen = null;
// How long the panel must have been seen hidden in a space for its showing
// there to be the person's opening. Moving to a space whose panel the tab
// bar restores hides it for a frame or two at most, seen once at most.
export const STEADY_MS = 400;

function activeSpace() {
  const all = spaces();
  return (Array.isArray(all) ? all.find((s) => s?.active)?.id : null) ?? focused()?.space ?? null;
}

// The person asked for a shell in the folder the panel shows.
export function ask() {
  asked = { space: activeSpace() };
}

// One look at whether the panel shows, from every copy's timer (the
// Projects list's runs while the panel is hidden): shown again in the
// same space after being seen hidden there for a while, the person opened
// it (the Terminal button, or the panel's own column chosen in the stack),
// and that asks for a shell.
export function watch(tree = layout(), now = Date.now()) {
  const space = activeSpace();
  const shows = panelShows(tree);
  if (!seen || seen.space !== space) seen = { space, from: null, last: null };
  if (asked && asked.space !== space) asked = null;
  if (shows) {
    if (seen.from !== null && seen.last - seen.from >= STEADY_MS) asked = { space };
    seen.from = seen.last = null;
  } else {
    seen.from ??= now;
    seen.last = now;
    asked = null;
  }
}

// --- following ------------------------------------------------------------------------------

// Which agent the panel is for: the pinned one; else the focused terminal
// when it is an agent's (the panel's own shell, a person's shell or a mod
// keeps the one before); else the one it was for.
export function targetOf(st, f = focused(), listed = agents()) {
  if (!st.follow && st.agent) return st.agent;
  const t = f?.terminal ?? null;
  if (t && listed.some((a) => a.address === t)) return t;
  return st.agent ?? null;
}

// One look: while the panel shows, the shell of the folder it is for, put
// into its body; when the folder has none, the hint saying so, or a new
// shell when the person asked for one (`ask`). Every copy's timer calls
// it; one look runs at a time.
let looking = false;
export async function follow() {
  watch();
  if (looking) return;
  looking = true;
  try {
    await look();
  } finally {
    looking = false;
  }
}

async function look() {
  const tree = layout();
  if (!find(tree, COLUMN)) return;
  sweep();
  if (!panelShows(tree)) return;
  const st = panelState();
  const agent = targetOf(st);
  const remote = Boolean(agent) && !local(agent);
  // Pinned, its folder stays even when the agent is gone (closed).
  const home = !agent || remote ? null : (!st.follow && st.agent === agent && st.home) || (await homeOf(agent)) || (agent === st.agent ? st.home : null);
  // The switch as chosen for this agent's folder (Worktree unless chosen).
  const kind = home ? (st.rooted.includes(home) ? "root" : "worktree") : st.kind;
  const dir = home ? (kind === "root" ? await rootOf(home, agent) : home) : null;
  // A switch while git answered: the next look is for it.
  const now = panelState();
  if (now.kind !== st.kind || !same(now.rooted, st.rooted) || now.follow !== st.follow || (!now.follow && now.agent !== st.agent)) return;
  // The person's ask, taken by this look (a later one is the next look's).
  const wanted = asked !== null;
  asked = null;
  let { ended = null, why = null } = st;
  if (dir !== st.dir) {
    ended = null;
    why = null;
  }
  const shells = panelShells();
  let address = dir ? shells.get(dir) ?? null : null;
  // The shell it showed there is gone, and not by a move to a tab: the
  // person ended it. It is dropped; a new one starts when asked (New shell).
  if (dir && st.dir === dir && st.address && !address && !ended && ![...shells.values()].includes(st.address)) {
    forget(st.address);
    ended = dir;
    why = "ended";
  }
  let note = null;
  let problem = null;
  if (!agent || (!dir && !remote)) note = "none";
  else if (remote) note = "remote";
  else if (!address && ended === dir) note = why;
  else if (!address && !wanted && st.note === "problem" && st.dir === dir) {
    // The start that failed there stays said until asked again.
    note = "problem";
    problem = st.problem;
  } else if (!address && !wanted) note = "none";
  else if (!address) {
    setPanelState({ agent, home, kind, dir, address: null, ended, why, note: "starting", problem: null });
    try {
      address = await openShell(dir);
    } catch (e) {
      note = "problem";
      problem = oneLine(String(e?.message ?? e)) || "The shell did not start.";
    }
  }
  setPanelState({ agent, home, kind, dir, address, ended, why, note: address ? null : note, problem });
  const content = address ? { terminal: address } : HINT;
  if (!same(bodyOf(layout()), content)) await putBody(content);
}

// --- what the views do ------------------------------------------------------------------

// Shows the panel: selects its column and shows the column it is in.
export function showPanel() {
  run("layout.select", { stack: STACK, child: COLUMN });
  run("layout.show", { id: PANEL });
}

// The panel on the worktree shell of the agent at `address` (a card's
// `>_` mark): the agent is focused too while the panel follows focus, and
// pinned to while it does not.
export function showShellOf(address) {
  const st = panelState();
  const home = homeNow(address);
  setPanelState({ agent: address, home, kind: "worktree", rooted: choose(st.rooted, home, "worktree"), ended: null, why: null });
  if (st.follow) run("agent.focus", { terminal: address });
  showPanel();
}

// A shell in the shown folder, which has none: Open shell, or New shell
// where the person ended (or moved away) the last one.
export function newShell() {
  setPanelState({ ended: null, why: null });
  ask();
}

// The kind of folder ("worktree", "root") for the agent shown, kept for its
// folder: a shell starts in the other folder when it has none, the switch
// being the person's ask. Another agent shows its own folder's choice. Or
// follow or pin.
export function setKind(kind) {
  const st = panelState();
  if (st.kind === kind) return;
  setPanelState({ kind, rooted: choose(st.rooted, st.home, kind), ended: null, why: null });
  ask();
}
export function setFollow(on) {
  setPanelState({ follow: on });
}

// Moves the shown shell into a tab of its agent's space: relabelled the
// tab's (no longer the panel's, still ended with its worktree), taken out
// of the panel (a terminal shows in one place), the agent shown, then
// given a tab there; the panel is hidden, and offers a new shell for the
// folder when it shows again.
export async function moveToTab() {
  const st = panelState();
  const address = st.address;
  if (!local(address)) return false;
  const r = await hc(["label", address, `${SHELL_LABEL}=tab`]);
  if (r.code !== 0) {
    setPanelState({ problem: hcProblem(r) });
    return false;
  }
  for (const [dir, a] of started) if (a === address) started.delete(dir);
  setPanelState({ address: null, ended: st.dir, why: "moved", note: "moved", problem: null });
  await putBody(HINT);
  if (local(st.agent) && agents().some((a) => a.address === st.agent)) run("agent.focus", { terminal: st.agent });
  run("terminal.keep", { terminal: address, place: "tab" });
  run("layout.hide", { id: PANEL });
  return true;
}

// Ends the shell at `address`, the shown one (End shell, once the person
// confirmed it): as `corgi close` ends a worktree's shells, its program is
// ended (`hc kill`, which answers once the terminal finished; whatever runs
// in it, its background jobs too, stops) and the finished terminal removed
// (`hc rm`). It stops being the panel's at once, so neither the `>_` mark
// nor a look shows it meanwhile, and the body shows the hint before the
// kill, so the app has no block of an ending terminal to take out (when the
// write loses to another writer it does, and the next look puts the block
// back). The panel then says "No shell here" with Open shell, and starts
// none by itself: nothing is asked. `sleep(ms)` (the view's `cx.sleep`)
// paces the removal while hc still lists it live; without it, or when the
// removal fails, `sweep` removes the finished terminal. `{ ok, problem }`.
export async function endShell(address, sleep = null) {
  if (!local(address)) return { ok: false, problem: "The panel ends this computer's shells only." };
  ending.set(address, false);
  for (const [dir, a] of started) if (a === address) started.delete(dir);
  asked = null;
  const st = panelState();
  if (st.address === address) setPanelState({ address: null, ended: null, why: null, note: "none", problem: null });
  for (let i = 0; i < 3 && !(await putBody(HINT)); i++);
  const killed = await hc(["kill", address, "--json"]);
  for (let i = 0; ; i++) {
    const r = await hc(["rm", address, "--json"]);
    if (r.code === 0 || hcKind(r) === "not_found") {
      ending.set(address, true);
      return { ok: true, problem: null };
    }
    // Not ended (hc refused the kill, the core is gone): the panel's
    // again, and said.
    if (killed.code !== 0) {
      ending.delete(address);
      return { ok: false, problem: `The shell did not end: ${hcProblem(killed, "kill")}` };
    }
    if (hcKind(r) === "live" && sleep && i < 20) {
      await sleep(150);
      continue;
    }
    // Ended: `sweep` removes it once hc lists it finished.
    ending.set(address, true);
    return { ok: true, problem: null };
  }
}
