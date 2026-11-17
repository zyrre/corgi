// Jira in the Herdr app: the active sprint of a board, the token user's issues
// first and the rest grouped by status under headings that fold; an issue
// (its summary, type, status, people, description with Jira's wiki markup
// made readable, subtasks, links and comments); and four actions, each asked
// about first: change its status (one of the transitions Jira offers the
// token's user), assign it to me, add a comment, log work. It reaches Jira
// Server or Data Center with a personal access token alone (api.js), so the
// token's Jira user decides what it may do: what Jira refuses shows as a
// line saying so, and nothing works around it.
// One view at a time, in a stack: the list, an issue, its status choices,
// the comment form, the work log form, the settings. Given `{ panel: true }`
// as its props (a child of the layout's right-hand panel stack, beside
// Files, Changes, GitLab and Jenkins, where install.sh --panel puts it), it
// draws itself as they do: a card with rounded corners and a border. The app
// gives a block's props through `props()`, not as init's argument.
// Standalone: no other mod and no CLI is needed.
// What it reads is read once for every copy of it (data.js), which also
// backs off from a Jira that fails and keeps a short failure quiet.
// The token user's issues assigned to them, commented on or moved by
// someone else post a toast (alerts.js), unless muted (the bell on an
// issue's row and in its view); the toast's click opens the panel on that
// issue. The panel keeps reading for the toasts while it is hidden or
// another child of its stack shows, more slowly; the app starts a stack's
// child only once it is shown, so the toasts begin once the panel has been
// shown after the app started. `jira.open-<KEY>` shows the panel on an issue.

import { View, div, svg } from "gpui-kit";
import { v_flex, h_flex, InputState, Input, TextareaState, Textarea } from "gpui-base";
import { theme, home, nextCommand, settings as look, shared, setShared, props as given, notify, layout, run } from "herdr";
import { Client, loadSettings, saveSettings, settingsPath, ready, blank } from "./api.js";
import {
  normalizeBase, hostOf, parseBoard, keyOf, isMe, nameOf, groups, categoryOf, ago, lastRead, timeOf, sprintEnds,
  parseTimeSpent, parseDay, dayOf, startedOn, requiredFields, oneLine, explain,
} from "./util.js";
import { blocks } from "./wiki.js";
import { icon, spinner, categoryColor, typeMark, statusPill, iconButton, pill } from "./ui.js";
import { store } from "./data.js";
import { MUTED, OPEN, AT_ONCE, muted, toast, summary, panelBlock, panelSelected, panelShows, showCommands, showStep } from "./alerts.js";

const LIST_EVERY = 60000; // the sprint's issues while the list shows
const BEHIND_EVERY = 180000; // the sprint's issues while another view shows over the list
const SPRINTS_EVERY = 300000; // the board's active sprints
const ISSUE_EVERY = 30000; // the issue shown, and its transitions while they show
const WATCH_EVERY = 60000; // the popups' read while the panel shows
// While the panel does not show: the popups' read, and what the view shows
// (read at once all the same when it was never read, so the view never waits
// for it when it shows again).
const HIDDEN_WATCH = 180000, HIDDEN_EVERY = 600000;
const FRESH = 5000; // opening a view reads what it shows unless it was read this lately
const FOLDED = "jira.folded"; // the list's headings folded or unfolded, kept by the app across restarts
const GOTO_FOR = 20000; // a goto is taken up by a copy that starts this soon after it
const PENDING_FOR = 15000; // the panel shown this soon after a toast it posted hidden goes to its issue
const ROWS = 40; // a heading's rows shown before "Show all"
const COMMENTS = 20; // an issue's last comments shown

const MONO = () => look().font_family ?? "JetBrains Mono";

export default class Jira extends View {
  init(initial, cx) {
    let layoutProps = null;
    try { layoutProps = given(); } catch { /* shown where it has no props */ }
    this.panel = Boolean((layoutProps ?? initial)?.panel);
    this.hidden = false; // the panel's column is hidden, or its stack shows another child
    this.selected = true; // its stack shows it, the column hidden or not
    this.hover = null; // the id of the row under the pointer: it shows its bell
    this.went = 0; // the last goto this copy took up (`store.goto.seq`)
    this.handle = cx.focus_handle();
    this.settings = null; // null until read
    this.src = null; // what Jira said under them (data.js), shared with every copy using the same
    this.saved = 0; // the store's settings this copy took up (`store.saved`)
    this.drawn = -1; // the store's version this copy saw last
    this.staleShown = false; // a "Last read … ago" line is drawn: it counts on
    this.expanded = new Set(); // headings showing all their rows
    // The views over the list: { kind: "issue" | "status" | "comment" | "worklog" | "settings", key }.
    this.stack = [];
    this.form = null; // the comment or work log form: { key, body | time, day, note, error }
    this.confirm = null; // an action being asked about: { kind: "transition" | "assign", key, transition }
    this.notice = null; // what an action came to: { tone: "ok" | "error", text }
    this.busy = null; // "save" | "check" | "transition" | "assign" | "comment" | "worklog"
    this.check = null; // the settings' check: { ok, text }
    this.typing = null; // the input with the keyboard
    this.inputs = {
      url: InputState.new({ placeholder: "https://jira.example.com" }),
      token: InputState.new({ placeholder: "A personal access token" }),
      board: InputState.new({ placeholder: "A board's id (42) or a project's key (DEMO)" }),
    };
    this.inputs.token.set_masked(true);
    this.order = Object.values(this.inputs); // Tab's order in the form on screen
    for (const input of this.order) this.wire(input, (cx) => this.save(cx));

    cx.spawn(async (cx) => {
      let s;
      try { s = await loadSettings(); } catch { s = { ...blank(), source: "none" }; }
      this.apply(s, cx);
      this.takeGoto(cx);
      cx.notify();
    });
    cx.timer.every(2000, (cx) => this.tick(cx));
    // `jira.refresh`, `jira.settings`, `jira.show` and `jira.open-<KEY>`, for keys.json and the toasts.
    cx.spawn(async (cx) => {
      for (;;) {
        let command;
        try { command = await nextCommand(); } catch { return; } // no more commands: the copy is going
        const key = keyOf(command);
        if (command === "refresh") this.refresh(cx);
        else if (command === "settings") this.show({ kind: "settings" }, cx);
        else if (command === "show") this.goTo(null, cx);
        else if (key) this.goTo(key, cx);
        cx.notify();
      }
    });
  }

  // An input of a form: Enter submits it (an input, not a textarea), and it knows when it has the keyboard.
  wire(input, submit) {
    if (submit) input.on("submit", (_e, cx) => submit(cx));
    input.on("change", (_e, cx) => cx.notify());
    input.on("focus", (_e, cx) => { this.typing = input; cx.notify(); });
    input.on("blur", (_e, cx) => { if (this.typing === input) this.typing = null; cx.notify(); });
  }

  // --- settings ------------------------------------------------------------------

  // Takes settings into use, for every copy (`store.use`), dropping this
  // copy's views over the last ones'.
  apply(s, cx) {
    store.use(s);
    this.saved = store.saved;
    this.settings = s;
    this.src = ready(s) ? store.source(s) : null;
    this.form = null; this.confirm = null;
    this.stack = this.stack.filter((v) => v.kind === "settings");
    for (const key of Object.keys(this.inputs)) this.inputs[key].set_value(s[key] ?? "");
    if (this.src) this.poll(cx);
  }

