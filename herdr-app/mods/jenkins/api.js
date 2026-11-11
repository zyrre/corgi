// Everything that talks to Jenkins, and the settings it is reached with.
// Plain HTTP to Jenkins' JSON API with the app's `fetch`, authenticated by an
// API token alone (HTTP Basic, `<user>:<token>`): no password, no SSO, no
// jenkins-cli. What the token's Jenkins user may not do comes back as an
// error of kind "forbidden", which the panel shows; nothing works around it.

import { appHome, readTextFile, writeTextFile, exec } from "herdr";
import * as fs from "fs/promises";
import { basicAuth, normalizeBase } from "./util.js";

// --- settings --------------------------------------------------------------------

// Kept outside the mod's own folder, which an install replaces whole, in a
// file only its owner can read: it holds the token.
export const settingsDir = () => `${appHome()}/jenkins`;
export const settingsPath = () => `${settingsDir()}/settings.json`;

const blank = () => ({ url: "", user: "", token: "", folders: "" });
const text = (value) => (typeof value === "string" ? value.trim() : "");

// { url, user, token, folders, source }: `source` is "file", "env" (nothing
// saved yet, and JENKINS_URL, JENKINS_USER and JENKINS_API_TOKEN were in the
// environment programs get) or "none".
export async function loadSettings() {
  let saved = null;
  try { saved = JSON.parse((await readTextFile(settingsPath())) ?? "null"); } catch { saved = null; }
  if (saved && typeof saved === "object") {
    return { url: normalizeBase(text(saved.url)), user: text(saved.user), token: text(saved.token), folders: text(saved.folders), source: "file" };
  }
  // The app does not start from a shell, so these are rarely there: a fallback, no more.
  try {
    const r = await exec("sh", ["-c", 'printf "%s\\n%s\\n%s\\n%s" "$JENKINS_URL" "$JENKINS_USER" "$JENKINS_API_TOKEN" "$JENKINS_FOLDERS"']);
    const [url, user, token, folders] = String(r.stdout ?? "").split("\n").map(text);
    if (r.code === 0 && url && user && token) return { url: normalizeBase(url), user, token, folders: folders ?? "", source: "env" };
  } catch { /* no shell: no fallback */ }
  return { ...blank(), source: "none" };
}

// Writes the settings so that the token is never readable by others: the file
// is made empty and closed to others first, then filled.
export async function saveSettings(settings) {
  const path = settingsPath();
  const body = `${JSON.stringify({ url: normalizeBase(settings.url), user: text(settings.user), token: text(settings.token), folders: text(settings.folders) }, null, 2)}\n`;
  await fs.mkdir(settingsDir(), { recursive: true });
  let before = await readTextFile(path);
  if (before === null) { await writeTextFile(path, "", null); before = ""; }
  await exec("chmod", ["600", path]);
  await writeTextFile(path, body, before);
  await exec("chmod", ["600", path]);
}

export const ready = (s) => Boolean(s?.url && s?.user && s?.token);

// --- requests ---------------------------------------------------------------------

export class JenkinsError extends Error {
  constructor(kind, message, status = 0, detail = "") {
    super(message);
    this.kind = kind; // "auth" | "forbidden" | "missing" | "network" | "http" | "settings"
    this.status = status;
    this.detail = detail;
  }
}

// What Jenkins said in a short plain-text answer; its HTML error pages say nothing a line can hold.
function detailOf(body) {
  const line = String(body ?? "").trim();
  return !line || line.startsWith("<") || line.length > 160 ? "" : line.split("\n")[0];
}

function failure(status, body) {
  const detail = detailOf(body);
  if (status === 401) return new JenkinsError("auth", "Jenkins did not accept this user and token", status, detail);
  if (status === 403) return new JenkinsError("forbidden", "Jenkins refused it", status, detail);
  if (status === 404) return new JenkinsError("missing", "Jenkins has nothing there", status, detail);
  return new JenkinsError("http", `Jenkins answered ${status}${detail ? `: ${detail}` : ""}`, status, detail);
}

// One Jenkins, as one set of settings reaches it. Keeps the CSRF crumb.
export class Client {
  constructor(settings) {
    this.base = normalizeBase(settings.url);
    this.user = settings.user;
    this.auth = basicAuth(settings.user, settings.token);
    this.crumb = undefined; // undefined: not asked yet; null: this Jenkins has no issuer
  }

