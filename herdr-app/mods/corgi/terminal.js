// The terminal panel's views (shells.js says how its shells are kept):
//
// `terminal`, its header, over the shell in the right-hand panel stack:
//
//   [Worktree | Project root]              ⌖ Follows focus   To tab   ✕
//   ⎇ worktree-quiet-river-1a2b · w-export-columns · Rename the ex…  ⏻
//
// The switch picks the folder: the agent's own (a worker's worktree, a
// supervisor's project root) or its project's root, each a shell of its
// own, started only when asked (shells.js). It is the shown agent's choice,
// kept for its folder; another agent shows its own (Worktree until picked). Follows focus: the panel shows
// the focused agent's shell, or "No shell here" when it has none, and keeps
// the last one while a terminal that is no agent's (its own shell, the
// person's) has the focus; Pinned keeps the agent it shows. To tab moves
// the shell into a tab of the agent's space, for room. ⏻ at the end of the
// second line (End shell, by the folder it names; the first line has no
// room left at the column's narrowest) ends the shown shell, after asking
// in a card that takes the header's place (End, Cancel): whatever runs in
// it stops, so it always asks, also when the shell is at its prompt, which
// may have a job running in the background. ✕ hides the panel; every
// shell keeps running. Its timer is what follows, and only while the panel
// shows.
//
// `terminal-hint`, the body while no shell shows: why, with Open shell
// where the focused agent's folder has none (no shell starts by itself),
// and New shell where the person ended the last one or moved it to a tab.
import { div, svg } from "gpui-kit";
import { v_flex, h_flex } from "gpui-base";
import { theme, run, checking, agents } from "herdr";
import { oneLine, tildePaths, spinner } from "./ui.js";
import { store } from "./data.js";
import { panelState, follow, setKind, setFollow, moveToTab, newShell, endShell, local } from "./shells.js";

const ICON = "mods/workspace-chrome/";
// How often the header looks at the focus.
export const FOLLOW_MS = 500;
// Narrower than this, Follows focus and To tab show as icons alone.
const ROOMY = 340;
// What End shell asks.
export const END_ASK = "End this shell? Anything running in it stops.";

const base = (path) => (path ?? "").replace(/\/+$/, "").split("/").pop();

// The agent's name and task for the header: Corgi's, else its title.
function who(address) {
  const c = store.agents.get(address);
  if (c) return { name: c.name ?? "", task: c.role === "supervisor" ? "Supervisor" : oneLine(c.task) };
  const a = agents().find((x) => x.address === address);
  return { name: oneLine(a?.title), task: "" };
}

export class TerminalHead {
  init(_props, cx) {
    this.width = Infinity;
    // End shell asked about: { address, busy, problem }.
    this.confirm = null;
    if (checking()) return;
    cx.timer.every(FOLLOW_MS, (cx) => cx.spawn(async () => follow()));
  }

  // End shell: asks first, in the header's place.
  askEnd(address, cx) {
    this.confirm = { address, busy: false, problem: null };
    cx.notify();
  }

  // End, in the card: the shell ended and removed (shells.js `endShell`);
  // the card goes, or says why it did not end.
  end(cx) {
    const asked = this.confirm;
    if (!asked || asked.busy) return;
    asked.busy = true;
    cx.notify();
    cx.spawn(async (cx) => {
      const r = await endShell(asked.address, (ms) => cx.sleep(ms));
      this.confirm = r.ok ? null : { ...asked, busy: false, problem: r.problem };
      cx.notify();
      await follow();
    });
  }

