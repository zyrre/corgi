// Corgi for the Herdr app (io.github.zyrre.corgi): each project's supervisor,
// workers and open questions on herdr-core, and whether `corgi daemon` runs,
// from `corgi status --json`. It only reads, and merges a worker's branch when
// asked; it never wakes or types into an agent. It is the only tab of the
// Corgi space, which `corgi app-space` makes; clicking an agent shows it. When
// it starts it has corgi build Corgi's layout again if Herdr's has changed.
import { View, div } from "gpui-kit";
import { v_flex, h_flex } from "gpui-base";
import { agents, exec, home, appHome, readTextFile, watchFolder, theme, run, notify } from "herdr";

// The Corgi release this mod belongs to. The release sets it (see AGENTS.md,
// "Releasing"); the mod installs or updates corgi to this version and no other.
const VERSION = "0.2.3";
// The installer, from the release's own tag: it installs that tag's corgi,
// verified against the reviewed checksum pins, without the Herdr TUI plugin.
const INSTALLER = `https://raw.githubusercontent.com/zyrre/corgi/v${VERSION}/herdr-app/install.sh`;
// What the person can run by hand when the mod's own install fails.
const BY_HAND = `curl -fsSL ${INSTALLER} | sh -s -- --version ${VERSION}`;
// What the mod runs: the CLI only, so the mod's own folder is left alone. A
// failed download fails the command (a pipe into sh would succeed on nothing).
const INSTALL_SCRIPT =
  `f=$(mktemp) || exit 1; curl -fsSL "${INSTALLER}" -o "$f" && sh "$f" --version ${VERSION} --cli-only; ` +
  `s=$?; rm -f "$f"; exit $s`;
// The installer may build from source; exec's longest wait is 10 minutes.
const INSTALL_TIMEOUT_MS = 600000;
// The widest the overview's column grows: it is a whole tab, and lines
// across a wide window are hard to read.
const COLUMN_PX = 960;
// How often the status is read again besides on file and agent changes.
const POLL_MS = 5000;
// Whether this app start has tried an install yet: shared by every copy, so
// a failed install is not retried in a loop, only by the Retry action.
let attempted = false;

// Where corgi may be, in order: what the installer wrote, then the usual places.
async function candidates() {
  const list = [];
  try {
    const config = JSON.parse((await readTextFile(`${appHome()}/mods/corgi/config.json`)) ?? "{}");
    if (config.corgi) list.push(config.corgi);
  } catch (_) {
    // No config: the mod was added on its own, or by the TUI plugin's install.
  }
  const h = home();
  list.push("corgi", `${h}/.local/bin/corgi`, `${h}/.local/share/corgi/target/release/corgi`, `${h}/.cargo/bin/corgi`);
  return [...new Set(list)];
}

// "0.2.3" as numbers, to compare; null for anything else.
function parseVersion(text) {
  const m = /(\d+)\.(\d+)\.(\d+)/.exec(text ?? "");
  return m ? m.slice(1, 4).map(Number) : null;
}

function older(a, b) {
  for (let i = 0; i < 3; i++) if (a[i] !== b[i]) return a[i] < b[i];
  return false;
}

// The first corgi that knows `status --json` and is not older than this mod,
// else what was found instead: { path, version } or { found: version | null }.
async function findCorgi() {
  const want = parseVersion(VERSION);
  let found = null;
  for (const path of await candidates()) {
    let version;
    try {
      const r = await exec(path, ["--version"], { timeout: 10000 });
      if (r.code !== 0) continue;
      version = parseVersion(r.stdout);
    } catch (_) {
      continue; // not there
    }
    if (!version) continue;
    found = found ?? version.join(".");
    if (older(version, want)) continue;
    try {
      const r = await exec(path, ["status", "--help"], { timeout: 10000 });
      if (r.code === 0 && r.stdout.includes("--json")) return { path, version: version.join(".") };
    } catch (_) {
      // it ran a moment ago; treat it as too old
    }
  }
  return { found };
}

