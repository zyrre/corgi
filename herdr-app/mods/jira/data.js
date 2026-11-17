// What the panel reads from Jira, read once for every copy of the mod.
//
// The app runs a mod's modules once per source version and shares them by
// every copy (the panel, an overlay, a tab), so the one `store` below is
// every copy's. Each copy asks it on its own timer for what its view shows,
// and the store reads a thing only when it is due by the thing's own times
// (`Entry.due`): however many copies tick, who the token is, the board, its
// sprints, a sprint's issues, an issue, its transitions and the popups' read
// are fetched once per due time. Each copy keeps its own view.
//
// A read runs as a task of the copy that asked (`cx`), which is redrawn when
// it ends; the others see `version` change on their next tick. The app's
// fetch has no timeout, so a read still running after READ_LIMIT counts as
// failed and its late answer is dropped. That is decided wherever the entry
// is looked at (`expire`), the drawing of a view included, so a read that
// never ends turns into an error on screen without waiting for another read
// to be asked for.
//
// Failing reads back off: the next try waits 10 s, then 30 s, then 60 s (and
// never less than the thing's usual time), and a good read resets that. A
// failure that may pass by itself (no answer, a 5xx, a timeout) stays quiet
// while there is earlier data to show: that stays, with a dim "Last read …
// ago" line, and the error shows after QUIET_FAILS failures in a row or
// QUIET_MS of failing. With nothing read yet, or a failure that will not
// pass by itself (401, 403, 404, 400, the settings), it shows at once.

import { Client, JiraError } from "./api.js";
import { Watch } from "./alerts.js";

export const BACKOFF = [10000, 30000, 60000]; // the waits after the 1st, 2nd and later failures in a row
export const QUIET_FAILS = 3;
export const QUIET_MS = 60000;
export const READ_LIMIT = 30000;

// Whether a failure may pass by itself: no answer, Jira busy or down for a moment.
export const transient = (e) => e?.kind === "network" || e?.kind === "timeout"
  || (e?.kind === "http" && (e.status >= 500 || e.status === 408 || e.status === 429));

// The wait before the next try of a read that failed `failures` times in a row.
export const backoff = (failures) => (failures > 0 ? BACKOFF[Math.min(failures, BACKOFF.length) - 1] : 0);

// One thing read from Jira: its last good data, and how its reads go.
export class Entry {
  constructor() {
    this.data = null; // the last good answer
    this.at = 0; // when it came
    this.error = null; // the last read's failure (a JiraError), until one succeeds
    this.failures = 0; // failed reads in a row
    this.since = 0; // when the first of them ended
    this.tried = 0; // when the last read ended, good or not
    this.loading = false;
    this.started = 0;
    this.seq = 0; // bumped by each read begun, and by giving up on one: a read that is no longer the last is dropped
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

  // A read that has run past READ_LIMIT failed: no answer in time. True when
  // it gave one up.
  expire(now = Date.now()) {
    if (!this.loading || now - this.started < READ_LIMIT) return false;
    this.seq += 1;
    this.failed(new JiraError("timeout", `no answer in ${READ_LIMIT / 1000} s`), now);
    store.version += 1;
    return true;
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

  // Whether its error is to be shown: at once with nothing read before, or
  // for one that will not pass by itself; else once it has gone on a while.
  loud(now = Date.now()) {
    this.expire(now);
    if (!this.error) return false;
    return !this.at || !transient(this.error) || this.failures >= QUIET_FAILS || now - this.since >= QUIET_MS;
  }

  // Failing, but quietly: the data shown is from `at`.
  stale(now = Date.now()) { return Boolean(this.error) && !this.loud(now); }
}

// What one Jira, as one set of settings reaches it, has said.
class Source {
  constructor(settings) {
    this.client = new Client(settings);
    this.entries = new Map(); // "<kind> <key>" → Entry
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
}

class JiraData {
  constructor() {
    this.version = 0; // bumped whenever anything read changes: a copy that saw another number draws again
    this.settings = null; // the settings last taken into use by any copy
    this.saved = 0; // bumped with them: a copy on older ones takes these up
    this.sources = new Map(); // url and token → Source
    // The popups' memory (alerts.js): what was seen of the token user's
    // issues, here so that an event is seen once by all copies, and
    // forgotten with the settings.
    this.watch = new Watch();
    this.goto = null; // an issue to go to, from a popup's click or `jira.open-<KEY>`: { seq, key, at }
    this.gone = 0; // the gotos so far
    // The last popup the panel posted while it did not show: { key, at }.
    // Its click runs a layout command (alerts.js `showStep`), as a hidden
    // copy gets no mod commands, so the panel goes to `key` when it shows soon after.
    this.pending = null;
    this.opened = 0; // the `at` of the last shared("jira.open") taken up
  }

  // The settings every copy uses from now on.
  use(settings) {
    const same = this.settings && ["url", "token", "board"].every((k) => (this.settings[k] ?? "") === (settings[k] ?? ""));
    if (same) return false;
    this.watch.reset();
    this.pending = null;
    this.settings = settings;
    this.saved += 1;
    return true;
  }

  // What Jira said under these settings, shared by every copy using them; a
  // source of other settings is let go.
  source(settings) {
    const key = JSON.stringify([settings.url, settings.token]);
    let s = this.sources.get(key);
    if (!s) {
      this.sources.clear();
      this.sources.set(key, (s = new Source(settings)));
    }
    return s;
  }
}

// The one every copy shares.
export const store = new JiraData();
