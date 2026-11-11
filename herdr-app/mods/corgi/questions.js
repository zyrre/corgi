// The Corgi mod's Questions panel (its `questions` view, main.js), in the
// layout's right-hand panel stack `panel-body` beside Files and Changes,
// where `corgi app-layout` puts it with a Questions button in the tab bar.
//
// It shows one project, the current one (the Projects list says which: the
// active space's, or the one whose count on a supervisor's row was
// clicked): its open questions to the user first, then its backlog. Each
// question is a card with its options as buttons (the recommended one
// marked), a field for an answer of the user's own, and the card's Send,
// which runs `corgi answer` with that one answer. The answer waits in the
// supervisor's inbox until the supervisor is between turns and its box
// holds no draft (queued), is then typed into its box (sent), and the card
// goes once the supervisor deletes the question's file. Answered cards are
// closed (title, what it blocks, the answer), and so are all cards while
// more than two are open; a click opens one. A backlog entry's Start asks
// the supervisor, the same way, to plan it and propose the plan to the
// user; nothing is started by the click.
//
// What it shows is read through data.js, once for every copy, and only
// while the panel shows: `corgi questions --json` and `corgi backlog
// --json`, again whenever `corgi status` says the project's questions or
// what waits in its inbox changed.
import { div } from "gpui-kit";
import { v_flex, h_flex, InputState, Input } from "gpui-base";
import { theme, layout, shared } from "herdr";
import { find, oneLine, spinner } from "./ui.js";
import { store, CURRENT } from "./data.js";

// More open questions than this, and every card starts closed.
const OPEN_CARDS = 2;
// Lines of a backlog entry's body shown.
const BODY_LINES = 3;

const stop = (_e, cx) => cx.stop_propagation();

// "3 min ago", from an RFC 3339 time.
function ago(at) {
  const t = Date.parse(at ?? "");
  if (!Number.isFinite(t)) return "";
  const s = Math.max(0, Math.round((Date.now() - t) / 1000));
  if (s < 60) return "just now";
  if (s < 3600) return `${Math.floor(s / 60)} min ago`;
  if (s < 86400) return `${Math.floor(s / 3600)} h ago`;
  return `${Math.floor(s / 86400)} d ago`;
}

// What an answer says on a card: an option's key, or the user's own words.
const said = (answer) => (answer?.startsWith("other: ") ? `“${answer.slice(7)}”` : answer ?? "");

export default class Questions {
  init(_props, cx) {
    this.cards = {}; // project + id -> { choice, input, sending, problem }
    this.opened = new Set(); // cards whose default (open or closed) a click turned over, by key
    this.starting = {}; // backlog heading -> { sending, problem }
    this.version = -1;
    cx.timer.every(1000, (cx) => {
      const current = shared(CURRENT);
      if (current && this.shown()) store.refreshPanel(cx, current.machine ?? null, current.dir, current.key);
      if (current) this.makeCards(current);
      if (this.version !== store.version) cx.notify();
    });
  }

  // A card's state, with its Other field, for each question read: made
  // here, on the timer, since the app makes no field while it draws. A
  // card read since draws without its field for at most a second.
  makeCards(current) {
    const project = JSON.stringify([current.machine ?? null, current.dir]);
    for (const q of store.panelOf(current.machine ?? null, current.dir)?.questions?.questions ?? []) {
      const key = `${project}\u0000${q.id}`;
      if (this.cards[key]) continue;
      const c = (this.cards[key] = { choice: null, sending: false, problem: null, input: InputState.new({ placeholder: "Other answer: type your own…" }) });
      c.input.on("change", (_e, cx) => {
        const text = c.input.value().trim();
        if (text) c.choice = { other: true };
        else if (c.choice?.other) c.choice = null;
        c.problem = null;
        cx.notify();
      });
      c.input.on("submit", (_e, cx) => this.send(cx, project, q.id));
    }
  }

  // Whether the layout shows this panel now: its stack's shown child is a
  // block of the Questions view, and the panel is not hidden.
  shown() {
    const root = layout();
    const panel = find(root, "panel");
    const body = find(root, "panel-body");
    const child = body && find(body, body.active);
    return Boolean(panel && !panel.hidden && child?.content?.props?.view === "questions");
  }

  // A card's state: what makeCards made, or, before it has, one without a
  // field.
  card(project, id) {
    return this.cards[`${project}\u0000${id}`] ?? { choice: null, sending: false, problem: null, input: null };
  }

