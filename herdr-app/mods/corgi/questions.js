// MOCKUP ONLY (questions and backlog round, 2026-11-11): the Corgi mod's
// `questions` view, in the layout's right-hand panel stack `panel-body`
// beside Files and Changes, drawn from canned data (mock.js). Three
// variants of the panel, chosen by `herdr-app do corgi.mock-panel-<a|b|c>`:
//
//   a  Questions and Backlog as two tabs at the top; every project's
//      questions, grouped by project, each card open with its options.
//   b  one scroll: the questions, grouped by project, as cards that are
//      closed (title, what it blocks, the recommendation) but for the one
//      opened, then a Backlog section, folded.
//   c  one project at a time, picked from an inline list of the projects
//      at the top (the one whose count was clicked): its questions, open,
//      then its backlog with a Start button per entry.
//
// Nothing is sent: a picked answer shows as it would wait to be sent, and
// Q9 as one sent.
import { div } from "gpui-kit";
import { v_flex, h_flex } from "gpui-base";
import { theme, setShared, spaces } from "herdr";
import { oneLine } from "./ui.js";
import { QUESTIONS, BACKLOG, PROJECTS, open, mock } from "./mock.js";

const stop = (_e, cx) => cx.stop_propagation();

export default class Questions {
  init(_props, cx) {
    cx.timer.every(1000, (cx) => cx.notify());
  }

  // --- pieces ----------------------------------------------------------------

  header(t, parts) {
    return h_flex().h(42).flex_none().pl(12).pr(8).gap(6).items_center().border_b(1).border_color(t.border).children(parts.filter(Boolean));
  }

  count(t, n, strong = true) {
    return div().flex_none().px(5).rounded(4).text_size(10.5).font_semibold()
      .when(strong && n > 0, (el) => el.bg(t.accent).text_color(t.accent_foreground))
      .when(!(strong && n > 0), (el) => el.bg(t.muted + "26").text_color(t.muted))
      .child(String(n));
  }

  // A segmented pair of tabs (variant a).
  tabs(t, page) {
    const tab = (id, label, n) => h_flex().id(`qtab-${id}`).h(26).px(9).gap(6).items_center().rounded(6).text_size(12.5)
      .when(page === id, (el) => el.bg(t.selected).text_color(t.foreground).font_medium())
      .when(page !== id, (el) => el.text_color(t.muted).hover((s) => s.bg(t.hover)))
      .on_click(() => setShared("corgi.mock.page", id))
      .child(label).child(this.count(t, n, id === "questions"));
    const backlog = PROJECTS.reduce((n, p) => n + BACKLOG[p].length, 0);
    return h_flex().gap(2).items_center().child(tab("questions", "Questions", open().length)).child(tab("backlog", "Backlog", backlog));
  }

  // What a question holds up: a merge, a plan, or nothing.
  blocks(q, t) {
    if (!q.blocks) return div().flex_none().text_size(10.5).text_color(t.dim).child("no block");
    const label = q.blocks.kind === "merge" ? `blocks merge · ${q.blocks.what}` : `blocks plan · ${q.blocks.what}`;
    return div().min_w(0).flex_shrink(1).truncate().px(5).rounded(4).border_1().border_color(t.blocked + "88").text_size(10.5).text_color(t.blocked).child(label);
  }

  projectHeading(t, name, n, entries = false) {
    return h_flex().h(26).mt(6).px(2).gap(6).items_center().text_size(11.5).text_color(t.dim)
      .child(div().font_semibold().text_color(t.muted).child(name))
      .child(div().child(entries ? `${n} ${n === 1 ? "entry" : "entries"}` : `${n} open`))
      .child(div().flex_1().h(1).bg(t.border));
  }

