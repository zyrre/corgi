// What the panel reads from Jenkins, read once for every copy of the mod.
//
// The app runs a mod's modules once per source version and shares them by
// every copy (the panel, an overlay, a tab), so the one `store` below is
// every copy's. Each copy asks it on its own timer for what its view shows,
// and the store reads a thing only when it is due by the thing's own times
// (`Entry.due`): however many copies tick, a list, a job, a build's stages, a
// job's runs or a build's log is fetched once per due time. Each copy keeps
// its own view (its stack, folded headings, scroll).
//
// A read runs as a task of the copy that asked (`cx`), which is redrawn when
// it ends; the others see `version` change on their next tick. A copy that
// closes mid-read takes its task with it, and a Jenkins that stalls never
// answers (the app's fetch has no timeout): a read still running after
// READ_LIMIT counts as failed, its late answer is dropped, and the next one
// may start.
//
// Failing reads back off: the next try waits 10 s, then 30 s, then 60 s (and
// never less than the thing's usual time), and a good read resets that. A
// failure that may pass by itself (no answer, a 5xx, a timeout) stays quiet
// at first: the last good data stays with a dim "Last read … ago" line, and
// the error shows only after QUIET_FAILS failures in a row or QUIET_MS of
// failing (`loud`). One that will not pass by itself (401, 403, 404, the
// settings) shows at once.

import { Client, JenkinsError } from "./api.js";
import { emptyLog, appendLog } from "./util.js";

export const BACKOFF = [10000, 30000, 60000]; // the waits after the 1st, 2nd and later failures in a row
export const QUIET_FAILS = 3;
export const QUIET_MS = 60000;
export const READ_LIMIT = 45000;
// A followed log is read on every tick of a copy's two-second timer; copies
// whose timers tick within this of each other share a read.
export const LOG_EVERY = 1500;
const LOG_KEEP = 10 * 60000; // a log no copy has shown for this long is let go

// Whether a failure may pass by itself: no answer, Jenkins busy or down for a moment.
export const transient = (e) => e?.kind === "network" || e?.kind === "timeout"
  || (e?.kind === "http" && (e.status >= 500 || e.status === 408 || e.status === 429));

// The wait before the next try of a read that failed `failures` times in a row.
export const backoff = (failures) => (failures > 0 ? BACKOFF[Math.min(failures, BACKOFF.length) - 1] : 0);

// One thing read from Jenkins: its last good data, and how its reads go.
export class Entry {
  constructor() {
    this.data = null; // the last good answer
    this.at = 0; // when it came
    this.error = null; // the last read's failure (a JenkinsError), until one succeeds
    this.failures = 0; // failed reads in a row
    this.since = 0; // when the first of them ended
    this.tried = 0; // when the last read ended, good or not
    this.loading = false;
    this.started = 0;
    this.seq = 0; // bumped by each read begun: a read that is no longer the last is dropped
  }

  // A read under way, and not given up on.
  running(now = Date.now()) { return this.loading && now - this.started < READ_LIMIT; }

  // Whether a read is due, for a thing read every `every` ms: a failed one
  // waits for its back-off, and one never read is due at once.
  due(every, now = Date.now()) {
    this.expire(now);
    if (this.loading) return false;
    if (!this.tried) return true;
    const wait = this.failures ? Math.max(every, backoff(this.failures)) : every;
    return now - this.tried >= wait;
  }

  // A read that has run past READ_LIMIT failed: no answer in time.
  expire(now = Date.now()) {
    if (this.loading && now - this.started >= READ_LIMIT) {
      this.seq += 1;
      this.failed(new JenkinsError("timeout", `no answer in ${READ_LIMIT / 1000} s`), now);
    }
  }

  begin(now = Date.now()) {
    this.loading = true;
    this.started = now;
    this.seq += 1;
    return this.seq;
  }

  succeeded(data, now = Date.now()) {
    Object.assign(this, { data, at: now, tried: now, error: null, failures: 0, since: 0, loading: false });
  }

  failed(error, now = Date.now()) {
    if (!this.failures) this.since = now;
    Object.assign(this, { error, tried: now, failures: this.failures + 1, loading: false });
  }

  // Whether its error is to be shown: at once for one that will not pass by
  // itself, else once it has gone on for a while.
  loud(now = Date.now()) {
    if (!this.error) return false;
    return !transient(this.error) || this.failures >= QUIET_FAILS || now - this.since >= QUIET_MS;
  }

  // Failing, but quietly: the data shown is from `at`.
  stale(now = Date.now()) { return Boolean(this.error) && !this.loud(now); }
}

