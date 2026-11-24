// A made-up Jira (Server and Data Center's REST API) for the Jira mod's tests
// and for looking at the mod in a window: a project DEMO with a Scrum board
// (7) whose filter is the project and whose active sprint has 64 issues of
// it, a Kanban board (8) without sprints, and another team's Scrum board (6,
// of project OTHER, whose filter finds DEMO's issues too and which Jira
// lists first for DEMO) sharing that sprint, in which OTHER has two issues;
// a few issues of both projects in no sprint; issues with descriptions in
// wiki markup, comments, subtasks, links, transitions (one whose screen
// requires a field) and work logged, behind the paths the mod asks a real
// Jira for: `/rest/api/2/myself`, `search` (a board's filter and a sprint,
// the token user's open issues, a project's open issues, and the token
// user's issues updated lately with their changelog, and a search of every
// issue's text, `text ~ "<words>"`), `issue/<key>`, its
// `transitions`, `assignee`, `comment` and `worklog`, and the agile API's
// `board`, `board/<id>`, its `configuration`, `project`, `sprint` and
// `sprint/<id>/issue`. Boards 7 and 6 say where they live (`location`),
// board 8 does not, as an older Jira's. Nothing here reaches a real Jira.
//
//   node fake-jira.mjs [port] [backlog]   serves until stopped; prints its address
//
// Two users, by a personal access token as Bearer auth:
//   jane / fake-token      may read, move, assign, comment and log work
//   viewer / viewer-token  may only read: an action gets 403 (an assignment
//                          401, as Jira Server answers one)
// Anything else is 401.
//
// What other people do, for the popups: `assign(key, to, by)`,
// `comment(key, by, body)` and `move(key, status, by)`, also as
// `POST /__fake/assign?key=DEMO-104&to=jane&by=bob`,
// `/__fake/comment?key=DEMO-102&by=alice&body=…` and
// `/__fake/move?key=DEMO-103&to=In%20Review&by=bob` from outside.
//
// It fails on demand: `fail({ mode, status, count, path })` makes the next
// `count` requests (all, by default) whose path matches `path` (a RegExp) be
// dropped without an answer (`refuse`), held until `release()` (`stall`), or
// answered with `status` (`status`); `recover()` ends it. As a program,
// `POST /__fake/fail?mode=…&status=…&count=…&path=…` and `/__fake/recover`.

import http from 'node:http';
import { pathToFileURL } from 'node:url';

const MIN = 60_000, DAY = 86_400_000;

export const USERS = {
  jane: { token: 'fake-token', displayName: 'Jane Doe', write: true },
  viewer: { token: 'viewer-token', displayName: 'Read Only', write: false },
  bob: { displayName: 'Bob Builder' },
  alice: { displayName: 'Alice Example' },
};
const user = (name) => (name ? { self: `/rest/api/2/user?username=${name}`, name, key: name, displayName: USERS[name]?.displayName ?? name, active: true, avatarUrls: {} } : null);

const CAT = {
  new: { id: 2, key: 'new', colorName: 'blue-gray', name: 'To Do' },
  indeterminate: { id: 4, key: 'indeterminate', colorName: 'yellow', name: 'In Progress' },
  done: { id: 3, key: 'done', colorName: 'green', name: 'Done' },
};
export const STATUS = {
  'To Do': { id: '1', name: 'To Do', statusCategory: CAT.new },
  'In Progress': { id: '3', name: 'In Progress', statusCategory: CAT.indeterminate },
  'In Review': { id: '10001', name: 'In Review', statusCategory: CAT.indeterminate },
  Done: { id: '10002', name: 'Done', statusCategory: CAT.done },
};
// Where each status may go: [transition id, name, to, fields its screen asks for].
const RESOLUTION = { resolution: { required: true, hasDefaultValue: false, name: 'Resolution', schema: { type: 'resolution', system: 'resolution' }, allowedValues: [{ id: '1', name: 'Done' }] } };
const FLOW = {
  'To Do': [['11', 'Start Progress', 'In Progress']],
  'In Progress': [['21', 'Send to Review', 'In Review'], ['31', 'Stop Progress', 'To Do']],
  'In Review': [['41', 'Approve', 'Done', RESOLUTION], ['51', 'Back to Progress', 'In Progress'], ['61', 'Close quietly', 'Done']],
  Done: [['71', 'Reopen', 'To Do']],
};
const TYPE = {
  Story: { id: '10001', name: 'Story', subtask: false },
  Bug: { id: '1', name: 'Bug', subtask: false },
  Task: { id: '3', name: 'Task', subtask: false },
  'Sub-task': { id: '5', name: 'Sub-task', subtask: true },
};
const PRIORITY = ['Highest', 'High', 'Medium', 'Low'].map((name, i) => ({ id: String(i + 1), name }));

