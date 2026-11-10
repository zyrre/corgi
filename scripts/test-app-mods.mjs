// Runs the Herdr app's mods (herdr-app/mods/*) under Node, past the first
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
  static new(options = {}) {
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
    toasts: [],
    commands: [], // `<mod>.<command>`s waiting for the mod, as from a key
    agentOrder: null,
    ...overrides,
  };
  const colors = new Proxy({}, { get: (_t, name) => (typeof name === 'string' ? `#${name}` : undefined) });
  w.api = {
    spaces: () => w.spaces,
    panes: () => w.panes,
    agents: () => w.agents,
    terminals: () => w.terminals,
    machines: () => [{ name: null, layout: 'ready' }],
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
    // The commands sent so far, then none: a mod's loop for them ends.
    nextCommand: () => (w.commands.length ? Promise.resolve(w.commands.shift()) : Promise.reject(new Error('no more commands in a test'))),
    // No folder is watched here: a mod falls back to its timers.
    watchFolder: () => Promise.reject(new Error('no watch in a test')),
    hc: async () => ({ code: 0, stdout: '{"machines":[]}', stderr: '' }),
    exec: async (command, args = [], options = {}) => {
      w.execs.push([command, ...args]);
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
  'setAgentOrder', 'run', 'notify', 'openOverlay', 'home', 'appHome', 'readTextFile', 'watchFolder', 'nextCommand', 'hc', 'exec',
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

let copies = 0;
// A mod, started in the window `w`: its view, and what its tasks and timers are.
async function start(mod, w) {
  globalThis.__world = w;
  // A fresh module each time: a mod keeps state at its top level.
  const module = await import(new URL(`../herdr-app/mods/${mod}/main.js?copy=${copies++}`, import.meta.url));
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
  const view = new module.default();
  // A mod with nothing to set up (the brand row) has no init.
  view.init?.({}, cx);
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
  return { view, cx, settle, tick, render: () => view.render(cx) };
}

// --- reading a rendered view -----------------------------------------------------

function walk(el, visit) {
  const node = kept(el);
  if (node === null || typeof node !== 'object') return;
  visit(node);
  for (const child of node.children ?? []) walk(child, visit);
}

// Every text under `el`, in the order it is drawn.
function texts(el) {
  const out = [];
  const collect = (el) => {
    const node = kept(el);
    if (typeof node === 'string') out.push(node);
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

// The Projects mod with git read for every space.
async function projects(w) {
  const mod = await start('projects', w);
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
  assert.equal(drawn[6], 'Projects');
  // Spaces outside a repository as Spaces shows them; a project with its
  // agents under it, the supervisor first, in place of its worktrees' rows;
  // a worktree with no agent keeps its row.
  assert.deepEqual(ids(root, 'space-').concat(), ['space-sp-corgi', 'space-sp-weather', 'space-sp-quiet', 'space-sp-home']);
  // Above the header, the herd card: the agents by state, a state with
  // none left out (no usage: this corgi does not answer `usage`).
  assert.deepEqual(texts(byId(root, 'herd-card')), ['HERD', '1', '!', '1', '1', '2']);
  assert.deepEqual(ids(root, 'herd-'), ['herd-card', 'herd-working', 'herd-blocked', 'herd-done', 'herd-idle']);
  assert.deepEqual(
    drawn.slice(7),
    [
      'C', 'Corgi',
      'W', 'weather',
      'Supervisor',
      'Fix the cache so that it holds', 'Opus 5.5', '·', '33%',
      'Review the cache fix', '!',
      'Write the docs',
      'worktree-quiet-lake-0a1b',
      '~', '~',
      // A title that is only a folder says nothing: the program's name.
      'Claude Code',
    ],
    drawn.join(' | '),
  );
  assert.deepEqual(ids(root, 'agent-'), ['agent-t2', 'agent-t3', 'agent-t4', 'agent-t5', 'agent-t7']);
  // agent.next and agent.prev follow the list.
  assert.deepEqual(w.agentOrder, ['t2', 't3', 't4', 't5', 't7']);

  // Two lines each. The first: the task, cut with an ellipsis before it
  // reaches the state, which keeps its place at the right edge.
  const row = byId(root, 'agent-t3');
  assert.equal(row.kind, 'v_flex');
  assert.equal(row.children.length, 2);
  const [first, second] = row.children.map(kept);
  assert.equal(first.children.length, 2);
  const [task, state] = first.children.map(kept);
  assert.deepEqual(texts(task), ['Fix the cache so that it holds']);
  assert.ok('truncate' in task.calls, 'the task is cut with an ellipsis');
  assert.deepEqual(task.calls.min_w, [0], 'the task can be cut to nothing');
  assert.ok('flex_1' in task.calls, 'the task fills the line, so the state stands at its right edge');
  assert.ok('flex_none' in state.calls, 'the state stays whole');
  assert.deepEqual(kept(state.children[0]).args, ['mods/workspace-chrome/spinner.svg']);
  assert.ok('spin' in kept(state.children[0]).calls);
  // The second, dim and smaller: the harness's icon, the model and the
  // context fill, rounded.
  assert.deepEqual(second.calls.text_size, [11]);
  assert.deepEqual(second.calls.text_color, ['#dim']);
  const [pic, model, dot, ctx] = second.children.map(kept);
  assert.equal(second.children.length, 4);
  assert.deepEqual(pic.args, ['icons/claude.svg']);
  assert.ok('flex_none' in pic.calls, 'the icon stays whole');
  assert.deepEqual(texts(model), ['Opus 5.5']);
  assert.ok('truncate' in model.calls);
  assert.deepEqual([texts(dot), texts(ctx)], [['·'], ['33%']]);
  // The worktree's name is no text of the row, only its tooltip.
  assert.deepEqual(row.calls.tooltip, ['worktree-calm-forest-4f96']);
  assert.ok(!texts(row).includes('worktree-calm-forest-4f96'));
  // What Corgi does not know is left out, with nothing in its place.
  assert.deepEqual(byId(root, 'agent-t4').children.map((child) => texts(child)), [['Review the cache fix', '!'], []], 'a blocked agent');
  assert.equal(kept(byId(root, 'agent-t4').children[1]).children.length, 1, 'only the icon');
  const t5 = byId(root, 'agent-t5');
  assert.deepEqual(texts(t5), ['Write the docs']);
  assert.deepEqual(t5.calls.tooltip, ['worktree-sunny-field-529d']);
  // An agent whose program has no icon keeps the rest.
  w.agents[2].program = { ...CLAUDE, icon: null };
  assert.equal(kept(byId(mod.render(), 'agent-t4').children[1]).children.length, 0);
  w.agents[2].program = CLAUDE;
  // On the primary checkout there is no worktree to name.
  assert.deepEqual(texts(byId(root, 'agent-t2')), ['Supervisor']);
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

  // Corgi is read as often as the Corgi mod reads it, and no more; plan
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
  assert.deepEqual(texts(kept(byId(mod.render(), 'agent-t3').children[1])), ['33%']);
}

// What is lit: only the agent with the focus, never a project's row with
// it; a project's row is not lit at all; a space outside any project is
// while none of its agents has the focus.
{
  const lit = (root) => [...ids(root, 'space-'), ...ids(root, 'agent-')].filter((id) => 'bg' in byId(root, id).calls);
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
  let root = mod.render();
  assert.ok('bg' in byId(root, 'agent-t3').calls, 'the first agent of the shown worktree');
  assert.ok(!('bg' in byId(root, 'agent-t4').calls));
  w.agents[2].focused = true;
  root = mod.render();
  assert.ok(!('bg' in byId(root, 'agent-t3').calls));
  assert.ok('bg' in byId(root, 'agent-t4').calls, 'the agent with the focus');
}

// Without corgi the list still shows every space and agent, by its own title.
{
  const w = window();
  w.programs = [git];
  const mod = await projects(w);
  const root = mod.render();
  const drawn = texts(root);
  // The herd card counts the app's agents alone: with no usage, no plan lines.
  assert.deepEqual(texts(byId(root, 'herd-card')), ['HERD', '1', '!', '1', '1', '1']);
  assert.deepEqual(
    drawn.slice(9),
    ['W', 'weather', 'Weather supervisor', 'Fixing the cache', 'Review the cache fix', '!', 'worktree-sunny-field-529d', 'worktree-quiet-lake-0a1b', '~', '~', 'Claude Code'],
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
  assert.deepEqual(texts(root).slice(0, 7), ['HERD', '1', '!', '1', '1', '2', 'Projects']);
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
    'HERD', '1', '!', '1', '1', '2',
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
  // The windows in the dashboard's colors.
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
  assert.deepEqual(ids(card2, 'herd-'), ['herd-card', 'herd-blocked', 'herd-merge', 'herd-done', 'herd-idle']);
  assert.equal(colorOf(card2, '64%'), '#ansi3');

  // The rail has no room for it.
  tagged.sidebar = 76;
  assert.equal(byId(again.render(), 'herd-card'), null);
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
  assert.equal(kept(mark.children[0]).kind, 'svg');
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

// `projects.open`, as a key of the person's sends it, is the +.
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
  // The Corgi space, which used to install and update corgi, is retired.
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
  assert.deepEqual(row.children.map((child) => texts(child)), [['Fix the cache so that it holds'], ['Opus 5.5', '·', '33%', 'MERGE']]);
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

  // Confirmed, it runs the merge the dashboard's m runs, and says so.
  click(mod.render(), 'merge-t3', mod.cx);
  click(mod.render(), 'merge-confirm-t3', mod.cx);
  await new Promise((resolve) => setImmediate(resolve));
  assert.deepEqual(merges(w), [['w-cache', '--project', REPO, '--json']]);
  assert.deepEqual(texts(panel(mod)), ['Merging…']);
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
    // Asked: the line the dashboard's s sends, with the failure alone.
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
  const mod = await start('corgi-brand', w);
  const root = mod.render();
  assert.deepEqual(texts(root), ['Herdr', '+', 'Corgi']);
  assert.deepEqual(pictures(root), ['frame/welcome/herdr.png', 'mods/corgi-brand/corgi.png', 'mods/corgi-brand/search.svg', 'mods/corgi-brand/sidebar.svg']);
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
  assert.deepEqual(pictures(tight), ['frame/welcome/herdr.png', 'mods/corgi-brand/search.svg', 'mods/corgi-brand/sidebar.svg']);
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
  assert.deepEqual(pictures(other), ['mods/corgi-brand/sidebar.svg']);
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
const ICONS = { 'mods/projects/pause.svg': '⏸', 'mods/projects/play.svg': '▶' };
const header = (root) => {
  const button = byId(root, 'agents-pause');
  if (!button) return null;
  const icon = kept(button.children[0]);
  return [`${ICONS[icon.args[0]]} ${texts(button).join('')}`];
};
// The state mark at the right of an agent's first line.
const markOf = (root, id) => {
  const first = kept(byId(root, id).children[0]);
  const mark = kept(kept(first.children[1]).children[0]);
  return mark.kind === 'svg' ? ICONS[mark.args[0]] ?? mark.args[0] : texts(mark).join('');
};

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
  assert.deepEqual(ids(root, 'agent-'), ['agent-t2', 'agent-t20', 'agent-t3', 'agent-t4', 'agent-t5', 'agent-t7']);
  assert.deepEqual(texts(kept(byId(root, 'agent-t5').children[0])), ['Write the docs']);
  assert.equal(markOf(root, 'agent-t5'), '⏸');
  assert.deepEqual(texts(kept(byId(root, 'agent-t20').children[0])), ['Fix the radar']);
  assert.equal(markOf(root, 'agent-t20'), '⏸');
  assert.deepEqual(texts(kept(byId(root, 'agent-t20').children[1])), ['Paused'], 'its terminal ended: no program or model to show');
  assert.equal(markOf(root, 'agent-t3'), 'mods/workspace-chrome/spinner.svg', 'a working row keeps its spinner');
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

// --- Corgi's layout: the installer changes it, never a mod ------------------------------

// Neither mod runs `corgi app-layout` (it would clone and switch layouts),
// switches the window's layout or says anything of the layout.
{
  const w = window();
  const mod = await projects(w);
  await mod.tick(5000);
  assert.ok(!w.execs.some((call) => call[1] === 'app-layout'), JSON.stringify(w.execs));
  assert.ok(!w.ran.some(([name]) => name.startsWith('layout.')));
  assert.deepEqual(w.toasts, []);
}
{
  const versions = { '--version': { stdout: 'corgi 9.9.9\n' }, status: (args) => (args[1] === '--help' ? { stdout: 'Usage: corgi status --json' } : STATUS) };
  const w = window(versions);
  const mod = await start('corgi', w);
  await mod.tick(5000);
  assert.ok(!w.execs.some((call) => call[1] === 'app-layout'), JSON.stringify(w.execs));
  assert.ok(!w.ran.some(([name]) => name.startsWith('layout.')));
  assert.deepEqual(w.toasts, []);
  const drawn = texts(mod.render());
  assert.ok(drawn.includes('weather'), drawn.join(' | '));
  assert.ok(!drawn.some((text) => text.startsWith('Corgi layout')));
}

console.log('app mods: ok');
