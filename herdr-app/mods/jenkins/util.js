// Pure helpers: addresses, states, time, parameters, console text. No Herdr,
// GPUI or network calls here, so they run anywhere.

// --- text as bytes -------------------------------------------------------------

// The UTF-8 bytes of a string (the app's script engine has no TextEncoder).
export function utf8(text) {
  const out = [];
  for (const ch of String(text)) {
    const c = ch.codePointAt(0);
    if (c < 0x80) out.push(c);
    else if (c < 0x800) out.push(0xc0 | (c >> 6), 0x80 | (c & 63));
    else if (c < 0x10000) out.push(0xe0 | (c >> 12), 0x80 | ((c >> 6) & 63), 0x80 | (c & 63));
    else out.push(0xf0 | (c >> 18), 0x80 | ((c >> 12) & 63), 0x80 | ((c >> 6) & 63), 0x80 | (c & 63));
  }
  return out;
}

// How many bytes a text is as UTF-8: how far a console log read went.
export function utf8Length(text) {
  let n = 0;
  for (const ch of String(text)) {
    const c = ch.codePointAt(0);
    n += c < 0x80 ? 1 : c < 0x800 ? 2 : c < 0x10000 ? 3 : 4;
  }
  return n;
}

const B64 = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
export function base64(bytes) {
  let out = "";
  for (let i = 0; i < bytes.length; i += 3) {
    const a = bytes[i], b = bytes[i + 1], c = bytes[i + 2];
    out += B64[a >> 2] + B64[((a & 3) << 4) | ((b ?? 0) >> 4)];
    out += b === undefined ? "=" : B64[((b & 15) << 2) | ((c ?? 0) >> 6)];
    out += c === undefined ? "=" : B64[c & 63];
  }
  return out;
}

// A Jenkins API token goes as HTTP Basic auth, `<user>:<token>`.
export const basicAuth = (user, token) => `Basic ${base64(utf8(`${user}:${token}`))}`;

// --- addresses -----------------------------------------------------------------