function lastLines(text, n) {
  return (text ?? "").trim().split("\n").slice(-n).join("\n");
}

// Text shown on one line: a `\n` would start a second one even when truncated.
const oneLine = (text) => (text ?? "").replace(/\s+/g, " ").trim();

const RANK = { blocked: 5, working: 4, done: 3, idle: 2, unknown: 1, exited: 0 };

// What starts the daemon, shown when it is not running.
const DAEMON_COMMAND = "corgi daemon install";

// The supervisor's state in the words the overview uses.
function supervisorWord(s) {
  if (!s) return "not running";
  if (s.state === "blocked") return "needs you";
  if (s.state === "working") return "working";
  if (s.state === "done" || s.state === "idle") return "resting";
  return s.state;
}

function stateColor(t, state) {
  return { working: t.working, blocked: t.blocked, done: t.done, idle: t.idle }[state] ?? t.unknown ?? t.dim;
}

function cacheText(c) {
  if (!c || c.state === "unknown") return "–";
  const mark = c.estimated ? "?" : "";
  if (c.state === "warm") return `warm${mark} ${c.minutes_left}m`;
  const k = c.tokens == null ? "" : c.tokens >= 1000 ? ` ${Math.round(c.tokens / 1000)}k` : ` ${c.tokens}`;
  return `cold${mark}${k}`;
}

// Why a project needs the user, if it does: questions, a blocked agent, work to merge.
function attention(p) {
  const reasons = [];
  if (p.questions.length) reasons.push(`${p.questions.length} question${p.questions.length > 1 ? "s" : ""}`);
  const blocked = p.agents.filter((a) => a.state === "blocked").length;
  if (blocked) reasons.push(`${blocked} blocked`);
  const merge = p.agents.filter((a) => a.tag === "merge").length;
  if (merge) reasons.push(`${merge} to merge`);
  return reasons;
}

export default class Corgi extends View {
  init(_props, cx) {
    this.corgi = null; // the program that answers, once found
    this.setupState = null; // { phase: "running" | "failed", title, error?, lines? }
    this.problem = null; // corgi status failing: its error
    this.status = null;
    this.open = null; // the project dir the project view shows
    this.confirm = null; // the worker a Merge waits on confirming
    this.merging = null;
    this.result = null; // { dir, name, outcome }
    this.questionText = {}; // file -> text, once opened
    this.loading = false;
    this.again = false;
    this.watched = new Set();
    this.agentKey = "";
    this.layout = null; // what `corgi app-layout` said: { result, message }
    this.layoutChecked = false;
    this.refresh(cx);
    cx.timer.every(POLL_MS, (cx) => this.refresh(cx));
    // Live agents changing is a sign the status did too; comparing is cheap.
    cx.timer.every(1000, (cx) => {
      const key = agents().map((a) => `${a.address}:${a.status}`).join(",");
      if (key !== this.agentKey) {
        this.agentKey = key;
        this.refresh(cx);
      }
    });
  }

  // Runs corgi with `args`; null when none is found (refresh installs it).
  async run(args, timeout) {
    if (!this.corgi) return null;
    try {
      return await exec(this.corgi, args, { timeout });
    } catch (e) {
      this.corgi = null; // gone: look again at the next refresh
      throw e;
    }
  }

  refresh(cx) {
    if (this.setupState?.phase === "running") return;
    if (this.loading) {
      this.again = true;
      return;
    }
    this.loading = true;
    cx.spawn(async (cx) => {
      try {
        if (!this.corgi) {
          const found = await findCorgi();
          if (found.path) {
            this.corgi = found.path;
            this.setupState = null;
          } else if (!attempted) {
            this.loading = false;
            this.install(cx, found.found);
            return;
          } else if (!this.setupState) {
            this.setupState = { phase: "failed", title: `Corgi ${VERSION} is not installed`, error: "" };
          }
        }
        if (this.corgi) {
          this.checkLayout(cx);
          const r = await this.run(["status", "--json"], 20000);
          if (r.code === 0) {
            this.status = JSON.parse(r.stdout);
            this.problem = null;
            this.watch(cx);
          } else {
            this.problem = (r.stderr || r.stdout).trim() || `corgi status exited with ${r.code}`;
          }
        }
      } catch (e) {
        this.problem = String(e);
      }
      this.loading = false;
      cx.notify();
      if (this.again) {
        this.again = false;
        this.refresh(cx);
      }
    });
  }

