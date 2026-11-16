// Runs the Media mod under Node, past the first frame `herdr-app check`
// renders: its item logic alone, then the mod itself with stand-ins for the
// app's `gpui-kit`, `gpui-base`, `herdr` and `fs/promises` modules, a scratch
// folder as the app's home and another as the drop folder. The mod's `exec`
// runs the real `sh`, `cp`, `mkdir`, `rm`, `head`, `mv` and `test`; `ffmpeg`,
// `qlmanage` and `open` are fakes that log their calls. Last, install.sh and
// its layout edits on scratch app homes. Nothing here touches a real app.
//
//   node herdr-app/mods/media/tests/test.mjs

import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import fs from 'node:fs';
import fsp from 'node:fs/promises';
import { registerHooks } from 'node:module';
import os from 'node:os';
import path from 'node:path';
import zlib from 'node:zlib';
import { fileURLToPath } from 'node:url';

const MOD = path.dirname(path.dirname(fileURLToPath(import.meta.url)));
const JENKINS = path.join(path.dirname(MOD), 'jenkins');

// Time the tests move: the mod reads Date.now.
let skew = 0;
const realNow = Date.now;
Date.now = () => realNow() + skew;

// --- media files ---------------------------------------------------------------

function crc32(buf) {
  let c, crc = 0xffffffff;
  for (const b of buf) {
    c = (crc ^ b) & 0xff;
    for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
    crc = (crc >>> 8) ^ c;
  }
  return (crc ^ 0xffffffff) >>> 0;
}
function chunk(type, data) {
  const len = Buffer.alloc(4); len.writeUInt32BE(data.length);
  const td = Buffer.concat([Buffer.from(type), data]);
  const crc = Buffer.alloc(4); crc.writeUInt32BE(crc32(td));
  return Buffer.concat([len, td, crc]);
}
// A w×h PNG (an APNG with `animated`).
function png(w, h, { animated = false } = {}) {
  const ihdr = Buffer.alloc(13);
  ihdr.writeUInt32BE(w, 0); ihdr.writeUInt32BE(h, 4); ihdr[8] = 8; ihdr[9] = 2;
  const raw = Buffer.alloc((w * 3 + 1) * h, 0x80);
  for (let y = 0; y < h; y++) raw[y * (w * 3 + 1)] = 0;
  return Buffer.concat([Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]), chunk('IHDR', ihdr),
    ...(animated ? [chunk('acTL', Buffer.from([0, 0, 0, 2, 0, 0, 0, 0]))] : []),
    chunk('IDAT', zlib.deflateSync(raw)), chunk('IEND', Buffer.alloc(0))]);
}
// A w×h GIF of `frames` frames of one color (an image descriptor each, LZW data of a clear and an end code).
function gif(w, h, frames) {
  const parts = [Buffer.from('GIF89a'), Buffer.from([w & 255, w >> 8, h & 255, h >> 8, 0x80, 0, 0]), Buffer.from([0, 0, 0, 255, 255, 255])];
  for (let i = 0; i < frames; i++) {
    parts.push(Buffer.from([0x21, 0xf9, 4, 4, 8, 0, 0, 0]));
    parts.push(Buffer.from([0x2c, 0, 0, 0, 0, w & 255, w >> 8, h & 255, h >> 8, 0, 2, 2, 0x44, 0x01, 0]));
  }
  parts.push(Buffer.from([0x3b]));
  return Buffer.concat(parts);
}
// A WebP's first bytes: VP8X with the animation flag, or a plain VP8L.
function webp(w, h, { animated = false } = {}) {
  const b = Buffer.alloc(40);
  b.write('RIFF', 0); b.writeUInt32LE(32, 4); b.write('WEBP', 8);
  if (animated) {
    b.write('VP8X', 12); b.writeUInt32LE(10, 16); b[20] = 0x02;
    b.writeUIntLE(w - 1, 24, 3); b.writeUIntLE(h - 1, 27, 3);
  } else {
    b.write('VP8L', 12); b.writeUInt32LE(10, 16); b[20] = 0x2f;
    b.writeUInt32LE(((w - 1) & 0x3fff) | (((h - 1) & 0x3fff) << 14), 21);
  }
  return b;
}
// A JPEG's markers as far as its size: SOI, an APP0, SOF0.
function jpeg(w, h) {
  return Buffer.from([0xff, 0xd8, 0xff, 0xe0, 0, 4, 0, 0, 0xff, 0xc0, 0, 11, 8, h >> 8, h & 255, w >> 8, w & 255, 3, 1, 0x22, 0, 0xff, 0xd9]);
}

// --- the stand-in modules ------------------------------------------------------

const KEPT = new WeakMap();
const kept = (el) => (el && typeof el === 'object' ? KEPT.get(el) ?? el : el);

function element(kind, args = []) {
  const target = { kind, args, children: [], handlers: {}, id: null, calls: {} };
  const proxy = new Proxy({}, {
    get(_target, name) {
      if (typeof name !== 'string' || name === 'then') return undefined;
      return (...a) => {
        if (name === 'child') target.children.push(a[0]);
        else if (name === 'children') target.children.push(...a[0]);
        else if (name === 'when') {
          if (a[0]) assert.equal(a[1](proxy), proxy, 'a .when callback must return the element it was given');
        } else if (name === 'id') {
          assert.equal(typeof a[0], 'string');
          target.id = a[0];
        } else if (name === 'on_mouse_down' || name === 'on_mouse_up') target.handlers[`${name}:${a[0]}`] = a[1];
        else if (name.startsWith('on_')) target.handlers[name] = a[0];
        else if (name === 'hover') a[0](element('style'));
        else target.calls[name] = a;
        return proxy;
      };
    },
  });
  KEPT.set(proxy, target);
  return proxy;
}

const kit = {
  View: class View {},
  div: () => element('div'),
  svg: (file) => element('svg', [file]),
  image: (file) => element('image', [file]),
  v_flex: () => element('v_flex'),
  h_flex: () => element('h_flex'),
};

