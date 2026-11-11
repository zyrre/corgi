// Runs the Jenkins mod under Node against the made-up Jenkins of
// fake-jenkins.mjs, past the first frame `herdr-app check` renders: with
// stand-ins for the app's `gpui-kit`, `gpui-base` and `herdr` modules, a
// scratch folder as the app's home, and Node's own `fetch` in place of the
// app's. A stand-in element keeps its texts, children and handlers, so a
// test reads what a view shows and clicks and types in it. Nothing here asks
// a real Jenkins or a real app.
//
//   node herdr-app/mods/jenkins/tests/test.mjs

import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { execFileSync, spawnSync } from 'node:child_process';
import fs from 'node:fs';
import { registerHooks } from 'node:module';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { startFakeJenkins, CRUMB } from './fake-jenkins.mjs';

const MOD = path.dirname(path.dirname(fileURLToPath(import.meta.url)));
const TOKEN = 'fake-token';

// Time the tests move: the mod and the made-up Jenkins both read Date.now.
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

// The retained text of an input: `type` is the person typing, `submit` Enter.
class InputState {
  static new(options = {}) {
    return new InputState(options);
  }
  constructor(options) {
    this.text = options.value ?? '';
    this.placeholder = options.placeholder ?? '';
    this.handlers = {};
    this.focused = 0;
    this.masked = false;
  }
  value() {
    return this.text;
  }
  set_value(next) {
    this.text = next;
  }
  set_masked(masked) {
    this.masked = masked;
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
  svg: (file) => element('svg', [file]),
  v_flex: () => element('v_flex'),
  h_flex: () => element('h_flex'),
  InputState,
  Input: { new: (state) => element('input', [state]) },
  // Every row is drawn: a test reads the whole list.
  v_virtual_list: (id, count, _size, _key, render) => {
    const list = element('v_virtual_list', [id, count]);
    list.children(render({ start: 0, end: count }));
    return list;
  },
  Scrollbar: { vertical: (id) => element('scrollbar', [id]) },
  VirtualListScrollHandle: { new: () => ({ bottoms: 0, scroll_to_bottom() { this.bottoms += 1; } }) },
};

// The app around the mod: its home (a scratch folder), what the environment
// holds, the commands waiting for the mod.
function world(overrides = {}) {
  const w = {
    home: fs.mkdtempSync(path.join(os.tmpdir(), 'jenkins-mod-')),
    env: '', // what `sh -c printf …` answers: the JENKINS_* variables, a line each
    pending: 0, // tasks of the mod still at work
    waiter: null,
    opened: [], // URLs handed to the browser
    execs: [],
    shared: {}, // what the app keeps for every mod across restarts (shared, setShared)
    props: null, // the props of the block showing the mod: props() gives them, init gets none
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
    // The next command sent (`mod.send`); until one is, the mod's loop for them waits and counts as at rest.
    nextCommand: () => new Promise((resolve) => {
      w.pending -= 1;
      w.waiter = (command) => {
        w.pending += 1;
        resolve(command);
      };
    }),
    props: () => w.props,
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
      w.execs.push([command, ...args]);
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
  'gpui-base': 'const k = globalThis.__kit; export const v_flex = k.v_flex, h_flex = k.h_flex, InputState = k.InputState, Input = k.Input, v_virtual_list = k.v_virtual_list, Scrollbar = k.Scrollbar, VirtualListScrollHandle = k.VirtualListScrollHandle;',
  herdr: ['theme', 'settings', 'home', 'appHome', 'nextCommand', 'props', 'shared', 'setShared', 'readTextFile', 'writeTextFile', 'exec']
    .map((name) => `export const ${name} = (...a) => globalThis.__world.api.${name}(...a);`).join('\n'),
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
    return next(url, url.includes('/mods/jenkins/') && url.split('?')[0].endsWith('.js') ? { ...context, format: 'module' } : context);
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
    if (!parent?.pathname.includes('/mods/jenkins/') || !parent.search || !specifier.startsWith('.')) return resolved;
    const url = new URL(resolved.url);
    url.search = parent.search;
    return { ...resolved, url: url.href };
  },
});