  get client() { return this.src?.client ?? null; }

  typed() {
    return { url: normalizeBase(this.inputs.url.value()), token: this.inputs.token.value().trim(), board: this.inputs.board.value().trim() };
  }

  // Asks Jira who the typed token is and which board it names, saving
  // nothing (Check), or after saving (Save).
  verify(cx, save) {
    const s = this.typed();
    if (!ready(s) || !s.board) {
      this.check = { ok: false, text: "The Jira URL, the token and a board (or project) are all needed." };
      cx.notify();
      return;
    }
    if (!parseBoard(s.board)) {
      this.check = { ok: false, text: `"${s.board}" is neither a board's id (such as 42) nor a project's key (such as DEMO).` };
      cx.notify();
      return;
    }
    if (this.busy) return;
    this.busy = save ? "save" : "check";
    this.check = null;
    cx.spawn(async (cx) => {
      let saved = true;
      try {
        if (save) await saveSettings(s);
      } catch (e) {
        saved = false;
        this.check = { ok: false, text: `Could not save the settings: ${e?.message ?? e}` };
      }
      if (saved) {
        const client = new Client(s);
        try {
          const who = await client.myself();
          const as = who.displayName && who.displayName !== who.name ? `${who.displayName} (${who.name})` : who.name;
          try {
            const board = await client.board(parseBoard(s.board));
            this.check = { ok: true, text: `Authenticated as ${as} · board ${board.id}, ${board.name}` };
          } catch (e) {
            this.check = { ok: false, text: `Authenticated as ${as}, but: ${explain(e, `find the board ${s.board}`, who.name)}` };
          }
        } catch (e) {
          this.check = { ok: false, text: explain(e, "ask who this token is") };
        }
        if (save) {
          this.apply({ ...s, source: "file" }, cx);
          // Saved and accepted: on to the sprint. Refused: stay, with the reason.
          this.stack = this.check.ok ? [] : [{ kind: "settings" }];
        }
      }
      this.busy = null;
      cx.notify();
    });
    cx.notify();
  }

  save(cx) { this.verify(cx, true); }

  // --- reading -------------------------------------------------------------------
  // Every read goes through the store (data.js), which every copy shares: it
  // reads a thing when it is due by the thing's own times, so copies that
  // show the same thing read it once.

  view() {
    if (this.settings && !this.client) return { kind: "settings" };
    return this.stack.at(-1) ?? { kind: "list" };
  }

  entry(kind, key = "") { return this.src?.peek(kind, key) ?? null; }

  // The token's Jira user, once Jira said: { name, key, displayName }.
  get me() { return this.entry("who")?.data ?? null; }
  meName() { return this.me?.displayName || this.me?.name || ""; }

  // The board, once found: { id, name, type }.
  get board() { return this.entry("board", this.settings?.board ?? "")?.data ?? null; }
  sprints() { return this.board ? this.entry("sprints", String(this.board.id))?.data?.sprints ?? [] : []; }
  issueOf(key) { return this.entry("issue", key)?.data ?? null; }

  // An entry's error as the view shows it: none while it fails quietly.
  errorOf(entry, doing) {
    return entry?.loud() ? explain(entry.error, doing, this.me?.name) : "";
  }

  readWho(cx, opts = {}) {
    // Once it has answered; until then at the back-off of its failures.
    this.src?.read("who", "", (c) => c.myself(), cx, { every: this.me ? Infinity : 0, ...opts });
  }

  // The board, then its sprints, then their issues: each good read starts
  // the next when it is due, so a first look needs no second tick.
  readBoard(cx, opts = {}) {
    const text = this.settings?.board ?? "", spec = parseBoard(text);
    if (!spec) return;
    this.src?.read("board", text, (c) => c.board(spec), cx, {
      every: this.board ? Infinity : 0, ...opts, then: (_e, cx) => this.readSprints(cx),
    });
  }

  readSprints(cx, opts = {}) {
    if (!this.board) return;
    const id = this.board.id;
    this.src?.read("sprints", String(id), (c) => c.sprints(id), cx, {
      every: this.slow(SPRINTS_EVERY), ...opts,
      then: (_e, cx) => { for (const s of this.sprints()) this.readIssues(s, cx); },
    });
  }

  readIssues(sprint, cx, opts = {}) {
    this.src?.read("issues", String(sprint.id), (c) => c.sprintIssues(sprint.id), cx, {
      every: this.slow(this.view().kind === "list" ? LIST_EVERY : BEHIND_EVERY), ...opts,
    });
  }

  // How often a thing is read that is read every `every` while the panel
  // shows: while it does not, every HIDDEN_EVERY at least.
  slow(every) { return this.hidden ? Math.max(every, HIDDEN_EVERY) : every; }

  readIssue(key, cx, opts = {}) {
    this.src?.read("issue", key, (c) => c.issue(key), cx, opts);
  }

  readTransitions(key, cx, opts = {}) {
    this.src?.read("transitions", key, (c) => c.transitions(key), cx, opts);
  }

  // The popups' read: the token user's issues updated lately, with their
  // history and comments (alerts.js). Once Jira said who the token is.
  readWatch(cx, opts = {}) {
    const me = this.me;
    if (!me) return;
    this.src?.read("watch", "", async (c) => {
      const asked = Date.now();
      return { ...(await c.watch(store.watch.window(asked))), asked };
    }, cx, {
      ...opts,
      then: (e) => this.announce(store.watch.read(e.data.issues, me, e.data.asked)),
    });
  }

  // Reads everything the view shows now, also what fails and waits for its
  // back-off: the refresh button, `jira.refresh`.
  refresh(cx) {
    if (!this.src) return;
    const view = this.view(), force = { force: true };
    if (!this.me) this.readWho(cx, force);
    if (!this.board) this.readBoard(cx, force);
    this.readSprints(cx, force);
    if (view.kind === "list") for (const s of this.sprints()) this.readIssues(s, cx, force);
    if (view.key) this.readIssue(view.key, cx, force);
    if (view.kind === "status") this.readTransitions(view.key, cx, force);
  }

  // What is due of what the view shows, and the popups' read. While the
  // panel does not show, all of it more slowly; what was never read, at once.
  poll(cx) {
    if (!this.src) return;
    const view = this.view();
    this.readWho(cx);
    this.readWatch(cx, { every: this.hidden ? HIDDEN_WATCH : WATCH_EVERY });
    if (view.kind === "settings") return;
    this.readBoard(cx);
    this.readSprints(cx);
    for (const s of this.sprints()) this.readIssues(s, cx);
    if (view.key) this.readIssue(view.key, cx, { every: this.slow(ISSUE_EVERY) });
    if (view.kind === "status") this.readTransitions(view.key, cx, { every: this.slow(ISSUE_EVERY) });
  }

  // The layout the window shows, or null.
  tree() {
    try { return layout(); } catch { return null; } // no layout here
  }

