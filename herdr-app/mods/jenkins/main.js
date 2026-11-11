// Jenkins in the Herdr app: the jobs of the folders you name, each with its
// last build's state under a heading that folds, a job's Pipeline stages
// (of its newest build, or of one picked from its recent builds) and each
// stage's part of the console, a build's whole console log (followed while it
// runs), and two actions, build (with a form for its parameters) and
// abort, each asked about first. It reaches Jenkins with an API token alone
// (api.js), so the token's Jenkins user decides what it may do: what Jenkins
// refuses shows as "This token may not …", and nothing works around it.
// One view at a time, in a stack: the list, a folder, a job, a stage, a
// build, the build form, the settings. Given `{ panel: true }` as its props
// (a side panel of the layout, which install.sh --panel puts there), it paints
// no background of its own, so the panel's shows, as in the sidebar. The app
// gives a block's props through `props()`, not as init's argument. Standalone: no other mod and no CLI is needed.

import { View, div, svg } from "gpui-kit";
import { v_flex, h_flex, InputState, Input, Scrollbar, v_virtual_list, VirtualListScrollHandle } from "gpui-base";
import { theme, home, nextCommand, settings as look, shared, setShared, props as given } from "herdr";
import { Client, loadSettings, saveSettings, settingsPath, ready } from "./api.js";
import {
  parsePlaces, childOf, leaf, hostOf, normalizeBase, isFolder, jobState, buildState, progress, ago, took, span,
  causeOf, buildParams, paramFields, formBody, emptyLog, appendLog, logLines, explain, oneLine, LABEL,
  envOf, isPipeline, stagesOf, stageState, stageTook, stageProgress, stageRange, STAGE_LABEL,
} from "./util.js";
import { icon, spinner, look as stateLook, mark, bar, iconButton, pill } from "./ui.js";

const LIST_EVERY = 60000, LIST_BUSY = 10000; // the lists: at rest, and while a build runs or waits
const JOB_EVERY = 30000, JOB_BUSY = 5000;
const ROW = 18; // a console line's height

const RUNS_EVERY = 60000; // a job's last runs, for what each stage took last time
const FOLDED = "jenkins.collapsed"; // the headings folded, kept by the app across restarts

const active = (items) => (items ?? []).some((j) => ["running", "queued"].includes(jobState(j)));

export default class Jenkins extends View {
  init(initial, cx) {
    let layout = null;
    try { layout = given(); } catch { /* shown where it has no props */ }
    this.panel = Boolean((layout ?? initial)?.panel);
    this.handle = cx.focus_handle();
    this.scroll = VirtualListScrollHandle.new();
    this.settings = null; // null until read
    this.client = null;
    this.who = null; this.whoError = "";
    this.places = [];
    this.lists = new Map(); // place path → { items, error, loading, at }
    this.jobs = new Map(); // job path → { data, error, loading, at }
    this.stages = new Map(); // "<job path>/<number>" → { data, error, missing, loading, at }: wfapi/describe
    this.runs = new Map(); // job path → { runs, loading, at }: wfapi/runs
    this.folded = new Set(); // the paths of the headings folded
    try { this.folded = new Set((shared(FOLDED) ?? []).filter((p) => typeof p === "string")); } catch { /* none kept */ }
    // The views over the list: { kind: "folder" | "job" | "stage" | "build" | "trigger" | "settings", place, number, stage }.
    // A job's `number` is the build picked (none: its newest); a stage's `stage` is { id, name, nth }.
    this.stack = [];
    this.log = null; // the console of the stage or build view: { place, number, build, text, error, loading, done, follow }
    this.form = null; // the build form: { place, fields, values, inputs }
    this.confirm = null; // an abort being asked about: { place, number }
    this.notice = null; // what an action came to: { tone: "ok" | "error", text }
    this.busy = null; // "save" | "check" | "trigger" | "abort"
    this.check = null; // the settings' check: { ok, text }
    this.gen = 0; // answers for settings that have since changed are dropped
    this.inputs = {
      url: InputState.new({ placeholder: "https://jenkins.example.com" }),
      user: InputState.new({ placeholder: "Your Jenkins user name" }),
      token: InputState.new({ placeholder: "An API token of that user" }),
      folders: InputState.new({ placeholder: "All of the top level" }),
    };
    this.inputs.token.set_masked(true);
    this.order = Object.values(this.inputs); // Tab's order in the form on screen
    for (const input of this.order) this.wire(input, (cx) => this.save(cx));

    cx.spawn(async (cx) => {
      let s;
      try { s = await loadSettings(); } catch { s = { url: "", user: "", token: "", folders: "", source: "none" }; }
      this.apply(s, cx);
      cx.notify();
    });
    cx.timer.every(2000, (cx) => this.tick(cx));
    // `jenkins.refresh` and `jenkins.settings`, for a key in keys.json.
    cx.spawn(async (cx) => {
      for (;;) {
        let command;
        try { command = await nextCommand(); } catch { return; } // no more commands: the copy is going
        if (command === "refresh") this.refresh(cx, true);
        else if (command === "settings") this.show({ kind: "settings" }, cx);
        cx.notify();
      }
    });
  }

  // An input of a form: Enter submits it, and it knows when it has the keyboard.
  wire(input, submit) {
    input.on("submit", (_e, cx) => submit(cx));
    input.on("change", (_e, cx) => cx.notify());
    input.on("focus", (_e, cx) => { this.typing = input; cx.notify(); });
    input.on("blur", (_e, cx) => { if (this.typing === input) this.typing = null; cx.notify(); });
  }

  // --- settings ------------------------------------------------------------------

  // Takes settings into use: a client for them, and nothing kept of the last ones'.
  apply(s, cx) {
    this.gen += 1;
    this.settings = s;
    this.client = ready(s) ? new Client(s) : null;
    this.places = parsePlaces(s.folders, normalizeBase(s.url));
    this.who = null; this.whoError = "";
    this.lists = new Map(); this.jobs = new Map(); this.stages = new Map(); this.runs = new Map();
    this.log = null; this.form = null; this.confirm = null;
    this.stack = this.stack.filter((v) => v.kind === "settings");
    for (const key of Object.keys(this.inputs)) this.inputs[key].set_value(s[key] ?? "");
    if (this.client) this.refresh(cx, true);
  }

  typed() {
    return {
      url: normalizeBase(this.inputs.url.value()), user: this.inputs.user.value().trim(),
      token: this.inputs.token.value().trim(), folders: this.inputs.folders.value().trim(),
    };
  }