  // Installs this mod's corgi release (`from`: the older version found), once
  // per app start unless the person retries; then reads the status.
  install(cx, from) {
    attempted = true;
    const title = from ? `Updating Corgi ${from} → ${VERSION}…` : `Installing Corgi ${VERSION}…`;
    this.setupState = { phase: "running", title };
    cx.notify();
    cx.spawn(async (cx) => {
      let failure = null;
      try {
        const r = await exec("sh", ["-c", INSTALL_SCRIPT], { timeout: INSTALL_TIMEOUT_MS });
        if (r.code !== 0) {
          failure = { error: `The installer exited with code ${r.code}.`, lines: lastLines(`${r.stdout}\n${r.stderr}`, 12) };
        } else {
          const found = await findCorgi();
          if (found.path) this.corgi = found.path;
          else failure = { error: `The installer finished, but no corgi ${VERSION} answers corgi status --json.`, lines: lastLines(r.stderr, 12) };
        }
      } catch (e) {
        failure = { error: `The installer did not finish: ${e}`, lines: "" };
      }
      this.setupState = failure ? { phase: "failed", title: from ? "Updating Corgi failed" : "Installing Corgi failed", ...failure } : null;
      cx.notify();
      if (!failure) this.refresh(cx);
    });
  }

  // Corgi's layout is Herdr's `main` with the Projects list in its sidebar,
  // and Herdr replaces `main` on its updates: once per start, corgi builds it
  // again if `main` changed. It never switches the window to it. What it
  // could not build from shows in the overview.
  checkLayout(cx) {
    if (this.layoutChecked) return;
    this.layoutChecked = true;
    cx.spawn(async (cx) => {
      try {
        const r = await exec(this.corgi, ["app-layout", "--home", appHome(), "--json"], { timeout: 20000 });
        this.layout = JSON.parse(r.stdout);
      } catch (_) {
        return; // a corgi from before it built layouts
      }
      if (this.layout.result === "rebuilt") {
        notify({ title: "Corgi layout rebuilt", body: "Herdr's main layout changed, and Corgi's follows it.", level: "info" });
      }
      cx.notify();
    });
  }

  // Follows the folders holding the supervisors' state, once each.
  watch(cx) {
    for (const p of this.status.projects) {
      if (!p.state_dir) continue;
      const parent = p.state_dir.replace(/\/[^/]+$/, "");
      if (this.watched.has(parent)) continue;
      this.watched.add(parent);
      cx.spawn(async (cx) => {
        for (;;) {
          try {
            await watchFolder(parent);
          } catch (_) {
            return; // the timer still refreshes
          }
          this.refresh(cx);
        }
      });
    }
  }

  merge(cx, project, name, extra) {
    this.confirm = null;
    this.merging = name;
    this.result = null;
    cx.notify();
    cx.spawn(async (cx) => {
      let outcome;
      try {
        const r = await this.run(["merge", name, "--project", project.dir, "--json", ...extra], 600000);
        outcome = r ? JSON.parse(r.stdout) : { result: "refused", message: "corgi is not installed" };
      } catch (e) {
        outcome = { result: "refused", message: `The merge did not answer: ${e}` };
      }
      this.merging = null;
      this.result = { dir: project.dir, name, outcome };
      cx.notify();
      this.refresh(cx);
    });
  }