// `projects`: those its filter (`filter`, a saved filter's id) finds issues of.
export const BOARDS = [
  { id: 6, name: 'OTHER team board', type: 'scrum', projects: ['OTHER', 'DEMO'], filter: 506, location: 'OTHER' },
  { id: 7, name: 'DEMO board', type: 'scrum', projects: ['DEMO'], filter: 507, location: 'DEMO' },
  { id: 8, name: 'DEMO kanban', type: 'kanban', projects: ['DEMO'], filter: 508 },
];
const PROJECTS = { DEMO: { id: '10100', key: 'DEMO', name: 'Demo' }, OTHER: { id: '10200', key: 'OTHER', name: 'Other team' } };
export const SPRINT_ID = 41;

const SUMMARIES = [
  'Show the sprint in the side panel', 'Log in fails after the token expires', 'Make the export run nightly',
  'Wrap long descriptions', 'Count the open alerts per site', 'The report page is slow with many rows',
  'Add a dark theme to the dashboard', 'Retry a read that timed out', 'Rename the "Default" view',
  'Document the settings file', 'Crash when the board has no sprint', 'Move the cache into the data folder',
  'Keep the column widths', 'Sort the list by priority', 'Paste images into comments', 'Empty state for a new project',
];

const DESCRIPTION = `h2. What we want
The panel shows the *active sprint* of a board, with _my issues_ first.

* Read the sprint every minute
* Fold the done ones
** Keep the folds across restarts
# First step
# Second step, see [the board|https://jira.example.com/secure/RapidBoard.jspa?rapidView=7]

{code:javascript}
const sprint = await jira.sprint(7);
console.log(sprint.name);
{code}

{quote}Ask [~bob] before changing the API.{quote}

||Field||Shown||
|Summary|yes|
|Labels|no|

{panel:title=Note}
Nothing here is real (/)
{panel}`;