  // Sends one card's answer.
  send(cx, project, id) {
    const current = shared(CURRENT);
    const c = this.card(project, id);
    if (!current || c.sending || !c.choice) return;
    const text = oneLine(c.input?.value() ?? "");
    if (c.choice.other && !text) return;
    const answer = c.choice.other ? `other: ${text}` : c.choice.key;
    c.sending = true;
    c.problem = null;
    cx.notify();
    cx.spawn(async (cx) => {
      const r = await store.answer(cx, current.machine ?? null, current.dir, [`${id}=${answer}`]);
      c.sending = false;
      if (r?.result === "queued") {
        c.choice = null;
        c.input?.set_value("");
        this.opened.delete(`${project}\u0000${id}`);
      } else c.problem = oneLine(r?.message) || "The answer could not be sent.";
      cx.notify();
    });
  }

  start(cx, heading) {
    const current = shared(CURRENT);
    const s = (this.starting[heading] ??= { sending: false, problem: null });
    if (!current || s.sending) return;
    s.sending = true;
    s.problem = null;
    cx.notify();
    cx.spawn(async (cx) => {
      const r = await store.answer(cx, current.machine ?? null, current.dir, ["--start", heading]);
      s.sending = false;
      if (r?.result !== "queued") s.problem = oneLine(r?.message) || "The request could not be sent.";
      cx.notify();
    });
  }

  toggle(key) {
    if (this.opened.has(key)) this.opened.delete(key);
    else this.opened.add(key);
  }

  // --- pieces ----------------------------------------------------------------------

  header(t, current, open) {
    return h_flex().h(42).flex_none().pl(12).pr(10).gap(6).items_center().border_b(1).border_color(t.border)
      .child(div().flex_none().text_size(13).font_semibold().child("Questions"))
      .when(open > 0, (el) => el.child(this.count(t, open)))
      .child(div().flex_1())
      .when(Boolean(current), (el) => el.child(div().min_w(0).truncate().text_size(12).text_color(t.muted).child(current?.name ?? "")));
  }

  count(t, n) {
    return div().flex_none().px(5).rounded(4).text_size(10.5).font_semibold().bg(t.accent).text_color(t.accent_foreground).child(String(n));
  }

  section(t, label, n) {
    return h_flex().h(26).mt(4).px(2).gap(6).flex_none().items_center().text_size(11.5).text_color(t.dim)
      .child(div().font_semibold().text_color(t.muted).child(label))
      .child(div().child(String(n)))
      .child(div().flex_1().h(1).bg(t.border));
  }

  // What a question holds up.
  blocks(q, t) {
    const b = q.blocks;
    if (!b) return null;
    if (b.kind === "none") return div().flex_none().text_size(10.5).text_color(t.dim).child("no block");
    const label = b.kind === "merge" ? `blocks merge · ${b.what}` : `blocks ${b.what}`;
    return div().min_w(0).flex_shrink(1).truncate().px(5).rounded(4).border_1().border_color(t.blocked + "88")
      .text_size(10.5).text_color(t.blocked).child(oneLine(label));
  }

  // Where a question's answer is, as a line, or null.
  answerState(q, t) {
    const a = q.answer;
    if (!a) return null;
    if (a.state === "queued") return div().text_color(t.accent).child(`Queued ${said(a.answer)}: goes when the supervisor is idle`);
    return div().text_color(t.done ?? t.muted).child(`✓ Sent ${said(a.answer)} ${ago(a.at)}`.trim());
  }

  option(q, o, c, t) {
    const rec = q.recommended === o.key;
    const picked = c.choice?.key === o.key;
    return h_flex().id(`opt-${q.id}-${o.key}`).px(8).py(6).gap(8).items_start().rounded(7).border_1()
      .border_color(picked ? t.accent : rec ? t.accent + "66" : t.border)
      .when(picked, (el) => el.bg(t.accent + "22"))
      .hover((s) => s.bg(t.hover)).cursor_pointer()
      .on_mouse_down("left", stop)
      .on_click((_e, cx) => {
        c.choice = { key: o.key };
        c.problem = null;
        cx.notify();
      })
      .child(h_flex().size(18).flex_none().rounded(5).items_center().justify_center().text_size(11).font_bold()
        .when(picked, (el) => el.bg(t.accent).text_color(t.accent_foreground))
        .when(!picked, (el) => el.bg(t.muted + "26").text_color(t.foreground))
        .child(o.key))
      // The mark on a line of its own: beside the choice it squeezed the
      // choice into a narrow column in a narrow panel.
      .child(v_flex().flex_1().min_w(0).gap(1)
        .when(rec, (el) => el.child(h_flex().child(div().flex_none().px(4).rounded(3).text_size(9.5).font_semibold().bg(t.accent + "33").text_color(t.accent).child("RECOMMENDED"))))
        .child(div().min_w(0).whitespace_normal().text_size(12.5).font_medium().text_color(t.foreground).child(oneLine(o.choice)))
        .when(Boolean(o.cost), (el) => el.child(div().whitespace_normal().text_size(11.5).text_color(t.muted).child(oneLine(o.cost)))));
  }

