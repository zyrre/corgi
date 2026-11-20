// Pure helpers: addresses, the board setting, issue keys, statuses, time,
// work logged, errors. No Herdr, GPUI or network calls here, so they run
// anywhere.

// --- addresses -------------------------------------------------------------------

// The base URL as typed: a scheme added when it has none, no slash at its end.
export function normalizeBase(text) {
  let url = String(text ?? "").trim();
  if (!url) return "";
  if (!/^[a-z][a-z0-9+.-]*:\/\//i.test(url)) url = `https://${url}`;
  return url.replace(/\/+$/, "");
}

export const hostOf = (base) => /^[a-z][a-z0-9+.-]*:\/\/([^/]+)/i.exec(base ?? "")?.[1] ?? "";

// What the settings' "Board" names: a board by its id (`42`, or a board's URL
// with `rapidView=42` or `/boards/42`), or a project by its key (`DEMO`), whose
// board is looked up. null when it is neither.
export function parseBoard(text) {
  const s = String(text ?? "").trim();
  if (!s) return null;
  if (/^\d+$/.test(s)) return { id: Number(s) };
  const url = /[?&]rapidView=(\d+)/.exec(s) ?? /\/boards\/(\d+)/.exec(s);
  if (url) return { id: Number(url[1]) };
  if (/^[A-Za-z][A-Za-z0-9_]*$/.test(s)) return { project: s.toUpperCase() };
  return null;
}

// A project's key, as it stands in JQL.
export const PROJECT = /^[A-Z][A-Z0-9_]*$/;

// A project's board, of the boards Jira lists for it (every board whose
// filter finds an issue of the project, other teams' among them): a Scrum
// board (the one with sprints) before another, and of those the one that
// lives in the project (`location.projectKey`, which newer Jiras give) before
// one that says nothing, before one that lives elsewhere; else the first.
export function pickBoard(boards, project) {
  const score = (b) => {
    const at = b?.location?.projectKey;
    return (b?.type === "scrum" ? 3 : 0) + (at ? (String(at).toUpperCase() === project ? 1 : -1) : 0);
  };
  let best = null;
  for (const b of boards ?? []) if (b && (!best || score(b) > score(best))) best = b;
  return best;
}

// --- issue keys ------------------------------------------------------------------

export const KEY = /^[A-Z][A-Z0-9_]*-\d+$/;

// The issue a command names: `open-DEMO-12` (or `open-demo-12`; a command name
// may arrive lowercased) is DEMO-12. null for anything else.
export function keyOf(command) {
  const m = /^open-([A-Za-z][A-Za-z0-9_]*-\d+)$/.exec(String(command ?? ""));
  return m ? m[1].toUpperCase() : null;
}

// The project part of a key: DEMO-12 is DEMO.
export const projectOf = (key) => String(key ?? "").replace(/-\d+$/, "");

// --- people ------------------------------------------------------------------------

// Whether a Jira user (an issue's assignee, a comment's author) is `me`, the
// token's user as `myself` gave it: by name or by key (Jira Data Center
// writes a user's key, `JIRAUSER…`, where older versions wrote the name).
export function isMe(user, me) {
  if (!user || !me) return false;
  const ids = [user.name, user.key, user.accountId].filter(Boolean);
  return ids.some((id) => id === me.name || id === me.key || (me.accountId && id === me.accountId));
}

export const nameOf = (user) => user?.displayName || user?.name || "";

// --- statuses --------------------------------------------------------------------

// Jira's status categories: to do (`new`), in progress (`indeterminate`),
// done. The order the list shows them in: what is at work first.
export const CATEGORY_ORDER = { indeterminate: 0, new: 1, done: 2 };
export const categoryOf = (issue) => {
  const key = issue?.fields?.status?.statusCategory?.key;
  return key in CATEGORY_ORDER ? key : "new";
};

// The sprint's issues as the list shows them: the token user's first, ordered
// by status category and otherwise by the sprint's rank (the order Jira gave
// them in), then the rest grouped by status, the groups in category order and
// otherwise in the order their first issue came. Groups: [{ id, name, category, issues }].
export function groups(issues, me) {
  const mine = [], rest = new Map();
  for (const issue of issues ?? []) {
    if (!issue?.key) continue;
    if (isMe(issue.fields?.assignee, me)) { mine.push(issue); continue; }
    const status = issue.fields?.status ?? {};
    const id = String(status.id ?? status.name ?? "?");
    if (!rest.has(id)) rest.set(id, { id, name: status.name ?? "No status", category: categoryOf(issue), issues: [] });
    rest.get(id).issues.push(issue);
  }
  const rank = (c) => CATEGORY_ORDER[c] ?? 1;
  const sortedMine = mine.map((issue, i) => ({ issue, i })).sort((a, b) => rank(categoryOf(a.issue)) - rank(categoryOf(b.issue)) || a.i - b.i).map((x) => x.issue);
  const sortedRest = [...rest.values()].map((g, i) => ({ g, i })).sort((a, b) => rank(a.g.category) - rank(b.g.category) || a.i - b.i).map((x) => x.g);
  return { mine: sortedMine, rest: sortedRest };
}

// The list's three sections, each issue once, in the first that has it: the
// sprints' issues as they are, then of the token user's those in no sprint
// shown, then of the project's those in neither. { mine, project, above }:
// `above` counts the token user's open issues the sprints show already.
export function sections(sprints, mine, project) {
  const seen = new Set();
  for (const issues of sprints ?? []) for (const issue of issues ?? []) if (issue?.key) seen.add(issue.key);
  const fresh = (issues) => (issues ?? []).filter((issue) => issue?.key && !seen.has(issue.key) && seen.add(issue.key));
  const own = fresh(mine);
  return { mine: own, above: (mine ?? []).filter((issue) => issue?.key).length - own.length, project: fresh(project) };
}

// --- time --------------------------------------------------------------------------

// How long ago, shortly: "now", "5m", "3h", "2d".
export function ago(ms, now = Date.now()) {
  if (!ms) return "";
  const s = Math.max(0, Math.round((now - ms) / 1000));
  if (s < 45) return "now";
  if (s < 3600) return `${Math.max(1, Math.round(s / 60))}m`;
  if (s < 86400) return `${Math.round(s / 3600)}h`;
  return `${Math.round(s / 86400)}d`;
}

// How old the data shown is, for "Last read … ago".
export function lastRead(ms, now = Date.now()) {
  const s = Math.max(0, Math.round((now - ms) / 1000));
  return s < 60 ? `${s}s` : s < 3600 ? `${Math.round(s / 60)} min` : `${Math.round(s / 3600)} h`;
}

const parseTime = (text) => {
  const t = Date.parse(String(text ?? "").replace(/([+-]\d\d)(\d\d)$/, "$1:$2"));
  return Number.isFinite(t) ? t : 0;
};
export const timeOf = parseTime;

// When a sprint ends, as words: "ends in 3 days", "ends today", "ended 2 days ago".
export function sprintEnds(sprint, now = Date.now()) {
  const end = parseTime(sprint?.endDate);
  if (!end) return "";
  const days = Math.round((end - now) / 86400000);
  if (Math.abs(end - now) < 86400000 && new Date(end).toDateString() === new Date(now).toDateString()) return "ends today";
  if (days > 0) return `ends in ${days} day${days === 1 ? "" : "s"}`;
  if (days === 0) return end > now ? "ends tomorrow" : "ended yesterday";
  return `ended ${-days} day${days === -1 ? "" : "s"} ago`;
}

// --- logging work --------------------------------------------------------------------

// Time spent as typed (`1h 30m`, `1h30m`, `90m`, `1.5h`, `2d`, `1w 2d`) in the
// form Jira takes (`1h 30m`); null when it is not one.
export function parseTimeSpent(text) {
  const s = String(text ?? "").trim().toLowerCase();
  if (!s) return null;
  const parts = [];
  const left = s.replace(/(\d+(?:[.,]\d+)?)\s*([wdhm])(?![a-z])/g, (_, n, unit) => { parts.push(`${n.replace(",", ".")}${unit}`); return " "; });
  if (left.trim() || !parts.length) return null;
  if (parts.every((p) => Number.parseFloat(p) === 0)) return null;
  return parts.join(" ");
}

const pad = (n, width = 2) => String(n).padStart(width, "0");

// A day as `YYYY-MM-DD`, in local time.
export const dayOf = (ms) => {
  const d = new Date(ms);
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}`;
};

// A day as typed, `YYYY-MM-DD`, when it is a real one and not in the future: { y, m, d }, else null.
export function parseDay(text, now = Date.now()) {
  const m = /^\s*(\d{4})-(\d{1,2})-(\d{1,2})\s*$/.exec(String(text ?? ""));
  if (!m) return null;
  const [y, mo, d] = [Number(m[1]), Number(m[2]), Number(m[3])];
  const date = new Date(y, mo - 1, d);
  if (date.getFullYear() !== y || date.getMonth() !== mo - 1 || date.getDate() !== d) return null;
  if (`${y}-${pad(mo)}-${pad(d)}` > dayOf(now)) return null;
  return { y, m: mo, d };
}

// When work logged on `day` started, as Jira takes it
// (`2026-11-17T09:00:00.000+0100`): now when the day is today, else 09:00 that day.
export function startedOn(day, now = Date.now()) {
  const today = dayOf(now) === `${day.y}-${pad(day.m)}-${pad(day.d)}`;
  const at = today ? new Date(now) : new Date(day.y, day.m - 1, day.d, 9, 0, 0, 0);
  const offset = -at.getTimezoneOffset();
  const sign = offset >= 0 ? "+" : "-", abs = Math.abs(offset);
  return `${at.getFullYear()}-${pad(at.getMonth() + 1)}-${pad(at.getDate())}T${pad(at.getHours())}:${pad(at.getMinutes())}:${pad(at.getSeconds())}.000${sign}${pad(Math.floor(abs / 60))}${pad(abs % 60)}`;
}

// --- transitions ---------------------------------------------------------------------

// The fields a transition's screen requires that have no default: Jira
// refuses the transition without them, and this panel does not fill in a
// screen. Their names.
export const requiredFields = (transition) => Object.entries(transition?.fields ?? {})
  .filter(([, f]) => f?.required && !f?.hasDefaultValue)
  .map(([id, f]) => f?.name || id);

// --- text ----------------------------------------------------------------------------

// Text for one line: the app starts a new line at every "\n".
export const oneLine = (text) => String(text ?? "").split(/\s*\n\s*/).filter(Boolean).join(" ");

// --- errors --------------------------------------------------------------------------

// What a failed read or action says to the person: what went wrong, and what
// to do. `doing`: what was tried ("read DEMO-12"), `who`: the token's user.
export function explain(error, doing, who = "") {
  const kind = error?.kind, detail = error?.detail ? ` (${error.detail})` : "";
  const as = who ? ` as ${who}` : "";
  switch (kind) {
    case "auth": return `Jira did not accept the token: check it in the settings${detail}.`;
    case "forbidden": return `Jira refused to let this token${as} ${doing}${detail}.`;
    case "missing": return `Could not ${doing}: Jira has nothing there${detail}.`;
    case "bad": return `Jira did not take it: ${error.detail || error.message}.`;
    case "network": return `Could not reach Jira to ${doing}: ${error.message}.`;
    case "timeout": return `Could not ${doing}: Jira gave no answer in time. Trying again.`;
    case "settings": return error.message;
    default: return `Could not ${doing}: ${error?.message ?? error}.`;
  }
}