// The made-up Jira, with its own clock (Date.now, which the tests move).
// `backlog`: so many more open DEMO issues in no sprint (DEMO-1001 and on).
export function makeJira({ backlog = 0 } = {}) {
  const issues = new Map();
  let ids = 10000, histories = 500, comments = 900, worklogs = 300;
  const now = () => Date.now();
  const stamp = (ms) => new Date(ms).toISOString().replace('Z', '+0000');
  const base = now();

  const add = (n, { type = 'Story', status = 'To Do', assignee = null, reporter = 'alice', summary, description = '', parent = null, priority = 2, age = 3, project = 'DEMO', sprint = SPRINT_ID } = {}) => {
    const key = `${project}-${n}`;
    const issue = {
      id: String(ids++), key, sprint,
      fields: {
        summary: summary ?? SUMMARIES[n % SUMMARIES.length], issuetype: TYPE[type], status: STATUS[status],
        assignee: user(assignee), reporter: user(reporter), priority: PRIORITY[priority],
        description: description || null, created: stamp(base - age * DAY), updated: stamp(base - age * DAY / 2),
        project: PROJECTS[project],
        parent: parent ? { id: '', key: parent } : undefined,
      },
      links: [], comments: [], histories: [], worklogs: [],
    };
    issues.set(key, issue);
    return issue;
  };

  // DEMO-101 … DEMO-164: Jane has a few in each state; the rest belong to others or nobody.
  const plan = (n) => {
    const i = n - 101;
    const status = i % 9 === 0 ? 'In Review' : i % 4 === 0 ? 'In Progress' : i % 3 === 0 ? 'Done' : 'To Do';
    const assignee = i % 11 === 0 || [102, 103, 107].includes(n) ? 'jane' : i % 5 === 0 ? null : i % 2 ? 'bob' : 'alice';
    const type = i % 7 === 3 ? 'Bug' : i % 5 === 2 ? 'Task' : 'Story';
    return { status, assignee, type, priority: i % 4 };
  };
  for (let n = 101; n <= 164; n++) {
    if (n === 150 || n === 151) continue;
    add(n, plan(n));
  }
  const main = issues.get('DEMO-101');
  Object.assign(main.fields, { summary: 'Show the active sprint in the side panel', description: DESCRIPTION, status: STATUS['In Progress'], assignee: user('jane'), issuetype: TYPE.Story });
  add(150, { type: 'Sub-task', status: 'Done', assignee: 'jane', parent: 'DEMO-101', summary: 'Read the board\'s sprints' });
  add(151, { type: 'Sub-task', status: 'To Do', assignee: null, parent: 'DEMO-101', summary: 'Draw the status headings' });
  for (const sub of ['DEMO-150', 'DEMO-151']) issues.get('DEMO-101').subtasks = [...(issues.get('DEMO-101').subtasks ?? []), sub];
  main.links.push({ id: '20001', type: 'Blocks', outward: 'DEMO-120' }, { id: '20002', type: 'Blocks', inward: 'DEMO-130' });
  main.comments.push(
    { id: String(comments++), author: user('bob'), body: 'Looks good. Can the *done* group start folded?', created: stamp(base - 2 * DAY) },
    { id: String(comments++), author: user('jane'), body: 'Yes: see the second step.\n{noformat}folded by default{noformat}', created: stamp(base - DAY) },
  );
  main.worklogs.push({ id: String(worklogs++), author: user('jane'), timeSpent: '2h', timeSpentSeconds: 7200, started: stamp(base - DAY), comment: '' });
  // In no sprint: one of nobody's, one of Jane's at work, one of Bob's, one done.
  add(200, { sprint: null, summary: 'An issue in no sprint' });
  add(201, { sprint: null, status: 'In Progress', assignee: 'jane', summary: 'Tidy the backlog', age: 2 });
  add(202, { sprint: null, assignee: 'bob', type: 'Bug', summary: 'The export misses the last row' });
  add(203, { sprint: null, status: 'Done', assignee: 'jane', summary: 'Done long ago, in no sprint' });
  // Another team's, two of them in the same sprint, which board 7 does not show.
  add(1, { project: 'OTHER', assignee: 'jane', summary: 'Help the other team with their import', age: 1 });
  add(2, { project: 'OTHER', assignee: 'bob', status: 'In Progress', summary: 'The other team\'s own work' });
  add(3, { project: 'OTHER', sprint: null, assignee: 'jane', status: 'In Review', summary: 'Review the other team\'s plan', age: 4 });
  for (let n = 1001; n <= 1000 + backlog; n++) add(n, { sprint: null, assignee: n % 3 ? 'bob' : null, summary: `Backlog item ${n}` });

  const LINK = { Blocks: { id: '10000', name: 'Blocks', inward: 'is blocked by', outward: 'blocks' } };
  const brief = (key) => {
    const i = issues.get(key);
    return i ? { id: i.id, key, fields: { summary: i.fields.summary, status: i.fields.status, priority: i.fields.priority, issuetype: i.fields.issuetype } } : { key };
  };
  const seconds = (text) => {
    const units = { w: 5 * 8 * 3600, d: 8 * 3600, h: 3600, m: 60 };
    let total = 0;
    String(text).replace(/(\d+(?:\.\d+)?)\s*([wdhm])/g, (_, n, u) => { total += Number(n) * units[u]; return ''; });
    return total;
  };
  const span = (s) => {
    const h = Math.floor(s / 3600), m = Math.round((s % 3600) / 60);
    return [h ? `${h}h` : '', m ? `${m}m` : ''].filter(Boolean).join(' ') || '0m';
  };

  // An issue's fields as Jira gives them, those asked for (all for none).
  const fieldsOf = (i, wanted) => {
    const spent = i.worklogs.reduce((s, w) => s + w.timeSpentSeconds, 0);
    const all = {
      ...i.fields,
      subtasks: (i.subtasks ?? []).map(brief),
      issuelinks: i.links.map((l) => ({ id: l.id, type: LINK[l.type], ...(l.outward ? { outwardIssue: brief(l.outward) } : { inwardIssue: brief(l.inward) }) })),
      comment: { comments: i.comments.map((c) => ({ ...c, updated: c.created, updateAuthor: c.author })), maxResults: i.comments.length, total: i.comments.length, startAt: 0 },
      timetracking: spent ? { timeSpent: span(spent), timeSpentSeconds: spent, remainingEstimate: '4h', remainingEstimateSeconds: 14400 } : {},
      parent: i.fields.parent ? brief(i.fields.parent.key) : undefined,
    };
    if (!wanted || wanted === '*all') return all;
    const out = {};
    for (const name of wanted.split(',')) if (name in all && all[name] !== undefined) out[name] = all[name];
    return out;
  };
  const shape = (i, fields, expand) => ({
    id: i.id, key: i.key, self: `/rest/api/2/issue/${i.id}`, fields: fieldsOf(i, fields),
    ...(expand?.includes('changelog') ? { changelog: { startAt: 0, maxResults: i.histories.length, total: i.histories.length, histories: i.histories } } : {}),
  });
  const touch = (i) => { i.fields.updated = stamp(now()); };
  const history = (i, by, items) => {
    i.histories.push({ id: String(histories++), author: user(by), created: stamp(now()), items });
    touch(i);
  };

  // A board as the agile API gives it: where it lives only when it says.
  const boardOf = (b) => ({ id: b.id, self: '', name: b.name, type: b.type, ...(b.location ? { location: { projectId: Number(PROJECTS[b.location].id), projectKey: b.location, projectTypeKey: 'software', name: `${PROJECTS[b.location].name} (${b.location})` } } : {}) });
  // What a board's saved filter finds: the issues of its projects, in their rank's order.
  const inFilter = (id) => {
    const b = BOARDS.find((x) => x.filter === id);
    return [...issues.values()].filter((i) => b?.projects.includes(i.fields.project.key));
  };
  // The words of an issue a text search finds it by: its summary's,
  // description's and comments', lower-cased.
  const wordsOf = (i) => new Set(tokens([i.fields.summary, i.fields.description, ...i.comments.map((c) => c.body)].join(' ')));
  // A page of issues, 50 at most, as a search and the agile API give one.
  const paged = (found, q) => {
    const startAt = Number(q.get('startAt') ?? 0), max = Math.min(Number(q.get('maxResults') ?? 50), 50);
    return { expand: 'schema,names', startAt, maxResults: max, total: found.length, issues: found.slice(startAt, startAt + max).map((i) => shape(i, q.get('fields'), q.get('expand'))) };
  };

  return {
    issues,
    stamp,
    // Someone else's doings, for the popups.
    assign(key, to, by = 'bob') {
      const i = issues.get(key), from = i.fields.assignee;
      i.fields.assignee = user(to);
      history(i, by, [{ field: 'assignee', fieldtype: 'jira', from: from?.key ?? null, fromString: from?.displayName ?? null, to: to ?? null, toString: user(to)?.displayName ?? null }]);
    },
    move(key, to, by = 'bob') {
      const i = issues.get(key), from = i.fields.status;
      i.fields.status = STATUS[to];
      history(i, by, [{ field: 'status', fieldtype: 'jira', from: from.id, fromString: from.name, to: STATUS[to].id, toString: to }]);
    },
    comment(key, by, body) {
      const i = issues.get(key);
      i.comments.push({ id: String(comments++), author: user(by), body, created: stamp(now()) });
      touch(i);
    },
    // The API, for a request already authenticated as `who` (a name of USERS).
    handle(method, url, body, who) {
      const write = USERS[who]?.write;
      const p = url.pathname, q = url.searchParams;
      let m;
      const json = (status, data) => ({ status, data });
      const refuse = (status, message) => json(status, { errorMessages: [message], errors: {} });
      if (method === 'GET' && p === '/rest/api/2/myself') return json(200, { ...user(who), emailAddress: `${who}@example.com`, timeZone: 'Europe/Stockholm', locale: 'en_US', groups: { size: 1, items: [] }, expand: 'groups' });
      if (method === 'GET' && p === '/rest/api/2/serverInfo') return json(200, { version: '11.3.0', deploymentType: 'Server' });
      if (method === 'GET' && (m = /^\/rest\/agile\/1\.0\/board\/(\d+)$/.exec(p))) {
        const b = BOARDS.find((x) => x.id === Number(m[1]));
        return b ? json(200, boardOf(b)) : refuse(404, `Board does not exist or you do not have permission to see it.`);
      }
      if (method === 'GET' && (m = /^\/rest\/agile\/1\.0\/board\/(\d+)\/(configuration|project)$/.exec(p))) {
        const b = BOARDS.find((x) => x.id === Number(m[1]));
        if (!b) return refuse(404, 'Board does not exist or you do not have permission to see it.');
        if (m[2] === 'configuration') return json(200, { id: b.id, name: b.name, type: b.type, self: '', filter: { id: String(b.filter), self: '' }, columnConfig: { columns: [] }, ranking: { rankCustomFieldId: 10005 } });
        return json(200, { maxResults: 50, startAt: 0, total: b.projects.length, isLast: true, values: b.projects.map((key) => ({ self: '', ...PROJECTS[key], projectTypeKey: 'software' })) });
      }
      if (method === 'GET' && (m = /^\/rest\/agile\/1\.0\/board\/(\d+)\/sprint\/(\d+)\/issue$/.exec(p))) {
        const b = BOARDS.find((x) => x.id === Number(m[1]));
        if (!b) return refuse(404, 'Board does not exist or you do not have permission to see it.');
        return json(200, paged(inFilter(b.filter).filter((i) => i.sprint === Number(m[2])), q));
      }
      if (method === 'GET' && p === '/rest/agile/1.0/board') {
        const project = q.get('projectKeyOrId');
        const found = BOARDS.filter((b) => !project || b.projects.includes(project.toUpperCase()));
        return json(200, { maxResults: 50, startAt: 0, total: found.length, isLast: true, values: found.map(boardOf) });
      }
      if (method === 'GET' && (m = /^\/rest\/agile\/1\.0\/board\/(\d+)\/sprint$/.exec(p))) {
        const b = BOARDS.find((x) => x.id === Number(m[1]));
        if (!b) return refuse(404, 'Board does not exist or you do not have permission to see it.');
        if (b.type !== 'scrum') return refuse(400, 'The board does not support sprints');
        return json(200, { maxResults: 50, startAt: 0, isLast: true, values: [{ id: SPRINT_ID, self: '', state: 'active', name: 'DEMO Sprint 12', startDate: stamp(base - 6 * DAY), endDate: stamp(base + 4 * DAY + 3_600_000), originBoardId: 7, goal: 'The sprint in the side panel' }] });
      }
      if (method === 'GET' && p === '/rest/api/2/search') {
        const jql = q.get('jql') ?? '';
        const open = (i) => i.fields.status.statusCategory.key !== 'done';
        let found;
        if ((m = /^sprint = (\d+) ORDER BY Rank ASC$/.exec(jql))) found = [...issues.values()].filter((i) => i.sprint === Number(m[1]));
        else if ((m = /^filter = (\d+) AND sprint = (\d+) ORDER BY Rank ASC$/.exec(jql))) {
          if (!BOARDS.some((b) => b.filter === Number(m[1]))) return refuse(400, `A value with ID '${m[1]}' does not exist for the field 'filter'.`);
          found = inFilter(Number(m[1])).filter((i) => i.sprint === Number(m[2]));
        } else if (jql === 'assignee = currentUser() AND statusCategory != Done ORDER BY updated DESC') {
          found = [...issues.values()].filter((i) => i.fields.assignee?.name === who && open(i))
            .sort((a, b) => Date.parse(b.fields.updated) - Date.parse(a.fields.updated));
        } else if ((m = /^project in \(("[A-Z]+"(?:, "[A-Z]+")*)\) AND statusCategory != Done ORDER BY Rank ASC$/.exec(jql))) {
          const keys = m[1].split(', ').map((k) => k.slice(1, -1));
          found = [...issues.values()].filter((i) => keys.includes(i.fields.project.key) && open(i));
        } else if ((m = /^assignee = currentUser\(\) AND updated >= -(\d+)m ORDER BY updated DESC$/.exec(jql))) {
          const since = now() - Number(m[1]) * MIN;
          found = [...issues.values()].filter((i) => i.fields.assignee?.name === who && Date.parse(i.fields.updated) >= since)
            .sort((a, b) => Date.parse(b.fields.updated) - Date.parse(a.fields.updated));
        } else if ((m = /^text ~ "((?:[^"\\]|\\.)*)" ORDER BY updated DESC$/.exec(jql))) {
          // As strict as Jira or stricter: a JQL escape other than \\ and \" is an
          // error, and so is a Lucene character or operator left unescaped.
          const text = unquote(m[1]);
          if (text === null) return refuse(400, `Error in the JQL Query: the string '${m[1]}' has an illegal escape.`);
          const terms = luceneTerms(text);
          if (terms === null) return refuse(400, `The text query '${text}' for field 'text' is not valid: probably your use of wild cards is not valid.`);
          found = [...issues.values()].filter((i) => { const has = wordsOf(i); return terms.length > 0 && terms.every((t) => has.has(t)); })
            .sort((a, b) => Date.parse(b.fields.updated) - Date.parse(a.fields.updated));
        } else return refuse(400, `The fake does not know this JQL: ${jql}`);
        return json(200, paged(found, q));
      }
      if ((m = /^\/rest\/api\/2\/issue\/([A-Z]+-\d+)(\/[a-z]+)?$/.exec(p))) {
        const i = issues.get(m[1]), what = m[2] ?? '';
        if (!i) return refuse(404, 'Issue Does Not Exist');
        if (method === 'GET' && !what) return json(200, shape(i, q.get('fields'), q.get('expand')));
        if (method === 'GET' && what === '/transitions') {
          return json(200, { expand: 'transitions', transitions: (FLOW[i.fields.status.name] ?? []).map(([id, name, to, fields]) => ({ id, name, to: STATUS[to], ...(q.get('expand')?.includes('transitions.fields') ? { fields: fields ?? {} } : {}) })) });
        }
        if (method === 'POST' && what === '/transitions') {
          if (!write) return refuse(403, 'You do not have permission to transition this issue.');
          const step = (FLOW[i.fields.status.name] ?? []).find(([id]) => id === String(body?.transition?.id));
          if (!step) return refuse(400, `Transition id '${body?.transition?.id}' is not valid for this issue.`);
          if (step[3] && !body?.fields?.resolution) return json(400, { errorMessages: [], errors: { resolution: 'Resolution is required.' } });
          const from = i.fields.status;
          i.fields.status = STATUS[step[2]];
          history(i, who, [{ field: 'status', fieldtype: 'jira', from: from.id, fromString: from.name, to: STATUS[step[2]].id, toString: step[2] }]);
          return json(204, null);
        }
        if (method === 'PUT' && what === '/assignee') {
          if (!write) return refuse(401, 'You do not have permission to assign issues.');
          if (!USERS[body?.name]) return json(400, { errorMessages: [], errors: { assignee: `User '${body?.name}' does not exist.` } });
          const from = i.fields.assignee;
          i.fields.assignee = user(body.name);
          history(i, who, [{ field: 'assignee', fieldtype: 'jira', from: from?.key ?? null, fromString: from?.displayName ?? null, to: body.name, toString: user(body.name).displayName }]);
          return json(204, null);
        }
        if (method === 'POST' && what === '/comment') {
          if (!write) return refuse(403, 'You do not have the permission to comment on this issue.');
          if (!String(body?.body ?? '').trim()) return json(400, { errorMessages: [], errors: { comment: 'Comment body can not be empty!' } });
          const c = { id: String(comments++), author: user(who), body: body.body, created: stamp(now()) };
          i.comments.push(c);
          touch(i);
          return json(201, { ...c, updated: c.created });
        }
        if (method === 'POST' && what === '/worklog') {
          if (!write) return refuse(403, 'You do not have the permission to associate a worklog to this issue.');
          const s = seconds(body?.timeSpent);
          if (!s) return json(400, { errorMessages: [], errors: { timeLogged: 'Invalid time duration entered.' } });
          if (!/^\d{4}-\d\d-\d\dT\d\d:\d\d:\d\d\.\d{3}[+-]\d{4}$/.test(body?.started ?? '')) return json(400, { errorMessages: [], errors: { started: 'Invalid date format.' } });
          const w = { id: String(worklogs++), author: user(who), timeSpent: body.timeSpent, timeSpentSeconds: s, started: body.started, comment: body.comment ?? '' };
          i.worklogs.push(w);
          touch(i);
          return json(201, w);
        }
      }
      return refuse(404, 'Not found');
    },
  };
}

