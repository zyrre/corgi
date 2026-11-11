// MOCKUP ONLY (questions and backlog round, 2026-11-11): canned questions
// and backlog for the `questions` view and the open-question count on a
// supervisor's row, and which variant of each the window shows. Nothing
// here is read from Corgi; the build round replaces it with `corgi status`
// and the question files.
//
// The variant shown is kept in shared state, so every copy of the mod (the
// Projects list, the panel) and the mock tab bar draw the same one. The
// Projects list's command loop sets it: `herdr-app do corgi.mock-panel-b`,
// `corgi.mock-row-c`, `corgi.mock-tab-a`, `corgi.mock-zero`, … (see
// `mockCommand`).
import { shared, setShared } from "herdr";

// A question as `<state>/questions/Q<n>.md` holds it, parsed. `answer`:
// what the person picked in the panel, { pick, phase: "picked" | "sent", at }.
export const QUESTIONS = [
  {
    project: "alpha",
    id: "Q7",
    title: "Keep the old column names for one release?",
    blocks: { kind: "merge", what: "Rename the export columns" },
    context: "The export's columns get clearer names (cust_id becomes customer_id, and so on). Two scheduled reports read the CSV by column name.",
    options: [
      { key: "A", choice: "Write both names for one release", cost: "the reports keep working; the CSV is wider until the old names go in 0.9" },
      { key: "B", choice: "Rename outright", cost: "simpler; both reports break until someone edits them" },
      { key: "C", choice: "Keep the old names", cost: "nothing breaks; the unclear names stay" },
    ],
    recommendation: { key: "A", why: "nothing breaks, and the old names have a date to go" },
    until: "The worker waits at the merge; nothing else is held up.",
    answer: { pick: "A", phase: "picked" },
  },
  {
    project: "alpha",
    id: "Q8",
    title: "Where should the parsed schedule be cached between runs, and what should make the cache stale when the source files change under it?",
    blocks: { kind: "merge", what: "Cache the parsed schedule between runs" },
    context: "Parsing the schedule takes 4 to 6 seconds on the large calendars and runs on every start. The worker has a cache working in memory; keeping it across runs needs a place on disk and a rule for when it is stale. The source files are edited by hand and by the import job, sometimes within the same minute, and the import job rewrites files whose content did not change.",
    options: [
      { key: "A", choice: "A file beside the schedule, stale on modification time", cost: "simplest; the import job's rewrites make it stale needlessly, so the large calendars still parse slowly after each import" },
      { key: "B", choice: "The user cache folder, keyed by a hash of the sources", cost: "right after every kind of edit; hashing costs about 80 ms on the largest calendar" },
      { key: "C", choice: "No cache on disk; parse in the background at start", cost: "no stale data ever; the first view still waits when it is opened at once" },
      { key: "D", choice: "Ask again after the import job is fixed to leave unchanged files alone", cost: "the merge waits a week or more" },
    ],
    recommendation: { key: "B", why: "it is never wrong after an edit, and 80 ms is well under the parse it saves" },
    until: "The worker waits at the merge; the import job's change (beta) does not depend on it.",
    answer: null,
  },
  {
    project: "alpha",
    id: "Q9",
    title: "Drop Node 20 from CI?",
    blocks: null,
    context: "Node 20 leaves maintenance next spring. Its CI job is the slowest and caught nothing this year.",
    options: [
      { key: "A", choice: "Drop it now", cost: "CI is 4 minutes faster" },
      { key: "B", choice: "Drop it with the 0.9 release", cost: "one more release tested on it" },
    ],
    recommendation: { key: "B", why: "the release notes can say so" },
    until: "Nothing waits.",
    answer: { pick: "B", phase: "sent", at: "2 min ago" },
  },
  {
    project: "beta",
    id: "Q3",
    title: "Retry failed imports automatically?",
    blocks: { kind: "plan", what: "importer-retries" },
    context: "About one import in fifty fails on a timeout from the upstream server and works when run again by hand.",
    options: [
      { key: "A", choice: "Retry three times with backoff", cost: "most failures go away; a real outage delays the alert by about 2 minutes" },
      { key: "B", choice: "Retry once, then alert", cost: "half the noise goes; the alert stays prompt" },
      { key: "C", choice: "No retries; tune the timeout", cost: "no hidden failures; needs a week of measurements first" },
    ],
    recommendation: { key: "B", why: "it halves the noise without hiding an outage" },
    until: "The importer-retries plan waits; the other importer work goes on.",
    answer: null,
  },
];

// `<state>/backlog.md`, parsed: one entry per `## ` heading with its bullets.
export const BACKLOG = {
  alpha: [
    { heading: "Export to Parquet", bullets: ["Asked for by the analytics team", "Needs the column rename (Q7) first"] },
    { heading: "Charts in the dark theme", bullets: ["Axis labels are unreadable on dark", "Use the theme's colors, not fixed ones"] },
    { heading: "A retry budget for the scheduler", bullets: ["Cap retries per hour, not per job"] },
    { heading: "Split the settings page", bullets: ["Two pages: account and project", "Keep the old URL as a redirect"] },
    { heading: "Faster cold start", bullets: ["Profile the first 2 seconds", "The schedule cache (Q8) may be most of it"] },
  ],
  beta: [
    { heading: "Import from the new CSV layout", bullets: ["The upstream adds two columns in March"] },
    { heading: "A dry-run flag for the importer", bullets: [] },
  ],
};

export const PROJECTS = ["alpha", "beta"];

// Open questions: those not answered yet (a picked answer not sent counts).
export const open = (project) => shared("corgi.mock.zero") ? [] : QUESTIONS.filter((q) => (!project || q.project === project) && q.answer?.phase !== "sent");

// Which variant shows (with the default for each).
export const mock = () => ({
  panel: shared("corgi.mock.panel") ?? "a",
  row: shared("corgi.mock.row") ?? "a",
  tab: shared("corgi.mock.tab") ?? "a",
  page: shared("corgi.mock.page") ?? "questions",
  // The card opened in variant b's list of closed cards (none: `mock-close`).
  opened: shared("corgi.mock.opened") ?? "Q8",
  backlogOpen: Boolean(shared("corgi.mock.backlog")),
  zero: Boolean(shared("corgi.mock.zero")),
  project: shared("corgi.questions.project") ?? null,
});

// The mock's commands, from the Projects list's command loop: true when
// `command` was one.
export function mockCommand(command) {
  const m = /^mock-(panel|row|tab|page)-([a-z]+)$/.exec(command);
  if (m) {
    setShared(`corgi.mock.${m[1]}`, m[2]);
    return true;
  }
  const opened = /^mock-open-q(\d+)$/.exec(command);
  if (opened) setShared("corgi.mock.opened", `Q${opened[1]}`);
  else if (command === "mock-close") setShared("corgi.mock.opened", "none");
  else if (command === "mock-backlog") setShared("corgi.mock.backlog", !shared("corgi.mock.backlog"));
  else if (command === "mock-zero") setShared("corgi.mock.zero", !shared("corgi.mock.zero"));
  else if (command === "mock-project-alpha" || command === "mock-project-beta") setShared("corgi.questions.project", command.slice(13));
  else if (command === "mock-project-none") setShared("corgi.questions.project", null);
  else return false;
  return true;
}