// Logs its call to $FAKE_LOG; what each fake does is in `exec` below.
function world({ ffmpeg = null, ffmpegFails = false, qlmanage = true, zoom = 1 } = {}) {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'media-mod-'));
  const w = {
    root,
    home: path.join(root, 'home'),
    drop: path.join(root, 'drop'),
    pending: 0,
    waiters: [],
    watchers: [],
    shared: {},
    props: null,
    toasts: [],
    ran: [],
    calls: [],
    layout: null,
    ffmpeg, ffmpegFails, qlmanage, zoom,
  };
  w.app = path.join(w.home, '.config', 'herdr-app');
  fs.mkdirSync(w.app, { recursive: true });
  fs.mkdirSync(path.join(w.app, 'media'));
  fs.writeFileSync(path.join(w.app, 'media', 'settings.json'), JSON.stringify({ folder: w.drop }));
  const colors = new Proxy({}, { get: (_t, name) => (typeof name === 'string' ? `#${name}` : undefined) });
  const real = new Set(['sh', 'cp', 'mkdir', 'rm', 'head', 'mv', 'test']);
  w.api = {
    theme: () => colors,
    settings: () => ({ zoom: w.zoom }),
    home: () => w.home,
    appHome: () => w.app,
    props: () => w.props,
    shared: (key) => w.shared[key],
    setShared: (key, value) => { w.shared[key] = JSON.parse(JSON.stringify(value)); return null; },
    layout: () => w.layout,
    run: (command, args) => { w.ran.push([command, args]); return null; },
    notify: (n) => { w.toasts.push(n); return `toast-${w.toasts.length}`; },
    // Waits until the test sends a command; while waiting it counts as at rest.
    nextCommand: () => new Promise((resolve) => {
      w.pending -= 1;
      w.waiters.push((command) => { w.pending += 1; resolve(command); });
    }),
    // Waits until the test says the folder changed (`w.changed()`).
    watchFolder: (dir) => new Promise((resolve, reject) => {
      if (!fs.existsSync(dir)) { reject(new Error(`no folder ${dir}`)); return; }
      w.pending -= 1;
      w.watchers.push(() => { w.pending += 1; resolve([dir]); });
    }),
    exec: async (command, args = []) => {
      w.calls.push([command, ...args]);
      if (command === 'sh' && args[1] === 'command -v ffmpeg') return w.ffmpeg ? { code: 0, stdout: `${w.ffmpeg}\n`, stderr: '' } : { code: 1, stdout: '', stderr: '' };
      if (real.has(command)) {
        const r = spawnSync(command, args, { encoding: 'utf8' });
        return { code: r.status, stdout: r.stdout, stderr: r.stderr };
      }
      if (command === w.ffmpeg) {
        if (w.ffmpegFails) return { code: 1, stdout: '', stderr: 'Unknown encoder libwebp' };
        fs.writeFileSync(args.at(-1), webp(640, 360, { animated: true }));
        return { code: 0, stdout: '', stderr: '' };
      }
      if (command === 'qlmanage') {
        if (!w.qlmanage) throw new Error('qlmanage: not installed');
        const out = args[args.indexOf('-o') + 1];
        fs.writeFileSync(path.join(out, `${path.basename(args.at(-1))}.png`), png(168, 120));
        return { code: 0, stdout: '', stderr: '' };
      }
      if (command === 'open') return { code: 0, stdout: '', stderr: '' };
      throw new Error(`${command}: not installed`);
    },
  };
  w.changed = () => { const ws = w.watchers.splice(0); for (const fire of ws) fire(); };
  w.drops = (name, data) => { fs.writeFileSync(path.join(w.drop, name), data); w.changed(); };
  return w;
}

const HERDR = ['theme', 'settings', 'home', 'appHome', 'props', 'shared', 'setShared', 'layout', 'run', 'notify', 'nextCommand', 'watchFolder', 'exec'];
const sources = {
  'gpui-kit': 'const k = globalThis.__kit; export const View = k.View, div = k.div, svg = k.svg, image = k.image;',
  'gpui-base': 'const k = globalThis.__kit; export const v_flex = k.v_flex, h_flex = k.h_flex;',
  herdr: HERDR.map((name) => `export const ${name} = (...a) => globalThis.__world.api.${name}(...a);`).join('\n'),
  // The app's fs/promises: Node's, with `exists`.
  'fs/promises': 'const f = globalThis.__fsp; export const readFile = f.readFile, writeFile = f.writeFile, readdir = f.readdir, unlink = f.unlink, mkdir = f.mkdir, rmdir = f.rmdir; export const exists = async (p) => { try { await f.access(p); return true; } catch { return false; } };',
};
globalThis.__kit = kit;
globalThis.__fsp = fsp;
registerHooks({
  resolve(specifier, context, next) {
    if (specifier in sources) return { url: `stand-in:${specifier}`, shortCircuit: true };
    return next(specifier, context);
  },
  load(url, context, next) {
    if (url.startsWith('stand-in:')) return { format: 'module', source: sources[url.slice(9)], shortCircuit: true };
    return next(url, url.includes('/mods/media/') && url.split('?')[0].endsWith('.js') ? { ...context, format: 'module' } : context);
  },
});
// Copies started with the same `module` name share the mod's modules (the store), as in the app.
registerHooks({
  resolve(specifier, context, next) {
    const resolved = next(specifier, context);
    const parent = context.parentURL ? new URL(context.parentURL) : null;
    if (!parent?.pathname.includes('/mods/media/') || !parent.search || !specifier.startsWith('.')) return resolved;
    const url = new URL(resolved.url);
    url.search = parent.search;
    return { ...resolved, url: url.href };
  },
});

const items = await import(new URL('../items.js', import.meta.url));

let copies = 0;
async function start(w, { props = null, module = `copy-${copies++}` } = {}) {
  globalThis.__world = w;
  w.props = props;
  const main = await import(new URL(`../main.js?module=${module}`, import.meta.url));
  const { store } = await import(new URL(`../store.js?module=${module}`, import.meta.url));
  const cx = {
    timers: [],
    notified: 0,
    notify() { this.notified += 1; },
    spawn(task) {
      w.pending += 1;
      Promise.resolve().then(() => task(cx)).catch((error) => { w.failed = error; }).finally(() => { w.pending -= 1; });
    },
    timer: { every: (ms, tick) => cx.timers.push({ ms, tick }) },
    stop_propagation() {},
  };
  const view = new main.default();
  view.init(null, cx);
  const settle = async () => {
    do await new Promise((resolve) => setImmediate(resolve)); while (w.pending > 0);
    if (w.failed) throw w.failed;
  };
  await settle();
  const mod = {
    view, cx, store, settle,
    render: () => view.render(cx),
    // A second passes (and a little more: a scan's own time varies, and a
    // case should not sit on the edge of a wait), and the timers fire.
    tick: async (ms = 1100) => {
      skew += ms;
      for (const timer of cx.timers) timer.tick(cx);
      await settle();
    },
    // Seconds pass, a tick each.
    wait: async (seconds) => { for (let i = 0; i < seconds; i++) await mod.tick(); },
    send: async (command) => {
      const waiter = w.waiters.shift();
      assert.ok(waiter, 'the mod waits for no command');
      waiter(command);
      await settle();
    },
    click: async (id) => {
      const el = byId(mod.render(), id);
      assert.ok(el, `nothing with the id ${id}; there is ${ids(mod.render(), 'media-').join(', ')}`);
      el.handlers.on_click({}, cx);
      await settle();
    },
    texts: () => texts(mod.render()),
    says: (text) => mod.texts().some((line) => line.includes(text)),
  };
  return mod;
}

function walk(el, visit) {
  const node = kept(el);
  if (node === null || typeof node !== 'object') return;
  visit(node);
  for (const child of node.children ?? []) walk(child, visit);
}
function texts(el) {
  const out = [];
  walk(el, (node) => { for (const c of node.children ?? []) if (typeof c === 'string') out.push(c); });
  return out;
}
function byId(root, id) {
  let found = null;
  walk(root, (node) => { if (node.id === id) found ??= node; });
  return found;
}
function ids(root, prefix) {
  const out = [];
  walk(root, (node) => { if (typeof node.id === 'string' && node.id.startsWith(prefix)) out.push(node.id); });
  return out;
}
function images(root) {
  const out = [];
  walk(root, (node) => { if (node.kind === 'image') out.push({ file: node.args[0], w: node.calls.w?.[0], h: node.calls.h?.[0] }); });
  return out;
}
const copiesIn = (w) => fs.readdirSync(path.join(w.app, 'media', 'items')).filter((f) => !f.startsWith('.')).sort();
const history = (w) => JSON.parse(fs.readFileSync(path.join(w.app, 'media', 'history.json'), 'utf8'));

