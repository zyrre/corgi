// Everything that talks to Jira, and the settings it is reached with. Plain
// HTTP to Jira Server's and Data Center's REST API (`/rest/api/2`, and the
// agile API `/rest/agile/1.0` for boards and sprints) with the app's `fetch`,
// authenticated by a personal access token alone (`Authorization: Bearer`):
// no password, no SSO, no jira-cli. What the token's Jira user may not do
// comes back as an error of kind "forbidden", which the panel shows; nothing
// works around it.

import { appHome, readTextFile, writeTextFile, exec } from "herdr";
import * as fs from "fs/promises";
import { normalizeBase } from "./util.js";

// --- settings --------------------------------------------------------------------

// Kept outside the mod's own folder, which an install replaces whole, in a
// file only its owner can read: it holds the token.
export const settingsDir = () => `${appHome()}/jira`;
export const settingsPath = () => `${settingsDir()}/settings.json`;

export const blank = () => ({ url: "", token: "", board: "" });
const text = (value) => (typeof value === "string" ? value.trim() : typeof value === "number" ? String(value) : "");

// { url, token, board, source }: `source` is "file", "env" (nothing saved
// yet, and JIRA_URL and JIRA_API_TOKEN were in the environment programs get)
// or "none".
export async function loadSettings() {
  let saved = null;
  try { saved = JSON.parse((await readTextFile(settingsPath())) ?? "null"); } catch { saved = null; }
  if (saved && typeof saved === "object") {
    return { url: normalizeBase(text(saved.url)), token: text(saved.token), board: text(saved.board), source: "file" };
  }
  // The app does not start from a shell, so these are rarely there: a fallback, no more.
  try {
    const r = await exec("sh", ["-c", 'printf "%s\\n%s\\n%s" "${JIRA_URL:-$JIRA_SERVER}" "$JIRA_API_TOKEN" "$JIRA_BOARD"']);
    const [url, token, board] = String(r.stdout ?? "").split("\n").map(text);
    if (r.code === 0 && url && token) return { url: normalizeBase(url), token, board: board ?? "", source: "env" };
  } catch { /* no shell: no fallback */ }
  return { ...blank(), source: "none" };
}

// Writes the settings so that the token is never readable by others: the file
// is made empty and closed to others first, then filled.
export async function saveSettings(settings) {
  const path = settingsPath();
  const body = `${JSON.stringify({ url: normalizeBase(settings.url), token: text(settings.token), board: text(settings.board) }, null, 2)}\n`;
  await fs.mkdir(settingsDir(), { recursive: true });
  let before = await readTextFile(path);
  if (before === null) { await writeTextFile(path, "", null); before = ""; }
  await exec("chmod", ["600", path]);
  await writeTextFile(path, body, before);
  await exec("chmod", ["600", path]);
}

export const ready = (s) => Boolean(s?.url && s?.token);

// --- requests ---------------------------------------------------------------------

export class JiraError extends Error {
  constructor(kind, message, status = 0, detail = "") {
    super(message);
    // "auth" | "forbidden" | "missing" | "bad" (400: Jira did not take what was sent) | "network" | "http"
    // | "settings" | "timeout" (data.js: no answer in time)
    this.kind = kind;
    this.status = status;
    this.detail = detail;
  }
}

// What Jira said: its `errorMessages` and `errors`, or a short plain answer.
// Its HTML error pages say nothing a line can hold.
export function detailOf(body) {
  const raw = String(body ?? "").trim();
  try {
    const j = JSON.parse(raw);
    const said = [...(Array.isArray(j?.errorMessages) ? j.errorMessages : []), ...Object.values(j?.errors ?? {})]
      .filter((m) => typeof m === "string" && m.trim()).map((m) => m.trim());
    if (said.length) return said.join(" ").slice(0, 300);
    if (typeof j?.message === "string") return j.message.slice(0, 300);
  } catch { /* not JSON */ }
  return !raw || raw.startsWith("<") || raw.length > 160 ? "" : raw.split("\n")[0];
}

function failure(status, body) {
  const detail = detailOf(body);
  if (status === 401) return new JiraError("auth", "Jira did not accept this token", status, detail);
  if (status === 403) return new JiraError("forbidden", "Jira refused it", status, detail);
  if (status === 404) return new JiraError("missing", "Jira has nothing there", status, detail);
  if (status === 400) return new JiraError("bad", detail || "Jira did not take it", status, detail);
  return new JiraError("http", `Jira answered ${status}${detail ? `: ${detail}` : ""}`, status, detail);
}

// What the list reads of each issue, and what the issue view reads.
export const LIST_FIELDS = "summary,status,assignee,issuetype,priority,updated,parent";
export const ISSUE_FIELDS = "summary,status,issuetype,assignee,reporter,priority,description,comment,subtasks,issuelinks,parent,created,updated,timetracking,project";
const WATCH_FIELDS = "summary,status,assignee,comment,updated";
const PAGE = 500; // issues a search asks for at once (Jira caps it at its own maximum, 1000 by default)
const MOST = 2000; // a sprint's issues the list reads at most

// One Jira, as one set of settings reaches it.
export class Client {
  constructor(settings) {
    this.base = normalizeBase(settings.url);
    this.auth = `Bearer ${settings.token}`;
  }