  async send(method, path, { headers = {}, body } = {}) {
    if (!this.base) throw new JenkinsError("settings", "No Jenkins URL is set");
    let res;
    try {
      res = await fetch(`${this.base}${path}`, { method, headers: { Authorization: this.auth, ...headers }, ...(body === undefined ? {} : { body }) });
    } catch (e) {
      // The app's message names the URL; the token is in a header, never in it.
      throw new JenkinsError("network", String(e?.message ?? e).replace(/^fetching \S+ failed: /, "").replace(/\s*\(https?:\/\/[^)]*\)/, "") || "no answer");
    }
    return res;
  }

  async getText(path) {
    const res = await this.send("GET", path);
    const body = await res.text();
    if (res.status >= 400) throw failure(res.status, body);
    return body;
  }

  async getJson(path, tree) {
    return this.parse(await this.getText(`${path}/api/json${tree ? `?tree=${tree}` : ""}`));
  }

  parse(body) {
    try { return JSON.parse(body); } catch { throw new JenkinsError("http", "Jenkins answered something that is not JSON: is this its base URL?"); }
  }

  // The crumb a Jenkins with CSRF protection wants on a POST; none when it has no issuer.
  async crumbHeader(fresh = false) {
    if (this.crumb === undefined || fresh) {
      try {
        const c = await this.getJson("/crumbIssuer");
        this.crumb = c?.crumbRequestField && c?.crumb ? { [c.crumbRequestField]: c.crumb } : null;
      } catch (e) {
        if (e.kind === "auth" || e.kind === "network") throw e;
        this.crumb = null;
      }
    }
    return this.crumb ?? {};
  }

  // A POST. Jenkins answers 201 (queued) or a redirect, which the app follows; a
  // refusal over the crumb is tried once more with a fresh one.
  async post(path, form) {
    const once = async (fresh) => {
      const headers = { ...(await this.crumbHeader(fresh)), ...(form === undefined ? {} : { "Content-Type": "application/x-www-form-urlencoded" }) };
      const res = await this.send("POST", path, { headers, body: form ?? "" });
      return { status: res.status, body: res.status >= 400 ? await res.text() : "" };
    };
    let r = await once(false);
    if (r.status === 403 && /crumb/i.test(r.body)) r = await once(true);
    if (r.status >= 400) throw failure(r.status, r.body);
    return r.status;
  }

  // --- reading ---

  // Who the token authenticates as: { id, name }.
  async whoAmI() {
    const who = await this.getJson("/whoAmI", "name,anonymous,authenticated");
    if (who?.anonymous || who?.authenticated === false || !who?.name) {
      throw new JenkinsError("auth", "Jenkins sees no user behind this user name and token", 401);
    }
    let name = who.name;
    try { name = (await this.getJson("/me", "id,fullName"))?.fullName || name; } catch { /* the id will do */ }
    return { id: who.name, name };
  }

  // The jobs and folders of a place, each job with its last build.
  async list(place) {
    const data = await this.getJson(place.path, "jobs[_class,name,color,inQueue,lastBuild[number,result,building,timestamp,duration,estimatedDuration]]");
    return Array.isArray(data?.jobs) ? data.jobs.filter((j) => j?.name) : [];
  }

  // A job with its parameters and its recent builds.
  job(place) {
    return this.getJson(place.path, "name,fullName,description,color,buildable,inQueue,"
      + "property[parameterDefinitions[name,type,description,choices,defaultParameterValue[value]]],"
      + "builds[number,result,building,timestamp,duration,estimatedDuration,actions[causes[shortDescription],parameters[name,value]]]{0,20}");
  }

  build(place, number) {
    return this.getJson(`${place.path}/${number}`, "number,result,building,timestamp,duration,estimatedDuration,actions[causes[shortDescription],parameters[name,value]]");
  }

  // A Pipeline build's stages, from the Pipeline Stage View plugin:
  // { id, status, startTimeMillis, durationMillis, stages: [{ id, name,
  // status, startTimeMillis, durationMillis, pauseDurationMillis, error? }] }.
  // A freestyle build, or a Jenkins without the plugin, answers 404 ("missing").
  async stages(place, number) {
    return this.parse(await this.getText(`${place.path}/${number}/wfapi/describe`));
  }

  // The job's last ten runs with their stages, newest first: what each stage
  // took last time.
  async runs(place) {
    const runs = this.parse(await this.getText(`${place.path}/wfapi/runs`));
    return Array.isArray(runs) ? runs : [];
  }

  // The page of a stage's log, for the browser.
  stagePage(place, number, node) {
    return `${this.base}${place.path}/${number}/execution/node/${encodeURIComponent(node)}/log/`;
  }

  // The console text from byte `start` on (0: all of it).
  logFrom(place, number, start) {
    return this.getText(`${place.path}/${number}/logText/progressiveText?start=${start}`);
  }

  // --- acting ---

  // Queues a build. `form`: the urlencoded parameters, or null for a job without any.
  trigger(place, form) {
    return form === null ? this.post(`${place.path}/build`) : this.post(`${place.path}/buildWithParameters`, form);
  }

  abort(place, number) {
    return this.post(`${place.path}/${number}/stop`);
  }

  // The page of a job, a build or a place, for the browser.
  page(place, number) {
    return `${this.base}${place.path}/${number === undefined ? "" : `${number}/`}`;
  }
}
