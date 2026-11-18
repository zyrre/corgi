// Runs the Herdr app's Corgi mod (herdr-app/mods/corgi) under Node, past the first
// frame `herdr-app check` renders: with stand-ins for the app's `gpui-kit`,
// `gpui-base` and `herdr` modules, a made-up window (spaces, panes, agents)
// and canned answers for the programs a mod runs. A stand-in element keeps
// its texts, children and handlers, so a test reads what a view shows and
// clicks and types in it. Nothing here asks a real app, hc or corgi.
//
//   node scripts/test-app-mods.mjs

import assert from 'node:assert/strict';
import { registerHooks } from 'node:module';

const HOME = '/Users/jane';
const APP = `${HOME}/.config/herdr-app`;
const CORGI = `${HOME}/repos/corgi/target/release/corgi`;

// --- the stand-in modules ------------------------------------------------------

// An element: every builder call returns it; children, texts and handlers
// are kept. `.when` runs its callback only when its condition holds, and the
// callback must give back the element it got, as in the app.
// What was kept of each element: { kind, args, children, handlers, id, calls }.
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
        } else if (name === 'id') target.id = a[0];
        else if (name === 'on_mouse_down' || name === 'on_mouse_up') target.handlers[`${name}:${a[0]}`] = a[1];
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

// The retained text of an input: `type` is the person typing.
class InputState {
  // The app makes no field while a view draws: "InputState.new(...) cannot
  // run during render".
  static new(options = {}) {
    if (globalThis.__rendering) throw new Error('InputState.new(...) cannot run during render; create state in init() or in an event handler');
    return new InputState(options);
  }
  constructor(options) {
    this.text = options.value ?? '';
    this.placeholder = options.placeholder ?? '';
    this.handlers = {};
    this.focused = 0;
  }
  value() {
    return this.text;
  }
  set_value(next) {
    this.text = next;
  }
  on(event, handler) {
    this.handlers[event] = handler;
    return true;
  }
  focus() {
    this.focused += 1;
  }
}

const kit = {
  View: class View {},
  div: () => element('div'),
  svg: (path) => element('svg', [path]),
  image: (path) => element('image', [path]),
  v_flex: () => element('v_flex'),
  h_flex: () => element('h_flex'),
  InputState,
  Input: { new: (state) => element('input', [state]) },
};

// What the window holds and what the mod did to it.
function world(overrides = {}) {
  const w = {
    spaces: [],
    panes: [],
    agents: [],
    terminals: [],
    focused: null,
    sidebar: 268,
    shared: {},
    files: {},
    programs: [], // [(command, args, options) => result | promise | undefined]
    ran: [], // app commands: [name, args]
    execs: [], // programs run: [command, ...args]
    calls: [], // the same, whole: { command, args, options }
    machineList: [], // the other machines, as `hc machine ls --json` lists them
    toasts: [],
    commands: [], // `<mod>.<command>`s waiting for the mod, as from a key
    agentOrder: null,
    hcs: [], // hc's arguments, each call
    hcPrograms: [], // [(args) => result | promise | undefined], before the machines list
    writes: [], // files written: [path, text]
    ...overrides,
  };
  const colors = new Proxy({}, { get: (_t, name) => (typeof name === 'string' ? `#${name}` : undefined) });
  w.api = {
    spaces: () => w.spaces,
    panes: () => w.panes,
    agents: () => w.agents,
    terminals: () => w.terminals,
    machines: () => [{ name: null, layout: 'ready' }, ...w.machineList.map((m) => ({ name: m.name, layout: 'ready' }))],
    focused: () => w.focused,
    layout: () => ({ id: 'root', children: [{ id: 'sidebar', size: { px: w.sidebar }, children: [] }] }),
    window: () => ({ platform: 'macos', active: true, fullscreen: false }),
    theme: () => colors,
    shared: (key) => w.shared[key],
    setShared: (key, value) => {
      w.shared[key] = value;
      return null;
    },
    setAgentOrder: (order) => {
      w.agentOrder = order;
      return null;
    },
    run: (name, args) => {
      w.ran.push([name, args]);
      if (name === 'layout.resize' && args.id === 'sidebar') w.sidebar = args.px;
      return null;
    },
    notify: (toast) => {
      w.toasts.push(toast);
      return 'toast';
    },
    openOverlay: (spec) => {
      w.ran.push(['overlay', spec]);
      return 'overlay';
    },
    home: () => HOME,
    appHome: () => APP,
    readTextFile: async (path) => w.files[path] ?? null,
    // As the app's: only over the text the writer read (null: a new file).
    writeTextFile: async (path, text, expected) => {
      if ((w.files[path] ?? null) !== expected) throw new Error(`${path}: stale snapshot`);
      w.files[path] = text;
      w.writes.push([path, text]);
      return null;
    },
    checking: () => false,
    // The commands sent so far, then none: a mod's loop for them ends.
    nextCommand: () => (w.commands.length ? Promise.resolve(w.commands.shift()) : Promise.reject(new Error('no more commands in a test'))),
    // No folder is watched here: a mod falls back to its timers.
    watchFolder: () => Promise.reject(new Error('no watch in a test')),
    hc: async (args = []) => {
      w.hcs.push(args);
      for (const program of w.hcPrograms) {
        const result = program(args);
        if (result !== undefined) return { code: 0, stdout: '', stderr: '', ...(await result) };
      }
      return { code: 0, stdout: JSON.stringify({ machines: w.machineList }), stderr: '' };
    },
    exec: async (command, args = [], options = {}) => {
      w.execs.push([command, ...args]);
      w.calls.push({ command, args, options });
      for (const program of w.programs) {
        // An answer, or the promise of one; nothing when it is another program.
        const result = program(command, args, options);
        if (result !== undefined) return { code: 0, stdout: '', stderr: '', ...(await result) };
      }
      throw new Error(`${command}: not installed`);
    },
  };
  return w;
}

const HERDR_EXPORTS = [
  'spaces', 'panes', 'agents', 'terminals', 'machines', 'focused', 'layout', 'window', 'theme', 'shared', 'setShared',
  'setAgentOrder', 'run', 'notify', 'openOverlay', 'home', 'appHome', 'readTextFile', 'writeTextFile', 'watchFolder', 'nextCommand',
  'hc', 'exec', 'props', 'checking',
];
const sources = {
  'gpui-kit': 'const k = globalThis.__kit; export const View = k.View, div = k.div, svg = k.svg, image = k.image;',
  'gpui-base': 'const k = globalThis.__kit; export const v_flex = k.v_flex, h_flex = k.h_flex, InputState = k.InputState, Input = k.Input;',
  herdr: HERDR_EXPORTS.map((name) => `export const ${name} = (...a) => globalThis.__world.api.${name}(...a);`).join('\n'),
};
globalThis.__kit = kit;
globalThis.localStorage = { getItem: () => null, setItem: () => {}, removeItem: () => {} };
registerHooks({
  resolve(specifier, context, next) {
    if (specifier in sources) return { url: `stand-in:${specifier}`, shortCircuit: true };
    return next(specifier, context);
  },
  load(url, context, next) {
    if (url.startsWith('stand-in:')) return { format: 'module', source: sources[url.slice(9)], shortCircuit: true };
    // A mod has no package.json to say its main.js is a module.
    return next(url, url.includes('/herdr-app/mods/') ? { ...context, format: 'module' } : context);
  },
});
// A mod's own modules (`./data.js`) come with the copy of main.js that
// imports them, so each `start` has modules of its own, as an app start
// does, and copies started with the same `module` share them, as the
// copies of one app's mod do.
registerHooks({
  resolve(specifier, context, next) {
    const resolved = next(specifier, context);
    const parent = context.parentURL ? new URL(context.parentURL) : null;
    if (!parent?.pathname.includes('/herdr-app/mods/') || !parent.search || !specifier.startsWith('.')) return resolved;
    const url = new URL(resolved.url);
    url.search = parent.search;
    return { ...resolved, url: url.href };
  },
});

let copies = 0;
// A copy of the Corgi mod, started in the window `w` with the block's
// `props`: its view, and what its tasks and timers are. Copies given the
// same `module` (a name) share its modules, as the copies of one mod in the
// app do; any other start loads them afresh, as an app start does.
async function start(w, props = {}, module = `copy-${copies++}`) {
  globalThis.__world = w;
  const loaded = await import(new URL(`../herdr-app/mods/corgi/main.js?module=${module}`, import.meta.url));
  const tasks = [];
  const cx = {
    notified: 0,
    timers: [],
    notify() {
      this.notified += 1;
    },
    spawn(task) {
      tasks.push(Promise.resolve(task(cx)));
    },
    // The AsyncContext's pause: none here, counted.
    slept: 0,
    sleep: async () => {
      cx.slept += 1;
    },
    timer: {
      every: (ms, tick) => cx.timers.push({ ms, tick }),
      after: (_ms, tick) => tasks.push(Promise.resolve().then(() => tick(cx))),
    },
    // A handle counts how often it was given the keyboard.
    focus_handle: () => ({
      focused: 0,
      focus() {
        this.focused += 1;
      },
    }),
    // How many clicks kept to themselves: a button inside a clickable row.
    stopped: 0,
    stop_propagation() {
      this.stopped += 1;
    },
  };
  const mod = new loaded.default();
  // The block's props come from `props()`, as in the app; init's own
  // argument is not them.
  w.api.props = () => props;
  mod.init(undefined, cx);
  // The view the props chose (projects.js, brand.js).
  const view = mod.part;
  // Runs until no task is left running, the ones started meanwhile too.
  const settle = async () => {
    while (tasks.length) await tasks.shift();
  };
  // One round of the timers of a period.
  const tick = async (ms) => {
    for (const timer of cx.timers.filter((timer) => timer.ms === ms)) timer.tick(cx);
    await settle();
  };
  await settle();
  const render = () => {
    globalThis.__rendering = true;
    try {
      return mod.render(cx);
    } finally {
      globalThis.__rendering = false;
    }
  };
  return { mod, view, cx, settle, tick, render };
}

// --- reading a rendered view -----------------------------------------------------

function walk(el, visit) {
  const node = kept(el);
  if (node === null || typeof node !== 'object') return;
  visit(node);
  for (const child of node.children ?? []) walk(child, visit);
}

// Every text under `el`, in the order it is drawn.
// A spinner's frames are a mark, as its ring was, not text the view says:
// they are left out.
// The mods' one spinner: braille frames stepped natively, in `working`, in
// a fixed box `size` wide and high (12 px, 10 for the pull mark), never an
// svg turned with `spin`.
const SPIN = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];
function assertSpinner(el, size = 12, what = 'a spinner') {
  const node = kept(el);
  assert.equal(node?.kind, 'div', `${what}: a div of frames, not ${node?.kind}`);
  assert.deepEqual(node.calls.frames, [SPIN, 90], `${what}: braille frames`);
  assert.deepEqual([node.calls.w, node.calls.h], [[size], [size]], `${what}: a fixed box`);
  assert.ok('flex_none' in node.calls && 'justify_center' in node.calls, `${what}: whole and centered`);
  assert.deepEqual(node.calls.text_color, ['#working'], `${what}: in working`);
  assert.ok(!('spin' in node.calls));
}
// The line that says `text`, its spinner first: the spinner before it.
function spinnerBefore(root, text) {
  let line = null;
  walk(root, (node) => {
    if ((node.children ?? []).some((child) => kept(kept(child)?.children?.[0]) === text)) line = node;
  });
  assert.ok(line, `no line says ${text}`);
  assertSpinner(line.children[0], 12, text);
}

// Spinners (braille frames, or a turning svg) under `el`.
function spinnersIn(el) {
  let n = 0;
  walk(el, (node) => { if ('frames' in node.calls || 'spin' in node.calls) n += 1; });
  return n;
}
// A state's badge: a small pill saying `text`, whole, in the state's color.
// Solid for most (its text in the fill's contrast; the stand-in theme's
// colors are names, which read as dark), a tint for IDLE, an outline for
// EXITED; MERGE is the merge mark, in the accent.
const BADGE_LOOK = {
  working: { bg: ['#working'], text_color: ['#ffffff'] },
  blocked: { bg: ['#blocked'], text_color: ['#ffffff'] },
  done: { bg: ['#done'], text_color: ['#ffffff'] },
  paused: { bg: ['#muted'], text_color: ['#ffffff'] },
  idle: { bg: ['#muted33'], text_color: ['#muted'] },
  exited: { border_color: ['#blocked'], text_color: ['#blocked'] },
  merge: { bg: ['#accent'], text_color: ['#accent_foreground'] },
};
function assertBadge(el, key, text) {
  const node = kept(el);
  assert.deepEqual(texts(node), [text]);
  assert.ok('flex_none' in node.calls && 'rounded' in node.calls, `${text}: a whole pill`);
  for (const [call, args] of Object.entries(BADGE_LOOK[key])) assert.deepEqual(node.calls[call], args, `${text}: ${call}`);
  if (key === 'exited') assert.ok(!('bg' in node.calls), 'EXITED is an outline');
}

// The badge at the right end of an agent row's second line (null: none).
const markOf = (root, id) => {
  const second = kept(byId(root, id).children[1]);
  const mark = kept(second.children.at(-1));
  return mark && ['badge-', 'merge-'].some((p) => mark.id?.startsWith(p)) ? texts(mark).join('') : null;
};

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
  walk(root, (node) => {
    if (node.id === id) found ??= node;
  });
  return found;
}

function ids(root, prefix) {
  const out = [];
  walk(root, (node) => {
    if (typeof node.id === 'string' && node.id.startsWith(prefix)) out.push(node.id);
  });
  return out;
}

function click(root, id, cx, handler = 'on_click') {
  const node = byId(root, id);
  assert.ok(node, `nothing is drawn with the id ${id}`);
  assert.ok(node.handlers[handler], `${id} has no ${handler}`);
  node.handlers[handler]({ click_count: 1 }, cx);
}

// --- a window like the one the brief describes -------------------------------------

const REPO = `${HOME}/repos/weather`;
const TREES = `${HOME}/.herdr/worktrees/weather`;
const CLAUDE = { key: 'claude', name: 'Claude Code', icon: 'icons/claude.svg', tint: true, shell: false, agent: true };
const SHELL = { key: 'shell', name: 'zsh', icon: null, tint: false, shell: true, agent: false };

function space(id, name, folder, extra = {}) {
  return { id, index: 0, name, named: false, folder, machine: null, branch: null, active: false, status: null, tabs: [{ id: `tab-${id}`, index: 1, name: '1', named: false, active: true }], ...extra };
}
function pane(spaceId, terminal, cwd, program, extra = {}) {
  return { pane: `pane-${terminal}`, terminal, tab: `tab-${spaceId}`, title: '', cwd, branch: null, status: null, app: null, program, focused: false, zoomed: false, ...extra };
}
function agent(spaceId, terminal, status, title, extra = {}) {
  return { address: terminal, pane: `pane-${terminal}`, space: spaceId, space_name: spaceId, tab: `tab-${spaceId}`, status, app: 'claude-code', program: CLAUDE, title, seq: 1, pinned: false, focused: false, ...extra };
}

// The folders that are checkouts, for the stand-in git: [top, common git dir].
const CHECKOUTS = {
  [REPO]: [REPO, `${REPO}/.git`],
  [`${TREES}/worktree-calm-forest-4f96`]: [`${TREES}/worktree-calm-forest-4f96`, `${REPO}/.git`],
  [`${TREES}/worktree-sunny-field-529d`]: [`${TREES}/worktree-sunny-field-529d`, `${REPO}/.git`],
  [`${TREES}/worktree-quiet-lake-0a1b`]: [`${TREES}/worktree-quiet-lake-0a1b`, `${REPO}/.git`],
};
function git(command, args, options) {
  if (command !== 'git') return undefined;
  const checkout = CHECKOUTS[options.cwd];
  if (!checkout) return { code: 128, stderr: 'fatal: not a git repository' };
  if (args.includes('status')) return { stdout: '# branch.head main\n# branch.ab +0 -0\n' };
  if (args.includes('--show-toplevel')) return { stdout: `${checkout[0]}\n` };
  return { stdout: `${checkout[1]}\n` };
}

const STATUS = {
  version: 2,
  generated: '2026-11-10T07:00:00Z',
  hc: { running: true, error: null },
  daemon: { running: true, pid: 4242, installed: true },
  projects: [{
    name: 'weather', dir: REPO, state_dir: `${HOME}/.local/state/corgi/supervisors/weather`, base: 'main',
    supervisor: null, questions: [], inbox_pending: 0, backlog: 0, ledger: { needs_answer: 0, ready_to_merge: 0, dispatched: 0 },
    agents: [
      { name: 'w-cache', role: 'worker', state: 'working', task: 'Fix the cache\nso that it holds', model: 'Opus 5.5', ctx_pct: 33.4, terminal: 't3', worktree: `${TREES}/worktree-calm-forest-4f96`, cwd: `${TREES}/worktree-calm-forest-4f96` },
      { name: 'weather-supervisor', role: 'supervisor', state: 'done', task: 'Supervisor', terminal: 't2', worktree: null, cwd: REPO },
      // Started, and has reported no status yet: the app lists no agent for it.
      { name: 'w-docs', role: 'worker', state: 'unknown', task: 'Write the docs', terminal: 't5', worktree: `${TREES}/worktree-sunny-field-529d`, cwd: `${TREES}/worktree-sunny-field-529d` },
    ],
  }],
};

// corgi, as the mods run it: `answers` by its first argument, each the JSON
// it prints, a whole result ({ code, stdout, stderr }), or a function of
// its arguments giving either, or the promise of either.
function corgi(answers) {
  return (command, args) => {
    if (command !== CORGI) return undefined;
    const answer = answers[args[0]];
    if (answer === undefined) return { code: 2, stderr: `corgi: unknown command or option: ${args[0]}` };
    return Promise.resolve(typeof answer === 'function' ? answer(args) : answer).then((value) =>
      'stdout' in value || 'code' in value ? value : { stdout: JSON.stringify(value) });
  };
}

function window(answers = {}) {
  const w = world({
    spaces: [
      space('sp-corgi', 'Corgi', HOME, { named: true }),
      space('sp-weather', 'weather', REPO),
      space('sp-calm', 'worktree-calm-forest-4f96', `${TREES}/worktree-calm-forest-4f96`),
      space('sp-sunny', 'worktree-sunny-field-529d', `${TREES}/worktree-sunny-field-529d`),
      space('sp-quiet', 'worktree-quiet-lake-0a1b', `${TREES}/worktree-quiet-lake-0a1b`),
      space('sp-home', '~', HOME, { active: true }),
    ],
    panes: [
      pane('sp-weather', 't2', REPO, CLAUDE),
      pane('sp-weather', 't9', REPO, SHELL),
      pane('sp-calm', 't3', `${TREES}/worktree-calm-forest-4f96`, CLAUDE),
      pane('sp-calm', 't4', `${TREES}/worktree-calm-forest-4f96`, CLAUDE),
      pane('sp-sunny', 't5', `${TREES}/worktree-sunny-field-529d`, CLAUDE, { title: 'claude' }),
      pane('sp-quiet', 't6', `${TREES}/worktree-quiet-lake-0a1b`, SHELL),
      pane('sp-home', 't7', HOME, CLAUDE),
      pane('sp-home', 't8', HOME, SHELL, { focused: true }),
    ],
    agents: [
      agent('sp-weather', 't2', 'idle', 'Weather supervisor'),
      agent('sp-calm', 't3', 'working', '✳ Fixing the cache'),
      // Started by hand in the same worktree.
      agent('sp-calm', 't4', 'blocked', '✳ Review the cache fix'),
      agent('sp-home', 't7', 'done', '~/notes'),
    ],
    focused: { space: 'sp-home', tab: 'tab-sp-home', pane: 'pane-t8', terminal: 't8' },
    files: { [`${APP}/mods/corgi/config.json`]: JSON.stringify({ corgi: CORGI }) },
  });
  w.programs = [git, corgi({ status: STATUS, ...answers })];
  return w;
}

// The Projects list (the mod's default view) with git read for every space.
async function projects(w, props = {}, module = undefined) {
  const mod = await start(w, props, module);
  await mod.tick(250);
  return mod;
}

// --- the Projects list ---------------------------------------------------------------

{
  const w = window();
  const mod = await projects(w);
  const root = mod.render();
  const drawn = texts(root);
  // The herd card first, above the header.
  assert.equal(drawn[5], 'Projects');
  // Spaces outside a repository as Spaces shows them; a project with its
  // agents under it, the supervisor first, in place of its worktrees' rows;
  // a worktree with no agent keeps its row.
  assert.deepEqual(ids(root, 'space-').concat(), ['space-sp-corgi', 'space-sp-weather', 'space-sp-quiet', 'space-sp-home']);
  // Above the header, the herd card: the agents by state as the rows'
  // badges with a count, a state with none left out (no usage: this corgi
  // does not answer `usage`). An agent in no state counts as idle.
  assert.deepEqual(texts(byId(root, 'herd-card')), ['HERD', 'WORKING 1', 'BLOCKED 1', 'DONE 1', 'IDLE 2']);
  assert.deepEqual(ids(root, 'herd-'), ['herd-card', 'herd-badges', 'herd-working', 'herd-blocked', 'herd-done', 'herd-idle']);
  assertBadge(byId(root, 'herd-working'), 'working', 'WORKING 1');
  assertBadge(byId(root, 'herd-idle'), 'idle', 'IDLE 2');
  assert.equal(byId(root, 'herd-idle').calls.tooltip[0], '2 idle');
  // The badges wrap whole beside the label, and the line grows with them.
  const badges = byId(root, 'herd-badges');
  assert.ok('flex_wrap' in badges.calls && 'flex_1' in badges.calls, 'the badges wrap in the room the label leaves');
  assert.deepEqual(badges.calls.min_w, [0]);
  assert.ok(!('truncate' in byId(root, 'herd-working').calls), 'a badge is never cut');
  const herdLine = byId(root, 'herd');
  assert.deepEqual([herdLine.calls.min_h, herdLine.calls.h], [[20], undefined], 'a line high at least, taller as it wraps');
  // No spinner on the card or in a row.
  assert.equal(spinnersIn(byId(root, 'herd-card')), 0);
  for (const id of ids(root, 'agent-')) assert.equal(spinnersIn(byId(root, id)), 0, id);
  assert.deepEqual(
    drawn.slice(6),
    [
      'C', 'Corgi',
      // A project's row counts its agents by state: one working, one
      // blocked, two idle (the supervisor, and one in no state).
      'W', 'weather', '1', '1', '2',
      // The supervisor first, then by state: blocked, working, …
      'Supervisor', 'IDLE',
      'Review the cache fix', 'BLOCKED',
      'Fix the cache so that it holds', 'Opus 5.5', '·', '33%', 'WORKING',
      // … and last one in no state: no badge.
      'Write the docs',
      'worktree-quiet-lake-0a1b',
      '~', '~',
      // A title that is only a folder says nothing: the program's name.
      'Claude Code', 'DONE',
    ],
    drawn.join(' | '),
  );
  assert.deepEqual(ids(root, 'agent-'), ['agent-t2', 'agent-t4', 'agent-t3', 'agent-t5', 'agent-t7']);
  // agent.next and agent.prev follow the list.
  assert.deepEqual(w.agentOrder, ['t2', 't4', 't3', 't5', 't7']);

  // Two lines each. The first: the task alone, the whole width, cut with an
  // ellipsis.
  const row = byId(root, 'agent-t3');
  assert.equal(row.kind, 'v_flex');
  assert.equal(row.children.length, 3, 'two lines, and the edge');
  const [first, second] = row.children.map(kept);
  assert.equal(first.children.length, 1);
  const [task] = first.children.map(kept);
  assert.deepEqual(texts(task), ['Fix the cache so that it holds']);
  assert.ok('truncate' in task.calls, 'the task is cut with an ellipsis');
  assert.deepEqual(task.calls.min_w, [0], 'the task can be cut to nothing');
  assert.ok('flex_1' in task.calls, 'the task fills the line');
  // The second, dim and smaller: the harness's icon, the model and the
  // context fill, rounded, and the state's badge at the right edge.
  assert.deepEqual(second.calls.text_size, [11]);
  assert.deepEqual(second.calls.text_color, ['#dim']);
  const [pic, model, dot, ctx, gap, state] = second.children.map(kept);
  assert.equal(second.children.length, 6);
  assert.ok('flex_1' in gap.calls, 'the badge stands at the right edge');
  assert.equal(state, byId(root, 'badge-t3'));
  assertBadge(state, 'working', 'WORKING');
  assert.ok(!('loop' in state.calls), 'WORKING is still: no fade');
  assert.deepEqual(pic.args, ['icons/claude.svg']);
  assert.ok('flex_none' in pic.calls, 'the icon stays whole');
  assert.deepEqual(texts(model), ['Opus 5.5']);
  assert.ok('truncate' in model.calls);
  assert.deepEqual([texts(dot), texts(ctx)], [['·'], ['33%']]);
  // The worktree's name is no text of the row, only its tooltip.
  assert.deepEqual(row.calls.tooltip, ['worktree-calm-forest-4f96']);
  assert.ok(!texts(row).includes('worktree-calm-forest-4f96'));
  // What Corgi does not know is left out, with nothing in its place: the
  // badge alone on the second line.
  assert.deepEqual(byId(root, 'agent-t4').children.map((child) => texts(child)), [['Review the cache fix'], ['BLOCKED'], []], 'a blocked agent');
  assertBadge(byId(root, 'badge-t4'), 'blocked', 'BLOCKED');
  assertBadge(byId(root, 'badge-t2'), 'idle', 'IDLE');
  assertBadge(byId(root, 'badge-t7'), 'done', 'DONE');
  assert.equal(byId(root, 'badge-t5'), null, 'an agent in no state has no badge');
  assert.equal(kept(byId(root, 'agent-t4').children[1]).children.length, 3, 'only the icon, and the badge after its gap');
  const t5 = byId(root, 'agent-t5');
  assert.deepEqual(texts(t5), ['Write the docs']);
  assert.deepEqual(t5.calls.tooltip, ['worktree-sunny-field-529d']);
  // An agent whose program has no icon keeps the rest.
  w.agents[2].program = { ...CLAUDE, icon: null };
  assert.equal(kept(byId(mod.render(), 'agent-t4').children[1]).children.length, 2, 'the badge alone');
  w.agents[2].program = CLAUDE;
  // On the primary checkout there is no worktree to name.
  assert.deepEqual(texts(byId(root, 'agent-t2')), ['Supervisor', 'IDLE']);
  assert.ok(!('tooltip' in byId(root, 'agent-t2').calls));

  // A click shows the agent's terminal; a space's row shows the space.
  click(root, 'agent-t5', mod.cx);
  assert.deepEqual(w.ran.at(-1), ['agent.focus', { terminal: 't5' }]);
  click(root, 'space-sp-quiet', mod.cx, 'on_mouse_down:left');
  click(root, 'space-sp-quiet', mod.cx, 'on_mouse_up:left');
  assert.deepEqual(w.ran.at(-1), ['space.select', { id: 'sp-quiet' }]);
  // Right-click on an agent is its space's menu.
  click(root, 'agent-t3', mod.cx, 'on_mouse_down:right');
  assert.deepEqual(w.ran.at(-1)[1].props, { tab: 'space', target: { id: 'sp-calm' } });

  // Corgi is read every 5 seconds, and no more often; plan
  // usage each minute, as often as its cache is renewed.
  assert.deepEqual(mod.cx.timers.map((timer) => timer.ms).sort((a, b) => a - b), [250, 1000, 5000, 15000, 60000]);
  const reads = () => w.execs.filter((call) => call[1] === 'status').length;
  const before = reads();
  await mod.tick(5000);
  assert.equal(reads(), before + 1);
  await mod.tick(1000);
  assert.equal(reads(), before + 2, 'the first look at the live agents');
  await mod.tick(1000);
  assert.equal(reads(), before + 2, 'unchanged agents read nothing');
  w.agents[1].status = 'done';
  await mod.tick(1000);
  assert.equal(reads(), before + 3, 'a changed agent does');
}