  toggleQuestion(cx, q) {
    if (!q.file) return;
    if (q.file in this.questionText) {
      delete this.questionText[q.file];
      cx.notify();
      return;
    }
    cx.spawn(async (cx) => {
      this.questionText[q.file] = (await readTextFile(q.file)) ?? "(the question's file is gone)";
      cx.notify();
    });
  }

  render(cx) {
    const t = theme();
    const root = v_flex().id("corgi").size_full().bg(t.surface).text_size(13).text_color(t.foreground);
    if (this.setupState) return root.child(this.setup(t));
    if (!this.status && this.problem) return root.p(12).child(div().text_color(t.blocked).child(`corgi status failed: ${this.problem}`));
    if (!this.status) return root.p(12).child(div().text_color(t.muted).child("Reading Corgi's status…"));
    const project = this.open && this.status.projects.find((p) => p.dir === this.open);
    return root.child(project ? this.projectView(cx, t, project) : this.overview(cx, t));
  }

  // Installing or updating corgi, or why that failed, with the command to run
  // by hand and a Retry.
  setup(t) {
    const s = this.setupState;
    const mono = (text) => div().p(8).rounded(6).bg(t.raised).text_size(12).font_family("JetBrains Mono").whitespace_normal().child(text);
    if (s.phase === "running") {
      return v_flex()
        .p(14)
        .gap(8)
        .child(div().font_semibold().text_size(15).child("Corgi"))
        .child(div().text_color(t.foreground).child(s.title))
        .child(div().text_color(t.dim).text_size(12).child("A source build, when no verified binary is available, takes a few minutes."));
    }
    return v_flex()
      .p(14)
      .gap(8)
      .child(div().font_semibold().text_size(15).child(s.title))
      .when(Boolean(s.error), (el) => el.child(div().text_color(t.blocked).child(s.error)))
      .when(Boolean(s.lines), (el) => el.child(mono(s.lines)))
      .child(div().text_color(t.muted).child("To install it by hand, run:"))
      .child(mono(BY_HAND))
      .child(
        h_flex().child(
          div()
            .id("retry")
            .px(12)
            .h(28)
            .flex()
            .items_center()
            .rounded(6)
            .bg(t.solid)
            .text_color(t.solid_foreground)
            .cursor_pointer()
            .hover((st) => st.opacity(0.88))
            .on_click((_e, cx) => {
              this.setupState = null;
              this.corgi = null;
              attempted = false;
              this.refresh(cx);
            })
            .child("Retry"),
        ),
      );
  }

  header(t, title, back) {
    const s = this.status;
    return h_flex()
      .px(12)
      .py(8)
      .gap(8)
      .items_center()
      .border_b(1)
      .border_color(t.border)
      .when(Boolean(back), (el) =>
        el.child(
          div()
            .id("back")
            .px(6)
            .rounded(4)
            .cursor_pointer()
            .text_color(t.muted)
            .hover((st) => st.bg(t.hover))
            .on_click(back)
            .child("← All projects"),
        ),
      )
      .child(div().flex_1().font_semibold().text_size(14).child(title))
      .when(s.hc?.running === false, (el) => el.child(div().text_color(t.blocked).text_size(11).child("herdr-core not running")))
      .when(Boolean(s.hc?.error), (el) => el.child(div().text_color(t.blocked).text_size(11).child("herdr-core not answering")))
      .child(div().text_color(t.dim).text_size(11).child(this.loading ? "…" : s.generated.slice(11, 19)));
  }

  // The column a view's content goes in, centred once the tab is wider.
  column() {
    return v_flex().w_full().max_w(COLUMN_PX).mx_auto().gap(10);
  }