// A layout like Herdr's: the right-hand panel column `panel` holding the stack panel-body.
function panelLayout({ hidden = true, active = 'files' } = {}) {
  return { id: 'root', type: 'row', children: [
    { id: 'work', type: 'column', children: [{ id: 'tabs', type: 'block' }, { id: 'main', type: 'block' }] },
    { id: 'panel', type: 'column', hidden, children: [{ id: 'panel-body', type: 'stack', active, children: [
      { id: 'files', type: 'block', content: { mod: 'files' } },
      { id: 'media-panel', type: 'block', content: { mod: 'media', props: { panel: true } } }] }] }],
    floating: [{ id: 'media-watch', content: { mod: 'media', props: { watch: true } } }] };
}

const cases = [];
const test = (name, run) => cases.push({ name, run });

// --- the item logic ------------------------------------------------------------

test('which files are media, and the listing the folder script prints', () => {
  assert.equal(items.kindOf('shot.png'), 'image');
  assert.equal(items.kindOf('Shot.JPEG'), 'image');
  assert.equal(items.kindOf('demo.gif'), 'image');
  assert.equal(items.kindOf('clip.mov'), 'video');
  assert.equal(items.kindOf('clip.webm'), 'video');
  assert.equal(items.kindOf('shot.txt'), 'caption');
  assert.equal(items.kindOf('notes.md'), null);
  assert.equal(items.kindOf('.shot.png'), null, 'hidden files are left alone');
  assert.equal(items.kindOf('shot.png.tmp'), null);
  assert.equal(items.kindOf('shot.png~'), null);
  assert.equal(items.kindOf('shot.png.part'), null);
  assert.deepEqual(items.parseListing('12\ta.png\n0\tb b.gif\nnope\n\n7\t\n'), [{ name: 'a.png', size: 12 }, { name: 'b b.gif', size: 0 }]);
});

test('a file is taken in once its size holds, and after a moment for its caption when it has none', () => {
  const state = { known: {}, pending: {} };
  const t0 = 1_000_000;
  assert.deepEqual(items.plan([{ name: 'a.png', size: 10 }], state, t0).take, [], 'first seen');
  assert.deepEqual(items.plan([{ name: 'a.png', size: 20 }], state, t0 + 1500).take, [], 'still growing');
  assert.deepEqual(items.plan([{ name: 'a.png', size: 20 }], state, t0 + 2000).take, [], 'not stable for a second yet');
  const { take } = items.plan([{ name: 'a.png', size: 20 }], state, t0 + 2600);
  assert.deepEqual(take, [{ name: 'a.png', size: 20, kind: 'image', caption: null }]);
  // An empty file waits.
  const s2 = { known: {}, pending: {} };
  items.plan([{ name: 'e.png', size: 0 }], s2, t0);
  assert.deepEqual(items.plan([{ name: 'e.png', size: 0 }], s2, t0 + 5000).take, []);
  // Without a caption it waits CAPTION_WAIT_MS from first seen, however stable.
  const s3 = { known: {}, pending: {} };
  items.plan([{ name: 'b.png', size: 5 }], s3, t0);
  assert.deepEqual(items.plan([{ name: 'b.png', size: 5 }], s3, t0 + items.STABLE_MS).take, []);
  assert.equal(items.plan([{ name: 'b.png', size: 5 }], s3, t0 + items.CAPTION_WAIT_MS).take.length, 1);
  // A caption that comes meanwhile is taken with it, once it holds too.
  const s4 = { known: {}, pending: {} };
  items.plan([{ name: 'c.png', size: 5 }], s4, t0);
  const both = [{ name: 'c.png', size: 5 }, { name: 'c.txt', size: 9 }];
  assert.deepEqual(items.plan(both, s4, t0 + 1200).take, [], 'the caption was just seen');
  assert.deepEqual(items.plan(both, s4, t0 + 1800).take, [{ name: 'c.png', size: 5, kind: 'image', caption: 'c.txt' }]);
});

test('files taken in are known, a later caption of one is late, and what leaves the folder is forgotten', () => {
  const state = { known: { 'a.png': 20 }, pending: {} };
  const t0 = 2_000_000;
  assert.deepEqual(items.plan([{ name: 'a.png', size: 20 }], state, t0), { take: [], late: [] });
  const files = [{ name: 'a.png', size: 20 }, { name: 'a.txt', size: 7 }];
  assert.deepEqual(items.plan(files, state, t0 + 1000).late, []);
  assert.deepEqual(items.plan(files, state, t0 + 1600).late, [{ name: 'a.png', caption: 'a.txt', size: 7 }]);
  state.known['a.txt'] = 7;
  assert.deepEqual(items.plan(files, state, t0 + 3000).late, [], 'once');
  // Overwritten with another size: a new file.
  items.plan([{ name: 'a.png', size: 99 }], state, t0 + 4000);
  assert.ok(state.pending['a.png']);
  assert.deepEqual(state.known, { 'a.png': 20 }, 'the caption that left is forgotten');
  items.plan([], state, t0 + 5000);
  assert.deepEqual(state, { known: {}, pending: {} });
});

test('captions, the history bound, the unseen count', () => {
  assert.equal(items.captionText('\n  Before and after  \nmore'), 'Before and after');
  assert.equal(items.captionText(''), '');
  assert.equal(items.captionText('x'.repeat(400)).length, 300);
  const list = [];
  for (let i = 0; i < 52; i++) {
    const gone = items.addItem(list, { id: `i${i}`, at: i });
    assert.deepEqual(gone.map((g) => g.id), i < 50 ? [] : [`i${i - 50}`]);
  }
  assert.equal(list.length, items.KEEP);
  assert.equal(list[0].id, 'i51');
  assert.equal(items.unseenCount(list, 48), 3);
  assert.equal(items.unseenCount(list, undefined), 50);
  assert.deepEqual(items.filesOf({ file: 'media/items/a.png' }), ['media/items/a.png']);
  assert.deepEqual(items.filesOf({ file: '/elsewhere/a.png' }), [], 'only the mod\'s own copies');
});

test('an image\'s size and whether it moves, from its first bytes', () => {
  assert.deepEqual(items.imageInfo(png(640, 400), 'a.png'), { format: 'png', width: 640, height: 400, animated: false });
  assert.deepEqual(items.imageInfo(png(32, 16, { animated: true }), 'a.png'), { format: 'apng', width: 32, height: 16, animated: true });
  assert.deepEqual(items.imageInfo(gif(240, 120, 1), 'a.gif'), { format: 'gif', width: 240, height: 120, animated: false });
  assert.deepEqual(items.imageInfo(gif(240, 120, 3), 'a.gif'), { format: 'gif', width: 240, height: 120, animated: true });
  assert.deepEqual(items.imageInfo(webp(1200, 484, { animated: true }), 'a.webp'), { format: 'webp', width: 1200, height: 484, animated: true });
  assert.deepEqual(items.imageInfo(webp(300, 200), 'a.webp'), { format: 'webp', width: 300, height: 200, animated: false });
  assert.deepEqual(items.imageInfo(jpeg(1024, 768), 'a.jpg'), { format: 'jpeg', width: 1024, height: 768, animated: false });
  assert.deepEqual(items.imageInfo(Buffer.from('<?xml version="1.0"?><svg xmlns="x" viewBox="0 0 48 24"><g/></svg>'), 'a.svg'), { format: 'svg', width: 48, height: 24, animated: false });
  assert.deepEqual(items.imageInfo(Buffer.from('<svg width="10px" height="20"/>'), 'a.svg'), { format: 'svg', width: 10, height: 20, animated: false });
  assert.deepEqual(items.imageInfo(Buffer.from('nonsense'), 'a.png'), { format: 'png', width: null, height: null, animated: false });
});