// A build's console log, read on from where the last read ended: what was
// read (`text`), the build as read with it, and whether it is all there.
export class Log extends Entry {
  constructor(place, number, build) {
    super();
    this.place = place;
    this.number = number;
    this.build = build;
    this.text = emptyLog();
    this.done = false; // read to its end, or failing for good
    this.followed = false; // read while the build ran
    this.shown = Date.now(); // when a copy last showed it
  }
}

// What one Jenkins, as one set of settings reaches it, has said.
class Source {
  constructor(settings) {
    this.client = new Client(settings);
    this.entries = new Map(); // "<kind> <key>" → Entry
    this.logs = new Map(); // "<job path>/<number>" → Log
  }

  entry(kind, key = "") {
    const id = `${kind} ${key}`;
    let e = this.entries.get(id);
    if (!e) this.entries.set(id, (e = new Entry()));
    return e;
  }

  peek(kind, key = "") { return this.entries.get(`${kind} ${key}`) ?? null; }

  // Whether a read of these kinds is under way.
  busy(kinds) {
    for (const [id, e] of this.entries) if (e.running() && kinds.includes(id.slice(0, id.indexOf(" ")))) return true;
    return false;
  }

  // Reads `kind key` with `get(client)` when due (`every` ms after the last
  // read, or at its back-off), or now when `force`d and none is running.
  // `then(entry, cx)` follows a good answer.
  read(kind, key, get, cx, { every = 0, force = false, then } = {}) {
    const e = this.entry(kind, key), now = Date.now();
    e.expire(now);
    if (e.loading || !(force || e.due(every, now))) return e;
    const seq = e.begin(now), client = this.client;
    cx.spawn(async (cx) => {
      let data, error = null;
      try { data = await get(client); } catch (err) { error = err; }
      if (e.seq !== seq) return; // given up on meanwhile
      if (error) e.failed(error);
      else e.succeeded(data);
      store.version += 1;
      if (!error) then?.(e, cx);
      cx.notify();
    });
    return e;
  }

  // The shared log of a build, made when first shown; logs no copy has shown
  // for a while are let go.
  log(place, number, build) {
    const key = `${place.path}/${number}`, now = Date.now();
    for (const [k, l] of this.logs) if (now - l.shown > LOG_KEEP) this.logs.delete(k);
    let l = this.logs.get(key);
    if (!l) this.logs.set(key, (l = new Log(place, number, build)));
    l.shown = now;
    return l;
  }

  // Reads the build and what its console has written since the last read. A
  // read starts at the byte the last one ended at, counted from the text
  // itself (the app's fetch shows no headers); a log with bytes that are not
  // UTF-8 can put that count off while it runs, so a build that was followed
  // is read once more, whole, when it ends. A failure that will not pass by
  // itself ends the reading; one that may is tried again at its back-off.
  pollLog(log, cx, { force = false, then } = {}) {
    const now = Date.now();
    log.expire(now);
    if (log.loading || log.done || !(force || log.due(LOG_EVERY, now))) return;
    const seq = log.begin(now), client = this.client;
    cx.spawn(async (cx) => {
      let error = null, build, text;
      try {
        build = await client.build(log.place, log.number);
        const chunk = await client.logFrom(log.place, log.number, log.text.offset);
        text = appendLog(log.text, chunk);
        if (!build.building && log.followed) text = appendLog(emptyLog(), await client.logFrom(log.place, log.number, 0));
      } catch (err) { error = err; }
      if (log.seq !== seq) return;
      if (error) {
        log.failed(error);
        if (!transient(error)) log.done = true;
      } else {
        log.build = build;
        log.text = text;
        if (build.building) log.followed = true;
        else log.done = true;
        log.succeeded(build);
      }
      store.version += 1;
      then?.(log, cx);
      cx.notify();
    });
  }
}

class JenkinsData {
  constructor() {
    this.version = 0; // bumped whenever anything read changes: a copy that saw another number draws again
    this.settings = null; // the settings last taken into use by any copy
    this.saved = 0; // bumped with them: a copy on older ones takes these up
    this.sources = new Map(); // url, user and token → Source
  }

  // The settings every copy uses from now on.
  use(settings) {
    const same = this.settings && ["url", "user", "token", "folders"].every((k) => (this.settings[k] ?? "") === (settings[k] ?? ""));
    if (same) return false;
    this.settings = settings;
    this.saved += 1;
    return true;
  }

  // What Jenkins said under these settings, shared by every copy using them;
  // a source of other settings is let go.
  source(settings) {
    const key = JSON.stringify([settings.url, settings.user, settings.token]);
    let s = this.sources.get(key);
    if (!s) {
      this.sources.clear();
      this.sources.set(key, (s = new Source(settings)));
    }
    return s;
  }
}

// The one every copy shares.
export const store = new JenkinsData();
