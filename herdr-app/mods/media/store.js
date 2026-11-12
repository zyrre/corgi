// What the Media mod knows, one for every copy of it (the app runs a mod's
// modules once per source version and shares them by its copies, so the
// panel and the watcher read and write one history): the drop folder's
// files, the items taken in from it, and the unseen count. Every copy's
// timer calls `tick`; the store does each thing once.
//
// The app draws only images inside its own home (`image()` takes a path
// relative to it), and agents may not write there, so the drop folder is
// elsewhere (~/.local/share/herdr-media by default) and each new file is
// copied into media/items/ of the app's home, which the mod writes. A
// video is never copied: it becomes an animated WebP (ffmpeg) or a still
// frame (qlmanage), and Open plays the file in the drop folder. The app
// draws an animated GIF or WebP as its first frame (seen 2026-11-12).

import { exec, notify, setShared, shared, appHome, home, layout, watchFolder } from "herdr";
import * as fs from "fs/promises";
import {
  KEEP, plan, parseListing, captionText, addItem, unseenCount, filesOf, imageInfo, extOf, baseOf, kindOf,
  panelShown, titleOf, oneLine,
} from "./items.js";

// The count the tab bar's Media button shows.
export const UNSEEN = "media.unseen";
export const DEFAULT_FOLDER = "~/.local/share/herdr-media";
const ITEMS = "media/items"; // in the app's home
const SCAN_EVERY = 15000; // a full look at the folder besides what watchFolder says
const SETTINGS_EVERY = 10000;
// A video's preview: at most this many seconds, this wide, this many frames a second.
export const CLIP = { seconds: 10, width: 640, fps: 12 };

// Prints `<size>\t<name>` for each file of the folder $1, oldest first;
// exits 3 when there is no such folder.
const LIST = 'cd "$1" 2>/dev/null || exit 3\nls -1Atr | while IFS= read -r f; do [ -f "$f" ] && printf \'%s\\t%s\\n\' "$(wc -c < "$f" | tr -d \' \')" "$f"; done\nexit 0';

export function expand(folder) {
  const f = String(folder ?? "").trim() || DEFAULT_FOLDER;
  if (f === "~") return home();
  return f.startsWith("~/") ? `${home()}/${f.slice(2)}` : f;
}

// The arguments that make a clip's preview, an animated WebP.
export function clipArgs(src, out) {
  return ["-nostdin", "-v", "error", "-y", "-t", String(CLIP.seconds), "-i", src, "-an",
    "-vf", `fps=${CLIP.fps},scale='min(${CLIP.width},iw)':-2:flags=lanczos`,
    "-loop", "0", "-c:v", "libwebp", "-q:v", "70", out];
}

async function run(command, args, options) {
  try { return await exec(command, args, options); } catch (e) { return { code: -1, stdout: "", stderr: String(e?.message ?? e) }; }
}

export class Store {
  constructor() {
    this.version = 0; // bumped on every change a view draws
    this.loaded = false; this.loading = false;
    this.items = []; // newest first
    this.seenAt = 0; // `at` of the newest item seen
    this.known = {}; // files of the drop folder taken in: name → size
    this.pending = {}; // files waiting to be whole
    this.folder = null; // the drop folder, absolute
    this.folderSetting = DEFAULT_FOLDER;
    this.ready = false; // the folder exists
    this.problem = null; // why the folder cannot be read, for the panel
    this.busy = false; this.dirty = true;
    this.lastScan = 0; this.lastTick = 0; this.lastSettings = 0;
    this.baseline = false; // the first run: what is in the folder already comes without a toast
    this.converting = new Set();
    this.ffmpeg = undefined; // its path, or null when there is none
    this.seq = 0;
    this.saving = null; this.saveAgain = false;
  }

  bump() { this.version += 1; }

  path(rel) { return `${appHome()}/${rel}`; }

  // --- reading and writing ---------------------------------------------------------

  async readSettings() {
    this.lastSettings = Date.now();
    let folder = DEFAULT_FOLDER;
    try {
      const text = await fs.readFile(this.path("media/settings.json"), "utf8");
      const s = JSON.parse(text);
      if (typeof s?.folder === "string" && s.folder.trim()) folder = s.folder.trim();
    } catch { /* none: the default */ }
    if (folder !== this.folderSetting || this.folder === null) {
      this.folderSetting = folder;
      this.folder = expand(folder);
      this.ready = false; this.dirty = true;
      this.pending = {};
      this.bump();
    }
  }