  overview(cx, t) {
    const projects = this.status.projects;
    return v_flex()
      .size_full()
      .child(this.header(t, "Corgi", null))
      .child(
        v_flex()
          .id("projects")
          .flex_1()
          .min_h(0)
          .overflow_y_scrollbar()
          .p(10)
          .child(
            this.column()
              .when(Boolean(this.status.daemon), (el) => el.child(this.daemonLine(t, this.status.daemon)))
              .when(Boolean(this.status.hc?.error), (el) => el.child(div().text_color(t.blocked).text_size(12).child(oneLine(`herdr-core did not answer: ${this.status.hc.error}`))))
              .when(this.layout?.result === "kept" || this.layout?.result === "missing", (el) => el.child(this.layoutLine(t, this.layout)))
              .when(projects.length === 0, (el) => el.child(div().text_color(t.muted).child("No projects with a supervisor or a Corgi agent.")))
              .children(projects.map((p) => this.projectBlock(cx, t, p))),
          ),
      );
  }

  // Whether `corgi daemon` runs: a quiet line when it does, else a warning
  // with the command that starts it.
  daemonLine(t, d) {
    if (d.running) {
      return div().text_color(t.dim).text_size(12).child(`Corgi daemon running${d.pid ? ` · pid ${d.pid}` : ""}`);
    }
    return v_flex()
      .p(10)
      .gap(6)
      .rounded(8)
      .border_1()
      .border_color(t.blocked)
      .bg(t.background)
      .child(div().text_color(t.blocked).font_semibold().whitespace_normal().child("Corgi daemon not running: supervisors on the Herdr app won't be woken or handed over"))
      .child(div().text_color(t.muted).text_size(12).child(d.installed ? "Its service is installed but not running. To restart it, run:" : "To start it, run:"))
      .child(div().p(6).rounded(6).bg(t.raised).text_size(12).font_family("JetBrains Mono").child(DAEMON_COMMAND));
  }

  // Why Corgi's layout was not built from Herdr's `main` this time: the last
  // build stays in use (kept), or there is none (missing).
  layoutLine(t, layout) {
    return div()
      .text_size(12)
      .text_color(layout.result === "kept" ? t.blocked : t.dim)
      .whitespace_normal()
      .child(`Corgi layout: ${oneLine(layout.message)}`);
  }

  projectBlock(cx, t, p) {
    const needs = attention(p);
    const workers = p.agents.filter((a) => a.role === "worker");
    const counts = {};
    for (const w of workers) counts[w.state] = (counts[w.state] ?? 0) + 1;
    const byState = Object.keys(counts)
      .sort((a, b) => (RANK[b] ?? 0) - (RANK[a] ?? 0))
      .map((s) => div().text_color(stateColor(t, s)).child(`${counts[s]} ${s}`));
    const sup = supervisorWord(p.supervisor);
    const facts = [
      p.ledger.ready_to_merge ? `${p.ledger.ready_to_merge} ready to merge in the ledger` : null,
      p.inbox_pending ? `${p.inbox_pending} inbox item${p.inbox_pending > 1 ? "s" : ""} waiting` : null,
      p.ledger.dispatched ? `${p.ledger.dispatched} dispatched` : null,
      p.backlog ? `backlog ${p.backlog}` : null,
    ].filter(Boolean);
    return v_flex()
      .id(`project-${p.dir}`)
      .p(10)
      .gap(5)
      .rounded(8)
      .border_1()
      .border_color(needs.length ? t.blocked : t.border)
      .bg(t.background)
      .cursor_pointer()
      .hover((s) => s.bg(t.hover))
      .on_click((_e, cx) => {
        this.open = p.dir;
        this.confirm = null;
        cx.notify();
      })
      .child(
        h_flex()
          .gap(8)
          .items_center()
          .child(div().font_semibold().text_size(15).child(p.name))
          .child(div().text_color(t.dim).text_size(11).child(p.base ?? ""))
          .child(div().flex_1())
          .when(needs.length > 0, (el) =>
            el.child(div().px(6).rounded(4).bg(t.blocked).text_color(t.background).text_size(11).font_semibold().child(`needs you · ${needs.join(", ")}`)),
          ),
      )
      .child(
        h_flex()
          .gap(6)
          .child(div().text_color(t.muted).child("Supervisor"))
          .child(div().text_color(p.supervisor ? stateColor(t, p.supervisor.state) : t.dim).child(sup))
          .when(Boolean(p.supervisor), (el) => el.child(div().text_color(t.dim).child(`ctx ${p.supervisor.ctx_pct ?? "–"}% · ${cacheText(p.supervisor.cache)}`))),
      )
      .child(
        h_flex()
          .gap(8)
          .flex_wrap()
          .child(div().text_color(t.muted).child(workers.length ? `${workers.length} worker${workers.length > 1 ? "s" : ""}` : "No workers"))
          .children(byState),
      )
      .when(p.questions.length > 0, (el) =>
        el.child(
          v_flex()
            .gap(2)
            .children(p.questions.slice(0, 3).map((q, i) => div().id(`q-${p.dir}-${i}`).truncate().text_color(t.foreground).child(`${q.id ?? "?"}  ${oneLine(q.title)}`)))
            .when(p.questions.length > 3, (e) => e.child(div().text_color(t.dim).child(`+${p.questions.length - 3} more`))),
        ),
      )
      .when(facts.length > 0, (el) => el.child(div().text_color(t.dim).text_size(12).child(facts.join(" · "))));
  }

