// A made-up Jenkins for the Jenkins mod's tests and for looking at the mod in
// a window: canned folders, jobs, builds and console logs behind the same
// JSON API paths the mod asks a real Jenkins for (`api/json?tree=…`,
// `consoleText`, `logText/progressiveText`, `build`, `buildWithParameters`,
// `stop`, `crumbIssuer`, `whoAmI`, `me`), Pipeline Stage View's
// `wfapi/describe` and `wfapi/runs` in the shapes a real Jenkins
// (2.479, read only) gave, and its `wfapi/pendingInputActions` with the
// input step's answers (`input/<id>/proceedEmpty`, `submit`, `abort`) for a
// build that waits at an input (`fixtures/real-input.json` has what the real
// one served for an answered one). Nothing here reaches a real Jenkins.
//
//   node fake-jenkins.mjs [port]     serves until stopped; prints its address
//
// Two users, by HTTP Basic auth as a real API token is used:
//   jane:fake-token      may read, build, abort and answer an input
//   viewer:viewer-token  may only read: build, abort and an answer get 403
// Anything else is 401. A POST without the crumb the issuer hands out is 403,
// as on a Jenkins with CSRF protection.
//
// It fails on demand, as a Jenkins behind a flaky VPN does: `fail({ mode,
// status, count, path })` makes the next `count` requests (all, by default)
// whose path matches `path` (a RegExp; any by default) be dropped without an
// answer (`refuse`: the client's fetch rejects), held until `release()`
// (`stall`), or answered with `status` (`status`: 503, 401, …); `recover()`
// ends it. Run as a program, `POST /__fake/fail?mode=…&status=…&count=…&path=…`
// and `POST /__fake/recover` do the same (no auth; never counted as requests).

import { createHash } from 'node:crypto';
import http from 'node:http';
import { pathToFileURL } from 'node:url';

export const USERS = {
  jane: { token: 'fake-token', fullName: 'Jane Doe', write: true },
  viewer: { token: 'viewer-token', fullName: 'Read Only', write: false },
};
export const CRUMB = { crumbRequestField: 'Jenkins-Crumb', crumb: 'fake-crumb-1' };

const FOLDER = 'com.cloudbees.hudson.plugins.folder.Folder';
const PIPELINE = 'org.jenkinsci.plugins.workflow.job.WorkflowJob';
const MIN = 60_000;

const BRANCH = { _class: 'hudson.model.StringParameterDefinition', name: 'gitBranch', type: 'StringParameterDefinition', description: 'The branch to build and deploy', defaultParameterValue: { value: 'master' } };
const PARAMS = [
  BRANCH,
  { _class: 'hudson.model.ChoiceParameterDefinition', name: 'site', type: 'ChoiceParameterDefinition', description: 'Where to roll out', defaultParameterValue: { value: 'both' }, choices: ['both', 'north', 'south'] },
  { _class: 'hudson.model.BooleanParameterDefinition', name: 'skipTests', type: 'BooleanParameterDefinition', description: 'Build without running the tests', defaultParameterValue: { value: false } },
  { _class: 'hudson.model.PasswordParameterDefinition', name: 'registryPassword', type: 'PasswordParameterDefinition', description: 'Only for a private registry', defaultParameterValue: {} },
  { _class: 'hudson.model.FileParameterDefinition', name: 'bundle', type: 'FileParameterDefinition', description: 'A file to upload' },
];

// The stages of a Pipeline job, each with its share of a build's time: dev
// builds and rolls out to dev; prod waits for a person's approval first.
const DEV = [['Checkout', 0.05], ['Build', 0.25], ['Test', 0.45], ['Deploy to dev', 0.25]];
const PROD = [['Build', 0.3], ['Deploy to staging', 0.2], ['Approve', 0.15], ['Deploy to prod', 0.35]];
// As the real prod jobs: a dummy roll-out, then release notes, asked for at an input.
const RELEASE = [['Build', 0.3], ['Deploy to dummy', 0.2], ['Release Notes', 0.15], ['Deploy to PROD', 0.35]];
const NOTES = 'Proceeding will generate release notes and auto deploy to production.';
// The stages that wait for a person at an input step.
const ASKS = new Set(['Approve', 'Release Notes']);