// A context fill but no model: the fill alone, with no dot before it.
{
  const status = structuredClone(STATUS);
  status.projects[0].agents[0].model = null;
  const mod = await projects(window({ status }));
  assert.deepEqual(texts(kept(byId(mod.render(), 'agent-t3').children[1])), ['33%', 'WORKING']);
}

// The prompt cache, after the context fill: the time it stays warm, `cold`
// once it lapsed, a `?` when only estimated, nothing when unknown. The
// clock is the test's, so that the countdown between reads can be seen.
{
  const realNow = Date.now;
  let clock = Date.parse('2026-11-16T09:00:00Z');
  Date.now = () => clock;
  try {
    // The card's second line, and its cache figure, for `cache` (and other
    // fields of w-cache) as `corgi status --json` gives them.
    const card = async (cache, extra = {}) => {
      const status = structuredClone(STATUS);
      Object.assign(status.projects[0].agents[0], { cache, ...extra });
      const w = window({ status });
      const mod = await projects(w);
      // The first look at the live agents reads the status again: done here,
      // so that only the clock moves the figure after it.
      await mod.tick(1000);
      const line = () => {
        const root = mod.render();
        return { texts: texts(kept(byId(root, 'agent-t3').children[1])), figure: byId(root, 'cache-t3') };
      };
      return { w, mod, line };
    };
    const warm = (minutes, extra = {}) => ({ state: 'warm', minutes_left: minutes, tokens: 180000, estimated: false, ...extra });

    // Warm: the minutes left, whole, with what they are in its tooltip.
    let { mod, line } = await card(warm(42));
    let now = line();
    assert.deepEqual(now.texts, ['Opus 5.5', '·', '33%', '·', '42m', 'WORKING']);
    assert.ok('flex_none' in now.figure.calls, 'the figure stays whole; the model is cut first');
    assert.deepEqual(now.figure.calls.tooltip, ['Prompt cache warm for 42 more minutes']);
    assert.equal(now.figure.calls.text_color, undefined, 'dim, as the line, while there is time');

    // It counts down from the read, between reads: a minute later, 41m,
    // and the 1-second timer draws the card again for it.
    clock += 61_000;
    const drawn = mod.cx.notified;
    await mod.tick(1000);
    assert.ok(mod.cx.notified > drawn, 'a minute counted down draws the card again');
    assert.deepEqual(line().texts.slice(3, 5), ['·', '41m']);
    assert.deepEqual(line().figure.calls.tooltip, ['Prompt cache warm for 41 more minutes']);
    // Another second changes no figure, and draws nothing for it.
    const still = mod.cx.notified;
    clock += 1_000;
    await mod.tick(1000);
    assert.equal(mod.cx.notified, still, 'nothing to draw while the figure holds');
    // The next read says 42 again (a prompt renewed it): the count starts over.
    await mod.tick(5000);
    assert.deepEqual(line().texts[4], '42m');

    // Five minutes or less: amber, to catch it before it goes.
    ({ mod, line } = await card(warm(6)));
    assert.equal(line().texts[4], '6m');
    assert.equal(line().figure.calls.text_color, undefined);
    clock += 30_000;
    assert.equal(line().texts[4], '6m', 'minutes_left is rounded up: half a minute on, still within 6');
    clock += 31_000;
    assert.equal(line().texts[4], '5m');
    assert.deepEqual(line().figure.calls.text_color, ['#ansi3']);
    ({ line } = await card(warm(1)));
    assert.deepEqual([line().texts[4], line().figure.calls.text_color], ['1m', ['#ansi3']]);
    assert.deepEqual(line().figure.calls.tooltip, ['Prompt cache warm for 1 more minute']);
    // Counted down to nothing between reads: cold, with what it re-reads.
    clock += 60_000;
    assert.equal(line().texts[4], 'cold');
    assert.deepEqual(line().figure.calls.tooltip, ['Cache cold: the next prompt re-reads 180k tokens']);
    assert.equal(line().figure.calls.text_color, undefined, 'cold is dim, as the line');

    // An hour and more: hours.
    ({ line } = await card(warm(60)));
    assert.equal(line().texts[4], '1h');
    ({ line } = await card(warm(125)));
    assert.equal(line().texts[4], '2h');

    // Estimated (Codex): a `?`, warm or cold, as `corgi fleet` marks it.
    ({ line } = await card(warm(42, { estimated: true })));
    assert.equal(line().texts[4], '42m?');
    assert.deepEqual(line().figure.calls.tooltip, ['Prompt cache warm for at least 42 more minutes (estimated; it may last longer)']);
    ({ line } = await card({ state: 'cold', minutes_left: 0, tokens: 1_250_000, estimated: true }));
    assert.equal(line().texts[4], 'cold?');
    assert.deepEqual(line().figure.calls.tooltip, ['Cache likely cold: the next prompt re-reads 1.3M tokens']);

    // Cold.
    ({ line } = await card({ state: 'cold', minutes_left: 0, tokens: 179_200, estimated: false }));
    assert.deepEqual(line().texts, ['Opus 5.5', '·', '33%', '·', 'cold', 'WORKING']);
    assert.deepEqual(line().figure.calls.tooltip, ['Cache cold: the next prompt re-reads 180k tokens']);

    // Unknown, or no cache at all: nothing, and no dot for it.
    for (const cache of [{ state: 'unknown', minutes_left: null, tokens: null, estimated: false }, undefined]) {
      ({ line } = await card(cache));
      assert.deepEqual(line().texts, ['Opus 5.5', '·', '33%', 'WORKING']);
      assert.equal(line().figure, null);
    }

    // A dot only between two things shown.
    ({ line } = await card(warm(42), { ctx_pct: null }));
    assert.deepEqual(line().texts, ['Opus 5.5', '·', '42m', 'WORKING']);
    ({ line } = await card(warm(42), { ctx_pct: null, model: null }));
    assert.deepEqual(line().texts, ['42m', 'WORKING']);

    // A narrow sidebar: the model is cut first, the figure stays whole; under
    // 200 px, where the model would be cut to a letter or to nothing, the
    // line leaves it out and the icon's tooltip names it.
    let w;
    ({ w, mod, line } = await card(warm(42)));
    w.sidebar = 215;
    assert.deepEqual(line().texts, ['Opus 5.5', '·', '33%', '·', '42m', 'WORKING']);
    assert.equal(byId(mod.render(), 'model-t3'), null);
    w.sidebar = 190;
    assert.deepEqual(line().texts, ['33%', '·', '42m', 'WORKING']);
    const named = byId(mod.render(), 'model-t3');
    assert.deepEqual(named.calls.tooltip, ['Opus 5.5']);
    assert.deepEqual(kept(named.children[0]).args, ['icons/claude.svg'], 'the icon, with the model as its tooltip');
    // Without a cache figure the model keeps its place, cut as before.
    ({ w, line } = await card(undefined));
    w.sidebar = 190;
    assert.deepEqual(line().texts, ['Opus 5.5', '·', '33%', 'WORKING']);
  } finally {
    Date.now = realNow;
  }
}

// What is lit: only the agent with the focus, never a project's row with
// it; a project's row is not lit at all; a space outside any project is
// while none of its agents has the focus.
{
  const lit = (root) => [...ids(root, 'space-'), ...ids(root, 'agent-')].filter((id) => byId(root, id).calls.bg?.[0] === '#selected');
  const w = window();
  const mod = await projects(w);
  // A shell in ~, a space outside any project: its row.
  assert.deepEqual(lit(mod.render()), ['space-sp-home']);
  // An agent in ~: the agent alone.
  w.agents[3].focused = true;
  assert.deepEqual(lit(mod.render()), ['agent-t7']);
  w.agents[3].focused = false;
  // The supervisor, in the project's own space: its row, not the project's.
  w.spaces.at(-1).active = false;
  w.spaces[1].active = true;
  w.agents[0].focused = true;
  assert.deepEqual(lit(mod.render()), ['agent-t2']);
  // A plain shell in the project's space: nothing.
  w.agents[0].focused = false;
  assert.deepEqual(lit(mod.render()), []);
  // A worker, in its worktree's space: its row.
  w.spaces[1].active = false;
  w.spaces[2].active = true;
  w.agents[1].focused = true;
  assert.deepEqual(lit(mod.render()), ['agent-t3']);
  // A worktree's space with no agent keeps its own row, lit while shown.
  w.agents[1].focused = false;
  w.spaces[2].active = false;
  w.spaces[4].active = true;
  assert.deepEqual(lit(mod.render()), ['space-sp-quiet']);
}

// The space that is shown is lit: its own row, or the agent standing for it.
{
  const w = window();
  w.spaces.at(-1).active = false;
  w.spaces[2].active = true;
  const mod = await projects(w);
  const lit = (root, id) => byId(root, id).calls.bg[0] === '#selected';
  let root = mod.render();
  // The first as listed: t4, blocked, before t3, working.
  assert.ok(lit(root, 'agent-t4'), 'the first agent of the shown worktree');
  assert.ok(!lit(root, 'agent-t3'));
  w.agents[1].focused = true;
  root = mod.render();
  assert.ok(!lit(root, 'agent-t4'));
  assert.ok(lit(root, 'agent-t3'), 'the agent with the focus');
}

// Each agent is a faintly shaded card with its state's color as a bar at
// its left, past the card's rounding, so the corners never cut into it.
// The lit card has the selection's fill and an outline every card has,
// unseen until lit.
{
  const w = window();
  w.agents[1].focused = true;
  const mod = await projects(w);
  const root = mod.render();
  const card = byId(root, 'agent-t4');
  assert.deepEqual([card.calls.bg, card.calls.border_color], [['#foreground0d'], ['#foreground00']], 'shaded, its outline unseen');
  assert.ok('border_1' in card.calls && 'relative' in card.calls);
  const lit = byId(root, 'agent-t3');
  assert.deepEqual([lit.calls.bg, lit.calls.border_color], [['#selected'], ['#muted']], 'lit: the selection and an outline');
  assert.ok('border_1' in lit.calls, 'the same outline: lighting moves nothing');
  const radius = card.calls.rounded[0];
  for (const [id, color] of [['t2', '#muted'], ['t3', '#working'], ['t4', '#blocked'], ['t5', '#muted'], ['t7', '#done']]) {
    const edge = byId(root, `edge-${id}`);
    assert.deepEqual(edge.calls.bg, [color], `${id}: the edge in its state's color (no state: idle's)`);
    assert.deepEqual([edge.calls.top, edge.calls.bottom], [[0], [0]], `${id}: from top to bottom`);
    assert.ok('absolute' in edge.calls);
    // Inside a 1 px border, the rounding ends `radius` from the card's edge.
    assert.ok(edge.calls.left[0] + 1 >= radius, `${id}: past the corners' rounding`);
    assert.ok(byId(root, `agent-${id}`).calls.pl[0] > edge.calls.left[0] + edge.calls.w[0], `${id}: the text clear of the bar`);
  }
  // 2 px under the project's row, 4 between cards.
  assert.deepEqual([byId(root, 'agent-t2').calls.mt, card.calls.mt], [[2], [4]]);
  // A narrow sidebar takes in the cards' margins: the second line keeps its room.
  const wide = card.calls.ml[0] + card.calls.pl[0] + card.calls.pr[0];
  w.sidebar = 190;
  const narrow = byId(mod.render(), 'agent-t4').calls;
  assert.ok(narrow.ml[0] + narrow.pl[0] + narrow.pr[0] < wide, 'narrower margins at 190 px');
  assert.ok(narrow.ml[0] + narrow.pl[0] + narrow.pr[0] + 2 <= 30, 'no less room than the rows had before cards');
}

// A project's row counts its agents by state, the supervisor's too: a
// count in each state's badge color, in the HERD card's order, a state with
// none left out. A space outside any project has none.
{
  const w = window();
  w.agents[2].status = 'working';
  const mod = await projects(w);
  const root = mod.render();
  assert.deepEqual(ids(root, 'count-'), ['count-sp-weather-working', 'count-sp-weather-idle']);
  assertBadge(byId(root, 'count-sp-weather-working'), 'working', '2');
  assertBadge(byId(root, 'count-sp-weather-idle'), 'idle', '2');
  assert.equal(byId(root, 'count-sp-weather-working').calls.tooltip[0], '2 working');
  // Between the name and the ahead and behind arrows, at the right.
  assert.ok('flex_none' in byId(root, 'counts-sp-weather').calls, 'never cut');
  assert.deepEqual(ids(byId(root, 'space-sp-home'), 'count'), [], '~ is no project');
  // A worker tagged ready to merge counts as MERGE.
  const status = structuredClone(STATUS);
  status.projects[0].agents[0].tag = 'merge';
  const merging = await projects(window({ status }));
  assert.deepEqual(texts(byId(merging.render(), 'counts-sp-weather')), ['1', '1', '2']);
  assert.deepEqual(ids(merging.render(), 'count-'), ['count-sp-weather-blocked', 'count-sp-weather-merge', 'count-sp-weather-idle']);
}

// A click on a project's row folds it to its supervisor: its other agents,
// its worktrees' rows and their cards are not drawn, agent.next and
// agent.prev pass them by, and its counts stay. Another click unfolds it.
// Which projects are folded outlasts the app (localStorage). Its space is
// not shown, nor is its row lit; a space outside any project is shown by
// a click, as before.
{
  const stored = new Map();
  const before = globalThis.localStorage;
  globalThis.localStorage = { getItem: (k) => stored.get(k) ?? null, setItem: (k, v) => stored.set(k, String(v)), removeItem: (k) => stored.delete(k) };
  try {
    const w = window();
    const mod = await projects(w);
    let root = mod.render();
    const chevron = (root) => byId(root, 'fold-sp-weather').args[0];
    assert.equal(chevron(root), 'mods/workspace-chrome/chevron-down.svg');
    assert.equal(byId(root, 'fold-sp-home'), null, 'no chevron where nothing folds');
    // A failed pull of a worktree with no agent: its card, until folded.
    mod.view.pulls[JSON.stringify([null, `${TREES}/worktree-quiet-lake-0a1b`])] = { phase: 'failed', message: 'Couldn\'t reach origin' };
    root = mod.render();
    assert.ok(ids(root, 'pull-card-').length === 1);
    const ran = w.ran.length;
    click(root, 'space-sp-weather', mod.cx, 'on_mouse_down:left');
    click(root, 'space-sp-weather', mod.cx, 'on_mouse_up:left');
    assert.equal(w.ran.length, ran, 'the space is not shown');
    root = mod.render();
    assert.deepEqual(ids(root, 'agent-'), ['agent-t2', 'agent-t7'], 'the supervisor stays');
    assert.deepEqual(ids(root, 'space-'), ['space-sp-corgi', 'space-sp-weather', 'space-sp-home'], 'the worktree with no agent goes too');
    assert.deepEqual(ids(root, 'pull-card-'), [], 'and its card');
    assert.deepEqual(w.agentOrder, ['t2', 't7']);
    assert.equal(chevron(root), 'mods/workspace-chrome/chevron-right.svg');
    assert.deepEqual(texts(byId(root, 'counts-sp-weather')), ['1', '1', '2'], 'the counts stay');
    assert.ok(!('bg' in byId(root, 'space-sp-weather').calls), 'never lit');
    assert.deepEqual(JSON.parse(stored.get('projects.folded')), [JSON.stringify([null, `${REPO}/.git`])]);

    // Remembered: a new start draws it folded.
    const again = await projects(window());
    assert.deepEqual(ids(again.render(), 'agent-'), ['agent-t2', 'agent-t7']);
    globalThis.__world = w;

    // Unfolded, it is all back.
    click(root, 'space-sp-weather', mod.cx, 'on_mouse_down:left');
    click(root, 'space-sp-weather', mod.cx, 'on_mouse_up:left');
    root = mod.render();
    assert.deepEqual(ids(root, 'agent-'), ['agent-t2', 'agent-t4', 'agent-t3', 'agent-t5', 'agent-t7']);
    assert.deepEqual(ids(root, 'pull-card-').length, 1);
    assert.deepEqual(w.agentOrder, ['t2', 't4', 't3', 't5', 't7']);
    assert.deepEqual(JSON.parse(stored.get('projects.folded')), []);

    // A space outside any project is shown by a click.
    click(root, 'space-sp-corgi', mod.cx, 'on_mouse_down:left');
    click(root, 'space-sp-corgi', mod.cx, 'on_mouse_up:left');
    assert.deepEqual(w.ran.at(-1), ['space.select', { id: 'sp-corgi' }]);
  } finally {
    globalThis.localStorage = before;
  }
}

// Within a project, the supervisor first, then its agents by state:
// blocked, working, MERGE, done, idle, then the rest (here one in no state); by
// name within a state, case-insensitive, and by terminal when two names
// are the same. A row moves only when its agent's state changes, and keeps
// its place while its state is missing for a moment.
{
  const w = window();
  w.agents.push(
    agent('sp-calm', 't13', 'blocked', '✳ Zap the bug'),
    agent('sp-sunny', 't12', 'working', '✳ add tests'),
    agent('sp-quiet', 't14', 'done', '✳ Check the docs'),
    agent('sp-quiet', 't10', 'done', '✳ beta notes'),
    agent('sp-sunny', 't15', 'idle', '✳ alpha task'),
    agent('sp-calm', 't11', 'idle', '✳ Alpha task'),
  );
  const mod = await projects(w);
  const listed = () => ids(mod.render(), 'agent-').map((id) => id.slice('agent-'.length));
  const ORDER = [
    't2', // the supervisor (idle)
    't4', 't13', // blocked: Review the cache fix, Zap the bug
    't12', 't3', // working: add tests, Fix the cache so that it holds
    't10', 't14', // done: beta notes, Check the docs
    't11', 't15', // idle: Alpha task, alpha task (by terminal)
    't5', // in no state: Write the docs
    't7', // the next space's
  ];
  assert.deepEqual(listed(), ORDER);
  // agent.next and agent.prev follow the list.
  assert.deepEqual(w.agentOrder, ORDER);

  // Zap the bug starts working: it alone moves, among the working.
  w.agents.find((a) => a.address === 't13').status = 'working';
  const moved = ['t2', 't4', 't12', 't3', 't13', 't10', 't14', 't11', 't15', 't5', 't7'];
  assert.deepEqual(listed(), moved);
  assert.deepEqual(w.agentOrder, moved);
  const without = (list) => list.filter((a) => a !== 't13');
  assert.deepEqual(without(listed()), without(ORDER), 'the others keep their order');

  // add tests's state is missing for a refresh: it keeps its place.
  const tests = w.agents.find((a) => a.address === 't12');
  tests.status = null;
  assert.deepEqual(listed(), moved, 'a missing state keeps the row in place');
  tests.status = 'working';
  assert.deepEqual(listed(), moved);
  // Missing for longer: it goes with those in no state, by name.
  const realNow = Date.now;
  try {
    tests.status = null;
    assert.deepEqual(listed(), moved);
    const missingAt = realNow();
    Date.now = () => missingAt + 9000;
    assert.deepEqual(listed(), moved, 'kept for a few seconds');
    Date.now = () => missingAt + 11000;
    assert.deepEqual(listed(), ['t2', 't4', 't3', 't13', 't10', 't14', 't11', 't15', 't12', 't5', 't7'], 'then placed as in no state');
  } finally {
    Date.now = realNow;
  }
  tests.status = 'working';
  assert.deepEqual(listed(), moved, 'and back with its state');

  // Fix the cache is done: among the done, by name.
  w.agents.find((a) => a.address === 't3').status = 'done';
  const finished = ['t2', 't4', 't12', 't13', 't10', 't14', 't3', 't11', 't15', 't5', 't7'];
  assert.deepEqual(listed(), finished);
  // Tagged ready to merge: MERGE has its own place, between working and
  // done, so it moves up.
  const status = structuredClone(STATUS);
  Object.assign(status.projects[0].agents[0], { tag: 'merge', branch: 'worktree/calm-forest-4f96', state: 'done' });
  w.programs[1] = corgi({ status });
  for (const timer of mod.cx.timers.filter((timer) => timer.ms === 5000)) timer.tick(mod.cx);
  await mod.settle();
  assert.ok(byId(mod.render(), 'merge-t3'), 'its MERGE mark');
  const merging = ['t2', 't4', 't12', 't13', 't3', 't10', 't14', 't11', 't15', 't5', 't7'];
  assert.deepEqual(listed(), merging);
  assert.deepEqual(w.agentOrder, merging);

  // Folded: the supervisor alone, and agent.next and agent.prev skip the rest.
  let root = mod.render();
  click(root, 'space-sp-weather', mod.cx, 'on_mouse_down:left');
  click(root, 'space-sp-weather', mod.cx, 'on_mouse_up:left');
  assert.deepEqual(listed(), ['t2', 't7']);
  assert.deepEqual(w.agentOrder, ['t2', 't7']);
  root = mod.render();
  click(root, 'space-sp-weather', mod.cx, 'on_mouse_down:left');
  click(root, 'space-sp-weather', mod.cx, 'on_mouse_up:left');
  assert.deepEqual(listed(), merging);
  assert.deepEqual(w.agentOrder, merging);
}