test('fitting a picture into a box, and how long ago', () => {
  assert.deepEqual(items.fit(640, 400, 320, 500), { w: 320, h: 200 });
  assert.deepEqual(items.fit(300, 900, 320, 300), { w: 100, h: 300 });
  assert.deepEqual(items.fit(100, 50, 320, 300), { w: 100, h: 50 }, 'never larger than it is');
  assert.deepEqual(items.fit(10, 5, 320, 300, { grow: true }), { w: 320, h: 160 });
  assert.deepEqual(items.fit(null, null, 400, 400), { w: 400, h: 300 }, 'unknown: 4:3');
  const now = 10_000_000;
  assert.equal(items.ago(now - 10_000, now), 'just now');
  assert.equal(items.ago(now - 5 * 60_000, now), '5 min ago');
  assert.equal(items.ago(now - 3 * 3600_000, now), '3 h ago');
  assert.equal(items.ago(now - 30 * 3600_000, now), 'yesterday');
  assert.equal(items.ago(now - 72 * 3600_000, now), '3 days ago');
});

test('whether the panel shows, and the commands that show it', () => {
  assert.equal(items.panelShown(panelLayout()), false, 'its column is hidden');
  assert.equal(items.panelShown(panelLayout({ hidden: false })), false, 'Files shows in the stack');
  assert.equal(items.panelShown(panelLayout({ hidden: false, active: 'media-panel' })), true);
  assert.equal(items.panelShown({ id: 'root', children: [] }), false);
  assert.deepEqual(items.showCommands(panelLayout()), [
    ['layout.select', { stack: 'panel-body', child: 'media-panel' }], ['layout.show', { id: 'panel' }]]);
  assert.deepEqual(items.showCommands({ id: 'root' }), []);
});

// --- the mod --------------------------------------------------------------------

test('a picture dropped with a caption: a toast that shows the panel, the count, the copy it draws, and the panel', async () => {
  const w = world();
  w.layout = panelLayout();
  const watcher = await start(w, { props: { watch: true }, module: 'm1' });
  assert.ok(fs.existsSync(w.drop), 'the drop folder is made');
  assert.equal(kept(watcher.render()).children.length, 0, 'the watcher draws nothing');
  const panel = await start(w, { props: { panel: true }, module: 'm1' });
  kept(panel.render()).handlers.on_size_change({ width: 700, height: 900 }, panel.cx);
  assert.ok(panel.says('Nothing to show yet'));
  assert.ok(panel.says(w.drop), 'the empty panel names the drop folder');

  w.drops('signups.png', png(640, 400));
  w.changed();
  await panel.tick();
  assert.equal(panel.store.items.length, 0, 'not taken in at once: it may still be written');
  w.drops('signups.txt', 'Sign-ups after the change\nmore\n');
  w.changed();
  await panel.wait(3);
  assert.equal(panel.store.items.length, 1);
  const [item] = panel.store.items;
  assert.equal(item.caption, 'Sign-ups after the change');
  assert.deepEqual(copiesIn(w), [`${item.id}.png`]);
  assert.equal(w.toasts.length, 1);
  assert.equal(w.toasts[0].title, 'Sign-ups after the change');
  assert.deepEqual(w.toasts[0].action, { name: 'media.show' });
  assert.equal(w.toasts[0].icon, `media/items/${item.id}.png`, 'the picture is the toast\'s icon');
  assert.equal(w.shared['media.unseen'], 1);
  assert.ok(fs.existsSync(path.join(w.drop, 'signups.png')), 'the dropped file stays');

  // The toast's click: media.show selects the panel and shows its column; seen, the count is 0.
  await watcher.send('show');
  assert.deepEqual(w.ran, [['layout.select', { stack: 'panel-body', child: 'media-panel' }], ['layout.show', { id: 'panel' }]]);
  w.layout = panelLayout({ hidden: false, active: 'media-panel' });
  await panel.tick();
  assert.equal(w.shared['media.unseen'], 0);
  assert.ok(panel.says('Sign-ups after the change'));
  assert.ok(panel.says('signups.png'));
  assert.ok(panel.says('Newest'));
  assert.deepEqual(images(panel.render()), [{ file: `media/items/${item.id}.png`, w: 640, h: 400 }]);

  // A second becomes the newest, the first is in the list below; seen at once, no toast while the panel shows.
  w.drops('dialog.png', png(800, 500));
  w.changed();
  await panel.wait(3);
  assert.equal(panel.store.items.length, 2);
  assert.equal(w.toasts.length, 1);
  assert.equal(w.shared['media.unseen'], 0);
  assert.ok(byId(panel.render(), `media-item-${item.id}`), 'the first is listed');
  assert.ok(panel.says('ALL'));
  const order = () => ids(panel.render(), 'media-item-');
  const before = order();
  assert.deepEqual(before, panel.store.items.map((i) => `media-item-${i.id}`), 'every item, newest first');
  const big = images(byId(panel.render(), 'media-big'))[0];
  assert.equal(big.file, `media/items/${panel.store.items[0].id}.png`);
  // Clicking the earlier one shows it large.
  await panel.click(`media-item-${item.id}`);
  assert.equal(images(byId(panel.render(), 'media-big'))[0].file, `media/items/${item.id}.png`);
  assert.deepEqual(order(), before, 'clicking does not reorder the list');
  assert.ok('bg' in kept(byId(panel.render(), `media-item-${item.id}`)).calls, 'the shown one is marked');
});

test('the picture fits the panel at the app\'s zoom; several at once make one toast', async () => {
  const w = world({ zoom: 1.25 });
  w.layout = panelLayout();
  const panel = await start(w, { props: { panel: true }, module: 'm2' });
  kept(panel.render()).handlers.on_size_change({ width: 500, height: 900 }, panel.cx);
  w.drops('a.png', png(1600, 400));
  w.drops('b.gif', gif(240, 120, 3));
  w.drops('b.txt', 'The spinner');
  await panel.wait(4);
  assert.equal(w.toasts.length, 1);
  assert.equal(w.toasts[0].title, '2 new items in Media');
  assert.equal(w.shared['media.unseen'], 2);
  await panel.click('media-item-' + panel.store.items[1].id); // a.png
  // 500 on screen at zoom 1.25 is 400 in w() units; less 38 of margins and frame.
  assert.deepEqual(images(byId(panel.render(), 'media-big'))[0], { file: `media/items/${panel.store.items[1].id}.png`, w: 362, h: 91 });
  // The GIF: its first frame, said so, with Open.
  panel.view.selected = null;
  assert.ok(panel.says('Animated GIF: the panel shows its first frame; Open plays it'));
  await panel.click('media-open');
  assert.deepEqual(w.calls.filter((c) => c[0] === 'open'), [['open', path.join(w.drop, 'b.gif')]]);
});

test('a caption that comes after the picture was taken in is added to it', async () => {
  const w = world();
  const panel = await start(w, { props: { panel: true }, module: 'm3' });
  w.drops('late.png', png(10, 10));
  await panel.wait(4);
  assert.equal(panel.store.items[0].caption, '');
  assert.equal(w.toasts[0].title, 'late.png', 'no caption: the file name');
  w.drops('late.txt', 'Came late');
  await panel.wait(3);
  assert.equal(panel.store.items[0].caption, 'Came late');
  assert.equal(history(w).items[0].caption, 'Came late', 'saved');
});