  // Every two seconds: takes up settings another copy saved, reads what is
  // due, and draws again when another copy's read changed something (or a
  // "Last read … ago" line counts on).
  tick(cx) {
    if (store.saved !== this.saved && store.settings && this.view().kind !== "settings") {
      this.apply(store.settings, cx);
      cx.notify();
    }
    // Whether this copy shows: always, unless it is the right-hand panel's
    // and the panel's column is hidden or its stack shows another child.
    const tree = this.panel ? this.tree() : null, was = this.hidden, selected = this.selected;
    this.hidden = Boolean(tree) && !panelShows(tree);
    this.selected = !tree || panelSelected(tree);
    this.clicked(was, selected, tree);
    this.takeShared(cx);
    this.takeGoto(cx);
    this.poll(cx);
    if (store.version !== this.drawn) {
      this.drawn = store.version;
      cx.notify();
    } else if (this.staleShown) cx.notify();
  }

  // --- toasts and gotos -------------------------------------------------------------

  mutes() {
    try { return new Set((shared(MUTED) ?? []).filter((k) => typeof k === "string")); } catch { return new Set(); }
  }

  toggleMute(key, cx) {
    const set = this.mutes();
    if (set.has(key)) set.delete(key);
    else set.add(key);
    try { setShared(MUTED, [...set]); } catch { /* kept until the app starts over */ }
    cx.notify();
  }

  // Posts a toast for each event (alerts.js) of an issue not muted, one for
  // all when there are more than AT_ONCE, whether the panel shows or not.
  // The read that found them runs once for every copy, so each is posted once.
  announce(events) {
    const keys = this.mutes(), loud = events.filter((e) => !muted(e, keys));
    if (!loud.length) return;
    const post = (n, key) => {
      try { notify({ ...n, action: this.action(key) }); } catch { /* no toasts here */ }
    };
    if (loud.length > AT_ONCE) { post(summary(loud), null); return; }
    for (const e of loud) post(toast(e), e.key);
  }

  // What a toast's click runs: `jira.open-<KEY>` (`jira.show` for the list),
  // which reaches the copies that show. The panel that does not show gets no
  // mod command, so its toast's click runs the layout command that brings it
  // nearest to showing (`showStep`), and the panel goes to `key` when it
  // shows (`clicked`).
  action(key) {
    const command = key ? `open-${key}` : "show";
    const step = this.hidden ? showStep(this.tree()) : null;
    if (!step) return { name: `jira.${command}` };
    store.pending = { key, at: Date.now() };
    return { name: step[0], args: step[1] };
  }

  // Each tick of the panel, after a toast it posted while it did not show:
  // once it shows, it goes where the click asked. A click that only selected
  // it in its stack, its column hidden still, is followed by the rest of what
  // shows it: the click runs one command.
  clicked(was, selected, tree) {
    const p = store.pending;
    if (!this.panel || !p) return;
    if (Date.now() - p.at > PENDING_FOR) { store.pending = null; return; }
    if (!this.hidden) {
      if (was) { store.pending = null; this.aim(p.key); }
      return;
    }
    if (this.selected && !selected) this.showPanel(tree);
  }

  // `shared("jira.open")`, set by another mod: the panel goes to that issue
  // and shows, hidden or not. One copy takes it up, once.
  takeShared(cx) {
    let asked = null;
    try { asked = shared(OPEN); } catch { return; }
    if (!asked || typeof asked !== "object" || !(Number(asked.at) > store.opened)) return;
    store.opened = Number(asked.at);
    const key = keyOf(`open-${asked.key}`);
    if (!key || Date.now() - store.opened > GOTO_FOR) return;
    this.goTo(key, cx);
  }

  // `jira.open-<KEY>` (`jira.show`: the list), from a toast's click, a key or
  // another mod: every shown copy gets it, the first keeps where it goes for
  // all of them, and the panel is shown. The copy that shows the panel goes
  // there (`takeGoto`), also one the command starts.
  goTo(key, cx) {
    this.aim(key);
    this.showPanel(this.tree());
    this.takeGoto(cx);
  }

  aim(key) {
    const now = Date.now(), last = store.goto;
    if (last && last.key === key && now - last.at < 3000) return;
    store.gone += 1;
    store.goto = { seq: store.gone, key, at: now };
  }

  showPanel(tree) {
    for (const [name, args] of showCommands(tree)) {
      try { run(name, args); } catch { /* not here */ }
    }
  }

  // Goes where a goto asked, once: the issue, or the list. The panel does,
  // or the copy that got it when the layout has no panel.
  takeGoto(cx) {
    const g = store.goto;
    if (!g || g.seq === this.went || !this.settings) return;
    this.went = g.seq;
    if (Date.now() - g.at > GOTO_FOR) return; // asked before this copy started
    if (!this.panel && panelBlock(this.tree())) return; // the panel goes there
    if (!this.client) return;
    this.stack = [];
    this.form = null; this.confirm = null;
    if (g.key) this.show({ kind: "issue", key: g.key }, cx);
    cx.notify();
  }

  // --- moving about --------------------------------------------------------------

  show(view, cx) {
    if (view.kind === "settings" && this.view().kind === "settings") return;
    this.stack.push(view);
    this.confirm = null;
    this.forget(view);
    // What it shows is read unless another view or copy read it a moment ago.
    if (view.key) this.readIssue(view.key, cx, { every: FRESH });
    if (view.kind === "status") this.readTransitions(view.key, cx, { every: FRESH });
    if (view.kind === "settings") { this.check = null; this.order = Object.values(this.inputs); }
    this.handle.focus();
    cx.notify();
  }

  // A notice is about one issue: it goes when another view shows (an ok
  // one when any other view shows).
  forget(view) {
    if (this.notice && (this.notice.tone === "ok" || (this.notice.key && this.notice.key !== view.key))) this.notice = null;
  }

  back(cx) {
    const left = this.stack.pop();
    if (!left) return false;
    if (this.notice?.key && this.notice.key !== this.view().key) this.notice = null;
    if (left.kind === "comment" || left.kind === "worklog") this.form = null;
    this.confirm = null;
    this.typing = null;
    this.handle.focus();
    cx.notify();
    return true;
  }

  open(url, cx) {
    try { cx.open_url(url); } catch (e) { this.notice = { tone: "error", text: `Could not open ${url}: ${e?.message ?? e}` }; cx.notify(); }
  }

  // --- acting --------------------------------------------------------------------

  // What Jira refusing an action says. Jira Server answers 401, not 403, to
  // an assignment the user may not make: with a token Jira accepted for
  // reading, that is a refusal too, not a bad token.
  refused(e, doing) {
    const as = e?.kind === "auth" && this.me ? { kind: "forbidden", detail: e.detail } : e;
    return explain(as, doing, this.me?.name);
  }

  // After an action, good or not: what it changed is read at once.
  acted(key, cx) {
    this.readIssue(key, cx, { force: true });
    this.readTransitions(key, cx, { force: true });
    for (const s of this.sprints()) this.readIssues(s, cx, { force: true });
  }

