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
import { execFileSync } from 'node:child_process';
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
  herdr: ['theme', 'settings', 'home', 'appHome', 'nextCommand', 'readTextFile', 'writeTextFile', 'exec']
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

const util = await import(new URL('../util.js', import.meta.url));

let copies = 0;
// The mod, started in the world `w`: its view, and what its tasks and timers are.
async function start(w) {
  globalThis.__world = w;
  const module = await import(new URL(`../main.js?copy=${copies++}`, import.meta.url));
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
  const view = new module.default();
  view.init({}, cx);
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
  assert.deepEqual(util.parsePlaces(`job/team/job/tools/\n${base}/job/instat-develop/, view/Deploys, team-dev,team-dev`, base), [
    { path: '/job/team/job/tools', label: 'team/tools' }, { path: '/job/instat-develop', label: 'instat-develop' },
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
  assert.deepEqual([reads(), prod()], [2, 1]);
  skew += 60_000;
  await mod.tick();
  assert.deepEqual([reads(), prod()], [3, 2]);

  // A folder in the list opens as a list of its own; the browser gets the folder's page.
  await mod.click(item(['team-dev', 'tools']));
  assert.equal(row(mod, item(['team-dev', 'tools', 'rotate-certs'])), 'rotate-certs #7 · 10h');
  await mod.click('jk-open-/job/team-dev/job/tools');
  assert.deepEqual(w.opened, [`${fake.url}/job/team-dev/job/tools/`]);
  await mod.key('escape');
  assert.ok(mod.says('team-prod'));
  await fake.close();
});

test('a job\'s builds, and a finished build\'s log', async () => {
  const fake = await startFakeJenkins();
  const w = configured(fake);
  const mod = await start(w);
  await mod.click(item(['team-dev', 'api_master']));
  assert.ok(mod.says('Builds the API agent and rolls it out to dev'));
  assert.equal(row(mod, 'jk-build-128'), '#128 gitBranch=master 6m 00s · 3h');
  assert.equal(row(mod, 'jk-build-127'), '#127 gitBranch=feature/export-columns 4m 00s · 1d');
  assert.equal(row(mod, 'jk-build-126'), '#126 Started by user Jane Doe 6m 00s · 2d');
  assert.equal(byId(mod.render(), 'jk-build-128-abort'), null, 'a finished build has nothing to abort');
  assert.ok(fake.requests.at(-1).query.tree.includes('builds[') && fake.requests.at(-1).query.tree.endsWith('{0,20}'));

  await mod.click('jk-build-127');
  const lines = texts(byId(mod.render(), 'jk-log-area'));
  assert.equal(lines[0], 'Started by user Jane Doe');
  assert.equal(lines.at(-1), 'Finished: FAILURE');
  assert.ok(lines.includes('    schedule_test.go:41: got 3 rows, want 4 (räksmörgås)'));
  assert.ok(lines.includes('FAIL    team/service/schedule    0.412s'), 'tabs are spaces');
  assert.ok(mod.says('api_master #127') && mod.says('failed') && mod.says('gitBranch=feature/export-columns'));
  assert.ok(mod.says(`${lines.length} lines`));
  assert.equal(byId(mod.render(), 'jk-abort'), null);
  assert.ok(mod.view.scroll.bottoms > 0, 'it shows the end of the log');
  const reads = fake.requests.length;
  await mod.tick();
  await mod.tick();
  assert.equal(fake.requests.length, reads, 'a finished build is not read again');

  await mod.click('jk-build-open');
  assert.deepEqual(w.opened, [`${fake.url}/job/team-dev/job/api_master/127/`]);
  await mod.key('escape');
  await mod.click('jk-job-open');
  assert.equal(w.opened[1], `${fake.url}/job/team-dev/job/api_master/`);
  await mod.key('escape');
  assert.ok(mod.says('team-prod'), 'Esc twice is the list again');
  await fake.close();
});

test('a running build\'s log is followed from where the last read ended, and read whole once it ends', async () => {
  const fake = await startFakeJenkins();
  const mod = await start(configured(fake));
  await mod.click(item(['team-dev', 'poller_master']));
  assert.match(row(mod, 'jk-build-57'), /^#57 gitBranch=worktree\/lucky-pine 3\d%$/);
  assert.ok(byId(mod.render(), 'jk-build-57-abort'), 'a running build can be aborted');
  await mod.click('jk-build-57');
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
  assert.equal(new Set(second).size, second.length, 'no line twice');
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
  await mod.click('jk-build-57');
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

test('a Jenkins that does not answer: the list says so and keeps what it had', async () => {
  const fake = await startFakeJenkins();
  const mod = await start(configured(fake));
  assert.equal(row(mod, item(['team-dev', 'api_master'])), 'api_master #128 · 3h');
  await fake.close();
  skew += 61_000;
  await mod.tick();
  const said = mod.texts().filter((line) => line.startsWith('Could not reach Jenkins to list the jobs of team-dev: '));
  assert.equal(said.length, 1, mod.texts().join('|'));
  assert.ok(!said[0].includes(TOKEN) && !said[0].includes('Basic'));
  assert.match(row(mod, item(['team-dev', 'api_master'])), /^api_master #128/, 'the rows stay');
  await mod.click(item(['team-dev', 'api_master']));
  assert.ok(mod.says('Could not reach Jenkins to read api_master: '));
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
  assert.deepEqual(fs.readdirSync(target).sort(), ['api.js', 'icons', 'main.js', 'mod.json', 'ui.js', 'util.js'], 'no tests, no README, nothing the check wrote');
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