  async send(method, path, json) {
    if (!this.base) throw new JiraError("settings", "No Jira URL is set");
    const headers = { Authorization: this.auth, Accept: "application/json" };
    // A POST or PUT with JSON; Jira's XSRF check passes requests that say no browser sent them.
    if (json !== undefined) Object.assign(headers, { "Content-Type": "application/json", "X-Atlassian-Token": "no-check" });
    let res;
    try {
      res = await fetch(`${this.base}${path}`, { method, headers, ...(json === undefined ? {} : { body: JSON.stringify(json) }) });
    } catch (e) {
      // The app's message names the URL; the token is in a header, never in it.
      throw new JiraError("network", String(e?.message ?? e).replace(/^fetching \S+ failed: /, "").replace(/\s*\(https?:\/\/[^)]*\)/, "") || "no answer");
    }
    const body = await res.text();
    if (res.status >= 400) throw failure(res.status, body);
    return body;
  }

  parse(body) {
    if (!String(body ?? "").trim()) return null;
    try { return JSON.parse(body); } catch { throw new JiraError("http", "Jira answered something that is not JSON: is this its base URL?"); }
  }

  async get(path) { return this.parse(await this.send("GET", path)); }

  // --- reading ---

  // Who the token is: { name, key, displayName }.
  async myself() {
    const me = await this.get("/rest/api/2/myself");
    if (!me?.name && !me?.key && !me?.accountId) throw new JiraError("auth", "Jira sees no user behind this token", 401);
    return { name: me.name ?? "", key: me.key ?? "", accountId: me.accountId ?? "", displayName: me.displayName || me.name || "" };
  }

  // The board the settings name: { id, name, type }. A project's is its
  // first Scrum board (the one with sprints), else its first.
  async board(spec) {
    if (!spec) throw new JiraError("settings", "Name a board (its id) or a project (its key) in the settings.");
    if (spec.id) {
      const b = await this.get(`/rest/agile/1.0/board/${spec.id}`);
      return { id: b?.id ?? spec.id, name: b?.name ?? `Board ${spec.id}`, type: b?.type ?? "" };
    }
    const found = await this.get(`/rest/agile/1.0/board?projectKeyOrId=${encodeURIComponent(spec.project)}&maxResults=50`);
    const boards = Array.isArray(found?.values) ? found.values : [];
    const b = boards.find((x) => x?.type === "scrum") ?? boards[0];
    if (!b) throw new JiraError("missing", `Jira has no board of project ${spec.project}`, 404, `no board of ${spec.project}`);
    return { id: b.id, name: b.name ?? `Board ${b.id}`, type: b.type ?? "", project: spec.project };
  }

  // The board's active sprints: [{ id, name, state, startDate, endDate, goal }].
  // A board without sprints (Kanban) answers 400, kept as `{ none: true }`.
  async sprints(boardId) {
    try {
      const r = await this.get(`/rest/agile/1.0/board/${boardId}/sprint?state=active&maxResults=50`);
      return { sprints: Array.isArray(r?.values) ? r.values : [] };
    } catch (e) {
      if (e.kind === "bad" && /sprint/i.test(e.detail)) return { sprints: [], none: true };
      throw e;
    }
  }

  // Issues found by JQL, every page of them up to `most`: { issues, total }.
  async search(jql, fields, { most = MOST, expand = "" } = {}) {
    const issues = [];
    let total = 0;
    for (let at = 0; at < most;) {
      const r = await this.get(`/rest/api/2/search?jql=${encodeURIComponent(jql)}&startAt=${at}&maxResults=${Math.min(PAGE, most - at)}&fields=${fields}${expand ? `&expand=${expand}` : ""}`);
      const page = Array.isArray(r?.issues) ? r.issues : [];
      issues.push(...page);
      total = r?.total ?? issues.length;
      at += page.length;
      if (!page.length || at >= total) break;
    }
    return { issues, total };
  }

  // A sprint's issues in the board's order (its rank). The agile API's
  // `sprint/<id>/issue` gives the same, but took 20 times as long on a
  // sprint of 450 issues.
  sprintIssues(sprintId) {
    return this.search(`sprint = ${Number(sprintId)} ORDER BY Rank ASC`, LIST_FIELDS);
  }

  issue(key) {
    return this.get(`/rest/api/2/issue/${encodeURIComponent(key)}?fields=${ISSUE_FIELDS}`);
  }

  // The transitions the token's user may take on an issue now, with the
  // fields their screens ask for: [{ id, name, to: { name, statusCategory }, fields }].
  async transitions(key) {
    const r = await this.get(`/rest/api/2/issue/${encodeURIComponent(key)}/transitions?expand=transitions.fields`);
    return Array.isArray(r?.transitions) ? r.transitions : [];
  }

  // The token user's issues updated in the last `minutes`, with their
  // history and comments: what the popups are made of.
  watch(minutes) {
    return this.search(`assignee = currentUser() AND updated >= -${Math.max(1, Math.ceil(minutes))}m ORDER BY updated DESC`, WATCH_FIELDS, { most: 50, expand: "changelog" });
  }

  // --- acting ---

  transition(key, id) {
    return this.send("POST", `/rest/api/2/issue/${encodeURIComponent(key)}/transitions`, { transition: { id: String(id) } });
  }

  // Assigns an issue to a user, by name (Jira Server and Data Center).
  assign(key, me) {
    return this.send("PUT", `/rest/api/2/issue/${encodeURIComponent(key)}/assignee`, { name: me.name });
  }

  comment(key, body) {
    return this.send("POST", `/rest/api/2/issue/${encodeURIComponent(key)}/comment`, { body });
  }

  // Logs work: { timeSpent: "1h 30m", started: "2026-11-17T09:00:00.000+0100", comment }.
  worklog(key, { timeSpent, started, comment }) {
    return this.send("POST", `/rest/api/2/issue/${encodeURIComponent(key)}/worklog`, { timeSpent, started, ...(comment ? { comment } : {}) });
  }

  // --- the browser ---

  page(key) { return `${this.base}/browse/${encodeURIComponent(key)}`; }
  boardPage(id) { return `${this.base}/secure/RapidBoard.jspa?rapidView=${encodeURIComponent(id)}`; }
}