// What a stage writes first, by its name.
const OPENING = {
  Checkout: [' > git fetch --tags --force --progress -- https://git.example.invalid/team/service.git', 'Checking out Revision 4f1c2aa (origin/master)'],
  Build: ['+ docker build -t registry.example.invalid/team/service:4f1c2aa .', 'Step 1/9 : FROM golang:1.25 AS build'],
  Test: ['+ go test ./...'],
  Approve: ['[Pipeline] input', 'Deploy to production?', 'Proceed or Abort'],
  // An input with parameters links to its form instead of offering Proceed.
  'Release Notes': ['[Pipeline] input', NOTES, 'Input requested'],
};
const deployOpening = (name) => [`+ kubectl apply -f deploy/${name.replace('Deploy to ', '')}/`, 'deployment.apps/service configured'];
// What a stage that ends a build early writes last, by the build's result.
const ENDING = {
  FAILURE: ['--- FAIL: TestParseSchedule (0.02s)', '    schedule_test.go:41: got 3 rows, want 4 (räksmörgås)', 'FAIL\tteam/service/schedule\t0.412s'],
  UNSTABLE: ['Recording test results', '2 tests failed, 318 passed'],
  ABORTED: ['Aborted by Jane Doe', 'Sending interrupt signal to process'],
};
const STATUS = { SUCCESS: 'SUCCESS', FAILURE: 'FAILED', UNSTABLE: 'UNSTABLE', ABORTED: 'ABORTED' };
const ERROR = {
  FAILURE: { message: 'script returned exit code 1', type: 'hudson.AbortException' },
  ABORTED: { message: 'Aborted by Jane Doe', type: 'org.jenkinsci.plugins.workflow.steps.FlowInterruptedException' },
  // Abort at an input step, as the real one says it.
  REJECTED: { message: 'Rejected by Jane Doe', type: 'org.jenkinsci.plugins.workflow.steps.FlowInterruptedException' },
};

// A build's stages as Pipeline has run them `now`: { name, node, start, end
// (null while it runs), status, error, ticks (the ms its log lines tick
// over) }, as many as have started. A running build follows its estimate; a
// finished one fits its stages into its duration and stops at the stage it
// ended in (`build.at`, else Test for a failure, the last stage else);
// `build.paused` names the stage that waits for input; `build.skip` the ones
// its `when` passed over.
function timeline(job, build, now) {
  const plan = job.stages ?? [];
  if (!plan.length) return [];
  const result = build.result, last = plan.length - 1;
  const named = (name) => {
    const i = plan.findIndex(([n]) => n === name);
    return i < 0 ? last : i;
  };
  const endAt = !result || result === 'SUCCESS' ? last : named(build.at ?? (result === 'ABORTED' ? '' : 'Test'));
  const through = result && result !== 'UNSTABLE' ? endAt : last; // an unstable build goes on
  const total = result ? build.duration : build.estimated ?? job.typical ?? 10 * MIN;
  const share = plan.slice(0, through + 1).reduce((sum, [, part]) => sum + part, 0);
  const out = [];
  let at = build.timestamp;
  for (let i = 0; i <= through; i++) {
    const [name, part] = plan[i];
    const length = Math.round((total * part) / (result ? share : 1));
    const stage = { name, node: String(4 + i * 25), start: at, end: at + length, status: 'SUCCESS', error: null, ticks: length };
    if (build.skip?.includes(name)) Object.assign(stage, { end: at + 40, status: 'NOT_EXECUTED', ticks: 0 });
    if (!result) {
      if (stage.start > now) break;
      if (build.paused === name) Object.assign(stage, { end: null, status: 'PAUSED_PENDING_INPUT', ticks: 0 });
      else if (stage.end > now) Object.assign(stage, { end: null, status: 'IN_PROGRESS', ticks: now - stage.start });
    } else if (i === endAt && result !== 'SUCCESS') {
      Object.assign(stage, { status: STATUS[result], error: (build.rejected && ERROR.REJECTED) || (ERROR[result] ?? null) });
    }
    if (ASKS.has(name)) stage.ticks = 0;
    out.push(stage);
    if (stage.end === null) break;
    at = stage.end;
  }
  return out;
}

