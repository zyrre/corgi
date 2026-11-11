// A made-up Jenkins for the Jenkins mod's tests and for looking at the mod in
// a window: canned folders, jobs, builds and console logs behind the same
// JSON API paths the mod asks a real Jenkins for (`api/json?tree=…`,
// `consoleText`, `logText/progressiveText`, `build`, `buildWithParameters`,
// `stop`, `crumbIssuer`, `whoAmI`, `me`). Nothing here reaches a real Jenkins.
//
//   node fake-jenkins.mjs [port]     serves until stopped; prints its address
//
// Two users, by HTTP Basic auth as a real API token is used:
//   jane:fake-token      may read, build and abort
//   viewer:viewer-token  may only read: build and abort answer 403
// Anything else is 401. A POST without the crumb the issuer hands out is 403,
// as on a Jenkins with CSRF protection.

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

const LOG_HEAD = [
  'Started by user Jane Doe',
  '[Pipeline] Start of Pipeline',
  '[Pipeline] node',
  'Running on agent-7 in /var/jenkins/workspace/job',
  '[Pipeline] stage (Checkout)',
  ' > git fetch --tags --force --progress -- https://git.example.invalid/team/service.git',
  'Checking out Revision 4f1c2aa (origin/master)',
  '[Pipeline] stage (Build)',
  '+ docker build -t registry.example.invalid/team/service:4f1c2aa .',
  'Step 1/9 : FROM golang:1.25 AS build',
  'Step 2/9 : COPY . /src',
  'Step 3/9 : RUN go build ./cmd/service',
];
const LOG_TAIL = {
  SUCCESS: ['[Pipeline] stage (Deploy)', '+ kubectl rollout status deployment/service-dev', 'deployment "service-dev" successfully rolled out', '[Pipeline] End of Pipeline', 'Finished: SUCCESS'],
  FAILURE: ['--- FAIL: TestParseSchedule (0.02s)', '    schedule_test.go:41: got 3 rows, want 4 (räksmörgås)', 'FAIL\tteam/service/schedule\t0.412s', 'ERROR: script returned exit code 1', '[Pipeline] End of Pipeline', 'Finished: FAILURE'],
  UNSTABLE: ['Recording test results', '2 tests failed, 318 passed', '[Pipeline] End of Pipeline', 'Finished: UNSTABLE'],
  ABORTED: ['Aborted by Jane Doe', 'Sending interrupt signal to process', '[Pipeline] End of Pipeline', 'Finished: ABORTED'],
};