  // The card End shell asks in, over the whole header.
  confirmCard(t, asked) {
    const button = (id, label, onClick, danger = false) => h_flex().id(id).h(24).px(10).gap(5).flex_none().items_center().rounded(6)
      .text_size(11.5).border_1().border_color(danger ? t.blocked : t.border).bg(t.background)
      .text_color(danger ? t.blocked : t.foreground)
      .when(!asked.busy, (el) => el.cursor_pointer().hover((s) => s.bg(t.hover)).on_click((_e, cx) => onClick(cx)))
      .when(danger && asked.busy, (el) => el.child(spinner(t, 11)))
      .when(danger && !asked.busy, (el) => el.child(svg("mods/corgi/power.svg").size(11)))
      .child(label);
    return h_flex().id("terminal-confirm").size_full().px(8).gap(8).items_center().rounded(7).border_1()
      .border_color(asked.problem ? t.blocked : t.border).bg(t.surface).text_size(11.5)
      .child(div().flex_1().min_w(0).whitespace_normal().text_color(asked.problem ? t.blocked : t.foreground)
        .child(asked.problem ?? END_ASK))
      .when(!asked.problem, (el) => el
        .child(button("terminal-end-no", "Cancel", (cx) => { this.confirm = null; cx.notify(); }))
        .child(button("terminal-end-yes", asked.busy ? "Ending…" : "End", (cx) => this.end(cx), true)))
      .when(Boolean(asked.problem), (el) => el
        .child(button("terminal-end-dismiss", "Dismiss", (cx) => { this.confirm = null; cx.notify(); })));
  }

  render() {
    const t = theme();
    const st = panelState();
    // Asked about the shell shown: dropped when the panel moved on to
    // another before End (it is ended, or said why not, once clicked).
    if (this.confirm && !this.confirm.busy && !this.confirm.problem && this.confirm.address !== st.address) this.confirm = null;
    const asked = this.confirm;
    if (asked) {
      return v_flex().id("terminal-head").size_full().pl(10).pr(10).py(5).text_color(t.foreground)
        .on_size_change((size, cx) => {
          if (size.width === this.width) return;
          this.width = size.width;
          cx.notify();
        })
        .child(this.confirmCard(t, asked));
    }
    const roomy = this.width >= ROOMY;
    const pinned = !st.follow;
    const seg = (kind, label) => div().id(`terminal-${kind}`).px(8).h(22).flex().items_center().rounded(5).cursor_pointer()
      .text_size(11.5).flex_none()
      .when(st.kind === kind, (el) => el.bg(t.background).text_color(t.foreground).shadow_sm())
      .when(st.kind !== kind, (el) => el.text_color(t.muted).hover((s) => s.text_color(t.foreground)))
      .on_click((_e, cx) => {
        setKind(kind);
        cx.spawn(async () => follow());
      })
      .child(label);
    const pill = (id, icon, label, tip, onClick, lit = false) => h_flex().id(id).h(22).px(roomy ? 7 : 4).gap(4).flex_none().items_center()
      .rounded(6).cursor_pointer().text_size(11).border_1().border_color(lit ? t.accent : t.border)
      .text_color(lit ? t.accent : t.muted).hover((s) => s.bg(t.hover).text_color(lit ? t.accent : t.foreground))
      .tooltip(tip).on_click(onClick)
      .child(svg(`${ICON}${icon}.svg`).size(12))
      .when(roomy, (el) => el.child(label));
    const { name, task } = st.agent ? who(st.agent) : { name: "", task: "" };
    const where = st.dir ? base(st.dir) : "";
    return v_flex().id("terminal-head").size_full().pl(12).pr(10).pt(5).gap(3).text_color(t.foreground)
      .on_size_change((size, cx) => {
        if (size.width === this.width) return;
        this.width = size.width;
        cx.notify();
      })
      .child(h_flex().h(26).gap(6).items_center()
        .child(h_flex().p(2).gap(1).flex_none().rounded(7).bg(t.hover).child(seg("worktree", "Worktree")).child(seg("root", "Project root")))
        .child(div().flex_1())
        .child(pill("terminal-pin", pinned ? "pin" : "pin-off", pinned ? "Pinned" : "Follows focus",
          pinned ? `Pinned to ${name || "this agent"}: click to follow the focused agent again` : "Follows the focused agent: click to keep this one",
          (_e, cx) => {
            setFollow(!st.follow);
            cx.spawn(async () => follow());
          }, pinned))
        .when(Boolean(st.address), (el) => el.child(pill("terminal-tab", "layers", "To tab", "Move this shell into a tab of the agent's space",
          (_e, cx) => cx.spawn(async () => moveToTab()))))
        .child(h_flex().id("terminal-hide").size(22).flex_none().items_center().justify_center().rounded(6).cursor_pointer()
          .text_color(t.muted).hover((s) => s.bg(t.hover).text_color(t.foreground))
          .tooltip("Hide the panel (its shells keep running)")
          .on_click(() => run("layout.hide", { id: "panel" }))
          .child(svg(`${ICON}x.svg`).size(13))))
      .child(h_flex().h(18).gap(4).items_center().text_size(11.5).text_color(t.dim)
        // What it shows, cut at the right where ⏻ needs the room.
        .child(h_flex().flex_1().min_w(0).overflow_hidden().gap(5).items_center()
          .when(Boolean(where), (el) => el
            .child(svg(`${ICON}${st.kind === "root" ? "folder" : "git-branch"}.svg`).size(11).flex_none())
            .child(div().id("terminal-dir").flex_none().text_color(t.muted).tooltip(tildePaths(st.dir)).child(where)))
          .when(Boolean(where && name), (el) => el.child(div().flex_none().child("·")))
          .when(Boolean(name), (el) => el.child(div().flex_none().child(name)))
          .when(Boolean(task && task !== name), (el) => el.child(div().flex_none().child("·")).child(div().flex_shrink(1).min_w(0).truncate().child(task)))
          .when(!where && !name, (el) => el.child(div().child("No agent yet"))))
        .when(local(st.address ?? ""), (el) => el.child(h_flex().id("terminal-end").size(18).flex_none()
          .items_center().justify_center().rounded(5).cursor_pointer().text_color(t.muted)
          .hover((s) => s.bg(t.hover).text_color(t.blocked))
          .tooltip("End this shell (asks first)")
          .on_click((_e, cx) => this.askEnd(st.address, cx))
          .child(svg("mods/corgi/power.svg").size(12)))));
  }
}