// The base URL as typed: a scheme added when it has none, no slash at its end.
export function normalizeBase(text) {
  let url = String(text ?? "").trim();
  if (!url) return "";
  if (!/^[a-z][a-z0-9+.-]*:\/\//i.test(url)) url = `https://${url}`;
  return url.replace(/\/+$/, "");
}

export const hostOf = (base) => /^[a-z][a-z0-9+.-]*:\/\/([^/]+)/i.exec(base ?? "")?.[1] ?? "";

const enc = (name) => encodeURIComponent(name);
const dec = (name) => {
  try { return decodeURIComponent(name); } catch { return name; }
};

// A place whose jobs are listed: { path, label }. `path` is its address under
// the base URL ("" for the top, "/job/a/job/b" for a folder, "/view/Deploys"
// for a view), `label` what a person calls it ("a/b").
// Typed as a folder's names ("acme-develop", "team/tools"), as the path
// Jenkins shows ("job/team/job/tools", "view/Deploys"), or as its whole URL.
export function parsePlace(text, base = "") {
  let entry = String(text ?? "").trim();
  if (base && entry.toLowerCase().startsWith(base.toLowerCase())) entry = entry.slice(base.length);
  else entry = entry.replace(/^[a-z][a-z0-9+.-]*:\/\/[^/]+/i, "");
  const parts = entry.split("/").filter(Boolean).map(dec);
  if (!parts.length) return { path: "", label: "" };
  const marked = parts.length % 2 === 0 && parts.every((part, i) => i % 2 === 1 || part === "job" || part === "view");
  const pairs = marked ? parts.map((part, i) => (i % 2 ? null : [part, parts[i + 1]])).filter(Boolean) : parts.map((part) => ["job", part]);
  return { path: pairs.map(([kind, name]) => `/${kind}/${enc(name)}`).join(""), label: pairs.map(([, name]) => name).join("/") };
}

// The places of the settings' list (commas or lines apart); none: the top.
export function parsePlaces(text, base = "") {
  const seen = new Set(), out = [];
  for (const entry of String(text ?? "").split(/[,\n]/).map((e) => e.trim()).filter(Boolean)) {
    const place = parsePlace(entry, base);
    if (!seen.has(place.path)) { seen.add(place.path); out.push(place); }
  }
  return out.length ? out : [{ path: "", label: "" }];
}

// A job or folder inside a place.
export const childOf = (place, name) => ({ path: `${place.path}/job/${enc(name)}`, label: place.label ? `${place.label}/${name}` : name });

// The last name of a label: "team/tools/rotate" is "rotate".
export const leaf = (label) => String(label ?? "").split("/").pop() || "Jenkins";

// --- states --------------------------------------------------------------------

const RESULT = { SUCCESS: "success", FAILURE: "failed", UNSTABLE: "unstable", ABORTED: "aborted", NOT_BUILT: "notbuilt" };
const COLOR = { blue: "success", green: "success", red: "failed", yellow: "unstable", aborted: "aborted", notbuilt: "notbuilt", nobuilt: "notbuilt", grey: "notbuilt", disabled: "disabled" };
export const LABEL = { success: "success", failed: "failed", unstable: "unstable", aborted: "aborted", notbuilt: "not built", disabled: "disabled", running: "running", queued: "queued", paused: "waiting for input" };

// A build's state: running, or how it ended.
export const buildState = (build) => (!build ? "notbuilt" : build.building ? "running" : RESULT[build.result] ?? "notbuilt");

// A folder (or a multibranch project, an organization): something with jobs in it, not a job.
export const isFolder = (item) => /Folder|MultiBranch|OrganizationFolder|\.Hudson$/.test(item?._class ?? "") || (Boolean(item) && item.color === undefined);

// A job's state in a list: its last build's, as Jenkins colors the job.
export function jobState(job) {
  const color = String(job?.color ?? "");
  if (color.startsWith("disabled")) return "disabled";
  if (job?.lastBuild?.building || color.endsWith("_anime")) return "running";
  if (job?.inQueue) return "queued";
  if (job?.lastBuild?.result) return RESULT[job.lastBuild.result] ?? "notbuilt";
  return COLOR[color.replace(/_anime$/, "")] ?? "notbuilt";
}

// How far a running build is, 0..1, from Jenkins' estimate; null without one.
export function progress(build, now = Date.now()) {
  if (!build?.building || !(build.estimatedDuration > 0) || !build.timestamp) return null;
  return Math.max(0, Math.min(0.99, (now - build.timestamp) / build.estimatedDuration));
}

// --- time ----------------------------------------------------------------------

// How long ago a time (ms) was: "now", "5m", "3h", "2d", "6w".
export function ago(ms, now = Date.now()) {
  if (!ms) return "";
  const s = Math.max(0, (now - ms) / 1000);
  if (s < 60) return "now";
  if (s < 3600) return `${Math.floor(s / 60)}m`;
  if (s < 86400) return `${Math.floor(s / 3600)}h`;
  if (s < 14 * 86400) return `${Math.floor(s / 86400)}d`;
  return `${Math.floor(s / (7 * 86400))}w`;
}

// How long ago data was read, for "Last read 40s ago": seconds under a minute.
export function lastRead(ms, now = Date.now()) {
  const s = Math.max(0, Math.floor((now - ms) / 1000));
  return s < 60 ? `${s}s` : ago(ms, now);
}

// How long something took (ms): "42s", "6m 05s", "1h 12m".
export function span(ms) {
  if (ms == null || ms < 0) return "";
  const sec = Math.round(ms / 1000);
  if (sec < 60) return `${sec}s`;
  const m = Math.floor(sec / 60);
  if (m < 60) return `${m}m ${String(sec % 60).padStart(2, "0")}s`;
  return `${Math.floor(m / 60)}h ${m % 60}m`;
}

// How long a build took, or for how long it has been running.
export const took = (build, now = Date.now()) => (!build ? "" : build.building ? span(now - build.timestamp) : span(build.duration));

// --- builds ---------------------------------------------------------------------

// Why a build started ("Started by user Jane Doe"), from its actions.
export const causeOf = (build) => (build?.actions ?? []).flatMap((a) => a?.causes ?? []).map((c) => c?.shortDescription).find(Boolean) ?? "";

// What a build was given: [{ name, value }], passwords left out.
export const buildParams = (build) => (build?.actions ?? []).flatMap((a) => a?.parameters ?? [])
  .filter((p) => p?.name && !/Password/.test(p._class ?? "") && p.value !== undefined && p.value !== null && typeof p.value !== "object")
  .map((p) => ({ name: p.name, value: String(p.value) }));

// The parameters a job asks for, each as a field of the build form:
// { name, kind: "text" | "boolean" | "choice" | "password" | "other", description, value, choices }.
// "other" (a file, a run, a credential) cannot be given here: Jenkins takes its default.
export function paramFields(job) {
  const defs = (job?.property ?? []).flatMap((p) => p?.parameterDefinitions ?? []);
  return defs.filter((d) => d?.name).map((d) => {
    const type = String(d.type ?? d._class ?? ""), preset = d.defaultParameterValue?.value;
    const base = { name: d.name, description: oneLine(d.description ?? ""), choices: [] };
    if (/Boolean/.test(type)) return { ...base, kind: "boolean", value: preset === true || preset === "true" };
    if (/Choice/.test(type)) {
      const choices = (Array.isArray(d.choices) ? d.choices : []).map(String);
      return { ...base, kind: "choice", choices, value: preset != null && choices.includes(String(preset)) ? String(preset) : choices[0] ?? "" };
    }
    if (/Password/.test(type)) return { ...base, kind: "password", value: "" };
    if (/String|Text/.test(type)) return { ...base, kind: "text", value: preset == null ? "" : String(preset) };
    return { ...base, kind: "other", value: "" };
  });
}

// The form's values as `buildWithParameters` takes them. A password left
// empty and a kind this form cannot give are left out: Jenkins uses its default.
export function formBody(fields, values) {
  return fields.filter((f) => f.kind !== "other" && !(f.kind === "password" && !values[f.name]))
    .map((f) => `${enc(f.name)}=${enc(String(values[f.name] ?? ""))}`).join("&");
}

// --- input steps --------------------------------------------------------------------

// Whether a Pipeline build waits at an input step, by its stages (`wfapi/describe`).
export const isPaused = (describe) => describe?.status === "PAUSED_PENDING_INPUT"
  || (describe?.stages ?? []).some((s) => s?.status === "PAUSED_PENDING_INPUT");

// What an input step a build waits at asks for (one of Pipeline Stage View's
// `pendingInputActions`: { id, message, proceedText, inputs }), as fields of
// the form `paramFields` makes. Each of its `inputs` is { type, name,
// description, definition }, the definition being the parameter's own
// (`defaultParameterValue`, `choices`); a default is also taken from
// `defaultValue` or `defaultVal`, which some versions give instead.
export function inputFields(input) {
  const defs = (input?.inputs ?? []).filter((p) => p && typeof p === "object").map((p) => {
    const d = p.definition && typeof p.definition === "object" ? p.definition : {};
    const preset = d.defaultParameterValue?.value ?? p.defaultParameterValue?.value ?? d.defaultValue ?? d.defaultVal ?? p.defaultValue;
    return {
      name: p.name ?? d.name, type: p.type ?? d.type ?? d._class ?? "", description: p.description ?? d.description ?? "",
      choices: d.choices ?? p.choices, defaultParameterValue: { value: preset },
    };
  });
  return paramFields({ property: [{ parameterDefinitions: defs }] });
}

// The form's values as an input step's `submit` takes them, in its `json`
// field's `parameter` list: [{ name, value }], a yes or no as a boolean. A
// kind this form cannot give is left out.
export const inputParams = (fields, values) => fields.filter((f) => f.kind !== "other")
  .map((f) => ({ name: f.name, value: f.kind === "boolean" ? Boolean(values[f.name]) : String(values[f.name] ?? "") }));

// --- console text -----------------------------------------------------------------

export const MAX_LINES = 5000;
const MAX_WIDTH = 1000;

// One line of text: a "\n" starts a second line even in a truncated element.
export const oneLine = (text) => String(text ?? "").split(/\s*\n\s*/).filter(Boolean).join(" ");

// Colors, cursor moves and Jenkins' hidden notes, which a panel cannot draw.
const clean = (line) => line.replace(/\x1b\[8m.*?\x1b\[0m/g, "").replace(/\x1b\[[0-9;?]*[A-Za-z]/g, "").replace(/\r/g, "").replace(/\t/g, "    ");

export const emptyLog = () => ({ lines: [], partial: "", offset: 0, dropped: 0, widest: 0 });

// Adds what a read returned to a log: whole lines are kept (the last
// MAX_LINES of them), a line still being written waits in `partial`, and
// `offset` moves on by the bytes read, which is where the next read starts.
export function appendLog(log, text) {
  if (!text) return log;
  const pieces = (log.partial + text).split("\n");
  const partial = pieces.pop();
  let lines = log.lines.concat(pieces.map((line) => {
    const out = clean(line);
    return out.length > MAX_WIDTH ? `${out.slice(0, MAX_WIDTH)}…` : out;
  }));
  let dropped = log.dropped;
  if (lines.length > MAX_LINES) { dropped += lines.length - MAX_LINES; lines = lines.slice(-MAX_LINES); }
  const widest = pieces.reduce((w, line) => Math.max(w, Math.min(line.length, MAX_WIDTH + 1)), log.widest);
  return { lines, partial, offset: log.offset + utf8Length(text), dropped, widest };
}

// The lines to draw: the kept ones and the one being written.
export const logLines = (log) => (log.partial ? [...log.lines, clean(log.partial).slice(0, MAX_WIDTH)] : log.lines);

// --- a folder's kind ---------------------------------------------------------------

// "dev" or "prod" when a folder's name says so (acme-develop, team-dev;
// acme-master, team-prod, live), else "": the tag beside its heading.
export function envOf(label) {
  const name = String(label ?? "").split("/").pop().toLowerCase();
  const word = (re) => new RegExp(`(^|[-_. ])(${re})($|[-_. ])`).test(name);
  if (word("prod|production|master|live")) return "prod";
  if (word("dev|develop|development")) return "dev";
  return "";
}

// --- pipeline stages -----------------------------------------------------------------

// A Pipeline job (its builds have stages), as opposed to a freestyle one.
export const isPipeline = (job) => /WorkflowJob/.test(job?._class ?? "");

// A stage's state, from Pipeline Stage View's status (`wfapi/describe`).
const STAGE = {
  SUCCESS: "success", FAILED: "failed", UNSTABLE: "unstable", ABORTED: "aborted", IN_PROGRESS: "running",
  PAUSED_PENDING_INPUT: "paused", NOT_EXECUTED: "skipped", NOT_BUILT: "skipped", QUEUED: "queued",
};
export const stageState = (status) => STAGE[status] ?? "notbuilt";
export const STAGE_LABEL = { ...LABEL, paused: "waiting for input", skipped: "skipped", pending: "not started" };

// How long a stage took, or for how long it has run: a running one is
// counted from its start, since the answer was read some seconds ago.
export function stageTook(stage, now = Date.now()) {
  if (!stage) return "";
  const state = stageState(stage.status);
  if ((state === "running" || state === "paused") && stage.startTimeMillis) return span(now - stage.startTimeMillis);
  return span(stage.durationMillis);
}

// The stages of a build as the view lists them: those that have started, in
// order, each with `nth` (how many before it bear its name, to find its part
// of the console), then, while the build runs, the ones still to come by the
// last finished run of the job (`runs`, newest first), as "pending". Each
// gets `expected`, that run's time for the stage of its name, when there is one.
export function stagesOf(describe, runs = [], building = false) {
  const seen = new Map();
  const started = (describe?.stages ?? []).filter((s) => s?.name).map((s) => {
    const nth = seen.get(s.name) ?? 0;
    seen.set(s.name, nth + 1);
    return { ...s, nth, state: stageState(s.status) };
  });
  const last = (runs ?? []).find((r) => r && r.id !== describe?.id && ["SUCCESS", "UNSTABLE"].includes(r.status));
  const before = new Map((last?.stages ?? []).map((s) => [s.name, s.durationMillis]));
  for (const s of started) if (before.has(s.name)) s.expected = before.get(s.name);
  if (!building || !last) return started;
  const names = new Set(started.map((s) => s.name));
  const pending = (last.stages ?? []).filter((s) => s?.name && !names.has(s.name))
    .map((s) => ({ id: null, name: s.name, nth: 0, state: "pending", status: "", expected: s.durationMillis }));
  return [...started, ...pending];
}

// How far a running stage is, 0..1, by the time it took last time; null without one.
export function stageProgress(stage, now = Date.now()) {
  if (stage?.state !== "running" || !(stage.expected > 0) || !stage.startTimeMillis) return null;
  return Math.max(0, Math.min(0.99, (now - stage.startTimeMillis) / stage.expected));
}

// Where a stage is in the console's lines: Pipeline writes `[Pipeline] { (Name)`
// where a stage starts and `[Pipeline] }`, `[Pipeline] // stage` where it ends,
// with every block inside it opened and closed the same way. `nth`: which
// stage of that name (0: the first). { from, to, open }: its lines are
// `lines.slice(from, to)`, `open` while it has not ended yet; null when the
// console has no such stage (a freestyle job, or a start no longer kept).
// The lines of parallel branches run together in the console, so a stage
// with parallel branches inside shows them all.
const OPEN = /^\[Pipeline\] (\[[^\]]*\] )?\{/, CLOSE = /^\[Pipeline\] (\[[^\]]*\] )?\}/;
const START = /^\[Pipeline\] (?:\[[^\]]*\] )?\{ \((.*)\)$/;
export function stageRange(lines, name, nth = 0) {
  let seen = 0;
  for (let i = 0; i < lines.length; i++) {
    if (START.exec(lines[i])?.[1] !== name || seen++ < nth) continue;
    let depth = 1;
    for (let j = i + 1; j < lines.length; j++) {
      if (OPEN.test(lines[j])) depth += 1;
      else if (CLOSE.test(lines[j]) && --depth === 0) return { from: i + 1, to: j, open: false };
    }
    return { from: i + 1, to: lines.length, open: true };
  }
  return null;
}

// --- what went wrong ---------------------------------------------------------------

// A failed request in a person's words. `doing`: what was tried ("build
// api_master"), `user`: the Jenkins user the token belongs to.
export function explain(error, doing, user) {
  const who = user ? `Jenkins user ${user}` : "its Jenkins user";
  const detail = error?.detail ? ` (${error.detail})` : "";
  switch (error?.kind) {
    case "auth": return `Jenkins did not accept this user and token (401). Check them in the settings.`;
    case "forbidden": return `This token may not ${doing}: ${who} lacks the permission${detail}.`;
    case "missing": return `Jenkins has nothing there to ${doing} (404): it is gone, or this token may not see it.`;
    case "network":
    case "timeout": return `Could not reach Jenkins to ${doing}: ${error.message}`;
    default: return `Could not ${doing}: ${error?.message ?? error}`;
  }
}