// Each project (a repository's space) sits in one rounded area: its row,
// its agents' cards and its worktrees' rows, on a faint tint inside a soft
// outline, both in its avatar's color. A space outside any repository has
// none; a folded project's area holds its row and supervisor alone. A
// project with no stored color has a neutral area. A narrow sidebar gives
// the area the list's margin, so what is in it keeps its room.
{
  const w = window();
  w.shared['space.colors'] = { 'sp-weather': 'ansi4', 'sp-corgi': 'ansi1' };
  const mod = await projects(w);
  let root = mod.render();
  assert.deepEqual(ids(root, 'area-'), ['area-sp-weather'], 'only the repository');
  const area = byId(root, 'area-sp-weather');
  assert.deepEqual([area.calls.bg, area.calls.border_color], [['#5b8def12'], ['#5b8def59']], 'its avatar\'s blue');
  assert.ok('border_1' in area.calls && 'rounded' in area.calls);
  const avatar = kept(kept(byId(area, 'space-sp-weather').children.at(-1)).children[0]);
  assert.deepEqual([texts(avatar), avatar.calls.bg], [['W'], ['#5b8def']], 'the avatar in the same color');
  assert.deepEqual(ids(area, 'space-'), ['space-sp-weather', 'space-sp-quiet'], 'its row and its worktree\'s with no agent');
  assert.deepEqual(ids(area, 'agent-'), ['agent-t2', 'agent-t4', 'agent-t3', 'agent-t5']);
  assert.deepEqual(ids(root, 'space-'), ['space-sp-corgi', 'space-sp-weather', 'space-sp-quiet', 'space-sp-home'], 'the plain spaces stay in the list');
  // Folded: its row and supervisor.
  click(root, 'space-sp-weather', mod.cx, 'on_mouse_down:left');
  click(root, 'space-sp-weather', mod.cx, 'on_mouse_up:left');
  root = mod.render();
  assert.deepEqual(byId(root, 'area-sp-weather').children.map((c) => kept(c).id), ['space-sp-weather', 'agent-t2']);
  click(root, 'space-sp-weather', mod.cx, 'on_mouse_down:left');
  click(root, 'space-sp-weather', mod.cx, 'on_mouse_up:left');

  // A stored hex is used as it is, cut to its color.
  w.shared['space.colors'] = { ...w.shared['space.colors'], 'sp-weather': '#123456ff' };
  assert.deepEqual(byId(mod.render(), 'area-sp-weather').calls.border_color, ['#12345659']);

  // At 190 px, the list's margin and the area's outline and inset make
  // the margin there was without an area.
  const list = (root) => {
    let found = null;
    walk(root, (node) => { if ('overflow_y_scrollbar' in node.calls) found ??= node; });
    return found;
  };
  const wide = list(mod.render()).calls.px[0];
  w.sidebar = 190;
  root = mod.render();
  assert.equal(list(root).calls.px[0] + 1 + byId(root, 'area-sp-weather').calls.p[0], wide);
  assert.ok(byId(root, 'space-sp-weather').calls.px[0] < 8, 'and the row takes in its own');
}
{
  // `~` never gets a stored color: in a repository, its area is neutral.
  const w = window();
  w.spaces[1].name = '~';
  const mod = await projects(w);
  const area = byId(mod.render(), 'area-sp-weather');
  assert.equal(w.shared['space.colors']['sp-weather'], undefined);
  assert.deepEqual([area.calls.bg, area.calls.border_color], [['#foreground08'], ['#border']]);
}

// Without corgi the list still shows every space and agent, by its own title.
{
  const w = window();
  w.programs = [git];
  const mod = await projects(w);
  const root = mod.render();
  const drawn = texts(root);
  // The herd card counts the app's agents alone: with no usage, no plan lines.
  assert.deepEqual(texts(byId(root, 'herd-card')), ['HERD', 'WORKING 1', 'BLOCKED 1', 'DONE 1', 'IDLE 1']);
  assert.deepEqual(
    drawn.slice(8),
    // No supervisor without corgi: the agents by state alone.
    ['W', 'weather', '1', '1', '1', 'Review the cache fix', 'BLOCKED', 'Fixing the cache', 'WORKING', 'Weather supervisor', 'IDLE', 'worktree-sunny-field-529d', 'worktree-quiet-lake-0a1b', '~', '~', 'Claude Code', 'DONE'],
    drawn.join(' | '),
  );
}

// The rail: a cell per space, no texts but their numbers, and a way back.
{
  const w = window();
  w.sidebar = 76;
  w.spaces[2].status = 'working';
  const mod = await projects(w);
  let root = mod.render();
  assert.deepEqual(texts(root), ['1', '·', '2', '·', '3', '4', '·', '5', '·', '6', '·', '»']);
  // A working space's mark is the braille spinner: a rail cell has no room
  // for a badge.
  let railSpinner = null;
  walk(byId(root, 'space-sp-calm'), (node) => { if ('frames' in node.calls) railSpinner = node; });
  assertSpinner(railSpinner, 12, 'the rail');
  assert.deepEqual(ids(root, 'agent-'), []);
  assert.equal(byId(root, 'open-project'), null);
  click(root, 'space-sp-calm', mod.cx);
  assert.deepEqual(w.ran.at(-1), ['space.select', { id: 'sp-calm' }]);
  // The + unfolds the sidebar and opens the field.
  click(root, 'project-open', mod.cx);
  await mod.settle();
  assert.equal(w.sidebar, 268);
  root = mod.render();
  assert.ok(byId(root, 'open-project'), 'the field is open');
  // Unfolded, the herd card is back above the header.
  assert.deepEqual(texts(root).slice(0, 6), ['HERD', 'WORKING 1', 'BLOCKED 1', 'DONE 1', 'IDLE 2', 'Projects']);
  // And » alone unfolds it.
  w.sidebar = 76;
  click(mod.render(), 'rail-unfold', mod.cx);
  assert.equal(w.sidebar, 268);
}

// --- The herd and plan usage card ---------------------------------------------------

{
  const now = Math.floor(Date.now() / 1000);
  const usage = {
    version: 1,
    now,
    providers: [
      { provider: 'codex', title: 'CODEX', usage: null, fetched_at: null, error: 'codex app-server timed out' },
      {
        provider: 'claude', title: 'CLAUDE', fetched_at: now - 150, error: null,
        usage: { provider: 'claude', plan_type: 'Max', banked_resets: null, five_hour: { used_percent: 31, resets_at: now + 8070 }, week: { used_percent: 86, resets_at: now + 310_000 } },
      },
    ],
  };
  const w = window({ usage: () => usage });
  w.sidebar = 357;
  const mod = await projects(w);
  const usageRuns = () => w.execs.filter((call) => call[1] === 'usage');
  // Read once Corgi answers status, then each minute, never with status.
  assert.deepEqual(usageRuns().map((call) => call.slice(1)), [['usage', '--json']]);
  await mod.tick(5000);
  assert.equal(usageRuns().length, 1);
  await mod.tick(60000);
  assert.equal(usageRuns().length, 2);

  const root = mod.render();
  const card = byId(root, 'herd-card');
  // The first thing in the mod, above the header and the rows.
  const drawn = texts(root);
  assert.equal(drawn[0], 'HERD');
  assert.equal(drawn[texts(card).length], 'Projects');
  assert.ok(drawn.indexOf('weather') > drawn.indexOf('CLAUDE'), 'the card stands above the rows');
  assert.deepEqual(texts(card), [
    'HERD', 'WORKING 1', 'BLOCKED 1', 'DONE 1', 'IDLE 2',
    'CODEX', 'unavailable',
    'CLAUDE', '5h', '31%', '2h 14m', '7d', '86%', '3d 14h',
  ]);
  // The card's own colors are the theme's, whatever the agents' states (one
  // is blocked here): its border and every title dim.
  assert.equal(card.calls.border_color[0], '#border');
  const colorOf = (root, text) => {
    let found = null;
    walk(root, (node) => {
      if (found === null && node.children?.length === 1 && kept(node.children[0]) === text) found = node.calls.text_color?.[0] ?? null;
    });
    return found;
  };
  for (const title of ['HERD', 'CODEX', 'CLAUDE']) assert.equal(colorOf(card, title), '#dim', title);
  // The windows in color.
  assert.equal(colorOf(card, '31%'), '#ansi2');
  assert.equal(colorOf(card, '86%'), '#ansi1');
  // The plan and the reading's age are the line's tooltip; a failed reading's
  // line says why.
  assert.equal(byId(root, 'usage-claude').calls.tooltip[0], 'Max · read 2m ago');
  assert.equal(byId(root, 'usage-codex').calls.tooltip[0], 'codex app-server timed out');
  // A state's count says what it counts.
  assert.equal(byId(root, 'herd-blocked').calls.tooltip[0], '1 blocked');
  // At Herdr's own 268 px the reset times go whole, into the tooltip.
  w.sidebar = 268;
  const narrow = mod.render();
  assert.deepEqual(texts(byId(narrow, 'usage-claude')), ['CLAUDE', '5h', '31%', '7d', '86%']);
  assert.equal(byId(narrow, 'usage-claude').calls.tooltip[0], 'Max · read 2m ago · 5h resets in 2h 14m · 7d resets in 3d 14h');

  // A worker tagged ready to merge counts as MERGE, not as its state; the
  // middle window's yellow.
  usage.providers[1].usage.five_hour.used_percent = 64;
  const status = structuredClone(STATUS);
  status.projects[0].agents[0].tag = 'merge';
  const tagged = window({ status, usage: () => usage });
  const again = await projects(tagged);
  const card2 = byId(again.render(), 'herd-card');
  assert.deepEqual(ids(card2, 'herd-'), ['herd-card', 'herd-badges', 'herd-blocked', 'herd-merge', 'herd-done', 'herd-idle']);
  assertBadge(byId(card2, 'herd-merge'), 'merge', 'MERGE 1');
  assert.equal(byId(card2, 'herd-merge').calls.tooltip[0], '1 to merge');
  assert.equal(colorOf(card2, '64%'), '#ansi3');
  // Its row shows MERGE in place of WORKING: the merge mark, the one badge.
  const taggedRow = again.render();
  assert.equal(markOf(taggedRow, 'agent-t3'), 'MERGE');
  assert.equal(byId(taggedRow, 'badge-t3'), null);
  assertBadge(byId(taggedRow, 'merge-t3'), 'merge', 'MERGE');
  assert.ok('on_click' in byId(taggedRow, 'merge-t3').handlers, 'MERGE still opens the merge question');

  // The rail has no room for it.
  tagged.sidebar = 76;
  assert.equal(byId(again.render(), 'herd-card'), null);
}

// An agent Corgi knows exited: EXITED, an outline, in its row and in HERD;
// one paused and tagged ready to merge is PAUSED, as the desktop window
// says. Many states wrap whole badges.
{
  const status = structuredClone(STATUS);
  status.projects[0].agents[2].state = 'exited';
  const w = window({ status });
  const mod = await projects(w);
  const root = mod.render();
  assert.equal(markOf(root, 'agent-t5'), 'EXITED');
  assertBadge(byId(root, 'badge-t5'), 'exited', 'EXITED');
  assert.deepEqual(texts(byId(root, 'herd-card')), ['HERD', 'WORKING 1', 'BLOCKED 1', 'DONE 1', 'IDLE 1', 'EXITED 1']);
  assertBadge(byId(root, 'herd-exited'), 'exited', 'EXITED 1');
}

{
  // Neither agents nor plans: no card.
  const w = window({ usage: { version: 1, now: 0, providers: [] } });
  w.agents = [];
  w.panes = [];
  const status = structuredClone(STATUS);
  status.projects[0].agents = [];
  w.programs = [git, corgi({ status, usage: { version: 1, now: 0, providers: [] } })];
  const mod = await projects(w);
  assert.equal(byId(mod.render(), 'herd-card'), null);
}

// --- Pull, from a checkout's ↓N ----------------------------------------------------------

// A window whose primary checkout is 3 behind origin/main and whose
// worktree with no agent (quiet-lake) 2 behind origin/spike; `pull(cwd)`
// answers each pull, and is called only for one. `pulls` keeps each pull's
// folder and what ran. A pull is a shell that runs `git pull --ff-only`
// alone, with no prompt for a login.
const PULL_SH = /^ssh="\$\{GIT_SSH_COMMAND:-\$\(git config --get core\.sshCommand\)\}"; export GIT_TERMINAL_PROMPT=0 GIT_SSH_COMMAND="\$\{ssh:-ssh\} -o BatchMode=yes"; exec git pull --ff-only$/;
function behindWindow(pull) {
  const w = window();
  const behind = { [REPO]: 3, [`${TREES}/worktree-quiet-lake-0a1b`]: 2 };
  const upstream = { [REPO]: 'origin/main', [`${TREES}/worktree-quiet-lake-0a1b`]: 'origin/spike' };
  w.pulls = [];
  w.programs = [
    (command, args, options) => {
      if (command === 'sh') {
        assert.equal(args[0], '-c');
        assert.match(args[1], PULL_SH);
        w.pulls.push({ cwd: options.cwd, args: ['pull', '--ff-only'] });
        return pull(options.cwd);
      }
      if (command !== 'git') return undefined;
      if (args.includes('status') && behind[options.cwd] !== undefined) {
        return { stdout: `# branch.head ${options.cwd === REPO ? 'main' : 'spike'}\n# branch.upstream ${upstream[options.cwd]}\n# branch.ab +1 -${behind[options.cwd]}\n` };
      }
      return git(command, args, options);
    },
    corgi({ status: STATUS }),
  ];
  w.behind = behind;
  return w;
}
const REPO_KEY = JSON.stringify([null, REPO]);
const QUIET_KEY = JSON.stringify([null, `${TREES}/worktree-quiet-lake-0a1b`]);
// A click on a ↓N: the press and the release, both its own.
function pressPull(root, key, cx) {
  const before = cx.stopped;
  click(root, `pull-${key}`, cx, 'on_mouse_down:left');
  click(root, `pull-${key}`, cx, 'on_mouse_up:left');
  assert.equal(cx.stopped, before + 2, 'the row under the mark neither shows its space nor drags');
}

// Only a checkout that is behind has a ↓N to click, with its count and upstream.
{
  const w = behindWindow(() => ({}));
  const mod = await projects(w);
  const root = mod.render();
  assert.deepEqual(ids(root, 'pull-'), [`pull-${REPO_KEY}`, `pull-${QUIET_KEY}`]);
  const mark = byId(root, `pull-${REPO_KEY}`);
  assert.deepEqual(texts(mark), ['↓3']);
  assert.ok('cursor_pointer' in mark.calls);
  assert.deepEqual(mark.calls.tooltip, ['Pull 3 commits from origin/main']);
  assert.deepEqual(texts(byId(root, `space-sp-weather`)).slice(-2), ['↑1', '↓3'], 'the ahead mark stays as it is');
  // Up to date: no mark at all.
  const plain = await projects(window());
  assert.deepEqual(ids(plain.render(), 'pull-'), []);
}

// A click runs exactly `git pull --ff-only` in that row's checkout; done,
// git is read again, the ↓N goes and nothing else is shown.
{
  let finish;
  const w = behindWindow(() => new Promise((resolve) => { finish = resolve; }));
  const mod = await projects(w);
  pressPull(mod.render(), QUIET_KEY, mod.cx);
  assert.deepEqual(w.pulls, [{ cwd: `${TREES}/worktree-quiet-lake-0a1b`, args: ['pull', '--ff-only'] }]);
  // Pulling: dimmed, with a spinner, and no other click starts one.
  let root = mod.render();
  const mark = byId(root, `pull-${QUIET_KEY}`);
  assert.ok('opacity' in mark.calls);
  assert.ok(!('cursor_pointer' in mark.calls));
  assertSpinner(mark.children[0], 10, 'the pull mark');
  pressPull(root, QUIET_KEY, mod.cx);
  pressPull(root, REPO_KEY, mod.cx);
  assert.equal(w.pulls.length, 1, 'nothing runs while a pull is in flight');
  assert.deepEqual(byId(root, `pull-${REPO_KEY}`).calls.tooltip, ['Another pull is running']);
  w.behind[`${TREES}/worktree-quiet-lake-0a1b`] = 0;
  finish({ stdout: 'Fast-forward\n' });
  await mod.settle();
  root = mod.render();
  assert.deepEqual(ids(root, 'pull-'), [`pull-${REPO_KEY}`]);
  assert.deepEqual(ids(root, 'pull-card-'), []);
  // Only the pull was run: no merge, stash or reset, and nothing of the app.
  assert.deepEqual(w.execs.filter(([command, ...args]) => !(command === 'git' && (args.includes('status') || args.includes('rev-parse'))) && command !== CORGI).map(([command, flag]) => [command, flag]), [['sh', '-c']]);
  // The next pull may run.
  pressPull(root, REPO_KEY, mod.cx);
  assert.deepEqual(w.pulls.at(-1), { cwd: REPO, args: ['pull', '--ff-only'] });
}

// Each way a pull fails, said plainly in a card under the row, with Dismiss.
{
  const failures = [
    [
      { code: 128, stderr: 'hint: Diverging branches can\'t be fast-forwarded, you need to either:\nhint:\nhint: \tgit merge --no-ff\nfatal: Not possible to fast-forward, aborting.\n' },
      'Can\'t fast-forward: main and origin/main have diverged',
    ],
    [
      { code: 1, stdout: 'Updating 1234567..89abcde\n', stderr: 'error: Your local changes to the following files would be overwritten by merge:\n\tsrc/a.rs\n\tREADME.md\nPlease commit your changes or stash them before you merge.\nAborting\n' },
      'Local changes would be overwritten: src/a.rs, README.md',
    ],
    [
      { code: 1, stderr: 'error: The following untracked working tree files would be overwritten by merge:\n\tnotes.txt\nPlease move or remove them before you merge.\nAborting\n' },
      'Untracked files would be overwritten: notes.txt',
    ],
    [
      { code: 128, stderr: 'ssh: Could not resolve hostname gitlab.example.com: nodename nor servname provided\nfatal: Could not read from remote repository.\n\nPlease make sure you have the correct access rights\n' },
      'Couldn\'t reach origin: Could not resolve hostname gitlab.example.com: nodename nor servname provided',
    ],
    [Promise.reject(new Error('timed out after 120000 ms')), 'git pull did not finish: timed out after 120000 ms'],
    // A login it may not ask for: at once, not after the timeout.
    [{ code: 128, stderr: 'fatal: could not read Username for \'https://gitlab.example.com\': terminal prompts disabled\n' }, 'Needs a login: run git pull in a terminal once'],
    [{ code: 128, stderr: 'git@gitlab.example.com: Permission denied (publickey).\nfatal: Could not read from remote repository.\n' }, 'Needs a login: run git pull in a terminal once'],
    [{ code: 128, stderr: 'Host key verification failed.\nfatal: Could not read from remote repository.\n' }, 'Needs a login: run git pull in a terminal once'],
  ];
  for (const [answer, said] of failures) {
    const w = behindWindow(() => answer);
    const mod = await projects(w);
    pressPull(mod.render(), REPO_KEY, mod.cx);
    await mod.settle();
    let root = mod.render();
    assert.deepEqual(ids(root, 'pull-card-'), [`pull-card-${REPO_KEY}`]);
    assert.deepEqual(texts(byId(root, `pull-card-${REPO_KEY}`)), [said, 'Dismiss']);
    // Under the project's row, before its agents.
    const list = ids(root, '');
    assert.equal(list.indexOf(`pull-card-${REPO_KEY}`), list.indexOf(`pull-${REPO_KEY}`) + 1, list.join(' '));
    assert.ok(list.indexOf(`pull-card-${REPO_KEY}`) < list.indexOf('agent-t2'), list.join(' '));
    assert.ok(byId(root, `pull-${REPO_KEY}`), 'still behind: the mark stays, to try again');
    assert.equal(w.pulls.length, 1);
    const before = mod.cx.stopped;
    click(root, `pull-dismiss-${REPO_KEY}`, mod.cx);
    assert.equal(mod.cx.stopped, before + 1);
    root = mod.render();
    assert.deepEqual(ids(root, 'pull-card-'), []);
  }
}

// A worktree's row shows its card under itself; brought up to date some
// other way, the card goes by itself.
{
  const w = behindWindow(() => ({ code: 128, stderr: 'fatal: Not possible to fast-forward, aborting.\n' }));
  const mod = await projects(w);
  pressPull(mod.render(), QUIET_KEY, mod.cx);
  await mod.settle();
  let root = mod.render();
  assert.deepEqual(texts(byId(root, `pull-card-${QUIET_KEY}`)), ['Can\'t fast-forward: spike and origin/spike have diverged', 'Dismiss']);
  const list = ids(root, '');
  assert.ok(list.indexOf(`pull-card-${QUIET_KEY}`) > list.indexOf('space-sp-quiet'));
  w.behind[`${TREES}/worktree-quiet-lake-0a1b`] = 0;
  mod.view.refreshGit([QUIET_KEY], mod.cx);
  await mod.settle();
  root = mod.render();
  assert.deepEqual(ids(root, 'pull-'), [`pull-${REPO_KEY}`]);
}

// --- Open project ----------------------------------------------------------------------

const SUGGESTED = {
  query: '',
  parent: `${HOME}/repos`,
  projects: [
    { dir: REPO, name: 'weather', badge: '3 agents', new: false, supervisor: 't2' },
    { dir: `${HOME}/repos/zebra`, name: 'zebra', badge: 'nearby', new: false, supervisor: null },
    { dir: `${HOME}/repos/api`, name: 'api', badge: 'recent', new: false, supervisor: null },
  ],
};
// What `corgi projects` answers: by its --query.
const listing = (byQuery) => (args) => byQuery[args[args.indexOf('--query') + 1]] ?? { ...SUGGESTED, projects: [] };
const key = (root, cx, name) => byId(root, 'open-project').handlers.on_key_down({ key: name, modifiers: {} }, cx);
async function type(mod, text) {
  mod.view.input.text = text;
  mod.view.input.handlers.change({}, mod.cx);
  await mod.settle();
}
async function enter(mod) {
  mod.view.input.handlers.submit({}, mod.cx);
  await mod.settle();
}
const opens = (w) => w.execs.filter((call) => call[1] === 'open').map((call) => call.slice(1));