const util = await import(new URL('../util.js', import.meta.url));
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
    stopped: 0,
    notify() {
      this.notified += 1;
    },
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
    stop_propagation() {
      this.stopped += 1;
    },
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
  const mod = {
    view,
    cx,
    settle,
    render: () => view.render(cx),
    // One round of the mod's timer.
    tick: async () => {
      for (const timer of cx.timers) timer.tick(cx);
      await settle();
    },
    // One round of it without waiting for what it starts (a read that stalls never ends).
    poke: async () => {
      for (const timer of cx.timers) timer.tick(cx);
      for (let i = 0; i < 20; i++) await new Promise((resolve) => setImmediate(resolve));
    },
    // A click on the element with this id, and what it started.
    click: async (id) => {
      const el = byId(mod.render(), id);
      assert.ok(el, `nothing with the id ${id}; there is ${ids(mod.render(), 'jk-').join(', ')}`);
      assert.ok(el.handlers.on_click, `${id} takes no click`);
      el.handlers.on_click({}, cx);
      await settle();
    },
    // A `jenkins.<command>`, as from a key in keys.json.
    send: async (command) => {
      w.waiter(command);
      await settle();
    },
    key: async (key, modifiers = {}) => {
      kept(mod.render()).handlers.on_key_down({ key, modifiers }, cx);
      await settle();
    },
    // Typing into an input, and Enter in it.
    type: (input, text) => {
      input.set_value(text);
      input.handlers.change?.({}, cx);
    },
    submit: async (input) => {
      input.handlers.submit({}, cx);
      await settle();
    },
    texts: () => texts(mod.render()),
    says: (text) => mod.texts().some((line) => line.includes(text)),
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

const spinners = (el) => {
  let n = 0;
  walk(el, (node) => {
    if ('frames' in node.calls) n += 1;
    assert.ok(!('spin' in node.calls), 'no turning svg: spinners are braille frames');
  });
  return n;
};

// The text of the row with this id, on one line.
const row = (mod, id) => {
  const el = byId(mod.render(), id);
  assert.ok(el, `no row ${id}; there is ${ids(mod.render(), 'jk-').join(', ')}`);
  return texts(el).join(' ');
};

// --- the cases ------------------------------------------------------------------

const cases = [];
const test = (name, run) => cases.push({ name, run });

// A world whose settings are saved already, for the made-up Jenkins.
function configured(fake, settings = {}) {
  const w = world();
  fs.mkdirSync(path.join(w.app, 'jenkins'), { recursive: true });
  fs.writeFileSync(path.join(w.app, 'jenkins', 'settings.json'), JSON.stringify({ url: fake.url, user: 'jane', token: TOKEN, folders: 'team-dev, team-prod', ...settings }));
  return w;
}
const posts = (fake) => fake.requests.filter((r) => r.method === 'POST');
const item = (names) => `jk-item-${names.map((n) => `/job/${encodeURIComponent(n)}`).join('')}`;

test('the helpers: addresses, bytes, states, parameters, console text', () => {
  const base = 'https://jenkins.example.com';
  assert.equal(util.normalizeBase(' jenkins.example.com/ '), base);
  assert.equal(util.normalizeBase('http://localhost:8080/jenkins//'), 'http://localhost:8080/jenkins');
  assert.equal(util.hostOf(base), 'jenkins.example.com');
  assert.deepEqual(util.parsePlaces('', base), [{ path: '', label: '' }]);
  assert.deepEqual(util.parsePlaces('team-dev, team/tools', base), [{ path: '/job/team-dev', label: 'team-dev' }, { path: '/job/team/job/tools', label: 'team/tools' }]);
  assert.deepEqual(util.parsePlaces(`job/team/job/tools/\n${base}/job/acme-develop/, view/Deploys, team-dev,team-dev`, base), [
    { path: '/job/team/job/tools', label: 'team/tools' }, { path: '/job/acme-develop', label: 'acme-develop' },
    { path: '/view/Deploys', label: 'Deploys' }, { path: '/job/team-dev', label: 'team-dev' },
  ]);
  assert.deepEqual(util.childOf({ path: '/job/a', label: 'a' }, 'b c'), { path: '/job/a/job/b%20c', label: 'a/b c' });

  for (const text of ['', 'a', 'ab', 'abc', 'jane:fake-token', 'räksmörgås:tök€n', '日本語 🐕']) {
    assert.equal(util.base64(util.utf8(text)), Buffer.from(text, 'utf8').toString('base64'), text);
    assert.equal(util.utf8Length(text), Buffer.byteLength(text, 'utf8'), text);
  }
  assert.equal(util.basicAuth('jane', 'tok'), `Basic ${Buffer.from('jane:tok').toString('base64')}`);

  assert.equal(util.jobState({ color: 'blue', lastBuild: { result: 'SUCCESS' } }), 'success');
  assert.equal(util.jobState({ color: 'red_anime', lastBuild: { building: true, result: null } }), 'running');
  assert.equal(util.jobState({ color: 'blue', inQueue: true, lastBuild: { result: 'SUCCESS' } }), 'queued');
  assert.equal(util.jobState({ color: 'yellow' }), 'unstable');
  assert.equal(util.jobState({ color: 'disabled', lastBuild: { result: 'SUCCESS' } }), 'disabled');
  assert.equal(util.jobState({ color: 'notbuilt' }), 'notbuilt');
  assert.equal(util.buildState({ building: false, result: 'ABORTED' }), 'aborted');
  assert.ok(util.isFolder({ _class: 'com.cloudbees.hudson.plugins.folder.Folder', name: 'x' }));
  assert.ok(util.isFolder({ _class: 'org.jenkinsci.plugins.workflow.multibranch.WorkflowMultiBranchProject', name: 'x' }));
  assert.ok(!util.isFolder({ _class: 'org.jenkinsci.plugins.workflow.job.WorkflowJob', color: 'blue' }));
  assert.equal(util.progress({ building: true, timestamp: 1000, estimatedDuration: 4000 }, 2000), 0.25);
  assert.equal(util.progress({ building: true, timestamp: 1000, estimatedDuration: -1 }, 2000), null);
  assert.equal(util.progress({ building: true, timestamp: 1000, estimatedDuration: 10 }, 9000), 0.99);
  assert.equal(util.ago(Date.now() - 3 * 3600_000), '3h');
  assert.equal(util.span(365_000), '6m 05s');

  const fields = util.paramFields({ property: [{}, { parameterDefinitions: [
    { name: 'branch', type: 'StringParameterDefinition', defaultParameterValue: { value: 'master' }, description: 'two\nlines' },
    { name: 'site', type: 'ChoiceParameterDefinition', choices: ['a', 'b'], defaultParameterValue: { value: 'b' } },
    { name: 'dry', type: 'BooleanParameterDefinition', defaultParameterValue: { value: true } },
    { name: 'secret', type: 'PasswordParameterDefinition', defaultParameterValue: {} },
    { name: 'file', type: 'FileParameterDefinition' },
  ] }] });
  assert.deepEqual(fields.map((f) => [f.name, f.kind, f.value]), [['branch', 'text', 'master'], ['site', 'choice', 'b'], ['dry', 'boolean', true], ['secret', 'password', ''], ['file', 'other', '']]);
  assert.equal(fields[0].description, 'two lines');
  assert.equal(util.formBody(fields, { branch: 'feature/a b&c', site: 'a', dry: false, secret: '' }), 'branch=feature%2Fa%20b%26c&site=a&dry=false');
  assert.equal(util.formBody(fields, { branch: '', site: 'a', dry: true, secret: 'p w' }), 'branch=&site=a&dry=true&secret=p%20w');

  let log = util.appendLog(util.emptyLog(), 'one\ntw');
  assert.deepEqual([log.lines, log.partial, log.offset], [['one'], 'tw', 6]);
  log = util.appendLog(log, 'o\n\x1b[31mred\x1b[0m\t\x1b[8mha:AAAA\x1b[0mseen\r\n');
  assert.deepEqual(log.lines, ['one', 'two', 'red    seen']);
  assert.deepEqual(util.logLines(util.appendLog(log, 'half')), ['one', 'two', 'red    seen', 'half']);
  const long = util.appendLog(util.emptyLog(), `${Array.from({ length: util.MAX_LINES + 7 }, (_v, i) => `l${i}`).join('\n')}\n`);
  assert.deepEqual([long.lines.length, long.dropped, long.lines[0]], [util.MAX_LINES, 7, 'l7']);

  assert.match(util.explain({ kind: 'forbidden', detail: 'viewer is missing the Job/Build permission' }, 'build api', 'viewer'), /^This token may not build api: Jenkins user viewer lacks the permission \(viewer is missing/);
  assert.match(util.explain({ kind: 'auth' }, 'x', 'jane'), /did not accept this user and token \(401\)/);

  assert.deepEqual(['acme-develop', 'acme-master', 'team-dev', 'team/team-prod', 'live', 'devops', 'tools', 'masterclass'].map(util.envOf),
    ['dev', 'prod', 'dev', 'prod', 'prod', '', '', '']);
});

test('the stage helpers: states, a stage\'s lines in the console, the ones to come', () => {
  assert.deepEqual(['SUCCESS', 'FAILED', 'IN_PROGRESS', 'PAUSED_PENDING_INPUT', 'NOT_EXECUTED', 'ABORTED', 'UNSTABLE', 'WHAT'].map(util.stageState),
    ['success', 'failed', 'running', 'paused', 'skipped', 'aborted', 'unstable', 'notbuilt']);
  assert.ok(util.isPipeline({ _class: 'org.jenkinsci.plugins.workflow.job.WorkflowJob' }) && !util.isPipeline({ _class: 'hudson.model.FreeStyleProject' }));

  // As a real Jenkins writes them: blocks inside a stage, a stage of the same name twice, parallel branches.
  const lines = [
    'Started by user Jane Doe', '[Pipeline] node', '[Pipeline] {',
    '[Pipeline] stage', '[Pipeline] { (Build)', '[Pipeline] withEnv', '[Pipeline] {', '+ make', '[Pipeline] }', '[Pipeline] // withEnv', 'built', '[Pipeline] }', '[Pipeline] // stage',
    '[Pipeline] stage', '[Pipeline] { (Deploy)', 'to north', '[Pipeline] }', '[Pipeline] // stage',
    '[Pipeline] stage', '[Pipeline] { (Deploy)', '[Pipeline] parallel', '[Pipeline] [a] { (Branch: a)', '[a] hello', '[Pipeline] [a] }', 'still going',
  ];
  assert.deepEqual(util.stageRange(lines, 'Build'), { from: 5, to: 11, open: false });
  assert.deepEqual(lines.slice(5, 11), ['[Pipeline] withEnv', '[Pipeline] {', '+ make', '[Pipeline] }', '[Pipeline] // withEnv', 'built']);
  const first = util.stageRange(lines, 'Deploy', 0);
  assert.deepEqual(lines.slice(first.from, first.to), ['to north']);
  assert.deepEqual(util.stageRange(lines, 'Deploy', 1), { from: 20, to: lines.length, open: true }, 'the second, still running');
  assert.deepEqual(util.stageRange(lines, 'Branch: a'), { from: 22, to: 23, open: false });
  assert.equal(util.stageRange(lines, 'Test'), null);
  assert.equal(util.stageRange(lines, 'Deploy', 2), null);

  // What a running build's stages are next to the job's last finished run.
  const describe = { id: '9', status: 'IN_PROGRESS', stages: [{ id: '4', name: 'Build', status: 'SUCCESS', durationMillis: 60_000 }, { id: '9', name: 'Test', status: 'IN_PROGRESS', startTimeMillis: 1_000, durationMillis: 5_000 }] };
  const runs = [{ id: '9', status: 'IN_PROGRESS', stages: [] }, { id: '8', status: 'FAILED', stages: [{ name: 'Build', durationMillis: 1 }] },
    { id: '7', status: 'SUCCESS', stages: [{ name: 'Build', durationMillis: 50_000 }, { name: 'Test', durationMillis: 40_000 }, { name: 'Deploy', durationMillis: 30_000 }] }];
  const stages = util.stagesOf(describe, runs, true);
  assert.deepEqual(stages.map((s) => [s.name, s.state, s.expected, s.nth]), [['Build', 'success', 50_000, 0], ['Test', 'running', 40_000, 0], ['Deploy', 'pending', 30_000, 0]]);
  assert.equal(util.stageProgress(stages[1], 11_000), 0.25);
  assert.equal(util.stageTook(stages[1], 11_000), '10s', 'a running stage counts from its start');
  assert.deepEqual(util.stagesOf(describe, runs, false).map((s) => s.name), ['Build', 'Test'], 'none to come once it has ended');
  assert.deepEqual(util.stagesOf({ stages: [{ name: 'A' }, { name: 'A' }] }).map((s) => s.nth), [0, 1]);
});

test('with no settings it asks for them, checks who the token is, and keeps them where only the owner reads', async () => {
  const fake = await startFakeJenkins();
  const w = world();
  const mod = await start(w);
  const file = path.join(w.app, 'jenkins', 'settings.json');
  assert.ok(mod.says('Jenkins URL') && mod.says('API token'), 'the settings show first');
  assert.equal(fake.requests.length, 0);
  assert.ok(mod.view.inputs.token.masked, 'the token is drawn as a password');
  assert.equal(byId(mod.render(), 'jk-set-cancel'), null, 'nothing to cancel to yet');

  await mod.click('jk-set-save');
  assert.ok(mod.says('The Jenkins URL, the user name and the token are all needed.'));
  assert.ok(!fs.existsSync(file));

  mod.type(mod.view.inputs.url, `${fake.url}/`);
  mod.type(mod.view.inputs.user, 'jane');
  mod.type(mod.view.inputs.token, 'not-the-token');
  await mod.click('jk-set-check');
  assert.ok(mod.says('Jenkins did not accept this user and token (401). Check them in the settings.'), mod.texts().join('|'));
  assert.ok(!fs.existsSync(file), 'a check saves nothing');

  assert.ok(!mod.says('http:// sends the token unencrypted'), 'this computer is no network');
  mod.type(mod.view.inputs.url, 'http://jenkins.example.com');
  assert.ok(mod.says('http:// sends the token unencrypted'));
  mod.type(mod.view.inputs.url, `${fake.url}/`);

  mod.type(mod.view.inputs.token, TOKEN);
  await mod.click('jk-set-check');
  assert.ok(mod.says('Authenticated as Jane Doe (jane)'));
  assert.ok(!fs.existsSync(file));
  assert.ok(mod.says('Jenkins URL'), 'still the settings');

  mod.type(mod.view.inputs.folders, 'team-prod');
  await mod.submit(mod.view.inputs.folders); // Enter saves
  assert.deepEqual(JSON.parse(fs.readFileSync(file, 'utf8')), { url: fake.url, user: 'jane', token: TOKEN, folders: 'team-prod' });
  assert.equal(fs.statSync(file).mode & 0o777, 0o600, 'only its owner reads the token');
  assert.ok(mod.says('team-prod') && !mod.says('Jenkins URL'), 'on to the jobs');
  assert.ok(row(mod, item(['team-prod', 'api_master'])).includes('#64'));
  assert.ok(!mod.texts().join('\n').includes(TOKEN), 'the token is never drawn');
  assert.ok(fake.requests.every((r) => r.user === 'jane' || r.status === 401), 'every request carries the user and token');

  // The gear shows them again, preset; Cancel and Esc go back.
  await mod.click('jk-settings');
  assert.equal(mod.view.inputs.url.value(), fake.url);
  assert.equal(mod.view.inputs.token.value(), TOKEN);
  await mod.click('jk-set-cancel');
  assert.ok(!mod.says('Jenkins URL'));

  // A saved token Jenkins refuses: saved all the same, and the settings stay with the reason.
  await mod.click('jk-settings');
  mod.type(mod.view.inputs.token, 'revoked');
  await mod.click('jk-set-save');
  assert.equal(JSON.parse(fs.readFileSync(file, 'utf8')).token, 'revoked');
  assert.ok(mod.says('Jenkins did not accept this user and token (401)') && mod.says('Jenkins URL'));
  await fake.close();

  // The first save of all, with a token Jenkins refuses: the reason shows, not an empty list.
  const again = await startFakeJenkins();
  const fresh = await start(world());
  fresh.type(fresh.view.inputs.url, again.url);
  fresh.type(fresh.view.inputs.user, 'jane');
  fresh.type(fresh.view.inputs.token, 'mistyped');
  await fresh.click('jk-set-save');
  assert.ok(fresh.says('Jenkins did not accept this user and token (401)') && fresh.says('API token'), fresh.texts().join('|'));
  await again.close();
});

test('with nothing saved it takes JENKINS_URL, JENKINS_USER and JENKINS_API_TOKEN, when the app has them', async () => {
  const fake = await startFakeJenkins();
  const w = world({ env: `${fake.url}\njane\n${TOKEN}\nteam-prod` });
  const mod = await start(w);
  assert.ok(row(mod, item(['team-prod', 'poller_master'])).includes('#31'));
  assert.ok(!fs.existsSync(path.join(w.app, 'jenkins', 'settings.json')), 'nothing is saved by itself');
  await mod.click('jk-settings');
  assert.ok(mod.says('These came from JENKINS_URL, JENKINS_USER and JENKINS_API_TOKEN; Save keeps them here.'));
  await fake.close();
});

test('the list: each folder\'s jobs with their last build, and a folder that is not there', async () => {
  const fake = await startFakeJenkins();
  const w = configured(fake, { folders: 'team-dev, no-such-folder, team-prod' });
  const mod = await start(w);
  const all = mod.texts();
  assert.ok(all.some((line) => line === `127.0.0.1:${fake.port} · jane`), 'the host and who the token is');
  assert.equal(byId(mod.render(), 'jk-who').calls.tooltip[0], 'Authenticated as Jane Doe (jane): what this Jenkins user may do, this panel may do');

  assert.equal(row(mod, item(['team-dev', 'api_master'])), 'api_master #128 · 3h');
  assert.match(row(mod, item(['team-dev', 'poller_master'])), /^poller_master #57 3\d%$/, 'a running build shows how far it is');
  assert.equal(spinners(byId(mod.render(), item(['team-dev', 'poller_master']))), 1);
  assert.equal(row(mod, item(['team-dev', 'parser_master'])), 'parser_master #301 · 42m');
  assert.equal(row(mod, item(['team-dev', 'reports_master'])), 'reports_master #19 · 3d');
  assert.equal(row(mod, item(['team-dev', 'new-service_master'])), 'new-service_master never built');
  assert.equal(row(mod, item(['team-dev', 'legacy_master'])), 'legacy_master disabled');
  assert.equal(row(mod, item(['team-dev', 'tools'])), 'tools');
  const icons = (id) => { const out = []; walk(byId(mod.render(), id), (n) => { if (n.kind === 'svg') out.push(path.basename(n.args[0], '.svg')); }); return out; };
  assert.deepEqual(icons(item(['team-dev', 'api_master'])), ['success']);
  assert.deepEqual(icons(item(['team-dev', 'parser_master'])), ['failed']);
  assert.deepEqual(icons(item(['team-dev', 'alarm-notify_master'])), ['unstable']);
  assert.deepEqual(icons(item(['team-dev', 'reports_master'])), ['aborted']);
  assert.deepEqual(icons(item(['team-dev', 'tools'])), ['folder', 'chevron-right']);
  for (const file of new Set(['jenkins', 'refresh', 'settings', 'external', ...ids(mod.render(), 'jk-item-').flatMap(icons)])) {
    assert.ok(fs.existsSync(path.join(MOD, 'icons', `${file}.svg`)), `icons/${file}.svg`);
  }

  assert.ok(mod.says('Jenkins has nothing there to list the jobs of no-such-folder (404): it is gone, or this token may not see it.'));
  assert.equal(row(mod, item(['team-prod', 'api_master'])), 'api_master #64 · 1d', 'the folders after it still list');
  const listed = fake.requests.filter((r) => r.path === '/job/team-dev/api/json');
  assert.equal(listed.length, 1);
  assert.ok(listed[0].query.tree.startsWith('jobs[_class,name,color,inQueue,lastBuild['), 'only the fields it draws are asked for');
  assert.equal(posts(fake).length, 0);

  // While a build runs the lists are read again every ten seconds; at rest, every minute.
  const reads = () => fake.requests.filter((r) => r.path === '/job/team-dev/api/json').length;
  const prod = () => fake.requests.filter((r) => r.path === '/job/team-prod/api/json').length;
  await mod.tick();
  assert.deepEqual([reads(), prod()], [1, 1], 'nothing is due yet');
  skew += 11_000;
  await mod.tick();
  assert.deepEqual([reads(), prod()], [2, 2], 'team-prod has a build that waits for input');
  skew += 60_000;
  await mod.tick();
  assert.deepEqual([reads(), prod()], [3, 3]);

  // Each folder of the settings is a large heading, tagged DEV or PROD when
  // its name says so, with how many jobs run, failed and are there.
  const head = (place) => `jk-head-/job/${place}`;
  assert.equal(row(mod, head('team-dev')), 'team-dev DEV 1 1 9');
  assert.equal(row(mod, head('team-prod')), 'team-prod PROD 2 4', 'its two builds that wait for input are not counted as running');
  let big = 0;
  walk(byId(mod.render(), head('team-dev')), (n) => { if (n.calls.text_size?.[0] === 18 && n.children.includes('team-dev')) big += 1; });
  assert.equal(big, 1, 'its name is large');
  assert.equal(row(mod, head('no-such-folder')), 'no-such-folder', 'no tag for a name that says neither');
  assert.deepEqual(icons(head('team-dev')).slice(0, 1), ['chevron-down']);

  // A click folds it, and the app keeps that: a panel started again finds it folded.
  await mod.click(head('team-dev'));
  assert.equal(byId(mod.render(), item(['team-dev', 'api_master'])), null);
  assert.equal(row(mod, head('team-dev')), 'team-dev DEV 1 1 9', 'folded, it still says how its jobs are');
  assert.deepEqual(icons(head('team-dev')).slice(0, 1), ['chevron-right']);
  assert.deepEqual(w.shared['jenkins.collapsed'], ['/job/team-dev']);
  const again = await start(w);
  assert.equal(byId(again.render(), item(['team-dev', 'api_master'])), null);
  assert.ok(byId(again.render(), item(['team-prod', 'api_master'])));
  await again.click(head('team-dev'));
  assert.ok(byId(again.render(), item(['team-dev', 'api_master'])));
  assert.deepEqual(w.shared['jenkins.collapsed'], []);
  await mod.click(head('team-dev'));

  // A folder in the list opens as a list of its own; the browser gets the folder's page.
  await mod.click(item(['team-dev', 'tools']));
  assert.equal(row(mod, item(['team-dev', 'tools', 'rotate-certs'])), 'rotate-certs #7 · 10h');
  await mod.click('jk-open-/job/team-dev/job/tools');
  assert.deepEqual(w.opened, [`${fake.url}/job/team-dev/job/tools/`]);
  await mod.key('escape');
  assert.ok(mod.says('team-prod'));
  await fake.close();
});

// The stage rows of a job view, each on one line, and the icon of one.
const stageRows = (mod) => ids(mod.render(), 'jk-stage-').map((id) => row(mod, id));
const iconsOf = (mod, id) => {
  const out = [];
  walk(byId(mod.render(), id), (n) => { if (n.kind === 'svg') out.push(path.basename(n.args[0], '.svg')); });
  return out;
};
const consoleReads = (fake) => fake.requests.filter((r) => r.path.endsWith('/logText/progressiveText')).length;

test('a job: its newest build\'s stages, another build\'s, a stage\'s part of the log and the whole log', async () => {
  const fake = await startFakeJenkins();
  const w = configured(fake);
  const mod = await start(w);
  await mod.click(item(['team-dev', 'api_master']));
  assert.ok(mod.says('Builds the API agent and rolls it out to dev'));
  assert.equal(row(mod, 'jk-build-128'), '#128 gitBranch=master 6m 00s · 3h');
  assert.equal(row(mod, 'jk-build-127'), '#127 gitBranch=feature/export-columns 4m 00s · 1d');
  assert.equal(row(mod, 'jk-build-126'), '#126 Started by user Jane Doe 6m 00s · 2d');
  assert.equal(byId(mod.render(), 'jk-build-128-abort'), null, 'a finished build has nothing to abort');
  const read = fake.requests.find((r) => r.path === '/job/team-dev/job/api_master/api/json');
  assert.ok(read.query.tree.includes('builds[') && read.query.tree.endsWith('{0,20}'));

  // The newest build's stages, at once.
  assert.ok(mod.says('Stages of #128 · success · took 6m 00s'));
  assert.deepEqual(stageRows(mod), ['Checkout 18s', 'Build 1m 30s', 'Test 2m 42s', 'Deploy to dev 1m 30s']);
  assert.deepEqual(iconsOf(mod, 'jk-stage-54'), ['success']);
  assert.equal(byId(mod.render(), 'jk-build-128').calls.bg?.[0], '#selected', 'the build shown is marked in the list');
  const describes = () => fake.requests.filter((r) => r.path.startsWith('/job/team-dev/job/api_master/') && r.path.endsWith('/wfapi/describe')).map((r) => r.path);
  assert.deepEqual(describes(), ['/job/team-dev/job/api_master/128/wfapi/describe']);

  // Another build's, picked from the list: the failed one says why.
  await mod.click('jk-build-127');
  assert.ok(mod.says('Stages of #127 · failed · took 4m 00s'), mod.texts().join('|'));
  assert.deepEqual(stageRows(mod), ['Checkout 16s', 'Build 1m 20s', 'Test 2m 24s script returned exit code 1']);
  assert.deepEqual(iconsOf(mod, 'jk-stage-54'), ['failed']);
  assert.equal(byId(mod.render(), 'jk-build-128').calls.bg, undefined);
  await mod.click('jk-build-126');
  assert.deepEqual(stageRows(mod), ['Checkout 18s', 'Build 1m 30s', 'Test skipped', 'Deploy to dev 1m 30s'], 'a stage its when passed over');
  assert.deepEqual(iconsOf(mod, 'jk-stage-54'), ['skipped']);
  await mod.click('jk-build-127');
  const reads = fake.requests.length;
  await mod.tick();
  await mod.tick();
  assert.equal(fake.requests.length, reads, 'the stages of a finished build are read once');

  // A stage's part of the console: from its start to its end, and why it failed.
  await mod.click('jk-stage-54');
  const lines = texts(byId(mod.render(), 'jk-log-area'));
  assert.equal(lines[0], '+ go test ./...');
  assert.equal(lines.at(-1), 'FAIL    team/service/schedule    0.412s', 'tabs are spaces');
  assert.ok(lines.includes('    schedule_test.go:41: got 3 rows, want 4 (räksmörgås)'));
  assert.ok(!lines.some((l) => l.startsWith('Started by') || l.startsWith('Finished:') || l.startsWith('[Pipeline]')));
  assert.ok(mod.says('took 2m 24s · api_master #127') && mod.says('script returned exit code 1'));
  assert.ok(mod.says(`${lines.length} lines`) && !mod.says('Following the log'));
  assert.ok(mod.view.scroll.bottoms > 0, 'it shows the end of the stage');
  await mod.click('jk-build-open');
  assert.deepEqual(w.opened, [`${fake.url}/job/team-dev/job/api_master/127/execution/node/54/log/`]);

  // The whole log, from there: read already.
  const consoles = consoleReads(fake);
  await mod.click('jk-whole-log');
  const whole = texts(byId(mod.render(), 'jk-log-area'));
  assert.equal(whole[0], 'Started by user Jane Doe');
  assert.equal(whole.at(-1), 'Finished: FAILURE');
  assert.ok(mod.says('api_master #127') && mod.says('failed') && mod.says('gitBranch=feature/export-columns'));
  assert.equal(byId(mod.render(), 'jk-abort'), null);
  assert.equal(consoleReads(fake), consoles);
  await mod.tick();
  assert.equal(consoleReads(fake), consoles, 'a finished build is not read again');
  await mod.click('jk-build-open');
  assert.equal(w.opened[1], `${fake.url}/job/team-dev/job/api_master/127/`);

  // Esc: the job, still showing the build picked; the console log from there too.
  await mod.key('escape');
  assert.ok(mod.says('Stages of #127 · failed · took 4m 00s'));
  await mod.click('jk-console');
  assert.equal(texts(byId(mod.render(), 'jk-log-area')).at(-1), 'Finished: FAILURE');
  await mod.key('escape');
  await mod.click('jk-job-open');
  assert.equal(w.opened[2], `${fake.url}/job/team-dev/job/api_master/`);
  await mod.key('escape');
  assert.ok(mod.says('team-prod'), 'Esc is the list again');
  await fake.close();
});

test('a running build\'s stages: the one at work, how far by last time, the ones still to come', async () => {
  const fake = await startFakeJenkins();
  const mod = await start(configured(fake));
  await mod.click(item(['team-dev', 'poller_master']));
  assert.ok(mod.says('Stages of #57'));
  assert.deepEqual(stageRows(mod), ['Checkout 18s', 'Build 1m 30s', 'Test 12s', 'Deploy to dev ~1m 30s']);
  assert.equal(spinners(byId(mod.render(), 'jk-stage-54')), 1, 'the stage at work turns');
  let bars = 0;
  walk(byId(mod.render(), 'jk-stage-54'), (n) => { if (n.calls.w?.[0] === 36) bars += 1; });
  assert.equal(bars, 1, 'with a bar, by the 2m 42s it took last time');
  assert.deepEqual(iconsOf(mod, 'jk-stage-next-Deploy to dev'), ['notbuilt']);
  assert.equal(byId(mod.render(), 'jk-stage-next-Deploy to dev').handlers.on_click, undefined, 'one still to come has no log');
  assert.equal(fake.requests.filter((r) => r.path.endsWith('/wfapi/runs')).length, 1);

  // Read again every five seconds while it runs.
  const describes = () => fake.requests.filter((r) => r.path.endsWith('/57/wfapi/describe')).length;
  assert.equal(describes(), 1);
  skew += 20_000;
  await mod.tick();
  assert.equal(describes(), 2);
  assert.equal(row(mod, 'jk-stage-54'), 'Test 32s');

  // Its log, followed.
  await mod.click('jk-stage-54');
  const log = () => texts(byId(mod.render(), 'jk-log-area'));
  assert.equal(log()[0], '+ go test ./...');
  assert.ok(mod.says('Following the log') && mod.says('running for 32s · poller_master #57'));
  const before = log().length;
  skew += 10_000;
  await mod.tick();
  assert.equal(log().length, before + 5, 'its new lines, as they come');

  // It ends: the stage is done, and so is the following.
  skew += 5 * 60_000;
  await mod.tick();
  assert.ok(!mod.says('Following the log') && mod.says('took 2m 42s · poller_master #57'), mod.texts().join('|'));
  assert.ok(!log().includes('[Pipeline] }') && !log().some((l) => l.startsWith('Deploy to dev')), 'only this stage\'s lines');
  await mod.key('escape');
  await mod.tick();
  assert.deepEqual(stageRows(mod), ['Checkout 18s', 'Build 1m 30s', 'Test 2m 42s', 'Deploy to dev 1m 30s']);
  await fake.close();
});

test('a build that waits for input, a freestyle job, and a Jenkins without Stage View', async () => {
  let fake = await startFakeJenkins();
  let mod = await start(configured(fake, { folders: '' }));
  await mod.click(item(['team-prod']));
  await mod.click(item(['team-prod', 'alarm-notify_master']));
  assert.deepEqual(stageRows(mod), ['Build 3m 36s', 'Deploy to staging 2m 24s', 'Approve waiting for input', 'Deploy to prod ~4m 12s']);
  assert.deepEqual(iconsOf(mod, 'jk-stage-54'), ['paused']);
  await mod.click('jk-stage-54');
  assert.deepEqual(texts(byId(mod.render(), 'jk-log-area')), ['[Pipeline] input', 'Deploy to production?', 'Proceed or Abort']);
  assert.ok(mod.says('waiting for input for 3m 00s'));
  await mod.key('escape');
  await mod.key('escape');
  await mod.key('escape');

  // A freestyle job has no stages: its builds open their console log.
  await mod.click(item(['nightly-cleanup']));
  assert.ok(mod.says('Build #2210 · success · took 2m 00s'));
  assert.ok(mod.says('This job is not a Pipeline, so it has no stages: its console log has it all.'));
  assert.ok(!fake.requests.some((r) => r.path.includes('/job/nightly-cleanup/') && r.path.includes('wfapi')), 'not even asked');
  await mod.click('jk-build-2210');
  assert.equal(texts(byId(mod.render(), 'jk-log-area')).at(-1), 'Finished: SUCCESS');
  await fake.close();

  // A Jenkins without Pipeline Stage View answers 404: the console log still has it all.
  fake = await startFakeJenkins({ stageView: false });
  mod = await start(configured(fake));
  await mod.click(item(['team-dev', 'api_master']));
  assert.ok(mod.says('Jenkins shows no stages for this build (no Pipeline Stage View): its console log has it all.'), mod.texts().join('|'));
  const asked = fake.requests.length;
  await mod.tick();
  assert.equal(fake.requests.length, asked, 'and is not asked again');
  await mod.click('jk-console');
  assert.equal(texts(byId(mod.render(), 'jk-log-area')).at(-1), 'Finished: SUCCESS');
  await fake.close();
});

test('a running build\'s log is followed from where the last read ended, and read whole once it ends', async () => {
  const fake = await startFakeJenkins();
  const mod = await start(configured(fake));
  await mod.click(item(['team-dev', 'poller_master']));
  assert.match(row(mod, 'jk-build-57'), /^#57 gitBranch=worktree\/lucky-pine 3\d%$/);
  assert.ok(byId(mod.render(), 'jk-build-57-abort'), 'a running build can be aborted');
  await mod.click('jk-console');
  const log = () => texts(byId(mod.render(), 'jk-log-area'));
  const first = log();
  assert.ok(mod.says('Following the log') && mod.says('running'));
  assert.ok(byId(mod.render(), 'jk-abort'));
  assert.ok(first.length > 60 && !first.includes('Finished: SUCCESS'));
  const starts = () => fake.requests.filter((r) => r.path.endsWith('/57/logText/progressiveText')).map((r) => Number(r.query.start));
  assert.deepEqual(starts(), [0]);

  skew += 20_000;
  await mod.tick();
  const second = log();
  assert.equal(second.length, first.length + 10, 'ten more lines in twenty seconds');
  assert.deepEqual(second.slice(0, first.length), first, 'what was read stays as it was');
  const ticked = (lines) => lines.filter((l) => /^ok |: step \d+ done$/.test(l));
  assert.equal(new Set(ticked(second)).size, ticked(second).length, 'no line twice');
  assert.equal(starts()[1], Buffer.byteLength(`${first.join('\n')}\n`.replaceAll('    ', '\t')), 'the next read starts at the byte the last ended at');
  const before = mod.view.scroll.bottoms;
  await mod.tick();
  assert.ok(mod.view.scroll.bottoms > before, 'it keeps to the end while it follows');

  // Scrolling up by hand stops that, until asked again.
  kept(byId(mod.render(), 'jk-log-area')).handlers.on_scroll_wheel({ delta: { x: 0, y: 30 } }, mod.cx);
  const held = mod.view.scroll.bottoms;
  await mod.tick();
  assert.equal(mod.view.scroll.bottoms, held);
  await mod.click('jk-follow');
  assert.ok(mod.view.scroll.bottoms > held);

  skew += 5 * 60_000; // past Jenkins' estimate: the build ends
  await mod.tick();
  const last = log();
  assert.equal(last.at(-1), 'Finished: SUCCESS');
  assert.equal(new Set(last.filter((l) => l.startsWith('ok'))).size, last.filter((l) => l.startsWith('ok')).length);
  assert.equal(starts().at(-1), 0, 'read whole once more at the end');
  assert.ok(!mod.says('Following the log') && mod.says('success'));
  assert.equal(byId(mod.render(), 'jk-abort'), null);
  const reads = fake.requests.length;
  await mod.tick();
  assert.equal(fake.requests.length, reads);
  await fake.close();
});

test('Build asks first, with a form for the job\'s parameters preset to their defaults', async () => {
  const fake = await startFakeJenkins({ queueMs: 1500 });
  const mod = await start(configured(fake));
  await mod.click(item(['team-dev', 'api_master']));
  await mod.click('jk-build');
  assert.equal(posts(fake).length, 0, 'Build… alone starts nothing');
  assert.ok(mod.says(`Starts a build of team-dev/api_master on 127.0.0.1:${fake.port} as jane.`));
  const form = mod.view.form;
  assert.deepEqual(form.fields.map((f) => f.name), ['gitBranch', 'site', 'skipTests', 'registryPassword', 'bundle']);
  assert.equal(form.inputs.gitBranch.value(), 'master', 'a text parameter is preset to its default');
  assert.ok(form.inputs.registryPassword.masked);
  assert.deepEqual(kept(byId(mod.render(), 'jk-param-site-0')).calls.border_color, ['#accent'], 'the default choice is picked');
  assert.ok(mod.says('The branch to build and deploy') && mod.says('Cannot be given from here: Jenkins uses its default.'));
  assert.equal(row(mod, 'jk-param-skipTests'), 'No');

  // Cancel (and Esc) start nothing.
  await mod.click('jk-build-no');
  assert.equal(posts(fake).length, 0);
  assert.ok(mod.says('Builds'));

  await mod.click('jk-build');
  mod.type(mod.view.form.inputs.gitBranch, 'feature/a b');
  await mod.click('jk-param-site-2');
  await mod.click('jk-param-skipTests');
  assert.equal(row(mod, 'jk-param-skipTests'), 'Yes');
  // Tab goes from field to field.
  await mod.key('tab');
  assert.equal(mod.view.form.inputs.gitBranch.focused, 1);
  await mod.click('jk-build-yes');

  const [post] = posts(fake);
  assert.equal(post.path, '/job/team-dev/job/api_master/buildWithParameters');
  assert.equal(post.body, 'gitBranch=feature%2Fa%20b&site=south&skipTests=true', 'an empty password and a file are left to Jenkins');
  assert.equal(post.crumb, CRUMB.crumb, 'with the crumb Jenkins asks for');
  assert.equal(post.status, 201);
  assert.ok(mod.says('Queued a build of api_master.'));
  assert.ok(mod.says('A build waits in the queue') && mod.says('Builds'), 'back at the job');
  assert.equal(mod.view.form, null);

  skew += 6000; // a job with a build waiting is read again every five seconds
  await mod.tick();
  assert.match(row(mod, 'jk-build-129'), /^#129 gitBranch=feature\/a b · site=south · skipTests=true/);
  assert.ok(byId(mod.render(), 'jk-build-129-abort'));
  assert.ok(!mod.says('A build waits in the queue'));
  await mod.click('jk-dismiss');
  assert.ok(!mod.says('Queued a build of api_master.'));

  // Enter in a field builds too; a job without parameters is asked about the same way.
  await mod.key('escape');
  await mod.click(item(['team-dev', 'a-job-with-a-rather-long-name-for-a-narrow-panel_master']));
  await mod.click('jk-build');
  assert.ok(mod.says('This job takes no parameters.'));
  assert.equal(posts(fake).length, 1);
  await mod.key('enter');
  assert.equal(posts(fake).at(-1).path, '/job/team-dev/job/a-job-with-a-rather-long-name-for-a-narrow-panel_master/build');
  assert.equal(posts(fake).at(-1).status, 201);

  // A disabled job's button does nothing.
  await mod.key('escape');
  await mod.click(item(['team-dev', 'legacy_master']));
  assert.equal(byId(mod.render(), 'jk-build').handlers.on_click, undefined);
  await fake.close();
});

test('Abort asks first, then stops the build', async () => {
  const fake = await startFakeJenkins();
  const mod = await start(configured(fake));
  await mod.click(item(['team-dev', 'poller_master']));
  await mod.click('jk-build-57-abort');
  assert.ok(mod.says('Abort #57 of poller_master? Jenkins stops the build where it is.'));
  assert.equal(posts(fake).length, 0, 'asking stops nothing');
  assert.ok(mod.says('Builds'), 'the click was the button\'s alone, not the row\'s');
  await mod.click('jk-abort-no');
  assert.ok(!mod.says('Abort #57 of poller_master?'));
  assert.equal(posts(fake).length, 0);

  // From the build's own view.
  await mod.click('jk-console');
  await mod.click('jk-abort');
  await mod.click('jk-abort-yes');
  assert.deepEqual(posts(fake).map((r) => [r.path, r.status, r.crumb]), [['/job/team-dev/job/poller_master/57/stop', 302, CRUMB.crumb]]);
  assert.ok(mod.says('Asked Jenkins to abort #57 of poller_master.'));
  assert.ok(mod.says('aborted') && !mod.says('Following the log'));
  assert.equal(texts(byId(mod.render(), 'jk-log-area')).at(-1), 'Finished: ABORTED');
  assert.equal(byId(mod.render(), 'jk-abort'), null);
  await mod.key('escape');
  assert.match(row(mod, 'jk-build-57'), /^#57 gitBranch=worktree\/lucky-pine \dm \d\ds · \dm$/);
  await fake.close();
});

// The input steps of the made-up Jenkins' waiting builds.
const inputId = (message) => {
  const hex = createHash('md5').update(message).digest('hex');
  return hex[0].toUpperCase() + hex.slice(1);
};
const APPROVE = inputId('Deploy to production?'); // team-prod/alarm-notify_master #12, at Approve: asks nothing
const NOTES = inputId('Proceeding will generate release notes and auto deploy to production.'); // team-prod/exporter_master #41, at Release Notes: asks three values
const inputReads = (fake) => fake.requests.filter((r) => r.path.endsWith('/wfapi/pendingInputActions')).map((r) => r.path);
const enabled = (mod, id) => Boolean(byId(mod.render(), id)?.handlers.on_click);

test('the input helpers, and the real Jenkins\' shapes the fake follows', async () => {
  assert.ok(util.isPaused({ status: 'PAUSED_PENDING_INPUT', stages: [] }));
  assert.ok(util.isPaused({ status: 'IN_PROGRESS', stages: [{ status: 'SUCCESS' }, { status: 'PAUSED_PENDING_INPUT' }] }), 'a parallel branch that waits');
  assert.ok(!util.isPaused({ status: 'IN_PROGRESS', stages: [{ status: 'IN_PROGRESS' }] }) && !util.isPaused(null) && !util.isPaused({ missing: true }));

  // As Pipeline Stage View lists them: each input's definition is the parameter's own.
  const fields = util.inputFields({ id: 'x', inputs: [
    { type: 'ChoiceParameterDefinition', name: 'env', description: 'where', definition: { choices: ['prod', 'test'], defaultParameterValue: { value: 'test' } } },
    { type: 'StringParameterDefinition', name: 'note', definition: { defaultValue: 'hi' } },
    { type: 'BooleanParameterDefinition', name: 'tell', definition: { defaultVal: true } },
    { type: 'PasswordParameterDefinition', name: 'pw', definition: {} },
    { type: 'FileParameterDefinition', name: 'file' },
  ] });
  assert.deepEqual(fields.map((f) => [f.name, f.kind, f.value]), [['env', 'choice', 'test'], ['note', 'text', 'hi'], ['tell', 'boolean', true], ['pw', 'password', ''], ['file', 'other', '']]);
  assert.deepEqual(util.inputParams(fields, { env: 'prod', note: 'a "b"', tell: false, pw: 's' }),
    [{ name: 'env', value: 'prod' }, { name: 'note', value: 'a "b"' }, { name: 'tell', value: false }, { name: 'pw', value: 's' }], 'a file cannot be given from here');
  assert.deepEqual(util.inputFields({ id: 'x', inputs: [] }), []);
  assert.deepEqual(util.inputFields({ id: 'x' }), []);

  // What the real Jenkins served for its Release Notes input, once answered (read only).
  const real = JSON.parse(fs.readFileSync(path.join(MOD, 'tests', 'fixtures', 'real-input.json'), 'utf8'));
  assert.ok(!util.isPaused(real.answered.describe));
  assert.deepEqual(real.answered.pendingInputActions, [], 'an answered build waits for nothing');
  assert.equal(real.abortedAtInput.describe.stages.at(-1).error.message, 'Rejected by Jane Doe');
  assert.match(real.answered.consoleLinks.proceed, /\/30\/input\/F[0-9a-f]{31}\/proceedEmpty$/, 'an id as the fake makes them');
  assert.match(APPROVE, /^[0-9A-F][0-9a-f]{31}$/);

  // The fake's describe of a waiting build has the real fields, and its stages theirs.
  const fake = await startFakeJenkins();
  const res = await fetch(`${fake.url}/job/team-prod/job/exporter_master/41/wfapi/describe`, { headers: { Authorization: util.basicAuth('jane', TOKEN) } });
  const made = await res.json();
  for (const key of Object.keys(real.answered.describe).filter((k) => k !== '_links')) assert.ok(key in made, `describe.${key}`);
  for (const key of Object.keys(real.answered.describe.stages[0])) assert.ok(key in made.stages[0], `stage.${key}`);
  assert.deepEqual([made.status, made.stages.at(-1).name, made.stages.at(-1).status], ['PAUSED_PENDING_INPUT', 'Release Notes', 'PAUSED_PENDING_INPUT']);
  await fake.close();
});

test('a build that waits at an input: its row and heading say so, its stages show the step, and Proceed answers at once', async () => {
  const fake = await startFakeJenkins();
  const mod = await start(configured(fake));
  assert.equal(row(mod, item(['team-prod', 'alarm-notify_master'])), 'alarm-notify_master #12 · waiting for input');
  assert.equal(row(mod, item(['team-prod', 'exporter_master'])), 'exporter_master #41 · waiting for input');
  assert.deepEqual(iconsOf(mod, item(['team-prod', 'alarm-notify_master'])), ['paused']);
  assert.equal(byId(mod.render(), item(['team-prod', 'alarm-notify_master'])).calls.tooltip[0], 'team-prod/alarm-notify_master: waiting for input');
  assert.match(row(mod, item(['team-dev', 'poller_master'])), /^poller_master #57 \d+%$/, 'one that runs still says how far');
  assert.equal(byId(mod.render(), 'jk-head-paused-/job/team-prod').calls.tooltip[0], '2 waiting for input');
  assert.deepEqual(inputReads(fake), [], 'the list asks no build what it waits for');
  assert.ok(!fake.requests.some((r) => r.path.includes('/api_master/') && r.path.endsWith('wfapi/describe')), 'and reads the stages of running builds alone');

  await mod.click(item(['team-prod', 'alarm-notify_master']));
  assert.deepEqual(stageRows(mod).slice(0, 3), ['Build 3m 36s', 'Deploy to staging 2m 24s', 'Approve waiting for input']);
  const card = `jk-input-${APPROVE}`;
  assert.equal(row(mod, card), 'Deploy to production? Abort… Proceed');
  assert.deepEqual(ids(mod.render(), 'jk-').filter((id) => id.startsWith('jk-stage-') || id.startsWith('jk-input-')),
    ['jk-stage-4', 'jk-stage-29', 'jk-stage-54', card, `${card}-abort`, `${card}-proceed`, 'jk-stage-next-Deploy to prod'], 'under the stage that waits');
  assert.match(row(mod, 'jk-build-12'), /^#12 gitBranch=master waiting for input$/);
  assert.deepEqual(inputReads(fake), ['/job/team-prod/job/alarm-notify_master/12/wfapi/pendingInputActions']);
  await mod.tick();
  assert.equal(inputReads(fake).length, 1, 'not again at once');
  skew += 11_000;
  await mod.tick();
  assert.equal(inputReads(fake).length, 2, 'every ten seconds while it waits');

  await mod.click(`${card}-proceed`);
  assert.deepEqual(posts(fake).map((r) => [r.path, r.status, r.crumb, r.body]), [[`/job/team-prod/job/alarm-notify_master/12/input/${APPROVE}/proceedEmpty`, 302, CRUMB.crumb, '']]);
  assert.equal(fake.find(['team-prod', 'alarm-notify_master']).builds[0].answer.by, 'jane');
  assert.ok(mod.says('Proceed: #12 of alarm-notify_master goes on.'), mod.texts().join('|'));
  assert.equal(byId(mod.render(), card), null, 'the step is answered');
  assert.match(stageRows(mod)[2], /^Approve \d/);
  assert.deepEqual(iconsOf(mod, 'jk-stage-54'), ['success']);
  assert.match(row(mod, 'jk-build-12'), /^#12 gitBranch=master \d+%$/, 'it runs on');
  await mod.key('escape');
  assert.match(row(mod, item(['team-prod', 'alarm-notify_master'])), /^alarm-notify_master #12 \d+%$/);
  const reads = inputReads(fake).length;
  skew += 11_000;
  await mod.tick();
  assert.equal(inputReads(fake).length, reads, 'nothing waits: nothing is asked');
  await fake.close();
});

test('an input that asks for values: its form, preset to the defaults, sends them with its proceed button', async () => {
  const fake = await startFakeJenkins();
  const mod = await start(configured(fake));
  await mod.click(item(['team-prod', 'exporter_master']));
  const card = `jk-input-${NOTES}`;
  assert.equal(row(mod, card), 'Proceeding will generate release notes and auto deploy to production. Asks for jiraEnv, summary, notify Abort… Release…');
  await mod.click(`${card}-proceed`);
  assert.equal(posts(fake).length, 0, 'the form first');
  assert.ok(mod.says('Enter: Release · Esc cancels') && mod.says('#41 of team-prod/exporter_master waits for this. Release answers it as jane.'));
  assert.ok(mod.says('Proceeding will generate release notes and auto deploy to production.'));
  assert.deepEqual(mod.view.form.fields.map((f) => [f.name, f.kind, f.value]), [['jiraEnv', 'choice', 'prod'], ['summary', 'text', ''], ['notify', 'boolean', true]]);
  assert.deepEqual(kept(byId(mod.render(), 'jk-param-jiraEnv-0')).calls.border_color, ['#accent']);
  assert.ok(mod.says('Where the release request goes'));
  assert.equal(row(mod, 'jk-param-notify'), 'Yes');

  // Cancel sends nothing.
  await mod.click('jk-input-no');
  assert.equal(posts(fake).length, 0);
  assert.ok(byId(mod.render(), card), 'back at the job, still waiting');

  await mod.click(`${card}-proceed`);
  mod.type(mod.view.form.inputs.summary, 'Faster exports & "quotes"');
  await mod.click('jk-param-jiraEnv-1');
  await mod.click('jk-param-notify');
  assert.equal(row(mod, 'jk-param-notify'), 'No');
  await mod.key('tab');
  assert.equal(mod.view.form.inputs.summary.focused, 1);
  await mod.key('enter');
  const [post] = posts(fake);
  assert.deepEqual([post.path, post.status, post.crumb], [`/job/team-prod/job/exporter_master/41/input/${NOTES}/submit`, 302, CRUMB.crumb]);
  const form = new URLSearchParams(post.body);
  assert.deepEqual(JSON.parse(form.get('json')), { parameter: [{ name: 'jiraEnv', value: 'test' }, { name: 'summary', value: 'Faster exports & "quotes"' }, { name: 'notify', value: false }] });
  assert.equal(form.get('proceed'), 'Release', 'the button, by which Jenkins tells proceed from abort');
  assert.equal(fake.find(['team-prod', 'exporter_master']).builds[0].answer.action, 'proceed');
  assert.ok(mod.says('Release: #41 of exporter_master goes on.'));
  assert.equal(mod.view.form, null);
  assert.ok(mod.says('Builds') && !byId(mod.render(), card), 'back at the job, answered');
  await fake.close();
});

test('Abort at an input asks first, from the stages, the stage\'s view or the form, and the build ends aborted', async () => {
  const fake = await startFakeJenkins();
  const mod = await start(configured(fake));
  await mod.click(item(['team-prod', 'alarm-notify_master']));
  const card = `jk-input-${APPROVE}`;
  await mod.click(`${card}-abort`);
  assert.ok(mod.says('Abort #12 of alarm-notify_master at its input? Jenkins ends the build as aborted.'));
  assert.equal(posts(fake).length, 0, 'asking aborts nothing');
  await mod.click('jk-abort-no');
  assert.ok(!mod.says('at its input?'));
  await mod.click(`${card}-abort`);
  await mod.key('escape');
  assert.ok(!mod.says('at its input?') && mod.says('Builds'), 'Esc takes the question away, not the view');
  assert.equal(posts(fake).length, 0);

  // The stage's own view shows the step too.
  await mod.click('jk-stage-54');
  assert.deepEqual(texts(byId(mod.render(), 'jk-log-area')), ['[Pipeline] input', 'Deploy to production?', 'Proceed or Abort']);
  assert.ok(byId(mod.render(), `${card}-proceed`));
  await mod.click(`${card}-abort`);
  await mod.click('jk-abort-yes');
  assert.deepEqual(posts(fake).map((r) => [r.path, r.status, r.crumb]), [[`/job/team-prod/job/alarm-notify_master/12/input/${APPROVE}/abort`, 302, CRUMB.crumb]]);
  assert.ok(mod.says('Aborted #12 of alarm-notify_master at its input.'));
  assert.equal(byId(mod.render(), card), null);
  assert.ok(mod.says('Rejected by Jane Doe'), 'why, as Jenkins says it');
  assert.equal(texts(byId(mod.render(), 'jk-log-area')).at(-1), 'Rejected by Jane Doe');
  await mod.key('escape');
  assert.match(row(mod, 'jk-build-12'), /^#12 gitBranch=master \dm \d\ds · \dm$/, 'it has ended');
  await mod.key('escape');
  assert.match(row(mod, item(['team-prod', 'alarm-notify_master'])), /^alarm-notify_master #12 · /);
  assert.deepEqual(iconsOf(mod, item(['team-prod', 'alarm-notify_master'])), ['aborted']);

  // From the form of one that asks for values.
  await mod.click(item(['team-prod', 'exporter_master']));
  await mod.click(`jk-input-${NOTES}-proceed`);
  await mod.click('jk-input-abort');
  assert.ok(mod.says('Abort #41 of exporter_master at its input?'));
  await mod.key('enter');
  assert.equal(posts(fake).length, 1, 'Enter does not answer while the question is open');
  await mod.click('jk-abort-yes');
  assert.equal(posts(fake).at(-1).path, `/job/team-prod/job/exporter_master/41/input/${NOTES}/abort`);
  assert.equal(mod.view.form, null);
  assert.ok(mod.says('Aborted #41 of exporter_master at its input.') && mod.says('Builds'));
  await fake.close();
});

test('while a build waits at an input step, its view\'s header has no Abort: the step\'s own is the one', async () => {
  const fake = await startFakeJenkins();
  const mod = await start(configured(fake));
  await mod.click(item(['team-prod', 'alarm-notify_master']));
  const card = `jk-input-${APPROVE}`;
  await mod.click('jk-stage-54');
  assert.equal(byId(mod.render(), 'jk-abort'), null, 'not in the waiting stage\'s view');
  assert.ok(enabled(mod, `${card}-abort`) && enabled(mod, `${card}-proceed`));
  await mod.click('jk-whole-log');
  assert.equal(byId(mod.render(), 'jk-abort'), null, 'nor in the whole log\'s');
  assert.ok(byId(mod.render(), `${card}-abort`));

  // Answered, it runs on: the header's Abort is back.
  await mod.click(`${card}-proceed`);
  assert.equal(byId(mod.render(), card), null);
  assert.ok(enabled(mod, 'jk-abort'), 'a running build that does not wait');
  await mod.key('escape');

  // A build that runs and never waited has it all along.
  await mod.key('escape');
  await mod.click(item(['team-dev', 'poller_master']));
  await mod.click('jk-console');
  assert.ok(enabled(mod, 'jk-abort'));
  assert.equal(ids(mod.render(), 'jk-input-').length, 0);
  await fake.close();
});

test('a failed answer says why and leaves the view as it was: a token that may only read, an input answered elsewhere', async () => {
  const fake = await startFakeJenkins();
  let mod = await start(configured(fake, { user: 'viewer', token: 'viewer-token' }));
  await mod.click(item(['team-prod', 'alarm-notify_master']));
  const card = `jk-input-${APPROVE}`;
  await mod.click(`${card}-proceed`);
  assert.ok(mod.says('This token may not proceed at the input of #12 of alarm-notify_master: Jenkins user viewer lacks the permission (viewer is missing the Job/Build permission).'), mod.texts().join('|'));
  assert.ok(enabled(mod, `${card}-proceed`) && enabled(mod, `${card}-abort`), 'the step still shows, its buttons work');
  await mod.click(`${card}-abort`);
  await mod.click('jk-abort-yes');
  assert.ok(mod.says('This token may not abort #12 of alarm-notify_master'));
  assert.deepEqual(posts(fake).map((r) => r.status), [403, 403]);
  assert.equal(fake.find(['team-prod', 'alarm-notify_master']).builds[0].paused, 'Approve', 'still waiting');
  assert.ok(byId(mod.render(), card) && !mod.says('at its input?'));

  // Answered in the browser meanwhile: Jenkins has no such input any more (404).
  mod = await start(configured(fake));
  await mod.click(item(['team-prod', 'alarm-notify_master']));
  const build = fake.find(['team-prod', 'alarm-notify_master']).builds[0];
  Object.assign(build, { paused: null, estimated: Date.now() - build.timestamp + 60_000 });
  assert.ok(byId(mod.render(), card), 'not read again yet');
  await mod.click(`${card}-proceed`);
  assert.ok(mod.says('Jenkins has no such input waiting on #12 of alarm-notify_master (404): it was answered, or the build ended.'), mod.texts().join('|'));
  assert.equal(byId(mod.render(), card), null, 'read again at once: it waits no more');
  assert.match(stageRows(mod)[2], /^Approve \d/);

  // The same with the form open: it says so, and only Cancel is left.
  await mod.key('escape');
  await mod.click(item(['team-prod', 'exporter_master']));
  await mod.click(`jk-input-${NOTES}-proceed`);
  const notes = fake.find(['team-prod', 'exporter_master']).builds[0];
  Object.assign(notes, { paused: null, result: 'ABORTED', at: 'Release Notes', rejected: true, duration: Date.now() - notes.timestamp });
  await mod.click('jk-input-yes');
  assert.ok(mod.says('Jenkins has no such input waiting on #41 of exporter_master (404)'));
  assert.ok(mod.says('This input no longer waits: it was answered, or the build ended.'));
  assert.ok(!enabled(mod, 'jk-input-yes') && !enabled(mod, 'jk-input-abort') && enabled(mod, 'jk-input-no'));
  await mod.key('escape');
  assert.ok(mod.says('Builds') && mod.view.form === null);
  await fake.close();
});

test('a token that may only read: build and abort say so, and nothing breaks', async () => {
  const fake = await startFakeJenkins();
  const mod = await start(configured(fake, { user: 'viewer', token: 'viewer-token' }));
  assert.equal(row(mod, item(['team-dev', 'api_master'])), 'api_master #128 · 3h', 'it reads as anyone');
  await mod.click(item(['team-dev', 'parser_master']));
  await mod.click('jk-build');
  await mod.click('jk-build-yes');
  assert.ok(mod.says('This token may not build parser_master: Jenkins user viewer lacks the permission (viewer is missing the Job/Build permission).'), mod.texts().join('|'));
  assert.ok(byId(mod.render(), 'jk-build-yes'), 'the form stays, to try again or cancel');
  assert.equal(fake.find(['team-dev', 'parser_master']).queued, null);
  await mod.key('escape');

  await mod.key('escape');
  await mod.click(item(['team-dev', 'poller_master']));
  await mod.click('jk-build-57-abort');
  await mod.click('jk-abort-yes');
  assert.ok(mod.says('This token may not abort #57 of poller_master: Jenkins user viewer lacks the permission (viewer is missing the Job/Build permission).'));
  assert.equal(fake.find(['team-dev', 'poller_master']).builds[0].result, null, 'still running');
  assert.ok(byId(mod.render(), 'jk-build-57'), 'the job still shows');
  assert.deepEqual(posts(fake).map((r) => r.status), [403, 403]);
  await fake.close();
});

test('crumbs: none on a Jenkins without an issuer, a fresh one when the kept one is refused', async () => {
  const plain = await startFakeJenkins({ crumbs: false });
  let mod = await start(configured(plain));
  await mod.click(item(['team-dev', 'parser_master']));
  await mod.click('jk-build');
  await mod.click('jk-build-yes');
  assert.deepEqual(posts(plain).map((r) => [r.status, r.crumb]), [[201, null]]);
  assert.ok(mod.says('Queued a build of parser_master.'));
  await plain.close();

  const fake = await startFakeJenkins();
  mod = await start(configured(fake));
  await mod.click(item(['team-dev', 'parser_master']));
  await mod.click('jk-build');
  await mod.click('jk-build-yes');
  fake.setCrumb('fake-crumb-2');
  await mod.key('escape');
  await mod.click(item(['team-dev', 'alarm-notify_master']));
  await mod.click('jk-build');
  await mod.click('jk-build-yes');
  assert.deepEqual(posts(fake).map((r) => [r.status, r.crumb]), [[201, 'fake-crumb-1'], [403, 'fake-crumb-1'], [201, 'fake-crumb-2']]);
  assert.ok(mod.says('Queued a build of alarm-notify_master.'));
  await fake.close();
});

test('a Jenkins that stops answering: quiet at first, the error after a while, the rows kept', async () => {
  const fake = await startFakeJenkins();
  const mod = await start(configured(fake));
  assert.equal(row(mod, item(['team-dev', 'api_master'])), 'api_master #128 · 3h');
  await fake.close();
  skew += 61_000;
  await mod.tick();
  assert.ok(!mod.says('Could not reach Jenkins'), 'one failed read shows no error');
  assert.ok(mod.says('Last read 1m ago · no answer since'), mod.texts().join('|'));
  assert.match(row(mod, item(['team-dev', 'api_master'])), /^api_master #128/, 'the rows stay');
  for (let i = 0; i < 30; i++) { skew += 2_000; await mod.tick(); }
  const said = mod.texts().filter((line) => line.startsWith('Could not reach Jenkins to list the jobs of team-dev: '));
  assert.equal(said.length, 1, mod.texts().join('|'));
  assert.ok(!said[0].includes(TOKEN) && !said[0].includes('Basic'));
  assert.ok(!mod.says('no answer since'), 'the error takes the quiet line\'s place');
  assert.match(row(mod, item(['team-dev', 'api_master'])), /^api_master #128/, 'the rows stay');
  await mod.click(item(['team-dev', 'api_master']));
  assert.ok(mod.says('Reading the job…') && !mod.says('Could not reach Jenkins to read api_master'), 'no data yet, and only one failure: still reading');
  await run([mod], 58_000);
  assert.ok(!mod.says('Could not reach Jenkins to read api_master'), 'a job at rest is tried every thirty seconds: two failures');
  await run([mod], 2_000);
  assert.ok(mod.says('Could not reach Jenkins to read api_master: '), mod.texts().join('|'));
});

// Ticks every two seconds for `ms`, as the app's timer does, and calls `each` after every tick.
const run = async (mods, ms, each = () => {}) => {
  for (let at = 0; at < ms; at += 2_000) {
    skew += 2_000;
    for (const mod of mods) await mod.tick();
    each(at + 2_000);
  }
};
const listReads = (fake, place = 'team-dev') => fake.requests.filter((r) => r.path === `/job/${place}/api/json`);

test('a blip of a failed read or two shows no error, only how old the rows are, and a good read clears that', async () => {
  for (const blip of [{ mode: 'refuse' }, { mode: 'status', status: 503 }, { mode: 'status', status: 502 }]) {
    skew = 0;
    const fake = await startFakeJenkins();
    const mod = await start(configured(fake));
    fake.fail({ ...blip, path: '^/job/team-dev/api/json$' });
    skew += 11_000; // a build runs in team-dev: its list is read every ten seconds
    await mod.tick();
    assert.equal(listReads(fake).at(-1).status, blip.mode === 'refuse' ? 'refused' : blip.status);
    assert.ok(!mod.says('Could not') && !mod.says('Jenkins answered'), `${blip.mode} ${blip.status ?? ''}: ${mod.texts().join('|')}`);
    assert.ok(mod.says('Last read 11s ago · no answer since'), mod.texts().join('|'));
    assert.equal(byId(mod.render(), 'jk-stale-/job/team-dev').calls.tooltip[0], 'Jenkins did not answer the last read: trying again');
    assert.equal(byId(mod.render(), 'jk-stale-/job/team-prod'), null, 'only the list that failed');
    assert.match(row(mod, item(['team-dev', 'api_master'])), /^api_master #128/);
    skew += 10_000;
    await mod.tick();
    assert.equal(listReads(fake).length, 3, 'tried again after ten seconds');
    assert.ok(!mod.says('Could not') && !mod.says('Jenkins answered') && mod.says('Last read 21s ago · no answer since'), 'two failures: still quiet');
    fake.recover();
    skew += 10_000;
    await mod.tick();
    assert.equal(listReads(fake).length, 3, 'the second failure waits thirty seconds');
    assert.ok(mod.says('Last read 31s ago'), 'the line counts on while it waits');
    skew += 20_000;
    await mod.tick();
    assert.equal(listReads(fake).length, 4);
    assert.ok(!mod.says('no answer since') && !mod.says('Could not'), 'a good read clears it');
    skew += 10_000;
    await mod.tick();
    assert.equal(listReads(fake).length, 5, 'and the list is read every ten seconds again');
    await fake.close();
  }
});

test('a long outage: the error after three failures, reads that back off to a minute, and the next good read clears it', async () => {
  const fake = await startFakeJenkins();
  const mod = await start(configured(fake));
  await mod.click(item(['team-dev', 'poller_master']));
  await mod.click('jk-console'); // a running build's log, read every tick while all is well
  // A read of the log asks for the build, then (when it answers) for the console from where it was.
  const logReads = () => fake.requests.filter((r) => r.path === '/job/team-dev/job/poller_master/57/api/json').length;
  // A tick's worth while all is well: the log on each tick, the lists every ten seconds.
  const before = { log: logReads(), list: listReads(fake).length };
  await run([mod], 20_000);
  const well = { log: logReads() - before.log, list: listReads(fake).length - before.list };
  assert.deepEqual(well, { log: 10, list: 2 }, 'twenty seconds at work');

  fake.fail({ mode: 'refuse' });
  const outageStart = fake.requests.length;
  let loud = null;
  const fails = [];
  await run([mod], 5 * 60_000, (at) => {
    if (loud === null && mod.says('Could not reach Jenkins to read this build\'s log: ')) loud = at;
    if (logReads() > (fails.at(-1)?.n ?? before.log + well.log)) fails.push({ at, n: logReads() });
  });
  const during = { log: logReads() - before.log - well.log, list: listReads(fake).length - before.list - well.list, all: fake.requests.length - outageStart };
  console.log(`      five minutes of no answer: ${during.log} log reads (${5 * 30} at a read a tick), ${during.list} reads of team-dev (30 at one every ten seconds), ${during.all} requests in all`);
  assert.deepEqual(fails.map((f) => f.at), [2_000, 12_000, 42_000, 102_000, 162_000, 222_000, 282_000], 'waits of 10 s, 30 s, then a minute');
  assert.equal(during.log, 7);
  assert.equal(during.list, 7);
  assert.equal(loud, 42_000, 'the error shows with the third failure');
  assert.ok(!mod.says('no answer since'));

  // Back: the next try reads it all again, and the error goes.
  fake.recover();
  await run([mod], 60_000);
  assert.ok(!mod.says('Could not reach Jenkins') && !mod.says('no answer since'), mod.texts().join('|'));
  assert.ok(mod.texts().some((line) => /^ok /.test(line)), 'the log shows');
  await fake.close();
});

test('a list read only every minute shows its error once it has failed for a minute', async () => {
  const fake = await startFakeJenkins();
  const mod = await start(configured(fake, { folders: 'team-dev/tools' })); // nothing runs there: every minute
  fake.fail({ mode: 'status', status: 503 });
  skew += 60_000;
  await mod.tick();
  assert.ok(mod.says('Last read 1m ago · no answer since') && !mod.says('Could not'));
  const reads = () => listReads(fake, 'team-dev/job/tools').length;
  assert.equal(reads(), 2);
  await run([mod], 58_000);
  assert.equal(reads(), 2, 'the back-off never reads it more often than at rest');
  skew += 2_000;
  await mod.tick();
  assert.equal(reads(), 3);
  assert.ok(mod.says('Could not list the jobs of team-dev/tools: Jenkins answered 503'), mod.texts().join('|'));
  await fake.close();
});

test('401 and 403 show at once, and back off too', async () => {
  for (const [status, said] of [[401, 'Jenkins did not accept this user and token (401). Check them in the settings.'], [403, 'This token may not list the jobs of team-dev: Jenkins user jane lacks the permission (Forbidden).']]) {
    skew = 0;
    const fake = await startFakeJenkins();
    const mod = await start(configured(fake));
    fake.fail({ mode: 'status', status, path: '^/job/team-dev/api/json$' });
    skew += 11_000;
    await mod.tick();
    assert.ok(mod.says(said), `${status}: ${mod.texts().join('|')}`);
    assert.ok(!mod.says('no answer since'));
    await run([mod], 8_000);
    assert.equal(listReads(fake).length, 2, 'not read again before ten seconds');
    await fake.close();
  }
});

test('a read that never ends counts as failed after a while, and the next one is not held up by it', async () => {
  const fake = await startFakeJenkins();
  const mod = await start(configured(fake));
  fake.fail({ mode: 'stall', path: '^/job/team-dev/api/json$' });
  skew += 11_000;
  await mod.poke();
  assert.equal(listReads(fake).at(-1).status, 'stalled');
  assert.ok(byId(mod.render(), 'jk-refresh').children.some((c) => 'frames' in kept(c).calls), 'it spins while it reads');
  skew += 30_000;
  await mod.poke();
  assert.equal(listReads(fake).length, 2, 'one read at a time');
  assert.ok(!mod.says('no answer since'));
  skew += 16_000; // 46 s: given up on
  await mod.poke();
  assert.ok(mod.says('Last read 57s ago · no answer since'), mod.texts().join('|'));
  assert.equal(listReads(fake).length, 2, 'the next waits for its back-off');
  fake.recover();
  skew += 10_000;
  await mod.poke();
  // The stalled read still holds a task, so wait for this one by real time.
  for (let i = 0; i < 200 && mod.says('no answer since'); i++) await new Promise((resolve) => setTimeout(resolve, 10));
  assert.equal(listReads(fake).length, 3);
  assert.ok(!mod.says('no answer since'), mod.texts().join('|'));
  // The stalled read's answer comes at last, and is dropped: the newer one stands.
  const shown = mod.texts().join('|');
  fake.release();
  await mod.settle();
  assert.equal(mod.texts().join('|'), shown);
  await fake.close();
});

test('who the token is: a start with no answer is quiet and asked again, not left as an error', async () => {
  const fake = await startFakeJenkins();
  fake.fail({ mode: 'refuse', path: '^/whoAmI/' });
  const mod = await start(configured(fake));
  assert.ok(!mod.says('Could not reach Jenkins to ask who this token is'), mod.texts().join('|'));
  assert.ok(mod.texts().includes(`127.0.0.1:${fake.port}`), 'the host alone until it answers');
  fake.recover();
  await run([mod], 10_000);
  assert.ok(mod.texts().includes(`127.0.0.1:${fake.port} · jane`), mod.texts().join('|'));
  const asked = fake.requests.filter((r) => r.path === '/whoAmI/api/json').length;
  await run([mod], 120_000);
  assert.equal(fake.requests.filter((r) => r.path === '/whoAmI/api/json').length, asked, 'once it has answered, never again');
  await fake.close();
});

test('the refresh button reads at once, back-off or not', async () => {
  const fake = await startFakeJenkins();
  const mod = await start(configured(fake));
  fake.fail({ mode: 'refuse' });
  for (let i = 0; i < 3; i++) await mod.click('jk-refresh');
  assert.ok(mod.says('Could not reach Jenkins to list the jobs of team-dev: '), 'three failures in a row');
  fake.recover();
  await mod.click('jk-refresh');
  assert.ok(!mod.says('Could not reach Jenkins'));
  assert.equal(listReads(fake).length, 5);
  await fake.close();
});

// The requests of `copies` copies of the mod in one app, all showing a
// running build's log after its job, the list behind them, for three minutes.
async function copiesRun(copies, module) {
  skew = 0;
  const fake = await startFakeJenkins();
  const w = configured(fake);
  const mods = [];
  for (let i = 0; i < copies; i++) mods.push(await start(w, module));
  for (const mod of mods) {
    await mod.click(item(['team-dev', 'poller_master']));
    await mod.click('jk-console');
  }
  await run(mods, 3 * 60_000);
  const shown = mods.map((mod) => texts(byId(mod.render(), 'jk-log-area')).length);
  const n = fake.requests.length;
  await fake.close();
  return { n, shown };
}

test('copies of the mod read once for all, and each keeps its own view', async () => {
  const one = await copiesRun(1, 'one');
  const two = await copiesRun(2, 'two');
  const three = await copiesRun(3, 'three');
  console.log(`      requests in three minutes: one copy ${one.n}, two copies ${two.n}, three copies ${three.n}`);
  assert.equal(two.n, one.n);
  assert.equal(three.n, one.n);
  assert.ok(two.shown.every((n) => n === two.shown[0] && n > 60), 'each shows the whole log');
  const apart = await copiesRun(2, undefined); // copies of two apps (or two versions of the mod) share nothing
  assert.ok(apart.n > 1.5 * one.n, `${apart.n}`);

  // Each copy its own view: one goes back to the list, the other stays on the log.
  skew = 0;
  const fake = await startFakeJenkins();
  const w = configured(fake);
  const a = await start(w, 'views'), b = await start(w, 'views');
  await a.click(item(['team-dev', 'poller_master']));
  assert.ok(b.says('team-prod') && !b.says('Builds'), 'b still shows the list');
  await a.click('jk-console');
  kept(byId(a.render(), 'jk-log-area')).handlers.on_scroll_wheel({ delta: { x: 0, y: 30 } }, a.cx);
  await b.click(item(['team-dev', 'poller_master']));
  await b.click('jk-console');
  assert.ok(byId(a.render(), 'jk-follow') && !byId(b.render(), 'jk-follow'), 'a stopped following, b follows');
  await a.key('escape');
  await a.key('escape');
  await run([a, b], 10_000);
  assert.ok(a.says('team-prod') && b.says('Following the log'));
  await fake.close();
});

test('settings saved in one copy are taken up by the others', async () => {
  const fake = await startFakeJenkins();
  const w = configured(fake);
  const a = await start(w, 'saved'), b = await start(w, 'saved');
  assert.ok(b.says('team-prod'));
  await a.click('jk-settings');
  a.type(a.view.inputs.folders, 'team-dev/tools');
  await a.click('jk-set-save');
  await b.tick();
  assert.ok(b.says('team-dev/tools') && !b.says('team-prod'), b.texts().join('|'));
  await fake.close();
});

test('the store: back-off, quiet and loud failures', () => {
  const e = new data.Entry();
  assert.ok(e.due(60_000, 1_000), 'never read: due at once');
  e.begin(1_000);
  e.succeeded([], 1_500);
  assert.ok(!e.due(10_000, 11_000) && e.due(10_000, 11_500));
  const no = { kind: 'network', message: 'no answer' };
  e.failed(no, 12_000);
  assert.ok(e.stale(12_000) && !e.loud(12_000));
  assert.ok(!e.due(2_000, 21_999) && e.due(2_000, 22_000), 'ten seconds after the first failure');
  e.failed(no, 22_000);
  assert.ok(!e.due(2_000, 51_999) && e.due(2_000, 52_000), 'thirty after the second');
  assert.ok(!e.loud(51_999) && e.loud(72_000), 'a minute of failing is loud');
  e.failed(no, 52_000);
  assert.ok(e.loud(52_000), 'so is the third failure');
  assert.ok(!e.due(2_000, 111_999) && e.due(2_000, 112_000), 'a minute after the third');
  e.failed(no, 112_000);
  assert.ok(e.due(2_000, 172_000), 'and no more than a minute after that');
  e.succeeded([], 180_000);
  assert.ok(!e.error && !e.failures && !e.stale(180_000) && e.due(2_000, 182_000));
  for (const kind of ['auth', 'forbidden', 'missing', 'settings']) {
    const f = new data.Entry();
    f.failed({ kind }, 1_000);
    assert.ok(f.loud(1_000), kind);
  }
  assert.ok(data.transient({ kind: 'http', status: 503 }) && data.transient({ kind: 'http', status: 429 }) && !data.transient({ kind: 'http', status: 400 }));
  assert.ok(!data.transient({ kind: 'http', status: 0 }), 'not JSON: a wrong URL, not a blip');
  const stuck = new data.Entry();
  stuck.begin(1_000);
  assert.ok(!stuck.due(0, 45_999) && stuck.loading);
  assert.ok(!stuck.due(0, 46_000) && stuck.error.kind === 'timeout' && !stuck.loading && stuck.failures === 1, 'given up on: a failure');
  assert.ok(stuck.due(0, 56_000), 'and tried again at its back-off');
  assert.equal(util.lastRead(1_000, 41_900), '40s');
  assert.equal(util.lastRead(1_000, 1_000 + 3 * 60_000), '3m');
});

test('in the right-hand panel (props().panel) it is a card as Files, Changes and GitLab are, elsewhere it fills its place', async () => {
  const fake = await startFakeJenkins();
  const root = (mod) => kept(mod.render()).calls;
  const tab = root(await start(configured(fake)));
  assert.equal(tab.bg?.[0], '#background', 'a tab or an overlay');
  assert.equal(tab.rounded, undefined);
  assert.equal(tab.border_1, undefined);
  const w = configured(fake);
  w.props = { panel: true };
  const panel = root(await start(w));
  assert.equal(panel.bg?.[0], '#background', 'the background its siblings have');
  assert.deepEqual(panel.rounded, [10]);
  assert.deepEqual(panel.border_1, []);
  assert.deepEqual(panel.border_color, ['#border']);
  await fake.close();
});

test('the refresh command reads again, and settings opens the settings', async () => {
  const fake = await startFakeJenkins();
  const mod = await start(configured(fake));
  const reads = () => fake.requests.filter((r) => r.path === '/job/team-dev/api/json').length;
  assert.equal(reads(), 1);
  await mod.send('refresh');
  assert.equal(reads(), 2);
  await mod.send('settings');
  assert.ok(mod.says('Jenkins settings'));
  await fake.close();
});

test('install.sh puts the mod\'s own files in an app home, and takes them out again', () => {
  const home = fs.mkdtempSync(path.join(os.tmpdir(), 'jenkins-install-'));
  const app = path.join(home, 'app');
  const checker = path.join(home, 'herdr-app');
  fs.writeFileSync(checker, '#!/bin/sh\necho "$@" >> "$0.log"\n: > "$2/jsconfig.json"\n', { mode: 0o755 });
  const run = (...args) => {
    try {
      return { code: 0, out: execFileSync('sh', [path.join(MOD, 'install.sh'), ...args], { env: { ...process.env, HERDR_APP_BIN: checker, HERDR_APP_HOME: '' }, encoding: 'utf8', stdio: ['ignore', 'pipe', 'pipe'] }) };
    } catch (e) {
      return { code: e.status, out: `${e.stdout}${e.stderr}` };
    }
  };
  assert.notEqual(run('--home', app).code, 0, 'no app home: nothing is made');
  assert.ok(!fs.existsSync(app));

  fs.mkdirSync(path.join(app, 'mods'), { recursive: true });
  assert.equal(run('--home', app).code, 0);
  const target = path.join(app, 'mods', 'jenkins');
  assert.deepEqual(fs.readdirSync(target).sort(), ['api.js', 'data.js', 'icons', 'main.js', 'mod.json', 'ui.js', 'util.js'], 'no tests, no README, nothing the check wrote');
  const modules = fs.readdirSync(MOD).filter((f) => f.endsWith('.js'));
  assert.deepEqual(modules.filter((f) => !fs.existsSync(path.join(target, f))), [], 'every module of the mod is installed');
  assert.equal(fs.readdirSync(path.join(target, 'icons')).length, fs.readdirSync(path.join(MOD, 'icons')).length);
  assert.ok(fs.readFileSync(`${checker}.log`, 'utf8').startsWith('check '), 'checked before it was put in place');
  assert.deepEqual(fs.readdirSync(path.join(app, 'mods')), ['jenkins'], 'no staging folder is left');

  // Again: nothing changes, so the app reloads nothing.
  const before = fs.statSync(path.join(target, 'main.js')).mtimeMs;
  fs.writeFileSync(path.join(target, 'jsconfig.json'), '{}'); // what the app writes there
  assert.equal(run('--home', app).code, 0);
  assert.equal(fs.statSync(path.join(target, 'main.js')).mtimeMs, before);

  // The settings outlive an uninstall, unless told to forget them.
  fs.mkdirSync(path.join(app, 'jenkins'));
  fs.writeFileSync(path.join(app, 'jenkins', 'settings.json'), '{}');
  assert.equal(run('--uninstall', '--home', app).code, 0);
  assert.ok(!fs.existsSync(target) && fs.existsSync(path.join(app, 'jenkins', 'settings.json')));
  assert.equal(run('--uninstall', '--forget', '--home', app).code, 0);
  assert.ok(!fs.existsSync(path.join(app, 'jenkins')));

  // Another mod of the same name is never replaced or removed.
  fs.mkdirSync(target);
  fs.writeFileSync(path.join(target, 'main.js'), '// somebody else\'s\n');
  assert.notEqual(run('--home', app).code, 0);
  assert.notEqual(run('--uninstall', '--home', app).code, 0);
  assert.equal(fs.readFileSync(path.join(target, 'main.js'), 'utf8'), '// somebody else\'s\n');
  fs.rmSync(home, { recursive: true });
});

test('install.sh --panel right puts it into a layout\'s right-hand panel stack, once, moves an earlier install\'s there, and --panel off and --uninstall take it out', () => {
  const home = fs.mkdtempSync(path.join(os.tmpdir(), 'jenkins-panel-'));
  const app = path.join(home, 'app');
  const checker = path.join(home, 'herdr-app');
  fs.writeFileSync(checker, '#!/bin/sh\n: > "$2/jsconfig.json"\n', { mode: 0o755 });
  const run = (...args) => {
    const r = spawnSync('sh', [path.join(MOD, 'install.sh'), ...args, '--home', app], { env: { ...process.env, HERDR_APP_BIN: checker, HERDR_APP_HOME: '' }, encoding: 'utf8' });
    return { code: r.status, out: `${r.stdout}${r.stderr}` };
  };
  // A layout as `corgi app-layout` leaves Herdr's main: a sidebar, and the
  // work column whose Files and Changes panel (with another mod's block in it) is hidden.
  const files = { id: 'panel-body', type: 'stack', active: 'files', children: [
    { id: 'files', type: 'block', content: { mod: 'files' } }, { id: 'changes', type: 'block', content: { mod: 'changes' } },
    { id: 'gitlab', type: 'block', content: { mod: 'gitlab' } }] };
  const corgi = { id: 'root', type: 'row', background: 'canvas', divider: { width: 1, color: 'border' }, children: [
    { id: 'sidebar', type: 'column', size: { px: 268 }, background: 'chrome', children: [{ id: 'spaces', type: 'block', content: { mod: 'projects' } }] },
    { id: 'work', type: 'column', resizable: false, children: [
      { id: 'tabs', type: 'block', size: { px: 42 }, content: { mod: 'tabs-gitlab' } },
      { id: 'work-body', type: 'row', gap: 8, children: [{ id: 'main', type: 'block', content: { tab: 'active' } },
        { id: 'panel', type: 'column', hidden: true, size: { px: 368 }, children: [files] }] },
      { id: 'status', type: 'block', size: { px: 28 }, content: { mod: 'workspace-status' } }] }],
    cloned_from: { name: 'main', version: '0.1.14' }, titlebar: 'none', note: 'räksmörgås' };
  fs.mkdirSync(path.join(app, 'mods'), { recursive: true });
  fs.mkdirSync(path.join(app, 'layouts'));
  const file = (name) => path.join(app, 'layouts', `${name}.json`);
  const read = (name) => JSON.parse(fs.readFileSync(file(name), 'utf8'));
  fs.writeFileSync(file('corgi'), JSON.stringify(corgi, null, 2), { mode: 0o600 });
  fs.writeFileSync(file('main'), JSON.stringify({ ...corgi, cloned_from: undefined }));
  fs.writeFileSync(path.join(app, 'installed.json'), JSON.stringify({ 'layout:main': {}, 'layout:terminals': {}, agents: {} }));
  fs.writeFileSync(path.join(app, 'shared.json'), JSON.stringify({ layout: 'corgi', 'space.colors': {} }));
  const main = fs.readFileSync(file('main'), 'utf8');

  // The layout with the stack's children `stack` and the top row's `extra` children.
  const shaped = (layout, stack, extra = []) => ({ ...layout, children: [layout.children[0], { ...layout.children[1], children: [layout.children[1].children[0],
    { ...layout.children[1].children[1], children: [layout.children[1].children[1].children[0], { ...layout.children[1].children[1].children[1], children: [stack] }] },
    layout.children[1].children[2]] }, ...extra] });
  const stackOf = (layout) => layout.children[1].children[1].children[1].children[0];
  const panel = { id: 'jenkins-panel', type: 'block', content: { mod: 'jenkins', props: { panel: true } } };

  // To the layout the window shows: one child more in the right-hand panel's
  // stack, after Files, Changes and GitLab, and nothing else changed (the
  // panel stays hidden, Files stays its shown child, the top row as it was).
  let r = run('--panel', 'right');
  assert.equal(r.code, 0, r.out);
  assert.ok(fs.existsSync(path.join(app, 'mods', 'jenkins', 'main.js')), 'the mod is installed with it');
  assert.match(r.out, /Jenkins is in its right-hand panel \(stack panel-body, block jenkins-panel\)/);
  assert.deepEqual(read('corgi'), shaped(corgi, { ...files, children: [...files.children, panel] }));
  assert.deepEqual(read('corgi').children.map((c) => c.id), ['sidebar', 'work'], 'no block of its own in the top row');
  assert.deepEqual(Object.keys(read('corgi')), Object.keys(corgi), 'its fields in their order');
  assert.ok(fs.readFileSync(file('corgi'), 'utf8').includes('räksmörgås'), 'text as it was, not escaped');
  assert.equal(fs.statSync(file('corgi')).mode & 0o777, 0o600, 'its mode kept');
  assert.deepEqual(fs.readdirSync(path.join(app, 'layouts')).sort(), ['corgi.json', 'main.json'], 'nothing left beside it');

  // Again: nothing changes, so the app reloads nothing.
  const before = fs.readFileSync(file('corgi'), 'utf8'), at = fs.statSync(file('corgi')).mtimeMs;
  r = run('--panel', 'right', '--layout', 'corgi');
  assert.equal(r.code, 0, r.out);
  assert.match(r.out, /shows Jenkins in its right-hand panel already \(block jenkins-panel\)/);
  assert.equal(fs.readFileSync(file('corgi'), 'utf8'), before);
  assert.equal(fs.statSync(file('corgi')).mtimeMs, at);

  // Herdr's own layouts, and ones that are not there, are left alone.
  r = run('--panel', 'right', '--layout', 'main');
  assert.notEqual(r.code, 0);
  assert.match(r.out, /main is Herdr's own layout.*clone it first/);
  assert.equal(fs.readFileSync(file('main'), 'utf8'), main);
  assert.notEqual(run('--panel', 'right', '--layout', 'nope').code, 0);
  assert.notEqual(run('--panel', 'right', '--layout', '../corgi').code, 0);
  assert.notEqual(run('--panel', 'left').code, 0);

  // A layout with no right-hand panel stack: left alone.
  const bare = { ...corgi, children: [corgi.children[0], { ...corgi.children[1], children: [corgi.children[1].children[0], { id: 'main', type: 'block', content: { tab: 'active' } }] }] };
  fs.writeFileSync(file('bare'), JSON.stringify(bare));
  r = run('--panel', 'right', '--layout', 'bare');
  assert.notEqual(r.code, 0);
  assert.match(r.out, /layout bare has no right-hand panel like Herdr's layouts.*left alone/);
  assert.deepEqual(read('bare'), bare);
  fs.rmSync(file('bare'));

  // An id taken already: the block gets another.
  fs.writeFileSync(file('mine'), JSON.stringify({ ...corgi, children: [...corgi.children, { id: 'jenkins-panel', type: 'column', children: [] }] }));
  assert.equal(run('--panel', 'right', '--layout', 'mine').code, 0);
  assert.deepEqual(read('mine').children.map((c) => c.id), ['sidebar', 'work', 'jenkins-panel']);
  assert.deepEqual(stackOf(read('mine')).children.map((c) => c.id), ['files', 'changes', 'gitlab', 'jenkins-panel-2']);

  // An earlier install's panel, a block `jenkins` at the end of the top row
  // (GitLab the stack's shown child): moved into the stack under an id of its
  // own, all else kept, and again nothing changes.
  const earlier = { id: 'jenkins', type: 'block', content: { mod: 'jenkins', props: { panel: true } }, size: { px: 320 }, min: 260, max: 640, background: 'chrome' };
  const shown = { ...files, active: 'gitlab' };
  fs.writeFileSync(file('old'), JSON.stringify(shaped(corgi, shown, [earlier]), null, 2));
  r = run('--panel', 'right', '--layout', 'old');
  assert.equal(r.code, 0, r.out);
  assert.match(r.out, /took out the Jenkins panel of an earlier install at the end of its top row \(jenkins\)/);
  assert.deepEqual(read('old'), shaped(corgi, { ...shown, children: [...files.children, panel] }));
  const migrated = fs.readFileSync(file('old'), 'utf8');
  assert.equal(run('--panel', 'right', '--layout', 'old').code, 0);
  assert.equal(fs.readFileSync(file('old'), 'utf8'), migrated);
  // Off takes out the old block too, for a layout not moved yet.
  fs.writeFileSync(file('old'), JSON.stringify(shaped(corgi, files, [earlier]), null, 2));
  assert.equal(run('--panel', 'off', '--layout', 'old').code, 0);
  assert.deepEqual(read('old'), shaped(corgi, files));
  fs.rmSync(file('old'));

  // Off: the panel goes; the layout is as it was, and the mod stays.
  r = run('--panel', 'off', '--layout', 'corgi');
  assert.equal(r.code, 0, r.out);
  assert.deepEqual(read('corgi'), corgi);
  assert.ok(fs.existsSync(path.join(app, 'mods', 'jenkins', 'main.js')));
  assert.match(run('--panel', 'off', '--layout', 'corgi').out, /has no Jenkins panel/);

  // A block a person put in by hand stays with --panel off; the uninstall takes
  // every one out, of every layout of theirs, and a stack shows another child.
  const own = shaped(corgi, { ...files, active: 'mine', children: [...files.children, { id: 'mine', type: 'block', content: { mod: 'jenkins' } }] });
  fs.writeFileSync(file('corgi'), JSON.stringify(own));
  assert.equal(run('--panel', 'right').code, 0);
  assert.match(run('--panel', 'off').out, /has no Jenkins panel/, 'right found the block by hand, so added none');
  r = run('--uninstall');
  assert.equal(r.code, 0, r.out);
  assert.deepEqual(stackOf(read('corgi')), { ...files, active: 'files' });
  assert.deepEqual(read('mine').children.map((c) => c.id), ['sidebar', 'work', 'jenkins-panel'], 'a column of that id is not Jenkins');
  assert.deepEqual(stackOf(read('mine')), files);
  assert.equal(fs.readFileSync(file('main'), 'utf8'), main);
  assert.ok(!fs.existsSync(path.join(app, 'mods', 'jenkins')));
  fs.rmSync(home, { recursive: true });
});

// Herdr's tab bar (`mods/tabs/main.js`, 0.1.14) as far as the Jenkins button
// needs it: the panel buttons and their `enabled` list, the per-space memory
// of the open panel, the window buttons the button goes before, and an asset
// path of its own, which a clone rewrites.
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
    const now = panel.hidden ? null : body?.active ?? null;
    if (space.id !== this.space) {
      const first = this.space === undefined;
      this.space = space.id; this.pending = null;
      if (!(first && !(space.id in this.panels))) {
        const stored = this.panels[space.id] ?? null, want = enabled.includes(stored) ? stored : null;
        if (want !== now) {
          this.pending = { want, until: Date.now() + 1000 };
          if (want) { run("layout.select", { stack: "panel-body", child: want }); run("layout.show", { id: "panel" }); }
          else run("layout.hide", { id: "panel" });
          return;
        }
      }
    }
    if (this.pending) {
      if (this.pending.want !== now && Date.now() < this.pending.until) return;
      this.pending = null;
    }
    if (this.panels[space.id] === now) return;
    this.panels[space.id] = now;
    localStorage.setItem("panels", JSON.stringify(this.panels));
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

// What herdr-gitlab's install.py (wire_tabs, read 2026-11-11) does to a copy
// of the tab bar: GitLab joins `enabled` and gets a button after Changes. It
// finds both lines by their text and replaces them with Python's
// str.replace, which replaces every place the text is, hence replaceAll.
function herdrGitlab(text) {
  const ENABLED = 'const enabled = shared("workspace.panels") ?? ["files", "changes"];';
  const CHANGES = '.when(enabled.includes("changes"), el => el.child(panelButton("changes", "Changes", "mods/changes/changes.svg")))';
  const GITLAB = '.when(enabled.includes("gitlab"), el => el.child(panelButton("gitlab", "GitLab", "mods/gitlab/icons/gitlab.svg")))';
  if (text.includes(GITLAB)) return text;
  assert.ok(text.includes(ENABLED) && text.includes(CHANGES), 'herdr-gitlab would refuse this tab bar');
  return text.replaceAll(ENABLED, '// The GitLab panel (mods/gitlab) sits in the same stack as Files and Changes.\n    const enabled = [...(shared("workspace.panels") ?? ["files", "changes"]), "gitlab"];')
    .replaceAll(CHANGES, `${CHANGES}\n      ${GITLAB}`);
}

// herdr-gitlab's copy of Herdr's tab bar.
const GITLAB_TABS = herdrGitlab(STOCK_TABS.replaceAll('mods/tabs/', 'mods/tabs-gitlab/'));

// The Jenkins button as the previous installer put it in (a block at the end of the top row it toggled).
const OLD_BUTTON = `      // >>> Jenkins button: install.sh of mods/jenkins puts it here, and takes it out (--panel off)
      .when(Boolean(find(root, "jenkins")), el => el.child((() => {
        // Jenkins (mods/jenkins) is its own panel on the right: this button shows and hides it.
        const on = !find(root, "jenkins").hidden;
        return h_flex().id("panel-jenkins").h(26).px(8).gap(5).flex_none().items_center().rounded(6)
          .text_size(12).bg(on ? t.selected : t.chrome).text_color(on ? t.foreground : t.muted)
          .hover(s => s.bg(t.hover)).on_mouse_down("left", stop)
          .on_click(() => run("layout.toggle", { id: "jenkins" }))
          .child(svg("mods/jenkins/icons/jenkins.svg").size(14)).child("Jenkins");
      })()))
      // <<< Jenkins button
`;
const withOldButton = (text) => text.replace('      .when(w?.buttons', `${OLD_BUTTON}      .when(w?.buttons`);

// A tab bar's main.js run as the app runs it, with stand-ins for the app: a
// window whose right-hand panel (`panel`, hidden or not) holds a stack with
// the children `stack`, `active` the one it shows. `render()` draws the bar
// and gives its panel buttons; `space` switches the space; what the bar runs
// is kept in `ran`, and a run of layout.select, show or hide changes the
// window as the app would.
function tabBar(text, { hidden = true, active = 'files', stack = ['files', 'changes', 'jenkins-panel'], panels } = {}) {
  const body = { id: 'panel-body', type: 'stack', active, children: stack.map((id) => ({ id })) };
  const panel = { id: 'panel', hidden, children: [body] };
  const root = { id: 'root', children: [{ id: 'tabs' }, panel] };
  const ran = [], store = new Map();
  let space = 's1';
  const colors = new Proxy({}, { get: (_t, name) => (typeof name === 'string' ? `#${name}` : undefined) });
  const run = (command, args) => {
    ran.push([command, args]);
    if (command === 'layout.select') body.active = args.child;
    if (command === 'layout.show') panel.hidden = false;
    if (command === 'layout.hide') panel.hidden = true;
  };
  const code = text.replace(/^import .*$/gm, '').replace('export default class', 'return class');
  // eslint-disable-next-line no-new-func
  const Tabs = new Function('View', 'div', 'svg', 'image', 'h_flex', 'spaces', 'panes', 'run', 'openOverlay', 'theme', 'window', 'shared', 'layout', 'settings', 'localStorage', code)(
    kit.View, kit.div, kit.svg, kit.svg, kit.h_flex, () => [{ id: space, active: true }], () => [], run, () => {}, () => colors, () => ({}),
    (key) => (key === 'workspace.panels' ? panels : undefined), () => root, () => ({ zoom: 1 }),
    { getItem: (k) => store.get(k) ?? null, setItem: (k, v) => store.set(k, v) });
  const bar = new Tabs();
  bar.init?.();
  return {
    ran, panel, body,
    render() {
      const el = bar.render();
      return ids(el, 'panel-').map((id) => byId(el, id));
    },
    space(id) { space = id; },
  };
}

test('install.sh --panel right puts a Jenkins button into the layout\'s tab bar: a copy of Herdr\'s, or the copy it shows', () => {
  const home = fs.mkdtempSync(path.join(os.tmpdir(), 'jenkins-button-'));
  const app = path.join(home, 'app'), mods = path.join(app, 'mods');
  const checker = path.join(home, 'herdr-app');
  // Logs what it checks, refuses while `<checker>.refuse` is there, and writes what the app writes into a mod folder.
  fs.writeFileSync(checker, '#!/bin/sh\necho "$@" >> "$0.log"\nif [ -e "$0.refuse" ]; then echo "SyntaxError: nope" >&2; exit 1; fi\n: > "$2/jsconfig.json"\n', { mode: 0o755 });
  const run = (...args) => {
    const r = spawnSync('sh', [path.join(MOD, 'install.sh'), ...args, '--home', app], { env: { ...process.env, HERDR_APP_BIN: checker, HERDR_APP_HOME: '' }, encoding: 'utf8' });
    return { code: r.status, out: `${r.stdout}${r.stderr}` };
  };
  const mod = (name, file) => path.join(mods, name, file);
  const text = (name, file = 'main.js') => fs.readFileSync(mod(name, file), 'utf8');
  const layoutFile = (name) => path.join(app, 'layouts', `${name}.json`);
  const read = (name) => JSON.parse(fs.readFileSync(layoutFile(name), 'utf8'));
  const buttons = (s) => s.split('panelButton("jenkins-panel"').length - 1;
  const stagesLeft = () => fs.readdirSync(mods).filter((f) => f.startsWith('.'));

  // An app home with Herdr's tab bar (and what the app writes beside it), and
  // a personal layout whose tabs block shows it, as a clone of main does.
  fs.mkdirSync(path.join(mods, 'tabs'), { recursive: true });
  fs.mkdirSync(path.join(app, 'layouts'));
  fs.writeFileSync(mod('tabs', 'main.js'), STOCK_TABS);
  fs.writeFileSync(mod('tabs', 'mod.json'), '{ "place": "none", "title": "Tabs" }');
  fs.writeFileSync(mod('tabs', 'win-min.svg'), '<svg/>');
  fs.writeFileSync(mod('tabs', 'jsconfig.json'), '{}');
  fs.writeFileSync(mod('tabs', 'gpui-kit.d.ts'), '');
  fs.writeFileSync(path.join(app, 'installed.json'), JSON.stringify({ 'layout:main': {}, 'layout:terminals': {}, tabs: {}, 'terminals-bar': {}, files: {} }));
  fs.writeFileSync(path.join(app, 'update-notes.json'), JSON.stringify({ last_run: '0.1.14' }));
  fs.writeFileSync(path.join(app, 'shared.json'), JSON.stringify({ layout: 'mine' }));
  const stack = { id: 'panel-body', type: 'stack', active: 'files', children: [
    { id: 'files', type: 'block', content: { mod: 'files' } }, { id: 'changes', type: 'block', content: { mod: 'changes' } }] };
  const mine = { id: 'root', type: 'row', children: [
    { id: 'sidebar', type: 'column', size: { px: 240 }, children: [{ id: 'spaces', type: 'block', content: { mod: 'spaces' } }] },
    { id: 'work', type: 'column', children: [
      { id: 'tabs', type: 'block', size: { px: 30 }, content: { mod: 'tabs' } },
      { id: 'work-body', type: 'row', children: [{ id: 'main', type: 'block', content: { tab: 'active' } },
        { id: 'panel', type: 'column', hidden: true, size: { px: 368 }, children: [stack] }] }] }],
    cloned_from: { name: 'main', version: '0.1.14' } };
  fs.writeFileSync(layoutFile('mine'), JSON.stringify(mine, null, 2));
  const stock = fs.readdirSync(path.join(mods, 'tabs')).sort();
  const panel = { id: 'jenkins-panel', type: 'block', content: { mod: 'jenkins', props: { panel: true } } };
  const withTabs = (layout, bar) => ({ ...layout, children: [layout.children[0], { ...layout.children[1], children: [{ ...layout.children[1].children[0], content: { mod: bar } }, layout.children[1].children[1]] }] });
  const withPanel = (layout) => {
    const work = layout.children[1], body = work.children[1];
    return { ...layout, children: [layout.children[0], { ...work, children: [work.children[0], { ...body, children: [body.children[0], { ...body.children[1], children: [{ ...stack, children: [...stack.children, panel] }] }] }] }] };
  };
  const stackOf = (layout) => layout.children[1].children[1].children[1].children[0];
  const shown = (name) => read(name).children[1].children[0].content.mod;

  // Herdr's own tab bar: cloned to tabs-jenkins, with the button, and the layout shows the copy.
  let r = run('--panel', 'right');
  assert.equal(r.code, 0, r.out);
  assert.match(r.out, /made mods\/tabs-jenkins/);
  assert.match(r.out, /run install\.sh --panel right again to bring the button back/);
  assert.deepEqual(read('mine'), withPanel(withTabs(mine, 'tabs-jenkins')));
  assert.deepEqual(fs.readdirSync(path.join(mods, 'tabs-jenkins')).sort(), ['main.js', 'mod.json', 'win-min.svg'], 'what the app writes is not copied');
  assert.deepEqual(JSON.parse(text('tabs-jenkins', 'mod.json')), { place: 'none', title: 'Tabs', cloned_from: { name: 'tabs', version: '0.1.14' } });
  const clone = text('tabs-jenkins');
  assert.ok(!clone.includes('mods/tabs/') && clone.includes('mods/tabs-jenkins/win-min.svg'), 'its own asset paths');
  assert.equal(buttons(clone), 1);
  assert.equal(text('tabs'), STOCK_TABS, 'Herdr\'s own is never changed');
  assert.deepEqual(fs.readdirSync(path.join(mods, 'tabs')).sort(), stock);
  assert.match(fs.readFileSync(`${checker}.log`, 'utf8'), /check .*\.jenkins-tabs-check/, 'the copy was checked before it was put in place');
  assert.deepEqual(stagesLeft(), [], 'no staging folder is left');
  assert.ok(clone.indexOf('panelButton("changes"') < clone.indexOf('panelButton("jenkins-panel"'), 'after Files and Changes');
  assert.ok(clone.indexOf('panelButton("jenkins-panel"') < clone.indexOf('.when(w?.buttons'), 'just before the window buttons');

  // The button, run in the tab bar: Jenkins with its icon, after Files and Changes.
  let bar = tabBar(clone);
  let [files, changes, jenkins] = bar.render();
  assert.deepEqual([files.id, changes.id, jenkins.id], ['panel-files', 'panel-changes', 'panel-jenkins-panel']);
  assert.deepEqual(jenkins.children.map((c) => kept(c).args?.[0] ?? c), ['mods/jenkins/icons/jenkins.svg', 'Jenkins']);
  assert.deepEqual(jenkins.calls.bg, ['#chrome'], 'not selected while the panel is hidden');
  // A click shows Jenkins in the panel; it is selected, and Files is not.
  jenkins.handlers.on_click();
  assert.deepEqual(bar.ran, [['layout.select', { stack: 'panel-body', child: 'jenkins-panel' }], ['layout.show', { id: 'panel' }]]);
  [files, changes, jenkins] = bar.render();
  assert.deepEqual([files.calls.bg, jenkins.calls.bg], [['#chrome'], ['#selected']]);
  // From Files to Jenkins: it switches, the panel stays shown.
  files.handlers.on_click();
  [files, changes, jenkins] = bar.render();
  assert.deepEqual([files.calls.bg, jenkins.calls.bg], [['#selected'], ['#chrome']]);
  bar.ran.length = 0;
  jenkins.handlers.on_click();
  assert.deepEqual(bar.ran, [['layout.select', { stack: 'panel-body', child: 'jenkins-panel' }], ['layout.show', { id: 'panel' }]]);
  // A click while Jenkins shows hides the panel.
  [files, changes, jenkins] = bar.render();
  bar.ran.length = 0;
  jenkins.handlers.on_click();
  assert.deepEqual(bar.ran, [['layout.hide', { id: 'panel' }]]);
  assert.deepEqual(bar.render()[2].calls.bg, ['#chrome']);
  // Remembered per space with Files and Changes: back in a space where it was open, it opens again.
  bar = tabBar(clone);
  bar.render();
  bar.render()[2].handlers.on_click();
  bar.render();
  bar.space('s2');
  bar.render();
  assert.equal(bar.panel.hidden, true, 'a space that had no panel open');
  bar.ran.length = 0;
  bar.space('s1');
  bar.render();
  assert.deepEqual(bar.ran, [['layout.select', { stack: 'panel-body', child: 'jenkins-panel' }], ['layout.show', { id: 'panel' }]]);
  // With Herdr's own list of panels in shared, too; and no button while the stack has no Jenkins.
  assert.deepEqual(tabBar(clone, { panels: ['files'] }).render().map((b) => b.id), ['panel-files', 'panel-jenkins-panel']);
  assert.deepEqual(tabBar(clone, { stack: ['files', 'changes'] }).render().map((b) => b.id), ['panel-files', 'panel-changes']);

  // Again: nothing changes.
  let at = [fs.statSync(mod('tabs-jenkins', 'main.js')).mtimeMs, fs.readFileSync(layoutFile('mine'), 'utf8')];
  r = run('--panel', 'right');
  assert.equal(r.code, 0, r.out);
  assert.match(r.out, /mods\/tabs-jenkins has the Jenkins button already/);
  assert.deepEqual([fs.statSync(mod('tabs-jenkins', 'main.js')).mtimeMs, fs.readFileSync(layoutFile('mine'), 'utf8')], at);

  // Herdr updated its tab bar: the copy, as cloned, is cloned again.
  fs.writeFileSync(mod('tabs', 'main.js'), STOCK_TABS.replace('min_w(0)', 'min_w(0).pl(12)'));
  r = run('--panel', 'right');
  assert.equal(r.code, 0, r.out);
  assert.match(r.out, /updated mods\/tabs-jenkins/);
  assert.ok(text('tabs-jenkins').includes('.pl(12)') && buttons(text('tabs-jenkins')) === 1);
  fs.writeFileSync(mod('tabs', 'main.js'), STOCK_TABS);

  // Off: the panel and the button go, the layout shows Herdr's tab bar again, and the copy goes.
  r = run('--panel', 'off');
  assert.equal(r.code, 0, r.out);
  assert.deepEqual(read('mine'), mine);
  assert.ok(!fs.existsSync(path.join(mods, 'tabs-jenkins')));
  assert.ok(fs.existsSync(mod('jenkins', 'main.js')), 'the mod stays');

  // An earlier install: the panel a block at the end of the top row, and
  // tabs-jenkins with the button that toggled it. The block moves into the
  // stack and the copy, still as cloned, is made again with the new button.
  assert.equal(run('--panel', 'right').code, 0);
  const cloned = text('tabs-jenkins').split('\n')[0];
  const oldClone = `${cloned}\n${withOldButton(STOCK_TABS.replaceAll('mods/tabs/', 'mods/tabs-jenkins/'))}`;
  fs.writeFileSync(mod('tabs-jenkins', 'main.js'), oldClone);
  const earlier = { id: 'jenkins', type: 'block', content: { mod: 'jenkins', props: { panel: true } }, size: { px: 320 }, min: 260, max: 640, background: 'chrome' };
  const oldLayout = withTabs(mine, 'tabs-jenkins');
  fs.writeFileSync(layoutFile('mine'), JSON.stringify({ ...oldLayout, children: [...oldLayout.children, earlier] }, null, 2));
  r = run('--panel', 'right');
  assert.equal(r.code, 0, r.out);
  assert.match(r.out, /updated mods\/tabs-jenkins/);
  assert.match(r.out, /took out the Jenkins panel of an earlier install/);
  assert.deepEqual(read('mine'), withPanel(withTabs(mine, 'tabs-jenkins')));
  assert.equal(text('tabs-jenkins'), clone, 'as a fresh install makes it');
  r = run('--panel', 'off');
  assert.equal(r.code, 0, r.out);
  assert.deepEqual(read('mine'), mine);
  assert.ok(!fs.existsSync(path.join(mods, 'tabs-jenkins')));

  // herdr-gitlab installed after it edits tabs-jenkins in place, as it edits
  // any copy the layout shows: the two work side by side. The copy is the
  // layout's from then on: only the Jenkins lines come and go.
  assert.equal(run('--panel', 'right').code, 0);
  const both = herdrGitlab(text('tabs-jenkins'));
  assert.equal(both.split('const enabled =').length - 1, 1, 'one enabled list');
  fs.writeFileSync(mod('tabs-jenkins', 'main.js'), both);
  bar = tabBar(both, { hidden: false, active: 'gitlab', stack: ['files', 'changes', 'gitlab', 'jenkins-panel'] });
  assert.deepEqual(bar.render().map((b) => b.id), ['panel-files', 'panel-changes', 'panel-gitlab', 'panel-jenkins-panel']);
  bar.render()[3].handlers.on_click();
  assert.deepEqual(bar.render().map((b) => b.calls.bg[0]), ['#chrome', '#chrome', '#chrome', '#selected']);
  r = run('--panel', 'off');
  assert.equal(r.code, 0, r.out);
  assert.match(r.out, /mods\/tabs-jenkins was changed since this installer made it: the layout keeps it/);
  assert.deepEqual(read('mine'), withTabs(mine, 'tabs-jenkins'));
  assert.equal(text('tabs-jenkins'), `${clone.split('\n')[0]}\n${herdrGitlab(STOCK_TABS.replaceAll('mods/tabs/', 'mods/tabs-jenkins/'))}`, 'herdr-gitlab\'s edit of the copy, nothing of Jenkins');
  r = run('--panel', 'right');
  assert.equal(r.code, 0, r.out);
  assert.match(r.out, /put a Jenkins button into mods\/tabs-jenkins\/main\.js/);
  assert.ok(buttons(text('tabs-jenkins')) === 1 && text('tabs-jenkins').includes('"GitLab"'));
  r = run('--uninstall');
  assert.equal(r.code, 0, r.out);
  assert.deepEqual(read('mine'), withTabs(mine, 'tabs-jenkins'));
  assert.equal(buttons(text('tabs-jenkins')), 0);
  fs.rmSync(path.join(mods, 'tabs-jenkins'), { recursive: true });

  // Another mod named tabs-jenkins is never taken over.
  fs.writeFileSync(layoutFile('mine'), JSON.stringify(mine, null, 2));
  fs.mkdirSync(path.join(mods, 'tabs-jenkins'));
  fs.writeFileSync(mod('tabs-jenkins', 'main.js'), '// somebody else\'s\n');
  r = run('--panel', 'right');
  assert.notEqual(r.code, 0);
  assert.match(r.out, /tabs-jenkins is another mod/);
  assert.deepEqual(read('mine'), mine, 'nothing was changed');
  assert.equal(text('tabs-jenkins'), '// somebody else\'s\n');
  fs.rmSync(path.join(mods, 'tabs-jenkins'), { recursive: true });
  fs.rmSync(path.join(mods, 'jenkins'), { recursive: true });

  // A copy the layout shows already (herdr-gitlab's tabs-gitlab, GitLab in
  // the stack): the button goes into it between markers, once, and comes out
  // leaving it byte for byte as it was.
  const gitlab = GITLAB_TABS;
  const gitlabLayout = (layout) => {
    const work = layout.children[1], body = work.children[1], s = body.children[1].children[0];
    return { ...layout, children: [layout.children[0], { ...work, children: [work.children[0], { ...body, children: [body.children[0], { ...body.children[1], children: [{ ...s, children: [...s.children, { id: 'gitlab', type: 'block', content: { mod: 'gitlab' } }] }] }] }] }] };
  };
  const mineGitlab = gitlabLayout(withTabs(mine, 'tabs-gitlab'));
  fs.mkdirSync(path.join(mods, 'tabs-gitlab'));
  fs.writeFileSync(mod('tabs-gitlab', 'main.js'), gitlab, { mode: 0o640 });
  fs.writeFileSync(mod('tabs-gitlab', 'mod.json'), JSON.stringify({ place: 'none', cloned_from: { name: 'tabs', version: '0.1.14' } }));
  fs.writeFileSync(layoutFile('mine'), JSON.stringify(mineGitlab, null, 2));
  r = run('--panel', 'right');
  assert.equal(r.code, 0, r.out);
  assert.match(r.out, /put a Jenkins button into mods\/tabs-gitlab\/main\.js/);
  assert.deepEqual(stackOf(read('mine')).children.map((c) => c.id), ['files', 'changes', 'gitlab', 'jenkins-panel'], 'after GitLab in the stack');
  assert.deepEqual(stackOf(read('mine')).children.at(-1), panel);
  assert.equal(shown('mine'), 'tabs-gitlab', 'the layout keeps showing the copy');
  assert.ok(!fs.existsSync(path.join(mods, 'tabs-jenkins')), 'no clone of its own');
  const edited = text('tabs-gitlab');
  assert.equal(buttons(edited), 1);
  assert.ok(edited.indexOf('panelButton("gitlab"') < edited.indexOf('panelButton("jenkins-panel"'), 'after the other mod\'s button');
  assert.equal(fs.statSync(mod('tabs-gitlab', 'main.js')).mode & 0o777, 0o640, 'its mode kept');
  assert.deepEqual(stagesLeft(), []);
  // Run there: Files, Changes, GitLab, Jenkins; from GitLab to Jenkins and back, one shows at a time.
  bar = tabBar(edited, { hidden: false, active: 'gitlab', stack: ['files', 'changes', 'gitlab', 'jenkins-panel'] });
  let row = bar.render();
  assert.deepEqual(row.map((b) => b.id), ['panel-files', 'panel-changes', 'panel-gitlab', 'panel-jenkins-panel']);
  assert.deepEqual(row.map((b) => b.calls.bg[0]), ['#chrome', '#chrome', '#selected', '#chrome']);
  row[3].handlers.on_click();
  assert.deepEqual(bar.render().map((b) => b.calls.bg[0]), ['#chrome', '#chrome', '#chrome', '#selected']);
  bar.render()[2].handlers.on_click();
  assert.deepEqual(bar.render().map((b) => b.calls.bg[0]), ['#chrome', '#chrome', '#selected', '#chrome']);
  at = [fs.statSync(mod('tabs-gitlab', 'main.js')).mtimeMs, fs.readFileSync(layoutFile('mine'), 'utf8')];
  r = run('--panel', 'right');
  assert.equal(r.code, 0, r.out);
  assert.deepEqual([fs.statSync(mod('tabs-gitlab', 'main.js')).mtimeMs, fs.readFileSync(layoutFile('mine'), 'utf8')], at, 'again: nothing changes');

  // A second layout showing the same copy keeps the button while it has the panel.
  fs.writeFileSync(layoutFile('other'), JSON.stringify(mineGitlab, null, 2));
  assert.equal(run('--panel', 'right', '--layout', 'other').code, 0);
  r = run('--panel', 'off');
  assert.equal(r.code, 0, r.out);
  assert.deepEqual(read('mine'), mineGitlab);
  assert.equal(text('tabs-gitlab'), edited, 'the other layout still has the panel');
  r = run('--panel', 'off', '--layout', 'other');
  assert.equal(r.code, 0, r.out);
  assert.equal(text('tabs-gitlab'), gitlab);
  fs.rmSync(layoutFile('other'));

  // An earlier install there: the old button between the markers is
  // replaced in place, the block moves into the stack, and off leaves the
  // copy as herdr-gitlab made it.
  fs.writeFileSync(mod('tabs-gitlab', 'main.js'), withOldButton(gitlab));
  fs.writeFileSync(layoutFile('mine'), JSON.stringify({ ...mineGitlab, children: [...mineGitlab.children, earlier] }, null, 2));
  r = run('--panel', 'right');
  assert.equal(r.code, 0, r.out);
  assert.match(r.out, /updated the Jenkins button in mods\/tabs-gitlab\/main\.js/);
  assert.equal(text('tabs-gitlab'), edited, 'as a fresh install makes it');
  assert.deepEqual(read('mine').children.map((c) => c.id), ['sidebar', 'work']);
  assert.deepEqual(stackOf(read('mine')).children.map((c) => c.id), ['files', 'changes', 'gitlab', 'jenkins-panel']);
  assert.equal(run('--panel', 'off').code, 0);
  assert.equal(text('tabs-gitlab'), gitlab);
  assert.deepEqual(read('mine'), mineGitlab);

  // A button put in by hand, not between the markers, is not doubled, and is left there.
  const byHand = gitlab.replace('.when(w?.buttons', '.child(panelButton("jenkins", "Jenkins", "x.svg"))\n      .when(w?.buttons');
  fs.writeFileSync(mod('tabs-gitlab', 'main.js'), byHand);
  r = run('--panel', 'right');
  assert.equal(r.code, 0, r.out);
  assert.match(r.out, /mods\/tabs-gitlab\/main\.js has a Jenkins button already, not between this installer's markers: left as it is/);
  assert.equal(text('tabs-gitlab'), byHand);
  r = run('--panel', 'off');
  assert.equal(r.code, 0, r.out);
  assert.match(r.out, /a Jenkins button that this installer did not put in: left as it is/);
  assert.equal(text('tabs-gitlab'), byHand);

  // Nowhere to put it, or a check that fails: nothing is written, and it says where the README shows it.
  const layoutBefore = fs.readFileSync(layoutFile('mine'), 'utf8');
  for (const unlike of [gitlab.replace('.when(w?.buttons', '.when(w?.controls'), gitlab.replace('this.syncPanel(space, panel, body, enabled);', ''), gitlab.replace('panelButton = ', 'panelBtn = ')]) {
    fs.writeFileSync(mod('tabs-gitlab', 'main.js'), unlike);
    r = run('--panel', 'right');
    assert.notEqual(r.code, 0);
    assert.match(r.out, /not like Herdr's tab bar.*README\.md.*Nothing was changed/);
    assert.equal(fs.readFileSync(layoutFile('mine'), 'utf8'), layoutBefore);
    assert.equal(text('tabs-gitlab'), unlike);
  }
  fs.writeFileSync(mod('tabs-gitlab', 'main.js'), gitlab);
  fs.writeFileSync(`${checker}.refuse`, '');
  r = run('--panel', 'right');
  assert.notEqual(r.code, 0);
  assert.match(r.out, /herdr-app check refused mods\/tabs-gitlab\/main\.js.*SyntaxError: nope/);
  assert.equal(fs.readFileSync(layoutFile('mine'), 'utf8'), layoutBefore);
  assert.equal(text('tabs-gitlab'), gitlab);
  assert.deepEqual(stagesLeft(), []);
  fs.rmSync(`${checker}.refuse`);

  // The uninstall takes the button out with the panel.
  assert.equal(run('--panel', 'right').code, 0);
  r = run('--uninstall');
  assert.equal(r.code, 0, r.out);
  assert.equal(text('tabs-gitlab'), gitlab);
  assert.deepEqual(read('mine'), mineGitlab);
  assert.ok(!fs.existsSync(path.join(mods, 'jenkins')));

  // A tab bar of Herdr's other than tabs (the Terminals layout's): the panel, and no button.
  fs.writeFileSync(layoutFile('mine'), JSON.stringify(withTabs(mine, 'terminals-bar'), null, 2));
  r = run('--panel', 'right');
  assert.equal(r.code, 0, r.out);
  assert.match(r.out, /Herdr's terminals-bar, which this installer cannot put a button into.*layout\.select --stack panel-body --child jenkins-panel/);
  assert.deepEqual(stackOf(read('mine')).children.at(-1), panel);
  assert.equal(shown('mine'), 'terminals-bar');
  fs.rmSync(home, { recursive: true });
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