  // Runs an action against Jira: `busy` while it runs, then a notice of
  // what it came to, and the issue read again.
  act(busy, key, doing, done, call, cx, after) {
    if (this.busy || !this.client) return;
    const client = this.client;
    this.busy = busy;
    cx.spawn(async (cx) => {
      try {
        await call(client);
        this.notice = { tone: "ok", text: done, key };
        after?.(true);
      } catch (e) {
        this.notice = { tone: "error", text: this.refused(e, doing), key };
        after?.(false);
      }
      this.acted(key, cx);
      this.busy = null;
      cx.notify();
    });
    cx.notify();
  }

  // A status: the transitions Jira offers, one picked, then asked about.
  // One whose screen requires a field this panel does not fill in is
  // refused here, with the way to the browser.
  askTransition(key, transition, cx) {
    const needs = requiredFields(transition);
    if (needs.length) {
      this.notice = { key, tone: "error", text: `${transition.name} asks for ${needs.join(", ")} on a screen this panel does not fill in: use the browser (↗) for this one.` };
      this.confirm = null;
    } else {
      this.confirm = { kind: "transition", key, transition };
    }
    cx.notify();
  }

  transition(cx) {
    const asked = this.confirm;
    if (asked?.kind !== "transition") return;
    const { key, transition } = asked, to = transition.to?.name ?? transition.name;
    this.act("transition", key, `move ${key} to ${to}`, `Moved ${key} to ${to}.`, (c) => c.transition(key, transition.id), cx, (ok) => {
      this.confirm = null;
      if (ok && this.view().kind === "status" && this.view().key === key) this.stack.pop();
    });
  }

  askAssign(key, cx) {
    this.confirm = { kind: "assign", key };
    cx.notify();
  }

  assign(cx) {
    const asked = this.confirm, me = this.me;
    if (asked?.kind !== "assign" || !me) return;
    this.act("assign", asked.key, `assign ${asked.key}`, `Assigned ${asked.key} to you.`, (c) => c.assign(asked.key, me), cx, () => { this.confirm = null; });
  }

  // The comment form; its button sends it, Cancel or Esc does not.
  askComment(key, cx) {
    const body = TextareaState.new({ placeholder: "Your comment. Jira's wiki markup works: *bold*, {code}…{code}", rows: 6 });
    this.wire(body, null);
    this.form = { key, body, error: "" };
    this.order = [body];
    this.show({ kind: "comment", key }, cx);
    body.focus();
  }

  sendComment(cx) {
    const form = this.form;
    if (!form?.body || this.busy) return;
    const text = form.body.value().trim();
    if (!text) { form.error = "Write something first."; cx.notify(); return; }
    form.error = "";
    this.act("comment", form.key, `comment on ${form.key}`, `Added your comment to ${form.key}.`, (c) => c.comment(form.key, text), cx, (ok) => {
      if (ok && this.form === form) { this.stack.pop(); this.form = null; }
    });
  }

  // The work log form: time spent, the day (today), a comment if any.
  askWorklog(key, cx) {
    const time = InputState.new({ placeholder: "1h 30m" });
    const day = InputState.new({ value: dayOf(Date.now()), placeholder: "YYYY-MM-DD" });
    const note = TextareaState.new({ placeholder: "What you did (optional)", rows: 3 });
    this.wire(time, (cx) => this.logWork(cx));
    this.wire(day, (cx) => this.logWork(cx));
    this.wire(note, null);
    this.form = { key, time, day, note, error: "" };
    this.order = [time, day, note];
    this.show({ kind: "worklog", key }, cx);
    time.focus();
  }

  logWork(cx) {
    const form = this.form;
    if (!form?.time || this.busy) return;
    const spent = parseTimeSpent(form.time.value()), day = parseDay(form.day.value());
    if (!spent) { form.error = "Time spent is a number and a unit: 1h 30m, 45m, 1d (w, d, h, m)."; cx.notify(); return; }
    if (!day) { form.error = "The date is a day as YYYY-MM-DD, today or before."; cx.notify(); return; }
    form.error = "";
    const note = form.note.value().trim();
    this.act("worklog", form.key, `log work on ${form.key}`, `Logged ${spent} on ${form.key}.`,
      (c) => c.worklog(form.key, { timeSpent: spent, started: startedOn(day), comment: note }), cx, (ok) => {
        if (ok && this.form === form) { this.stack.pop(); this.form = null; }
      });
  }

  // --- pieces --------------------------------------------------------------------

  line(text, t, color) {
    return div().px(10).py(6).text_size(12).text_color(color ?? t.dim).child(text);
  }

  // What an entry's reading has come to, for the rows where its data goes:
  // the error once it is to be shown; while it fails quietly, a dim line
  // that the data is from the last good read; with nothing read and nothing
  // wrong, `waiting` with a spinner. Never "Reading…" for a read given up on.
  status(entry, doing, t, now, id, waiting) {
    const error = this.errorOf(entry, doing);
    if (error) return [this.line(error, t, t.blocked)];
    if (entry?.data) {
      if (!entry.stale(now)) return [];
      this.staleShown = true;
      return [div().id(`ji-stale-${id}`).px(10).py(4).text_size(11).text_color(t.dim)
        .tooltip(`Jira did not answer the last ${entry.failures === 1 ? "read" : `${entry.failures} reads`}: trying again`)
        .child(`Last read ${lastRead(entry.at, now)} ago · no answer since`)];
    }
    return waiting ? [h_flex().px(10).py(6).gap(6).items_center().text_size(12).text_color(t.dim).child(spinner(t, 12)).child(waiting)] : [];
  }

  header(t, parts) {
    return h_flex().h(42).flex_none().pl(8).pr(8).gap(6).items_center().border_b(1).border_color(t.border).children(parts.filter(Boolean));
  }

  backButton(t) {
    return iconButton("ji-back", "back", "Back (Esc)", (cx) => this.back(cx), t, { size: 26 });
  }

  refreshButton(t) {
    const loading = this.src?.busy(["issues", "issue", "sprints", "board", "transitions"]);
    return div().id("ji-refresh").size(24).flex_none().flex().items_center().justify_center().rounded(6).text_color(t.dim)
      .cursor_pointer().hover((s) => s.bg(t.hover).text_color(t.foreground)).tooltip("Refresh")
      .on_click((_e, cx) => { this.refresh(cx); cx.notify(); })
      .child(loading ? spinner(t, 13) : svg(icon("refresh")).size(14));
  }