{
  const fresh = { dir: `${HOME}/repos/fresh`, name: 'fresh', badge: 'new project', new: true, supervisor: null };
  const answers = {
    projects: listing({
      '': SUGGESTED,
      ze: { ...SUGGESTED, projects: [SUGGESTED.projects[1], { dir: `${HOME}/repos/ze`, name: 'ze', badge: 'new project', new: true, supervisor: null }] },
      fresh: { ...SUGGESTED, projects: [fresh] },
      '~/re': { ...SUGGESTED, projects: [{ dir: `${HOME}/repos`, name: 'repos', badge: 'typed path', new: false, supervisor: null }] },
      '~/repos/': SUGGESTED,
      nothing: { ...SUGGESTED, projects: [] },
    }),
    open: (args) => (args[1].endsWith('/api')
      ? { code: 1, stdout: JSON.stringify({ result: 'failed', message: 'hc: t9 is blocked\n(permission)' }) }
      : { result: 'started', message: 'Started', project: args[1], name: 'x-supervisor', terminal: 't20', created: args.includes('--create'), place: 'shell' }),
  };
  const w = window(answers);
  const mod = await projects(w);
  assert.equal(byId(mod.render(), 'open-project'), null, 'closed until + is clicked');
  assert.deepEqual(byId(mod.render(), 'project-open').calls.tooltip, ['Open project']);

  click(mod.render(), 'project-open', mod.cx);
  await mod.settle();
  assert.ok(mod.view.input.focused > 0, 'the field takes the keyboard');
  // Corgi's known projects, with the folders the list's spaces are in.
  const asked = w.execs.filter((call) => call[1] === 'projects').at(-1);
  assert.deepEqual(asked.slice(1, 5), ['projects', '--json', '--query', '']);
  assert.deepEqual(asked.filter((_arg, n) => asked[n - 1] === '--open'), [HOME, REPO, `${TREES}/worktree-calm-forest-4f96`, `${TREES}/worktree-sunny-field-529d`, `${TREES}/worktree-quiet-lake-0a1b`]);
  let root = mod.render();
  assert.deepEqual(texts(byId(root, 'open-project')).slice(0, 9), ['weather', '~/repos/weather', 'running', 'zebra', '~/repos/zebra', 'nearby', 'api', '~/repos/api', 'recent']);
  assert.ok('bg' in byId(root, 'open-0').calls, 'the first is preset');
  // The keys wrap whole rather than be cut.
  const keys = byId(root, 'open-keys');
  assert.deepEqual(texts(keys), ['Enter opens', 'Tab completes', 'Esc closes']);
  assert.ok('flex_wrap' in keys.calls && !('truncate' in keys.calls));
  assert.ok(keys.children.map(kept).every((k) => 'whitespace_nowrap' in k.calls && 'flex_none' in k.calls));

  // Enter on a project whose supervisor runs shows it, and starts nothing.
  await enter(mod);
  assert.deepEqual(w.ran.at(-1), ['agent.focus', { terminal: 't2' }]);
  assert.deepEqual(opens(w), []);
  assert.equal(byId(mod.render(), 'open-project'), null, 'and the field is put away');

  // Typing narrows; the arrows move; Enter starts the supervisor, offering
  // the shells that wait: the focused space's first, then one in the project.
  click(mod.render(), 'project-open', mod.cx);
  await mod.settle();
  assert.equal(mod.view.input.value(), '', 'opened empty again');
  await type(mod, 'ze');
  root = mod.render();
  assert.deepEqual(texts(byId(root, 'open-project')).slice(0, 6), ['zebra', '~/repos/zebra', 'nearby', 'ze', '~/repos/ze', 'new project']);
  key(root, mod.cx, 'down');
  assert.ok('bg' in byId(mod.render(), 'open-1').calls);
  key(mod.render(), mod.cx, 'down');
  assert.ok('bg' in byId(mod.render(), 'open-0').calls, 'around to the first');
  await enter(mod);
  assert.deepEqual(opens(w), [['open', `${HOME}/repos/zebra`, '--json', '--terminal', 't8']]);
  assert.deepEqual(w.ran.at(-1), ['agent.focus', { terminal: 't20' }]);
  assert.equal(byId(mod.render(), 'open-project'), null);

  // A shell that waits in the project itself is offered after the focused
  // space's: in another space, only when it is in that folder.
  w.panes.push(pane('sp-weather', 't10', `${HOME}/repos/zebra`, SHELL));
  click(mod.render(), 'project-open', mod.cx);
  await type(mod, 'ze');
  await enter(mod);
  assert.deepEqual(opens(w).at(-1), ['open', `${HOME}/repos/zebra`, '--json', '--terminal', 't8', '--terminal', 't10']);

  // A new name is made beside the others.
  click(mod.render(), 'project-open', mod.cx);
  await type(mod, 'fresh');
  assert.deepEqual(texts(byId(mod.render(), 'open-project')).slice(0, 3), ['fresh', '~/repos/fresh', 'new project']);
  await enter(mod);
  assert.deepEqual(opens(w).at(-1).slice(0, 4), ['open', `${HOME}/repos/fresh`, '--json', '--create']);

  // Tab completes a path, and its directories come next.
  click(mod.render(), 'project-open', mod.cx);
  await type(mod, '~/re');
  key(mod.render(), mod.cx, 'tab');
  await mod.settle();
  assert.equal(mod.view.input.value(), '~/repos/');
  assert.deepEqual(w.execs.filter((call) => call[1] === 'projects').at(-1).slice(3, 5), ['--query', '~/repos/']);
  assert.equal(texts(byId(mod.render(), 'open-project'))[0], 'weather');

  // A failure shows in the field, which stays open with what was typed.
  await type(mod, '');
  key(mod.render(), mod.cx, 'up');
  const ran = w.ran.length;
  await enter(mod);
  root = mod.render();
  assert.ok(byId(root, 'open-project'), 'still open');
  assert.ok(texts(root).includes('hc: t9 is blocked (permission)'), texts(root).join(' | '));
  assert.equal(w.ran.length, ran, 'and nothing is shown');
  // Typing again clears it.
  await type(mod, 'nothing');
  root = mod.render();
  assert.ok(!texts(root).includes('hc: t9 is blocked (permission)'));
  assert.ok(texts(root).includes('No project or folder by that name.'));
  await enter(mod);
  assert.ok(texts(mod.render()).some((text) => text.startsWith('Nothing to open by that name')));

  // Esc puts the field away; a plain space is one click from it.
  key(mod.render(), mod.cx, 'escape');
  assert.equal(byId(mod.render(), 'open-project'), null);
  click(mod.render(), 'project-open', mod.cx);
  await mod.settle();
  click(mod.render(), 'open-plain-space', mod.cx);
  assert.deepEqual(w.ran.at(-1), ['space.new', undefined]);
  assert.equal(byId(mod.render(), 'open-project'), null);
}

// `corgi.open`, as a key of the person's sends it, is the +.
{
  const w = window({ projects: SUGGESTED });
  w.commands.push('open');
  const mod = await projects(w);
  assert.ok(byId(mod.render(), 'open-project'), 'the field is open');
  assert.equal(texts(byId(mod.render(), 'open-project'))[0], 'weather');
}

// While a supervisor starts the field says so, and a second Enter starts nothing.
{
  let finish;
  const w = window({
    projects: { ...SUGGESTED, projects: [SUGGESTED.projects[1]] },
    open: () => new Promise((resolve) => { finish = resolve; }),
  });
  const mod = await projects(w);
  click(mod.render(), 'project-open', mod.cx);
  await mod.settle();
  mod.view.input.handlers.submit({}, mod.cx);
  await new Promise((resolve) => setImmediate(resolve));
  assert.ok(texts(mod.render()).includes("Starting zebra's supervisor…"));
  spinnerBefore(mod.render(), "Starting zebra's supervisor…");
  mod.view.input.handlers.submit({}, mod.cx);
  await new Promise((resolve) => setImmediate(resolve));
  assert.equal(opens(w).length, 1);
  finish({ result: 'started', terminal: 't30' });
  await mod.settle();
  assert.deepEqual(w.ran.at(-1), ['agent.focus', { terminal: 't30' }]);
}

// A corgi from before `corgi projects` says so instead of listing nothing.
{
  const w = window();
  const mod = await projects(w);
  click(mod.render(), 'project-open', mod.cx);
  await mod.settle();
  const said = texts(mod.render()).find((text) => text.startsWith('This corgi is older than the Projects list'));
  assert.ok(said, 'says so');
  // It names the installer, not the retired Corgi space.
  assert.ok(said.includes('herdr-app/install.sh') && !said.includes('Corgi space'), said);
}

// With no corgi at all the field says how to get one.
{
  const w = window();
  w.programs = [git];
  const mod = await projects(w);
  click(mod.render(), 'project-open', mod.cx);
  await mod.settle();
  const said = texts(mod.render()).find((text) => text.startsWith('The corgi command was not found'));
  assert.ok(said && said.includes('herdr-app/install.sh') && !said.includes('Corgi space'), texts(mod.render()).join(' | '));
}

// --- Merge, on a worker tagged ready to merge ----------------------------------------------

const BRANCH = 'worktree/calm-forest-4f96';
// The window's status with w-cache tagged MERGE, and a supervisor in `state`
// (null: none runs).
function tagged(state = 'idle') {
  const status = structuredClone(STATUS);
  const [project] = status.projects;
  Object.assign(project.agents[0], { tag: 'merge', branch: BRANCH, state: 'done' });
  project.agents[2].branch = 'worktree/sunny-field-529d';
  if (state) {
    project.agents[1].state = state;
    project.supervisor = project.agents[1];
  } else project.agents.splice(1, 1);
  return status;
}
const merges = (w) => w.execs.filter((call) => call[1] === 'merge').map((call) => call.slice(2));
const panel = (mod) => byId(mod.render(), 'merge-panel-t3');
// Clicks the mark and confirms: the merge has run, with `outcome`.
async function merged(outcome, state) {
  const w = window({ status: tagged(state), merge: (args) => (args.includes('--ask-supervisor') ? { result: 'asked', message: 'Asked weather-supervisor\nto fix the merge of w-cache' } : outcome) });
  const mod = await projects(w);
  click(mod.render(), 'merge-t3', mod.cx);
  click(mod.render(), 'merge-confirm-t3', mod.cx);
  await mod.settle();
  return { w, mod };
}

{
  let finish;
  const w = window({ status: tagged(), merge: () => new Promise((resolve) => { finish = resolve; }) });
  const mod = await projects(w);
  let root = mod.render();
  // The tagged worker's row has the mark at the right of its second line,
  // after a spacer; no other row has one.
  assert.deepEqual(ids(root, 'merge-'), ['merge-t3']);
  const row = byId(root, 'agent-t3');
  assert.deepEqual(row.children.map((child) => texts(child)), [['Fix the cache so that it holds'], ['Opus 5.5', '·', '33%', 'MERGE'], []]);
  const line = kept(row.children[1]).children.map(kept);
  const [spacer, mark] = line.slice(-2);
  assert.ok('flex_1' in spacer.calls, 'the mark is pushed to the right edge');
  assert.ok('flex_none' in mark.calls, 'the mark stays whole');
  assert.deepEqual(mark.calls.tooltip, [`Merge ${BRANCH} into main and push`]);
  assert.deepEqual(texts(byId(root, 'agent-t5')), ['Write the docs'], 'an untagged worker has none');
  assert.equal(panel(mod), null);

  // A click on it asks under the row, and is not also the row's click.
  const ran = w.ran.length;
  click(root, 'merge-t3', mod.cx);
  assert.equal(mod.cx.stopped, 1);
  assert.equal(w.ran.length, ran, 'the agent is not shown');
  root = mod.render();
  assert.deepEqual(texts(panel(mod)), [`Merge ${BRANCH} into main and push?`, 'Cancel', 'Merge + push']);
  // The question stands right under its worker's row.
  const list = kept(byId(root, 'agent-t3'));
  let siblings;
  walk(root, (node) => {
    if ((node.children ?? []).some((child) => kept(child) === list)) siblings = node.children.map((child) => kept(child).id);
  });
  assert.equal(siblings[siblings.indexOf('agent-t3') + 1], 'merge-panel-t3');

  // Cancel puts it away, and nothing was merged; so does the mark again.
  click(root, 'merge-cancel-t3', mod.cx);
  assert.equal(panel(mod), null);
  click(mod.render(), 'merge-t3', mod.cx);
  assert.ok(panel(mod));
  click(mod.render(), 'merge-t3', mod.cx);
  assert.equal(panel(mod), null);
  assert.deepEqual(merges(w), []);

  // Confirmed, it runs the merge, and says so.
  click(mod.render(), 'merge-t3', mod.cx);
  click(mod.render(), 'merge-confirm-t3', mod.cx);
  await new Promise((resolve) => setImmediate(resolve));
  assert.deepEqual(merges(w), [['w-cache', '--project', REPO, '--json']]);
  assert.deepEqual(texts(panel(mod)), ['Merging…']);
  spinnerBefore(panel(mod), 'Merging…');
  // While it runs, a second click starts nothing and puts nothing away.
  click(mod.render(), 'merge-t3', mod.cx);
  assert.deepEqual(texts(panel(mod)), ['Merging…']);
  assert.equal(merges(w).length, 1);

  // Merged: the result, and the status is read again (the tag is gone).
  const reads = w.execs.filter((call) => call[1] === 'status').length;
  w.programs[1] = corgi({ status: STATUS, merge: () => new Promise((resolve) => { finish = resolve; }) });
  finish({ result: 'merged', message: `Merged and pushed ${BRANCH}\ninto main`, commit: 'abc1234', reason: null, error: null });
  await mod.settle();
  // Merged and pushed: closing the worker is offered, Close the default.
  assert.deepEqual(texts(panel(mod)), ['w-cache: merged', `Merged and pushed ${BRANCH} into main`, 'Close w-cache and remove its worktree?', 'Keep', 'Close']);
  // Green, not the amber of an agent that works.
  assert.deepEqual(panel(mod).calls.border_color, ['#ansi2']);
  assert.equal(w.execs.filter((call) => call[1] === 'status').length, reads + 1);
  assert.deepEqual(ids(mod.render(), 'merge-t'), [], 'no mark is left');
  // Keep: the card goes at once, with nothing to dismiss, and nothing is
  // closed.
  click(mod.render(), 'close-keep-t3', mod.cx);
  assert.equal(panel(mod), null);
  assert.equal(mod.view.merge, null);
  assert.deepEqual(w.execs.filter((call) => call[1] === 'close'), []);
  assert.ok(byId(mod.render(), 'agent-t3'), 'the worker stays');
}

// A conflict: `corgi merge` aborted it and handed it to the supervisor
// itself, so there is nothing left to ask, nor for the person to do: handed
// over, in blue, not an error.
{
  const message = `Merging ${BRANCH} into main stopped on conflicts in a.txt; aborted it and asked weather-supervisor to have w-cache merge main into its branch`;
  const { mod } = await merged({ result: 'conflicts', message, commit: null, reason: null, error: null, supervisor: 'weather-supervisor' });
  assert.deepEqual(texts(panel(mod)), ['w-cache: conflicts, handed to weather-supervisor', message, 'Dismiss']);
  assert.deepEqual(panel(mod).calls.border_color, ['#ansi4']);
}

// A conflict with no supervisor to hand it to stays in the checkout for the
// person to resolve: red.
{
  const message = `Merging ${BRANCH} into main stopped on conflicts in a.txt; resolve them in ${REPO} and run git merge --continue, or git merge --abort`;
  const { mod } = await merged({ result: 'conflicts', message, commit: null, reason: null, error: null, supervisor: null }, null);
  assert.deepEqual(texts(panel(mod)), ['w-cache: conflicts', message, 'Dismiss']);
  assert.deepEqual(panel(mod).calls.border_color, ['#blocked']);
}

// A failed push: the branch is merged; a running supervisor can be asked
// about the push, with the commit the merge landed at.
{
  const outcome = { result: 'push-failed', message: `Merged ${BRANCH} into main, but the push failed: main is protected`, commit: 'abc1234', reason: null, error: 'main is protected' };
  const { w, mod } = await merged(outcome);
  assert.deepEqual(texts(panel(mod)), ['w-cache: push-failed', outcome.message, 'Ask supervisor to fix', 'Dismiss']);
  assert.deepEqual(panel(mod).calls.border_color, ['#ansi3'], 'merged, but not pushed');
  click(mod.render(), 'merge-ask-t3', mod.cx);
  await mod.settle();
  assert.deepEqual(merges(w).at(-1), ['w-cache', '--project', REPO, '--json', '--ask-supervisor', 'main is protected', '--commit', 'abc1234']);
  assert.deepEqual(texts(panel(mod)), ['Asked weather-supervisor to fix the merge of w-cache', 'Dismiss']);
  assert.deepEqual(panel(mod).calls.border_color, ['#ansi2']);
  click(mod.render(), 'merge-dismiss-t3', mod.cx);
  assert.equal(panel(mod), null);
}

// After a merge that went through, the worker is closed at a click on
// Close, or with Enter, which Close holds: `corgi close`, and a close that
// went through leaves no card. While it runs, the card stands under the
// project once the worker's row is gone.
const closes = (w) => w.execs.filter((call) => call[1] === 'close').map((call) => call.slice(2));
const MERGED = { result: 'merged', message: `Merged and pushed ${BRANCH} into main`, commit: 'abc1234', reason: null, error: null, supervisor: null };
const WORKTREE = `${TREES}/worktree-calm-forest-4f96`;
const CLOSED = { result: 'closed', message: `Closed w-cache and removed its worktree ${WORKTREE};\nbranch ${BRANCH} stays`, reason: null, dirty: [], worktree: WORKTREE, branch: BRANCH, removed: true };
// The status once w-cache is closed: its row and its space are gone.
function closedWindow(w) {
  const status = tagged();
  status.projects[0].agents.splice(0, 1);
  w.programs[1] = corgi({ status, 'app-layout': { result: 'unchanged' } });
  w.agents = w.agents.filter((a) => a.address !== 't3');
}
{
  let finish;
  const { w, mod } = await merged(MERGED);
  // Close holds the keyboard: given it when the question came, and again
  // once it is drawn.
  assert.ok(mod.view.closeFocus.focused >= 2, `focused ${mod.view.closeFocus.focused} times`);
  const close = byId(mod.render(), 'close-confirm-t3');
  assert.ok(close.calls.track_focus[0] === mod.view.closeFocus, 'Close is the element the keyboard means');
  assert.ok('on_key_down' in close.handlers);
  // Another key does nothing; Enter closes.
  w.programs[1] = corgi({ status: tagged(), close: () => new Promise((resolve) => { finish = resolve; }) });
  close.handlers.on_key_down({ key: 'a', modifiers: {} }, mod.cx);
  assert.deepEqual(closes(w), []);
  byId(mod.render(), 'close-confirm-t3').handlers.on_key_down({ key: 'enter', modifiers: {} }, mod.cx);
  await new Promise((resolve) => setImmediate(resolve));
  assert.deepEqual(closes(w), [['w-cache', '--project', REPO, '--json']]);
  assert.deepEqual(texts(panel(mod)), ['w-cache: merged', MERGED.message, 'Closing…']);
  spinnerBefore(panel(mod), 'Closing…');
  // Another worker's MERGE mark does nothing meanwhile.
  const other = tagged();
  Object.assign(other.projects[0].agents[2], { tag: 'merge', state: 'done' });
  w.programs[1] = corgi({ status: other });
  // The timers fire, without waiting for the close, which is still running.
  const refresh = async () => {
    for (const timer of mod.cx.timers.filter((timer) => timer.ms === 5000)) timer.tick(mod.cx);
    for (let n = 0; n < 5; n++) await new Promise((resolve) => setImmediate(resolve));
  };
  await refresh();
  assert.ok(byId(mod.render(), 'merge-t5'), 'another worker tagged');
  click(mod.render(), 'merge-t5', mod.cx);
  assert.deepEqual(texts(panel(mod)), ['w-cache: merged', MERGED.message, 'Closing…']);
  // A refresh meanwhile, which sees the worker gone, keeps the card, under
  // its project's agents now that the worker's row is gone.
  closedWindow(w);
  await refresh();
  const root = mod.render();
  assert.equal(byId(root, 'agent-t3'), null);
  assert.deepEqual(texts(panel(mod)), ['w-cache: merged', MERGED.message, 'Closing…']);
  let siblings;
  walk(root, (node) => {
    if ((node.children ?? []).some((child) => kept(child)?.id === 'merge-panel-t3')) siblings = node.children.map((child) => kept(child).id);
  });
  assert.equal(siblings[siblings.indexOf('merge-panel-t3') - 1], 'agent-t5', "under the project, after its last agent");
  // Closed: the card is gone at once, with nothing to dismiss.
  finish(CLOSED);
  await mod.settle();
  assert.equal(panel(mod), null);
  assert.equal(mod.view.merge, null);
  await mod.tick(5000);
  assert.equal(panel(mod), null);
}

// A close that went through before the worker's row is gone leaves no card
// either, and the status is read again.
{
  const { w, mod } = await merged(MERGED);
  w.programs[1] = corgi({ status: tagged(), close: CLOSED });
  const reads = w.execs.filter((call) => call[1] === 'status').length;
  click(mod.render(), 'close-confirm-t3', mod.cx);
  await mod.settle();
  assert.equal(panel(mod), null);
  assert.ok(byId(mod.render(), 'agent-t3'), 'the row goes when the status says so');
  assert.ok(w.execs.filter((call) => call[1] === 'status').length > reads, 'the status is read again');
}

// A worktree with changes is refused: what is uncommitted is said, and
// "Close anyway" closes with --force; Keep leaves both.
{
  const DIRTY = { result: 'refused', reason: 'dirty', message: 'The worktree of w-cache has uncommitted or untracked changes: M cache.rs, ?? notes.txt', dirty: ['M cache.rs', '?? notes.txt'], worktree: WORKTREE, branch: BRANCH, removed: false };
  for (const answer of ['anyway', 'keep']) {
    const { w, mod } = await merged(MERGED);
    w.programs[1] = corgi({ status: tagged(), close: (args) => (args.includes('--force') ? CLOSED : { code: 1, stdout: JSON.stringify(DIRTY) }) });
    click(mod.render(), 'close-confirm-t3', mod.cx);
    await mod.settle();
    assert.deepEqual(texts(panel(mod)), ['w-cache: merged', MERGED.message, DIRTY.message, 'Close w-cache anyway and discard them?', 'Keep', 'Close anyway']);
    assert.deepEqual(panel(mod).calls.border_color, ['#blocked'], 'the person decides');
    assert.deepEqual(closes(w), [['w-cache', '--project', REPO, '--json']]);
    if (answer === 'keep') {
      click(mod.render(), 'close-keep-t3', mod.cx);
      assert.equal(panel(mod), null, 'kept: no card');
      assert.equal(closes(w).length, 1);
      continue;
    }
    click(mod.render(), 'close-force-t3', mod.cx);
    await mod.settle();
    assert.deepEqual(closes(w).at(-1), ['w-cache', '--project', REPO, '--json', '--force']);
    assert.equal(panel(mod), null, 'closed anyway: no card');
  }
}

// Escape on Close is Keep: no card.
{
  const { w, mod } = await merged(MERGED);
  byId(mod.render(), 'close-confirm-t3').handlers.on_key_down({ key: 'escape', modifiers: {} }, mod.cx);
  assert.equal(panel(mod), null);
  assert.deepEqual(closes(w), []);
}

// A close that failed is said in the card, which stays under the worker.
{
  const { w, mod } = await merged(MERGED);
  const failed = { result: 'failed', reason: null, message: 'Close of w-cache failed: hc: refused', dirty: [], worktree: WORKTREE, branch: BRANCH, removed: false };
  w.programs[1] = corgi({ status: tagged(), close: { code: 1, stdout: JSON.stringify(failed) } });
  click(mod.render(), 'close-confirm-t3', mod.cx);
  await mod.settle();
  assert.deepEqual(texts(panel(mod)), ['w-cache: merged', MERGED.message, failed.message, 'Dismiss']);
  assert.deepEqual(panel(mod).calls.border_color, ['#blocked']);
}

// Only a merge that was merged and pushed offers the close: not conflicts,
// a refusal or a failed push.
for (const outcome of [
  { result: 'conflicts', message: 'stopped on conflicts in a.txt', commit: null, reason: null, error: null, supervisor: 'weather-supervisor' },
  { result: 'refused', message: 'origin/main has 2 commit(s) main lacks', commit: null, reason: 'behind', error: null },
  { result: 'push-failed', message: 'Merged, but the push failed: main is protected', commit: 'abc1234', reason: null, error: 'main is protected' },
]) {
  const { mod } = await merged(outcome);
  assert.equal(byId(mod.render(), 'close-confirm-t3'), null, outcome.result);
  assert.ok(!texts(panel(mod)).some((text) => text.startsWith('Close')), outcome.result);
}

// Refused because the upstream moved on: pulled first, at a click.
{
  const outcome = { result: 'refused', message: 'origin/main has 2 commit(s) main lacks: pull them in first, then merge', commit: null, reason: 'behind', error: null };
  const { w, mod } = await merged(outcome);
  assert.deepEqual(texts(panel(mod)), ['w-cache: refused', outcome.message, 'Pull, merge + push', 'Dismiss']);
  click(mod.render(), 'merge-retry-t3', mod.cx);
  await mod.settle();
  assert.deepEqual(merges(w).at(-1), ['w-cache', '--project', REPO, '--json', '--pull']);
}