// The lines a build has written by `now`: a Pipeline's stages between the
// markers Jenkins writes (`[Pipeline] { (Build)` … `[Pipeline] // stage`), a
// line every two seconds of each, and the end once it has finished; a
// freestyle job's steps, plainly.
function logLines(job, build, now) {
  const lines = [`Started by user ${USERS[build.by ?? 'jane'].fullName}`];
  let tick = 0;
  const ticks = (ms, line) => {
    for (let i = Math.floor(Math.max(0, ms) / 2000); i > 0 && tick < 4000; i--) lines.push(line(++tick));
  };
  const elapsed = (build.result ? build.duration : now - build.timestamp);
  if (!job.stages) {
    lines.push('Running as SYSTEM', '[nightly-cleanup] $ /bin/sh -xe /tmp/jenkins7731.sh');
    ticks(elapsed, (i) => `+ rm -rf /var/cache/build/${String(i).padStart(4, '0')}`);
    if (build.result) lines.push(`Finished: ${build.result}`);
    return lines;
  }
  lines.push('[Pipeline] Start of Pipeline', '[Pipeline] node', 'Running on agent-7 in /var/jenkins/workspace/job', '[Pipeline] {');
  for (const stage of timeline(job, build, now)) {
    lines.push('[Pipeline] stage', `[Pipeline] { (${stage.name})`);
    if (stage.status === 'NOT_EXECUTED') lines.push(`Stage "${stage.name}" skipped due to when conditional`);
    else {
      lines.push(...(OPENING[stage.name] ?? deployOpening(stage.name)));
      ticks(stage.ticks, stage.name === 'Test'
        ? (i) => `ok  \tteam/service/pkg${String(i).padStart(4, '0')}\t0.${String((i * 37) % 1000).padStart(3, '0')}s`
        : (i) => `${stage.name}: step ${i} done`);
      if (ASKS.has(stage.name) && stage.end !== null && !build.rejected) lines.push(`Approved by ${USERS[build.answer?.by ?? 'jane'].fullName}`);
      if (build.rejected && stage.status === 'ABORTED') lines.push(`Rejected by ${USERS[build.answer?.by ?? 'jane'].fullName}`);
      else if (ENDING[build.result] && STATUS[build.result] === stage.status) lines.push(...ENDING[build.result]);
      else if (stage.name.startsWith('Deploy') && stage.end !== null) lines.push('deployment "service" successfully rolled out');
    }
    if (stage.end === null) return lines; // still at it
    lines.push('[Pipeline] }', '[Pipeline] // stage');
  }
  if (!build.result) return lines;
  lines.push('[Pipeline] }', '[Pipeline] // node', '[Pipeline] End of Pipeline');
  if (build.result === 'FAILURE') lines.push('ERROR: script returned exit code 1');
  lines.push(`Finished: ${build.result}`);
  return lines;
}

// Pipeline Stage View's account of a build (`wfapi/describe`), as the real
// plugin gives it: the build's status, and each stage that has started.
function describe(job, build, now, href) {
  const stages = timeline(job, build, now);
  const status = build.result ? STATUS[build.result] ?? 'SUCCESS'
    : stages.some((s) => s.status === 'PAUSED_PENDING_INPUT') ? 'PAUSED_PENDING_INPUT' : 'IN_PROGRESS';
  const end = build.result ? build.timestamp + build.duration : now;
  return {
    _links: { self: { href: `${href}wfapi/describe` } },
    id: String(build.number), name: `#${build.number}`, status,
    startTimeMillis: build.timestamp, endTimeMillis: end, durationMillis: end - build.timestamp, queueDurationMillis: 2,
    pauseDurationMillis: stages.filter((s) => s.status === 'PAUSED_PENDING_INPUT').reduce((sum, s) => sum + (now - s.start), 0),
    stages: stages.map((s) => ({
      _links: { self: { href: `${href}execution/node/${s.node}/wfapi/describe` } },
      id: s.node, name: s.name, execNode: '', status: s.status,
      ...(s.error ? { error: s.error } : {}),
      startTimeMillis: s.start, durationMillis: (s.end ?? now) - s.start,
      pauseDurationMillis: s.status === 'PAUSED_PENDING_INPUT' ? now - s.start : 0,
    })),
  };
}

// `a,b[c,d[e]]{0,10}` as Jenkins reads `tree`: the named fields alone, a
// list cut to its range.
function parseTree(text) {
  let at = 0;
  const fields = () => {
    const out = [];
    for (;;) {
      let name = '';
      while (at < text.length && !',[]{}'.includes(text[at])) name += text[at++];
      const field = { name: name.trim(), children: null, range: null };
      if (text[at] === '[') {
        at += 1;
        field.children = fields();
        at += 1; // ]
      }
      if (text[at] === '{') {
        const end = text.indexOf('}', at);
        const [from, to] = text.slice(at + 1, end).split(',');
        field.range = [Number(from || 0), to === undefined || to === '' ? Infinity : Number(to)];
        at = end + 1;
      }
      if (field.name) out.push(field);
      if (text[at] !== ',') return out;
      at += 1;
    }
  };
  return fields();
}