  // The card's foot: the Other field, then where its answer is, and Send.
  foot(q, c, t, project) {
    const ready = Boolean(c.choice) && !c.sending;
    const send = div().id(`send-${q.id}`).flex_none().px(10).h(26).flex().items_center().rounded(6).text_size(12).font_semibold()
      .when(ready, (el) => el.bg(t.accent).text_color(t.accent_foreground).cursor_pointer().on_click((_e, cx) => this.send(cx, project, q.id)))
      .when(!ready, (el) => el.bg(t.muted + "26").text_color(t.dim))
      .on_mouse_down("left", stop)
      .child(c.choice?.key ? `Send ${c.choice.key}` : "Send");
    const state = c.sending
      ? h_flex().gap(6).items_center().child(spinner(t)).child(div().text_color(t.muted).child("Sending…"))
      : c.problem
        ? div().text_color(t.blocked).child(c.problem)
        : this.answerState(q, t);
    return v_flex().gap(6)
      .when(Boolean(c.input), (el) => el.child(Input.new(c.input).h(28).px(8).rounded(6).border_1().border_color(c.choice?.other ? t.accent : t.border)
        .bg(t.background).text_size(12).text_color(t.foreground)))
      .child(h_flex().gap(8).items_center().text_size(11.5)
        .child(div().flex_1().min_w(0).whitespace_normal().child(state ?? ""))
        .child(send));
  }

  // A question, open: everything, the options as buttons.
  open(q, t, project) {
    const c = this.card(project, q.id);
    const key = `${project}\u0000${q.id}`;
    const card = v_flex().id(`card-${q.id}`).p(10).gap(8).rounded(9).border_1().border_color(t.border).bg(t.raised ?? t.background)
      .child(h_flex().gap(6).items_start()
        .child(div().flex_none().pt(1).text_size(11).font_family("JetBrains Mono").text_color(t.dim).child(q.id))
        .child(div().flex_1().min_w(0).whitespace_normal().text_size(13).font_semibold().text_color(t.foreground).child(oneLine(q.title) || "Question"))
        .child(div().id(`fold-${q.id}`).flex_none().px(2).text_size(11).text_color(t.dim).cursor_pointer().tooltip("Close")
          .on_click((_e, cx) => { this.toggle(key); cx.notify(); }).child("▾")));
    if (!q.parsed) {
      return card
        .child(div().whitespace_normal().text_size(12).text_color(t.muted).child(q.raw ?? ""))
        .child(this.foot(q, c, t, project));
    }
    const blocks = this.blocks(q, t);
    const notes = [q.recommendation && `Recommends ${oneLine(q.recommendation)}`, q.until && `Until answered: ${oneLine(q.until)}`].filter(Boolean).join(" ");
    return card
      .when(Boolean(blocks), (el) => el.child(h_flex().gap(6).items_center().child(blocks)))
      .when(Boolean(q.context), (el) => el.child(div().whitespace_normal().text_size(12).text_color(t.muted).child(oneLine(q.context))))
      .child(v_flex().gap(5).children(q.options.map((o) => this.option(q, o, c, t))))
      .when(Boolean(notes), (el) => el.child(div().whitespace_normal().text_size(11.5).text_color(t.dim).child(notes)))
      .child(this.foot(q, c, t, project));
  }

  // A question, closed: its title, what it blocks, and where its answer is
  // (or the recommendation); a click opens it.
  closed(q, t, project) {
    const key = `${project}\u0000${q.id}`;
    const option = q.options?.find((o) => o.key === q.recommended);
    const blocks = this.blocks(q, t);
    const below = this.answerState(q, t)
      ?? div().truncate().text_color(t.muted).child(option ? `★ ${option.key}. ${oneLine(option.choice)}` : "Open it to answer");
    return v_flex().id(`closed-${q.id}`).px(10).py(8).gap(4).rounded(9).border_1().border_color(t.border).bg(t.raised ?? t.background)
      .cursor_pointer().hover((s) => s.bg(t.hover))
      .when(q.answer?.state === "sent", (el) => el.opacity(0.75))
      .on_click((_e, cx) => { this.toggle(key); cx.notify(); })
      .child(h_flex().gap(6).items_center()
        .child(div().flex_none().text_size(11).font_family("JetBrains Mono").text_color(t.dim).child(q.id))
        .child(div().flex_1().min_w(0).truncate().text_size(12.5).font_semibold().text_color(t.foreground).child(oneLine(q.title) || "Question"))
        .child(div().flex_none().text_size(11).text_color(t.dim).child("▸")))
      .when(Boolean(blocks), (el) => el.child(h_flex().gap(6).items_center().child(blocks)))
      .child(div().text_size(11.5).child(below));
  }