  // One option as a button: its letter, the choice, what it costs. The
  // recommended one is marked; the picked one filled; a sent card's are dim.
  option(q, o, t) {
    const rec = q.recommendation.key === o.key;
    const picked = q.answer?.pick === o.key;
    const sent = q.answer?.phase === "sent";
    return h_flex().id(`opt-${q.id}-${o.key}`).px(8).py(6).gap(8).items_start().rounded(7).border_1()
      .border_color(picked ? t.accent : rec ? t.accent + "66" : t.border)
      .when(picked && !sent, (el) => el.bg(t.accent + "22"))
      .when(sent && !picked, (el) => el.opacity(0.45))
      .when(!sent, (el) => el.hover((s) => s.bg(t.hover)).cursor_pointer())
      .on_mouse_down("left", stop)
      .child(h_flex().size(18).flex_none().rounded(5).items_center().justify_center().text_size(11).font_bold()
        .when(picked, (el) => el.bg(t.accent).text_color(t.accent_foreground))
        .when(!picked, (el) => el.bg(t.muted + "26").text_color(t.foreground))
        .child(o.key))
      .child(v_flex().flex_1().min_w(0).gap(1)
        .child(h_flex().gap(6).items_center()
          .child(div().min_w(0).flex_shrink(1).whitespace_normal().text_size(12.5).font_medium().text_color(t.foreground).child(o.choice))
          .when(rec, (el) => el.child(div().flex_none().px(4).rounded(3).text_size(9.5).font_semibold().bg(t.accent + "33").text_color(t.accent).child("RECOMMENDED"))))
        .child(div().whitespace_normal().text_size(11.5).text_color(t.muted).child(o.cost)));
  }

  // The answer's state at the card's foot: none yet, picked (waits for
  // Send), or sent (waits for the supervisor to take it).
  answerLine(q, t) {
    const a = q.answer;
    if (a?.phase === "sent") {
      return div().whitespace_normal().text_size(11.5).text_color(t.dim)
        .child(div().text_color(t.done ?? t.muted).child(`✓ Sent "${q.id}: ${a.pick}" to ${q.project}'s supervisor ${a.at}.`))
        .child(div().child("The card goes once the supervisor has taken it."));
    }
    const other = div().id(`other-${q.id}`).px(8).h(24).flex().items_center().rounded(6).border_1().border_color(t.border).text_size(12).text_color(t.muted)
      .hover((s) => s.bg(t.hover)).child("Other answer…");
    return h_flex().gap(6).items_center().text_size(11.5)
      .child(other)
      .child(div().flex_1())
      .when(a?.phase === "picked", (el) => el.child(div().text_color(t.accent).child(`${a.pick} picked · not sent`)));
  }

  // A question, open: everything, the options as buttons.
  card(q, t, { project = false } = {}) {
    return v_flex().id(`card-${q.id}`).p(10).gap(8).rounded(9).border_1().border_color(t.border).bg(t.raised ?? t.background)
      .child(h_flex().gap(6).items_start()
        .child(div().flex_none().pt(1).text_size(11).font_family("JetBrains Mono").text_color(t.dim).child(q.id))
        .child(div().flex_1().min_w(0).whitespace_normal().text_size(13).font_semibold().text_color(t.foreground).child(q.title))
        .when(project, (el) => el.child(div().flex_none().text_size(11).text_color(t.dim).child(q.project))))
      .child(h_flex().gap(6).items_center().child(this.blocks(q, t)))
      .child(div().whitespace_normal().text_size(12).text_color(t.muted).child(q.context))
      .child(v_flex().gap(5).children(q.options.map((o) => this.option(q, o, t))))
      .child(div().whitespace_normal().text_size(11.5).text_color(t.dim).child(`Recommends ${q.recommendation.key}: ${q.recommendation.why}. Until answered: ${oneLine(q.until)}`))
      .child(this.answerLine(q, t));
  }

  // A question, closed: its title, what it blocks and the recommendation;
  // a click opens it (variant b).
  closed(q, t) {
    const a = q.answer;
    const state = a?.phase === "sent" ? `✓ sent ${a.pick}` : a?.phase === "picked" ? `${a.pick} picked` : null;
    return v_flex().id(`closed-${q.id}`).px(10).py(8).gap(4).rounded(9).border_1().border_color(t.border).bg(t.raised ?? t.background)
      .cursor_pointer().hover((s) => s.bg(t.hover))
      .when(a?.phase === "sent", (el) => el.opacity(0.6))
      .on_click(() => setShared("corgi.mock.opened", q.id))
      .child(h_flex().gap(6).items_center()
        .child(div().flex_none().text_size(11).font_family("JetBrains Mono").text_color(t.dim).child(q.id))
        .child(div().flex_1().min_w(0).truncate().text_size(12.5).font_semibold().text_color(t.foreground).child(q.title))
        .child(div().flex_none().text_size(11).text_color(t.dim).child("▸")))
      .child(h_flex().gap(6).items_center()
        .child(this.blocks(q, t))
        .child(div().flex_1())
        .when(Boolean(state), (el) => el.child(div().flex_none().text_size(11).text_color(a?.phase === "sent" ? t.done ?? t.muted : t.accent).child(state))))
      .child(div().truncate().text_size(11.5).text_color(t.muted).child(`★ ${q.recommendation.key}. ${q.options.find((o) => o.key === q.recommendation.key).choice}`));
  }

