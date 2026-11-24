// Runs the Jira mod under Node against the made-up Jira of fake-jira.mjs,
// past the first frame `herdr-app check` renders: with stand-ins for the
// app's `gpui-kit`, `gpui-base` and `herdr` modules, a scratch folder as the
// app's home, and Node's own `fetch` in place of the app's. A stand-in
// element keeps its texts, children and handlers, so a test reads what a
// view shows and clicks and types in it. Its installer runs on scratch app
// homes, beside the Jenkins and Media mods' installers. Nothing here asks a
// real Jira or a real app.
//
//   node herdr-app/mods/jira/tests/test.mjs [part of a case's name]

import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import fs from 'node:fs';
import { registerHooks } from 'node:module';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { startFakeJira, SPRINT_ID } from './fake-jira.mjs';

const MOD = path.dirname(path.dirname(fileURLToPath(import.meta.url)));
const MODS = path.dirname(MOD);
const TOKEN = 'fake-token', VIEWER = 'viewer-token';

// Time the tests move: the mod and the made-up Jira both read Date.now.
let skew = 0;
const realNow = Date.now;
Date.now = () => realNow() + skew;

// --- the stand-in modules ------------------------------------------------------

// An element: every builder call returns it; children, texts and handlers
// are kept. `.when` runs its callback only when its condition holds, and the
// callback must give back the element it got, as in the app.
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

// The retained text of an input or a textarea: `type` is the person typing, `submit` Enter.
let rendering = false;
class InputState {
  static new(options = {}) {
    assert.ok(!rendering, 'an InputState is made while a view draws: the app refuses that ("cannot run during render")');
    return new this(options);
  }
  constructor(options) {
    this.text = options.value ?? '';
    this.placeholder = options.placeholder ?? '';
    this.rows = options.rows ?? 1;
    this.handlers = {};
    this.focused = 0;
    this.masked = false;
  }
  value() { return this.text; }
  set_value(next) { this.text = next; }
  set_masked(masked) { this.masked = masked; }
  set_rows(rows) { this.rows = rows; }
  on(event, handler) { this.handlers[event] = handler; return true; }
  focus() { this.focused += 1; }
}
class TextareaState extends InputState {}

const kit = {
  View: class View {},
  div: () => element('div'),
  svg: (file) => element('svg', [file]),
  v_flex: () => element('v_flex'),
  h_flex: () => element('h_flex'),
  InputState,
  TextareaState,
  Input: { new: (state) => element('input', [state]) },
  Textarea: { new: (state) => element('textarea', [state]) },
  ScrollHandle: { new: () => ({ scroll_to_item() {}, release: () => true }) },
};

// The app around the mod: its home (a scratch folder), what the environment
// holds, the commands waiting for the mod.
function world(overrides = {}) {
  const w = {
    home: fs.mkdtempSync(path.join(os.tmpdir(), 'jira-mod-')),
    env: '', // what `sh -c printf …` answers: the JIRA_* variables, a line each
    pending: 0, // tasks of the mod still at work
    waiter: null,
    opened: [], // URLs handed to the browser
    shared: {}, // what the app keeps for every mod across restarts (shared, setShared)
    props: null, // the props of the block showing the mod: props() gives them, init gets none
    toasts: [], // what the mod posted with notify()
    layout: null, // the layout the window shows: layout() gives it
    ran: [], // the app's commands the mod ran: [name, args]
    ...overrides,
  };
  w.app = path.join(w.home, '.config', 'herdr-app');
  fs.mkdirSync(w.app, { recursive: true });
  const colors = new Proxy({}, { get: (_t, name) => (typeof name === 'string' ? `#${name}` : undefined) });
  w.api = {
    theme: () => colors,
    settings: () => ({ font_family: 'JetBrains Mono' }),
    home: () => w.home,
    appHome: () => w.app,
    // The next command sent; until one is, the mod's loop for them waits and counts as at rest.
    nextCommand: () => new Promise((resolve) => {
      w.pending -= 1;
      w.waiter = (command) => {
        w.pending += 1;
        resolve(command);
      };
    }),
    props: () => w.props,
    notify: (n) => {
      w.toasts.push(JSON.parse(JSON.stringify(n)));
      return `toast-${w.toasts.length}`;
    },
    layout: () => w.layout,
    run: (name, args) => {
      w.ran.push([name, args]);
      return null;
    },
    shared: (key) => w.shared[key],
    setShared: (key, value) => {
      w.shared[key] = JSON.parse(JSON.stringify(value));
      return null;
    },
    readTextFile: async (file) => (fs.existsSync(file) ? fs.readFileSync(file, 'utf8') : null),
    // As the app's: refused when the file is not what the caller last read.
    writeTextFile: async (file, text, expected) => {
      const now = fs.existsSync(file) ? fs.readFileSync(file, 'utf8') : null;
      if (now !== expected) throw new Error('stale snapshot');
      fs.writeFileSync(file, text);
      return null;
    },
    exec: async (command, args = []) => {
      if (command === 'chmod') {
        fs.chmodSync(args[1], Number.parseInt(args[0], 8));
        return { code: 0, stdout: '', stderr: '' };
      }
      if (command === 'sh') return { code: 0, stdout: w.env, stderr: '' };
      throw new Error(`${command}: not installed`);
    },
  };
  return w;
}

const sources = {
  'gpui-kit': 'const k = globalThis.__kit; export const View = k.View, div = k.div, svg = k.svg;',
  'gpui-base': 'const k = globalThis.__kit; export const v_flex = k.v_flex, h_flex = k.h_flex, InputState = k.InputState, Input = k.Input, TextareaState = k.TextareaState, Textarea = k.Textarea, ScrollHandle = k.ScrollHandle;',
  herdr: ['theme', 'settings', 'home', 'appHome', 'nextCommand', 'props', 'notify', 'layout', 'run', 'shared', 'setShared', 'readTextFile', 'writeTextFile', 'exec']
    .map((name) => `export const ${name} = (...a) => globalThis.__world.api.${name}(...a);`).join('\n'),
  'fs/promises': 'export const mkdir = (...a) => import("node:fs/promises").then((m) => m.mkdir(...a));',
};
globalThis.__kit = kit;
registerHooks({
  resolve(specifier, context, next) {
    if (specifier in sources) return { url: `stand-in:${specifier}`, shortCircuit: true };
    return next(specifier, context);
  },
  load(url, context, next) {
    if (url.startsWith('stand-in:')) return { format: 'module', source: sources[url.slice(9)], shortCircuit: true };
    // A mod has no package.json to say its files are modules.
    return next(url, url.includes('/mods/jira/') && url.split('?')[0].endsWith('.js') ? { ...context, format: 'module' } : context);
  },
});

// A mod's own modules (`./data.js`) come with the copy of main.js that
// imports them, so each `start` has modules of its own, as an app start
// does, and copies started with the same `module` share them (data.js's
// store), as the copies of one mod in the app do.
registerHooks({
  resolve(specifier, context, next) {
    const resolved = next(specifier, context);
    const parent = context.parentURL ? new URL(context.parentURL) : null;
    if (!parent?.pathname.includes('/mods/jira/') || !parent.search || !specifier.startsWith('.')) return resolved;
    const url = new URL(resolved.url);
    url.search = parent.search;
    return { ...resolved, url: url.href };
  },
});

const util = await import(new URL('../util.js', import.meta.url));
const wiki = await import(new URL('../wiki.js', import.meta.url));
const alerts = await import(new URL('../alerts.js', import.meta.url));
const data = await import(new URL('../data.js', import.meta.url));

let copies = 0;
// The mod, started in the world `w`: its view, and what its tasks and timers
// are. Copies given the same `module` (a name) share the mod's modules.
async function start(w, module = `copy-${copies++}`) {
  globalThis.__world = w;
  const module_ = await import(new URL(`../main.js?module=${module}`, import.meta.url));
  const cx = {
    notified: 0,
    timers: [],
    notify() { this.notified += 1; },
    spawn(task) {
      w.pending += 1;
      Promise.resolve().then(() => task(cx)).catch((error) => {
        w.failed = error;
      }).finally(() => {
        w.pending -= 1;
      });
    },
    timer: { every: (ms, tick) => cx.timers.push({ ms, tick }), after: (_ms, run) => cx.spawn(async (cx) => run(cx)) },
    focus_handle: () => ({ focused: 0, focus() { this.focused += 1; } }),
    stop_propagation() {},
    open_url: (url) => w.opened.push(url),
  };
  const view = new module_.default();
  view.init(null, cx); // as the app: a block's props come from props()
  // Until no task is at work: the ones started meanwhile too.
  const settle = async () => {
    do await new Promise((resolve) => setImmediate(resolve)); while (w.pending > 0);
    if (w.failed) throw w.failed;
  };
  await settle();
  const render = () => {
    globalThis.__world = w;
    rendering = true;
    try { return view.render(cx); } finally { rendering = false; }
  };
  const mod = {
    view,
    cx,
    settle,
    render,
    tick: async () => {
      globalThis.__world = w;
      for (const timer of cx.timers) timer.tick(cx);
      await settle();
    },
    // One round of the timer without waiting for what it starts (a read that stalls never ends).
    poke: async () => {
      globalThis.__world = w;
      for (const timer of cx.timers) timer.tick(cx);
      for (let i = 0; i < 20; i++) await new Promise((resolve) => setImmediate(resolve));
    },
    click: async (id) => {
      mod.press(id);
      await settle();
    },
    // A click without waiting for what it starts (a read that stalls never ends).
    press: (id) => {
      const el = byId(render(), id);
      assert.ok(el, `nothing with the id ${id}; there is ${ids(render(), 'ji-').join(', ')}`);
      assert.ok(el.handlers.on_click, `${id} takes no click`);
      el.handlers.on_click({}, cx);
    },
    // A `jira.<command>`, as from a key in keys.json or a toast.
    send: async (command) => {
      w.waiter(command);
      await settle();
    },
    key: async (key, modifiers = {}) => {
      kept(render()).handlers.on_key_down({ key, modifiers }, cx);
      await settle();
    },
    type: (input, text) => {
      input.set_value(text);
      input.handlers.change?.({}, cx);
    },
    submit: async (input) => {
      input.handlers.submit({}, cx);
      await settle();
    },
    texts: () => texts(render()),
    says: (text) => mod.texts().some((line) => line.includes(text)),
    input: (id) => kept(byId(render(), id).children.find((c) => ['input', 'textarea'].includes(kept(c).kind))).args[0],
  };
  return mod;
}

// --- reading a rendered view -----------------------------------------------------

function walk(el, visit) {
  const node = kept(el);
  if (node === null || typeof node !== 'object') return;
  visit(node);
  for (const child of node.children ?? []) walk(child, visit);
}