  async load() {
    this.loading = true;
    await this.readSettings();
    let saved = null;
    try { saved = JSON.parse(await fs.readFile(this.path("media/history.json"), "utf8")); } catch { saved = null; }
    if (saved && Array.isArray(saved.items)) {
      this.items = saved.items.filter((i) => i && typeof i.id === "string").slice(0, KEEP);
      this.seenAt = Number(saved.seenAt) || 0;
      this.known = saved.known && typeof saved.known === "object" ? { ...saved.known } : {};
      if (saved.folder && saved.folder !== this.folder) this.known = {}; // another folder's files
    } else {
      this.baseline = true;
    }
    try { await fs.mkdir(this.path(ITEMS), { recursive: true }); } catch { /* there */ }
    await this.prune();
    this.loaded = true; this.loading = false;
    this.publish();
    this.bump();
  }

  // Deletes what media/items holds that no item draws (left by a quit, a
  // removal that failed, an older version).
  async prune() {
    let names = [];
    try { names = await fs.readdir(this.path(ITEMS)); } catch { return; }
    const used = new Set(this.items.flatMap(filesOf));
    for (const name of names) {
      const rel = `${ITEMS}/${name}`;
      if (used.has(rel)) continue;
      if (name.startsWith(".ql-")) await run("rm", ["-rf", this.path(rel)]);
      else { try { await fs.unlink(this.path(rel)); } catch { /* gone */ } }
    }
  }

  save() {
    if (this.saving) { this.saveAgain = true; return this.saving; }
    const data = JSON.stringify({ version: 1, folder: this.folder, seenAt: this.seenAt, known: this.known, items: this.items }, null, 1);
    const dir = this.path("media"), file = this.path("media/history.json");
    this.saving = (async () => {
      try {
        await fs.mkdir(dir, { recursive: true });
        await fs.writeFile(file, `${data}\n`);
      } catch { /* the next change saves again */ }
      this.saving = null;
      if (this.saveAgain) { this.saveAgain = false; await this.save(); }
    })();
    return this.saving;
  }

  // The unseen count, for the tab bar's button.
  publish() {
    const n = unseenCount(this.items, this.seenAt);
    try { if (shared(UNSEEN) !== n) setShared(UNSEEN, n); } catch { /* no app state here */ }
  }

  markSeen() {
    const newest = this.items[0]?.at ?? 0;
    if (newest <= this.seenAt) return false;
    this.seenAt = newest;
    this.publish();
    this.save();
    this.bump();
    return true;
  }

  // --- the folder ----------------------------------------------------------------------

  async list() {
    let r = await run("sh", ["-c", LIST, "sh", this.folder]);
    if (r.code === 3) {
      await run("mkdir", ["-p", this.folder]);
      r = await run("sh", ["-c", LIST, "sh", this.folder]);
    }
    if (r.code !== 0) {
      const was = this.problem;
      this.problem = r.code === 3 ? `Could not make the folder ${this.folder}` : `Could not read ${this.folder}: ${oneLine(r.stderr) || `exit ${r.code}`}`;
      if (was !== this.problem) this.bump();
      this.ready = false;
      return null;
    }
    if (this.problem || !this.ready) { this.problem = null; this.ready = true; this.bump(); }
    return parseListing(r.stdout);
  }

  async caption(name) {
    const r = await run("head", ["-c", "4096", `${this.folder}/${name}`]);
    return r.code === 0 ? captionText(r.stdout) : "";
  }

  async scan() {
    this.lastScan = Date.now();
    const files = await this.list();
    if (!files) return [];
    const now = Date.now();
    const { take, late } = plan(files, this, now);
    let changed = false;
    for (const l of late) {
      this.known[l.caption] = l.size;
      const item = this.items.find((i) => i.name === l.name);
      if (item) { item.caption = await this.caption(l.caption); changed = true; }
    }
    // More than the panel keeps at once (a first look at a full folder): the newest.
    const skipped = take.length > KEEP ? take.splice(0, take.length - KEEP) : [];
    for (const f of skipped) this.known[f.name] = f.size;
    const added = [];
    for (const f of take) added.push(await this.ingest(f));
    if (added.length || changed || skipped.length) {
      if (this.baseline && added.length) this.seenAt = Math.max(this.seenAt, ...added.map((i) => i.at));
      this.publish();
      await this.save();
      this.bump();
    }
    const announce = this.baseline ? [] : added;
    if (this.baseline && !Object.keys(this.pending).length) this.baseline = false;
    return announce;
  }