// A text's words as Jira's index keeps them: lower-cased, split at all but
// letters and digits, without the commonest English words.
const STOP = new Set(['a', 'an', 'and', 'are', 'as', 'at', 'be', 'but', 'by', 'for', 'if', 'in', 'into', 'is', 'it', 'no', 'not', 'of', 'on', 'or', 'such', 'that', 'the', 'their', 'then', 'there', 'these', 'they', 'this', 'to', 'was', 'will', 'with']);
const tokens = (text) => String(text ?? '').toLowerCase().split(/[^\p{L}\p{N}]+/u).filter((w) => w && !STOP.has(w));

// A JQL string's text: \\ and \" are its escapes (JQL has a few more, which the mod never writes); null for another.
function unquote(text) {
  let out = '';
  for (let i = 0; i < text.length; i++) {
    if (text[i] !== '\\') { out += text[i]; continue; }
    const next = text[++i];
    if (next !== '\\' && next !== '"') return null;
    out += next;
  }
  return out;
}

// A Lucene query's words, or null when a character with a meaning in Lucene
// (or an operator, AND, OR, NOT) is in it unescaped: the fake takes only
// plain words, each matched whole.
function luceneTerms(query) {
  const words = [];
  let word = '';
  for (let i = 0; i < query.length; i++) {
    const c = query[i];
    if (c === '\\') {
      if (i + 1 >= query.length) return null;
      word += query[++i];
    } else if (/\s/.test(c)) { words.push(word); word = ''; }
    else if ('+-&|!(){}[]^"~*?:/'.includes(c)) return null;
    else word += c;
  }
  words.push(word);
  if (words.some((w) => /^(AND|OR|NOT)$/.test(w))) return null;
  return tokens(words.join(' '));
}