function prune(value, fields) {
  if (Array.isArray(value)) return value.map((item) => prune(item, fields));
  if (value === null || typeof value !== 'object' || !fields) return value;
  const out = {};
  if ('_class' in value) out._class = value._class;
  for (const field of fields) {
    if (!(field.name in value)) continue;
    let inner = value[field.name];
    if (Array.isArray(inner) && field.range) inner = inner.slice(field.range[0], field.range[1]);
    out[field.name] = field.children ? prune(inner, field.children) : inner;
  }
  return out;
}

// The input steps the waiting builds ask, as Pipeline's `input` step holds
// them: its id, message, proceed button and parameters. A step that names no
// id gets the MD5 of its message, its first letter a capital (the real one:
// `Faf07b7bb8539aeb2d643328e8f89389`).
const inputId = (message) => {
  const hex = createHash('md5').update(message).digest('hex');
  return hex[0].toUpperCase() + hex.slice(1);
};
const APPROVE = { id: inputId('Deploy to production?'), message: 'Deploy to production?', proceedText: 'Proceed', parameters: [] };
const RELEASE_NOTES = {
  id: inputId(NOTES), message: NOTES, proceedText: 'Release', parameters: [
    { _class: 'hudson.model.ChoiceParameterDefinition', name: 'jiraEnv', type: 'ChoiceParameterDefinition', description: 'Where the release request goes', defaultParameterValue: { _class: 'hudson.model.StringParameterValue', name: 'jiraEnv', value: 'prod' }, choices: ['prod', 'test'] },
    { _class: 'hudson.model.StringParameterDefinition', name: 'summary', type: 'StringParameterDefinition', description: 'One line for the release notes', defaultParameterValue: { _class: 'hudson.model.StringParameterValue', name: 'summary', value: '' } },
    { _class: 'hudson.model.BooleanParameterDefinition', name: 'notify', type: 'BooleanParameterDefinition', description: 'Tell the channel', defaultParameterValue: { _class: 'hudson.model.BooleanParameterValue', name: 'notify', value: true } },
  ],
};

const finished = (number, result, agoMin, tookMin, extra = {}) => ({ number, result, ago: agoMin * MIN, duration: tookMin * MIN, ...extra });

// The made-up server's jobs. A build's `ago` is when it started, before the
// server did; a running one has no result.
function seed() {
  const job = (name, builds, extra = {}) => ({ kind: 'job', name, builds, parameters: [BRANCH], disabled: false, queued: null, stages: DEV, ...extra });
  const prod = (name, builds, extra = {}) => job(name, builds, { stages: PROD, ...extra });
  const folder = (name, children) => ({ kind: 'folder', name, children });
  return folder('', [
    folder('team-dev', [
      job('api_master', [finished(128, 'SUCCESS', 185, 6, { parameters: { gitBranch: 'master' } }), finished(127, 'FAILURE', 1500, 4, { parameters: { gitBranch: 'feature/export-columns' } }), finished(126, 'SUCCESS', 3000, 6, { skip: ['Test'] }), finished(125, 'ABORTED', 4400, 1), finished(124, 'SUCCESS', 9000, 7)], { parameters: PARAMS, description: 'Builds the API agent and rolls it out to dev' }),
      job('poller_master', [{ number: 57, result: null, ago: 2 * MIN, estimated: 6 * MIN, parameters: { gitBranch: 'worktree/lucky-pine' } }, finished(56, 'SUCCESS', 700, 6), finished(55, 'SUCCESS', 2100, 5)]),
      job('parser_master', [finished(301, 'FAILURE', 42, 3, { at: 'Build' }), finished(300, 'SUCCESS', 1400, 5)]),
      job('alarm-notify_master', [finished(88, 'UNSTABLE', 300, 9), finished(87, 'SUCCESS', 2000, 8)]),
      job('reports_master', [finished(19, 'ABORTED', 60 * 24 * 3, 2), finished(18, 'SUCCESS', 60 * 24 * 9, 12)]),
      job('a-job-with-a-rather-long-name-for-a-narrow-panel_master', [finished(4, 'SUCCESS', 60 * 24 * 20, 1)], { parameters: [] }),
      job('new-service_master', []),
      job('legacy_master', [finished(912, 'SUCCESS', 60 * 24 * 200, 15)], { disabled: true }),
      folder('tools', [job('rotate-certs', [finished(7, 'SUCCESS', 600, 1)], { parameters: [] })]),
    ]),
    folder('team-prod', [
      prod('api_master', [finished(64, 'SUCCESS', 60 * 26, 14)]),
      prod('poller_master', [finished(31, 'SUCCESS', 60 * 50, 13)]),
      // Waits for a person to approve the roll-out: it never ends by itself.
      prod('alarm-notify_master', [{ number: 12, result: null, ago: 9 * MIN, paused: 'Approve', input: APPROVE, parameters: { gitBranch: 'master' } }, finished(11, 'SUCCESS', 60 * 30, 12)], { typical: 12 * MIN }),
      // Waits at its release notes, which ask for values first.
      job('exporter_master', [{ number: 41, result: null, ago: 25 * MIN, paused: 'Release Notes', input: RELEASE_NOTES, parameters: { gitBranch: 'master' } }, finished(40, 'SUCCESS', 60 * 48, 11)], { stages: RELEASE, typical: 11 * MIN }),
    ]),
    job('nightly-cleanup', [finished(2210, 'SUCCESS', 400, 2)], { parameters: [], kindClass: 'hudson.model.FreeStyleProject', stages: null }),
  ]);
}