// A failed fetch: merged without it at a click, or handed to the supervisor.
{
  const outcome = { result: 'refused', message: 'Could not fetch origin/main: no route. Fix the fetch and merge again, or merge without fetching', commit: null, reason: 'fetch-failed', error: 'could not fetch origin/main: no route' };
  const { w, mod } = await merged(outcome);
  assert.deepEqual(texts(panel(mod)), ['w-cache: refused', outcome.message, 'Merge + push without fetching', 'Ask supervisor to fix', 'Dismiss']);
  click(mod.render(), 'merge-retry-t3', mod.cx);
  await mod.settle();
  assert.deepEqual(merges(w).at(-1), ['w-cache', '--project', REPO, '--json', '--skip-upstream']);
}

// Refused before it started (a dirty primary checkout): "Ask supervisor to
// fix" only while the project's supervisor runs.
{
  const outcome = { result: 'refused', message: 'Merge unavailable: the primary checkout has uncommitted changes', commit: null, reason: 'unavailable', error: 'the primary checkout has uncommitted changes' };
  for (const [state, offered] of [['idle', true], ['working', true], ['exited', false], [null, false]]) {
    const { w, mod } = await merged(outcome, state);
    assert.deepEqual(texts(panel(mod)), ['w-cache: refused', outcome.message, ...(offered ? ['Ask supervisor to fix'] : []), 'Dismiss'], `supervisor ${state}`);
    assert.equal(byId(mod.render(), 'merge-ask-t3') !== null, offered);
    if (state !== 'idle') continue;
    // Asked: the line that hands it over, with the failure alone.
    let answer;
    w.programs[1] = corgi({ status: tagged(), merge: () => new Promise((resolve) => { answer = resolve; }) });
    click(mod.render(), 'merge-ask-t3', mod.cx);
    await new Promise((resolve) => setImmediate(resolve));
    assert.deepEqual(merges(w).at(-1), ['w-cache', '--project', REPO, '--json', '--ask-supervisor', 'the primary checkout has uncommitted changes']);
    assert.deepEqual(texts(panel(mod)), ['w-cache: refused', outcome.message, 'Asking the supervisor…']);
    // The supervisor stopped meanwhile: said, and the result stays.
    answer({ code: 1, stdout: JSON.stringify({ result: 'refused', message: `No supervisor of ${REPO} is running to ask`, reason: 'no-supervisor' }) });
    await mod.settle();
    assert.deepEqual(texts(panel(mod)), ['w-cache: refused', outcome.message, `No supervisor of ${REPO} is running to ask`, 'Ask supervisor to fix', 'Dismiss']);
  }
}

// Nothing a supervisor could fix (no such worker, a corgi that names no
// error): no button, even with a supervisor running.
{
  const { mod } = await merged({ result: 'refused', message: `No worker w-cache runs in ${REPO}`, commit: null, reason: 'no-agent', error: null });
  assert.deepEqual(texts(panel(mod)), ['w-cache: refused', `No worker w-cache runs in ${REPO}`, 'Dismiss']);
}

// A merge that printed no JSON, or did not answer, is said as it is.
{
  const { mod } = await merged({ code: 2, stderr: 'corgi: unknown command\nor option: merge' });
  assert.deepEqual(texts(panel(mod)), ['w-cache: refused', 'corgi: unknown command or option: merge', 'Dismiss']);
  const w = window({ status: tagged() });
  w.programs.push((command, args) => (args[0] === 'merge' ? Promise.reject(new Error('timed out')) : undefined));
  w.programs.splice(1, 1, (command, args) => (args[0] === 'merge' ? undefined : corgi({ status: tagged() })(command, args)));
  const late = await projects(w);
  click(late.render(), 'merge-t3', late.cx);
  click(late.render(), 'merge-confirm-t3', late.cx);
  await late.settle();
  assert.deepEqual(texts(panel(late)), ['w-cache: refused', 'The merge did not answer: Error: timed out', 'Dismiss']);
}

// A question about a worker that lost its tag meanwhile is put away.
{
  const w = window({ status: tagged() });
  const mod = await projects(w);
  click(mod.render(), 'merge-t3', mod.cx);
  assert.ok(panel(mod));
  w.programs[1] = corgi({ status: STATUS });
  await mod.tick(5000);
  assert.equal(panel(mod), null);
  assert.deepEqual(ids(mod.render(), 'merge-'), []);
}

// --- Which Corgi runs ------------------------------------------------------------------

{
  const COMMIT = '5b5089578d010404248f957f59845a3e75224ee9';
  const build = { version: '0.2.3', commit: COMMIT, dirty: false };
  const line = async (corgiBuild, daemon = {}) => {
    const status = { ...STATUS, daemon: { ...STATUS.daemon, build, ...daemon } };
    if (corgiBuild) status.corgi = { ...build, source: `${HOME}/repos/corgi`, newer: null, ...corgiBuild };
    const mod = await projects(window({ status }));
    const shown = byId(mod.render(), 'corgi-version');
    return shown && texts(shown);
  };
  // The corgi that answers, built from what its checkout holds, with a
  // daemon of the same build: one dim line, last in the list.
  assert.deepEqual(await line({}), ['Corgi 0.2.3 · 5b50895']);
  const mod = await projects(window({ status: { ...STATUS, daemon: { ...STATUS.daemon, build }, corgi: { ...build, source: `${HOME}/repos/corgi`, newer: null } } }));
  assert.equal(texts(mod.render()).at(-1), 'Corgi 0.2.3 · 5b50895');
  assert.deepEqual(await line({ dirty: true }, { build: { ...build, dirty: true } }), ['Corgi 0.2.3 · 5b50895 · modified']);
  // A release built without Git names no commit.
  assert.deepEqual(await line({ commit: null, source: null }, { build: { ...build, commit: null } }), ['Corgi 0.2.3']);

  // A branch merged into its checkout since: said, with what to run there.
  assert.deepEqual(
    await line({ newer: { branch: 'main', commits: 2, commit: '79f57e6aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa' } }),
    ['Corgi 0.2.3 · 5b50895', '· main is newer', 'cd ~/repos/corgi && CORGI_BUILD=source herdr-app/install.sh'],
  );
  assert.deepEqual(
    await line({ newer: { branch: null, commits: 1, commit: COMMIT } }),
    ['Corgi 0.2.3 · 5b50895', '· its checkout is newer', 'cd ~/repos/corgi && CORGI_BUILD=source herdr-app/install.sh'],
  );

  // A daemon left running another build: said, with what restarts it.
  assert.deepEqual(
    await line({}, { build: { ...build, commit: '4563c06bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb' } }),
    ['Corgi 0.2.3 · 5b50895', 'The daemon runs another build: 4563c06', 'corgi daemon install'],
  );
  assert.deepEqual(await line({}, { build: { ...build, dirty: true } }), ['Corgi 0.2.3 · 5b50895', 'The daemon runs another build: 5b50895 · modified', 'corgi daemon install']);
  assert.deepEqual(await line({}, { build: { version: '0.2.2', commit: null, dirty: false } }), ['Corgi 0.2.3 · 5b50895', 'The daemon runs another build: Corgi 0.2.2', 'corgi daemon install']);
  // One from before daemons said what they were built from.
  assert.deepEqual(await line({}, { build: null }), ['Corgi 0.2.3 · 5b50895', 'The daemon runs an older build', 'corgi daemon install']);
  // Both at once.
  assert.deepEqual(
    await line({ newer: { branch: 'main', commits: 1, commit: COMMIT } }, { build: null }),
    ['Corgi 0.2.3 · 5b50895', '· main is newer', 'cd ~/repos/corgi && CORGI_BUILD=source herdr-app/install.sh', 'The daemon runs an older build', 'corgi daemon install'],
  );
  // A daemon that does not run wakes no supervisor: said, with what starts
  // it, whether its service is installed or not, and once only.
  assert.deepEqual(
    await line({}, { running: false, pid: null, build: null, installed: false }),
    ['Corgi 0.2.3 · 5b50895', "Corgi daemon not running: supervisors won't be woken or handed over", 'corgi daemon install'],
  );
  assert.deepEqual(
    await line({ newer: { branch: 'main', commits: 1, commit: COMMIT } }, { running: false, pid: null, build: null, installed: true }),
    ['Corgi 0.2.3 · 5b50895', '· main is newer', 'cd ~/repos/corgi && CORGI_BUILD=source herdr-app/install.sh', "Corgi daemon installed but not running: supervisors won't be woken or handed over", 'corgi daemon install'],
  );
  // A corgi too old to say what it is still says that its daemon is down.
  assert.deepEqual(await line(null, { running: false, pid: null, installed: true }), ["Corgi daemon installed but not running: supervisors won't be woken or handed over", 'corgi daemon install']);
  {
    const down = await projects(window({ status: { ...STATUS, daemon: { running: false, pid: null, installed: false } } }));
    const warning = kept(byId(down.render(), 'corgi-version').children[0]);
    assert.deepEqual(warning.calls.text_color, ['#blocked'], 'in the color of what needs the person');
  }

  // A corgi from before it said what it was built from, its daemon
  // running: no line; nor on the rail.
  assert.equal(await line(null), null);
  const w = window({ status: { ...STATUS, corgi: { ...build, source: null, newer: null } } });
  w.sidebar = 76;
  const rail = await projects(w);
  assert.equal(byId(rail.render(), 'corgi-version'), null);
}

// --- the brand row: "Herdr + Corgi" at the sidebar's top --------------------------------

// The images and icons under `el`, by path.
function pictures(el) {
  const out = [];
  walk(el, (node) => {
    if (node.kind === 'image' || node.kind === 'svg') out.push(node.args[0]);
  });
  return out;
}

// At the sidebar's width: both logos and names, the `+` dim between them,
// the two names alike, Corgi's cut first, and the shipped row's buttons.
{
  const w = world();
  const mod = await start(w, { view: 'brand' });
  const root = mod.render();
  assert.deepEqual(texts(root), ['Herdr', '+', 'Corgi']);
  assert.deepEqual(pictures(root), ['frame/welcome/herdr.png', 'mods/corgi/corgi.png', 'mods/corgi/search.svg', 'mods/corgi/sidebar.svg']);
  assert.deepEqual(kept(root).calls.pl, [76], 'room for the traffic lights');
  const herdr = byId(root, 'brand-herdr');
  const corgi = byId(root, 'brand-corgi');
  for (const name of [herdr, corgi]) {
    assert.ok(name.calls.font_semibold && name.calls.truncate, 'a name is semibold and cut with …');
    assert.deepEqual(name.calls.text_color, ['#foreground']);
    assert.deepEqual(name.calls.min_w, [0]);
  }
  assert.ok(corgi.calls.flex_shrink[0] >= 1000 * herdr.calls.flex_shrink[0], 'the word Corgi gives way before Herdr');
  assert.ok(byId(root, 'brand-names').calls.overflow_hidden, 'nothing spills under the buttons');
  // The logos and the "+" are never cut: only the words shrink.
  walk(root, (node) => {
    if (node.kind === 'image' || node.id === 'brand-plus') assert.ok(node.calls.flex_none, `${node.id ?? node.args[0]} keeps its width`);
  });
  assert.deepEqual(byId(root, 'brand-plus').calls.text_color, ['#dim']);
  assert.ok(!byId(root, 'brand-plus').calls.font_semibold);
  walk(root, (node) => {
    if (node.kind === 'image') assert.deepEqual([node.calls.size, node.calls.rounded], [[20], [5]], node.args[0]);
  });
  // Too narrow for both logos whole beside the traffic lights: Herdr's alone.
  w.sidebar = 210;
  const tight = mod.render();
  assert.deepEqual(texts(tight), []);
  assert.deepEqual(pictures(tight), ['frame/welcome/herdr.png', 'mods/corgi/search.svg', 'mods/corgi/sidebar.svg']);
  w.sidebar = 220;
  assert.deepEqual(texts(mod.render()), ['Herdr', '+', 'Corgi'], 'room for both logos: the words are cut by the window');
  w.sidebar = 268;

  // The buttons do what the shipped row's do.
  click(root, 'search', mod.cx);
  assert.deepEqual(w.ran.at(-1), ['mod.open', { name: 'picker' }]);
  assert.ok(byId(root, 'search').handlers['on_mouse_down:left'], 'a press on a button does not drag the window');
  click(root, 'sidebar-toggle', mod.cx);
  assert.deepEqual(w.ran.at(-1), ['layout.resize', { id: 'sidebar', px: 76 }]);
  assert.equal(w.shared['sidebar.width'], 268);
  assert.equal(w.shared['sidebar.collapsed'], true);

  // The macOS rail: the traffic lights fill it, as in the shipped row.
  const rail = mod.render();
  assert.deepEqual(texts(rail), []);
  assert.deepEqual(pictures(rail), []);
  assert.deepEqual(kept(rail).calls.pl, [6]);

  // Elsewhere the rail keeps the toggle, which unfolds it to the width it had.
  w.api.window = () => ({ platform: 'linux', active: true, fullscreen: false });
  w.sidebar = 44;
  const other = mod.render();
  assert.deepEqual(texts(other), []);
  assert.deepEqual(pictures(other), ['mods/corgi/sidebar.svg']);
  click(other, 'sidebar-toggle', mod.cx);
  assert.deepEqual(w.ran.at(-1), ['layout.resize', { id: 'sidebar', px: 268 }]);
  assert.deepEqual(kept(mod.render()).calls.pl, [14], 'no traffic lights off macOS');
  // Without them, the narrowest sidebar that is no rail has room for both logos.
  w.sidebar = 160;
  assert.deepEqual(texts(mod.render()), ['Herdr', '+', 'Corgi']);
}

// --- Pause and resume: the header's "⏸ Agents" ----------------------------------------------

// The window's status from a corgi that knows a pause: none in force, or
// w-docs (its terminal still shown) and w-radar (its terminal ended with
// its agent) paused.
function pauseStatus(any) {
  const status = structuredClone(STATUS);
  status.paused = { any, agents: [] };
  if (any) {
    const project = status.projects[0];
    Object.assign(project.agents[2], { state: 'paused', paused: true });
    project.agents.push({ name: 'w-radar', role: 'worker', state: 'paused', paused: true, task: 'Fix the radar', terminal: 't20', worktree: `${TREES}/worktree-radar`, cwd: `${TREES}/worktree-radar` });
    status.paused.agents = [{ name: 'w-docs', terminal: 't5' }, { name: 'w-radar', terminal: 't20' }];
  }
  return status;
}
const pauses = (w) => w.execs.filter((call) => call[1] === 'pause' || call[1] === 'resume').map((call) => call.slice(1));
// The button as it reads: its icon (⏸ or ▶) and its text.
const ICONS = { 'mods/corgi/pause.svg': '⏸', 'mods/corgi/play.svg': '▶' };
const header = (root) => {
  const button = byId(root, 'agents-pause');
  if (!button) return null;
  const icon = kept(button.children[0]);
  return [`${ICONS[icon.args[0]]} ${texts(button).join('')}`];
};
// The state mark at the right of an agent's first line.

// No button from a corgi that knows no pause.
{
  const mod = await projects(window());
  assert.equal(byId(mod.render(), 'agents-pause'), null);
}

// "⏸ Agents" pauses the working agents at once, shows its progress, and
// flips to "▶ Agents".
{
  let finish;
  const w = window({ status: pauseStatus(false), pause: () => new Promise((resolve) => { finish = resolve; }) });
  const mod = await projects(w);
  let root = mod.render();
  assert.deepEqual(header(root), ['⏸ Agents']);
  const button = byId(root, 'agents-pause');
  assert.match(button.calls.tooltip[0], /^Pause the agents that are working, before you go offline/);
  // Left of the +.
  let siblings = null;
  walk(root, (node) => {
    if ((node.children ?? []).some((child) => kept(child)?.id === 'project-open')) siblings = node.children.map((child) => kept(child).id);
  });
  assert.deepEqual(siblings, ['agents-pause', 'project-open']);

  // w-cache works (the window says so): no question, the pause runs.
  click(root, 'agents-pause', mod.cx);
  assert.deepEqual(pauses(w), [['pause', '--json']]);
  assert.deepEqual(texts(byId(mod.render(), 'agents-pause-panel')), ['Pausing 1 agent…']);
  spinnerBefore(byId(mod.render(), 'agents-pause-panel'), 'Pausing 1 agent…');
  // A second click while it runs does nothing.
  click(mod.render(), 'agents-pause', mod.cx);
  assert.equal(pauses(w).length, 1);

  w.programs[1] = corgi({ status: pauseStatus(true) });
  finish({ result: 'paused', message: 'Paused 1 agent', paused: ['w-cache'], failed: [], left: [] });
  await mod.settle();
  root = mod.render();
  assert.equal(byId(root, 'agents-pause-panel'), null, 'nothing more on success');
  assert.deepEqual(header(root), ['▶ Agents']);
  assert.deepEqual(w.toasts, []);
}

// With no agent working the click changes nothing: a toast says so.
{
  const w = window({ status: pauseStatus(false) });
  w.agents = w.agents.map((a) => ({ ...a, status: a.status === 'working' ? 'done' : a.status }));
  const status = pauseStatus(false);
  for (const a of status.projects[0].agents) if (a.state === 'working') a.state = 'done';
  w.programs[1] = corgi({ status });
  const mod = await projects(w);
  click(mod.render(), 'agents-pause', mod.cx);
  await mod.settle();
  assert.deepEqual(pauses(w), [], 'corgi pause is not even run');
  assert.deepEqual(w.toasts, [{ title: 'No agents are working', body: 'Nothing was paused.', level: 'info', timeout: 3000 }]);
  const root = mod.render();
  assert.deepEqual(header(root), ['⏸ Agents']);
  assert.equal(byId(root, 'agents-pause-panel'), null, 'no card');

  // Corgi found none working after all (they finished meanwhile): the same.
  const raced = window({ status: pauseStatus(false), pause: { result: 'nothing', message: 'No agent is working', paused: [], failed: [], left: [] } });
  const other = await projects(raced);
  click(other.render(), 'agents-pause', other.cx);
  await other.settle();
  assert.deepEqual(pauses(raced), [['pause', '--json']]);
  assert.equal(raced.toasts.length, 1);
  assert.equal(byId(other.render(), 'agents-pause-panel'), null);
  assert.deepEqual(header(other.render()), ['⏸ Agents']);
}

// While agents are paused: "▶ Agents", a ⏸ in their rows (one whose
// terminal ended stands under its project), and the click resumes them.
{
  const w = window({ status: pauseStatus(true), resume: { result: 'resumed', message: 'Resumed 2 agents', resumed: ['w-docs', 'w-radar'], failed: [], left: [] } });
  const mod = await projects(w);
  const root = mod.render();
  assert.deepEqual(header(root), ['▶ Agents']);
  assert.match(byId(root, 'agents-pause').calls.tooltip[0], /^Resume the 2 paused agents on their own conversation/);
  // Drawn as the + beside it, paused or not: no fill or colour of its own;
  // only the icon tells paused apart.
  for (const id of ['agents-pause', 'project-open']) {
    const b = byId(root, id);
    assert.ok(!('bg' in b.calls) && !('text_color' in b.calls) && !('border_1' in b.calls), `${id}: ${Object.keys(b.calls)}`);
    assert.deepEqual([b.calls.h ?? b.calls.size, b.calls.rounded], [[26], [6]]);
  }
  assert.deepEqual(kept(byId(root, 'agents-pause').children[0]).calls.size, [14]);
  // The paused last in their project, by name: Fix the radar, Write the docs.
  assert.deepEqual(ids(root, 'agent-'), ['agent-t2', 'agent-t4', 'agent-t3', 'agent-t20', 'agent-t5', 'agent-t7']);
  assert.deepEqual(texts(kept(byId(root, 'agent-t5').children[0])), ['Write the docs']);
  assert.equal(markOf(root, 'agent-t5'), 'PAUSED');
  assertBadge(byId(root, 'badge-t5'), 'paused', 'PAUSED');
  assert.deepEqual(texts(kept(byId(root, 'agent-t20').children[0])), ['Fix the radar']);
  assert.equal(markOf(root, 'agent-t20'), 'PAUSED');
  assert.deepEqual(texts(kept(byId(root, 'agent-t20').children[1])), ['PAUSED'], 'its terminal ended: no program or model to show, the badge alone');
  assert.equal(markOf(root, 'agent-t3'), 'WORKING', 'a working row keeps its badge');
  // HERD counts them as paused, not idle.
  assert.equal(byId(root, 'herd-paused').calls.tooltip[0], '2 paused');

  click(root, 'agents-pause', mod.cx);
  assert.deepEqual(texts(byId(mod.render(), 'agents-pause-panel')), ['Resuming 2 agents…']);
  w.programs[1] = corgi({ status: pauseStatus(false) });
  await mod.settle();
  assert.deepEqual(pauses(w), [['resume', '--json']]);
  assert.equal(byId(mod.render(), 'agents-pause-panel'), null);
  assert.deepEqual(header(mod.render()), ['⏸ Agents']);
}

// An agent that did not stop, or did not come back, is listed in a card
// with Dismiss.
{
  const partial = { result: 'partial', message: 'Paused 1 agent', paused: ['w-cache'], failed: [{ name: 'w-docs', why: 'did not exit; left running' }], left: [{ name: 'w-codex', why: 'has no resume line, so Corgi could not bring it back' }] };
  const w = window({ status: pauseStatus(false), pause: partial });
  const mod = await projects(w);
  click(mod.render(), 'agents-pause', mod.cx);
  w.programs[1] = corgi({ status: pauseStatus(true) });
  await mod.settle();
  const card = byId(mod.render(), 'agents-pause-panel');
  assert.deepEqual(texts(card), ['Not paused:', 'w-docs: did not exit; left running', 'w-codex: has no resume line, so Corgi could not bring it back', 'Dismiss']);
  assert.deepEqual(card.calls.border_color, ['#blocked']);
  assert.deepEqual(header(mod.render()), ['▶ Agents'], 'the one that stopped is paused');
  click(mod.render(), 'agents-pause-dismiss', mod.cx);
  assert.equal(byId(mod.render(), 'agents-pause-panel'), null);

  const back = { result: 'partial', message: 'Resumed 1 agent', resumed: ['w-docs'], failed: [{ name: 'w-radar', why: 'did not come back: its agent exited at once (terminal t21)' }], left: [] };
  const r = window({ status: pauseStatus(true), resume: back });
  const again = await projects(r);
  click(again.render(), 'agents-pause', again.cx);
  await again.settle();
  assert.deepEqual(texts(byId(again.render(), 'agents-pause-panel')), ['Did not come back:', 'w-radar: did not come back: its agent exited at once (terminal t21)', 'Dismiss']);

  // A refusal (hc not running) says why.
  const refused = window({ status: pauseStatus(false), pause: { code: 1, stdout: JSON.stringify({ result: 'refused', message: 'herdr-core (hc) is not running; Corgi does not start it for a pause', paused: [], failed: [], left: [] }) } });
  const third = await projects(refused);
  click(third.render(), 'agents-pause', third.cx);
  await third.settle();
  assert.deepEqual(texts(byId(third.render(), 'agents-pause-panel')), ['Nothing was paused', 'herdr-core (hc) is not running; Corgi does not start it for a pause', 'Dismiss']);
  assert.deepEqual(header(third.render()), ['⏸ Agents']);
}

// The rail has no room for it.
{
  const w = window({ status: pauseStatus(true) });
  w.sidebar = 76;
  const mod = await projects(w);
  assert.equal(byId(mod.render(), 'agents-pause'), null);
}

// --- Another machine's agents: its own Corgi, through herdr-core ----------------------------