  // What an action came to, until dismissed; and the action being asked about.
  cards(t) {
    const out = [];
    if (this.notice) {
      const color = this.notice.tone === "ok" ? t.ansi2 : t.blocked;
      out.push(h_flex().id("ji-notice").flex_none().mx(10).mt(8).p(8).gap(8).items_start().rounded(7).border_1().border_color(color).bg(t.surface).text_size(12)
        .child(div().flex_1().min_w(0).text_color(this.notice.tone === "ok" ? t.foreground : t.blocked).child(this.notice.text))
        .child(iconButton("ji-dismiss", "x", "Dismiss", (cx) => { this.notice = null; cx.notify(); }, t, { size: 20 })));
    }
    if (this.confirm) {
      const c = this.confirm, issue = this.issueOf(c.key), from = issue?.fields?.status?.name;
      const assignee = issue?.fields?.assignee;
      const question = c.kind === "transition"
        ? `${c.transition.name}: move ${c.key}${from ? ` from ${from}` : ""} to ${c.transition.to?.name ?? c.transition.name}?`
        : `Assign ${c.key} to you (${this.meName()})? ${assignee ? `It is ${nameOf(assignee)}'s now.` : "Nobody has it now."}`;
      const yes = c.kind === "transition" ? "Move" : "Assign to me";
      out.push(v_flex().id("ji-confirm").flex_none().mx(10).mt(8).p(8).gap(8).rounded(7).border_1().border_color(t.accent).bg(t.surface).text_size(12)
        .child(div().child(question))
        .child(h_flex().gap(6).justify_end()
          .child(pill("ji-confirm-no", "Cancel", t, { onClick: (cx) => { this.confirm = null; cx.notify(); } }))
          .child(pill("ji-confirm-yes", yes, t, { tone: "solid", icon: c.kind === "transition" ? "move" : "user", busy: this.busy === c.kind, onClick: (cx) => (c.kind === "transition" ? this.transition(cx) : this.assign(cx)) }))));
    }
    return out;
  }

  // The bell of an issue: bell-off while muted, always; a bell to mute it
  // only while the pointer is over its row (`shown`). A row keeps its place
  // free, so that its texts do not move.
  bell(key, on, shown, t, size = 22) {
    if (!on && !shown) return div().size(size).flex_none();
    return iconButton(`ji-mute-${key}`, on ? "bell-off" : "bell",
      on ? `Popups muted for ${key}: click to unmute` : `Mute popups for ${key}`, (cx) => this.toggleMute(key, cx), t, { size });
  }

  hovered(id, on, cx) {
    if (on) this.hover = id;
    else if (this.hover === id) this.hover = null;
    cx.notify();
  }

  // An issue of the list: its type, key and summary, and the assignee (the
  // rest) or its status (the token user's). A click opens it.
  issueRow(issue, t, mutes, mine) {
    const f = issue.fields ?? {}, id = `ji-row-${issue.key}`, category = categoryOf(issue);
    const right = mine
      ? div().flex_none().max_w(110).truncate().text_size(11).text_color(categoryColor(category, t)).child(f.status?.name ?? "")
      : div().flex_none().max_w(110).truncate().text_size(11).text_color(t.dim).child(f.assignee ? nameOf(f.assignee) : "Unassigned");
    return h_flex().id(id).h(30).flex_none().pl(8).pr(2).gap(8).items_center().rounded(6).cursor_pointer().hover((s) => s.bg(t.hover))
      .tooltip(`${issue.key}: ${oneLine(f.summary)}\n${f.issuetype?.name ?? "Issue"} · ${f.status?.name ?? ""} · ${f.assignee ? nameOf(f.assignee) : "Unassigned"}`)
      .on_click((_e, cx) => this.show({ kind: "issue", key: issue.key }, cx))
      .on_hover((on, cx) => this.hovered(id, on, cx))
      .child(typeMark(f.issuetype, t))
      .child(div().flex_none().text_size(11.5).text_color(t.dim).child(issue.key))
      .child(div().flex_1().min_w(0).truncate().text_color(category === "done" ? t.muted : t.foreground).child(oneLine(f.summary)))
      .child(right)
      .child(this.bell(issue.key, mutes.has(issue.key), this.hover === id, t));
  }

  // Which headings are folded: as the person left them, else the done ones.
  folds() {
    try { const f = shared(FOLDED); return f && typeof f === "object" ? f : {}; } catch { return {}; }
  }

  isFolded(id, category) {
    const f = this.folds();
    return typeof f[id] === "boolean" ? f[id] : category === "done";
  }

  fold(id, category, cx) {
    const f = { ...this.folds(), [id]: !this.isFolded(id, category) };
    try { setShared(FOLDED, f); } catch { /* kept until the panel starts over */ }
    cx.notify();
  }

  // A heading of the list and its rows (the first ROWS, then "Show all").
  group(id, title, category, issues, t, mutes, mine) {
    const folded = this.isFolded(id, category), hid = `ji-head-${id}`;
    const rows = [h_flex().id(hid).h(32).flex_none().mt(4).pl(4).pr(8).gap(8).items_center().rounded(7)
      .cursor_pointer().hover((s) => s.bg(t.hover)).tooltip(folded ? "Show these issues" : "Fold them away")
      .on_click((_e, cx) => this.fold(id, category, cx))
      .child(svg(icon(folded ? "chevron-right" : "chevron-down")).size(14).flex_none().text_color(t.muted))
      .child(mine ? div().flex_none().font_semibold().child(title) : statusPill({ name: title, statusCategory: { key: category } }, t))
      .child(div().flex_1())
      .child(div().flex_none().text_size(12).text_color(t.dim).child(String(issues.length)))];
    if (folded) return rows;
    const all = this.expanded.has(id), shown = all ? issues : issues.slice(0, ROWS);
    rows.push(...shown.map((issue) => this.issueRow(issue, t, mutes, mine)));
    if (!all && issues.length > ROWS) {
      rows.push(div().id(`ji-more-${id}`).h(28).flex_none().px(10).flex().items_center().text_size(12).text_color(t.accent).cursor_pointer()
        .hover((s) => s.bg(t.hover)).rounded(6)
        .on_click((_e, cx) => { this.expanded.add(id); cx.notify(); })
        .child(`Show all ${issues.length}`));
    }
    return rows;
  }

  // A sprint: its heading, then the token user's issues, then the rest by status.
  sprintPart(sprint, t, now, mutes) {
    const entry = this.entry("issues", String(sprint.id)), data = entry?.data;
    const ends = sprintEnds(sprint, now);
    const rows = [v_flex().id(`ji-sprint-${sprint.id}`).flex_none().mt(6).px(6).py(4).gap(1)
      .when(Boolean(oneLine(sprint.goal)), (el) => el.tooltip(`Goal: ${oneLine(sprint.goal)}`))
      .child(h_flex().gap(8).items_center()
        .child(svg(icon("sprint")).size(15).flex_none().text_color(t.muted))
        .child(div().flex_1().min_w(0).truncate().text_size(16).font_semibold().child(sprint.name ?? `Sprint ${sprint.id}`))
        .when(Boolean(data), (el) => el.child(div().flex_none().text_size(12).text_color(t.dim).child(`${data.issues.length}${data.total > data.issues.length ? ` of ${data.total}` : ""}`))))
      .when(Boolean(ends), (el) => el.child(div().pl(23).text_size(11).text_color(t.dim).child(ends)))];
    rows.push(...this.status(entry, `read the issues of ${sprint.name ?? "the sprint"}`, t, now, `issues-${sprint.id}`, "Reading the sprint…"));
    if (!data) return rows;
    if (!data.issues.length) { rows.push(this.line("No issues in this sprint.", t)); return rows; }
    const g = groups(data.issues, this.me);
    if (this.me) {
      rows.push(...(g.mine.length ? this.group(`mine-${sprint.id}`, "Assigned to you", "indeterminate", g.mine, t, mutes, true)
        : [this.line(`None of it is assigned to you (${this.meName()}).`, t)]));
    }
    for (const s of g.rest) rows.push(...this.group(`status-${sprint.id}-${s.id}`, s.name, s.category, s.issues, t, mutes, false));
    return rows;
  }