// The lines a build has written `elapsed` ms after it started: the head, a
// line every two seconds while it runs, and its result's tail once finished.
function logLines(build, elapsed) {
  const lines = [...LOG_HEAD];
  const steps = Math.max(0, Math.floor(Math.min(elapsed, build.result ? build.duration : elapsed) / 2000));
  for (let i = 1; i <= Math.min(steps, 4000); i++) lines.push(`ok  \tteam/service/pkg${String(i).padStart(4, '0')}\t0.${String((i * 37) % 1000).padStart(3, '0')}s`);
  if (build.result) lines.push(...(LOG_TAIL[build.result] ?? LOG_TAIL.SUCCESS));
  return lines;
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

const finished = (number, result, agoMin, tookMin, extra = {}) => ({ number, result, ago: agoMin * MIN, duration: tookMin * MIN, ...extra });

// The made-up server's jobs. A build's `ago` is when it started, before the
// server did; a running one has no result.
function seed() {
  const job = (name, builds, extra = {}) => ({ kind: 'job', name, builds, parameters: [BRANCH], disabled: false, queued: null, ...extra });
  const folder = (name, children) => ({ kind: 'folder', name, children });
  return folder('', [
    folder('team-dev', [
      job('api_master', [finished(128, 'SUCCESS', 185, 6, { parameters: { gitBranch: 'master' } }), finished(127, 'FAILURE', 1500, 4, { parameters: { gitBranch: 'feature/export-columns' } }), finished(126, 'SUCCESS', 3000, 6), finished(125, 'ABORTED', 4400, 1), finished(124, 'SUCCESS', 9000, 7)], { parameters: PARAMS, description: 'Builds the API agent and rolls it out to dev' }),
      job('poller_master', [{ number: 57, result: null, ago: 2 * MIN, estimated: 6 * MIN, parameters: { gitBranch: 'worktree/lucky-pine' } }, finished(56, 'SUCCESS', 700, 6), finished(55, 'SUCCESS', 2100, 5)]),
      job('parser_master', [finished(301, 'FAILURE', 42, 3), finished(300, 'SUCCESS', 1400, 5)]),
      job('alarm-notify_master', [finished(88, 'UNSTABLE', 300, 9), finished(87, 'SUCCESS', 2000, 8)]),
      job('reports_master', [finished(19, 'ABORTED', 60 * 24 * 3, 2), finished(18, 'SUCCESS', 60 * 24 * 9, 12)]),
      job('a-job-with-a-rather-long-name-for-a-narrow-panel_master', [finished(4, 'SUCCESS', 60 * 24 * 20, 1)], { parameters: [] }),
      job('new-service_master', []),
      job('legacy_master', [finished(912, 'SUCCESS', 60 * 24 * 200, 15)], { disabled: true }),
      folder('tools', [job('rotate-certs', [finished(7, 'SUCCESS', 600, 1)], { parameters: [] })]),
    ]),
    folder('team-prod', [
      job('api_master', [finished(64, 'SUCCESS', 60 * 26, 14)]),
      job('poller_master', [finished(31, 'SUCCESS', 60 * 50, 13)]),
    ]),
    job('nightly-cleanup', [finished(2210, 'SUCCESS', 400, 2)], { parameters: [], kindClass: 'hudson.model.FreeStyleProject' }),
  ]);
}

/**
 * Starts the server. `options`:
 *   port       0 (any free one) by default
 *   clock      () => ms, the server's time (Date.now)
 *   crumbs     false: a Jenkins without CSRF protection (no crumb issuer, none asked)
 *   queueMs    how long a triggered build waits in the queue (1500)
 * Resolves with { url, port, requests, root, find, setCrumb, close }.
 */
export async function startFakeJenkins(options = {}) {
  const clock = options.clock ?? Date.now;
  const crumbs = options.crumbs ?? true;
  const queueMs = options.queueMs ?? 1500;
  const started = clock();
  const root = seed();
  const requests = []; // { method, path, query, user, body, status, crumb }
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

  const handle = (req, res, body) => {
    const url = new URL(req.url, base);
    const entry = { method: req.method, path: decodeURIComponent(url.pathname), query: Object.fromEntries(url.searchParams), user: null, body, status: 0 };
    requests.push(entry);
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
      if (tail.length === 2 && tail[1] === 'stop') {
        const build = node.builds.find((b) => String(b.number) === tail[0]);
        if (!build) return done(404, 'Not found');
        if (build.result === null) {
          build.result = 'ABORTED';
          build.duration = clock() - build.timestamp;
        }
        return done(302, '', { Location: `${base}/${jobPath(names)}${build.number}/` });
      }
      return done(404, 'Not found');
    }

    if (tail.join('/') === 'api/json') return json(itemJson(names, node));
    if (tail.length === 0) return done(200, `The page of ${names.join('/') || 'Jenkins'}`);
    if (node.kind !== 'job') return done(404, 'Not found');
    const build = tail[0] === 'lastBuild' ? node.builds[0] : node.builds.find((b) => String(b.number) === tail[0]);
    if (!build) return done(404, 'Not found');
    const rest2 = tail.slice(1).join('/');
    if (rest2 === '') return done(200, `The page of ${names.join('/')} #${build.number}`);
    if (rest2 === 'api/json') return json(buildJson(names, node, build));
    const bytes = Buffer.from(`${logLines(build, clock() - build.timestamp).join('\n')}\n`, 'utf8');
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