// Every text under `el`, in the order it is drawn; a spinner's frames are a mark, not text.
function texts(el) {
  const out = [];
  const collect = (el) => {
    const node = kept(el);
    if (typeof node === 'string') out.push(node);
    else if (node && typeof node === 'object' && 'frames' in node.calls) return;
    else if (node && typeof node === 'object') for (const child of node.children ?? []) collect(child);
  };
  collect(el);
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

const row = (mod, id) => {
  const el = byId(mod.render(), id);
  assert.ok(el, `no row ${id}; there is ${ids(mod.render(), 'ji-').join(', ')}`);
  return texts(el).join(' ');
};
const svgsOf = (mod, id) => {
  const out = [];
  walk(byId(mod.render(), id), (n) => { if (n.kind === 'svg') out.push(path.basename(n.args[0], '.svg')); });
  return out;
};

// --- the cases ------------------------------------------------------------------

const cases = [];
const test = (name, run) => cases.push({ name, run });

// A world whose settings are saved already, for the made-up Jira.
function configured(fake, settings = {}) {
  const w = world();
  fs.mkdirSync(path.join(w.app, 'jira'), { recursive: true });
  fs.writeFileSync(path.join(w.app, 'jira', 'settings.json'), JSON.stringify({ url: fake.url, token: TOKEN, board: '7', ...settings }));
  return w;
}
const acts = (fake) => fake.requests.filter((r) => r.method !== 'GET');
const reads = (fake, re) => fake.requests.filter((r) => r.method === 'GET' && re.test(`${r.path}${decodeURIComponent(r.query)}`));
const SPRINT = new RegExp(`jql=filter = 507 AND sprint = ${SPRINT_ID} ORDER BY Rank ASC&`);
const MINE = /jql=assignee = currentUser\(\) AND statusCategory != Done ORDER BY updated DESC&/;
const PROJECT = /jql=project in \("DEMO"\) AND statusCategory != Done ORDER BY Rank ASC&/;
// Ticks every two seconds for `ms`, as the app's timer does.
const run = async (mods, ms) => {
  for (let at = 0; at < ms; at += 2_000) {
    skew += 2_000;
    for (const mod of mods) await mod.tick();
  }
};
// A layout as Herdr's, with the panel in its right-hand stack: the column
// `panel` hidden or not, and the stack showing `active`.
const panelLayout = ({ hidden = true, active = 'files' } = {}) => ({
  type: 'row',
  children: [
    { id: 'work', type: 'block', content: { mod: 'terminals' } },
    { id: 'panel', type: 'column', hidden, children: [{ id: 'panel-body', type: 'stack', active, children: [
      { id: 'files', type: 'block', content: { mod: 'files' } },
      { id: 'jira-panel', type: 'block', content: { mod: 'jira', props: { panel: true } } },
    ] }] },
  ],
});
const SHOWN = { hidden: false, active: 'jira-panel' };
const SELECT = ['layout.select', { stack: 'panel-body', child: 'jira-panel' }];

test('the helpers: the board setting, a project\'s board, keys, groups, sections, time spent, days, transitions, errors', () => {
  assert.equal(util.normalizeBase(' jira.example.com/ '), 'https://jira.example.com');
  assert.deepEqual(util.parseBoard('42'), { id: 42 });
  assert.deepEqual(util.parseBoard('https://jira.example.com/secure/RapidBoard.jspa?rapidView=42&projectKey=DEMO'), { id: 42 });
  assert.deepEqual(util.parseBoard('https://jira.example.com/jira/software/projects/DEMO/boards/9'), { id: 9 });
  assert.deepEqual(util.parseBoard('demo'), { project: 'DEMO' });
  assert.equal(util.parseBoard('two words'), null);
  assert.equal(util.parseBoard(''), null);
  assert.equal(util.keyOf('open-DEMO-12'), 'DEMO-12');
  assert.equal(util.keyOf('open-demo-12'), 'DEMO-12', 'a command name may come lowercased');
  assert.equal(util.keyOf('open-AB_2-7'), 'AB_2-7');
  for (const no of ['open-12', 'open-DEMO', 'show', 'open-DEMO-12-x', 'opendemo-1']) assert.equal(util.keyOf(no), null, no);
  const me = { name: 'jane', key: 'JIRAUSER10001' };
  assert.ok(util.isMe({ name: 'jane' }, me) && util.isMe({ key: 'JIRAUSER10001', name: 'x' }, me) && !util.isMe({ name: 'bob' }, me) && !util.isMe(null, me));

  const issue = (key, status, category, assignee) => ({ key, fields: { status: { id: status, name: status, statusCategory: { key: category } }, assignee: assignee ? { name: assignee } : null } });
  const g = util.groups([
    issue('D-1', 'Done', 'done', 'jane'), issue('D-2', 'To Do', 'new', 'bob'), issue('D-3', 'Doing', 'indeterminate', 'jane'),
    issue('D-4', 'Done', 'done', null), issue('D-5', 'Review', 'indeterminate', 'bob'), issue('D-6', 'To Do', 'new', 'jane'),
    issue('D-7', 'Doing', 'indeterminate', 'bob'),
  ], me);
  assert.deepEqual(g.mine.map((i) => i.key), ['D-3', 'D-6', 'D-1'], 'at work, to do, done; else in rank order');
  assert.deepEqual(g.rest.map((s) => [s.name, s.issues.map((i) => i.key)]), [['Review', ['D-5']], ['Doing', ['D-7']], ['To Do', ['D-2']], ['Done', ['D-4']]]);

  // A project's board: Scrum before Kanban; the one living in the project before one that does not say, before another project's.
  const b = (id, type, at) => ({ id, type, ...(at ? { location: { projectKey: at } } : {}) });
  assert.equal(util.pickBoard([b(1, 'kanban', 'DEMO'), b(2, 'scrum', 'OTHER'), b(3, 'scrum'), b(4, 'scrum', 'DEMO'), b(5, 'scrum', 'DEMO')], 'DEMO').id, 4);
  assert.equal(util.pickBoard([b(1, 'kanban', 'DEMO'), b(2, 'scrum', 'OTHER'), b(3, 'scrum'), b(6, 'scrum')], 'DEMO').id, 3);
  assert.equal(util.pickBoard([b(1, 'kanban'), b(2, 'scrum', 'OTHER')], 'DEMO').id, 2, 'the one with sprints');
  assert.equal(util.pickBoard([b(1, 'kanban', 'OTHER'), b(2, 'kanban')], 'DEMO').id, 2);
  assert.equal(util.pickBoard([], 'DEMO'), null);
  // The sections: each issue once, in the first that has it.
  const k = (...keys) => keys.map((key) => ({ key }));
  const parts = util.sections([k('D-1', 'D-2'), k('D-3')], k('D-9', 'D-2', 'X-1', 'D-3'), k('D-1', 'D-4', 'D-9', 'D-5'));
  assert.deepEqual([parts.mine.map((i) => i.key), parts.above, parts.project.map((i) => i.key)], [['D-9', 'X-1'], 2, ['D-4', 'D-5']]);
  assert.deepEqual(util.sections([], undefined, undefined), { mine: [], above: 0, project: [] });

  for (const [typed, jira] of [['1h 30m', '1h 30m'], ['1h30m', '1h 30m'], ['90m', '90m'], ['1.5h', '1.5h'], ['1,5h', '1.5h'], [' 2d ', '2d'], ['1w 2d 3h 4m', '1w 2d 3h 4m'], ['1H', '1h']]) {
    assert.equal(util.parseTimeSpent(typed), jira, typed);
  }
  for (const no of ['', '1', 'h', '1x', '1 hour', '0m', 'abc 1h']) assert.equal(util.parseTimeSpent(no), null, no);
  const now = new Date(2026, 10, 17, 14, 5, 6).getTime();
  assert.equal(util.dayOf(now), '2026-11-17');
  assert.deepEqual(util.parseDay('2026-11-16', now), { y: 2026, m: 11, d: 16 });
  for (const no of ['2026-11-18', '2026-02-30', '17/11/2026', '']) assert.equal(util.parseDay(no, now), null, no);
  assert.match(util.startedOn({ y: 2026, m: 11, d: 17 }, now), /^2026-11-17T14:05:06\.000[+-]\d{4}$/, 'today: now');
  assert.match(util.startedOn({ y: 2026, m: 11, d: 3 }, now), /^2026-11-03T09:00:00\.000[+-]\d{4}$/, 'another day: nine in the morning');
  const at = (days) => ({ endDate: new Date(now + days * 86_400_000).toISOString() });
  assert.equal(util.sprintEnds(at(3.2), now), 'ends in 3 days');
  assert.equal(util.sprintEnds(at(-2), now), 'ended 2 days ago');
  assert.equal(util.sprintEnds({}, now), '');
  assert.equal(util.timeOf('2026-11-17T09:00:00.000+0100'), Date.parse('2026-11-17T08:00:00Z'));
  assert.deepEqual(util.requiredFields({ fields: { resolution: { required: true, name: 'Resolution' }, comment: { required: false }, fixVersions: { required: true, hasDefaultValue: true } } }), ['Resolution']);
  assert.deepEqual(util.requiredFields({}), []);
});

test('wiki markup reads as text: headings, lists, code, quotes, panels, tables, links and emphasis', () => {
  assert.equal(wiki.inline('A *bold* and _italic_ word, {{mono}}, -gone- and +under+'), 'A bold and italic word, mono, gone and under');
  assert.equal(wiki.inline('See [the board|https://jira.example.com/b?x=1_2_3] or [https://example.com/a_b_c] or [~bob]'), 'See the board or https://example.com/a_b_c or @bob');
  assert.equal(wiki.inline('{color:red}Red{color} !shot.png|thumbnail! (/) (x) (!)'), 'Red [image: shot.png] ✓ ✗ ⚠');
  assert.equal(wiki.inline('2*3*4 and snake_case_name and a-b-c stay'), '2*3*4 and snake_case_name and a-b-c stay');
  assert.equal(wiki.inline('\\*not bold\\*'), '*not bold*');
  const b = wiki.blocks('h2. Title\nA line\nanother\n\n* one\n** two\n# first\n# second\n{code:js}\nconst a = *b*;\n\n  x();\n{code}\n{quote}Quoted *word*{quote}\nbq. Short quote\n{panel:title=Note|borderStyle=dashed}\nIn a panel\n{panel}\n||A||B||\n|[link|https://example.com]|2|\n----\nLast\\\\broken');
  assert.deepEqual(b, [
    { kind: 'h', level: 2, text: 'Title' },
    { kind: 'p', text: 'A line' },
    { kind: 'p', text: 'another' },
    { kind: 'gap' },
    { kind: 'li', level: 1, ordered: false, n: 0, text: 'one' },
    { kind: 'li', level: 2, ordered: false, n: 0, text: 'two' },
    { kind: 'li', level: 1, ordered: true, n: 1, text: 'first' },
    { kind: 'li', level: 1, ordered: true, n: 2, text: 'second' },
    { kind: 'code', lines: ['const a = *b*;', '', '  x();'] },
    { kind: 'quote', title: '', lines: ['Quoted word'] },
    { kind: 'quote', title: '', lines: ['Short quote'] },
    { kind: 'quote', title: 'Note', lines: ['In a panel'] },
    { kind: 'row', head: true, cells: ['A', 'B'] },
    { kind: 'row', head: false, cells: ['link', '2'] },
    { kind: 'rule' },
    { kind: 'p', text: 'Last' },
    { kind: 'p', text: 'broken' },
  ]);
  assert.deepEqual(wiki.blocks('{noformat}raw *stays*{noformat}'), [{ kind: 'code', lines: ['raw *stays*'] }]);
  assert.deepEqual(wiki.blocks('{code}\nnever closed'), [{ kind: 'code', lines: ['never closed'] }]);
  assert.deepEqual(wiki.blocks(''), []);
  assert.deepEqual(wiki.blocks(null), []);
  assert.equal(wiki.gist('h3. Hi\n* a *b*\n{code}x{code}'), 'Hi a b x');
  assert.equal(wiki.gist('word '.repeat(50), 20).length, 20);
});

test('with no settings it asks for them, checks the token and the board, and keeps them where only the owner reads', async () => {
  const fake = await startFakeJira();
  const w = world();
  const mod = await start(w);
  assert.ok(mod.says('Give this panel a Jira personal access token'), mod.texts().join('|'));
  assert.equal(mod.input('ji-set-token').masked, true, 'the token is masked');
  await mod.click('ji-set-check');
  assert.ok(mod.says('are all needed'));
  mod.type(mod.input('ji-set-url'), fake.url);
  mod.type(mod.input('ji-set-token'), 'wrong');
  mod.type(mod.input('ji-set-board'), 'DEMO');
  await mod.click('ji-set-check');
  assert.ok(mod.says('Jira did not accept the token'), mod.texts().join('|'));
  mod.type(mod.input('ji-set-token'), TOKEN);
  mod.type(mod.input('ji-set-board'), 'not a board');
  await mod.click('ji-set-check');
  assert.ok(mod.says('is neither a board\'s id'));
  mod.type(mod.input('ji-set-board'), 'NOPE');
  await mod.click('ji-set-check');
  assert.ok(mod.says('Authenticated as Jane Doe (jane), but: Could not find the board NOPE: Jira has nothing there (no board of NOPE).'), mod.texts().join('|'));
  mod.type(mod.input('ji-set-board'), 'demo');
  await mod.click('ji-set-check');
  assert.ok(mod.says('Authenticated as Jane Doe (jane) · board 7, DEMO board'), mod.texts().join('|'));
  assert.ok(!fs.existsSync(path.join(w.app, 'jira', 'settings.json')), 'Check saves nothing');
  await mod.submit(mod.input('ji-set-board')); // Enter saves
  const file = path.join(w.app, 'jira', 'settings.json');
  assert.deepEqual(JSON.parse(fs.readFileSync(file, 'utf8')), { url: fake.url, token: TOKEN, board: 'demo' });
  assert.equal(fs.statSync(file).mode & 0o777, 0o600);
  assert.equal(mod.view.view().kind, 'list');
  await mod.tick();
  assert.ok(mod.says('DEMO Sprint 12'), mod.texts().join('|'));
  // The token goes in a header, never into a URL or anything drawn.
  assert.ok(fake.requests.every((r) => !`${r.path}${r.query}`.includes(TOKEN)));
  assert.ok(fake.requests.every((r) => r.auth === 'Bearer …' || r.auth === ''), 'Bearer auth');
  assert.ok(!mod.texts().some((t) => t.includes(TOKEN)));
  await mod.click('ji-settings');
  assert.ok(!mod.texts().some((t) => t.includes(TOKEN)), 'not even in the settings: the field is masked, not text');
  await fake.close();
});

test('with nothing saved it takes JIRA_URL, JIRA_API_TOKEN and JIRA_BOARD, when the app has them', async () => {
  const fake = await startFakeJira();
  const w = world({ env: `${fake.url}\n${TOKEN}\n7` });
  const mod = await start(w);
  await mod.tick();
  assert.ok(mod.says('DEMO Sprint 12'), mod.texts().join('|'));
  await mod.click('ji-settings');
  assert.ok(mod.says('These came from JIRA_URL and JIRA_API_TOKEN'));
  await fake.close();
});

test('the list: three sections, each issue once: the sprint as the board shows it, the token user\'s other issues, the project\'s others', async () => {
  const fake = await startFakeJira();
  const w = configured(fake);
  const mod = await start(w);
  await mod.tick();
  // The sprint is read by the board's filter: the other team's issues in the same sprint stay out.
  assert.equal(reads(fake, /\/board\/7\/configuration$/).length, 1);
  assert.equal(reads(fake, SPRINT).length, 2, 'two pages of the fake\'s 50');
  assert.equal(reads(fake, /jql=sprint = /).length, 0, 'never the sprint without the board\'s filter');
  assert.equal([...fake.jira.issues.values()].filter((i) => i.sprint === SPRINT_ID).length, 66, 'the sprint itself has two of another team\'s');
  assert.equal(reads(fake, MINE).length, 1);
  assert.equal(reads(fake, PROJECT).length, 2, 'two pages as well');
  assert.ok(mod.says('DEMO board · jane'));
  assert.deepEqual(ids(mod.render(), 'ji-sec-'), [`ji-sec-sprint-${SPRINT_ID}`, 'ji-sec-mine', 'ji-sec-project']);
  assert.equal(row(mod, `ji-sec-sprint-${SPRINT_ID}`), 'Current sprint · DEMO board DEMO Sprint 12 · ends in 4 days 64');
  assert.equal(row(mod, 'ji-sec-mine'), 'Assigned to me 7 more in the sprint above 3');
  assert.equal(row(mod, 'ji-sec-project'), 'All DEMO issues open, and not shown above 2');
  const order = ids(mod.render(), 'ji-head-');
  assert.deepEqual(order, [`ji-head-mine-${SPRINT_ID}`, `ji-head-status-${SPRINT_ID}-3`, `ji-head-status-${SPRINT_ID}-10001`, `ji-head-status-${SPRINT_ID}-1`, `ji-head-status-${SPRINT_ID}-10002`, 'ji-head-status-project-1'], 'at work (in the order their first issue comes), to do, done; then the project\'s');
  assert.equal(row(mod, `ji-head-mine-${SPRINT_ID}`), 'Assigned to you 10');
  const rows = ids(mod.render(), 'ji-row-');
  assert.deepEqual(rows.slice(0, 10), ['DEMO-101', 'DEMO-145', 'DEMO-102', 'DEMO-103', 'DEMO-112', 'DEMO-123', 'DEMO-156', 'DEMO-107', 'DEMO-134', 'DEMO-150'].map((k) => `ji-row-${k}`));
  assert.equal(row(mod, 'ji-row-DEMO-101'), 'DEMO-101 Show the active sprint in the side panel In Progress');
  assert.deepEqual(svgsOf(mod, 'ji-row-DEMO-150'), ['subtask'], 'a sub-task\'s mark');
  assert.match(row(mod, 'ji-row-DEMO-108'), /^DEMO-108 .* Bob Builder$/, 'the rest say whose');
  assert.match(row(mod, 'ji-row-DEMO-106'), /Unassigned$/);
  assert.ok(!rows.includes('ji-row-DEMO-104'), 'Done is folded at first');
  assert.ok(!rows.includes('ji-row-OTHER-2'), 'another team\'s issue in the same sprint: not the board\'s, not the user\'s, not the project\'s');
  // Under the sprint: the user's open issues it does not show (any project, the last changed first), then the project's.
  assert.deepEqual(rows.slice(rows.indexOf('ji-row-DEMO-151') + 1), ['OTHER-1', 'DEMO-201', 'OTHER-3', 'DEMO-200', 'DEMO-202'].map((k) => `ji-row-${k}`));
  assert.equal(new Set(rows).size, rows.length, 'no issue twice');
  assert.equal(row(mod, 'ji-row-OTHER-3'), 'OTHER-3 Review the other team\'s plan In Review');
  assert.match(row(mod, 'ji-row-DEMO-202'), /Bob Builder$/);
  assert.ok(!mod.says('DEMO-203'), 'done, in no sprint: nowhere');
  // An issue opens from any section, and its bell mutes it.
  await mod.click('ji-row-OTHER-3');
  assert.ok(mod.says('Review the other team\'s plan') && mod.says('Assignee: Jane Doe'), mod.texts().join('|'));
  await mod.key('escape');
  byId(mod.render(), 'ji-row-DEMO-202').handlers.on_hover(true, mod.cx); // its bell shows under the pointer
  await mod.click('ji-mute-DEMO-202');
  assert.deepEqual(w.shared['jira.muted'], ['DEMO-202']);
  // Folding: kept by the app; a click unfolds Done, and folds a section away.
  await mod.click(`ji-head-status-${SPRINT_ID}-10002`);
  assert.ok(ids(mod.render(), 'ji-row-').includes('ji-row-DEMO-104'));
  await mod.click(`ji-head-mine-${SPRINT_ID}`);
  assert.ok(!ids(mod.render(), 'ji-row-').includes('ji-row-DEMO-101'));
  await mod.click('ji-sec-project');
  assert.ok(!ids(mod.render(), 'ji-row-').includes('ji-row-DEMO-200') && ids(mod.render(), 'ji-row-').includes('ji-row-DEMO-201'));
  await mod.click(`ji-sec-sprint-${SPRINT_ID}`);
  assert.deepEqual(ids(mod.render(), 'ji-row-'), ['OTHER-1', 'DEMO-201', 'OTHER-3'].map((k) => `ji-row-${k}`));
  assert.deepEqual(ids(mod.render(), 'ji-sec-'), [`ji-sec-sprint-${SPRINT_ID}`, 'ji-sec-mine', 'ji-sec-project'], 'the headings stay');
  assert.deepEqual(w.shared['jira.folded'], { [`status-${SPRINT_ID}-10002`]: false, [`mine-${SPRINT_ID}`]: true, 'sec-project': true, [`sec-sprint-${SPRINT_ID}`]: true });
  await mod.click(`ji-sec-sprint-${SPRINT_ID}`);
  assert.ok(!byId(mod.render(), `ji-more-status-${SPRINT_ID}-1`), 'To Do has fewer than 40 here');
  // The sprint every minute while the list shows, the user's issues every two, the project's every five.
  const n = [SPRINT, MINE, PROJECT].map((re) => reads(fake, re).length);
  await run([mod], 60_000);
  assert.deepEqual([SPRINT, MINE, PROJECT].map((re) => reads(fake, re).length), [n[0] + 2, n[1], n[2]]);
  await run([mod], 4 * 60_000);
  assert.deepEqual([SPRINT, MINE, PROJECT].map((re) => reads(fake, re).length), [n[0] + 10, n[1] + 2, n[2] + 2]);
  // What moves moves between the sections: an issue of the backlog given to the user is among theirs.
  fake.jira.assign('DEMO-200', 'jane', 'bob');
  await mod.click('ji-refresh');
  assert.equal(row(mod, 'ji-sec-mine'), 'Assigned to me 7 more in the sprint above 4');
  assert.equal(ids(mod.render(), 'ji-row-').filter((id) => id === 'ji-row-DEMO-200').length, 1);
  await mod.click('ji-board-open');
  assert.deepEqual(w.opened, [`${fake.url}/secure/RapidBoard.jspa?rapidView=7`]);
  await fake.close();
});

test('the board: by its id or its project\'s key (the project\'s own Scrum board, not another team\'s); Kanban has no sprint section; one that is not there', async () => {
  const fake = await startFakeJira();
  // By the project's key: Jira lists another team's board first, whose filter finds DEMO's issues too.
  let mod = await start(configured(fake, { board: 'demo' }));
  await mod.tick();
  assert.ok(mod.says('Current sprint · DEMO board') && !mod.says('OTHER team board'), mod.texts().join('|'));
  assert.equal(reads(fake, /\/board\?projectKeyOrId=DEMO&/).length, 1);
  assert.equal(reads(fake, SPRINT).length, 2);
  assert.equal(reads(fake, PROJECT).length, 2, 'the project is the one named');
  assert.equal(reads(fake, /\/board\/\d+\/project/).length, 0);
  const byKey = ids(mod.render(), 'ji-row-');
  // By its id: the same list, the project being where the board lives.
  fake.requests.length = 0;
  mod = await start(configured(fake, { board: '7' }));
  await mod.tick();
  assert.deepEqual(ids(mod.render(), 'ji-row-'), byKey);
  assert.equal(reads(fake, PROJECT).length, 2);
  assert.equal(reads(fake, /\/board\/\d+\/project/).length, 0, 'the board says where it lives');
  // Another team's board, by its id: its filter, its project; the same sprint, other issues.
  mod = await start(configured(fake, { board: '6' }));
  await mod.tick();
  assert.equal(reads(fake, new RegExp(`jql=filter = 506 AND sprint = ${SPRINT_ID} ORDER BY Rank ASC&`)).length, 2);
  assert.equal(row(mod, `ji-sec-sprint-${SPRINT_ID}`), 'Current sprint · OTHER team board DEMO Sprint 12 · ends in 4 days 66');
  assert.equal(row(mod, 'ji-sec-project'), 'All OTHER issues open, and not shown above 0');
  assert.ok(mod.says('No other open issues.'));
  assert.deepEqual(ids(mod.render(), 'ji-row-').slice(-2), ['ji-row-DEMO-201', 'ji-row-OTHER-3'], 'the user\'s, in no sprint');
  // A Kanban board, which does not say where it lives (an older Jira): no sprint section, the project from its filter's.
  fake.requests.length = 0;
  mod = await start(configured(fake, { board: '8' }));
  await mod.tick();
  assert.deepEqual(ids(mod.render(), 'ji-sec-'), ['ji-sec-mine', 'ji-sec-project'], mod.texts().join('|'));
  assert.ok(mod.says('DEMO kanban · jane') && !mod.says('Current sprint') && !mod.says('Kanban'), mod.texts().join('|'));
  assert.equal(reads(fake, /\/board\/8\/project/).length, 1);
  assert.equal(reads(fake, /jql=.*sprint = /).length, 0);
  assert.equal(row(mod, 'ji-sec-mine'), 'Assigned to me 10');
  const kanban = ids(mod.render(), 'ji-row-');
  assert.deepEqual(kanban.slice(0, 3), ['ji-row-OTHER-1', 'ji-row-DEMO-201', 'ji-row-DEMO-101'], 'the last changed first');
  assert.equal(new Set(kanban).size, kanban.length, 'no issue twice');
  assert.equal(row(mod, 'ji-sec-project'), `All DEMO issues open, and not shown above ${kanban.length - 10}`);
  assert.ok(kanban.includes('ji-row-DEMO-105') && !kanban.includes('ji-row-OTHER-2') && !kanban.includes('ji-row-DEMO-104'), 'open, of the project');
  mod = await start(configured(fake, { board: '99' }));
  await mod.tick();
  assert.ok(mod.says('Could not find the board 99: Jira has nothing there'), mod.texts().join('|'));
  assert.deepEqual(ids(mod.render(), 'ji-sec-'), ['ji-sec-mine'], 'the user\'s issues need no board');
  mod = await start(configured(fake, { board: '' }));
  await mod.tick();
  assert.ok(mod.says('Name a board (its id) or a project (its key) in the settings.'));
  await fake.close();
});

test('a board whose filter Jira will not name: the sprint through the agile API, still the board\'s issues alone', async () => {
  const fake = await startFakeJira();
  fake.fail({ mode: 'status', status: 403, path: '/configuration$' });
  const mod = await start(configured(fake));
  await mod.tick();
  assert.equal(reads(fake, new RegExp(`/board/7/sprint/${SPRINT_ID}/issue\\?fields=`)).length, 2, 'two pages');
  assert.equal(reads(fake, /sprint = /).length, 0);
  assert.equal(row(mod, `ji-sec-sprint-${SPRINT_ID}`), 'Current sprint · DEMO board DEMO Sprint 12 · ends in 4 days 64');
  assert.ok(!ids(mod.render(), 'ji-row-').includes('ji-row-OTHER-2'));
  await fake.close();
});

test('a project with more open issues than are read: the list says it is capped', async () => {
  const fake = await startFakeJira(0, { backlog: 1010 });
  const mod = await start(configured(fake));
  await mod.tick();
  assert.equal(reads(fake, PROJECT).length, 20, '1000 of them, 50 at a time');
  assert.ok(mod.says('Capped: the first 1000 of 1066 open issues were read. The rest are in the browser.'), mod.texts().filter((t) => /Capped|open issues/.test(t)).join('|'));
  assert.ok(byId(mod.render(), 'ji-more-status-project-1'), 'a long heading shows 40 and Show all');
  const rows = ids(mod.render(), 'ji-row-');
  assert.equal(new Set(rows).size, rows.length);
  await fake.close();
});

test('an issue: its facts, its description made readable, subtasks, links, comments, and the way to the browser', async () => {
  const fake = await startFakeJira();
  const w = configured(fake);
  const mod = await start(w);
  await mod.tick();
  await mod.click('ji-row-DEMO-101');
  const view = mod.texts().join('|');
  assert.ok(mod.says('Show the active sprint in the side panel'));
  assert.ok(mod.says('Story · Highest priority · updated 2d ago'), view);
  assert.ok(mod.says('Assignee: Jane Doe · Reporter: Alice Example'));
  assert.ok(mod.says('Logged 2h · 4h left'));
  const desc = texts(byId(mod.render(), 'ji-description'));
  assert.deepEqual(desc.slice(0, 4), ['What we want', 'The panel shows the active sprint of a board, with my issues first.', '•', 'Read the sprint every minute']);
  assert.ok(desc.includes('◦') && desc.includes('1.') && desc.includes('2.'));
  assert.ok(desc.includes('Second step, see the board'));
  assert.ok(desc.includes('const sprint = await jira.sprint(7);'));
  assert.ok(desc.includes('Ask @bob before changing the API.') && desc.includes('Note') && desc.includes('Nothing here is real ✓'));
  assert.ok(desc.includes('Field') && desc.includes('Labels'));
  assert.ok(!desc.some((t) => /\{code|\{quote|\{panel|\|\||h2\.|\[~|\*active/.test(t)), `no raw markup: ${desc.join('|')}`);
  assert.equal(row(mod, 'ji-sub-DEMO-150'), 'DEMO-150 Read the board\'s sprints Done');
  assert.match(row(mod, 'ji-link-20001'), /^blocks DEMO-120 /);
  assert.match(row(mod, 'ji-link-20002'), /^is blocked by DEMO-130 /);
  assert.ok(mod.says('Comments · 2'));
  assert.ok(mod.says('Looks good. Can the done group start folded?'));
  assert.ok(mod.says('folded by default'));
  assert.ok(!byId(mod.render(), 'ji-act-assign'), 'it is Jane\'s already: no Assign to me');
  await mod.click('ji-issue-open');
  assert.deepEqual(w.opened, [`${fake.url}/browse/DEMO-101`]);
  // A subtask opens; it shows its parent, which opens; Esc goes back.
  await mod.click('ji-sub-DEMO-151');
  assert.ok(mod.says('Draw the status headings') && mod.says('Part of'));
  assert.ok(byId(mod.render(), 'ji-act-assign'), 'nobody\'s: Assign to me');
  await mod.key('escape');
  assert.equal(mod.view.view().key, 'DEMO-101');
  await mod.key('escape');
  assert.equal(mod.view.view().kind, 'list');
  // Read every half minute while it shows.
  await mod.click('ji-row-DEMO-101');
  const n = reads(fake, /\/issue\/DEMO-101\?/).length;
  await run([mod], 60_000);
  assert.equal(reads(fake, /\/issue\/DEMO-101\?/).length, n + 2);
  await fake.close();
});

test('changing the status: the transitions Jira offers, asked first; one that needs a screen is refused with the way to the browser', async () => {
  const fake = await startFakeJira();
  const mod = await start(configured(fake));
  await mod.tick();
  await mod.click('ji-row-DEMO-145'); // Jane's, In Progress
  await mod.click('ji-act-status');
  assert.deepEqual(ids(mod.render(), 'ji-tr-'), ['ji-tr-21', 'ji-tr-31']);
  assert.equal(row(mod, 'ji-tr-21'), 'Send to Review IN REVIEW');
  await mod.click('ji-tr-21');
  assert.ok(mod.says('Send to Review: move DEMO-145 from In Progress to In Review?'), mod.texts().join('|'));
  assert.deepEqual(acts(fake), [], 'nothing sent before Move');
  await mod.click('ji-confirm-no');
  assert.ok(!byId(mod.render(), 'ji-confirm'));
  await mod.click('ji-tr-21');
  await mod.click('ji-confirm-yes');
  assert.deepEqual(acts(fake).map((r) => [r.method, r.path, JSON.parse(r.body)]), [['POST', '/rest/api/2/issue/DEMO-145/transitions', { transition: { id: '21' } }]]);
  assert.equal(fake.jira.issues.get('DEMO-145').fields.status.name, 'In Review');
  assert.equal(mod.view.view().kind, 'issue', 'back on the issue');
  assert.ok(mod.says('Moved DEMO-145 to In Review.'));
  assert.ok(mod.texts().includes('IN REVIEW'), 'read again');
  // Approve asks for a resolution on its screen: refused here, nothing sent.
  await mod.click('ji-act-status');
  assert.ok(row(mod, 'ji-tr-41').includes('asks for more'));
  await mod.click('ji-tr-41');
  assert.ok(mod.says('Approve asks for Resolution on a screen this panel does not fill in: use the browser'), mod.texts().join('|'));
  assert.ok(!byId(mod.render(), 'ji-confirm'));
  assert.equal(acts(fake).length, 1);
  await mod.key('escape');
  assert.ok(mod.says('Approve asks for Resolution'), 'on its issue, the refusal stays');
  await mod.key('escape');
  await mod.click('ji-row-DEMO-102');
  assert.ok(!byId(mod.render(), 'ji-notice'), 'another issue: it goes');
  await mod.key('escape');
  await mod.click('ji-row-DEMO-145');
  await mod.click('ji-act-status');
  // One without a screen goes; Enter confirms too.
  await mod.click('ji-tr-61');
  await mod.key('enter');
  assert.equal(fake.jira.issues.get('DEMO-145').fields.status.name, 'Done');
  await fake.close();
});

test('assign to me, a comment and work logged: each asked first, each sent as Jira takes it', async () => {
  const fake = await startFakeJira();
  const mod = await start(configured(fake));
  await mod.tick();
  await mod.click('ji-row-DEMO-108'); // Bob's
  await mod.click('ji-act-assign');
  assert.ok(mod.says('Assign DEMO-108 to you (Jane Doe)? It is Bob Builder\'s now.'));
  assert.deepEqual(acts(fake), []);
  await mod.click('ji-confirm-yes');
  assert.deepEqual(acts(fake).map((r) => [r.method, r.path, JSON.parse(r.body)]), [['PUT', '/rest/api/2/issue/DEMO-108/assignee', { name: 'jane' }]]);
  assert.ok(mod.says('Assigned DEMO-108 to you.'));
  assert.ok(mod.says('Assignee: Jane Doe'));
  assert.ok(!byId(mod.render(), 'ji-act-assign'));

  // A comment: its form, nothing sent while empty, Cancel sends nothing.
  await mod.click('ji-act-comment');
  assert.equal(mod.view.view().kind, 'comment');
  await mod.click('ji-comment-yes');
  assert.ok(mod.says('Write something first.'));
  await mod.click('ji-comment-no');
  assert.equal(mod.view.view().kind, 'issue');
  await mod.click('ji-act-comment');
  mod.type(mod.input('ji-comment-body'), 'On it.\n* first\n* second');
  await mod.click('ji-comment-yes');
  const sent = acts(fake).at(-1);
  assert.deepEqual([sent.method, sent.path, JSON.parse(sent.body)], ['POST', '/rest/api/2/issue/DEMO-108/comment', { body: 'On it.\n* first\n* second' }]);
  assert.equal(mod.view.view().kind, 'issue');
  assert.ok(mod.says('Added your comment to DEMO-108.'));
  assert.ok(mod.says('On it.') && mod.says('second'), 'read again, drawn as markup');

  // Work logged: what is typed is checked first.
  await mod.click('ji-act-worklog');
  const time = mod.input('ji-work-time'), day = mod.input('ji-work-day');
  assert.equal(day.value(), util.dayOf(Date.now()), 'today by default');
  mod.type(time, 'an hour');
  await mod.submit(time);
  assert.ok(mod.says('Time spent is a number and a unit'));
  mod.type(time, '1h30m');
  assert.ok(byId(mod.render(), 'ji-work-yes') && row(mod, 'ji-work-yes') === 'Log 1h 30m');
  mod.type(day, '2999-01-01');
  await mod.click('ji-work-yes');
  assert.ok(mod.says('The date is a day as YYYY-MM-DD, today or before.'));
  mod.type(day, util.dayOf(Date.now() - 86_400_000));
  mod.type(mod.input('ji-work-note'), 'Paired with Bob');
  const posts = acts(fake).length;
  await mod.click('ji-work-yes');
  assert.equal(acts(fake).length, posts + 1);
  const log = acts(fake).at(-1), body = JSON.parse(log.body);
  assert.equal(log.path, '/rest/api/2/issue/DEMO-108/worklog');
  assert.equal(body.timeSpent, '1h 30m');
  assert.equal(body.comment, 'Paired with Bob');
  assert.match(body.started, new RegExp(`^${util.dayOf(Date.now() - 86_400_000)}T09:00:00\\.000[+-]\\d{4}$`));
  assert.ok(mod.says('Logged 1h 30m on DEMO-108.'));
  assert.ok(mod.says('Logged 1h 30m · 4h left'), 'read again');
  // Every POST and PUT says it comes from no browser (Jira's XSRF check).
  await fake.close();
});

test('a token that may only read: each action says Jira refused it, and nothing breaks', async () => {
  const fake = await startFakeJira();
  const mod = await start(configured(fake, { token: VIEWER }));
  await mod.tick();
  assert.ok(mod.says('DEMO Sprint 12') && mod.says('None of it is assigned to you (Read Only).'), mod.texts().join('|'));
  await mod.click('ji-row-DEMO-108');
  await mod.click('ji-act-assign');
  await mod.click('ji-confirm-yes');
  assert.ok(mod.says('Jira refused to let this token as viewer assign DEMO-108 (You do not have permission to assign issues.).'), mod.texts().join('|'));
  assert.ok(!mod.says('did not accept the token'), 'a 401 to an action is a refusal, not a bad token');
  await mod.click('ji-act-status');
  await mod.click('ji-tr-11');
  await mod.click('ji-confirm-yes');
  assert.ok(mod.says('Jira refused to let this token as viewer move DEMO-108 to In Progress (You do not have permission to transition this issue.).'), mod.texts().join('|'));
  assert.equal(mod.view.view().kind, 'status', 'stays where it was');
  await mod.key('escape');
  await mod.click('ji-act-comment');
  mod.type(mod.input('ji-comment-body'), 'Hello');
  await mod.click('ji-comment-yes');
  assert.ok(mod.says('Jira refused to let this token as viewer comment on DEMO-108'));
  assert.equal(mod.view.view().kind, 'comment', 'the comment stays, to copy elsewhere');
  await mod.key('escape');
  await mod.click('ji-act-worklog');
  mod.type(mod.input('ji-work-time'), '15m');
  await mod.click('ji-work-yes');
  assert.ok(mod.says('Jira refused to let this token as viewer log work on DEMO-108'));
  assert.equal(fake.jira.issues.get('DEMO-108').fields.assignee.name, 'bob');
  await mod.click('ji-dismiss');
  assert.ok(!byId(mod.render(), 'ji-notice'));
  await fake.close();
});

test('a Jira that stops answering: the rows stay with how old they are, then the error; with nothing read the error shows at once', async () => {
  const fake = await startFakeJira();
  const mod = await start(configured(fake));
  await mod.tick();
  fake.fail({ mode: 'status', status: 503, path: '^/rest/api/2/search$' });
  await run([mod], 62_000);
  assert.ok(mod.says('Last read 1 min ago · no answer since') || mod.says('Last read 62s ago'), mod.texts().join('|'));
  assert.ok(ids(mod.render(), 'ji-row-').length > 10, 'the rows stay');
  await run([mod], 90_000);
  assert.ok(mod.says('Could not read the issues of DEMO Sprint 12: Jira answered 503.'), mod.texts().join('|'));
  fake.recover();
  await run([mod], 62_000);
  assert.ok(!mod.says('Could not') && !mod.says('no answer since'), mod.texts().join('|'));
  // An issue never read: its error at once, not "Reading…" while it fails.
  fake.fail({ mode: 'refuse', path: '/issue/DEMO-120$' });
  await mod.click('ji-row-DEMO-120');
  assert.ok(mod.says('Could not reach Jira to read DEMO-120'), mod.texts().join('|'));
  assert.ok(!mod.says('Reading DEMO-120…'));
  fake.recover();
  await run([mod], 30_000); // its back-off, at least its usual half minute
  assert.ok(mod.says('DEMO-120') && !mod.says('Could not'), mod.texts().join('|'));
  await fake.close();
});

test('a read that never ends: the view shows an error once it is given up on, without another read asked, and the next one goes', async () => {
  const fake = await startFakeJira();
  const mod = await start(configured(fake));
  await mod.tick();
  fake.fail({ mode: 'stall', path: '/issue/DEMO-130$' });
  mod.press('ji-row-DEMO-130'); // its read never ends, so nothing settles: poke instead
  await mod.poke();
  assert.ok(mod.says('Reading DEMO-130…'), mod.texts().join('|'));
  assert.ok(byId(mod.render(), 'ji-refresh').children.some((c) => 'frames' in kept(c).calls), 'it spins while it reads');
  skew += data.READ_LIMIT + 1_000;
  // Drawing alone (no tick, so nothing asked again) turns it into an error.
  assert.ok(mod.says('Could not read DEMO-130: Jira gave no answer in time. Trying again.'), mod.texts().join('|'));
  assert.ok(!mod.says('Reading DEMO-130…'));
  fake.recover();
  skew += 30_000; // its back-off, at least its usual half minute
  await mod.poke();
  for (let i = 0; i < 200 && !mod.says('Assignee:'); i++) await new Promise((resolve) => setTimeout(resolve, 10));
  assert.ok(mod.says('Assignee:') && !mod.says('Could not'), mod.texts().join('|'));
  // The stalled read's answer comes at last, and is dropped.
  const shown = mod.texts().join('|');
  fake.release();
  await new Promise((resolve) => setTimeout(resolve, 50));
  assert.equal(mod.texts().join('|'), shown);
  await fake.close();
});

test('hidden, the panel still reads what its view shows, slowly, and at once when it shows again; never-read things at once', async () => {
  const fake = await startFakeJira();
  const w = configured(fake);
  w.props = { panel: true };
  w.layout = panelLayout(SHOWN);
  const panel = await start(w);
  await panel.tick();
  await panel.click('ji-row-DEMO-101');
  const issueReads = () => reads(fake, /\/issue\/DEMO-101\?/).length;
  w.layout = panelLayout({ hidden: true, active: 'jira-panel' });
  let n = issueReads();
  await run([panel], 9 * 60_000);
  assert.equal(issueReads(), n, 'not every half minute while hidden');
  await run([panel], 2 * 60_000);
  assert.equal(issueReads(), n + 1, 'every ten minutes');
  // Shown again: at once.
  n = issueReads();
  w.layout = panelLayout(SHOWN);
  await run([panel], 2_000);
  assert.equal(issueReads(), n + 1);
  // An issue opened while hidden (by another mod's jira.open) is read at once, hidden or not.
  w.layout = panelLayout({ hidden: true, active: 'jira-panel' });
  await run([panel], 4_000);
  panel.view.stack.push({ kind: 'issue', key: 'DEMO-102' });
  await panel.tick();
  assert.equal(reads(fake, /\/issue\/DEMO-102\?/).length, 1);
  await fake.close();
});

test('the popups\' logic: what came before the first read makes none, and others\' changes to the token user\'s issues make one each', () => {
  const me = { name: 'jane', key: 'jane' };
  const at = (ms) => new Date(ms).toISOString();
  const t0 = Date.parse('2026-11-17T10:00:00Z');
  const issue = (key, histories = [], comments = []) => ({ key, fields: { summary: `${key} summary`, comment: { comments } }, changelog: { histories } });
  const w = new alerts.Watch();
  assert.equal(w.window(t0), alerts.WINDOW_MIN);
  const old = issue('D-1', [{ id: '1', author: { name: 'bob' }, created: at(t0 - 60_000), items: [{ field: 'assignee', to: 'jane' }] }]);
  assert.deepEqual(w.read([old], me, t0), [], 'before the first read');
  const events = w.read([
    issue('D-1', [{ id: '1', author: { name: 'bob' }, created: at(t0 - 60_000), items: [{ field: 'assignee', to: 'jane' }] },
      { id: '2', author: { name: 'bob', displayName: 'Bob' }, created: at(t0 + 1_000), items: [{ field: 'status', fromString: 'To Do', toString: 'In Progress' }] }]),
    issue('D-2', [{ id: '3', author: { name: 'alice', displayName: 'Alice' }, created: at(t0 + 2_000), items: [{ field: 'assignee', fromString: 'Bob', to: 'jane' }] },
      { id: '4', author: { name: 'jane' }, created: at(t0 + 3_000), items: [{ field: 'status', toString: 'Done' }] }]),
    issue('D-3', [], [{ id: '7', author: { name: 'alice', displayName: 'Alice' }, body: 'Please *look* at {{this}}', created: at(t0 + 4_000) },
      { id: '8', author: { name: 'jane' }, body: 'mine', created: at(t0 + 5_000) }]),
  ], me, t0 + 60_000);
  assert.deepEqual(events.map((e) => [e.kind, e.key, e.by]), [['status', 'D-1', 'Bob'], ['assigned', 'D-2', 'Alice'], ['comment', 'D-3', 'Alice']]);
  assert.deepEqual(alerts.toast(events[0]), { title: 'D-1 is In Progress', body: 'D-1 summary · Bob moved it from To Do', level: 'info' });
  assert.deepEqual(alerts.toast(events[1]), { title: 'D-2 is yours now', body: 'D-2 summary · Alice assigned it to you', level: 'info' });
  assert.deepEqual(alerts.toast(events[2]), { title: 'Alice commented on D-3', body: 'Please look at this', level: 'info' });
  assert.deepEqual(w.read([issue('D-3', [], [{ id: '7', author: { name: 'alice' }, body: 'x', created: at(t0 + 4_000) }])], me, t0 + 120_000), [], 'each once');
  assert.equal(w.window(t0 + 120_000 + 30 * 60_000), 35, 'the window covers the time since the last read');
  assert.equal(alerts.summary(events).title, '3 updates on your Jira issues');
  w.reset();
  assert.equal(w.since, 0);
});

test('popups: none at first, then one each for an assignment, a comment and a status change by others; none for the token user\'s own or a muted issue', async () => {
  const fake = await startFakeJira();
  const w = configured(fake);
  w.props = { panel: true };
  w.layout = panelLayout(SHOWN);
  const panel = await start(w);
  await run([panel], 4_000);
  assert.deepEqual(w.toasts, [], 'what is so already makes none');
  const watchReads = () => reads(fake, /assignee = currentUser\(\) AND updated/).length;
  assert.equal(watchReads(), 1);
  fake.jira.assign('DEMO-104', 'jane', 'bob');
  skew += 1_000;
  fake.jira.comment('DEMO-102', 'alice', 'Can you look at *this*?');
  skew += 1_000;
  fake.jira.move('DEMO-103', 'In Progress', 'bob');
  await run([panel], 60_000);
  assert.equal(watchReads(), 2, 'every minute while shown');
  assert.deepEqual(w.toasts.map((t) => [t.title, t.body]), [
    ['DEMO-104 is yours now', `${fake.jira.issues.get('DEMO-104').fields.summary} · Bob Builder assigned it to you`],
    ['Alice Example commented on DEMO-102', 'Can you look at this?'],
    ['DEMO-103 is In Progress', `${fake.jira.issues.get('DEMO-103').fields.summary} · Bob Builder moved it from To Do`],
  ]);
  assert.deepEqual(w.toasts.map((t) => t.action), [{ name: 'jira.open-DEMO-104' }, { name: 'jira.open-DEMO-102' }, { name: 'jira.open-DEMO-103' }]);
  // Jane's own doings make none; nor does a muted issue.
  await panel.click('ji-row-DEMO-102');
  await panel.click('ji-act-comment');
  panel.type(panel.input('ji-comment-body'), 'Done.');
  await panel.click('ji-comment-yes');
  await panel.click('ji-mute-DEMO-102');
  assert.deepEqual(w.shared['jira.muted'], ['DEMO-102']);
  assert.deepEqual(svgsOf(panel, 'ji-mute-DEMO-102'), ['bell-off']);
  fake.jira.comment('DEMO-102', 'bob', 'muted');
  await run([panel], 60_000);
  assert.equal(w.toasts.length, 3);
  // More than three at once: one for all.
  for (const key of ['DEMO-112', 'DEMO-123', 'DEMO-156', 'DEMO-145']) fake.jira.comment(key, 'alice', 'Hi');
  await run([panel], 60_000);
  assert.deepEqual(w.toasts.slice(3).map((t) => [t.title, t.body.split(', ').sort().join(', '), t.action]), [['4 updates on your Jira issues', 'DEMO-112, DEMO-123, DEMO-145, DEMO-156', { name: 'jira.show' }]]);
  // Two copies post one; another start of the app posts none for what it finds.
  const again = await start(w);
  await run([panel, again], 60_000);
  assert.equal(w.toasts.length, 4);
  await fake.close();
});

test('a popup\'s click: hidden, the panel is shown by a layout command and opens on the issue; shown, jira.open-<KEY> takes it there', async () => {
  const fake = await startFakeJira();
  const w = configured(fake);
  w.props = { panel: true };
  w.layout = panelLayout(SHOWN);
  const panel = await start(w);
  await panel.tick();
  w.layout = panelLayout({ hidden: true, active: 'jira-panel' });
  await run([panel], 4_000);
  fake.jira.assign('DEMO-106', 'jane', 'bob');
  await run([panel], 3 * 60_000);
  assert.deepEqual(w.toasts.map((t) => t.title), ['DEMO-106 is yours now'], 'hidden, it reads and posts, every three minutes');
  assert.deepEqual(w.toasts[0].action, { name: 'layout.show', args: { id: 'panel' } });
  w.layout = panelLayout(SHOWN); // what the click did
  await panel.tick();
  assert.deepEqual([panel.view.view().kind, panel.view.view().key], ['issue', 'DEMO-106']);
  // Behind Files: the click selects it, and the panel shows its column itself.
  await panel.key('escape');
  w.layout = panelLayout();
  fake.jira.comment('DEMO-101', 'bob', 'Ping');
  await run([panel], 3 * 60_000);
  assert.deepEqual(w.toasts[1].action, { name: SELECT[0], args: SELECT[1] });
  w.ran = [];
  w.layout = panelLayout({ hidden: true, active: 'jira-panel' });
  await panel.tick();
  assert.deepEqual(w.ran, [SELECT, ['layout.show', { id: 'panel' }]]);
  w.layout = panelLayout(SHOWN);
  await panel.tick();
  assert.equal(panel.view.view().key, 'DEMO-101');
  // Shown: the command itself.
  await panel.key('escape');
  fake.jira.move('DEMO-112', 'In Progress', 'alice');
  await run([panel], 60_000);
  assert.deepEqual(w.toasts[2].action, { name: 'jira.open-DEMO-112' });
  await panel.send('open-DEMO-112');
  assert.deepEqual([panel.view.view().kind, panel.view.view().key], ['issue', 'DEMO-112']);
  await fake.close();
});

test('jira.open-<KEY> and shared("jira.open"): the panel shows on that issue, from a key, another copy or another mod, hidden or not', async () => {
  const fake = await startFakeJira();
  const w = configured(fake);
  // Without a panel in the layout (a tab): the copy goes there itself, any case of the key.
  const tab = await start(w);
  await tab.send('open-demo-120');
  assert.deepEqual([tab.view.view().kind, tab.view.view().key], ['issue', 'DEMO-120']);
  assert.ok(tab.says('DEMO-120'));
  await tab.send('open-NOT-A-KEY-X');
  assert.equal(tab.view.view().key, 'DEMO-120', 'not a key: nothing');
  await tab.send('show');
  assert.equal(tab.view.view().kind, 'list');

  // With the panel: an overlay copy that gets the command leaves it to the
  // panel and shows it; the panel goes there on its tick.
  const v = configured(fake);
  v.props = { panel: true };
  v.layout = panelLayout(SHOWN);
  const panel = await start(v, 'shared');
  await panel.tick();
  v.layout = panelLayout({ hidden: true, active: 'files' });
  await panel.tick();
  v.props = null;
  const overlay = await start(v, 'shared');
  v.ran = [];
  await overlay.send('open-DEMO-133');
  assert.deepEqual(v.ran, [SELECT, ['layout.show', { id: 'panel' }]]);
  assert.equal(overlay.view.view().kind, 'list', 'the panel goes there, not the overlay');
  v.layout = panelLayout(SHOWN);
  await panel.tick();
  assert.deepEqual([panel.view.view().kind, panel.view.view().key], ['issue', 'DEMO-133']);

  // Another mod sets shared("jira.open"): the hidden panel shows itself and goes there.
  await panel.key('escape');
  v.layout = panelLayout({ hidden: true, active: 'jira-panel' });
  await panel.tick();
  v.ran = [];
  v.shared['jira.open'] = { key: 'DEMO-140', at: Date.now() };
  await panel.tick();
  assert.deepEqual(v.ran, [SELECT, ['layout.show', { id: 'panel' }]]);
  assert.deepEqual([panel.view.view().kind, panel.view.view().key], ['issue', 'DEMO-140']);
  await panel.key('escape');
  await run([panel], 4_000);
  assert.equal(panel.view.view().kind, 'list', 'taken up once');
  // One left from an earlier app session is old: nothing.
  const later = await start(v, 'later');
  await later.tick();
  assert.equal(later.view.view().kind, 'list');
  await fake.close();
});

test('copies read once for all, and take up settings another saved', async () => {
  const fake = await startFakeJira();
  const w = configured(fake);
  const a = await start(w, 'two'), b = await start(w, 'two');
  await a.tick();
  await b.tick();
  fake.requests.length = 0;
  await run([a, b], 5 * 60_000);
  const both = fake.requests.length;
  const c = await start(configured(fake), 'one');
  await c.tick();
  fake.requests.length = 0;
  await run([c], 5 * 60_000);
  assert.equal(both, fake.requests.length, `two copies ${both}, one ${fake.requests.length}`);
  assert.ok(both > 0);
  // Settings saved in one are taken up by the other.
  await a.click('ji-settings');
  a.type(a.input('ji-set-board'), 'DEMO');
  await a.click('ji-set-save');
  await b.tick();
  assert.equal(b.view.settings.board, 'DEMO');
  await fake.close();
});

test('in the right-hand panel (props().panel) it is a card as Files and Changes are; elsewhere it fills its place', async () => {
  const fake = await startFakeJira();
  const w = configured(fake);
  w.props = { panel: true };
  const panel = await start(w);
  assert.ok('rounded' in kept(panel.render()).calls && 'border_1' in kept(panel.render()).calls);
  w.props = null;
  const tab = await start(w);
  assert.ok(!('rounded' in kept(tab.render()).calls));
  await fake.close();
});

// --- install.sh -----------------------------------------------------------------

// Herdr's tab bar (`mods/tabs/main.js`, 0.1.15) as far as the button needs
// it: the panel buttons and their `enabled` list, the per-space memory of
// the open panel (here: what it was given), the window buttons the button
// goes before, and an asset path of its own, which a clone rewrites.
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
// buttons; `enabled` is the list its syncPanel got; a click runs `run`.
function tabBar(text, { stack = ['files', 'changes', 'jira-panel'], hidden = true, active = 'files' } = {}) {
  const body = { id: 'panel-body', type: 'stack', active, children: stack.map((id) => ({ id })) };
  const panel = { id: 'panel', hidden, children: [body] };
  const root = { id: 'root', children: [{ id: 'tabs' }, panel] };
  const ran = [];
  const colors = new Proxy({}, { get: (_t, name) => (typeof name === 'string' ? `#${name}` : undefined) });
  const code = text.replace(/^import .*$/gm, '').replace('export default class', 'return class');
  // eslint-disable-next-line no-new-func
  const Tabs = new Function('View', 'div', 'svg', 'image', 'h_flex', 'spaces', 'panes', 'run', 'openOverlay', 'theme', 'window', 'shared', 'layout', 'settings', 'localStorage', code)(
    kit.View, kit.div, kit.svg, kit.svg, kit.h_flex, () => [{ id: 's1', active: true }], () => [], (c, a) => ran.push([c, a]), () => {}, () => colors, () => ({}),
    () => undefined, () => root, () => ({ zoom: 1 }), { getItem: () => null, setItem: () => {} });
  const bar = new Tabs();
  bar.init();
  return {
    ran,
    render() {
      const el = bar.render();
      return ids(el, 'panel-').map((id) => ({ id, texts: texts(byId(el, id)), click: () => byId(el, id).handlers.on_click() }));
    },
    get enabled() { return bar.enabled; },
  };
}

// A scratch app home: Herdr's tab bar, its installed.json, a personal
// layout `mine` (as a clone of main) whose tab bar is `bar`.
function appHome(bar = 'tabs') {
  const home = fs.mkdtempSync(path.join(os.tmpdir(), 'jira-install-'));
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
  return {
    home, app, checker,
    run: runIn(path.join(MOD, 'install.sh')),
    jenkins: runIn(path.join(MODS, 'jenkins', 'install.sh')),
    media: (...args) => runIn(path.join(MODS, 'media', 'install.sh'))(...args, ...(args.includes('right') ? ['--folder', path.join(home, 'drop')] : [])),
    read: (name = 'mine') => JSON.parse(fs.readFileSync(path.join(app, 'layouts', `${name}.json`), 'utf8')),
    main: (b) => fs.readFileSync(path.join(app, 'mods', b, 'main.js'), 'utf8'),
    mods: () => fs.readdirSync(path.join(app, 'mods')).sort(),
  };
}
const find = (node, id) => (node?.id === id ? node : (node?.children ?? []).map((c) => find(c, id)).find(Boolean) ?? null);

test('the search\'s helpers: a query as keys and words, and its text quoted and escaped for JQL and Lucene', () => {
  const q = (text, projects = ['DEMO']) => util.parseQuery(text, projects);
  assert.equal(q(''), null);
  assert.equal(q('   \n '), null);
  assert.deepEqual(q('pm-1234'), { keys: ['PM-1234'], open: true, words: '', number: '' }, 'a key, any case');
  assert.deepEqual(q(' 1234 '), { keys: ['DEMO-1234'], open: true, words: '', number: '1234' }, 'a bare number: a key of the board\'s project');
  assert.deepEqual(q('0042', ['DEMO', 'OTHER']), { keys: ['DEMO-42', 'OTHER-42'], open: true, words: '', number: '42' }, 'of each of its projects');
  assert.deepEqual(q('1234', []), { keys: [], open: false, words: '1234', number: '' }, 'no project known: words');
  assert.deepEqual(q('PM-12 timeout'), { keys: ['PM-12'], open: false, words: 'timeout', number: '' });
  assert.deepEqual(q('PM-12 pm-12 PM-13'), { keys: ['PM-12', 'PM-13'], open: false, words: '', number: '' }, 'each key once; more than one is a list');
  assert.deepEqual(q('fix 404 errors'), { keys: [], open: false, words: 'fix 404 errors', number: '' }, 'a number among words is a word');
  assert.deepEqual(q('PM-'), { keys: [], open: false, words: 'PM-', number: '' });
  assert.equal(q(Array.from({ length: 15 }, (_, i) => `A-${i + 1}`).join(' ')).keys.length, 10, 'ten keys at most');

  assert.equal(util.textJql('sprint panel'), 'text ~ "sprint panel" ORDER BY updated DESC');
  assert.equal(util.textJql('say "hi"'), 'text ~ "say \\\\\\"hi\\\\\\"" ORDER BY updated DESC', 'a quote: escaped for Lucene, then both for JQL');
  assert.equal(util.textJql('a\\b'), 'text ~ "a\\\\\\\\b" ORDER BY updated DESC', 'a backslash: four in the JQL');
  assert.equal(util.textJql('c++ (x)'), 'text ~ "c\\\\+\\\\+ \\\\(x\\\\)" ORDER BY updated DESC');
  assert.equal(util.textJql('this AND that OR NOT TO'), 'text ~ "this and that or not to" ORDER BY updated DESC', 'Lucene\'s operators as words');
  assert.equal(util.textJql('  two\n\tlines  '), 'text ~ "two lines" ORDER BY updated DESC');
  for (const none of ['', '   ', '()[]{}', '*?', '"', '\\', '+-&|!^~:/', '" * "']) assert.equal(util.textJql(none), null, none);
  assert.equal(util.textJql('x'.repeat(500)).length, 'text ~ "" ORDER BY updated DESC'.length + 300, 'cut at 300 characters');
});

// Types `text` into the search field and presses Enter.
const search = async (mod, text) => {
  mod.type(mod.view.query, text);
  await mod.submit(mod.view.query);
};
const TEXT = /jql=text ~ /;

test('the search field: a key or a bare number opens the issue, a missing one says so; Esc and back go back as they came', async () => {
  const fake = await startFakeJira();
  const w = configured(fake);
  const mod = await start(w);
  await mod.tick();
  assert.ok(byId(mod.render(), 'ji-search'), 'the field is over the sections');
  assert.ok(!byId(mod.render(), 'ji-search-clear'), 'no × while it is empty');
  // Folds and the list's scroll, to see them come back.
  await mod.click('ji-sec-project');
  const sections = ids(mod.render(), 'ji-');
  const list = kept(byId(mod.render(), 'ji-list'));
  assert.equal(list.calls.track_scroll[0], mod.view.listScroll, 'the sections keep their place in a handle of the view');

  // A full key, any case: Enter opens it, read once.
  await search(mod, 'demo-120');
  assert.equal(mod.view.view().kind, 'issue');
  assert.equal(mod.view.view().key, 'DEMO-120');
  assert.ok(mod.says('Assignee:'), mod.texts().join('|'));
  assert.equal(reads(fake, /\/issue\/DEMO-120\?/).length, 1, 'the search\'s read is the view\'s');
  assert.equal(reads(fake, TEXT).length, 0, 'a key alone searches no text');
  // Back: the results, the key in them; the field keeps the query.
  await mod.key('escape');
  assert.equal(mod.view.view().kind, 'list');
  assert.ok(!byId(mod.render(), 'ji-list') && byId(mod.render(), 'ji-results'));
  assert.equal(row(mod, 'ji-found-keys'), 'Key match 1');
  assert.deepEqual(ids(mod.render(), 'ji-row-'), ['ji-row-DEMO-120']);
  assert.equal(mod.view.query.value(), 'demo-120');
  assert.ok(byId(mod.render(), 'ji-search-clear'));
  // Esc again: the sections as they were.
  await mod.key('escape');
  assert.equal(mod.view.search, null);
  assert.equal(mod.view.query.value(), '');
  assert.deepEqual(ids(mod.render(), 'ji-'), sections);
  assert.equal(kept(byId(mod.render(), 'ji-list')).calls.track_scroll[0], mod.view.listScroll);

  // A bare number: a key of the board's project.
  await search(mod, '130');
  assert.equal(mod.view.view().key, 'DEMO-130');
  await mod.key('escape');
  // A key that is not there: said, and no text searched.
  await search(mod, 'DEMO-999');
  assert.equal(mod.view.view().kind, 'list');
  assert.ok(mod.says('DEMO-999 does not exist, or this token may not see it.'), mod.texts().join('|'));
  assert.ok(!mod.says('No issue matches'));
  await search(mod, '999');
  assert.ok(mod.says('DEMO-999 does not exist, or this token may not see it.'));
  assert.equal(reads(fake, TEXT).length, 0);
  // The ×: the sections as they were.
  await mod.click('ji-search-clear');
  assert.deepEqual(ids(mod.render(), 'ji-'), sections);
  // Emptying the field does the same.
  await search(mod, 'DEMO-999');
  assert.ok(byId(mod.render(), 'ji-results'));
  mod.type(mod.view.query, '');
  assert.deepEqual(ids(mod.render(), 'ji-'), sections);
  await fake.close();
});

test('the search field: words search every issue\'s text, closed ones dimmed; with a key, the key first and each issue once; no hits; capped at 50', async () => {
  const fake = await startFakeJira(0, { backlog: 60 });
  const w = configured(fake);
  const mod = await start(w);
  await mod.tick();
  // Free text, in another project too.
  await search(mod, 'other team');
  assert.equal(mod.view.view().kind, 'list');
  assert.equal(row(mod, 'ji-found-text'), 'Text matches 3');
  assert.deepEqual(ids(mod.render(), 'ji-row-').sort(), ['ji-row-OTHER-1', 'ji-row-OTHER-2', 'ji-row-OTHER-3']);
  assert.ok(!byId(mod.render(), 'ji-found-keys'));
  assert.ok(!byId(mod.render(), 'ji-sec-mine'), 'the results replace the sections');
  // A closed one: found, and dimmed; its row shows its status.
  await search(mod, 'long ago');
  assert.deepEqual(ids(mod.render(), 'ji-row-'), ['ji-row-DEMO-203']);
  assert.equal(kept(byId(mod.render(), 'ji-row-DEMO-203')).calls.opacity?.[0], 0.6);
  assert.match(row(mod, 'ji-row-DEMO-203'), /Done$/);
  await search(mod, 'other team');
  assert.equal(kept(byId(mod.render(), 'ji-row-OTHER-2')).calls.opacity, undefined, 'an open one is not');
  // A key and words: the key first, then the text's, without it again.
  await search(mod, 'DEMO-101 sprint');
  const rows = ids(mod.render(), 'ji-row-');
  assert.equal(rows[0], 'ji-row-DEMO-101');
  assert.equal(rows.filter((id) => id === 'ji-row-DEMO-101').length, 1, 'each issue once');
  assert.ok(rows.length > 3 && rows.includes('ji-row-DEMO-112'), rows.join(' '));
  assert.deepEqual(ids(mod.render(), 'ji-found-'), ['ji-found-keys', 'ji-found-text']);
  assert.equal(mod.view.view().kind, 'list', 'a key with words opens nothing');
  // A key that is not there, with words: said, and the words searched.
  await search(mod, 'DEMO-999 sprint');
  assert.ok(mod.says('DEMO-999 does not exist, or this token may not see it.') && rows.length > 3);
  // No hits.
  await search(mod, 'zebra');
  assert.ok(mod.says('No issue matches “zebra”.'), mod.texts().join('|'));
  // Capped: the 50 last updated, and a line saying so.
  await search(mod, 'backlog item');
  assert.equal(row(mod, 'ji-found-text'), 'Text matches 50 of 60');
  assert.equal(ids(mod.render(), 'ji-row-').length, 50);
  assert.ok(mod.says('The 50 last updated of 60 matches: add words to narrow it down.'));
  assert.equal(reads(fake, /jql=text ~ "backlog item"/).length, 1, 'one page of 50');
  // A result opens as a row does, with its actions; back is the results.
  await search(mod, 'other team');
  await mod.click('ji-row-OTHER-3');
  assert.equal(mod.view.view().key, 'OTHER-3');
  assert.ok(byId(mod.render(), 'ji-act-status') && byId(mod.render(), 'ji-act-comment'));
  await mod.click('ji-back');
  assert.equal(row(mod, 'ji-found-text'), 'Text matches 3');
  await fake.close();
});

test('the search field: no text breaks the JQL or adds to it; only Lucene\'s characters is no hits, asking nothing', async () => {
  const fake = await startFakeJira();
  const mod = await start(configured(fake));
  await mod.tick();
  const nasty = [
    'sprint" OR project = OTHER OR text ~ "x', 'sprint\\" OR key = DEMO-1', 'a\\b', 'C++ (side) [panel] {x}', 'AND OR NOT',
    'what? *wild* ~fuzzy^2 key:value /regex/ a&&b || !c', '\'single\' "double"', 'trailing\\', '\\"', 'ünïcödé 文字', 'sprint) OR (1=1',
    'order by key', 'side panel ORDER BY created ASC', 'EMPTY null', '\u0000ctrl\u0007', 'DEMO-101 "',
  ];
  for (const text of nasty) {
    const before = fake.requests.length;
    await search(mod, text);
    const asked = fake.requests.slice(before).filter((r) => TEXT.test(decodeURIComponent(r.query)));
    for (const r of asked) assert.match(decodeURIComponent(r.query), /jql=text ~ "(?:[^"\\]|\\.)*" ORDER BY updated DESC&fields=/, text);
    assert.ok(!mod.view.search.error, `${text}: ${mod.view.search.error?.message}`);
    assert.ok(!mod.says('did not take') && !mod.says('Could not'), `${text}: ${mod.texts().join('|')}`);
  }
  // The quote did not end the string: OTHER's issues come only when their text has the words.
  await search(mod, 'sprint" OR project = OTHER OR text ~ "x');
  assert.ok(mod.says('No issue matches'), mod.texts().join('|'));
  // Lucene's characters alone: no hits, and Jira not asked.
  for (const text of ['()', '* ? :', '"""', '\\\\', '[]{}^~']) {
    const before = fake.requests.length;
    await search(mod, text);
    assert.equal(fake.requests.length, before, text);
    assert.ok(mod.says(`No issue matches “${text}”.`), mod.texts().join('|'));
  }
  await fake.close();
});

test('the search field: an error shows as a read\'s does; a slow answer is given up on; an older answer never replaces a newer; the timed reads leave the results', async () => {
  const fake = await startFakeJira();
  const mod = await start(configured(fake));
  await mod.tick();
  // An error from Jira.
  fake.fail({ mode: 'status', status: 503, path: '^/rest/api/2/search$' });
  await search(mod, 'other team');
  assert.ok(mod.says('Could not search for “other team”: Jira answered 503.'), mod.texts().join('|'));
  fake.recover();
  await mod.submit(mod.view.query);
  assert.equal(row(mod, 'ji-found-text'), 'Text matches 3');
  // A search with no answer: searching, then given up on after the read limit.
  fake.fail({ mode: 'stall', path: '^/rest/api/2/search$', count: 1 });
  mod.type(mod.view.query, 'long ago');
  mod.view.query.handlers.submit({}, mod.cx);
  await mod.poke();
  assert.ok(mod.says('Searching Jira for “long ago”…'), mod.texts().join('|'));
  skew += data.READ_LIMIT + 1_000;
  assert.ok(mod.says('Jira gave no answer to the search for “long ago” in time. Enter searches again.'), mod.texts().join('|'));
  fake.release();
  for (let i = 0; i < 50; i++) await new Promise((resolve) => setTimeout(resolve, 5));
  assert.ok(mod.says('in time'), 'its late answer (a 503) is dropped');
  // Two in a row: the first stalls, the second answers; the first's late answer changes nothing.
  fake.fail({ mode: 'stall', path: '^/rest/api/2/search$', count: 1 });
  mod.type(mod.view.query, 'long ago');
  mod.view.query.handlers.submit({}, mod.cx);
  await mod.poke();
  mod.type(mod.view.query, 'other team');
  mod.view.query.handlers.submit({}, mod.cx);
  for (let i = 0; i < 200 && !byId(mod.render(), 'ji-found-text'); i++) await new Promise((resolve) => setTimeout(resolve, 10));
  assert.equal(row(mod, 'ji-found-text'), 'Text matches 3');
  fake.recover();
  fake.release();
  await mod.settle();
  assert.equal(row(mod, 'ji-found-text'), 'Text matches 3');
  assert.ok(!mod.says('Could not') && !mod.says('long ago'), mod.texts().join('|'));
  // The timed reads go on behind the results, and leave them.
  const n = reads(fake, SPRINT).length;
  await run([mod], 4 * 60_000);
  assert.ok(reads(fake, SPRINT).length > n, 'the sprint is still read');
  assert.equal(row(mod, 'ji-found-text'), 'Text matches 3');
  assert.ok(byId(mod.render(), 'ji-results'));
  // The refresh button searches again.
  const asked = reads(fake, TEXT).length;
  await mod.click('ji-refresh');
  assert.equal(reads(fake, TEXT).length, asked + 1);
  assert.equal(row(mod, 'ji-found-text'), 'Text matches 3');
  await fake.close();
});

test('install.sh puts the mod\'s own files in an app home, once, checked first; --uninstall takes them out, --forget the settings too', () => {
  const h = appHome();
  const target = path.join(h.app, 'mods', 'jira');
  let r = h.run();
  assert.equal(r.code, 0, r.out);
  const modules = fs.readdirSync(MOD).filter((f) => f.endsWith('.js'));
  assert.deepEqual(fs.readdirSync(target).sort(), [...modules, 'icons', 'mod.json'].sort(), 'every module, no tests, README or installer, nothing the check wrote');
  assert.equal(fs.readdirSync(path.join(target, 'icons')).length, fs.readdirSync(path.join(MOD, 'icons')).length);
  assert.ok(fs.readFileSync(`${h.checker}.log`, 'utf8').startsWith('check '), 'checked before it was put in place');
  assert.ok(!h.mods().some((f) => f.startsWith('.')), 'no staging folder is left');
  assert.deepEqual(find(h.read(), 'tabs').content, { mod: 'tabs' }, 'no --panel: no layout changes');
  assert.match(r.out, /show it with: install\.sh --panel right/);
  const before = fs.statSync(path.join(target, 'main.js')).mtimeMs;
  r = h.run();
  assert.match(r.out, /up to date/);
  assert.equal(fs.statSync(path.join(target, 'main.js')).mtimeMs, before, 'again: nothing changes');
  // The settings stay unless --forget.
  fs.mkdirSync(path.join(h.app, 'jira'));
  fs.writeFileSync(path.join(h.app, 'jira', 'settings.json'), '{}', { mode: 0o600 });
  r = h.run('--uninstall');
  assert.equal(r.code, 0, r.out);
  assert.match(r.out, /the settings and the token stay/);
  assert.ok(!fs.existsSync(target) && fs.existsSync(path.join(h.app, 'jira', 'settings.json')));
  assert.equal(h.run().code, 0);
  r = h.run('--uninstall', '--forget');
  assert.equal(r.code, 0, r.out);
  assert.ok(!fs.existsSync(target) && !fs.existsSync(path.join(h.app, 'jira')));
  // Another mod of the same name is never replaced or removed; arguments are checked.
  fs.mkdirSync(target);
  fs.writeFileSync(path.join(target, 'main.js'), '// somebody else\'s\n');
  assert.notEqual(h.run().code, 0);
  assert.notEqual(h.run('--uninstall').code, 0);
  assert.equal(fs.readFileSync(path.join(target, 'main.js'), 'utf8'), '// somebody else\'s\n');
  for (const bad of [['--forget'], ['--panel', 'left'], ['--layout', 'mine'], ['--uninstall', '--panel', 'right'], ['--layout', '../x', '--panel', 'right'], ['--what']]) {
    assert.notEqual(h.run(...bad).code, 0, bad.join(' '));
  }
  fs.rmSync(h.home, { recursive: true });
});

test('install.sh --panel right: the panel in the stack and a Jira button in a copy of Herdr\'s tab bar; again changes nothing; --panel off and --uninstall put it back', () => {
  const h = appHome();
  const layoutBefore = h.read();
  const modsBefore = h.mods();
  let r = h.run('--panel', 'right');
  assert.equal(r.code, 0, r.out);
  const mine = h.read();
  const stack = find(mine, 'panel-body');
  assert.deepEqual(stack.children.at(-1), { id: 'jira-panel', type: 'block', content: { mod: 'jira', props: { panel: true } } });
  assert.equal(stack.active, 'files', 'the shown child stays');
  assert.equal(mine.floating, undefined, 'no block of its own floats: the panel reads for the popups itself');
  assert.equal(find(mine, 'tabs').content.mod, 'tabs-jira');
  assert.equal(mine.note, 'räksmörgås');
  assert.equal(h.main('tabs'), STOCK_TABS, 'Herdr\'s own is never changed');
  const clone = h.main('tabs-jira');
  assert.match(clone.split('\n')[0], /^\/\/ Herdr's tabs, cloned by mods\/jira\/install\.sh/);
  assert.ok(!clone.includes('mods/tabs/'), 'asset paths point at the copy');
  assert.ok(clone.includes('    this.syncPanel(space, panel, body, enabled);\n'), 'the call itself is left as it is');
  // The tab bar as the app runs it: the button, which shows Jira in the panel, and the panel in syncPanel's list.
  const bar = tabBar(clone);
  assert.deepEqual(bar.render().map((b) => [b.id, b.texts]), [['panel-files', ['Files']], ['panel-changes', ['Changes']], ['panel-jira-panel', ['Jira']]]);
  assert.deepEqual(bar.enabled, ['files', 'changes', 'jira-panel']);
  bar.render().at(-1).click();
  assert.deepEqual(bar.ran, [SELECT, ['layout.show', { id: 'panel' }]]);
  assert.deepEqual(tabBar(clone, { stack: ['files', 'changes'] }).render().map((b) => b.id), ['panel-files', 'panel-changes'], 'no panel, no button');
  // Again: nothing changes.
  const text = JSON.stringify(mine);
  r = h.run('--panel', 'right');
  assert.equal(r.code, 0, r.out);
  assert.equal(JSON.stringify(h.read()), text);
  assert.equal(h.main('tabs-jira'), clone);
  r = h.run('--panel', 'off');
  assert.equal(r.code, 0, r.out);
  assert.deepEqual(h.read(), layoutBefore, 'the layout as it was');
  assert.deepEqual(h.mods(), [...modsBefore, 'jira'].sort(), 'the copy of the tab bar is gone, the mod stays');
  assert.equal(h.run('--panel', 'right').code, 0);
  r = h.run('--uninstall');
  assert.equal(r.code, 0, r.out);
  assert.deepEqual(h.read(), layoutBefore);
  assert.deepEqual(h.mods(), modsBefore);
  // Herdr's own layout is refused; a layout without the panel's stack is left alone.
  fs.writeFileSync(path.join(h.app, 'layouts', 'main.json'), '{"id":"root"}');
  r = h.run('--panel', 'right', '--layout', 'main');
  assert.notEqual(r.code, 0);
  assert.match(r.out, /main is Herdr's own layout/);
  fs.writeFileSync(path.join(h.app, 'layouts', 'bare.json'), '{"id":"root","type":"row","children":[]}');
  r = h.run('--panel', 'right', '--layout', 'bare');
  assert.notEqual(r.code, 0);
  assert.match(r.out, /no right-hand panel like Herdr's layouts/);
  fs.rmSync(h.home, { recursive: true });
});

test('beside the Jenkins and Media mods and Corgi: each one\'s lines in a shared tab bar stay, in any order, and each takes out only its own', () => {
  // Jenkins first (its clone tabs-jenkins), then Jira and Media into that copy, then Corgi's lines.
  const h = appHome();
  const before = h.read();
  let r = h.jenkins('--panel', 'right');
  assert.equal(r.code, 0, r.out);
  const jenkinsOnly = h.main('tabs-jenkins');
  r = h.run('--panel', 'right');
  assert.equal(r.code, 0, r.out);
  assert.match(r.out, /put a Jira button into mods\/tabs-jenkins\/main\.js/);
  assert.ok(!h.mods().includes('tabs-jira'), 'no copy of its own: the layout\'s copy is edited');
  r = h.media('--panel', 'right');
  assert.equal(r.code, 0, r.out);
  fs.writeFileSync(path.join(h.app, 'mods', 'tabs-jenkins', 'main.js'), corgiLines(h.main('tabs-jenkins')));
  const stack = ['files', 'changes', 'jenkins-panel', 'jira-panel', 'media-panel', 'corgi-questions'];
  let bar = tabBar(h.main('tabs-jenkins'), { stack });
  assert.deepEqual(bar.render().map((b) => b.id), stack.map((id) => `panel-${id}`));
  assert.deepEqual(new Set(bar.enabled), new Set(stack));
  // Again, with all of them there: unchanged.
  const all = h.main('tabs-jenkins');
  assert.equal(h.run('--panel', 'right').code, 0);
  assert.equal(h.main('tabs-jenkins'), all);
  // Jira off: the others' lines as they were.
  r = h.run('--panel', 'off');
  assert.equal(r.code, 0, r.out);
  assert.ok(!h.main('tabs-jenkins').includes('jira'), 'none of Jira\'s lines');
  bar = tabBar(h.main('tabs-jenkins'), { stack: stack.filter((id) => id !== 'jira-panel') });
  assert.deepEqual(bar.render().map((b) => b.id), stack.filter((id) => id !== 'jira-panel').map((id) => `panel-${id}`));
  assert.equal(find(h.read(), 'tabs').content.mod, 'tabs-jenkins');
  r = h.media('--panel', 'off');
  assert.equal(r.code, 0, r.out);
  assert.equal(h.main('tabs-jenkins'), corgiLines(jenkinsOnly));
  fs.rmSync(h.home, { recursive: true });

  // Jira first (tabs-jira), then Jenkins into that copy (it replaces the
  // syncPanel call, which Jira's line leaves as it is); Jenkins off, then Jira off.
  const g = appHome();
  assert.equal(g.run('--panel', 'right').code, 0);
  r = g.jenkins('--panel', 'right');
  assert.equal(r.code, 0, r.out);
  bar = tabBar(g.main('tabs-jira'), { stack: ['files', 'changes', 'jira-panel', 'jenkins-panel'] });
  assert.deepEqual(bar.render().map((b) => b.id), ['panel-files', 'panel-changes', 'panel-jira-panel', 'panel-jenkins-panel']);
  assert.deepEqual(bar.enabled, ['files', 'changes', 'jira-panel', 'jenkins-panel']);
  assert.equal(g.run('--panel', 'right').code, 0, 'Jira again, after Jenkins');
  assert.deepEqual(tabBar(g.main('tabs-jira'), { stack: ['files', 'changes', 'jira-panel', 'jenkins-panel'] }).render().map((b) => b.id), ['panel-files', 'panel-changes', 'panel-jira-panel', 'panel-jenkins-panel']);
  r = g.jenkins('--panel', 'off');
  assert.equal(r.code, 0, r.out);
  r = g.run('--panel', 'off');
  assert.equal(r.code, 0, r.out);
  assert.deepEqual(g.read(), before, 'the layout as it was, back on Herdr\'s tab bar');
  assert.ok(!g.mods().includes('tabs-jira'));
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
  assert.equal(tabBar(h.main('tabs-gitlab')).render().at(-1).id, 'panel-jira-panel');
  r = h.run('--uninstall');
  assert.equal(r.code, 0, r.out);
  assert.equal(h.main('tabs-gitlab'), gitlab);
  assert.deepEqual(h.read(), before);
  fs.rmSync(h.home, { recursive: true });
});

test('nothing of the user\'s: no real host, key or name in the mod\'s files', () => {
  const files = fs.readdirSync(MOD, { recursive: true }).filter((f) => /\.(js|mjs|md|py|sh|json)$/.test(f));
  for (const f of files) {
    const text = fs.readFileSync(path.join(MOD, f), 'utf8');
    for (const host of text.match(/https?:\/\/[a-z0-9.-]+/gi) ?? []) {
      assert.match(host, /^https?:\/\/(jira\.example\.com|example\.com|127\.0\.0\.1|fake|localhost|herdr\.dev|lucide\.dev|github\.com|www\.w3\.org)$/, `${f}: ${host}`);
    }
  }
});

// `node tests/test.mjs <text>` runs only the cases whose name has it.
const only = process.argv[2] ?? '';
let failed = 0;
for (const { name, run } of cases.filter((c) => c.name.includes(only))) {
  skew = 0;
  try {
    await run();
    console.log(`ok    ${name}`);
  } catch (error) {
    failed += 1;
    console.log(`FAIL  ${name}\n${error.stack}`);
  }
}
const ran = cases.filter((c) => c.name.includes(only)).length;
console.log(failed ? `${failed} of ${ran} failed` : `all ${ran} passed`);
process.exit(failed ? 1 : 0);