  body(id, rows) {
    return v_flex().id(id).flex_1().min_h(0).items_center().overflow_y_scrollbar()
      .child(v_flex().w_full().max_w(820).px(6).pb(12).children(rows));
  }

  // --- views ---------------------------------------------------------------------

  renderList(t, now) {
    const host = hostOf(this.settings.url), board = this.board, me = this.me;
    const rows = [...this.cards(t)];
    const whoError = this.errorOf(this.entry("who"), "ask who this token is");
    if (whoError) rows.push(this.line(whoError, t, t.blocked));
    const spec = parseBoard(this.settings.board);
    if (!spec) {
      rows.push(this.line("Name a board (its id) or a project (its key) in the settings.", t, t.ansi3));
    } else if (!board) {
      rows.push(...this.status(this.entry("board", this.settings.board), `find the board ${this.settings.board}`, t, now, "board", "Finding the board…"));
    } else {
      const entry = this.entry("sprints", String(board.id));
      rows.push(...this.status(entry, `read the sprints of ${board.name}`, t, now, "sprints", "Reading the sprint…"));
      if (entry?.data?.none) rows.push(this.line(`${board.name} is a board without sprints (Kanban): name a Scrum board, or its project, in the settings.`, t, t.ansi3));
      else if (entry?.data && !entry.data.sprints.length) rows.push(this.line(`${board.name} has no active sprint.`, t));
      const mutes = this.mutes();
      for (const sprint of this.sprints()) rows.push(...this.sprintPart(sprint, t, now, mutes));
    }
    return [
      this.header(t, [
        svg(icon("jira")).size(16).ml(4).flex_none().text_color(t.muted),
        div().flex_none().font_semibold().child("Jira"),
        div().id("ji-who").flex_1().min_w(0).truncate().text_size(11).text_color(t.dim)
          .when(Boolean(me), (el) => el.tooltip(`Authenticated as ${this.meName()} (${me.name}) on ${host}: what this Jira user may do, this panel may do`))
          .child(board ? `${board.name}${me ? ` · ${me.name}` : ""}` : host),
        this.refreshButton(t),
        board ? iconButton("ji-board-open", "external", "Open the board in the browser", (cx) => this.open(this.client.boardPage(board.id), cx), t) : null,
        iconButton("ji-settings", "settings", "Settings", (cx) => this.show({ kind: "settings" }, cx), t),
      ]),
      this.body("ji-list", rows),
    ];
  }

  // Wiki markup's blocks (wiki.js) drawn: headings, lines, list items, code,
  // quotes and panels, table rows and rules.
  wikiBlocks(text, t, id) {
    return blocks(text).map((b, i) => {
      const bid = `${id}-${i}`;
      switch (b.kind) {
        case "h": return div().id(bid).pt(6).text_size(b.level <= 1 ? 15 : b.level === 2 ? 14 : 13).font_semibold().child(b.text);
        case "gap": return div().h(6).flex_none();
        case "rule": return div().h(1).my(4).flex_none().bg(t.border);
        case "li": return h_flex().id(bid).pl(4 + (b.level - 1) * 14).gap(6).items_start()
          .child(div().flex_none().w(b.ordered ? 18 : 10).text_color(t.dim).child(b.ordered ? `${b.n}.` : b.level > 1 ? "◦" : "•"))
          .child(div().flex_1().min_w(0).child(b.text));
        case "code": return v_flex().id(bid).my(2).p(8).rounded(6).bg(t.surface).border_1().border_color(t.border)
          .font_family(MONO()).text_size(11.5).overflow_hidden()
          .children((b.lines.length ? b.lines : [""]).map((l) => div().child(l || " ")));
        case "quote": return v_flex().id(bid).my(2).pl(10).gap(2).border_l(2).border_color(t.border).text_color(t.muted)
          .when(Boolean(b.title), (el) => el.child(div().font_semibold().text_color(t.foreground).child(b.title)))
          .children(b.lines.map((l) => div().child(l)));
        case "row": return h_flex().id(bid).gap(10).py(2).border_b(1).border_color(t.border)
          .children(b.cells.map((c) => div().flex_1().min_w(0).when(b.head, (el) => el.font_semibold()).child(c)));
        default: return div().id(bid).child(b.text || " ");
      }
    });
  }

  // An issue's heading in the view: a label, and the rows under it.
  part(label, t) {
    return div().h(28).flex_none().pt(12).px(2).text_size(11).font_medium().text_color(t.dim).child(label);
  }

  // A related issue (a subtask, a link's other end, the parent): a row that opens it.
  relatedRow(id, other, t, label = "") {
    const f = other?.fields ?? {};
    return h_flex().id(id).h(28).flex_none().px(6).gap(8).items_center().rounded(6).cursor_pointer().hover((s) => s.bg(t.hover))
      .on_click((_e, cx) => this.show({ kind: "issue", key: other.key }, cx))
      .when(Boolean(label), (el) => el.child(div().flex_none().max_w(110).truncate().text_size(11).text_color(t.dim).child(label)))
      .child(typeMark(f.issuetype, t, 13))
      .child(div().flex_none().text_size(11.5).text_color(t.dim).child(other.key))
      .child(div().flex_1().min_w(0).truncate().child(oneLine(f.summary)))
      .when(Boolean(f.status), (el) => el.child(div().flex_none().max_w(100).truncate().text_size(11).text_color(categoryColor(f.status?.statusCategory?.key, t)).child(f.status.name)));
  }