test('a file being written is taken in once whole', async () => {
  const w = world();
  const panel = await start(w, { module: 'm4' });
  const whole = png(200, 100);
  w.drops('big.png', whole.subarray(0, 30));
  await panel.tick();
  for (let i = 1; i <= 4; i++) {
    w.drops('big.png', whole.subarray(0, Math.min(whole.length, 30 + i * 60)));
    await panel.tick();
    assert.equal(panel.store.items.length, 0, `growing (${i})`);
  }
  w.drops('big.png', whole);
  await panel.wait(4);
  assert.equal(panel.store.items.length, 1);
  assert.deepEqual([panel.store.items[0].width, panel.store.items[0].height], [200, 100]);
  assert.deepEqual(fs.readFileSync(path.join(w.app, panel.store.items[0].file)), whole);
});

test('the history keeps 50, deleting the copies of what leaves it and never a dropped file', async () => {
  const w = world();
  const panel = await start(w, { module: 'm5' });
  for (let i = 0; i < 48; i++) w.drops(`s${String(i).padStart(2, '0')}.png`, png(4, 4));
  await panel.wait(4);
  assert.equal(panel.store.items.length, 48);
  for (let i = 48; i < 53; i++) w.drops(`s${i}.png`, png(4, 4));
  await panel.wait(4);
  assert.equal(panel.store.items.length, items.KEEP);
  assert.equal(copiesIn(w).length, items.KEEP);
  assert.equal(fs.readdirSync(w.drop).length, 53, 'the drop folder keeps them all');
  assert.equal(history(w).items.length, items.KEEP);
});

test('Remove takes one out and deletes its copy; Clear all, asked first, deletes the dropped files and their captions', async () => {
  const w = world();
  const panel = await start(w, { props: { panel: true }, module: 'm6' });
  w.drops('a.png', png(4, 4)); w.drops('a.txt', 'A');
  w.drops('b.png', png(4, 4));
  w.drops('notes.md', 'not media');
  await panel.wait(4);
  assert.equal(panel.store.items.length, 2);
  const b = panel.store.items[0];
  await panel.click('media-remove');
  assert.deepEqual(panel.store.items.map((i) => i.name), ['a.png']);
  assert.ok(!copiesIn(w).includes(`${b.id}.png`));
  assert.ok(fs.existsSync(path.join(w.drop, 'b.png')), 'Remove leaves the dropped file');
  await panel.wait(16); // a full scan: it does not come back
  assert.equal(panel.store.items.length, 1);

  await panel.click('media-clear');
  assert.ok(panel.says('Remove the item? The files they came from are deleted from the drop folder too.'));
  await panel.click('media-clear-no');
  assert.equal(panel.store.items.length, 1);
  await panel.click('media-clear');
  await panel.click('media-clear-yes');
  assert.equal(panel.store.items.length, 0);
  assert.deepEqual(copiesIn(w), []);
  assert.deepEqual(fs.readdirSync(w.drop).sort(), ['notes.md'], 'every media file it took in and their captions are gone; nothing else');
  assert.ok(panel.says('Nothing to show yet'));
});

test('after a restart: the history is read again, nothing is taken in twice, and stray copies are deleted', async () => {
  const w = world();
  const first = await start(w, { module: 'm7a' });
  w.drops('a.png', png(4, 4));
  await first.wait(4);
  assert.equal(w.toasts.length, 1);
  fs.writeFileSync(path.join(w.app, 'media', 'items', 'stray.png'), 'x');
  const again = await start(w, { module: 'm7b' });
  await again.wait(4);
  assert.deepEqual(again.store.items.map((i) => i.name), ['a.png']);
  assert.equal(w.toasts.length, 1, 'no toast for what was taken in before');
  assert.deepEqual(copiesIn(w), [`${again.store.items[0].id}.png`]);
});

test('a first run takes in what the folder holds already, without a toast or a count', async () => {
  const w = world();
  fs.mkdirSync(w.drop);
  fs.writeFileSync(path.join(w.drop, 'old.png'), png(4, 4));
  const mod = await start(w, { module: 'm8' });
  await mod.wait(5);
  assert.equal(mod.store.items.length, 1);
  assert.equal(w.toasts.length, 0);
  assert.equal(w.shared['media.unseen'], 0);
  w.drops('new.png', png(4, 4));
  await mod.wait(4);
  assert.equal(w.toasts.length, 1);
  assert.equal(w.shared['media.unseen'], 1);
});

test('a video without ffmpeg: a still frame from Quick Look, Open plays the clip; without Quick Look, no preview', async () => {
  const w = world();
  const panel = await start(w, { props: { panel: true }, module: 'm9' });
  w.drops('demo.mov', Buffer.alloc(5000, 1));
  await panel.wait(4);
  const [item] = panel.store.items;
  assert.equal(item.preview, 'still');
  assert.equal(item.file, `media/items/${item.id}.png`);
  assert.deepEqual([item.width, item.height], [168, 120]);
  assert.deepEqual(copiesIn(w), [`${item.id}.png`], 'the clip itself is not copied, and Quick Look\'s folder is gone');
  assert.ok(panel.says('Video: a still frame; Open plays it'));
  assert.ok(w.calls.some((c) => c[0] === 'qlmanage' && c.includes('-t') && c.at(-1) === path.join(w.drop, 'demo.mov')));
  await panel.click('media-open');
  assert.deepEqual(w.calls.filter((c) => c[0] === 'open').at(-1), ['open', path.join(w.drop, 'demo.mov')]);

  w.qlmanage = false;
  w.drops('other.webm', Buffer.alloc(100, 1));
  await panel.wait(4);
  assert.equal(panel.store.items[0].preview, 'none');
  assert.equal(panel.store.items[0].file, null);
  assert.ok(panel.says('Video: no preview; Open plays it'));
});

test('a video with ffmpeg: an animated WebP of at most 10 seconds and 640 wide, made in the background; a failed one falls back to the still', async () => {
  const w = world({ ffmpeg: '/opt/fake/bin/ffmpeg' });
  const panel = await start(w, { props: { panel: true }, module: 'm10' });
  w.drops('run.mp4', Buffer.alloc(5000, 1));
  await panel.wait(4);
  const [item] = panel.store.items;
  assert.equal(item.preview, 'converted');
  assert.equal(item.file, `media/items/${item.id}.webp`);
  assert.deepEqual([item.width, item.height], [640, 360]);
  const call = w.calls.find((c) => c[0] === '/opt/fake/bin/ffmpeg');
  assert.deepEqual(call.slice(1), ['-nostdin', '-v', 'error', '-y', '-t', '10', '-i', path.join(w.drop, 'run.mp4'), '-an',
    '-vf', "fps=12,scale='min(640,iw)':-2:flags=lanczos", '-loop', '0', '-c:v', 'libwebp', '-q:v', '70', path.join(w.app, 'media', 'items', `${item.id}.webp`)]);
  assert.ok(panel.says('Video: the panel shows its first frame; Open plays it'));
  assert.ok(!w.calls.some((c) => c[0] === 'qlmanage'));

  w.ffmpegFails = true;
  w.drops('run2.mp4', Buffer.alloc(5000, 2));
  await panel.wait(4);
  assert.equal(panel.store.items[0].preview, 'still');
  assert.ok(!copiesIn(w).some((f) => f === `${panel.store.items[0].id}.webp`), 'the failed clip is deleted');
});