  // Takes one file in: an item, newest first, with a copy of an image in
  // the app's home. A video's preview is made afterwards (`convert`).
  async ingest(f) {
    const now = Date.now();
    const id = `${now.toString(36)}${(this.seq++).toString(36)}`;
    const item = { id, name: f.name, kind: f.kind, size: f.size, at: now, caption: "", file: null };
    this.known[f.name] = f.size;
    if (f.caption) {
      item.caption = await this.caption(f.caption);
      this.known[f.caption] = this.pending[f.caption]?.size ?? 0;
      delete this.pending[f.caption];
    }
    delete this.pending[f.name];
    if (f.kind === "image") {
      const rel = `${ITEMS}/${id}.${extOf(f.name) || "img"}`;
      const r = await run("cp", [`${this.folder}/${f.name}`, this.path(rel)]);
      if (r.code === 0) {
        item.file = rel;
        Object.assign(item, await this.info(rel, f.name));
      } else item.problem = "Could not copy it";
    } else {
      item.state = "converting";
    }
    for (const gone of addItem(this.items, item)) await this.forget(gone);
    return item;
  }

  async info(rel, name) {
    try {
      const i = imageInfo(await fs.readFile(this.path(rel)), name);
      return { format: i.format, width: i.width, height: i.height, animated: i.animated };
    } catch { return { format: extOf(name), width: null, height: null, animated: false }; }
  }

  // Deletes an item's own files (never the drop folder's).
  async forget(item) {
    for (const rel of filesOf(item)) { try { await fs.unlink(this.path(rel)); } catch { /* gone */ } }
  }

  // --- videos --------------------------------------------------------------------------

  async findFfmpeg() {
    if (this.ffmpeg !== undefined) return this.ffmpeg;
    const r = await run("sh", ["-c", "command -v ffmpeg"]);
    this.ffmpeg = r.code === 0 && r.stdout.trim() ? r.stdout.trim().split("\n")[0] : null;
    return this.ffmpeg;
  }

  // A video's preview, in the background: an animated WebP when ffmpeg is
  // there (the app draws its first frame), else a still frame from macOS's
  // Quick Look, else none (Open still plays it).
  async convert(item) {
    if (this.converting.has(item.id)) return;
    this.converting.add(item.id);
    try {
      const src = `${this.folder}/${item.name}`;
      const ffmpeg = await this.findFfmpeg();
      let done = false;
      if (ffmpeg) {
        const rel = `${ITEMS}/${item.id}.webp`;
        const r = await run(ffmpeg, clipArgs(src, this.path(rel)), { timeout: 180000 });
        if (r.code === 0 && await fs.exists(this.path(rel))) {
          Object.assign(item, { file: rel, preview: "converted" }, await this.info(rel, `${item.id}.webp`));
          item.animated = false; // the item is a video: its note says so
          done = true;
        } else {
          try { await fs.unlink(this.path(rel)); } catch { /* none */ }
        }
      }
      if (!done) {
        const tmp = `${ITEMS}/.ql-${item.id}`;
        await run("mkdir", ["-p", this.path(tmp)]);
        const r = await run("qlmanage", ["-t", "-s", "800", "-o", this.path(tmp), src], { timeout: 60000 });
        const made = `${tmp}/${item.name}.png`;
        if (r.code === 0 && await fs.exists(this.path(made))) {
          const rel = `${ITEMS}/${item.id}.png`;
          const mv = await run("mv", [this.path(made), this.path(rel)]);
          if (mv.code === 0) { Object.assign(item, { file: rel, preview: "still" }, await this.info(rel, `${item.id}.png`)); done = true; }
        }
        await run("rm", ["-rf", this.path(tmp)]);
      }
      if (!done) item.preview = "none";
      delete item.state;
      if (!this.items.includes(item)) await this.forget(item); // removed meanwhile
      else { await this.save(); this.bump(); }
    } finally {
      this.converting.delete(item.id);
    }
  }

  // --- what a person does -----------------------------------------------------------

  async remove(id) {
    const at = this.items.findIndex((i) => i.id === id);
    if (at < 0) return;
    const [item] = this.items.splice(at, 1);
    await this.forget(item);
    this.publish();
    await this.save();
    this.bump();
  }