const LAB = 'lab';
const LAB_CORGI = '/home/jo/.local/bin/corgi';
const API_TREE = `${TREES}/worktree-calm-forest-4f96`;
// lab's Corgi: a project in the same folder as this computer's weather
// (two machines may each have one there), with lab's own terminals t2 and
// t3, w-api tagged to merge into develop. `paused`: w-api paused, and
// w-radar too, whose terminal ended with its agent.
function labStatus({ working = false, paused = false } = {}) {
  const supervisor = { name: 'weather-supervisor', role: 'supervisor', state: 'idle', task: 'Supervisor', model: 'Opus 5.5', ctx_pct: 10, terminal: 't2', worktree: null, cwd: REPO };
  const api = { name: 'w-api', role: 'worker', state: working ? 'working' : 'done', task: 'Serve the forecast', model: 'Sonnet 5.5', ctx_pct: 50.2, terminal: 't3', tag: working || paused ? null : 'merge', branch: 'worktree/api', worktree: API_TREE, cwd: API_TREE };
  const agents = [supervisor, api];
  if (paused) {
    Object.assign(api, { state: 'paused', paused: true });
    agents.push({ name: 'w-radar', role: 'worker', state: 'paused', paused: true, task: 'Fix the radar', terminal: 't20', worktree: `${TREES}/worktree-radar`, cwd: `${TREES}/worktree-radar` });
  }
  return {
    ...structuredClone(STATUS),
    paused: { any: paused, agents: paused ? [{ name: 'w-api', terminal: 't3' }, { name: 'w-radar', terminal: 't20' }] : [] },
    projects: [{ ...structuredClone(STATUS.projects[0]), base: 'develop', supervisor, agents }],
  };
}
// This computer's window with lab added and connected: lab's weather space
// and its worktree's, with lab's supervisor and w-api in them. `located`:
// what the look for corgi on lab answers (a function of nothing, or a
// result); `answers`: lab's corgi, as `corgi()` takes them; `local`: this
// computer's; `pull`: lab's checkout is 2 behind and a pull there answers this.
function labWindow({ located = { stdout: `${LAB_CORGI}\n` }, answers = {}, local = {}, pull = null } = {}) {
  const w = window(local);
  w.machineList = [{ name: LAB, state: 'connected', target: 'jo@lab' }];
  w.spaces.push(
    space('lab/sp-weather', 'weather', REPO, { machine: LAB }),
    space('lab/sp-calm', 'worktree-calm-forest-4f96', API_TREE, { machine: LAB }),
  );
  w.panes.push(pane('lab/sp-weather', 'lab/t2', REPO, CLAUDE), pane('lab/sp-calm', 'lab/t3', API_TREE, CLAUDE));
  // lab's supervisor never titled its terminal: the app gives it its address.
  w.agents.push(agent('lab/sp-weather', 'lab/t2', 'idle', 'lab/t2'), agent('lab/sp-calm', 'lab/t3', 'done', '✳ Serving the forecast'));
  w.lab = { answers };
  w.labPulls = [];
  w.programs.unshift((command, args, options) => {
    if (options.on !== LAB) return undefined;
    if (command === 'sh' && /command -v corgi/.test(args[1])) return typeof located === 'function' ? located() : located;
    if (command === 'sh' && pull) {
      assert.match(args[1], PULL_SH);
      w.labPulls.push(options);
      return pull;
    }
    if (pull && command === 'git' && args.includes('status') && options.cwd === REPO) {
      return { stdout: '# branch.head main\n# branch.upstream origin/main\n# branch.ab +0 -2\n' };
    }
    if (command === LAB_CORGI) return corgi({ status: labStatus(), ...w.lab.answers })(CORGI, args);
    return undefined;
  });
  return w;
}
// What ran on lab, and this computer's corgi: [program, args, options].
const onLab = (w) => w.calls.filter((c) => c.options.on === LAB && c.command !== 'git').map((c) => [c.command, c.args, c.options]);
const labRuns = (w, verb) => onLab(w).filter(([command, args]) => command === LAB_CORGI && args[0] === verb);
const localRuns = (w, verb) => w.calls.filter((c) => c.command === CORGI && c.args[0] === verb).map((c) => [c.args, c.options]);

// lab's agents get their second line and MERGE from lab's own Corgi, found
// and read through herdr-core; this computer's keep theirs, though the
// terminals and the folder have the same names.
{
  const w = labWindow();
  const mod = await projects(w);
  const root = mod.render();
  assert.deepEqual(byId(root, 'agent-lab/t3').children.map((child) => texts(child)), [['Serve the forecast'], ['Sonnet 5.5', '·', '50%', 'MERGE'], []]);
  assert.deepEqual(texts(byId(root, 'agent-lab/t2')), ['Supervisor', 'Opus 5.5', '·', '10%', 'IDLE']);
  assert.deepEqual(byId(root, 'merge-lab/t3').calls.tooltip, ['Merge worktree/api into develop and push'], "lab's project, not this computer's");
  assert.deepEqual(texts(byId(root, 'agent-t3')), ['Fix the cache so that it holds', 'Opus 5.5', '·', '33%', 'WORKING']);
  assert.deepEqual(ids(root, 'merge-'), ['merge-lab/t3']);
  // Under lab's header, after this computer's.
  assert.deepEqual(ids(root, 'agent-'), ['agent-t2', 'agent-t4', 'agent-t3', 'agent-t5', 'agent-t7', 'agent-lab/t2', 'agent-lab/t3']);
  // Asked on lab: where corgi is (a shell there), then its status, once.
  const [look, status] = onLab(w);
  assert.deepEqual([look[0], look[1][0], look[2]], ['sh', '-c', { on: LAB, timeout: 20000 }]);
  for (const place of ['$app/mods/corgi/config.json', '$app/mods/projects/config.json', 'command -v corgi', "-lc 'command -v corgi'", '$HOME/.local/bin/corgi', '$HOME/.cargo/bin/corgi']) {
    assert.ok(look[1][1].includes(place), `the look tries ${place}`);
  }
  assert.deepEqual(status, [LAB_CORGI, ['status', '--json'], { on: LAB, timeout: 20000 }]);
  assert.equal(onLab(w).length, 2);
  // This computer's corgi is asked as before, with no machine.
  for (const [, options] of localRuns(w, 'status')) assert.deepEqual(options, { timeout: 20000 });
  // HERD counts lab's agents too, its tagged worker as MERGE.
  assert.deepEqual(ids(root, 'herd-'), ['herd-card', 'herd-badges', 'herd-working', 'herd-blocked', 'herd-merge', 'herd-done', 'herd-idle']);
  assert.equal(byId(root, 'herd-merge').calls.tooltip[0], '1 to merge');
  assert.equal(byId(root, 'herd-idle').calls.tooltip[0], '3 idle');
  assert.equal(byId(root, 'machine-no-corgi-lab'), null);
}

// lab's corgi is found once, and read every 5 seconds at most, less often
// when it is slow: never sooner than three times its last read took.
{
  const realNow = Date.now;
  let clock = realNow();
  Date.now = () => clock;
  try {
    let slow = 4000;
    const w = labWindow({ answers: { status: () => { clock += slow; return labStatus(); } } });
    const mod = await projects(w);
    const reads = () => labRuns(w, 'status').length;
    assert.equal(reads(), 1);
    // Took 4 s: the next read waits 12 s from its start.
    clock += 5000;
    await mod.tick(5000);
    await mod.tick(1000);
    assert.equal(reads(), 1);
    // Live agents changing reads this computer's at once, lab's not yet.
    w.agents[0] = { ...w.agents[0], status: 'working' };
    await mod.tick(1000);
    assert.equal(reads(), 1);
    clock += 3000;
    slow = 0;
    await mod.tick(5000);
    assert.equal(reads(), 2);
    // Quick again: every 5 seconds, no sooner.
    clock += 4999;
    await mod.tick(5000);
    assert.equal(reads(), 2);
    clock += 1;
    await mod.tick(5000);
    assert.equal(reads(), 3);
    assert.equal(onLab(w).filter(([command]) => command === 'sh').length, 1, 'found once');
  } finally {
    Date.now = realNow;
  }
}

// MERGE on lab's worker: the merge, the close and the question for the
// supervisor run on lab, and lab's status is read again after each.
{
  const MERGED_API = { result: 'merged', message: 'Merged and pushed worktree/api into develop', commit: 'abc1234', reason: null, error: null, supervisor: null };
  const CLOSED_API = { result: 'closed', message: 'Closed w-api', reason: null, dirty: [], worktree: API_TREE, branch: 'worktree/api', removed: true };
  const w = labWindow({ answers: { merge: MERGED_API, close: CLOSED_API } });
  const mod = await projects(w);
  click(mod.render(), 'merge-lab/t3', mod.cx);
  assert.deepEqual(texts(byId(mod.render(), 'merge-panel-lab/t3')), ['Merge worktree/api into develop and push?', 'Cancel', 'Merge + push']);
  const reads = labRuns(w, 'status').length;
  click(mod.render(), 'merge-confirm-lab/t3', mod.cx);
  await mod.settle();
  assert.deepEqual(labRuns(w, 'merge'), [[LAB_CORGI, ['merge', 'w-api', '--project', REPO, '--json'], { timeout: 600000, on: LAB }]]);
  assert.equal(labRuns(w, 'status').length, reads + 1, 'read again at once');
  assert.deepEqual(texts(byId(mod.render(), 'merge-panel-lab/t3')).slice(0, 3), ['w-api: merged', MERGED_API.message, 'Close w-api and remove its worktree?']);
  click(mod.render(), 'close-confirm-lab/t3', mod.cx);
  await mod.settle();
  assert.deepEqual(labRuns(w, 'close'), [[LAB_CORGI, ['close', 'w-api', '--project', REPO, '--json'], { timeout: 600000, on: LAB }]]);
  assert.equal(labRuns(w, 'status').length, reads + 2);
  assert.deepEqual(localRuns(w, 'merge').concat(localRuns(w, 'close')), [], "nothing of it runs on this computer");

  // A push that failed: lab's supervisor runs (this computer's does not),
  // so it can be asked, on lab.
  const failed = { result: 'push-failed', message: 'Merged, but the push failed', commit: 'def5678', reason: null, error: 'rejected: fetch first', supervisor: null };
  const v = labWindow({ answers: { merge: (args) => (args.includes('--ask-supervisor') ? { result: 'asked', message: 'Asked weather-supervisor' } : failed) } });
  const other = await projects(v);
  click(other.render(), 'merge-lab/t3', other.cx);
  click(other.render(), 'merge-confirm-lab/t3', other.cx);
  await other.settle();
  click(other.render(), 'merge-ask-lab/t3', other.cx);
  await other.settle();
  assert.deepEqual(labRuns(v, 'merge').map(([, args, options]) => [args.slice(5), options]), [
    [[], { timeout: 600000, on: LAB }],
    [['--ask-supervisor', 'rejected: fetch first', '--commit', 'def5678'], { timeout: 600000, on: LAB }],
  ]);
  assert.deepEqual(texts(byId(other.render(), 'merge-panel-lab/t3')), ['Asked weather-supervisor', 'Dismiss']);
}

// A ↓N on lab's checkout pulls there.
{
  const w = labWindow({ pull: { stdout: 'Fast-forward\n' } });
  const mod = await projects(w);
  const key = JSON.stringify([LAB, REPO]);
  assert.deepEqual(ids(mod.render(), 'pull-'), [`pull-${key}`], "only lab's checkout is behind");
  pressPull(mod.render(), key, mod.cx);
  await mod.settle();
  assert.deepEqual(w.labPulls, [{ cwd: REPO, on: LAB, timeout: 120000 }]);
}

// "⏸ Agents" pauses on each machine where an agent of Corgi's works, all
// at once; an agent left behind is listed with its machine. "▶ Agents"
// resumes on each machine that has one paused.
{
  let finish;
  const w = labWindow({
    local: { status: pauseStatus(false), pause: { result: 'paused', message: 'Paused 1 agent', paused: ['w-cache'], failed: [], left: [] } },
    answers: { status: labStatus({ working: true }), pause: () => new Promise((resolve) => { finish = resolve; }) },
  });
  w.agents = w.agents.map((a) => (a.address === 'lab/t3' ? { ...a, status: 'working' } : a));
  const mod = await projects(w);
  click(mod.render(), 'agents-pause', mod.cx);
  await new Promise((resolve) => setImmediate(resolve));
  assert.deepEqual(texts(byId(mod.render(), 'agents-pause-panel')), ['Pausing 2 agents…']);
  assert.deepEqual(localRuns(w, 'pause'), [[['pause', '--json'], { timeout: 300000 }]]);
  assert.deepEqual(labRuns(w, 'pause'), [[LAB_CORGI, ['pause', '--json'], { timeout: 300000, on: LAB }]]);
  // What this computer's Corgi says once w-cache is paused.
  w.programs[2] = corgi({ status: pauseStatus(true) });
  finish({ result: 'partial', message: 'Paused 0 agents', paused: [], failed: [{ name: 'w-api', why: 'did not exit; left running' }], left: [] });
  await mod.settle();
  assert.deepEqual(texts(byId(mod.render(), 'agents-pause-panel')), ['Not paused:', 'w-api on lab: did not exit; left running', 'Dismiss']);
  assert.deepEqual(header(mod.render()), ['▶ Agents'], "this computer's w-cache is paused");

  // Only lab has agents paused: the resume runs there alone. Its paused
  // worker whose terminal ended stands under lab's project, not this
  // computer's of the same folder.
  const r = labWindow({ local: { status: pauseStatus(false) }, answers: { status: labStatus({ paused: true }), resume: { result: 'resumed', message: 'Resumed 2 agents', resumed: ['w-api', 'w-radar'], failed: [], left: [] } } });
  const again = await projects(r);
  const root = again.render();
  assert.deepEqual(header(root), ['▶ Agents']);
  assert.match(byId(root, 'agents-pause').calls.tooltip[0], /^Resume the 2 paused agents/);
  assert.deepEqual(ids(root, 'agent-'), ['agent-t2', 'agent-t4', 'agent-t3', 'agent-t5', 'agent-t7', 'agent-lab/t2', 'agent-lab/t20', 'agent-lab/t3']);
  assert.equal(markOf(root, 'agent-lab/t20'), 'PAUSED');
  click(root, 'agents-pause', again.cx);
  await again.settle();
  assert.deepEqual(localRuns(r, 'resume'), []);
  assert.deepEqual(labRuns(r, 'resume'), [[LAB_CORGI, ['resume', '--json'], { timeout: 300000, on: LAB }]]);

  // A machine that refuses while the other pauses is a line of the card's.
  const refusal = labWindow({
    local: { status: pauseStatus(false), pause: { result: 'paused', message: 'Paused 1 agent', paused: ['w-cache'], failed: [], left: [] } },
    answers: { status: labStatus({ working: true }), pause: { code: 1, stdout: JSON.stringify({ result: 'refused', message: 'herdr-core (hc) is not running', paused: [], failed: [], left: [] }) } },
  });
  refusal.agents = refusal.agents.map((a) => (a.address === 'lab/t3' ? { ...a, status: 'working' } : a));
  const third = await projects(refusal);
  click(third.render(), 'agents-pause', third.cx);
  await third.settle();
  assert.deepEqual(texts(byId(third.render(), 'agents-pause-panel')), ['Not paused:', 'Agents on lab: herdr-core (hc) is not running', 'Dismiss']);
}

// Without corgi on lab its rows show with their own titles and no Corgi
// data; its header says so once, quietly. It is looked for again a minute
// later, not every 5 seconds.
{
  const realNow = Date.now;
  let clock = realNow();
  Date.now = () => clock;
  try {
    const w = labWindow({ located: { code: 3, stdout: '' } });
    const mod = await projects(w);
    const root = mod.render();
    assert.deepEqual(ids(root, 'machine-no-corgi-'), ['machine-no-corgi-lab']);
    const note = byId(root, 'machine-no-corgi-lab');
    assert.deepEqual(texts(note), ["Corgi isn't installed on lab"]);
    assert.deepEqual(note.calls.text_color, ['#dim']);
    assert.deepEqual(texts(byId(root, 'agent-lab/t3')), ['Serving the forecast', 'DONE']);
    assert.deepEqual(texts(byId(root, 'agent-lab/t2')), ['Claude Code', 'IDLE']);
    assert.deepEqual(ids(root, 'merge-'), []);
    // This computer's rows are as ever.
    assert.deepEqual(texts(byId(root, 'agent-t3')), ['Fix the cache so that it holds', 'Opus 5.5', '·', '33%', 'WORKING']);
    const looks = () => onLab(w).filter(([command]) => command === 'sh').length;
    assert.equal(looks(), 1);
    assert.deepEqual(labRuns(w, 'status'), []);
    clock += 30000;
    await mod.tick(5000);
    assert.equal(looks(), 1);
    clock += 30000;
    await mod.tick(5000);
    assert.equal(looks(), 2);

    // Removed after it was found: the read that fails looks for it again.
    let there = true;
    const v = labWindow({ located: () => (there ? { stdout: `${LAB_CORGI}\n` } : { code: 3, stdout: '' }) });
    const other = await projects(v);
    assert.deepEqual(ids(other.render(), 'merge-'), ['merge-lab/t3']);
    there = false;
    v.programs.unshift((command) => {
      if (command === LAB_CORGI) throw new Error(`${LAB_CORGI}: not installed`);
    });
    other.view.refreshRemote(LAB, other.cx, true);
    await other.settle();
    assert.deepEqual(ids(other.render(), 'merge-'), []);
    other.view.refreshRemote(LAB, other.cx, true);
    await other.settle();
    assert.deepEqual(texts(byId(other.render(), 'machine-no-corgi-lab')), ["Corgi isn't installed on lab"]);
  } finally {
    Date.now = realNow;
  }
}

// lab away (herdr-core cannot reach it): its rows show without data and
// nothing is said; a machine that is not connected is not asked at all.
{
  const w = labWindow();
  w.programs.unshift((_command, _args, options) => {
    if (options.on === LAB) throw new Error('lab is unreachable');
  });
  const mod = await projects(w);
  const root = mod.render();
  assert.deepEqual(texts(byId(root, 'agent-lab/t3')), ['Serving the forecast', 'DONE']);
  assert.equal(byId(root, 'machine-no-corgi-lab'), null);
  assert.deepEqual(texts(byId(root, 'agent-t3')), ['Fix the cache so that it holds', 'Opus 5.5', '·', '33%', 'WORKING']);

  // Data it had goes when it stops answering.
  const v = labWindow();
  const other = await projects(v);
  assert.deepEqual(ids(other.render(), 'merge-'), ['merge-lab/t3']);
  v.programs.unshift((_command, _args, options) => {
    if (options.on === LAB) throw new Error('lab is unreachable');
  });
  other.view.refreshRemote(LAB, other.cx, true);
  await other.settle();
  assert.deepEqual(ids(other.render(), 'merge-'), []);

  const asleep = labWindow();
  asleep.machineList = [{ name: LAB, state: 'unreachable', target: 'jo@lab' }];
  await projects(asleep);
  assert.deepEqual(onLab(asleep), []);
}

// --- Corgi's layout: the installer changes it, never a mod ------------------------------

// The Projects list never runs `corgi app-layout` (it would clone and switch
// layouts), switches the window's layout or says anything of the layout.
{
  const w = window();
  const mod = await projects(w);
  await mod.tick(5000);
  assert.ok(!w.execs.some((call) => call[1] === 'app-layout'), JSON.stringify(w.execs));
  assert.ok(!w.ran.some(([name]) => name.startsWith('layout.')));
  assert.deepEqual(w.toasts, []);
}