test('a video still converting says so; one removed meanwhile leaves no file behind', async () => {
  const w = world({ ffmpeg: '/opt/fake/bin/ffmpeg' });
  let release;
  const held = new Promise((resolve) => { release = resolve; });
  const exec = w.api.exec;
  w.api.exec = async (command, args) => {
    if (command === '/opt/fake/bin/ffmpeg') { w.pending -= 1; await held; w.pending += 1; }
    return exec(command, args);
  };
  const panel = await start(w, { props: { panel: true }, module: 'm11' });
  w.drops('slow.mov', Buffer.alloc(100, 1));
  await panel.wait(4);
  assert.equal(panel.store.items[0].state, 'converting');
  assert.ok(panel.says('Making a preview…'));
  assert.ok(panel.says('Video: making a preview…'));
  await panel.click('media-remove');
  release();
  await panel.settle();
  assert.deepEqual(copiesIn(w), []);
});

test('a drop folder that cannot be made says so in the panel', async () => {
  const w = world();
  fs.writeFileSync(path.join(w.root, 'file'), 'a file, not a folder');
  fs.writeFileSync(path.join(w.app, 'media', 'settings.json'), JSON.stringify({ folder: path.join(w.root, 'file', 'drop') }));
  const panel = await start(w, { props: { panel: true }, module: 'm12' });
  await panel.wait(2);
  assert.ok(panel.says(`Could not make the folder ${path.join(w.root, 'file', 'drop')}`));
  // Another folder in the settings is taken up.
  fs.writeFileSync(path.join(w.app, 'media', 'settings.json'), JSON.stringify({ folder: '~/drops' }));
  await panel.wait(11);
  assert.equal(panel.store.folder, path.join(w.home, 'drops'));
  assert.ok(fs.existsSync(path.join(w.home, 'drops')));
  assert.ok(!panel.texts().some((t) => t.startsWith('Could not')));
});

test('in the panel it is a card as Files and Changes are; elsewhere it fills its place', async () => {
  const w = world();
  const panel = await start(w, { props: { panel: true }, module: 'm13' });
  assert.ok('rounded' in kept(panel.render()).calls && 'border_1' in kept(panel.render()).calls);
  const tab = await start(w, { module: 'm14' });
  assert.ok(!('rounded' in kept(tab.render()).calls));
  assert.ok(tab.says('Media'));
});

// --- install.sh -----------------------------------------------------------------

// Herdr's tab bar (`mods/tabs/main.js`, 0.1.15) as far as the button needs it.
const STOCK_TABS = `import { View, div, svg, image } from "gpui-kit";
import { h_flex } from "gpui-base";
import { spaces, panes, run, openOverlay, theme, window, shared, layout, settings } from "herdr";

function find(node, id) {
  if (node?.id === id) return node;
  for (const child of node?.children ?? []) {
    const found = find(child, id);
    if (found) return found;
  }
  return null;
}

export default class Tabs extends View {
  init() {
    this.panels = JSON.parse(localStorage.getItem("panels") ?? "{}");
  }

  syncPanel(space, panel, body, enabled) {
    if (!space || !panel) return;
    this.enabled = enabled;
  }

  render() {
    const t = theme();
    const space = spaces().find(p => p.active);
    const stop = (_e, cx) => cx.stop_propagation();
    const button = (id, label, action) => div().id(id).size(26).on_mouse_down("left", stop).on_click(action).child(label);
    const root = layout(), w = window();
    const panel = find(root, "panel"), body = find(root, "panel-body");
    const enabled = shared("workspace.panels") ?? ["files", "changes"];
    this.syncPanel(space, panel, body, enabled);
    const panelButton = (id, label, icon) => {
      const on = panel && !panel.hidden && body?.active === id;
      return h_flex().id(\`panel-\${id}\`).h(26).px(8).gap(5).flex_none().items_center().rounded(6)
        .text_size(12).bg(on ? t.selected : t.chrome).text_color(on ? t.foreground : t.muted)
        .hover(s => s.bg(t.hover)).on_mouse_down("left", stop)
        .on_click(() => {
          if (on) run("layout.hide", { id: "panel" });
          else { run("layout.select", { stack: "panel-body", child: id }); run("layout.show", { id: "panel" }); }
        }).child(svg(icon).size(14)).child(label);
    };
    return h_flex().size_full().min_w(0).px(10).gap(3).items_center().bg(t.chrome)
      .child(button("tab-new", "+", () => openOverlay({ mod: "new-menu", kind: "popup" })))
      .when(enabled.includes("files"), el => el.child(panelButton("files", "Files", "mods/files/file.svg")))
      .when(enabled.includes("changes"), el => el.child(panelButton("changes", "Changes", "mods/changes/changes.svg")))
      .when(w?.buttons, el => el.child(button("minimize", w.platform === "windows" ? svg("mods/tabs/win-min.svg").size(16) : "−", () => run("window.minimize")))
        .child(button("close-window", w.platform === "windows" ? svg("mods/tabs/win-close.svg").size(16) : svg("mods/pane/close.svg").size(13), () => run("window.close"))));
  }
}
`;

// What Corgi's app-layout (src/app_layout/panel.rs) does to a tab bar: its
// list line above the syncPanel call and the comment lines just over it, its
// button before the window buttons.
function corgiLines(text) {
  const lines = text.split(/(?<=\n)/);
  const call = lines.findIndex((l) => l.trimStart().startsWith('this.syncPanel('));
  let at = call;
  while (at > 0 && lines[at - 1].trimStart().startsWith('//')) at -= 1;
  const indent = lines[call].match(/^\s*/)[0];
  lines.splice(at, 0, `${indent}// >>> Corgi questions panel: corgi app-layout adds it\n`,
    `${indent}try { if (find(body, "corgi-questions") && !enabled.includes("corgi-questions")) enabled.push("corgi-questions"); } catch (_) {}\n`,
    `${indent}// <<< Corgi questions panel\n`);
  return lines.join('').replace('      .when(w?.buttons', '      // >>> Corgi questions button\n      .when(Boolean(find(body, "corgi-questions")), el => el.child(panelButton("corgi-questions", "Questions", "mods/corgi/question.svg")))\n      // <<< Corgi questions button\n      .when(w?.buttons');
}

// A tab bar's main.js run as the app runs it: `render()` gives its panel
// buttons, each with its texts; `enabled` is the list its syncPanel got.
function tabBar(text, { stack = ['files', 'changes', 'media-panel'], unseen } = {}) {
  const body = { id: 'panel-body', type: 'stack', active: 'files', children: stack.map((id) => ({ id })) };
  const panel = { id: 'panel', hidden: true, children: [body] };
  const root = { id: 'root', children: [{ id: 'tabs' }, panel] };
  const colors = new Proxy({}, { get: (_t, name) => (typeof name === 'string' ? `#${name}` : undefined) });
  const code = text.replace(/^import .*$/gm, '').replace('export default class', 'return class');
  // eslint-disable-next-line no-new-func
  const Tabs = new Function('View', 'div', 'svg', 'image', 'h_flex', 'spaces', 'panes', 'run', 'openOverlay', 'theme', 'window', 'shared', 'layout', 'settings', 'localStorage', code)(
    kit.View, kit.div, kit.svg, kit.svg, kit.h_flex, () => [{ id: 's1', active: true }], () => [], () => {}, () => {}, () => colors, () => ({}),
    (key) => (key === 'media.unseen' ? unseen : undefined), () => root, () => ({ zoom: 1 }), { getItem: () => null, setItem: () => {} });
  const bar = new Tabs();
  bar.init();
  return {
    render() {
      const el = bar.render();
      return ids(el, 'panel-').map((id) => ({ id, texts: texts(byId(el, id)) }));
    },
    get enabled() { return bar.enabled; },
  };
}