  // Asks Jenkins who the typed user and token are, saving nothing.
  verify(cx, then) {
    const s = this.typed();
    if (!ready(s)) {
      this.check = { ok: false, text: "The Jenkins URL, the user name and the token are all needed." };
      cx.notify();
      return;
    }
    if (this.busy) return;
    this.busy = then ? "save" : "check";
    this.check = null;
    cx.spawn(async (cx) => {
      let saved = true;
      try {
        if (then) await saveSettings(s);
      } catch (e) {
        saved = false;
        this.check = { ok: false, text: `Could not save the settings: ${e?.message ?? e}` };
      }
      if (saved) {
        try {
          const who = await new Client(s).whoAmI();
          this.check = { ok: true, text: `Authenticated as ${who.name === who.id ? who.id : `${who.name} (${who.id})`}` };
        } catch (e) {
          this.check = { ok: false, text: explain(e, "ask who this token is", s.user) };
        }
        if (then) {
          this.apply({ ...s, source: "file" }, cx);
          // Saved and accepted: on to the jobs. Refused: stay, with the reason.
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

  view() {
    if (this.settings && !this.client) return { kind: "settings" };
    return this.stack.at(-1) ?? { kind: "list" };
  }

  // The Jenkins user the token belongs to: as Jenkins names it, else as typed.
  user() { return this.who?.id ?? this.settings?.user ?? ""; }

  refresh(cx, all = false) {
    if (!this.client) return;
    const gen = this.gen, client = this.client;
    if (!this.who && !this.checking) {
      this.checking = true;
      cx.spawn(async (cx) => {
        try { const who = await client.whoAmI(); if (gen === this.gen) { this.who = who; this.whoError = ""; } }
        catch (e) { if (gen === this.gen) this.whoError = explain(e, "ask who this token is", this.settings.user); }
        this.checking = false;
        cx.notify();
      });
    }
    const view = this.view();
    for (const place of this.places) this.loadList(place, cx);
    for (const v of this.stack) if (v.kind === "folder") this.loadList(v.place, cx);
    if (all && (view.kind === "job" || view.kind === "trigger")) this.loadJob(view.place, cx);
    if (all && view.kind === "stage") this.loadStages(view.place, view.number, cx, true);
    if (all && (view.kind === "build" || view.kind === "stage") && this.log) { this.log.done = false; this.pollLog(cx); }
  }

  loadList(place, cx) {
    const had = this.lists.get(place.path) ?? { items: null, error: "" };
    if (had.loading || !this.client) return;
    const gen = this.gen, client = this.client;
    this.lists.set(place.path, { ...had, loading: true });
    cx.spawn(async (cx) => {
      let next;
      try { next = { items: await client.list(place), error: "" }; }
      catch (e) { next = { items: had.items, error: explain(e, `list the jobs of ${place.label || "Jenkins"}`, this.user()) }; }
      if (gen === this.gen) this.lists.set(place.path, { ...next, loading: false, at: Date.now() });
      cx.notify();
    });
  }

  loadJob(place, cx) {
    const had = this.jobs.get(place.path) ?? { data: null, error: "" };
    if (had.loading || !this.client) return;
    const gen = this.gen, client = this.client;
    this.jobs.set(place.path, { ...had, loading: true });
    cx.spawn(async (cx) => {
      let next;
      try { next = { data: await client.job(place), error: "" }; }
      catch (e) { next = { data: had.data, error: explain(e, `read ${leaf(place.label)}`, this.user()) }; }
      if (gen !== this.gen) return;
      this.jobs.set(place.path, { ...next, loading: false, at: Date.now() });
      // The stages of the build the job view shows, once the job says which that is.
      const view = this.view();
      if (view.kind === "job" && view.place.path === place.path) this.loadPicked(view, cx);
      cx.notify();
    });
  }

  // The build a job view shows: the one picked from its builds, else its newest.
  picked(view) {
    const builds = this.jobs.get(view.place.path)?.data?.builds ?? [];
    return builds.find((b) => b.number === view.number) ?? builds[0] ?? null;
  }

  // Reads the stages of the build a job view shows when they are due: once
  // for a finished build, every few seconds while it runs.
  loadPicked(view, cx, force = false) {
    const job = this.jobs.get(view.place.path)?.data, build = this.picked(view);
    if (!build || !isPipeline(job)) return;
    const entry = this.stages.get(`${view.place.path}/${build.number}`);
    if (force || this.stagesDue(entry, build.building)) this.loadStages(view.place, build.number, cx, force);
  }

  // Stages are read again while their build runs (or, read while it ran, has not been seen to end).
  stagesDue(entry, building) {
    if (!entry) return true;
    if (entry.loading || entry.missing) return false;
    const live = building || ["IN_PROGRESS", "PAUSED_PENDING_INPUT", "QUEUED"].includes(entry.data?.status);
    return live ? Date.now() - (entry.at ?? 0) >= JOB_BUSY : !entry.data && !entry.error;
  }

  loadStages(place, number, cx, force = false) {
    const key = `${place.path}/${number}`, had = this.stages.get(key) ?? { data: null, error: "" };
    if (had.loading || !this.client || (had.missing && !force)) return;
    const gen = this.gen, client = this.client;
    this.stages.set(key, { ...had, loading: true });
    cx.spawn(async (cx) => {
      let next;
      try { next = { data: await client.stages(place, number), error: "", missing: false }; }
      catch (e) {
        next = e.kind === "missing" ? { data: null, error: "", missing: true }
          : { data: had.data, error: explain(e, `read the stages of #${number}`, this.user()), missing: false };
      }
      if (gen !== this.gen) return;
      this.stages.set(key, { ...next, loading: false, at: Date.now() });
      if (next.data) this.loadRuns(place, cx);
      cx.notify();
    });
  }

  // The job's last runs, for what each stage took last time: read now and
  // then once a minute, while its stages are shown.
  loadRuns(place, cx) {
    const had = this.runs.get(place.path);
    if (had?.loading || !this.client || (had && Date.now() - had.at < RUNS_EVERY)) return;
    const gen = this.gen, client = this.client;
    this.runs.set(place.path, { runs: had?.runs ?? [], loading: true, at: had?.at ?? 0 });
    cx.spawn(async (cx) => {
      let runs = had?.runs ?? [];
      try { runs = await client.runs(place); } catch { /* only an estimate: none */ }
      if (gen === this.gen) this.runs.set(place.path, { runs, loading: false, at: Date.now() });
      cx.notify();
    });
  }

  // Reads the build and what its console has written since the last read. A
  // read starts at the byte the last one ended at, counted from the text
  // itself (the app's fetch shows no headers); a log with bytes that are not
  // UTF-8 can put that count off while it runs, so a build that was followed
  // is read once more, whole, when it ends.
  pollLog(cx) {
    const log = this.log;
    if (!log || log.loading || log.done || !this.client) return;
    const client = this.client;
    log.loading = true;
    cx.spawn(async (cx) => {
      try {
        const build = await client.build(log.place, log.number);
        const chunk = await client.logFrom(log.place, log.number, log.text.offset);
        log.build = build;
        log.text = appendLog(log.text, chunk);
        if (build.building) log.followed = true;
        else {
          if (log.followed) log.text = appendLog(emptyLog(), await client.logFrom(log.place, log.number, 0));
          log.done = true;
        }
        log.error = "";
      } catch (e) {
        log.error = explain(e, "read this build's log", this.user());
        if (e.kind !== "network") log.done = true;
      }
      log.loading = false;
      if (this.log === log && log.follow) this.toEnd(cx);
      cx.notify();
    });
  }

  // Shows the log's last line: now, and once more when the lines just added
  // have been laid out (the list scrolls to the end it knew of).
  toEnd(cx) {
    this.scroll.scroll_to_bottom();
    cx.timer.after(80, (cx) => { if (this.log?.follow) { this.scroll.scroll_to_bottom(); cx.notify(); } });
  }

  // Every two seconds: reads again what is due. A list while a build runs or
  // waits is read more often than one at rest; a followed log every time.
  tick(cx) {
    if (!this.client) return;
    const now = Date.now(), view = this.view();
    const due = (entry, rest, busy, isBusy) => !entry?.loading && now - (entry?.at ?? 0) >= (isBusy ? busy : rest);
    for (const place of [...this.places, ...this.stack.filter((v) => v.kind === "folder").map((v) => v.place)]) {
      const entry = this.lists.get(place.path);
      if (due(entry, LIST_EVERY, LIST_BUSY, active(entry?.items))) this.loadList(place, cx);
    }
    if (view.kind === "job") {
      const entry = this.jobs.get(view.place.path), job = entry?.data;
      if (due(entry, JOB_EVERY, JOB_BUSY, Boolean(job?.inQueue || (job?.builds ?? []).some((b) => b.building)))) this.loadJob(view.place, cx);
      this.loadPicked(view, cx);
    }
    if (view.kind === "stage" && this.stagesDue(this.stages.get(`${view.place.path}/${view.number}`), Boolean(this.log?.build?.building))) {
      this.loadStages(view.place, view.number, cx);
    }
    if (view.kind === "build" || view.kind === "stage") this.pollLog(cx);
  }

  // --- moving about --------------------------------------------------------------

  show(view, cx) {
    if (view.kind === "settings" && this.view().kind === "settings") return;
    this.stack.push(view);
    this.confirm = null;
    if (this.notice?.tone === "ok") this.notice = null;
    if (view.kind === "folder") this.loadList(view.place, cx);
    if (view.kind === "job") { this.loadJob(view.place, cx); this.loadPicked(view, cx); }
    if (view.kind === "settings") { this.check = null; this.order = Object.values(this.inputs); }
    if (view.kind === "build" || view.kind === "stage") {
      const known = (this.jobs.get(view.place.path)?.data?.builds ?? []).find((b) => b.number === view.number) ?? null;
      this.log = { place: view.place, number: view.number, build: known, text: emptyLog(), error: "", loading: false, done: false, follow: true, followed: false };
      this.pollLog(cx);
    }
    if (view.kind === "stage") this.loadStages(view.place, view.number, cx);
    this.handle.focus();
    cx.notify();
  }

  back(cx) {
    const left = this.stack.pop();
    if (!left) return false;
    if (left.kind === "build" || left.kind === "stage") this.log = null;
    if (left.kind === "trigger") this.form = null;
    this.confirm = null;
    const now = this.view();
    if (now.kind === "job") this.loadJob(now.place, cx);
    this.handle.focus();
    cx.notify();
    return true;
  }

  // Shows the stages of another of the job's builds, in the same view.
  pick(place, number, cx) {
    const view = this.view();
    if (view.kind !== "job" || view.place.path !== place.path) return;
    view.number = number;
    this.loadPicked(view, cx);
    cx.notify();
  }

  // From a stage's part of the console to all of it, which is read already:
  // the stage view becomes the build's, and Esc goes back to the job.
  wholeLog(cx) {
    const view = this.view();
    if (view.kind !== "stage") return;
    this.stack[this.stack.length - 1] = { kind: "build", place: view.place, number: view.number };
    if (this.log) { this.log.follow = true; this.toEnd(cx); }
    cx.notify();
  }

  // --- acting --------------------------------------------------------------------

  // The build form: the job's parameters, each preset to its default.
  askBuild(place, cx) {
    const job = this.jobs.get(place.path)?.data;
    if (!job) return;
    const fields = paramFields(job), values = {}, inputs = {};
    for (const f of fields) {
      values[f.name] = f.value;
      if (f.kind === "text" || f.kind === "password") {
        inputs[f.name] = InputState.new({ value: f.value, placeholder: f.kind === "password" ? "Empty: the job's default" : "" });
        if (f.kind === "password") inputs[f.name].set_masked(true);
        this.wire(inputs[f.name], (cx) => this.build(cx));
      }
    }
    this.form = { place, fields, values, inputs };
    this.order = Object.values(inputs);
    this.show({ kind: "trigger", place }, cx);
  }

  build(cx) {
    const form = this.form;
    if (!form || this.busy || !this.client) return;
    const client = this.client, name = leaf(form.place.label);
    for (const [key, input] of Object.entries(form.inputs)) form.values[key] = input.value();
    this.busy = "trigger";
    cx.spawn(async (cx) => {
      try {
        await client.trigger(form.place, form.fields.length ? formBody(form.fields, form.values) : null);
        this.notice = { tone: "ok", text: `Queued a build of ${name}.` };
        if (this.form === form) { this.stack.pop(); this.form = null; }
        this.loadJob(form.place, cx);
        for (const place of this.places) this.loadList(place, cx);
      } catch (e) {
        this.notice = { tone: "error", text: explain(e, `build ${name}`, this.user()) };
      }
      this.busy = null;
      cx.notify();
    });
    cx.notify();
  }

  askAbort(place, number, cx) {
    this.confirm = { place, number };
    cx.notify();
  }

  abort(cx) {
    const asked = this.confirm;
    if (!asked || this.busy || !this.client) return;
    const client = this.client, what = `#${asked.number} of ${leaf(asked.place.label)}`;
    this.busy = "abort";
    cx.spawn(async (cx) => {
      try {
        await client.abort(asked.place, asked.number);
        this.notice = { tone: "ok", text: `Asked Jenkins to abort ${what}.` };
        this.loadJob(asked.place, cx);
        if (this.log) { this.log.done = false; this.pollLog(cx); }
      } catch (e) {
        this.notice = { tone: "error", text: explain(e, `abort ${what}`, this.user()) };
      }
      this.confirm = null;
      this.busy = null;
      cx.notify();
    });
    cx.notify();
  }

  open(url, cx) {
    try { cx.open_url(url); } catch (e) { this.notice = { tone: "error", text: `Could not open ${url}: ${e?.message ?? e}` }; cx.notify(); }
  }

  // --- pieces --------------------------------------------------------------------

  line(text, t, color) {
    return div().px(10).py(6).text_size(12).text_color(color ?? t.dim).child(text);
  }

  header(t, parts) {
    return h_flex().h(42).flex_none().pl(8).pr(8).gap(6).items_center().border_b(1).border_color(t.border).children(parts.filter(Boolean));
  }

  backButton(t) {
    return iconButton("jk-back", "back", "Back (Esc)", (cx) => this.back(cx), t, { size: 26 });
  }

  refreshButton(t) {
    const loading = [...this.lists.values(), ...this.jobs.values()].some((e) => e.loading) || Boolean(this.log?.loading && !this.log.build);
    return div().id("jk-refresh").size(24).flex_none().flex().items_center().justify_center().rounded(6).text_color(t.dim)
      .cursor_pointer().hover((s) => s.bg(t.hover).text_color(t.foreground)).tooltip("Refresh")
      .on_click((_e, cx) => { this.refresh(cx, true); cx.notify(); })
      .child(loading ? spinner(t, 13) : svg(icon("refresh")).size(14));
  }

  // What an action came to, until dismissed; and the abort being asked about.
  cards(t) {
    const out = [];
    if (this.notice) {
      const color = this.notice.tone === "ok" ? t.ansi2 : t.blocked;
      out.push(h_flex().id("jk-notice").flex_none().mx(10).mt(8).p(8).gap(8).items_start().rounded(7).border_1().border_color(color).bg(t.surface).text_size(12)
        .child(div().flex_1().min_w(0).text_color(this.notice.tone === "ok" ? t.foreground : t.blocked).child(this.notice.text))
        .child(iconButton("jk-dismiss", "x", "Dismiss", (cx) => { this.notice = null; cx.notify(); }, t, { size: 20 })));
    }
    if (this.confirm) {
      const { place, number } = this.confirm;
      out.push(v_flex().id("jk-confirm").flex_none().mx(10).mt(8).p(8).gap(8).rounded(7).border_1().border_color(t.border).bg(t.surface).text_size(12)
        .child(div().child(`Abort #${number} of ${leaf(place.label)}? Jenkins stops the build where it is.`))
        .child(h_flex().gap(6).justify_end()
          .child(pill("jk-abort-no", "Keep running", t, { onClick: (cx) => { this.confirm = null; cx.notify(); } }))
          .child(pill("jk-abort-yes", "Abort", t, { tone: "danger", icon: "stop", busy: this.busy === "abort", onClick: (cx) => this.abort(cx) }))));
    }
    return out;
  }

  // A job's or a build's numbers at the right end of its row.
  meter(build, t, now) {
    const part = progress(build, now);
    return h_flex().flex_none().gap(6).items_center().text_size(11).text_color(t.dim)
      .when(part !== null, (el) => el.child(bar(part, t)).child(div().flex_none().text_color(t.working).child(`${Math.round(part * 100)}%`)))
      .when(part === null, (el) => el.child(div().flex_none().text_color(t.working).child(took(build, now))));
  }

  jobRow(place, item, t, now) {
    const at = childOf(place, item.name), id = `jk-item-${at.path}`;
    if (isFolder(item)) {
      return h_flex().id(id).h(30).flex_none().px(8).gap(8).items_center().rounded(6).cursor_pointer().hover((s) => s.bg(t.hover))
        .on_click((_e, cx) => this.show({ kind: "folder", place: at }, cx))
        .child(svg(icon("folder")).size(14).flex_none().text_color(t.muted))
        .child(div().flex_1().min_w(0).truncate().child(item.name))
        .child(svg(icon("chevron-right")).size(12).flex_none().text_color(t.dim));
    }
    const state = jobState(item), last = item.lastBuild, quiet = state === "disabled" || state === "notbuilt";
    const right = state === "running" && last?.building ? h_flex().flex_none().gap(6).items_center().child(div().flex_none().text_size(11).text_color(t.dim).child(`#${last.number}`)).child(this.meter(last, t, now))
      : div().flex_none().text_size(11).text_color(state === "queued" ? t.ansi3 : t.dim)
        .child(state === "queued" ? "queued" : state === "disabled" ? "disabled" : last ? `#${last.number} · ${ago(last.timestamp, now)}` : "never built");
    return h_flex().id(id).h(30).flex_none().px(8).gap(8).items_center().rounded(6).cursor_pointer().hover((s) => s.bg(t.hover))
      .tooltip(`${at.label}: ${LABEL[state]}${last && !last.building ? `, #${last.number} took ${span(last.duration)}` : ""}`)
      .on_click((_e, cx) => this.show({ kind: "job", place: at }, cx))
      .child(mark(state, t))
      .child(div().flex_1().min_w(0).truncate().text_color(quiet ? t.muted : t.foreground).child(item.name))
      .child(right);
  }

  // Folds a heading of the list, or unfolds it; the app keeps which are folded.
  fold(place, cx) {
    if (this.folded.has(place.path)) this.folded.delete(place.path);
    else this.folded.add(place.path);
    try { setShared(FOLDED, [...this.folded]); } catch { /* kept until the panel starts over */ }
    cx.notify();
  }

  // "DEV" or "PROD" beside a heading, when the folder's name says which.
  envTag(env, t) {
    const color = env === "prod" ? t.ansi3 : t.ansi4;
    return div().flex_none().h(18).px(6).flex().items_center().rounded(5).border_1().border_color(color)
      .text_size(10.5).font_semibold().text_color(color).child(env.toUpperCase());
  }

  // A folder of the settings' list: its name, large, and a click folds it.
  // Folded, it still says how many of its jobs run and how many failed.
  heading(place, items, t, title) {
    const folded = this.folded.has(place.path), env = envOf(place.label);
    const count = (state) => (items ?? []).filter((j) => jobState(j) === state).length;
    const running = count("running"), failed = count("failed");
    // The name and its tag keep their width; in a narrow panel the counts give way first.
    return h_flex().id(`jk-head-${place.path}`).h(46).flex_none().mt(4).pl(4).pr(4).gap(8).items_center().rounded(7).overflow_hidden()
      .cursor_pointer().hover((s) => s.bg(t.hover)).tooltip(folded ? "Show its jobs" : "Fold it away")
      .on_click((_e, cx) => this.fold(place, cx))
      .child(svg(icon(folded ? "chevron-right" : "chevron-down")).size(16).flex_none().text_color(t.muted))
      .child(div().flex_none().text_size(18).font_semibold().child(title))
      .when(Boolean(env), (el) => el.child(this.envTag(env, t)))
      .child(h_flex().flex_1().min_w(0).overflow_hidden().gap(8).items_center().justify_end()
        .when(running > 0, (el) => el.child(h_flex().id(`jk-head-running-${place.path}`).flex_none().gap(3).items_center().text_size(12).text_color(t.working)
          .tooltip(`${running} running`).child(spinner(t, 12)).child(String(running))))
        .when(failed > 0, (el) => el.child(h_flex().id(`jk-head-failed-${place.path}`).flex_none().gap(3).items_center().text_size(12).text_color(t.ansi1)
          .tooltip(`${failed} failed last time`).child(svg(icon("failed")).size(12)).child(String(failed))))
        .when(Boolean(items), (el) => el.child(div().flex_none().text_size(12).text_color(t.dim).child(String(items.length))))
        .child(iconButton(`jk-open-${place.path}`, "external", "Open in the browser", (cx) => this.open(this.client.page(place), cx), t, { size: 22 })));
  }

  // A place's header and its rows: a large heading that folds for the
  // folders of the list (`big`), a small one for a folder opened from it.
  section(place, t, now, title, big = false) {
    const entry = this.lists.get(place.path), items = entry?.items;
    const rows = [big ? this.heading(place, items, t, title) : h_flex().h(28).flex_none().pt(6).px(8).gap(6).items_center().text_size(11).font_medium().text_color(t.dim)
      .child(div().min_w(0).truncate().child(title))
      .when(Boolean(items), (el) => el.child(div().flex_none().child(String(items.length))))
      .child(div().flex_1())
      .child(iconButton(`jk-open-${place.path}`, "external", "Open in the browser", (cx) => this.open(this.client.page(place), cx), t, { size: 20 }))];
    if (big && this.folded.has(place.path)) {
      if (entry?.error) rows.push(this.line(entry.error, t, t.blocked));
      return rows;
    }
    if (entry?.error) rows.push(this.line(entry.error, t, t.blocked));
    if (!items) { if (!entry?.error) rows.push(this.line("Reading the jobs…", t)); }
    else if (!items.length) rows.push(this.line("No jobs here, or none this token may see.", t));
    else rows.push(...items.map((item) => this.jobRow(place, item, t, now)));
    return rows;
  }

  body(id, rows) {
    return v_flex().id(id).flex_1().min_h(0).items_center().overflow_y_scrollbar()
      .child(v_flex().w_full().max_w(820).px(6).pb(12).children(rows));
  }

  // --- views ---------------------------------------------------------------------

  renderList(t, now) {
    const host = hostOf(this.settings.url);
    const who = this.who ? (this.who.name === this.who.id ? this.who.id : `${this.who.name} (${this.who.id})`) : "";
    const rows = [...this.cards(t)];
    if (this.whoError) rows.push(this.line(this.whoError, t, t.blocked));
    for (const place of this.places) rows.push(...this.section(place, t, now, place.label || host, true));
    return [
      this.header(t, [
        svg(icon("jenkins")).size(16).ml(4).flex_none().text_color(t.muted),
        div().flex_none().font_semibold().child("Jenkins"),
        div().id("jk-who").flex_1().min_w(0).truncate().text_size(11).text_color(t.dim)
          .when(Boolean(who), (el) => el.tooltip(`Authenticated as ${who}: what this Jenkins user may do, this panel may do`))
          .child(who ? `${host} · ${this.who.id}` : host),
        this.refreshButton(t),
        iconButton("jk-settings", "settings", "Settings", (cx) => this.show({ kind: "settings" }, cx), t),
      ]),
      this.body("jk-list", rows),
    ];
  }

  renderFolder(view, t, now) {
    return [
      this.header(t, [
        this.backButton(t),
        svg(icon("folder")).size(14).flex_none().text_color(t.muted),
        div().flex_1().min_w(0).truncate().font_semibold().child(view.place.label),
        this.refreshButton(t),
      ]),
      this.body("jk-folder", [...this.cards(t), ...this.section(view.place, t, now, view.place.label)]),
    ];
  }

  // A build of the job view's list. A Pipeline job's build shows its stages
  // above the list when clicked (`picked`); a freestyle one's opens its log.
  buildRow(place, build, t, now, pipeline, picked) {
    const state = buildState(build), params = buildParams(build).map((p) => `${p.name}=${p.value}`).join(" · ");
    const id = `jk-build-${build.number}`;
    return h_flex().id(id).h(30).flex_none().px(8).gap(8).items_center().rounded(6).cursor_pointer().hover((s) => s.bg(t.hover))
      .when(picked, (el) => el.bg(t.selected))
      .tooltip(`#${build.number}: ${LABEL[state]}. Click for its ${pipeline ? "stages" : "log"}`)
      .on_click((_e, cx) => (pipeline ? this.pick(place, build.number, cx) : this.show({ kind: "build", place, number: build.number }, cx)))
      .child(mark(state, t))
      .child(div().flex_none().font_medium().child(`#${build.number}`))
      .child(div().flex_1().min_w(0).truncate().text_size(11.5).text_color(t.dim).child(oneLine(params || causeOf(build))))
      .when(build.building, (el) => el.child(this.meter(build, t, now))
        .child(iconButton(`${id}-abort`, "stop", "Abort this build…", (cx) => this.askAbort(place, build.number, cx), t, { size: 22, danger: true })))
      .when(!build.building, (el) => el.child(div().flex_none().text_size(11).text_color(t.dim).child(`${took(build, now)} · ${ago(build.timestamp, now)}`)));
  }

  // A stage of a build: its state, name and time, a bar while it runs (by
  // what it took last time), and why it failed. A click shows its log.
  stageRow(place, number, stage, t, now) {
    const { state } = stage, part = stageProgress(stage, now), id = `jk-stage-${stage.id ?? `next-${stage.name}`}`;
    const quiet = state === "pending" || state === "skipped";
    const time = state === "pending" ? (stage.expected > 0 ? `~${span(stage.expected)}` : "")
      : state === "paused" ? "waiting for input" : state === "skipped" ? "skipped" : stageTook(stage, now);
    const timeColor = state === "running" ? t.working : state === "paused" ? t.ansi3 : t.dim;
    const why = oneLine(stage.error?.message ?? "");
    return v_flex().id(id).flex_none().px(8).py(6).gap(2).rounded(6)
      .when(state !== "pending", (el) => el.cursor_pointer().hover((s) => s.bg(t.hover))
        .on_click((_e, cx) => this.show({ kind: "stage", place, number, stage: { id: stage.id, name: stage.name, nth: stage.nth } }, cx)))
      .tooltip(`${stage.name}: ${STAGE_LABEL[state] ?? state}${state === "pending" ? "" : ". Click for its log"}`)
      .child(h_flex().gap(8).items_center()
        .child(mark(state, t))
        .child(div().flex_1().min_w(0).truncate().text_color(quiet ? t.muted : t.foreground).when(state === "running", (el) => el.font_medium()).child(stage.name))
        .when(part !== null, (el) => el.child(bar(part, t, 36)))
        .child(div().flex_none().text_size(11).text_color(timeColor).child(time)))
      .when(Boolean(why), (el) => el.child(div().pl(22).truncate().text_size(11.5).text_color(state === "failed" ? t.ansi1 : t.muted).child(why)));
  }

  // The stages of the build a job view shows, and the way to its whole log.
  stagesPart(place, job, build, t, now) {
    const head = h_flex().h(30).flex_none().pt(8).px(8).gap(6).items_center().text_size(11).font_medium().text_color(t.dim)
      .child(div().flex_1().min_w(0).truncate().child(`${isPipeline(job) ? "Stages of" : "Build"} #${build.number}${build.building ? "" : ` · ${LABEL[buildState(build)]} · took ${took(build, now)}`}`))
      .child(h_flex().id("jk-console").flex_none().gap(4).items_center().px(6).h(22).rounded(6).cursor_pointer().text_color(t.accent)
        .hover((s) => s.bg(t.hover)).tooltip(`The console log of #${build.number}`)
        .on_click((_e, cx) => this.show({ kind: "build", place, number: build.number }, cx))
        .child(svg(icon("console")).size(12)).child("Console log"));
    if (!isPipeline(job)) return [head, this.line("This job is not a Pipeline, so it has no stages: its console log has it all.", t)];
    const entry = this.stages.get(`${place.path}/${build.number}`);
    if (entry?.missing) return [head, this.line("Jenkins shows no stages for this build (no Pipeline Stage View): its console log has it all.", t)];
    const rows = [head];
    if (entry?.error) rows.push(this.line(entry.error, t, t.blocked));
    if (!entry?.data) { if (!entry?.error) rows.push(this.line("Reading the stages…", t)); return rows; }
    const stages = stagesOf(entry.data, this.runs.get(place.path)?.runs, build.building);
    if (!stages.length) rows.push(this.line(build.building ? "No stage has started yet." : "This build ran no stages.", t));
    rows.push(...stages.map((s) => this.stageRow(place, build.number, s, t, now)));
    return rows;
  }

  renderJob(view, t, now) {
    const entry = this.jobs.get(view.place.path), job = entry?.data, name = leaf(view.place.label);
    const rows = [...this.cards(t)];
    if (entry?.error) rows.push(this.line(entry.error, t, t.blocked));
    if (!job) { if (!entry?.error) rows.push(this.line("Reading the job…", t)); }
    else {
      const builds = job.builds ?? [], picked = this.picked(view), pipeline = isPipeline(job);
      rows.push(v_flex().px(10).pt(10).pb(4).gap(3)
        .child(div().text_size(11).text_color(t.dim).child(view.place.label))
        .when(Boolean(oneLine(job.description)), (el) => el.child(div().text_size(12).text_color(t.muted).child(oneLine(job.description)))));
      if (job.inQueue) {
        rows.push(h_flex().h(30).flex_none().px(8).gap(8).items_center().text_size(12).text_color(t.ansi3)
          .child(mark("queued", t)).child("A build waits in the queue"));
      }
      if (picked) rows.push(...this.stagesPart(view.place, job, picked, t, now));
      rows.push(div().h(28).flex_none().pt(10).px(8).text_size(11).font_medium().text_color(t.dim).child(builds.length ? "Builds" : "Never built"));
      rows.push(...builds.map((b) => this.buildRow(view.place, b, t, now, pipeline, pipeline && b.number === picked?.number)));
    }
    return [
      this.header(t, [
        this.backButton(t),
        job ? mark(jobState({ ...job, lastBuild: job.builds?.[0] }), t) : null,
        div().flex_1().min_w(0).truncate().font_semibold().child(name),
        job ? pill("jk-build", "Build…", t, { icon: "play", disabled: job.buildable === false, tip: job.buildable === false ? "This job is disabled in Jenkins" : `Build ${name}: asks first`, onClick: (cx) => this.askBuild(view.place, cx) }) : null,
        this.refreshButton(t),
        iconButton("jk-job-open", "external", "Open in the browser", (cx) => this.open(this.client.page(view.place), cx), t),
      ]),
      this.body("jk-job", rows),
    ];
  }

  logRow(line, t) {
    const color = /^Finished: SUCCESS/.test(line) ? t.ansi2 : /^Finished: |\bERROR\b|\bFAIL(ED|URE)?\b|^\s*--- FAIL/.test(line) ? t.ansi1 : /^\[Pipeline\]|^Started by/.test(line) ? t.dim : t.foreground;
    return div().w_full().h(ROW).flex_none().px(12).truncate().text_color(color).child(line || " ");
  }

  // The stage a stage view shows, as its build's stages were read last.
  stageOf(view) {
    const stages = this.stages.get(`${view.place.path}/${view.number}`)?.data?.stages ?? [];
    let nth = -1;
    const found = stages.find((s) => s.id === view.stage.id) ?? stages.find((s) => s.name === view.stage.name && ++nth === view.stage.nth);
    return found ? { ...found, state: stageState(found.status) } : null;
  }

  // A build's console log, or (a stage view) the part of it one stage wrote.
  renderBuild(view, t, now) {
    const log = this.log, build = log?.build, state = build ? buildState(build) : null, l = state ? stateLook(state, t) : null;
    const isStage = view.kind === "stage", stage = isStage ? this.stageOf(view) : null;
    const all = log ? logLines(log.text) : [];
    const range = isStage ? stageRange(all, view.stage.name, view.stage.nth) : null;
    const lines = !isStage ? all : range ? all.slice(range.from, range.to) : [];
    const list = `jk-log-${view.place.path}-${view.number}${isStage ? `-${view.stage.name}-${view.stage.nth}` : ""}`;
    const params = build ? buildParams(build).map((p) => `${p.name}=${p.value}`).join(" · ") : "";
    const width = Math.max(300, (log?.text.widest ?? 0) * 7 + 40);
    const facts = !build ? "" : build.building
      ? `Started ${ago(build.timestamp, now)} ago · running for ${took(build, now)}`.replace("now ago", "just now")
      : `${ago(build.timestamp, now)} ago · took ${took(build, now)}`.replace("now ago", "just now");
    const shown = isStage ? stage?.state ?? null : state, sl = shown ? stateLook(shown, t) : l;
    const parts = [
      this.header(t, [
        this.backButton(t),
        shown ? mark(shown, t) : null,
        div().flex_none().max_w(260).truncate().font_semibold().child(isStage ? view.stage.name : `${leaf(view.place.label)} #${view.number}`),
        div().flex_1().min_w(0).truncate().text_size(11).text_color(sl?.color ?? t.dim)
          .child(isStage ? (shown ? STAGE_LABEL[shown] ?? "" : "") : state ? LABEL[state] : ""),
        build?.building ? pill("jk-abort", "Abort…", t, { tone: "danger", icon: "stop", tip: "Abort this build: asks first", onClick: (cx) => this.askAbort(view.place, view.number, cx) }) : null,
        this.refreshButton(t),
        iconButton("jk-build-open", "external", "Open in the browser", (cx) => this.open(isStage && stage?.id ? this.client.stagePage(view.place, view.number, stage.id) : this.client.page(view.place, view.number), cx), t),
      ]),
      ...this.cards(t),
    ];
    if (isStage) {
      const time = stage ? (stage.state === "paused" ? `waiting for input for ${stageTook(stage, now)}` : stage.state === "running" ? `running for ${stageTook(stage, now)}` : `took ${stageTook(stage, now)}`) : "";
      const why = oneLine(stage?.error?.message ?? "");
      parts.push(v_flex().flex_none().px(12).py(8).gap(3).border_b(1).border_color(t.border).text_size(11.5).text_color(t.dim)
        // The time first: a narrow panel cuts the end.
        .child(div().truncate().child([time, `${leaf(view.place.label)} #${view.number}`].filter(Boolean).join(" · ")))
        .when(Boolean(why), (el) => el.child(div().truncate().text_color(stage.state === "failed" ? t.ansi1 : t.muted).child(why))));
    } else if (build) {
      const part = progress(build, now);
      parts.push(v_flex().flex_none().px(12).py(8).gap(3).border_b(1).border_color(t.border).text_size(11.5).text_color(t.dim)
        .child(h_flex().gap(8).items_center()
          .child(div().min_w(0).truncate().child([facts, oneLine(causeOf(build))].filter(Boolean).join(" · ")))
          .when(part !== null, (el) => el.child(bar(part, t, 60)).child(div().flex_none().text_color(t.working).child(`${Math.round(part * 100)}%`))))
        .when(Boolean(params), (el) => el.child(div().truncate().child(params))));
    }
    if (log?.error) parts.push(this.line(log.error, t, t.blocked));
    const empty = log?.error ? ""
      : !log?.done && !all.length ? "Reading the log…"
        : !isStage ? (log?.done ? "This build wrote nothing." : "Reading the log…")
          : range ? (range.open ? "Nothing yet." : "This stage wrote nothing.")
            : log?.text.dropped ? `This stage began before the last ${all.length} lines, which are all that is kept: the browser has it.`
              : log?.done ? "The console has no part for this stage." : "Reading the log…";
    if (!lines.length) parts.push(div().flex_1().min_h(0).child(this.line(empty, t)));
    else {
      parts.push(div().id("jk-log-area").flex_1().min_h(0).relative().font_family(look().font_family ?? "JetBrains Mono").text_size(11.5)
        // Scrolling by hand stops the following; the footer's button takes it up again.
        .on_scroll_wheel((e, cx) => { if (log.follow && (e?.delta?.y ?? 0) > 0) { log.follow = false; cx.notify(); } })
        .child(div().id("jk-log-x").size_full().overflow_x_scrollbar()
          .child(div().w_full().min_w(width).h_full().child(v_virtual_list(list, lines.length, ROW, (i) => String(i),
            (range) => lines.slice(range.start, range.end).map((line) => this.logRow(line, t))).track_scroll(this.scroll).size_full())))
        .child(Scrollbar.vertical(list).viewport_from_layout().absolute().inset_0()));
    }
    const live = Boolean(build?.building) && !log?.done && (!isStage || Boolean(range?.open));
    parts.push(h_flex().h(26).flex_none().px(12).gap(8).items_center().border_t(1).border_color(t.border).text_size(11).text_color(t.dim)
      .when(live, (el) => el.child(spinner(t, 12)))
      .child(div().flex_1().min_w(0).truncate().child([
        live ? "Following the log" : null,
        `${lines.length} line${lines.length === 1 ? "" : "s"}`,
        !isStage && log?.text.dropped ? `the last ${lines.length} of ${lines.length + log.text.dropped}: the browser has the rest` : null,
      ].filter(Boolean).join(" · ")))
      .when(isStage, (el) => el.child(div().id("jk-whole-log").flex_none().cursor_pointer().text_color(t.accent)
        .tooltip("The whole console log of this build").on_click((_e, cx) => this.wholeLog(cx)).child("Whole log")))
      .when(Boolean(log) && !log.follow, (el) => el.child(div().id("jk-follow").flex_none().cursor_pointer().text_color(t.accent)
        .on_click((_e, cx) => { log.follow = true; this.toEnd(cx); cx.notify(); }).child(live ? "Follow" : "To the end"))));
    return parts;
  }

  // An input in its frame; lit while it has the keyboard.
  field(id, input, t) {
    return div().id(id).h(32).flex_none().px(10).flex().items_center().rounded(7).border_1()
      .border_color(this.typing === input ? t.accent : t.input).bg(t.background).cursor_text()
      .on_mouse_down("left", (_e, cx) => cx.stop_propagation())
      .child(Input.new(input).w_full().h_full().text_size(12.5).text_color(t.foreground));
  }

  labeled(label, hint, control, t) {
    return v_flex().gap(4).flex_none()
      .child(h_flex().gap(8).items_end()
        .child(div().flex_none().text_size(12).font_medium().child(label))
        .when(Boolean(hint), (el) => el.child(div().flex_1().min_w(0).text_size(11).text_color(t.dim).child(hint))))
      .child(control);
  }

  renderTrigger(view, t) {
    const form = this.form, name = leaf(view.place.label), host = hostOf(this.settings.url);
    const controls = (form?.fields ?? []).map((f) => {
      if (f.kind === "text" || f.kind === "password") return this.labeled(f.name, f.description, this.field(`jk-param-${f.name}`, form.inputs[f.name], t), t);
      if (f.kind === "boolean") {
        const on = Boolean(form.values[f.name]);
        return this.labeled(f.name, f.description, h_flex().id(`jk-param-${f.name}`).gap(8).items_center().h(26).cursor_pointer().text_size(12.5)
          .on_click((_e, cx) => { form.values[f.name] = !on; cx.notify(); })
          .child(div().size(16).flex_none().flex().items_center().justify_center().rounded(4).border_1()
            .border_color(on ? t.accent : t.input).bg(on ? t.accent : t.background)
            .when(on, (el) => el.child(svg(icon("check")).size(11).text_color(t.accent_foreground))))
          .child(on ? "Yes" : "No"), t);
      }
      if (f.kind === "choice") {
        return this.labeled(f.name, f.description, h_flex().gap(6).flex_wrap().children(f.choices.map((choice, i) => {
          const picked = form.values[f.name] === choice;
          return div().id(`jk-param-${f.name}-${i}`).h(26).px(9).flex().items_center().rounded(7).border_1().text_size(12).cursor_pointer()
            .border_color(picked ? t.accent : t.border).bg(picked ? t.selected : t.background).hover((s) => s.bg(t.hover))
            .on_click((_e, cx) => { form.values[f.name] = choice; cx.notify(); }).child(choice);
        })), t);
      }
      return this.labeled(f.name, f.description, div().text_size(12).text_color(t.dim).child("Cannot be given from here: Jenkins uses its default."), t);
    });
    const rows = [
      ...this.cards(t),
      v_flex().px(10).pt(12).gap(12)
        .child(div().text_size(12).text_color(t.muted).child(`Starts a build of ${view.place.label} on ${host} as ${this.user()}.`))
        .children(controls.length ? controls : [div().text_size(12).text_color(t.dim).child("This job takes no parameters.")])
        .child(h_flex().pt(4).gap(8).justify_end()
          .child(pill("jk-build-no", "Cancel", t, { onClick: (cx) => this.back(cx) }))
          .child(pill("jk-build-yes", "Build", t, { tone: "solid", icon: "play", busy: this.busy === "trigger", onClick: (cx) => this.build(cx) }))),
    ];
    return [
      this.header(t, [this.backButton(t), div().flex_1().min_w(0).truncate().font_semibold().child(`Build ${name}`), div().flex_none().pr(6).text_size(11).text_color(t.dim).child("Enter builds · Esc cancels")]),
      this.body("jk-trigger", rows),
    ];
  }

  renderSettings(t) {
    const s = this.settings, first = !this.client, path = settingsPath().replace(home(), "~");
    const typed = normalizeBase(this.inputs.url.value());
    const open = /^http:\/\//i.test(typed) && !/^(localhost|127\.0\.0\.1|\[::1\])(:\d+)?$/i.test(hostOf(typed));
    const rows = [
      v_flex().px(10).pt(12).gap(12)
        .child(div().text_size(12).text_color(t.muted).child(first
          ? "Give this panel a Jenkins API token (Jenkins: your name, Security, API Token). The token's Jenkins user decides what the panel may do: one that may only read gets a panel that only reads."
          : "The token's Jenkins user decides what this panel may do: one that may only read gets a panel that only reads."))
        .child(this.labeled("Jenkins URL", open ? "http:// sends the token unencrypted" : "", this.field("jk-set-url", this.inputs.url, t), t))
        .child(this.labeled("User", "the Jenkins user the token belongs to", this.field("jk-set-user", this.inputs.user, t), t))
        .child(this.labeled("API token", "", this.field("jk-set-token", this.inputs.token, t), t))
        .child(this.labeled("Folders", "commas apart, such as team-dev, team/tools or view/Deploys", this.field("jk-set-folders", this.inputs.folders, t), t))
        .when(Boolean(this.check), (el) => el.child(h_flex().id("jk-check").gap(6).items_start().text_size(12).text_color(this.check.ok ? t.ansi2 : t.blocked)
          .when(this.check.ok, (row) => row.child(svg(icon("check")).size(13).mt(1).flex_none()))
          .child(div().flex_1().min_w(0).child(this.check.text))))
        .child(h_flex().pt(2).gap(8).justify_end()
          .when(!first, (el) => el.child(pill("jk-set-cancel", "Cancel", t, { onClick: (cx) => this.back(cx) })))
          .child(pill("jk-set-check", "Check", t, { busy: this.busy === "check", disabled: this.busy === "save", tip: "Asks Jenkins who this token is, and saves nothing", onClick: (cx) => this.verify(cx, false) }))
          .child(pill("jk-set-save", "Save", t, { tone: "solid", busy: this.busy === "save", disabled: this.busy === "check", onClick: (cx) => this.save(cx) })))
        .child(div().text_size(11).text_color(t.dim).child(`Kept in ${path}, which only you can read.${s?.source === "env" ? " These came from JENKINS_URL, JENKINS_USER and JENKINS_API_TOKEN; Save keeps them here." : ""}`)),
    ];
    return [
      this.header(t, [
        first ? svg(icon("jenkins")).size(16).ml(4).flex_none().text_color(t.muted) : this.backButton(t),
        div().flex_1().min_w(0).truncate().font_semibold().child(first ? "Jenkins" : "Jenkins settings"),
      ]),
      this.body("jk-settings-body", rows),
    ];
  }

  // --- render --------------------------------------------------------------------

  // Esc goes back a view; Tab moves between a form's fields.
  key(e, cx) {
    const view = this.view();
    if (e.key === "escape") {
      if (this.confirm) this.confirm = null;
      else if (this.typing && view.kind !== "trigger" && view.kind !== "settings") this.handle.focus();
      else if (!this.back(cx)) return;
    } else if (e.key === "tab" && this.order.length && (view.kind === "trigger" || view.kind === "settings")) {
      const at = this.order.indexOf(this.typing), step = e.modifiers?.shift ? -1 : 1;
      this.order[(at + step + this.order.length) % this.order.length].focus();
    } else if (e.key === "enter" && view.kind === "trigger" && !this.typing) this.build(cx);
    else return;
    cx.stop_propagation();
    cx.notify();
  }

  render() {
    const t = theme(), now = Date.now(), view = this.view();
    const parts = !this.settings ? [this.line("Reading the settings…", t)]
      : view.kind === "settings" ? this.renderSettings(t)
        : view.kind === "folder" ? this.renderFolder(view, t, now)
          : view.kind === "job" ? this.renderJob(view, t, now)
            : view.kind === "build" || view.kind === "stage" ? this.renderBuild(view, t, now)
              : view.kind === "trigger" ? this.renderTrigger(view, t)
                : this.renderList(t, now);
    return v_flex().id("jenkins").track_focus(this.handle).size_full().min_h(0).overflow_hidden()
      .when(!this.panel, (el) => el.bg(t.background)).text_color(t.foreground).text_size(13)
      .on_key_down((e, cx) => this.key(e, cx))
      .children(parts);
  }
}
