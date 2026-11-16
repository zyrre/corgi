// The popups (toasts): what makes one, and what it says. A listed job's
// newest build that ends (success, failure, unstable, aborted) or starts to
// wait at an input step posts one; a job or a section (a folder of the
// settings) can be muted. Pure, apart from what is handed in: the store's
// `Watch` decides, `toast` words it, and main.js posts it with the app's
// `notify`.
//
// What was already so when it was first seen makes none: the first read
// after the app starts, or after the settings change, is the baseline, and
// so is a job seen for the first time. A build that starts makes none.

import { childOf, isFolder, leaf, span, envOf } from "./util.js";

// The job paths and the section (folder) paths muted, kept by the app across restarts.
export const MUTED = "jenkins.muted";
export const MUTED_SECTIONS = "jenkins.mutedSections";

const RESULT = { SUCCESS: "success", FAILURE: "failed", UNSTABLE: "unstable", ABORTED: "aborted" };
// More than this many in one read make one toast for all of them.
export const AT_ONCE = 3;

// The stages that wait for input in a build's `wfapi/describe`: their ids
// and names. A build that waits outside any stage counts as one named "".
export function waitsIn(describe) {
  const stages = (describe?.stages ?? []).filter((s) => s?.status === "PAUSED_PENDING_INPUT").map((s) => ({ id: String(s.id ?? s.name), name: s.name ?? "" }));
  if (!stages.length && describe?.status === "PAUSED_PENDING_INPUT") return [{ id: "build", name: "" }];
  return stages;
}

// The stage a build failed at, by its stages as read last; "" when none says so.
export const failedStage = (describe) => (describe?.stages ?? []).find((s) => s?.status === "FAILED")?.name ?? "";

// What the toasts know of each listed job: its newest build when last seen.
// One for every copy of the mod (data.js keeps it), so an event is seen once.
export class Watch {
  constructor() {
    // job path → { section, job, number, building, waits: Set of stage ids already seen waiting, quiet }.
    // `quiet`: the build ran when first seen, and its stages have not been read
    // since, so a wait they show was already so.
    this.jobs = new Map();
  }

  reset() { this.jobs.clear(); }

  // A list of a section was read: the events of its jobs' newest builds.
  // [{ kind: "ended", section, job, number, state, duration }]
  list(section, items) {
    const events = [];
    for (const item of items ?? []) {
      if (!item?.name || isFolder(item)) continue;
      const job = childOf(section, item.name), b = item.lastBuild, prev = this.jobs.get(job.path);
      const number = b?.number ?? 0, building = Boolean(b?.building);
      const fresh = { section, job, number, building, waits: new Set(), quiet: false };
      if (!prev) { this.jobs.set(job.path, { ...fresh, quiet: building }); continue; } // first seen: what is, is
      prev.section = section;
      if (!b || number < prev.number) { Object.assign(prev, fresh); continue; } // its builds were deleted: nothing new
      if (building) {
        if (number !== prev.number) Object.assign(prev, fresh); // a build started
        else prev.building = true;
        continue;
      }
      if (number === prev.number && !prev.building) continue; // ended before, and seen so
      Object.assign(prev, { ...fresh, waits: prev.number === number ? prev.waits : new Set() });
      const state = RESULT[b.result];
      if (state) events.push({ kind: "ended", section, job, number, state, duration: b.duration ?? 0 });
    }
    return events;
  }

  // A build's stages were read: an event for each input step it has started
  // to wait at since. [{ kind: "waits", section, job, number, stage }]
  stages(path, number, describe) {
    const prev = this.jobs.get(path);
    if (!prev || !describe || describe.missing || number < prev.number) return [];
    if (!["IN_PROGRESS", "PAUSED_PENDING_INPUT", "QUEUED"].includes(describe.status)) return []; // ended: the list says how
    if (number > prev.number) Object.assign(prev, { number, building: true, waits: new Set(), quiet: false }); // started since the list was read
    const events = [];
    for (const wait of waitsIn(describe)) {
      if (prev.waits.has(wait.id)) continue;
      prev.waits.add(wait.id);
      if (!prev.quiet) events.push({ kind: "waits", section: prev.section, job: prev.job, number, stage: wait.name });
    }
    prev.quiet = false;
    return events;
  }
}