// A scratch app home: Herdr's tab bar, its installed.json, a personal
// layout `mine` (as a clone of main) whose tab bar is `bar`.
function appHome(bar = 'tabs') {
  const home = fs.mkdtempSync(path.join(os.tmpdir(), 'media-install-'));
  const app = path.join(home, 'app');
  fs.mkdirSync(path.join(app, 'mods', 'tabs'), { recursive: true });
  fs.mkdirSync(path.join(app, 'layouts'));
  fs.writeFileSync(path.join(app, 'mods', 'tabs', 'main.js'), STOCK_TABS);
  fs.writeFileSync(path.join(app, 'mods', 'tabs', 'mod.json'), '{ "place": "none" }\n');
  fs.writeFileSync(path.join(app, 'installed.json'), JSON.stringify({ tabs: '1', files: '1', 'layout:main': '1' }));
  const layout = { id: 'root', type: 'row', note: 'räksmörgås', children: [
    { id: 'sidebar', type: 'column', size: { px: 240 }, children: [{ id: 'spaces', type: 'block', content: { mod: 'spaces' } }] },
    { id: 'work', type: 'column', children: [
      { id: 'tabs', type: 'block', size: { px: 30 }, content: { mod: bar } },
      { id: 'work-body', type: 'row', children: [{ id: 'main', type: 'block', content: { tab: 'active' } },
        { id: 'panel', type: 'column', hidden: true, size: { px: 368 }, children: [
          { id: 'panel-body', type: 'stack', active: 'files', children: [
            { id: 'files', type: 'block', content: { mod: 'files' } }, { id: 'changes', type: 'block', content: { mod: 'changes' } }] }] }] }] }],
    cloned_from: { name: 'main', version: '0.1.15' } };
  fs.writeFileSync(path.join(app, 'layouts', 'mine.json'), `${JSON.stringify(layout, null, 2)}\n`);
  fs.writeFileSync(path.join(app, 'shared.json'), JSON.stringify({ layout: 'mine' }));
  const checker = path.join(home, 'herdr-app');
  fs.writeFileSync(checker, '#!/bin/sh\necho "$@" >> "$0.log"\nif [ -e "$0.refuse" ]; then echo "SyntaxError: nope" >&2; exit 1; fi\n: > "$2/jsconfig.json"\n', { mode: 0o755 });
  const runIn = (script) => (...args) => {
    const r = spawnSync('sh', [script, ...args, '--home', app], { env: { ...process.env, HERDR_APP_BIN: checker, HERDR_APP_HOME: '' }, encoding: 'utf8' });
    return { code: r.status, out: `${r.stdout}${r.stderr}` };
  };
  const tree = (dir) => fs.readdirSync(dir, { recursive: true }).filter((f) => !f.includes('jsconfig')).sort();
  return {
    home, app, checker,
    run: runIn(path.join(MOD, 'install.sh')),
    jenkins: runIn(path.join(JENKINS, 'install.sh')),
    read: (name = 'mine') => JSON.parse(fs.readFileSync(path.join(app, 'layouts', `${name}.json`), 'utf8')),
    main: (bar) => fs.readFileSync(path.join(app, 'mods', bar, 'main.js'), 'utf8'),
    mods: () => fs.readdirSync(path.join(app, 'mods')).sort(),
    tree,
  };
}
const find = (node, id) => (node?.id === id ? node : (node?.children ?? []).map((c) => find(c, id)).find(Boolean) ?? null);