  renderIssue(view, t, now) {
    const key = view.key, entry = this.entry("issue", key), issue = entry?.data, f = issue?.fields ?? {};
    const mutes = this.mutes(), mine = isMe(f.assignee, this.me), busy = Boolean(this.busy);
    const rows = [...this.cards(t)];
    rows.push(...this.status(entry, `read ${key}`, t, now, "issue", `Reading ${key}…`));
    if (issue) {
      const facts = [f.issuetype?.name, f.priority?.name ? `${f.priority.name} priority` : "", f.updated ? `updated ${ago(timeOf(f.updated), now)} ago`.replace("now ago", "just now") : ""].filter(Boolean).join(" · ");
      const people = [`Assignee: ${f.assignee ? nameOf(f.assignee) : "nobody"}`, f.reporter ? `Reporter: ${nameOf(f.reporter)}` : ""].filter(Boolean).join(" · ");
      const tt = f.timetracking ?? {};
      const work = [tt.timeSpent ? `Logged ${tt.timeSpent}` : "", tt.remainingEstimate ? `${tt.remainingEstimate} left` : ""].filter(Boolean).join(" · ");
      if (f.parent?.key) rows.push(div().pt(8).child(this.relatedRow(`ji-parent-${f.parent.key}`, f.parent, t, "Part of")));
      rows.push(v_flex().px(6).pt(10).gap(4)
        .child(div().id("ji-summary").text_size(15).font_semibold().child(oneLine(f.summary)))
        .child(div().text_size(11.5).text_color(t.dim).child(facts))
        .child(div().text_size(11.5).text_color(t.dim).child(people))
        .when(Boolean(work), (el) => el.child(div().text_size(11.5).text_color(t.dim).child(work))));
      rows.push(h_flex().px(6).pt(10).gap(6).flex_wrap()
        .child(pill("ji-act-status", "Status…", t, { icon: "move", disabled: busy, tip: "Change its status: pick one of the steps Jira offers, then confirm", onClick: (cx) => this.show({ kind: "status", key }, cx) }))
        .when(!mine, (el) => el.child(pill("ji-act-assign", "Assign to me", t, { icon: "user", disabled: busy || !this.me, busy: this.busy === "assign", tip: "Asks first", onClick: (cx) => this.askAssign(key, cx) })))
        .child(pill("ji-act-comment", "Comment…", t, { icon: "message", disabled: busy, onClick: (cx) => this.askComment(key, cx) }))
        .child(pill("ji-act-worklog", "Log work…", t, { icon: "clock", disabled: busy, onClick: (cx) => this.askWorklog(key, cx) })));
      rows.push(this.part("Description", t));
      rows.push(f.description?.trim() ? v_flex().id("ji-description").px(6).gap(3).text_size(12.5).children(this.wikiBlocks(f.description, t, "ji-desc"))
        : this.line("No description.", t));
      const subtasks = f.subtasks ?? [];
      if (subtasks.length) {
        rows.push(this.part(`Subtasks · ${subtasks.length}`, t));
        rows.push(...subtasks.map((s) => this.relatedRow(`ji-sub-${s.key}`, s, t)));
      }
      const links = (f.issuelinks ?? []).filter((l) => l.outwardIssue || l.inwardIssue);
      if (links.length) {
        rows.push(this.part(`Links · ${links.length}`, t));
        rows.push(...links.map((l) => {
          const other = l.outwardIssue ?? l.inwardIssue, how = l.outwardIssue ? l.type?.outward : l.type?.inward;
          return this.relatedRow(`ji-link-${l.id ?? other.key}`, other, t, how ?? l.type?.name ?? "");
        }));
      }
      const comments = f.comment?.comments ?? [], total = f.comment?.total ?? comments.length;
      rows.push(this.part(`Comments · ${total}`, t));
      if (!comments.length) rows.push(this.line("No comments yet.", t));
      if (comments.length > COMMENTS || total > comments.length) {
        rows.push(this.line(`${total - Math.min(COMMENTS, comments.length)} earlier comments are in the browser.`, t));
      }
      rows.push(...comments.slice(-COMMENTS).map((c) => v_flex().id(`ji-comment-${c.id}`).flex_none().mx(2).mt(4).p(8).gap(3).rounded(7).border_1().border_color(t.border).text_size(12.5)
        .child(h_flex().gap(6).items_center().text_size(11.5)
          .child(div().flex_1().min_w(0).truncate().font_medium().child(nameOf(c.author) || "Someone"))
          .child(div().flex_none().text_color(t.dim).child(`${ago(timeOf(c.created), now)} ago`.replace("now ago", "just now"))))
        .children(this.wikiBlocks(c.body, t, `ji-c${c.id}`))));
    }
    return [
      this.header(t, [
        this.backButton(t),
        issue ? typeMark(f.issuetype, t) : null,
        div().flex_none().font_semibold().child(key),
        // In a narrow panel the status gives way, not the buttons.
        h_flex().flex_1().min_w(0).overflow_hidden().when(Boolean(issue), (el) => el.child(statusPill(f.status, t))),
        this.bell(key, mutes.has(key), true, t, 24),
        this.refreshButton(t),
        iconButton("ji-issue-open", "external", "Open in the browser", (cx) => this.open(this.client.page(key), cx), t),
      ]),
      this.body("ji-issue", rows),
    ];
  }

  // The status choices: the transitions Jira offers the token's user now.
  renderStatus(view, t, now) {
    const key = view.key, issue = this.issueOf(key), entry = this.entry("transitions", key);
    const rows = [...this.cards(t)];
    rows.push(v_flex().px(10).pt(12).gap(4)
      .child(div().text_size(12).text_color(t.muted).child(issue ? `${key} is ${issue.fields?.status?.name ?? "?"} now. Pick where it goes; you are asked before it moves.` : `Pick where ${key} goes; you are asked before it moves.`)));
    rows.push(...this.status(entry, `read where ${key} can go`, t, now, "transitions", "Reading where it can go…"));
    const list = entry?.data;
    if (list && !list.length) rows.push(this.line("Jira offers this token no step from here.", t));
    for (const tr of list ?? []) {
      const needs = requiredFields(tr), picked = this.confirm?.kind === "transition" && this.confirm.transition.id === tr.id;
      rows.push(h_flex().id(`ji-tr-${tr.id}`).h(34).flex_none().px(8).gap(8).items_center().rounded(6).cursor_pointer()
        .hover((s) => s.bg(t.hover)).when(picked, (el) => el.bg(t.selected))
        .tooltip(needs.length ? `Asks for ${needs.join(", ")}: in the browser` : `${tr.name}: asks first`)
        .on_click((_e, cx) => this.askTransition(key, tr, cx))
        .child(svg(icon("move")).size(13).flex_none().text_color(t.dim))
        .child(div().flex_1().min_w(0).truncate().child(tr.name))
        .when(needs.length > 0, (el) => el.child(div().flex_none().text_size(11).text_color(t.ansi3).child("asks for more")))
        .child(statusPill(tr.to ?? { name: tr.name }, t)));
    }
    return [
      this.header(t, [this.backButton(t), div().flex_1().min_w(0).truncate().font_semibold().child(`Status of ${key}`), this.refreshButton(t)]),
      this.body("ji-status", rows),
    ];
  }

  // An input in its frame; lit while it has the keyboard.
  field(id, input, t) {
    return div().id(id).h(32).flex_none().px(10).flex().items_center().rounded(7).border_1()
      .border_color(this.typing === input ? t.accent : t.input).bg(t.background).cursor_text()
      .on_mouse_down("left", (_e, cx) => cx.stop_propagation())
      .child(Input.new(input).w_full().h_full().text_size(12.5).text_color(t.foreground));
  }

  area(id, input, t, height) {
    return div().id(id).h(height).flex_none().p(8).rounded(7).border_1()
      .border_color(this.typing === input ? t.accent : t.input).bg(t.background).cursor_text()
      .on_mouse_down("left", (_e, cx) => cx.stop_propagation())
      .child(Textarea.new(input).w_full().h_full().text_size(12.5).text_color(t.foreground));
  }

  labeled(label, hint, control, t) {
    return v_flex().gap(4).flex_none()
      .child(h_flex().gap(8).items_end()
        .child(div().flex_none().text_size(12).font_medium().child(label))
        .when(Boolean(hint), (el) => el.child(div().flex_1().min_w(0).text_size(11).text_color(t.dim).child(hint))))
      .child(control);
  }

  formError(t) {
    return this.form?.error ? div().id("ji-form-error").text_size(12).text_color(t.blocked).child(this.form.error) : null;
  }

