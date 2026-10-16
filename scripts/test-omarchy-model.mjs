import assert from 'node:assert/strict';
import fs from 'node:fs';
import vm from 'node:vm';

// Exercise the same JavaScript loaded by QML, without a duplicate implementation.
const model = vm.createContext({});
vm.runInContext(fs.readFileSync(new URL('../omarchy/Model.js', import.meta.url), 'utf8'), model);
const agent = (pane_id, project_group, extra = {}) => ({pane_id, project_group, agent_status: 'idle', ...extra});
const snapshot = {agents: [
  agent('lead', 'App', {supervisor: true, scratch: false}),
  agent('worker', 'App', {agent_status: 'blocked'}),
  agent('finished', 'App', {agent_status: 'done'}),
  agent('other-lead', 'Other', {supervisor: true}),
  agent('other-worker', 'Other'),
  agent('solo', 'Solo'),
  agent('scratch', 'Scratch', {supervisor: true, scratch: true}),
  agent('scratch-worker', 'Scratch'),
  agent('named-supervisor', 'Legacy', {name: 'Supervisor'}),
  agent('legacy-worker', 'Legacy'),
]};
const panes = (rows) => Array.from(rows.filter(row => !row.heading), row => row.pane_id);
assert.deepEqual(panes(model.dashboardRows(snapshot)), ['lead', 'other-lead', 'solo', 'scratch', 'scratch-worker', 'named-supervisor', 'legacy-worker']);
const heading = model.dashboardRows(snapshot)[0];
assert.equal(heading.workerSummary, '2 workers · ▲ 1 · ✓ 1');
assert.equal(heading.workerCounts.blocked, 1);
let state = model.activateAgent(snapshot, [], '', 'lead');
assert.equal(state.pane, '', 'first click must only reveal project');
assert.deepEqual(Array.from(state.projects), ['App']);
assert.deepEqual(panes(model.dashboardRows(snapshot, '', state.projects)).slice(0, 3), ['lead', 'worker', 'finished']);
state = model.activateAgent(snapshot, state.projects, state.pane, 'worker');
assert.equal(state.pane, 'worker', 'exposed worker opens transcript');
assert.deepEqual(panes(model.dashboardRows(snapshot, state.pane, state.projects)), ['worker']);
state = model.activateAgent(snapshot, state.projects, state.pane, 'worker');
assert.equal(state.pane, '', 'second worker click leaves transcript');
state = model.activateAgent(snapshot, state.projects, '', 'lead');
assert.equal(state.pane, 'lead', 'exposed supervisor also opens transcript');
assert.equal(model.activateAgent(snapshot, [], '', 'solo').pane, 'solo');
assert.equal(model.activateAgent(snapshot, [], '', 'scratch').pane, 'scratch');
state = model.activateAgent(snapshot, ['App'], '', 'other-lead');
assert.deepEqual(Array.from(state.projects), ['App', 'Other'], 'multiple projects retain independent expansion');
const refreshed = {agents: snapshot.agents.filter(agent => !['worker', 'other-lead', 'other-worker'].includes(agent.pane_id))};
state = model.reconcileNavigation(refreshed, ['App', 'Other'], 'worker', 'worker');
assert.equal(state.pane, '', 'removed transcript cannot remain selected');
assert.equal(state.selected, 'lead');
assert.deepEqual(Array.from(state.projects), ['App'], 'removed projects must lose expansion');
state = model.reconcileNavigation(snapshot, [], '', 'worker');
assert.equal(state.selected, 'lead', 'hidden workers cannot remain keyboard action targets');
state = model.backNavigation(snapshot, ['App', 'Other'], 'worker', 'worker');
assert.equal(state.pane, '', 'back first leaves transcript');
assert.deepEqual(Array.from(state.projects), ['App', 'Other']);
state = model.backNavigation(snapshot, state.projects, state.pane, state.selected);
assert.deepEqual(Array.from(state.projects), ['Other'], 'back then collapses selected project');
assert.equal(state.selected, 'lead', 'collapsing worker selection selects actual supervisor');
state = model.backNavigation(snapshot, state.projects, state.pane, state.selected);
assert.deepEqual(Array.from(state.projects), [], 'back can collapse remaining expanded project');
assert.equal(model.backNavigation(snapshot, state.projects, state.pane, state.selected), null, 'final back closes popup');
state = model.toggleProject(snapshot, ['App'], 'App', 'lead');
assert.deepEqual(Array.from(state.projects), []);
assert.equal(state.selected, 'lead');
assert.equal(state.pane, '');
assert.deepEqual(panes(model.dashboardRows({agents: []})), []);
const closedRows = model.dashboardRows(snapshot);
assert.equal(closedRows.find(row => row.pane_id === 'lead').collapsedProject, true);
assert.equal(model.dashboardRows(snapshot, '', ['App']).find(row => row.pane_id === 'lead').collapsedProject, false);
assert.equal(model.dashboardRows(snapshot, 'lead', ['App']).find(row => row.pane_id === 'lead').collapsedProject, false);
assert.equal(closedRows.find(row => row.pane_id === 'scratch').collapsedProject, false);
assert.equal(closedRows.find(row => row.pane_id === 'solo').collapsedProject, false);
assert.equal(snapshot.agents[0].collapsedProject, undefined, 'row decoration must not mutate snapshot');
const fallback = model.herdLines(model.summarize(snapshot));
assert.deepEqual(Array.from(fallback, row => row.spans[2].text.trim()), ['WORKING', 'BLOCKED', 'MERGE', 'DONE', 'IDLE']);

const states = {agents: [agent('lead', 'All', {supervisor: true}),
  agent('working', 'All', {agent_status: 'working'}),
  agent('blocked', 'All', {agent_status: 'blocked'}),
  agent('merge', 'All', {agent_status: 'done', ready_to_merge: true}),
  agent('done', 'All', {agent_status: 'done'}), agent('idle', 'All'),
  agent('unknown', 'All', {agent_status: 'unknown'})]};
const segments = model.dashboardRows(states)[0].workerSegments;
assert.deepEqual(Array.from(segments, part => part.text), ['6 workers', '▲ 1', '⇡ 1', '● 1', '✓ 1', '○ 1', '? 1']);
assert.deepEqual(Array.from(segments, part => part.tone), ['foreground', 'yellow', 'magenta', 'cyan', 'green', 'muted', 'magenta']);
assert.equal(model.dashboardRows(states)[0].workerCounts.done, 1, 'merge workers are not counted twice');
assert.deepEqual(Array.from(model.dashboardRows(states, '', ['All'])[0].workerSegments, part => part.text), Array.from(segments, part => part.text));
assert.equal(model.workerSummarySegments([states.agents[0]])[0].text, 'no workers');

assert.deepEqual(Array.from(segments.slice(1), part => part.label), ['1 blocked', '1 merge', '1 working', '1 done', '1 idle', '1 unknown']);
console.log('Omarchy model grouping, navigation, and worker summary checks passed');