test('install.sh puts the mod\'s own files in an app home, once; --folder names the drop folder; --uninstall takes it all out', () => {
  const h = appHome();
  const target = path.join(h.app, 'mods', 'media');
  let r = h.run();
  assert.equal(r.code, 0, r.out);
  assert.deepEqual(fs.readdirSync(target).sort(), ['icons', 'items.js', 'main.js', 'mod.json', 'store.js'], 'no tests, no README, no installer, nothing the check wrote');
  const modules = fs.readdirSync(MOD).filter((f) => f.endsWith('.js'));
  assert.deepEqual(modules.filter((f) => !fs.existsSync(path.join(target, f))), [], 'every module of the mod is installed');
  assert.equal(fs.readdirSync(path.join(target, 'icons')).length, fs.readdirSync(path.join(MOD, 'icons')).length);
  assert.ok(fs.readFileSync(`${h.checker}.log`, 'utf8').startsWith('check '), 'checked before it was put in place');
  assert.ok(!fs.readdirSync(path.join(h.app, 'mods')).some((f) => f.startsWith('.')), 'no staging folder is left');
  assert.deepEqual(h.read().children[1].children[0].content, { mod: 'tabs' }, 'no --panel: no layout changes');
  const before = fs.statSync(path.join(target, 'main.js')).mtimeMs;
  assert.equal(h.run().code, 0);
  assert.equal(fs.statSync(path.join(target, 'main.js')).mtimeMs, before, 'again: nothing changes');

  const drop = path.join(h.home, 'drops');
  r = h.run('--folder', drop);
  assert.equal(r.code, 0, r.out);
  assert.deepEqual(JSON.parse(fs.readFileSync(path.join(h.app, 'media', 'settings.json'), 'utf8')), { folder: drop });
  assert.ok(fs.existsSync(drop));
  r = h.run('--folder', path.join(h.app, 'drops'));
  assert.notEqual(r.code, 0);
  assert.match(r.out, /must be outside the app's home/);

  fs.mkdirSync(path.join(h.app, 'media', 'items'));
  fs.writeFileSync(path.join(drop, 'kept.png'), 'x');
  r = h.run('--uninstall');
  assert.equal(r.code, 0, r.out);
  assert.ok(!fs.existsSync(target) && !fs.existsSync(path.join(h.app, 'media')));
  assert.ok(fs.existsSync(path.join(drop, 'kept.png')), 'the drop folder stays');

  // Another mod of the same name is never replaced or removed.
  fs.mkdirSync(target);
  fs.writeFileSync(path.join(target, 'main.js'), '// somebody else\'s\n');
  assert.notEqual(h.run().code, 0);
  assert.notEqual(h.run('--uninstall').code, 0);
  assert.equal(fs.readFileSync(path.join(target, 'main.js'), 'utf8'), '// somebody else\'s\n');
  fs.rmSync(h.home, { recursive: true });
});

test('install.sh --panel right: the panel in the stack, the watcher, a Media button with the count; --panel off and --uninstall put it all back', () => {
  const h = appHome();
  const layoutBefore = h.read();
  const modsBefore = h.mods();
  let r = h.run('--panel', 'right');
  assert.equal(r.code, 0, r.out);
  const mine = h.read();
  const stack = find(mine, 'panel-body');
  assert.deepEqual(stack.children.at(-1), { id: 'media-panel', type: 'block', content: { mod: 'media', props: { panel: true } } });
  assert.equal(stack.active, 'files', 'the shown child stays');
  assert.deepEqual(mine.floating, [{ id: 'media-watch', content: { mod: 'media', props: { watch: true } }, edge: 'bottom', align: 'end' }]);
  assert.equal(find(mine, 'tabs').content.mod, 'tabs-media');
  assert.equal(mine.note, 'räksmörgås');
  assert.equal(h.main('tabs'), STOCK_TABS, 'Herdr\'s own is never changed');
  const clone = h.main('tabs-media');
  assert.match(clone.split('\n')[0], /^\/\/ Herdr's tabs, cloned by mods\/media\/install\.sh/);
  assert.ok(!clone.includes('mods/tabs/'), 'asset paths point at the copy');
  assert.deepEqual(JSON.parse(fs.readFileSync(path.join(h.app, 'mods', 'tabs-media', 'mod.json'), 'utf8')).cloned_from, { name: 'tabs' });
  // The tab bar as the app runs it: the button, with the count while there is one, and the panel in syncPanel's list.
  let bar = tabBar(clone);
  assert.deepEqual(bar.render(), [{ id: 'panel-files', texts: ['Files'] }, { id: 'panel-changes', texts: ['Changes'] }, { id: 'panel-media-panel', texts: ['Media'] }]);
  assert.deepEqual(bar.enabled, ['files', 'changes', 'media-panel']);
  assert.deepEqual(tabBar(clone, { unseen: 3 }).render().at(-1), { id: 'panel-media-panel', texts: ['Media', '3'] });
  assert.deepEqual(tabBar(clone, { stack: ['files', 'changes'] }).render().map((b) => b.id), ['panel-files', 'panel-changes'], 'no panel, no button');

  // Again: nothing changes.
  const text = JSON.stringify(mine);
  r = h.run('--panel', 'right');
  assert.equal(r.code, 0, r.out);
  assert.equal(JSON.stringify(h.read()), text);
  assert.equal(h.main('tabs-media'), clone);

  r = h.run('--panel', 'off');
  assert.equal(r.code, 0, r.out);
  assert.deepEqual(h.read(), layoutBefore, 'the layout as it was');
  assert.deepEqual(h.mods(), [...modsBefore, 'media'].sort(), 'the copy of the tab bar is gone, the mod stays');
  r = h.run('--panel', 'right');
  assert.equal(r.code, 0, r.out);
  r = h.run('--panel', 'off');
  assert.equal(r.code, 0, r.out);
  r = h.run('--uninstall');
  assert.equal(r.code, 0, r.out);
  assert.deepEqual(h.read(), layoutBefore);
  assert.deepEqual(h.mods(), modsBefore);
  // --uninstall alone takes the panel out too.
  assert.equal(h.run('--panel', 'right').code, 0);
  assert.equal(h.run('--uninstall').code, 0);
  assert.deepEqual(h.read(), layoutBefore);
  assert.deepEqual(h.mods(), modsBefore);
  fs.rmSync(h.home, { recursive: true });
});

test('beside the Jenkins mod and Corgi: each one\'s lines in a shared tab bar stay, in either order, and each takes out only its own', () => {
  // Jenkins first (its clone tabs-jenkins), then Media into that copy, then Corgi's lines.
  const h = appHome();
  const before = h.read();
  let r = h.jenkins('--panel', 'right');
  assert.equal(r.code, 0, r.out);
  const jenkinsOnly = h.main('tabs-jenkins');
  r = h.run('--panel', 'right');
  assert.equal(r.code, 0, r.out);
  assert.match(r.out, /put a Media button into mods\/tabs-jenkins\/main\.js/);
  assert.ok(!h.mods().includes('tabs-media'), 'no copy of its own: the layout\'s copy is edited');
  fs.writeFileSync(path.join(h.app, 'mods', 'tabs-jenkins', 'main.js'), corgiLines(h.main('tabs-jenkins')));
  let bar = tabBar(h.main('tabs-jenkins'), { stack: ['files', 'changes', 'jenkins-panel', 'media-panel', 'corgi-questions'], unseen: 2 });
  assert.deepEqual(bar.render().map((b) => b.id), ['panel-files', 'panel-changes', 'panel-jenkins-panel', 'panel-media-panel', 'panel-corgi-questions']);
  assert.deepEqual(bar.enabled, ['files', 'changes', 'media-panel', 'corgi-questions', 'jenkins-panel']);
  // Media again with Corgi's lines there: unchanged.
  const all = h.main('tabs-jenkins');
  assert.equal(h.run('--panel', 'right').code, 0);
  assert.equal(h.main('tabs-jenkins'), all);
  // Media off: Jenkins' and Corgi's lines as they were.
  r = h.run('--panel', 'off');
  assert.equal(r.code, 0, r.out);
  assert.equal(h.main('tabs-jenkins'), corgiLines(jenkinsOnly));
  assert.equal(find(h.read(), 'tabs').content.mod, 'tabs-jenkins');
  assert.ok(find(h.read(), 'jenkins-panel'));
  assert.equal(h.read().floating, undefined);
  fs.rmSync(h.home, { recursive: true });

  // Media first (tabs-media), then Jenkins into that copy; Jenkins off, then Media off.
  const g = appHome();
  assert.equal(g.run('--panel', 'right').code, 0);
  r = g.jenkins('--panel', 'right');
  assert.equal(r.code, 0, r.out);
  bar = tabBar(g.main('tabs-media'), { stack: ['files', 'changes', 'media-panel', 'jenkins-panel'] });
  assert.deepEqual(bar.render().map((b) => b.id), ['panel-files', 'panel-changes', 'panel-media-panel', 'panel-jenkins-panel']);
  assert.deepEqual(bar.enabled, ['files', 'changes', 'media-panel', 'jenkins-panel']);
  r = g.jenkins('--panel', 'off');
  assert.equal(r.code, 0, r.out);
  r = g.run('--panel', 'off');
  assert.equal(r.code, 0, r.out);
  assert.deepEqual(g.read(), before, 'the layout as it was, back on Herdr\'s tab bar');
  assert.ok(!g.mods().includes('tabs-media'));
  fs.rmSync(g.home, { recursive: true });
});

test('install.sh --panel right on a tab bar that is a copy already (herdr-gitlab\'s) edits it between markers; a refused check writes nothing', () => {
  const h = appHome('tabs-gitlab');
  fs.mkdirSync(path.join(h.app, 'mods', 'tabs-gitlab'));
  const gitlab = STOCK_TABS.replaceAll('mods/tabs/', 'mods/tabs-gitlab/');
  fs.writeFileSync(path.join(h.app, 'mods', 'tabs-gitlab', 'main.js'), gitlab);
  const before = h.read();
  fs.writeFileSync(`${h.checker}.refuse`, '');
  let r = h.run('--panel', 'right');
  assert.notEqual(r.code, 0);
  fs.rmSync(`${h.checker}.refuse`);
  assert.equal(h.main('tabs-gitlab'), gitlab, 'nothing was written');
  assert.deepEqual(h.read(), before);
  r = h.run('--panel', 'right');
  assert.equal(r.code, 0, r.out);
  assert.equal(find(h.read(), 'tabs').content.mod, 'tabs-gitlab');
  assert.deepEqual(tabBar(h.main('tabs-gitlab')).render().at(-1).id, 'panel-media-panel');
  r = h.run('--uninstall');
  assert.equal(r.code, 0, r.out);
  assert.equal(h.main('tabs-gitlab'), gitlab);
  assert.deepEqual(h.read(), before);
  // Herdr's own layout is refused.
  fs.writeFileSync(path.join(h.app, 'layouts', 'main.json'), '{"id":"root"}');
  r = h.run('--panel', 'right', '--layout', 'main');
  assert.notEqual(r.code, 0);
  assert.match(r.out, /main is Herdr's own layout/);
  fs.rmSync(h.home, { recursive: true });
});

let failed = 0;
for (const { name, run } of cases) {
  skew = 0;
  try {
    await run();
    console.log(`ok    ${name}`);
  } catch (error) {
    failed += 1;
    console.log(`FAIL  ${name}\n${error.stack}`);
  }
}
console.log(failed ? `${failed} of ${cases.length} failed` : `all ${cases.length} passed`);
process.exit(failed ? 1 : 0);