  // Every item out, and the files they came from out of the drop folder
  // (with their captions): the only time the mod deletes a file there.
  async clear() {
    const items = this.items.splice(0);
    for (const item of items) await this.forget(item);
    const names = Object.keys(this.known).filter((n) => kindOf(n) === "image" || kindOf(n) === "video");
    const captions = Object.keys(this.known).filter((n) => kindOf(n) === "caption" && names.some((m) => baseOf(m) === baseOf(n)));
    const paths = [...names, ...captions].map((n) => `${this.folder}/${n}`);
    if (paths.length) await run("rm", ["-f", "--", ...paths]);
    this.seenAt = Date.now();
    this.publish();
    await this.save();
    this.dirty = true;
    this.bump();
  }

  // Opens a file with the system's viewer: the dropped file while it is
  // there, else the mod's copy of it.
  async open(item) {
    const dropped = `${this.folder}/${item.name}`;
    const r = await run("test", ["-f", dropped]);
    const file = r.code === 0 ? dropped : item.file ? this.path(item.file) : dropped;
    const opened = await run("open", [file]);
    if (opened.code !== 0) await run("xdg-open", [file]);
  }

  async openFolder() {
    if (!this.folder) return;
    await run("mkdir", ["-p", this.folder]);
    const opened = await run("open", [this.folder]);
    if (opened.code !== 0) await run("xdg-open", [this.folder]);
  }

  // --- the toast ------------------------------------------------------------------------

  announce(added) {
    if (!added.length) return;
    let shown = false;
    try { shown = panelShown(layout()); } catch { /* no layout here */ }
    if (shown) { this.markSeen(); return; }
    const first = added[added.length - 1];
    const icon = first.kind === "image" && first.file && first.format !== "svg" ? first.file : undefined;
    const toast = added.length === 1
      ? { title: oneLine(titleOf(first)).slice(0, 120), body: first.caption ? `${first.name} · click to show it` : "In the Media panel: click to show it" }
      : { title: `${added.length} new items in Media`, body: oneLine(added.map(titleOf).join(", ")).slice(0, 160) };
    try { notify({ ...toast, level: "info", ...(icon ? { icon } : {}), action: { name: "media.show" } }); } catch { /* no toasts here */ }
  }

  // --- the loop --------------------------------------------------------------------------

  // Called by every copy's timer, about every second; does what is due, once.
  tick(cx) {
    const now = Date.now();
    if (!this.loaded) {
      if (!this.loading) cx.spawn(async (cx) => { await this.load(); this.kick(cx); cx.notify(); });
      return;
    }
    if (now - this.lastTick < 900) return;
    this.lastTick = now;
    if (unseenCount(this.items, this.seenAt) > 0) {
      let shown = false;
      try { shown = panelShown(layout()); } catch { /* no layout here */ }
      if (shown) this.markSeen();
    }
    if (now - this.lastSettings >= SETTINGS_EVERY) cx.spawn(async () => { await this.readSettings(); });
    if (this.dirty || Object.keys(this.pending).length || now - this.lastScan >= SCAN_EVERY) this.kick(cx);
    for (const item of this.items) if (item.state === "converting" && !this.converting.has(item.id)) this.startConvert(item, cx);
  }

  startConvert(item, cx) {
    cx.spawn(async (cx) => { await this.convert(item); cx.notify(); });
  }

  // Scans the folder now, unless a scan runs (then once more after it).
  kick(cx) {
    if (!this.loaded || !this.folder) return;
    if (this.busy) { this.dirty = true; return; }
    this.busy = true;
    this.dirty = false;
    cx.spawn(async (cx) => {
      try {
        for (let round = 0; round < 3; round++) {
          const added = await this.scan();
          this.announce(added);
          for (const item of added) if (item.state === "converting") this.startConvert(item, cx);
          if (!this.dirty) break;
          this.dirty = false;
        }
      } finally {
        this.busy = false;
      }
      cx.notify();
    });
  }

  // Waits for changes in the folder and scans when there are some. Each
  // copy runs one (watchFolder is shared by every mod); it ends when the
  // folder cannot be watched, and `tick` starts another when it can.
  watch(cx, copy) {
    const folder = this.folder;
    copy.watching = true;
    // What came before this watch began is found by one scan more.
    this.dirty = true;
    cx.spawn(async (cx) => {
      try {
        while (this.folder === folder) {
          await watchFolder(folder);
          this.dirty = true;
          this.kick(cx);
        }
      } catch { /* not there yet, or not watchable: the timer scans */ }
      copy.watching = false;
    });
  }
}

export const store = new Store();