// No module of the mod turns an svg for a spinner: every one is the
// braille frames of `spinner` (the app turns `spin` at about 10 fps while
// the window is quiet). The rows and HERD have none at all (their badges
// say WORKING).
{
  const { readdirSync, readFileSync } = await import('node:fs');
  const dir = new URL('../herdr-app/mods/corgi/', import.meta.url);
  const modules = readdirSync(dir).filter((name) => name.endsWith('.js'));
  assert.ok(modules.includes('projects.js') && modules.includes('data.js'), modules.join(' '));
  for (const name of modules) {
    const source = readFileSync(new URL(name, dir), 'utf8');
    assert.ok(!/\.spin\(/.test(source), `corgi/${name} still turns an svg with .spin(`);
    assert.ok(!source.includes('spinner.svg'), `corgi/${name} still draws spinner.svg`);
  }
}

// --- One mod, its views chosen by the blocks' props ---------------------------------

// The installer's test for the retired overview mod (`mods/corgi` whose
// main.js sets up its own install of corgi) must never match this one.
{
  const { readFileSync } = await import('node:fs');
  const main = readFileSync(new URL('../herdr-app/mods/corgi/main.js', import.meta.url), 'utf8');
  assert.ok(main.includes('io.github.zyrre.corgi'), "the installer's mark of Corgi's mods");
  assert.ok(!/^const INSTALLER = /m.test(main), 'the mark of the retired overview mod');
}

// The brand row runs nothing; a view this Corgi lacks says so and runs
// nothing either.
{
  const w = window();
  const brand = await start(w, { view: 'brand' });
  await brand.tick(5000);
  assert.deepEqual(texts(brand.render()), ['Herdr', '+', 'Corgi']);
  const later = await start(w, { view: 'board' });
  await later.tick(5000);
  assert.deepEqual(texts(later.render()), ['This Corgi has no view board: update Corgi.']);
  assert.deepEqual(w.execs, []);
  assert.deepEqual(w.ran, []);
}

// --- the Questions panel ---------------------------------------------------------------

// Q7 and Q8 as `corgi questions --json` gives them: Q8 strays from the
// form, Q9 is answered and queued.
const QUESTION = (id, extra = {}) => ({
  id, number: Number(id.slice(1)), title: `Question ${id}?`, blocks: { kind: 'merge', what: 'Fix the cache' },
  context: 'Why it matters.', options: [{ key: 'A', choice: 'Keep it', cost: 'nothing breaks' }, { key: 'B', choice: 'Drop it', cost: 'simpler' }],
  recommended: 'A', recommendation: 'A, because nothing breaks.', until: 'The merge waits.', parsed: true, raw: `### ${id}`,
  file: `${HOME}/.local/state/corgi/supervisors/weather/questions/${id}.md`, answer: null, ...extra,
});
const ASKED = {
  project: REPO, open: 2,
  questions: [
    QUESTION('Q7'),
    QUESTION('Q8', { parsed: false, title: '', options: [], recommended: null, blocks: null, raw: 'Should we drop Node 20?' }),
    QUESTION('Q9', { answer: { answer: 'B', state: 'queued', at: '2026-11-10T07:00:00Z' } }),
  ],
};
const BACKLOG = {
  project: REPO, title: 'Later', intro: '',
  entries: [
    { heading: 'Export to Parquet', body: '- asked for\n- needs the rename', start: null },
    { heading: 'Dark charts', body: '', start: { answer: '', state: 'sent', at: '2026-11-10T07:00:00Z' } },
  ],
};
const ASKING = {
  ...STATUS,
  projects: [{ ...STATUS.projects[0], open_questions: 2, questions: [{ id: 'Q7', title: 'Question Q7?' }], backlog: 2 }],
};
// The layout with its right-hand panel showing the stack child `active`.
const panelLayout = (w, active = 'corgi-questions', hidden = false) => {
  w.api.layout = () => ({ id: 'root', children: [
    { id: 'sidebar', size: { px: w.sidebar }, children: [] },
    { id: 'panel', hidden, children: [{ id: 'panel-body', type: 'stack', active, children: [
      { id: 'files', type: 'block', content: { mod: 'files' } },
      { id: 'corgi-questions', type: 'block', content: { mod: 'corgi', props: { view: 'questions' } } },
    ] }] },
  ] });
};
const show = (w, id) => {
  for (const s of w.spaces) s.active = s.id === id;
};
const answers = (w) => w.execs.filter((call) => call[1] === 'answer').map((call) => call.slice(2));
const reads = (w, verb) => w.execs.filter((call) => call[1] === verb).length;
// The list works out the current project as it draws, and tells the panel
// on its next second.
const publish = async (list) => {
  list.render();
  await list.tick(1000);
};

// A supervisor's open questions: a pill on its first line, which shows the
// panel on its project; the list tells the panel the current project.
{
  const w = window({ status: ASKING });
  show(w, 'sp-weather');
  const list = await projects(w, {}, 'questions-pill');
  await publish(list);
  const root = list.render();
  const pill = byId(root, 'questions-t2');
  assert.ok(pill, 'the supervisor row has the count');
  assert.deepEqual(texts(pill), ['2']);
  assert.deepEqual(kept(pill.children[0]).args, ['mods/corgi/question.svg']);
  // On the first line, after the task; the second keeps its state's badge.
  const row = byId(root, 'agent-t2');
  assert.ok(texts(kept(row.children[0])).includes('2'));
  assert.equal(markOf(root, 'agent-t2'), 'IDLE');
  // Workers have none.
  assert.equal(byId(root, 'questions-t3'), null);
  assert.deepEqual(w.shared['corgi.questions.current'], {
    machine: null, dir: REPO, name: 'weather', open: 2, key: JSON.stringify([2, ['Q7'], 0, 2]),
  });
  assert.equal(w.shared['corgi.questions.count'], 2);
  // Another space: no project, no count.
  show(w, 'sp-home');
  await publish(list);
  assert.equal(w.shared['corgi.questions.current'], null);
  assert.equal(w.shared['corgi.questions.count'], 0);
  // The pill picks its project while that space shows, and shows the panel.
  const stopped = list.cx.stopped;
  click(list.render(), 'questions-t2', list.cx);
  assert.ok(list.cx.stopped > stopped, 'the row does not take the click');
  assert.deepEqual(w.ran.slice(-2), [['layout.select', { stack: 'panel-body', child: 'corgi-questions' }], ['layout.show', { id: 'panel' }]]);
  assert.equal(w.shared['corgi.questions.current'].dir, REPO);
  show(w, 'sp-corgi');
  await publish(list);
  assert.equal(w.shared['corgi.questions.current'], null, 'until another space shows');
  // The count is the questions without an answer; none, no pill.
  const none = window({ status: { ...ASKING, projects: [{ ...ASKING.projects[0], open_questions: 0 }] } });
  assert.equal(byId((await projects(none)).render(), 'questions-t2'), null);
}

// The panel: read only while shown, the current project's questions then
// its backlog; one answer per card, sent with the card's Send.
{
  const w = window({ status: ASKING, questions: ASKED, backlog: BACKLOG, answer: { result: 'queued', message: 'Queued' } });
  show(w, 'sp-weather');
  panelLayout(w, 'files');
  const list = await projects(w, {}, 'questions-panel');
  await publish(list);
  const panel = await start(w, { view: 'questions' }, 'questions-panel');
  await panel.tick(1000);
  assert.equal(reads(w, 'questions'), 0, 'not read while Files shows');
  panelLayout(w);
  await panel.tick(1000);
  assert.equal(reads(w, 'questions'), 1);
  assert.equal(reads(w, 'backlog'), 1);
  assert.deepEqual(w.calls.find((c) => c.args[0] === 'questions').args, ['questions', '--project', REPO, '--json']);
  await panel.tick(1000);
  assert.equal(reads(w, 'questions'), 1, 'not again while nothing changed');

  let root = panel.render();
  let drawn = texts(root);
  assert.ok(drawn.includes('weather'), 'the project is named');
  // Two open: Q7 and Q8 open, Q9 (answered) closed; then the backlog.
  assert.ok(byId(root, 'card-Q7') && byId(root, 'card-Q8') && byId(root, 'closed-Q9'));
  assert.ok(drawn.includes('RECOMMENDED'));
  assert.ok(drawn.includes('Should we drop Node 20?'), 'a question off the form shows its text');
  assert.ok(drawn.includes('Queued B: goes when the supervisor is idle'));
  assert.ok(drawn.indexOf('Backlog') > drawn.indexOf('Question Q9?'));
  assert.ok(drawn.includes('Export to Parquet') && drawn.includes('✓ Plan asked'));
  assert.ok(byId(root, 'start-0') && !byId(root, 'start-1'));

  // Send waits for a choice; an option picks it.
  assert.equal(byId(root, 'send-Q7').handlers.on_click, undefined, 'nothing to send yet');
  click(root, 'opt-Q7-B', panel.cx);
  root = panel.render();
  assert.deepEqual(texts(byId(root, 'send-Q7')), ['Send B']);
  click(root, 'send-Q7', panel.cx);
  await panel.settle();
  assert.deepEqual(answers(w), [['--project', REPO, '--json', 'Q7=B']]);
  assert.equal(reads(w, 'questions'), 2, 'read again after an answer');

  // The Other field: what is typed there is the answer.
  root = panel.render();
  const field = [];
  walk(byId(root, 'card-Q8'), (node) => { if (node.kind === 'input') field.push(node.args[0]); });
  assert.equal(field.length, 1);
  field[0].text = 'Keep it\nuntil March';
  field[0].handlers.change(null, panel.cx);
  click(panel.render(), 'send-Q8', panel.cx);
  await panel.settle();
  assert.deepEqual(answers(w).at(-1), ['--project', REPO, '--json', 'Q8=other: Keep it until March']);
  assert.equal(field[0].text, '', 'the field is emptied once queued');

  // A refusal stays on the card.
  w.programs[1] = corgi({ status: ASKING, questions: ASKED, backlog: BACKLOG, answer: { result: 'refused', message: 'Q7 is not open' } });
  click(panel.render(), 'opt-Q7-A', panel.cx);
  click(panel.render(), 'send-Q7', panel.cx);
  await panel.settle();
  assert.ok(texts(byId(panel.render(), 'card-Q7')).includes('Q7 is not open'));

  // A backlog Start asks for a plan.
  w.programs[1] = corgi({ status: ASKING, questions: ASKED, backlog: BACKLOG, answer: { result: 'queued' } });
  click(panel.render(), 'start-0', panel.cx);
  await panel.settle();
  assert.deepEqual(answers(w).at(-1), ['--project', REPO, '--json', '--start', 'Export to Parquet']);

  // A closed card opens with a click, and an open one closes.
  click(panel.render(), 'closed-Q9', panel.cx);
  assert.ok(byId(panel.render(), 'card-Q9'));
  click(panel.render(), 'fold-Q7', panel.cx);
  assert.ok(byId(panel.render(), 'closed-Q7'));
}

// More than two open: every card starts closed. No project: says so. A
// Corgi without the command: says to update.
{
  const many = { ...ASKED, questions: ['Q1', 'Q2', 'Q3'].map((id) => QUESTION(id)) };
  const w = window({ status: ASKING, questions: many, backlog: { ...BACKLOG, entries: [] } });
  show(w, 'sp-weather');
  panelLayout(w);
  const list = await projects(w, {}, 'questions-many');
  await publish(list);
  const panel = await start(w, { view: 'questions' }, 'questions-many');
  await panel.tick(1000);
  const root = panel.render();
  assert.deepEqual(ids(root, 'closed-'), ['closed-Q1', 'closed-Q2', 'closed-Q3']);
  assert.deepEqual(ids(root, 'card-'), []);
  assert.ok(texts(root).includes('The backlog is empty'));

  const away = window({ status: ASKING });
  panelLayout(away);
  const lone = await projects(away, {}, 'questions-none');
  await publish(lone);
  const empty = await start(away, { view: 'questions' }, 'questions-none');
  await empty.tick(1000);
  assert.ok(texts(empty.render()).includes('No project'));
  assert.equal(reads(away, 'questions'), 0);

  const old = window({ status: ASKING });
  show(old, 'sp-weather');
  panelLayout(old);
  await publish(await projects(old, {}, 'questions-old'));
  const older = await start(old, { view: 'questions' }, 'questions-old');
  await older.tick(1000);
  assert.ok(texts(older.render()).some((t) => /no questions command: update Corgi/.test(t)));
}

// Copies of the mod share what Corgi says: however many blocks show the
// Projects list (and the brand row beside them), `corgi status`, `corgi
// usage` and `hc machine ls` run once per refresh, and every copy shows
// the answer.
{
  const USAGE = { now: 1, providers: [] };
  const w = window({ usage: USAGE });
  let machineLists = 0;
  w.api.hc = async () => {
    machineLists += 1;
    return { code: 0, stdout: JSON.stringify({ machines: w.machineList }), stderr: '' };
  };
  const reads = (what) => w.execs.filter((call) => call[1] === what).length;
  const a = await projects(w, {}, 'shared');
  const b = await projects(w, { view: 'projects' }, 'shared');
  const brand = await start(w, { view: 'brand' }, 'shared');
  // The first copy read for both; the second found it read.
  assert.equal(reads('status'), 1);
  assert.equal(reads('usage'), 1);
  assert.equal(machineLists, 1);
  assert.deepEqual(texts(byId(b.render(), 'agent-t3'))[0], 'Fix the cache so that it holds');
  // Their timers tick out of step: still one read a period, by whichever
  // copy's tick comes first.
  for (let round = 1; round <= 3; round++) {
    await b.tick(5000);
    await a.tick(5000);
    await brand.tick(5000);
    assert.equal(reads('status'), 1 + round, `round ${round}`);
  }
  await a.tick(60000);
  await b.tick(60000);
  assert.equal(reads('usage'), 2);
  await b.tick(15000);
  await a.tick(15000);
  assert.equal(machineLists, 2);
  // The live agents changing reads at once, once.
  const before = reads('status');
  w.agents[0] = { ...w.agents[0], status: 'working' };
  await a.tick(1000);
  await b.tick(1000);
  assert.equal(reads('status'), before + 1);
  // A read one copy made shows in the other on its next tick.
  w.programs[1] = corgi({ status: { ...STATUS, projects: [{ ...STATUS.projects[0], agents: STATUS.projects[0].agents.map((x) => (x.terminal === 't3' ? { ...x, task: 'Warm the cache' } : x)) }] }, usage: USAGE });
  const drawn = b.cx.notified;
  await a.tick(5000);
  await b.tick(5000);
  assert.equal(reads('status'), before + 2);
  await b.tick(1000);
  assert.ok(b.cx.notified > drawn, 'the other copy draws again');
  assert.deepEqual(texts(byId(b.render(), 'agent-t3'))[0], 'Warm the cache');
  // A copy that is gone (its timers stopped) leaves the reading to the rest.
  for (let round = 1; round <= 2; round++) {
    await b.tick(5000);
    assert.equal(reads('status'), before + 2 + round);
  }
  // The brand row ran nothing of its own.
  assert.ok(w.execs.every(([command, verb]) => command === 'git' || ['status', 'usage'].includes(verb)), JSON.stringify(w.execs));
}

// A copy started on its own, as after the mod is saved again (each copy
// starts over on new modules), reads for itself.
{
  const w = window();
  await projects(w, {}, 'one');
  await projects(w, {}, 'two');
  assert.equal(w.execs.filter((call) => call[1] === 'status').length, 2);
}

// --- the terminal panel ------------------------------------------------------------

// The layout file with the terminal panel as `corgi app-layout` puts it,
// shown (or not): the window's live layout is the file, with the panel's
// `hidden` and the stack's `active`.
const TERMINAL_LAYOUT = `${APP}/layouts/corgi.json`;
function terminalWindow({ shown = true, body = { mod: 'corgi', props: { view: 'terminal-hint' } } } = {}) {
  const w = window();
  const column = { id: 'corgi-terminal', type: 'column', children: [
    { id: 'corgi-terminal-head', type: 'block', size: { px: 58 }, content: { mod: 'corgi', props: { view: 'terminal' } } },
    ...(body ? [{ id: 'corgi-terminal-body', type: 'block', padding: [6, 10], content: body }] : []),
  ] };
  const tree = { id: 'root', children: [
    { id: 'sidebar', size: { px: 268 }, children: [] },
    { id: 'panel', type: 'column', children: [{ id: 'panel-body', type: 'stack', children: [
      { id: 'files', type: 'block', content: { mod: 'files' } },
      { id: 'other', type: 'block', content: { mod: 'gitlab' } },
      column,
    ] }] },
  ] };
  w.files[TERMINAL_LAYOUT] = JSON.stringify(tree, null, 2);
  w.shared.layout = 'corgi';
  w.view = { hidden: !shown, active: 'corgi-terminal' };
  w.api.layout = () => {
    const live = JSON.parse(w.files[TERMINAL_LAYOUT]);
    const panel = live.children[1];
    panel.hidden = w.view.hidden;
    panel.children[0].active = w.view.active;
    live.children[0].size.px = w.sidebar;
    return live;
  };
  // hc: a shell started is live and labelled; a label or a kill changes it.
  let next = 20;
  w.hcPrograms.push((args) => {
    if (args[0] === 'run') {
      const labels = {};
      args.forEach((a, i) => { if (args[i - 1] === '--label') { const [k, ...v] = a.split('='); labels[k] = v.join('='); } });
      const address = `t${next++}`;
      w.terminals.push({ address, state: 'live', cwd: args[args.indexOf('--cwd') + 1], labels, argv: args.slice(args.indexOf('--') + 1) });
      return { stdout: JSON.stringify({ address, state: 'live' }) };
    }
    if (args[0] === 'label') {
      const t = w.terminals.find((x) => x.address === args[1]);
      const [k, v] = args[2].split('=');
      t.labels[k] = v;
      return { stdout: JSON.stringify(t) };
    }
    // As hc's: a kill answers once the terminal finished; a live one is
    // not removed, a gone one is not found.
    const gone = () => ({ code: 3, stderr: JSON.stringify({ error: { kind: 'not_found', message: `no terminal "${args[1]}"` } }) });
    if (args[0] === 'kill') {
      const t = w.terminals.find((x) => x.address === args[1]);
      if (!t) return gone();
      t.state = 'finished';
      return { stdout: JSON.stringify(t) };
    }
    if (args[0] === 'rm') {
      const t = w.terminals.find((x) => x.address === args[1]);
      if (t?.state === 'live') return { code: 1, stderr: JSON.stringify({ error: { kind: 'live', message: `${args[1]} is still running` } }) };
      w.terminals = w.terminals.filter((x) => x.address !== args[1]);
      return { stdout: JSON.stringify({ removed: args[1] }) };
    }
    return undefined;
  });
  // The person's login shell.
  w.programs.push((command, args) => (command === '/bin/sh' && /UserShell/.test(args[1] ?? '') ? { stdout: '/bin/zsh' } : undefined));
  return w;
}
const bodyNow = (w) => JSON.parse(w.files[TERMINAL_LAYOUT]).children[1].children[0].children[2].children.find((c) => c.id === 'corgi-terminal-body')?.content ?? null;
const runs = (w) => w.hcs.filter((a) => a[0] === 'run');
const focus = (w, terminal) => {
  w.focused = { space: 'sp-calm', tab: 'tab-sp-calm', pane: `pane-${terminal}`, terminal };
};
// The Projects list (it reads Corgi's status for every copy) and the
// panel's header, sharing the mod's modules.
async function terminalPanel(w, module) {
  const list = await projects(w, {}, module);
  const head = await start(w, { view: 'terminal' }, module);
  return { list, head };
}

// No shell starts by itself (decision 2026-11-17): the focused agent's
// folder with none shows "No shell here" and Open shell; Open shell starts
// one, in the person's login shell, put into the panel's body; another
// agent's folder shows the same until asked; back, the first one again.
{
  const w = terminalWindow();
  const { head } = await terminalPanel(w, 'terminal-follow');
  const calm = `${TREES}/worktree-calm-forest-4f96`;
  focus(w, 't3');
  await head.tick(500);
  await head.tick(500);
  assert.equal(runs(w).length, 0, 'none on focus');
  assert.equal(w.shared['corgi.terminal'].note, 'none');
  assert.equal(w.shared['corgi.terminal'].dir, calm);
  assert.deepEqual(bodyNow(w), { mod: 'corgi', props: { view: 'terminal-hint' } });
  const hint = await start(w, { view: 'terminal-hint' }, 'terminal-follow');
  assert.deepEqual(texts(hint.render()), ['No shell here', 'Open shell']);
  assert.equal(byId(hint.render(), 'terminal-new'), null);
  click(hint.render(), 'terminal-open', hint.cx);
  await hint.settle();
  assert.deepEqual(runs(w), [[
    'run', '--bg', '--keep', '--json', '--no-parent', '--no-notify', '--name', 'sh-worktree-calm-forest-4f96',
    '--cwd', calm, '--label', 'corgi.shell=panel', '--label', `corgi.shell.dir=${calm}`, '--', '/bin/zsh', '-l',
  ]]);
  assert.deepEqual(bodyNow(w), { terminal: 't20' });
  assert.equal(w.shared['corgi.terminal'].dir, calm);
  assert.equal(w.shared['corgi.terminal'].note, null);
  // Only the body block changed in the file.
  const written = JSON.parse(w.files[TERMINAL_LAYOUT]);
  assert.deepEqual(written.children[1].children[0].children.map((c) => c.id), ['files', 'other', 'corgi-terminal']);
  assert.equal(written.children[1].hidden, undefined, 'the panel state is not written');
  // Nothing to do while nothing changed.
  const writes = w.writes.length;
  await head.tick(500);
  assert.equal(w.writes.length, writes);
  assert.equal(runs(w).length, 1);
  // The supervisor: its folder, the project root, has no shell: the hint again.
  focus(w, 't2');
  await head.tick(500);
  assert.equal(runs(w).length, 1, 'none on focus');
  assert.equal(w.shared['corgi.terminal'].dir, REPO);
  assert.deepEqual(bodyNow(w), { mod: 'corgi', props: { view: 'terminal-hint' } });
  assert.deepEqual(texts(hint.render()), ['No shell here', 'Open shell']);
  click(hint.render(), 'terminal-open', hint.cx);
  await hint.settle();
  assert.equal(runs(w).length, 2);
  assert.equal(runs(w)[1][runs(w)[1].indexOf('--cwd') + 1], REPO);
  assert.deepEqual(bodyNow(w), { terminal: 't21' });
  // Back: the first shell, still running whatever it ran.
  focus(w, 't3');
  await head.tick(500);
  assert.equal(runs(w).length, 2, 'no new shell');
  assert.deepEqual(bodyNow(w), { terminal: 't20' });
  // A terminal that is no agent's (the panel's own shell, a person's) keeps it.
  focus(w, 't20');
  await head.tick(500);
  focus(w, 't8');
  await head.tick(500);
  assert.deepEqual(bodyNow(w), { terminal: 't20' });

  // The switch: the project root's shell, then the worktree's again.
  click(head.render(), 'terminal-root', head.cx);
  await head.settle();
  assert.equal(w.shared['corgi.terminal'].kind, 'root');
  assert.deepEqual(bodyNow(w), { terminal: 't21' }, "the root's shell is the supervisor's");
  assert.ok(texts(head.render()).includes('weather'));
  click(head.render(), 'terminal-worktree', head.cx);
  await head.settle();
  assert.deepEqual(bodyNow(w), { terminal: 't20' });
  assert.equal(runs(w).length, 2);
  const drawn = texts(head.render());
  assert.ok(drawn.includes('worktree-calm-forest-4f96') && drawn.includes('w-cache') && drawn.includes('Fix the cache so that it holds'), drawn.join(' | '));
  assert.ok(drawn.includes('Follows focus'));

  // Pinned: another agent's focus leaves it.
  click(head.render(), 'terminal-pin', head.cx);
  await head.settle();
  assert.equal(w.shared['corgi.terminal'].follow, false);
  assert.ok(texts(head.render()).includes('Pinned'));
  focus(w, 't2');
  await head.tick(500);
  assert.deepEqual(bodyNow(w), { terminal: 't20' });
  click(head.render(), 'terminal-pin', head.cx);
  await head.settle();
  assert.deepEqual(bodyNow(w), { terminal: 't21' }, 'follows again');
  // An agent started by hand in another checkout, with no shell: the hint,
  // and following on to it starts nothing.
  w.panes.find((p) => p.terminal === 't7').cwd = `${HOME}/notes`;
  focus(w, 't7');
  await head.tick(500);
  assert.equal(runs(w).length, 2);
  assert.equal(w.shared['corgi.terminal'].note, 'none');
}

// The Worktree | Project root switch is an ask: the other folder, without a
// shell, gets one; with one, it shows it. The same kind again asks nothing.
{
  const w = terminalWindow();
  const { head } = await terminalPanel(w, 'terminal-switch');
  focus(w, 't3');
  await head.tick(500);
  assert.equal(runs(w).length, 0);
  click(head.render(), 'terminal-worktree', head.cx);
  await head.settle();
  assert.equal(runs(w).length, 0, 'the kind it shows already: no ask');
  click(head.render(), 'terminal-root', head.cx);
  await head.settle();
  assert.equal(runs(w).length, 1);
  assert.equal(runs(w)[0][runs(w)[0].indexOf('--cwd') + 1], REPO);
  assert.deepEqual(bodyNow(w), { terminal: 't20' });
  // Back to the worktree, which has none: the switch asks for it too.
  click(head.render(), 'terminal-worktree', head.cx);
  await head.settle();
  assert.equal(runs(w).length, 2);
  assert.deepEqual(bodyNow(w), { terminal: 't21' });
  // Taken by that look: a later focus on a folder without one starts none.
  w.panes.find((p) => p.terminal === 't7').cwd = `${HOME}/notes`;
  focus(w, 't7');
  await head.tick(500);
  assert.equal(runs(w).length, 2);
  // A start that fails says why, and stays said, with Open shell to try again.
  w.hcPrograms.unshift((args) => (args[0] === 'run' ? { code: 1, stderr: JSON.stringify({ error: { message: 'core is gone' } }) } : undefined));
  const hint = await start(w, { view: 'terminal-hint' }, 'terminal-switch');
  click(hint.render(), 'terminal-open', hint.cx);
  await hint.settle();
  assert.equal(w.shared['corgi.terminal'].note, 'problem');
  await head.tick(500);
  assert.equal(w.shared['corgi.terminal'].note, 'problem', 'still said');
  assert.deepEqual(texts(hint.render()), ['core is gone', 'Open shell']);
  const tries = runs(w).length;
  await head.tick(500);
  assert.equal(runs(w).length, tries, 'not tried again by itself');
  w.hcPrograms.shift();
  click(hint.render(), 'terminal-open', hint.cx);
  await hint.settle();
  assert.equal(w.shared['corgi.terminal'].note, null);
  assert.equal(runs(w).length, tries + 1);
}

// The switch is the shown agent's: Project root picked for one worker
// leaves every other agent on its own folder, in its own space, and that
// worker on the root's shell when the panel comes back to it. Once kept
// for every agent, each of a project's workers showed the one root shell
// (2026-11-17).
{
  const w = terminalWindow();
  const { head } = await terminalPanel(w, 'terminal-per-agent');
  const hint = await start(w, { view: 'terminal-hint' }, 'terminal-per-agent');
  const calm = `${TREES}/worktree-calm-forest-4f96`;
  const sunny = `${TREES}/worktree-sunny-field-529d`;
  // w-docs, in its worktree's space.
  w.agents.push(agent('sp-sunny', 't5', 'working', '✳ Write the docs'));
  const focusIn = (space, terminal) => {
    w.spaces.forEach((s) => { s.active = s.id === space; });
    w.focused = { space, tab: `tab-${space}`, pane: `pane-${terminal}`, terminal };
  };
  focusIn('sp-calm', 't3');
  await head.tick(500);
  click(hint.render(), 'terminal-open', hint.cx);
  await hint.settle();
  assert.deepEqual(bodyNow(w), { terminal: 't20' });
  click(head.render(), 'terminal-root', head.cx);
  await head.settle();
  assert.deepEqual(bodyNow(w), { terminal: 't21' }, "w-cache's project root");
  assert.equal(runs(w)[1][runs(w)[1].indexOf('--cwd') + 1], REPO);

  // w-docs, in its own space: its own folder, which has no shell, not the
  // root's shell picked for w-cache.
  focusIn('sp-sunny', 't5');
  await head.tick(500);
  assert.equal(w.shared['corgi.terminal'].kind, 'worktree');
  assert.equal(w.shared['corgi.terminal'].dir, sunny);
  assert.deepEqual(bodyNow(w), { mod: 'corgi', props: { view: 'terminal-hint' } });
  assert.ok(texts(head.render()).includes('worktree-sunny-field-529d'));
  click(hint.render(), 'terminal-open', hint.cx);
  await hint.settle();
  assert.deepEqual(bodyNow(w), { terminal: 't22' });
  assert.equal(runs(w)[2][runs(w)[2].indexOf('--cwd') + 1], sunny);

  // Back to w-cache: the root's shell, as picked for it; both still run.
  focusIn('sp-calm', 't3');
  await head.tick(500);
  assert.equal(w.shared['corgi.terminal'].kind, 'root');
  assert.deepEqual(bodyNow(w), { terminal: 't21' });
  // Its worktree's shell is there too, by the switch.
  click(head.render(), 'terminal-worktree', head.cx);
  await head.settle();
  assert.deepEqual(bodyNow(w), { terminal: 't20' });
  assert.equal(runs(w).length, 3, 'no shell started by the moves');
  assert.ok(['t20', 't21', 't22'].every((a) => w.terminals.find((t) => t.address === a)?.state === 'live'));
  // The other agent in w-cache's space (same worktree) follows its own choice: Worktree.
  click(head.render(), 'terminal-root', head.cx);
  await head.settle();
  focus(w, 't4');
  await head.tick(500);
  assert.deepEqual(bodyNow(w), { terminal: 't21' }, "the same folder's choice: it is w-cache's worktree too");
  // A `>_` mark shows the worktree's shell, and keeps that choice.
  const shells = await import(new URL('../herdr-app/mods/corgi/shells.js?module=terminal-per-agent', import.meta.url));
  shells.showShellOf('t3');
  await head.tick(500);
  assert.deepEqual(bodyNow(w), { terminal: 't20' });
  focusIn('sp-sunny', 't5');
  await head.tick(500);
  focusIn('sp-calm', 't3');
  await head.tick(500);
  assert.deepEqual(bodyNow(w), { terminal: 't20' });
}

// A panel state from before the choice was kept per folder (`kind` alone,
// "root"): the root was picked for the agent it showed, and only for it.
{
  const w = terminalWindow();
  const calm = `${TREES}/worktree-calm-forest-4f96`;
  w.shared['corgi.terminal'] = { kind: 'root', follow: true, agent: 't3', home: calm, dir: REPO, address: null, note: 'none' };
  const { head } = await terminalPanel(w, 'terminal-old-state');
  w.agents.push(agent('sp-sunny', 't5', 'working', '✳ Write the docs'));
  focus(w, 't5');
  await head.tick(500);
  assert.equal(w.shared['corgi.terminal'].kind, 'worktree');
  assert.equal(w.shared['corgi.terminal'].dir, `${TREES}/worktree-sunny-field-529d`);
  focus(w, 't3');
  await head.tick(500);
  assert.equal(w.shared['corgi.terminal'].kind, 'root');
  assert.equal(w.shared['corgi.terminal'].dir, REPO);
}

// The Terminal button: the panel shown in the space it was hidden in (for
// a while) is the person opening it, which asks for a shell. A space the
// window moves to, whose panel the tab bar restores (the terminal panel
// where it was opened, or another panel or none elsewhere), asks nothing,
// even when the panel is seen hidden there for a moment first.
{
  const realNow = Date.now;
  let clock = 1_000_000;
  Date.now = () => clock;
  try {
    const w = terminalWindow({ shown: false });
    const { list, head } = await terminalPanel(w, 'terminal-open');
    const active = (id) => w.spaces.forEach((s) => { s.active = s.id === id; });
    // w-docs, in its worktree's space.
    w.agents.push(agent('sp-sunny', 't5', 'working', '✳ Write the docs'));
    const both = async () => {
      clock += 250;
      await list.tick(250);
      clock += 250;
      await head.tick(500);
    };
    active('sp-calm');
    focus(w, 't3');
    await both();
    await both();
    // Shown with the Files panel: not the terminal panel, nothing.
    w.view = { hidden: false, active: 'files' };
    await both();
    assert.equal(runs(w).length, 0);
    assert.equal(w.writes.length, 0, 'hidden, nothing written');
    // The Terminal button.
    w.view.active = 'corgi-terminal';
    await both();
    assert.equal(runs(w).length, 1, 'opened: a shell');
    assert.deepEqual(bodyNow(w), { terminal: 't20' });
    await both();
    assert.equal(runs(w).length, 1, 'asked once');

    // To the supervisor's space: the tab bar restores its own panel there
    // (none): hidden, nothing starts.
    active('sp-weather');
    focus(w, 't2');
    w.view.hidden = true;
    await both();
    await both();
    assert.equal(runs(w).length, 1);
    // To a space where the terminal panel was open before: the tab bar
    // shows it again, after a moment hidden. Not an opening.
    active('sp-sunny');
    focus(w, 't5');
    clock += 30;
    await list.tick(250);
    w.view.hidden = false;
    await both();
    await both();
    assert.equal(runs(w).length, 1, 'restored: no shell started');
    assert.equal(w.shared['corgi.terminal'].note, 'none');
    assert.deepEqual(bodyNow(w), { mod: 'corgi', props: { view: 'terminal-hint' } });
    // Back in the first space: its shell is still there.
    active('sp-calm');
    focus(w, 't3');
    await both();
    assert.deepEqual(bodyNow(w), { terminal: 't20' });
    assert.equal(runs(w).length, 1);
    // A moment hidden in the same space is no opening either.
    w.view.hidden = true;
    focus(w, 't4');
    clock += 30;
    await list.tick(250);
    w.panes.find((p) => p.terminal === 't4').cwd = `${HOME}/scratch`;
    w.view.hidden = false;
    await both();
    assert.equal(runs(w).length, 1);
    assert.equal(w.shared['corgi.terminal'].note, 'none');
    // Hidden for a while, then the button: an opening again.
    w.view.hidden = true;
    await both();
    await both();
    w.view.hidden = false;
    await both();
    assert.equal(runs(w).length, 2);
    assert.equal(runs(w)[1][runs(w)[1].indexOf('--cwd') + 1], `${HOME}/scratch`);
  } finally {
    Date.now = realNow;
  }
}

// Folders: Corgi's worktree for its worker, git's checkout for an agent
// started by hand (and its project's root from git's common directory);
// another machine's agent gets none.
{
  const w = terminalWindow();
  const { head } = await terminalPanel(w, 'terminal-folders');
  const calm = `${TREES}/worktree-calm-forest-4f96`;
  w.panes.find((p) => p.terminal === 't4').cwd = `${calm}/src`;
  CHECKOUTS[`${calm}/src`] = [calm, `${REPO}/.git`];
  focus(w, 't4');
  await head.tick(500);
  assert.equal(w.shared['corgi.terminal'].dir, calm, "the checkout's top, not the folder it is in");
  w.shared['corgi.terminal'] = { ...w.shared['corgi.terminal'], kind: 'root', rooted: [calm] };
  await head.tick(500);
  assert.equal(w.shared['corgi.terminal'].dir, REPO);
  delete CHECKOUTS[`${calm}/src`];
  // Another machine's agent.
  w.agents.push(agent('sp-lab', 'lab/t3', 'working', 'far'));
  focus(w, 'lab/t3');
  const before = runs(w).length;
  await head.tick(500);
  assert.equal(w.shared['corgi.terminal'].note, 'remote');
  assert.equal(runs(w).length, before);
  assert.deepEqual(bodyNow(w), { mod: 'corgi', props: { view: 'terminal-hint' } });
  const hint = await start(w, { view: 'terminal-hint' }, 'terminal-folders');
  assert.ok(texts(hint.render()).some((x) => /another machine/.test(x)));
}

// The tracking is hc's labels: a live shell of the panel's, by its folder;
// a finished one, one moved to a tab, another machine's or the person's
// own shell are not the panel's, and a finished one is removed from hc.
{
  const w = terminalWindow();
  const shells = await import(new URL('../herdr-app/mods/corgi/shells.js?module=terminal-track', import.meta.url));
  globalThis.__world = w;
  const list = [
    { address: 't30', state: 'live', labels: { 'corgi.shell': 'panel', 'corgi.shell.dir': '/a' } },
    { address: 't31', state: 'finished', labels: { 'corgi.shell': 'panel', 'corgi.shell.dir': '/b' } },
    { address: 't32', state: 'live', labels: { 'corgi.shell': 'tab', 'corgi.shell.dir': '/c' } },
    { address: 'lab/t33', state: 'live', labels: { 'corgi.shell': 'panel', 'corgi.shell.dir': '/d' } },
    { address: 't34', state: 'live', labels: {} },
  ];
  assert.deepEqual([...shells.panelShells(list)], [['/a', 't30']]);
  shells.sweep(list);
  shells.sweep(list);
  assert.deepEqual(w.hcs, [['rm', 't31']], 'once');
  assert.equal(shells.shellName('/x/Worktree Calm.forest'), 'sh-worktree-calm-forest');
  // Put back: the body block the app took out when its shell ended.
  const tree = { id: 'r', children: [{ id: 'corgi-terminal', children: [{ id: 'corgi-terminal-head' }] }] };
  assert.equal(shells.placeBody(tree, { terminal: 't9' }), true);
  assert.deepEqual(tree.children[0].children[1], { id: 'corgi-terminal-body', type: 'block', padding: [6, 10], content: { terminal: 't9' } });
  assert.equal(shells.placeBody(tree, { terminal: 't9' }), false);
  assert.equal(shells.placeBody({ id: 'r' }, { terminal: 't9' }), false, 'no panel, nothing');
  // Follow or pinned.
  const listed = [{ address: 't2' }, { address: 't3' }];
  assert.equal(shells.targetOf({ follow: true, agent: 't2' }, { terminal: 't3' }, listed), 't3');
  assert.equal(shells.targetOf({ follow: true, agent: 't2' }, { terminal: 't20' }, listed), 't2');
  assert.equal(shells.targetOf({ follow: false, agent: 't2' }, { terminal: 't3' }, listed), 't2');
  assert.equal(shells.targetOf({ follow: true }, null, listed), null);
}

// Only while the panel shows: hidden, or another child of the stack shown,
// it starts and writes nothing, Open shell clicked before included.
{
  const w = terminalWindow({ shown: false });
  const { head } = await terminalPanel(w, 'terminal-hidden');
  focus(w, 't3');
  await head.tick(500);
  w.view = { hidden: false, active: 'files' };
  const shells = await import(new URL('../herdr-app/mods/corgi/shells.js?module=terminal-hidden', import.meta.url));
  shells.ask();
  await head.tick(500);
  assert.equal(runs(w).length, 0);
  assert.equal(w.writes.length, 0);
}

// The body block put back after the app took it out, and a write that
// another writer's change came between is not made: the next one is, on
// the file as it is then, with that change kept.
{
  const w = terminalWindow({ body: null });
  const { head } = await terminalPanel(w, 'terminal-put-back');
  w.terminals.push({ address: 't20', state: 'live', labels: { 'corgi.shell': 'panel', 'corgi.shell.dir': `${TREES}/worktree-calm-forest-4f96` } });
  focus(w, 't3');
  const read = w.api.readTextFile;
  let raced = false;
  w.api.readTextFile = async (path) => {
    const text = await read(path);
    if (path === TERMINAL_LAYOUT && !raced) {
      raced = true;
      const theirs = JSON.parse(w.files[path]);
      theirs.children[1].children[0].children[1].content = { mod: 'gitlab', props: { theirs: true } };
      w.files[path] = JSON.stringify(theirs);
    }
    return text;
  };
  await head.tick(500);
  assert.equal(w.writes.length, 0, 'refused: the file changed after it was read');
  await head.tick(500);
  assert.equal(w.writes.length, 1);
  assert.deepEqual(bodyNow(w), { terminal: 't20' });
  const file = JSON.parse(w.files[TERMINAL_LAYOUT]);
  assert.deepEqual(file.children[1].children[0].children[1].content, { mod: 'gitlab', props: { theirs: true } }, "the other writer's change stays");
}

// A shell the person ended (`exit`): dropped, removed from hc, and the
// panel says so with a New shell button; elsewhere and back, a new one.
// To tab: relabelled, out of the panel, a tab of the agent's space.
{
  const w = terminalWindow();
  const { head } = await terminalPanel(w, 'terminal-end');
  focus(w, 't3');
  await head.tick(500);
  const shells = await import(new URL('../herdr-app/mods/corgi/shells.js?module=terminal-end', import.meta.url));
  shells.newShell();
  await head.tick(500);
  assert.equal(runs(w).length, 1);
  w.terminals.find((t) => t.address === 't20').state = 'finished';
  // The app took the block out of the file.
  const file = JSON.parse(w.files[TERMINAL_LAYOUT]);
  file.children[1].children[0].children[2].children.pop();
  w.files[TERMINAL_LAYOUT] = JSON.stringify(file);
  await head.tick(500);
  assert.equal(runs(w).length, 1, 'no new shell by itself');
  assert.equal(w.shared['corgi.terminal'].note, 'ended');
  assert.deepEqual(bodyNow(w), { mod: 'corgi', props: { view: 'terminal-hint' } }, 'put back, with the hint');
  assert.ok(w.hcs.some((a) => a[0] === 'rm' && a[1] === 't20'));
  const hint = await start(w, { view: 'terminal-hint' }, 'terminal-end');
  assert.deepEqual(texts(hint.render()), ['The shell in worktree-calm-forest-4f96 ended.', 'New shell']);
  click(hint.render(), 'terminal-new', hint.cx);
  await hint.settle();
  assert.equal(runs(w).length, 2);
  assert.deepEqual(bodyNow(w), { terminal: 't21' });

  // To tab.
  click(head.render(), 'terminal-tab', head.cx);
  await head.settle();
  assert.equal(w.terminals.find((t) => t.address === 't21').labels['corgi.shell'], 'tab');
  assert.deepEqual(bodyNow(w), { mod: 'corgi', props: { view: 'terminal-hint' } });
  assert.deepEqual(w.ran.slice(-3), [['agent.focus', { terminal: 't3' }], ['terminal.keep', { terminal: 't21', place: 'tab' }], ['layout.hide', { id: 'panel' }]]);
  assert.equal(w.shared['corgi.terminal'].note, 'moved');
  // Shown again: no shell starts by itself where one moved away.
  await head.tick(500);
  assert.equal(runs(w).length, 2);
  // Another folder, then back: none starts by itself; Open shell, offered
  // again there, starts one.
  focus(w, 't2');
  await head.tick(500);
  focus(w, 't3');
  await head.tick(500);
  assert.equal(runs(w).length, 2);
  assert.equal(w.shared['corgi.terminal'].note, 'none');
  click(hint.render(), 'terminal-open', hint.cx);
  await hint.settle();
  assert.equal(runs(w).length, 3);
}

// The `>_` mark: on a card whose folder has a shell, also at 190 px; a click
// shows the panel with it (and focuses the agent, as the panel follows).
{
  const w = terminalWindow({ shown: false });
  const { list, head } = await terminalPanel(w, 'terminal-mark');
  assert.equal(byId(list.render(), 'shell-t3'), null);
  w.terminals.push({ address: 't40', state: 'live', labels: { 'corgi.shell': 'panel', 'corgi.shell.dir': `${TREES}/worktree-calm-forest-4f96` } });
  await list.tick(1000);
  for (const px of [268, 190]) {
    w.sidebar = px;
    const root = list.render();
    const mark = byId(root, 'shell-t3');
    assert.ok(mark, `a mark at ${px} px`);
    // On the card's first line, after its task.
    const first = kept(byId(root, 'agent-t3').children[0]);
    assert.ok(first.children.map(kept).includes(mark));
    assert.equal(byId(root, 'shell-t2'), null, 'the root has none');
  }
  click(list.render(), 'shell-t3', list.cx);
  assert.equal(list.cx.stopped, 1, 'the card is not clicked too');
  assert.deepEqual(w.ran.slice(-3), [
    ['agent.focus', { terminal: 't3' }],
    ['layout.select', { stack: 'panel-body', child: 'corgi-terminal' }],
    ['layout.show', { id: 'panel' }],
  ]);
  w.view.hidden = false;
  focus(w, 't3');
  await head.tick(500);
  assert.deepEqual(bodyNow(w), { terminal: 't40' }, 'that shell, no new one');
  assert.equal(runs(w).length, 0);
  assert.deepEqual(kept(byId(list.render(), 'shell-t3')).calls.border_color, ['#accent'], 'lit while the panel shows it');
}

// End shell: asks first, in a card in the header's place; Cancel keeps
// the shell; End ends it (hc kill, then hc rm, as corgi close does): gone
// from hc, no longer tracked, its `>_` mark gone, the panel on the hint
// with Open shell and no shell started by itself, and another folder's
// shell left running.
{
  const w = terminalWindow();
  const { list, head } = await terminalPanel(w, 'terminal-endshell');
  const shells = await import(new URL('../herdr-app/mods/corgi/shells.js?module=terminal-endshell', import.meta.url));
  const calm = `${TREES}/worktree-calm-forest-4f96`;
  const sunny = `${TREES}/worktree-sunny-field-529d`;
  focus(w, 't3');
  await head.tick(500);
  assert.equal(byId(head.render(), 'terminal-end'), null, 'no shell shows: no End shell');
  shells.newShell();
  await head.tick(500);
  assert.deepEqual(bodyNow(w), { terminal: 't20' });
  // Another folder's shell, running a loop.
  w.terminals.push({ address: 't40', state: 'live', labels: { 'corgi.shell': 'panel', 'corgi.shell.dir': sunny } });
  await list.tick(1000);
  assert.ok(byId(list.render(), 'shell-t3'), 'the mark, before');
  const ends = () => w.hcs.filter((a) => a[0] === 'kill' || a[0] === 'rm');

  // Asked first.
  click(head.render(), 'terminal-end', head.cx);
  assert.deepEqual(texts(byId(head.render(), 'terminal-confirm')), ['End this shell? Anything running in it stops.', 'Cancel', 'End']);
  assert.equal(byId(head.render(), 'terminal-hide'), null, 'the card takes the header');
  assert.deepEqual(ends(), [], 'nothing ended while it asks');
  // Cancel keeps it.
  click(head.render(), 'terminal-end-no', head.cx);
  await head.settle();
  assert.equal(byId(head.render(), 'terminal-confirm'), null);
  assert.ok(byId(head.render(), 'terminal-end'));
  assert.deepEqual(ends(), []);
  assert.equal(w.terminals.find((t) => t.address === 't20').state, 'live');
  assert.deepEqual(bodyNow(w), { terminal: 't20' });

  // The panel moves on to another agent's shell while it asks: dropped.
  click(head.render(), 'terminal-end', head.cx);
  focus(w, 't5');
  w.agents.push(agent('sp-sunny', 't5', 'working', '✳ Write the docs'));
  await head.tick(500);
  assert.deepEqual(bodyNow(w), { terminal: 't40' });
  assert.equal(byId(head.render(), 'terminal-confirm'), null, 'not asked about this one');
  focus(w, 't3');
  await head.tick(500);
  assert.deepEqual(bodyNow(w), { terminal: 't20' });

  // End. The body shows the hint before the kill, so the app has no
  // block of an ending terminal to take out.
  let bodyAtKill = null;
  w.hcPrograms.unshift((args) => {
    if (args[0] === 'kill') bodyAtKill = bodyNow(w);
    return undefined;
  });
  click(head.render(), 'terminal-end', head.cx);
  click(head.render(), 'terminal-end-yes', head.cx);
  await head.settle();
  assert.deepEqual(ends(), [['kill', 't20', '--json'], ['rm', 't20', '--json']], 'killed, then removed');
  assert.deepEqual(bodyAtKill, { mod: 'corgi', props: { view: 'terminal-hint' } });
  assert.equal(w.terminals.some((t) => t.address === 't20'), false, 'gone from hc, not left finished');
  assert.equal(byId(head.render(), 'terminal-confirm'), null);
  assert.equal(byId(head.render(), 'terminal-end'), null);
  assert.equal(w.shared['corgi.terminal'].address, null);
  assert.equal(w.shared['corgi.terminal'].note, 'none', 'not "ended": the person asked for it');
  assert.equal(shells.shellIn(calm), null, 'no longer tracked');
  assert.deepEqual(bodyNow(w), { mod: 'corgi', props: { view: 'terminal-hint' } });
  await list.tick(1000);
  assert.equal(byId(list.render(), 'shell-t3'), null, 'the mark is gone');
  const hint = await start(w, { view: 'terminal-hint' }, 'terminal-endshell');
  assert.deepEqual(texts(hint.render()), ['No shell here', 'Open shell']);
  // No new shell by itself, also after the panel was hidden and opened.
  for (let i = 0; i < 3; i++) await head.tick(500);
  assert.equal(runs(w).length, 1);
  assert.deepEqual(bodyNow(w), { mod: 'corgi', props: { view: 'terminal-hint' } });
  // The other folder's shell runs on, still tracked.
  assert.equal(w.terminals.find((t) => t.address === 't40').state, 'live');
  assert.equal(shells.shellIn(sunny), 't40');
  // Open shell: a fresh one.
  click(hint.render(), 'terminal-open', hint.cx);
  await hint.settle();
  assert.equal(runs(w).length, 2);
  assert.deepEqual(bodyNow(w), { terminal: 't21' });
  assert.equal(w.shared['corgi.terminal'].note, null);
}

// End shell while the app is behind: hc lists the shell live for a moment
// after the kill (its removal is tried again, paced), and the hint's write
// loses to other writers, so the app takes the block out when the shell
// ends: it is put back with the hint, no problem said and no shell started.
{
  const w = terminalWindow();
  const { list, head } = await terminalPanel(w, 'terminal-endshell-late');
  const shells = await import(new URL('../herdr-app/mods/corgi/shells.js?module=terminal-endshell-late', import.meta.url));
  focus(w, 't3');
  await head.tick(500);
  shells.newShell();
  await head.tick(500);
  assert.deepEqual(bodyNow(w), { terminal: 't20' });
  // The kill lands two removals later; until then hc says it is live.
  let late = 2;
  let release;
  const held = new Promise((resolve) => { release = resolve; });
  w.hcPrograms.unshift((args) => {
    if (args[0] === 'kill') return held.then(() => ({ stdout: '{}' }));
    if (args[0] === 'rm' && args[1] === 't20' && late-- > 0) return { code: 1, stderr: JSON.stringify({ error: { kind: 'live', message: 't20 is still running' } }) };
    if (args[0] === 'rm' && args[1] === 't20') w.terminals = w.terminals.filter((t) => t.address !== 't20');
    return undefined;
  });
  // Other writers win every write of the hint.
  const write = w.api.writeTextFile;
  let losing = true;
  w.api.writeTextFile = async (path, text, expected) => {
    if (losing && path === TERMINAL_LAYOUT) throw new Error(`${path}: stale snapshot`);
    return write(path, text, expected);
  };
  click(head.render(), 'terminal-end', head.cx);
  click(head.render(), 'terminal-end-yes', head.cx);
  // While the kill is under way: no longer the panel's, though hc lists it live.
  await new Promise((resolve) => setImmediate(resolve));
  assert.equal(w.terminals.find((t) => t.address === 't20').state, 'live');
  assert.equal(shells.shellIn(`${TREES}/worktree-calm-forest-4f96`), null);
  await list.tick(1000);
  assert.equal(byId(list.render(), 'shell-t3'), null, 'the mark goes at once');
  assert.ok(texts(byId(head.render(), 'terminal-confirm')).includes('Ending…'));
  // The shell ends; the app takes its block out of the file.
  losing = false;
  const file = JSON.parse(w.files[TERMINAL_LAYOUT]);
  file.children[1].children[0].children[2].children.pop();
  w.files[TERMINAL_LAYOUT] = JSON.stringify(file);
  w.terminals.find((t) => t.address === 't20').state = 'finished';
  release();
  await head.settle();
  assert.equal(head.cx.slept, 2, 'paced between removals');
  assert.equal(w.terminals.some((t) => t.address === 't20'), false);
  assert.deepEqual(bodyNow(w), { mod: 'corgi', props: { view: 'terminal-hint' } }, 'put back, with the hint');
  assert.equal(w.shared['corgi.terminal'].note, 'none');
  assert.equal(w.shared['corgi.terminal'].problem, null);
  assert.equal(byId(head.render(), 'terminal-confirm'), null);
  await head.tick(500);
  assert.equal(runs(w).length, 1, 'no shell started');
}

// End shell that hc refuses (its core is gone): said in the card, and the
// shell shows again, still tracked.
{
  const w = terminalWindow();
  const { head } = await terminalPanel(w, 'terminal-endshell-refused');
  const shells = await import(new URL('../herdr-app/mods/corgi/shells.js?module=terminal-endshell-refused', import.meta.url));
  focus(w, 't3');
  await head.tick(500);
  shells.newShell();
  await head.tick(500);
  w.hcPrograms.unshift((args) => (args[0] === 'kill' ? { code: 1, stderr: JSON.stringify({ error: { kind: 'io', message: 'core is gone' } }) } : undefined));
  click(head.render(), 'terminal-end', head.cx);
  click(head.render(), 'terminal-end-yes', head.cx);
  await head.settle();
  assert.deepEqual(texts(byId(head.render(), 'terminal-confirm')), ['The shell did not end: core is gone', 'Dismiss']);
  assert.equal(w.terminals.find((t) => t.address === 't20').state, 'live');
  assert.deepEqual(bodyNow(w), { terminal: 't20' }, 'shown again');
  assert.equal(shells.shellIn(`${TREES}/worktree-calm-forest-4f96`), 't20');
  click(head.render(), 'terminal-end-dismiss', head.cx);
  assert.ok(byId(head.render(), 'terminal-end'));
  assert.equal(runs(w).length, 1);
}

console.log('app mods: ok');