/**
 * Starts the server. `options`:
 *   port       0 (any free one) by default
 *   clock      () => ms, the server's time (Date.now)
 *   crumbs     false: a Jenkins without CSRF protection (no crumb issuer, none asked)
 *   queueMs    how long a triggered build waits in the queue (1500)
 *   stageView  false: a Jenkins without Pipeline Stage View (wfapi answers 404)
 * Resolves with { url, port, requests, root, find, setCrumb, fail, recover, release, close }.
 */
export async function startFakeJenkins(options = {}) {
  const clock = options.clock ?? Date.now;
  const crumbs = options.crumbs ?? true;
  const queueMs = options.queueMs ?? 1500;
  const stageView = options.stageView ?? true;
  const started = clock();
  const root = seed();
  const requests = []; // { method, path, query, user, body, status, crumb }; status "refused" or "stalled" in an outage
  let outage = null; // { mode, status, left, path }
  let held = []; // the stalled requests' ways on: release() takes them
  let crumb = CRUMB.crumb; // setCrumb: the issuer hands out another, and the old one is refused
  let base = '';

  const walkBuilds = (node, visit) => {
    if (node.kind === 'job') node.builds.forEach(visit);
    else node.children.forEach((child) => walkBuilds(child, visit));
  };
  walkBuilds(root, (build) => {
    build.timestamp = started - build.ago;
  });

  const find = (names) => {
    let node = root;
    for (const name of names) {
      node = node.kind === 'folder' ? node.children.find((child) => child.name === name) : null;
      if (!node) return null;
    }
    return node;
  };

  // Moves what time moves: a queued build starts, a running one finishes.
  const settle = (job) => {
    const now = clock();
    if (job.queued && now - job.queued.at >= queueMs) {
      const number = (job.builds[0]?.number ?? 0) + 1;
      job.builds.unshift({ number, result: null, timestamp: job.queued.at + queueMs, estimated: 60_000, parameters: job.queued.parameters, by: job.queued.by });
      job.queued = null;
    }
    for (const build of job.builds) {
      if (build.result === null && build.estimated && now - build.timestamp >= build.estimated) {
        build.result = 'SUCCESS';
        build.duration = build.estimated;
      }
    }
  };

  const jobPath = (names) => names.map((name) => `job/${encodeURIComponent(name)}/`).join('');
  const color = (job) => {
    if (job.disabled) return 'disabled';
    const [last, previous] = job.builds;
    if (!last) return 'notbuilt';
    const of = (b) => ({ SUCCESS: 'blue', FAILURE: 'red', UNSTABLE: 'yellow', ABORTED: 'aborted' })[b?.result] ?? 'notbuilt';
    return last.result === null ? `${of(previous)}_anime` : of(last);
  };
  const buildJson = (names, job, build) => ({
    _class: 'org.jenkinsci.plugins.workflow.job.WorkflowRun',
    number: build.number,
    displayName: `#${build.number}`,
    fullDisplayName: `${names.join(' » ')} #${build.number}`,
    url: `${base}/${jobPath(names)}${build.number}/`,
    building: build.result === null,
    result: build.result,
    timestamp: build.timestamp,
    duration: build.result === null ? 0 : build.duration,
    estimatedDuration: build.estimated ?? job.builds.find((b) => b.result === 'SUCCESS')?.duration ?? -1,
    actions: [
      { _class: 'hudson.model.CauseAction', causes: [{ _class: 'hudson.model.Cause$UserIdCause', shortDescription: `Started by user ${USERS[build.by ?? 'jane'].fullName}`, userId: build.by ?? 'jane' }] },
      ...(build.parameters ? [{ _class: 'hudson.model.ParametersAction', parameters: Object.entries(build.parameters).map(([name, value]) => ({ _class: 'hudson.model.StringParameterValue', name, value })) }] : []),
      {},
    ],
  });
  // An input step as Pipeline Stage View's `pendingInputActions` lists it.
  const inputJson = (names, build, input) => {
    const at = `/${jobPath(names)}${build.number}/`;
    return {
      id: input.id,
      proceedText: input.proceedText,
      message: input.message,
      inputs: input.parameters.map((d) => ({ type: d.type, name: d.name, description: d.description, definition: d })),
      proceedUrl: `${at}wfapi/inputSubmit?inputId=${input.id}`,
      abortUrl: `${at}input/${input.id}/abort`,
      redirectApprovalUrl: `${at}input/`,
    };
  };
  const itemJson = (names, node) => {
    const url = `${base}/${jobPath(names)}`;
    if (node.kind === 'folder') {
      return { _class: names.length ? FOLDER : 'hudson.model.Hudson', name: node.name, displayName: node.name, fullName: names.join('/'), url, jobs: node.children.map((child) => itemJson([...names, child.name], child)) };
    }
    settle(node);
    const builds = node.builds.map((build) => buildJson(names, node, build));
    const last = (pick) => builds.find(pick) ?? null;
    return {
      _class: node.kindClass ?? PIPELINE,
      name: node.name,
      displayName: node.name,
      fullName: names.join('/'),
      description: node.description ?? '',
      url,
      color: color(node),
      buildable: !node.disabled,
      inQueue: Boolean(node.queued),
      builds,
      lastBuild: builds[0] ?? null,
      lastCompletedBuild: last((b) => !b.building),
      lastSuccessfulBuild: last((b) => b.result === 'SUCCESS'),
      property: node.parameters.length ? [{ _class: 'jenkins.model.BuildDiscarderProperty' }, { _class: 'hudson.model.ParametersDefinitionProperty', parameterDefinitions: node.parameters }] : [{ _class: 'jenkins.model.BuildDiscarderProperty' }],
    };
  };

  const send = (res, status, body, headers = {}) => {
    const text = typeof body === 'string' ? body : JSON.stringify(body);
    res.writeHead(status, { 'Content-Type': typeof body === 'string' ? 'text/plain;charset=UTF-8' : 'application/json;charset=utf-8', 'X-Jenkins': '2.479.1', ...headers });
    res.end(text);
    return status;
  };

  const fail = ({ mode = 'refuse', status = 503, count = Infinity, path = null } = {}) => {
    outage = { mode, status: Number(status), left: Number(count), path: path ? new RegExp(path) : null };
  };
  const recover = () => {
    outage = null;
  };
  // The stalled requests are answered now, as if Jenkins had come back.
  const release = () => {
    const ways = held;
    held = [];
    for (const go of ways) go();
  };

  const handle = (req, res, body, stalled = false) => {
    const url = new URL(req.url, base);
    if (url.pathname.startsWith('/__fake/')) {
      if (url.pathname === '/__fake/fail') fail(Object.fromEntries(url.searchParams));
      else if (url.pathname === '/__fake/recover') recover();
      else if (url.pathname === '/__fake/release') release();
      else return send(res, 404, 'Not found');
      return send(res, 200, 'ok');
    }
    const entry = { method: req.method, path: decodeURIComponent(url.pathname), query: Object.fromEntries(url.searchParams), user: null, body, status: 0 };
    if (!stalled) requests.push(entry);
    if (!stalled && outage && outage.left > 0 && (!outage.path || outage.path.test(entry.path))) {
      outage.left -= 1;
      if (outage.mode === 'refuse') {
        entry.status = 'refused';
        req.socket.destroy();
        return;
      }
      if (outage.mode === 'stall') {
        entry.status = 'stalled';
        held.push(() => {
          if (!res.writableEnded && !res.destroyed) handle(req, res, body, true);
        });
        return;
      }
      entry.status = send(res, outage.status, outage.status === 401 ? 'Unauthorized' : outage.status === 403 ? 'Forbidden' : 'Service Unavailable');
      return;
    }
    const done = (status, payload, headers) => {
      entry.status = send(res, status, payload, headers);
    };

    const auth = /^Basic (.+)$/.exec(req.headers.authorization ?? '');
    if (!auth) return done(403, 'Authentication required');
    const [name, ...rest] = Buffer.from(auth[1], 'base64').toString('utf8').split(':');
    const user = USERS[name];
    if (!user || user.token !== rest.join(':')) return done(401, 'Unauthorized');
    entry.user = name;

    const parts = url.pathname.split('/').filter(Boolean).map(decodeURIComponent);
    const tree = url.searchParams.get('tree');
    const json = (value) => done(200, tree ? prune(value, parseTree(tree)) : value);

    if (parts.join('/') === 'whoAmI/api/json') return json({ _class: 'hudson.security.WhoAmI', anonymous: false, authenticated: true, authorities: ['authenticated'], name });
    if (parts.join('/') === 'me/api/json') return json({ _class: 'hudson.model.User', id: name, fullName: user.fullName, absoluteUrl: `${base}/user/${name}` });
    if (parts.join('/') === 'crumbIssuer/api/json') return crumbs ? json({ _class: 'hudson.security.csrf.DefaultCrumbIssuer', ...CRUMB, crumb }) : done(404, 'Not found');

    // /job/a/job/b/<rest…>
    const names = [];
    let at = 0;
    while (parts[at] === 'job' && at + 1 < parts.length) {
      names.push(parts[at + 1]);
      at += 2;
    }
    const node = find(names);
    if (!node) return done(404, 'Not found');
    const tail = parts.slice(at);
    if (node.kind === 'job') settle(node);

    if (req.method === 'POST') {
      entry.crumb = req.headers[CRUMB.crumbRequestField.toLowerCase()] ?? null;
      if (crumbs && entry.crumb !== crumb) return done(403, 'No valid crumb was included in the request');
      if (node.kind !== 'job') return done(404, 'Not found');
      if (!user.write) return done(403, `${name} is missing the Job/Build permission`);
      const action = tail.join('/');
      if (action === 'build' || action === 'buildWithParameters') {
        if (node.disabled) return done(409, 'The job is disabled');
        const given = { ...Object.fromEntries(url.searchParams), ...Object.fromEntries(new URLSearchParams(body)) };
        if (action === 'build' && node.parameters.length) return done(400, 'This job takes parameters: use buildWithParameters');
        const parameters = {};
        for (const def of node.parameters) parameters[def.name] = given[def.name] ?? def.defaultParameterValue?.value ?? '';
        node.queued = { at: clock(), parameters: action === 'build' ? null : parameters, by: name };
        return done(201, '', { Location: `${base}/queue/item/${requests.length}/` });
      }
      // An input step's answers: Proceed with nothing (`proceedEmpty`), the
      // form (`submit`: `json` with the values, and `proceed`, the button,
      // without which Jenkins takes the form as an abort), or `abort`. An
      // input no longer waiting (answered, the build ended) is 404.
      if (tail.length === 4 && tail[1] === 'input') {
        const build = node.builds.find((b) => String(b.number) === tail[0]);
        const input = build && build.result === null && build.paused && build.input?.id === tail[2] ? build.input : null;
        if (!input || !['proceedEmpty', 'submit', 'abort'].includes(tail[3])) return done(404, 'Not found');
        const form = new URLSearchParams(body);
        let parameters = null;
        if (tail[3] === 'submit') {
          try {
            parameters = JSON.parse(form.get('json') ?? '{}').parameter ?? [];
          } catch {
            return done(400, 'The form is not JSON');
          }
          const unknown = parameters.find((p) => !input.parameters.some((d) => d.name === p?.name));
          if (unknown) return done(400, `No such parameter definition: ${unknown?.name}`);
        }
        const proceed = tail[3] === 'proceedEmpty' || (tail[3] === 'submit' && form.has('proceed'));
        const stage = build.paused;
        build.answer = { action: proceed ? 'proceed' : 'abort', how: tail[3], parameters, button: form.get('proceed'), by: name };
        build.paused = null;
        if (proceed) {
          // It goes on: the rest of its stages take another minute.
          build.estimated = clock() - build.timestamp + 60_000;
        } else {
          Object.assign(build, { at: stage, rejected: true, result: 'ABORTED', duration: clock() - build.timestamp });
        }
        return done(302, '', { Location: `${base}/${jobPath(names)}${build.number}/` });
      }
      if (tail.length === 2 && tail[1] === 'stop') {
        const build = node.builds.find((b) => String(b.number) === tail[0]);
        if (!build) return done(404, 'Not found');
        if (build.result === null) {
          build.at = timeline(node, build, clock()).at(-1)?.name;
          build.result = 'ABORTED';
          build.duration = clock() - build.timestamp;
        }
        return done(302, '', { Location: `${base}/${jobPath(names)}${build.number}/` });
      }
      return done(404, 'Not found');
    }

    if (tail.join('/') === 'api/json') return json(itemJson(names, node));
    // Pipeline Stage View: a Pipeline job's last ten runs; a freestyle job has none (404).
    if (tail.join('/') === 'wfapi/runs') {
      if (node.kind !== 'job' || !node.stages || !stageView) return done(404, 'Not found');
      return done(200, node.builds.slice(0, 10).map((b) => describe(node, b, clock(), `/${jobPath(names)}${b.number}/`)));
    }
    if (tail.length === 0) return done(200, `The page of ${names.join('/') || 'Jenkins'}`);
    if (node.kind !== 'job') return done(404, 'Not found');
    const build = tail[0] === 'lastBuild' ? node.builds[0] : node.builds.find((b) => String(b.number) === tail[0]);
    if (!build) return done(404, 'Not found');
    const rest2 = tail.slice(1).join('/');
    if (rest2 === '') return done(200, `The page of ${names.join('/')} #${build.number}`);
    if (rest2 === 'api/json') return json(buildJson(names, node, build));
    if (rest2 === 'wfapi/pendingInputActions') {
      if (!node.stages || !stageView) return done(404, 'Not found');
      const input = build.result === null && build.paused ? build.input : null;
      return done(200, input ? [inputJson(names, build, input)] : []);
    }
    if (rest2 === 'wfapi/describe') return node.stages && stageView ? done(200, describe(node, build, clock(), `/${jobPath(names)}${build.number}/`)) : done(404, 'Not found');
    const bytes = Buffer.from(`${logLines(node, build, clock()).join('\n')}\n`, 'utf8');
    if (rest2 === 'consoleText') return done(200, bytes.toString('utf8'));
    if (rest2 === 'logText/progressiveText') {
      let from = Number(url.searchParams.get('start') ?? 0);
      if (!(from >= 0) || from > bytes.length) from = 0; // "the text rolled over", as Jenkins does
      return done(200, bytes.subarray(from).toString('utf8'), { 'X-Text-Size': String(bytes.length), ...(build.result === null ? { 'X-More-Data': 'true' } : {}) });
    }
    return done(404, 'Not found');
  };

  const server = http.createServer((req, res) => {
    const chunks = [];
    req.on('data', (chunk) => chunks.push(chunk));
    req.on('end', () => {
      try {
        handle(req, res, Buffer.concat(chunks).toString('utf8'));
      } catch (error) {
        send(res, 500, String(error?.stack ?? error));
      }
    });
  });
  await new Promise((resolve, reject) => {
    server.once('error', reject);
    server.listen(options.port ?? 0, '127.0.0.1', resolve);
  });
  const port = server.address().port;
  base = `http://127.0.0.1:${port}`;
  return {
    url: base,
    port,
    requests,
    root,
    find,
    setCrumb: (next) => {
      crumb = next;
    },
    fail,
    recover,
    release,
    close: () => new Promise((resolve) => {
      server.closeAllConnections?.();
      server.close(resolve);
    }),
  };
}

if (import.meta.url === pathToFileURL(process.argv[1] ?? '').href) {
  const fake = await startFakeJenkins({ port: Number(process.argv[2] ?? 0) });
  console.log(`fake Jenkins on ${fake.url} (jane:fake-token builds, viewer:viewer-token only reads)`);
}