  backlogEntry(t, e, i) {
    const s = this.starting[e.heading];
    const lines = (e.body ?? "").split("\n").map((l) => l.trim()).filter(Boolean);
    const shown = lines.slice(0, BODY_LINES);
    const more = lines.length - shown.length;
    let action;
    if (s?.sending) action = spinner(t);
    else if (e.start?.state === "queued") action = div().flex_none().text_size(11).text_color(t.accent).child("Plan asked · queued");
    else if (e.start?.state === "sent") action = div().flex_none().text_size(11).text_color(t.done ?? t.muted).child("✓ Plan asked");
    else {
      action = div().id(`start-${i}`).flex_none().px(8).h(22).flex().items_center().rounded(6).border_1().border_color(t.border)
        .text_size(11.5).text_color(t.muted).cursor_pointer().hover((st) => st.bg(t.hover).text_color(t.foreground))
        .tooltip("Ask the supervisor to plan this and propose the plan to you; nothing is started")
        .on_mouse_down("left", stop).on_click((_e, cx) => this.start(cx, e.heading)).child("Start");
    }
    return v_flex().id(`backlog-${i}`).px(8).py(6).gap(2).rounded(7).hover((st) => st.bg(t.hover))
      .child(h_flex().gap(8).items_start()
        .child(div().flex_1().min_w(0).whitespace_normal().text_size(12.5).font_medium().text_color(t.foreground).child(oneLine(e.heading)))
        .child(action))
      .children(shown.map((l) => div().whitespace_normal().text_size(11.5).text_color(t.muted).child(l)))
      .when(more > 0, (el) => el.child(div().text_size(11).text_color(t.dim).child(`… ${more} more line${more === 1 ? "" : "s"}`)))
      .when(Boolean(s?.problem), (el) => el.child(div().whitespace_normal().text_size(11.5).text_color(t.blocked).child(s?.problem ?? "")));
  }

  note(t, title, text) {
    return v_flex().py(22).px(12).gap(4).items_center().text_size(12.5).text_color(t.dim)
      .child(div().text_color(t.muted).child(title))
      .when(Boolean(text), (el) => el.child(div().whitespace_normal().text_size(11.5).child(text)));
  }

  render() {
    this.version = store.version;
    const t = theme();
    const current = shared(CURRENT) ?? null;
    const shell = (parts) => v_flex().size_full().min_h(0).bg(t.background).rounded(10).border_1().border_color(t.border)
      .text_color(t.foreground).text_size(13).overflow_hidden().children(parts);
    if (!current) {
      return shell([
        this.header(t, null, 0),
        this.note(t, "No project", "Show a project's space, or click the count on a supervisor's row in the Projects list: its questions and backlog show here."),
      ]);
    }
    const project = JSON.stringify([current.machine ?? null, current.dir]);
    const read = store.panelOf(current.machine ?? null, current.dir);
    const questions = read?.questions?.questions ?? [];
    const entries = read?.backlog?.entries ?? [];
    const open = questions.filter((q) => !q.answer).length;
    const list = [];
    if (!read?.questions && !read?.error) list.push(this.note(t, "Reading…", ""));
    else if (!read.questions) list.push(this.note(t, "Corgi did not answer", read.error));
    else {
      list.push(this.section(t, "Questions", open));
      if (!questions.length) list.push(this.note(t, "No open questions", "The supervisor asks here when a decision is yours."));
      const allClosed = open > OPEN_CARDS;
      for (const q of questions) {
        const key = `${project}\u0000${q.id}`;
        // Closed unless clicked open: answered cards, and every card while many are open.
        const closedFirst = Boolean(q.answer) || allClosed;
        list.push(closedFirst !== this.opened.has(key) ? this.closed(q, t, project) : this.open(q, t, project));
      }
      list.push(this.section(t, "Backlog", entries.length));
      if (!entries.length) list.push(this.note(t, "The backlog is empty", ""));
      else entries.forEach((e, i) => list.push(this.backlogEntry(t, e, i)));
    }
    return shell([
      this.header(t, current, read?.questions ? open : current.open ?? 0),
      v_flex().flex_1().min_h(0).px(10).pt(4).pb(12).gap(8).overflow_y_scrollbar().children(list),
    ]);
  }
}