// What the body says while no shell shows.
function said(st) {
  switch (st.note) {
    case "starting":
      return `Starting a shell in ${tildePaths(st.dir ?? "")}…`;
    case "ended":
      return `The shell in ${base(st.dir)} ended.`;
    case "moved":
      return `The shell in ${base(st.dir)} moved to a tab.`;
    case "remote":
      return "Its agent runs on another machine: the panel opens shells on this computer's agents.";
    case "problem":
      return st.problem || "The shell did not start.";
    default:
      return st.dir ? "No shell here" : "Focus an agent to open a shell in its folder here.";
  }
}

export class TerminalHint {
  render() {
    const t = theme();
    const st = panelState();
    const here = local(st.agent ?? "") && Boolean(st.dir);
    // Open shell where there is none yet, New shell where the last one went.
    const button = !here ? null
      : st.note === "ended" || st.note === "moved" ? { id: "terminal-new", label: "New shell" }
        : st.note === "none" || st.note === "problem" ? { id: "terminal-open", label: "Open shell" }
          : null;
    return v_flex().size_full().items_center().justify_center().gap(10).px(16).text_size(12).text_color(t.dim)
      .child(div().whitespace_normal().text_center().text_color(st.note === "problem" ? t.blocked : t.dim).child(said(st)))
      .when(Boolean(button), (el) => el.child(h_flex().id(button.id).h(24).px(10).gap(5).items_center().rounded(6).cursor_pointer()
        .border_1().border_color(t.border).text_color(t.foreground).hover((s) => s.bg(t.hover))
        .tooltip(`Start a shell in ${tildePaths(st.dir ?? "")}`)
        .on_click((_e, cx) => {
          newShell();
          cx.spawn(async () => follow());
        })
        .child(svg("mods/corgi/terminal.svg").size(12)).child(button.label)));
  }
}