  // The bar at the foot while answers are picked and not sent: one Send per
  // supervisor, which types them as one message.
  sendBar(t, project) {
    const picked = QUESTIONS.filter((q) => q.answer?.phase === "picked" && (!project || q.project === project) && !mock().zero);
    if (!picked.length) return null;
    const by = [...new Set(picked.map((q) => q.project))];
    return v_flex().flex_none().px(12).py(8).gap(6).border_t(1).border_color(t.border)
      .children(by.map((p) => {
        const mine = picked.filter((q) => q.project === p);
        return h_flex().gap(8).items_center()
          .child(v_flex().flex_1().min_w(0)
            .child(div().truncate().text_size(12).text_color(t.foreground).child(`${mine.length} answer${mine.length > 1 ? "s" : ""} for ${p}'s supervisor`))
            .child(div().truncate().text_size(11).font_family("JetBrains Mono").text_color(t.dim).child(mine.map((q) => `${q.id}: ${q.answer.pick}`).join("  "))))
          .child(div().id(`send-${p}`).flex_none().px(10).h(26).flex().items_center().rounded(6).bg(t.accent).text_color(t.accent_foreground).text_size(12).font_semibold().child("Send"));
      }));
  }

  empty(t, what) {
    return v_flex().py(28).gap(4).items_center().text_size(12.5).text_color(t.dim)
      .child(div().text_color(t.muted).child(what))
      .child(div().text_size(11.5).child("Corgi asks here when a decision is yours."));
  }

  backlogEntry(t, p, e, i, start) {
    return h_flex().id(`bl-${p}-${i}`).px(8).py(6).gap(8).items_start().rounded(7).hover((s) => s.bg(t.hover))
      .child(v_flex().flex_1().min_w(0).gap(2)
        .child(div().whitespace_normal().text_size(12.5).font_medium().text_color(t.foreground).child(e.heading))
        .children(e.bullets.map((b) => div().whitespace_normal().text_size(11.5).text_color(t.muted).child(`• ${b}`))))
      .when(start, (el) => el.child(div().id(`start-${p}-${i}`).flex_none().px(8).h(22).flex().items_center().rounded(6).border_1().border_color(t.border).text_size(11.5).text_color(t.muted)
        .hover((s) => s.bg(t.hover).text_color(t.foreground)).child("Start")));
  }

  // --- the variants -------------------------------------------------------------

  // a: two tabs; every project's questions, grouped, open.
  tabsVariant(t, m) {
    const body = v_flex().flex_1().min_h(0).px(10).pb(12).gap(8).overflow_y_scrollbar();
    let list;
    if (m.page === "backlog") {
      list = PROJECTS.flatMap((p) => [
        this.projectHeading(t, p, BACKLOG[p].length, true),
        ...BACKLOG[p].map((e, i) => this.backlogEntry(t, p, e, i, true)),
      ]);
    } else if (!open().length) {
      list = [this.empty(t, "No open questions")];
    } else {
      list = PROJECTS.flatMap((p) => {
        const qs = QUESTIONS.filter((q) => q.project === p);
        return qs.length ? [this.projectHeading(t, p, open(p).length), ...qs.map((q) => this.card(q, t))] : [];
      });
    }
    return [this.header(t, [this.tabs(t, m.page), div().flex_1()]), body.children(list), m.page === "questions" ? this.sendBar(t) : null];
  }