  renderComment(view, t) {
    const form = this.form, issue = this.issueOf(view.key);
    const rows = [
      ...this.cards(t),
      v_flex().px(10).pt(12).gap(10)
        .child(div().text_size(12).text_color(t.muted).child(`${view.key}${issue ? `: ${oneLine(issue.fields?.summary)}` : ""}`))
        .child(form?.body ? this.area("ji-comment-body", form.body, t, 140) : this.line("No form.", t))
        .children([this.formError(t)].filter(Boolean))
        .child(div().text_size(11).text_color(t.dim).child(`Posted as ${this.meName() || "the token's user"}, for everyone who may see ${view.key}.`))
        .child(h_flex().pt(2).gap(8).justify_end()
          .child(pill("ji-comment-no", "Cancel", t, { onClick: (cx) => this.back(cx) }))
          .child(pill("ji-comment-yes", "Add comment", t, { tone: "solid", icon: "message", busy: this.busy === "comment", onClick: (cx) => this.sendComment(cx) }))),
    ];
    return [
      this.header(t, [this.backButton(t), div().flex_1().min_w(0).truncate().font_semibold().child(`Comment on ${view.key}`), div().flex_none().pr(6).text_size(11).text_color(t.dim).child("Esc cancels")]),
      this.body("ji-comment", rows),
    ];
  }

  renderWorklog(view, t) {
    const form = this.form, issue = this.issueOf(view.key);
    const spent = form?.time ? parseTimeSpent(form.time.value()) : null;
    const rows = [
      ...this.cards(t),
      v_flex().px(10).pt(12).gap(12)
        .child(div().text_size(12).text_color(t.muted).child(`${view.key}${issue ? `: ${oneLine(issue.fields?.summary)}` : ""}`))
        .when(Boolean(form?.time), (el) => el
          .child(this.labeled("Time spent", "such as 1h 30m, 45m or 1d", this.field("ji-work-time", form.time, t), t))
          .child(this.labeled("Date", `the day it was done; today is ${dayOf(Date.now())}`, this.field("ji-work-day", form.day, t), t))
          .child(this.labeled("Comment", "optional", this.area("ji-work-note", form.note, t, 76), t)))
        .children([this.formError(t)].filter(Boolean))
        .child(div().text_size(11).text_color(t.dim).child(`Logged as ${this.meName() || "the token's user"}.`))
        .child(h_flex().pt(2).gap(8).justify_end()
          .child(pill("ji-work-no", "Cancel", t, { onClick: (cx) => this.back(cx) }))
          .child(pill("ji-work-yes", spent ? `Log ${spent}` : "Log work", t, { tone: "solid", icon: "clock", busy: this.busy === "worklog", onClick: (cx) => this.logWork(cx) }))),
    ];
    return [
      this.header(t, [this.backButton(t), div().flex_1().min_w(0).truncate().font_semibold().child(`Log work on ${view.key}`), div().flex_none().pr(6).text_size(11).text_color(t.dim).child("Enter logs · Esc cancels")]),
      this.body("ji-worklog", rows),
    ];
  }

  renderSettings(t) {
    const s = this.settings, first = !this.client, path = settingsPath().replace(home(), "~");
    const typed = normalizeBase(this.inputs.url.value());
    const open = /^http:\/\//i.test(typed) && !/^(localhost|127\.0\.0\.1|\[::1\])(:\d+)?$/i.test(hostOf(typed));
    const rows = [
      v_flex().px(10).pt(12).gap(12)
        .child(div().text_size(12).text_color(t.muted).child(first
          ? "Give this panel a Jira personal access token (Jira: your profile, Personal Access Tokens). The token's Jira user decides what the panel may do: one that may only read gets a panel that only reads."
          : "The token's Jira user decides what this panel may do: one that may only read gets a panel that only reads."))
        .child(this.labeled("Jira URL", open ? "http:// sends the token unencrypted" : "", this.field("ji-set-url", this.inputs.url, t), t))
        .child(this.labeled("Personal access token", "", this.field("ji-set-token", this.inputs.token, t), t))
        .child(this.labeled("Board", "a board's id, or a project's key to find its board", this.field("ji-set-board", this.inputs.board, t), t))
        .when(Boolean(this.check), (el) => el.child(h_flex().id("ji-check").gap(6).items_start().text_size(12).text_color(this.check.ok ? t.ansi2 : t.blocked)
          .when(this.check.ok, (row) => row.child(svg(icon("check")).size(13).mt(1).flex_none()))
          .child(div().flex_1().min_w(0).child(this.check.text))))
        .child(h_flex().pt(2).gap(8).justify_end()
          .when(!first, (el) => el.child(pill("ji-set-cancel", "Cancel", t, { onClick: (cx) => this.back(cx) })))
          .child(pill("ji-set-check", "Check", t, { busy: this.busy === "check", disabled: this.busy === "save", tip: "Asks Jira who this token is and finds the board, saving nothing", onClick: (cx) => this.verify(cx, false) }))
          .child(pill("ji-set-save", "Save", t, { tone: "solid", busy: this.busy === "save", disabled: this.busy === "check", onClick: (cx) => this.save(cx) })))
        .child(div().text_size(11).text_color(t.dim).child(`Kept in ${path}, which only you can read.${s?.source === "env" ? " These came from JIRA_URL and JIRA_API_TOKEN; Save keeps them here." : ""}`)),
    ];
    return [
      this.header(t, [
        first ? svg(icon("jira")).size(16).ml(4).flex_none().text_color(t.muted) : this.backButton(t),
        div().flex_1().min_w(0).truncate().font_semibold().child(first ? "Jira" : "Jira settings"),
      ]),
      this.body("ji-settings-body", rows),
    ];
  }

  // --- render --------------------------------------------------------------------

  // Esc goes back a view (or drops the question); Tab moves between a form's fields.
  key(e, cx) {
    const view = this.view(), forms = ["comment", "worklog", "settings"];
    if (e.key === "escape") {
      if (this.confirm) this.confirm = null;
      else if (!this.back(cx)) return;
    } else if (e.key === "tab" && this.order.length && forms.includes(view.kind)) {
      const at = this.order.indexOf(this.typing), step = e.modifiers?.shift ? -1 : 1;
      this.order[(at + step + this.order.length) % this.order.length].focus();
    } else if (e.key === "enter" && this.confirm && !this.typing) {
      if (this.confirm.kind === "transition") this.transition(cx);
      else this.assign(cx);
    } else return;
    cx.stop_propagation();
    cx.notify();
  }

  render() {
    const t = theme(), now = Date.now(), view = this.view();
    this.staleShown = false;
    const parts = !this.settings ? [this.line("Reading the settings…", t)]
      : view.kind === "settings" ? this.renderSettings(t)
        : view.kind === "issue" ? this.renderIssue(view, t, now)
          : view.kind === "status" ? this.renderStatus(view, t, now)
            : view.kind === "comment" ? this.renderComment(view, t)
              : view.kind === "worklog" ? this.renderWorklog(view, t)
                : this.renderList(t, now);
    return v_flex().id("jira").track_focus(this.handle).size_full().min_h(0).overflow_hidden().bg(t.background)
      .when(this.panel, (el) => el.rounded(10).border_1().border_color(t.border)).text_color(t.foreground).text_size(13)
      .on_key_down((e, cx) => this.key(e, cx))
      .children(parts);
  }
}