  projectView(cx, t, p) {
    const result = this.result && this.result.dir === p.dir ? this.result : null;
    const sorted = [...p.agents].sort((a, b) => (a.role === "supervisor" ? -1 : b.role === "supervisor" ? 1 : 0));
    return v_flex()
      .size_full()
      .child(
        this.header(t, p.name, (_e, cx) => {
          this.open = null;
          this.confirm = null;
          cx.notify();
        }),
      )
      .child(
        v_flex()
          .id("project")
          .flex_1()
          .min_h(0)
          .overflow_y_scrollbar()
          .p(10)
          .child(
            this.column()
              .child(div().text_color(t.dim).text_size(12).child(`${p.dir} · ${p.base ?? "no branch"}`))
              .when(Boolean(result), (el) => el.child(this.resultBanner(cx, t, p, result)))
              .child(div().font_semibold().child(sorted.length ? "Agents" : "No agents running"))
              .children(sorted.map((a) => this.agentRow(cx, t, p, a)))
              .when(p.questions.length > 0, (el) =>
                el.child(div().pt(4).font_semibold().child(`Open questions (${p.questions.length})`)).children(p.questions.map((q, i) => this.questionRow(cx, t, p, q, i))),
              )
              .child(
                div()
                  .text_color(t.dim)
                  .text_size(12)
                  .child(
                    `Ledger: ${p.ledger.needs_answer} need an answer · ${p.ledger.ready_to_merge} ready to merge · ${p.ledger.dispatched} dispatched · inbox ${p.inbox_pending} waiting · backlog ${p.backlog}`,
                  ),
              ),
          ),
      );
  }