// Whether an event's job is muted: its own setting, or its section's.
export const muted = (event, jobs, sections) => jobs.has(event.job.path) || sections.has(event.section.path);

// A section as a toast names it: its name, and DEV or PROD when its name says so.
export function sectionName(section, host = "") {
  const env = envOf(section.label);
  return [section.label || host || "Jenkins", env ? env.toUpperCase() : ""].filter(Boolean).join(" · ");
}

const OUTCOME = { success: "succeeded", failed: "failed", unstable: "is unstable", aborted: "was aborted", waits: "waits for input" };
// The app's levels: what failed is an error, what needs a look a warning.
const LEVEL = { success: "info", failed: "error", unstable: "warning", aborted: "info", waits: "warning" };

// A toast's title, body and level. `stages`: the build's stages as last read, if any.
export function toast(event, { stages = null, host = "" } = {}) {
  const what = event.kind === "waits" ? "waits" : event.state;
  const failedAt = what === "failed" ? failedStage(stages) : "";
  const detail = what === "waits" ? (event.stage ? `at ${event.stage}` : "")
    : failedAt ? `failed at ${failedAt}` : event.duration > 0 ? `took ${span(event.duration)}` : "";
  return {
    title: `${leaf(event.job.label)} #${event.number} ${OUTCOME[what]}`,
    body: [sectionName(event.section, host), detail].filter(Boolean).join(" · "),
    level: LEVEL[what],
  };
}

// One toast for several events of one read.
export function summary(events, host = "") {
  const failed = events.filter((e) => e.state === "failed").length;
  return {
    title: `${events.length} Jenkins builds need a look`,
    body: events.map((e) => `${leaf(e.job.label)} #${e.number} ${OUTCOME[e.kind === "waits" ? "waits" : e.state]}`).join(", ").slice(0, 160) || sectionName(events[0]?.section ?? {}, host),
    level: failed ? "error" : events.some((e) => e.kind === "waits" || e.state === "unstable") ? "warning" : "info",
  };
}

// --- the panel in the layout --------------------------------------------------------

function walk(node, path, visit) {
  if (!node || typeof node !== "object") return;
  visit(node, path);
  for (const child of [...(Array.isArray(node.children) ? node.children : []), ...(Array.isArray(node.floating) ? node.floating : [])]) {
    walk(child, [...path, node], visit);
  }
}

// The block of the right-hand panel showing this mod (`props.panel`), with its ancestors, or null.
export function panelBlock(layout) {
  let found = null;
  walk(layout, [], (node, path) => {
    const c = node.content;
    if (!found && c && c.mod === "jenkins" && c.props?.panel === true) found = { node, path };
  });
  return found;
}

// Whether the panel is on screen: no ancestor hidden, and each stack on the
// way shows the child it is in (a stack with no `active` shows its first).
export function panelShown(layout) {
  const found = panelBlock(layout);
  if (!found || found.node.hidden) return false;
  const chain = [...found.path, found.node];
  for (let i = 0; i < chain.length - 1; i++) {
    const node = chain[i], child = chain[i + 1];
    if (node.hidden) return false;
    if (node.type === "stack") {
      const active = node.active ?? node.children?.find((c) => c?.id)?.id;
      if (active !== child.id) return false;
    }
  }
  return true;
}

// The commands that show the panel: select it in its stack, then show every
// hidden block above it (Herdr's `panel` column). [] when there is no panel.
export function showCommands(layout) {
  const found = panelBlock(layout);
  if (!found) return [];
  const chain = [...found.path, found.node];
  const out = [];
  for (let i = 0; i < chain.length - 1; i++) {
    if (chain[i].type === "stack" && chain[i].id && chain[i + 1].id) out.push(["layout.select", { stack: chain[i].id, child: chain[i + 1].id }]);
  }
  for (const node of chain) if (node.hidden && node.id) out.push(["layout.show", { id: node.id }]);
  return out;
}