// The made-up Jira on a local port: `{ url, jira, requests, fail, recover, release, close }`.
export async function startFakeJira(port = 0, options = {}) {
  const jira = makeJira(options);
  const requests = [];
  let failing = null; // { mode, status, count, path }
  const held = [];
  const server = http.createServer((req, res) => {
    let raw = '';
    req.on('data', (chunk) => { raw += chunk; });
    req.on('end', () => {
      const url = new URL(req.url, 'http://fake');
      if (url.pathname.startsWith('/__fake/')) return control(url, res);
      const auth = req.headers.authorization ?? '';
      requests.push({ method: req.method, path: url.pathname, query: url.search, auth: auth.replace(/\S+$/, '…'), body: raw });
      if (failing && (!failing.path || failing.path.test(url.pathname)) && failing.count !== 0) {
        if (failing.count > 0) failing.count -= 1;
        if (failing.mode === 'refuse') { req.socket.destroy(); return; }
        if (failing.mode === 'stall') { held.push(res); return; }
        res.writeHead(failing.status ?? 503, { 'Content-Type': 'text/html' });
        res.end('<html><body>Service Unavailable</body></html>');
        return;
      }
      const who = Object.entries(USERS).find(([, u]) => u.token && auth === `Bearer ${u.token}`)?.[0];
      if (!who) {
        res.writeHead(401, { 'Content-Type': 'application/json' });
        res.end(JSON.stringify({ errorMessages: ['You are not authenticated. Authentication required to perform this operation.'], errors: {} }));
        return;
      }
      let body = null;
      try { body = raw ? JSON.parse(raw) : null; } catch { body = null; }
      if (['POST', 'PUT'].includes(req.method) && raw && req.headers['x-atlassian-token'] !== 'no-check') {
        res.writeHead(403, { 'Content-Type': 'text/plain' });
        res.end('XSRF check failed');
        return;
      }
      const { status, data } = jira.handle(req.method, url, body, who);
      res.writeHead(status, data === null ? {} : { 'Content-Type': 'application/json' });
      res.end(data === null ? '' : JSON.stringify(data));
    });
  });
  const fake = {
    jira, requests,
    fail({ mode = 'status', status = 503, count = -1, path = null } = {}) { failing = { mode, status, count, path: path ? new RegExp(path) : null }; },
    recover() { failing = null; },
    release() { for (const res of held.splice(0)) { res.writeHead(503); res.end(); } },
    close() { fake.release(); server.closeAllConnections?.(); return new Promise((resolve) => server.close(resolve)); },
  };
  function control(url, res) {
    const q = Object.fromEntries(url.searchParams);
    const ok = () => { res.writeHead(200, { 'Content-Type': 'text/plain' }); res.end('ok\n'); };
    try {
      if (url.pathname === '/__fake/fail') { fake.fail({ ...q, status: Number(q.status ?? 503), count: Number(q.count ?? -1) }); return ok(); }
      if (url.pathname === '/__fake/recover') { fake.recover(); fake.release(); return ok(); }
      if (url.pathname === '/__fake/assign') { jira.assign(q.key, q.to ?? 'jane', q.by ?? 'bob'); return ok(); }
      if (url.pathname === '/__fake/move') { jira.move(q.key, q.to, q.by ?? 'bob'); return ok(); }
      if (url.pathname === '/__fake/comment') { jira.comment(q.key, q.by ?? 'alice', q.body ?? 'A comment.'); return ok(); }
    } catch (e) {
      res.writeHead(400, { 'Content-Type': 'text/plain' });
      res.end(`${e.message}\n`);
      return undefined;
    }
    res.writeHead(404);
    res.end();
    return undefined;
  }
  await new Promise((resolve) => server.listen(port, '127.0.0.1', resolve));
  fake.url = `http://127.0.0.1:${server.address().port}`;
  return fake;
}

if (import.meta.url === pathToFileURL(process.argv[1] ?? '').href) {
  const fake = await startFakeJira(Number(process.argv[2] ?? 0), { backlog: Number(process.argv[3] ?? 0) });
  console.log(`fake Jira at ${fake.url} (tokens: fake-token, viewer-token)`);
}