  agentRow(cx, t, p, a) {
    const tagged = a.tag === "merge";
    const confirming = this.confirm === a.name;
    const merging = this.merging === a.name;
    // A button's click is its own, not also the row's.
    const button = (id, label, bg, fg, onClick) =>
      div()
        .id(id)
        .px(10)
        .h(24)
        .flex()
        .items_center()
        .rounded(6)
        .bg(bg)
        .text_color(fg)
        .text_size(12)
        .cursor_pointer()
        .hover((s) => s.opacity(0.88))
        .on_click((e, cx) => {
          cx.stop_propagation();
          onClick(e, cx);
        })
        .child(label);
    return v_flex()
      .id(`agent-${a.terminal}`)
      .p(8)
      .gap(3)
      .rounded(6)
      .border_1()
      .border_color(tagged ? t.accent : t.border)
      .bg(t.background)
      // Shows the agent's terminal.
      .when(Boolean(a.terminal), (el) =>
        el
          .cursor_pointer()
          .hover((s) => s.bg(t.hover))
          .on_click(() => run("agent.focus", { terminal: a.terminal })),
      )
      .child(
        h_flex()
          .gap(8)
          .items_center()
          .child(div().w(8).h(8).rounded(4).bg(stateColor(t, a.state)))
          .child(div().font_semibold().child(a.name))
          .child(div().text_color(t.dim).text_size(11).child(a.role))
          .child(div().text_color(stateColor(t, a.state)).text_size(12).child(a.state))
          .child(div().flex_1())
          .when(tagged, (el) => el.child(div().px(5).rounded(4).bg(t.accent).text_color(t.accent_foreground).text_size(11).font_semibold().child("MERGE")))
          .when(tagged && !confirming && !merging && !this.merging, (el) =>
            el.child(
              button(`merge-${a.terminal}`, "Merge", t.solid, t.solid_foreground, (_e, cx) => {
                this.confirm = a.name;
                this.result = null;
                cx.notify();
              }),
            ),
          )
          .when(merging, (el) => el.child(div().text_color(t.muted).text_size(12).child("Merging…"))),
      )
      .child(div().truncate().child(oneLine(a.task) || "–"))
      .child(div().text_color(t.dim).text_size(12).child(`${a.model ?? "–"} · ctx ${a.ctx_pct ?? "–"}% · cache ${cacheText(a.cache)} · ${a.branch ?? "–"}`))
      .when(confirming, (el) =>
        el.child(
          h_flex()
            .gap(8)
            .pt(4)
            .items_center()
            .child(div().flex_1().text_color(t.foreground).child(`Merge ${a.branch ?? a.name} into ${p.base ?? "the base branch"} and push?`))
            .child(
              button(`cancel-${a.terminal}`, "Cancel", t.raised, t.foreground, (_e, cx) => {
                this.confirm = null;
                cx.notify();
              }),
            )
            .child(button(`confirm-${a.terminal}`, "Merge + push", t.solid, t.solid_foreground, (_e, cx) => this.merge(cx, p, a.name, []))),
        ),
      );
  }

  resultBanner(cx, t, p, result) {
    const o = result.outcome;
    const color = o.result === "merged" ? t.working : o.result === "push-failed" ? t.done : t.blocked;
    const retry = o.reason === "behind" ? ["Pull, merge + push", ["--pull"]] : o.reason === "fetch-failed" ? ["Merge + push without fetching", ["--skip-upstream"]] : null;
    return v_flex()
      .p(8)
      .gap(6)
      .rounded(6)
      .border_1()
      .border_color(color)
      .child(div().text_color(color).font_semibold().child(`${result.name}: ${o.result}`))
      .child(div().text_color(t.foreground).child(o.message))
      .when(Boolean(retry) && !this.merging, (el) =>
        el.child(
          h_flex().child(
            div()
              .id("merge-retry")
              .px(10)
              .h(24)
              .flex()
              .items_center()
              .rounded(6)
              .bg(t.solid)
              .text_color(t.solid_foreground)
              .text_size(12)
              .cursor_pointer()
              .on_click((_e, cx) => this.merge(cx, p, result.name, retry[1]))
              .child(retry[0]),
          ),
        ),
      );
  }

  questionRow(cx, t, p, q, i) {
    const text = q.file ? this.questionText[q.file] : undefined;
    return v_flex()
      .id(`question-${p.dir}-${i}`)
      .p(8)
      .gap(4)
      .rounded(6)
      .bg(t.background)
      .when(Boolean(q.file), (el) => el.cursor_pointer().hover((s) => s.bg(t.hover)))
      .on_click((_e, cx) => this.toggleQuestion(cx, q))
      .child(
        h_flex()
          .gap(8)
          .child(div().font_semibold().text_color(t.blocked).child(q.id ?? "?"))
          .child(div().flex_1().child(oneLine(q.title)))
          .when(Boolean(q.blocks), (el) => el.child(div().text_color(t.dim).text_size(11).child(q.blocks))),
      )
      .when(text !== undefined, (el) => el.child(div().text_color(t.muted).text_size(12).whitespace_normal().child(text)));
  }
}