  // b: one scroll, cards closed but one, the backlog folded at the end.
  scrollVariant(t, m) {
    const qs = m.zero ? [] : QUESTIONS;
    const list = qs.length
      ? PROJECTS.flatMap((p) => {
        const mine = qs.filter((q) => q.project === p);
        return mine.length ? [this.projectHeading(t, p, open(p).length), ...mine.map((q) => (q.id === m.opened ? this.card(q, t) : this.closed(q, t)))] : [];
      })
      : [this.empty(t, "No open questions")];
    const total = PROJECTS.reduce((n, p) => n + BACKLOG[p].length, 0);
    const fold = h_flex().id("backlog-fold").h(30).mt(10).px(4).gap(6).items_center().rounded(6).cursor_pointer().hover((s) => s.bg(t.hover))
      .on_click(() => setShared("corgi.mock.backlog", !m.backlogOpen))
      .child(div().w(10).text_size(11).text_color(t.dim).child(m.backlogOpen ? "▾" : "▸"))
      .child(div().text_size(12.5).font_semibold().text_color(t.foreground).child("Backlog"))
      .child(this.count(t, total, false))
      .child(div().flex_1())
      .child(div().text_size(11).text_color(t.dim).child(PROJECTS.map((p) => `${p} ${BACKLOG[p].length}`).join(" · ")));
    const backlog = m.backlogOpen ? PROJECTS.flatMap((p) => [this.projectHeading(t, p, BACKLOG[p].length, true), ...BACKLOG[p].map((e, i) => this.backlogEntry(t, p, e, i, false))]) : [];
    return [
      this.header(t, [div().text_size(13).font_semibold().child("Questions"), this.count(t, open().length), div().flex_1(),
        div().text_size(11).text_color(t.dim).child("click a card to open it")]),
      v_flex().flex_1().min_h(0).px(10).pb(12).gap(6).overflow_y_scrollbar().children(list).child(fold).children(backlog),
      this.sendBar(t),
    ];
  }

  // The project shown in c: the one whose count was clicked, else the
  // focused space's, else the first with questions.
  shownProject(m) {
    if (m.project) return m.project;
    const name = spaces().find((s) => s.active)?.name;
    return PROJECTS.includes(name) ? name : PROJECTS.find((p) => open(p).length) ?? PROJECTS[0];
  }

  // c: one project at a time, picked from an inline list.
  projectVariant(t, m) {
    const p = this.shownProject(m);
    const pick = (name) => h_flex().id(`qproj-${name}`).h(26).px(8).gap(6).items_center().rounded(6).text_size(12.5)
      .when(name === p, (el) => el.bg(t.selected).text_color(t.foreground).font_medium())
      .when(name !== p, (el) => el.text_color(t.muted).hover((s) => s.bg(t.hover)))
      .on_click(() => setShared("corgi.questions.project", name))
      .child(name).child(this.count(t, open(name).length));
    const qs = m.zero ? [] : QUESTIONS.filter((q) => q.project === p);
    const section = (label, n) => h_flex().h(26).mt(6).px(2).gap(6).items_center().text_size(11.5).text_color(t.dim)
      .child(div().font_semibold().text_color(t.muted).child(label)).child(div().child(String(n))).child(div().flex_1().h(1).bg(t.border));
    return [
      this.header(t, [h_flex().gap(2).children(PROJECTS.map(pick)), div().flex_1()]),
      v_flex().flex_1().min_h(0).px(10).pb(12).gap(8).overflow_y_scrollbar()
        .child(section("Questions", open(p).length))
        .children(qs.length ? qs.map((q) => this.card(q, t)) : [this.empty(t, `No open questions in ${p}`)])
        .child(section("Backlog", BACKLOG[p].length))
        .children(BACKLOG[p].map((e, i) => this.backlogEntry(t, p, e, i, true))),
      this.sendBar(t, p),
    ];
  }

  render() {
    const t = theme();
    const m = mock();
    const parts = m.panel === "b" ? this.scrollVariant(t, m) : m.panel === "c" ? this.projectVariant(t, m) : this.tabsVariant(t, m);
    return v_flex().size_full().min_h(0).bg(t.background).rounded(10).border_1().border_color(t.border).text_color(t.foreground).text_size(13).overflow_hidden()
      .children(parts.filter(Boolean));
  }
}
