// The popups (toasts): what makes one, and what it says. Made from one read,
// the token user's issues updated lately with their history and comments
// (api.js `watch`): an issue assigned to the token's user, and a comment on
// or a status change of one of their issues, each by someone else. An issue
// can be muted. Pure, apart from what is handed in: the store's `Watch`
// decides, `toast` words it, and main.js posts it with the app's `notify`.
//
// What happened before the first read (after the app starts, or the
// settings change) makes none: that read sets the time from which on events
// count, and each event (a history entry, a comment) is posted once.

import { isMe, nameOf, timeOf } from "./util.js";
import { gist } from "./wiki.js";

export const MOD = "jira";
// The issue keys muted, kept by the app across restarts.
export const MUTED = "jira.muted";
// `setShared("jira.open", { key: "DEMO-12", at: Date.now() })` from another
// mod opens the panel on that issue, whether the panel shows or not (a mod
// command, `jira.open-DEMO-12`, reaches shown copies only).
export const OPEN = "jira.open";
// More than this many in one read make one toast for all of them.
export const AT_ONCE = 3;
// The JQL's window: the minutes since the last good read and this many
// more, never fewer than WINDOW_MIN; a day at most.
export const WINDOW_MIN = 10;
const WINDOW_MAX = 24 * 60;

// What the toasts know: since when events count, and what was posted.
export class Watch {
  constructor() { this.reset(); }

  reset() {
    this.since = 0; // when the first read was asked for: what is older, was so already
    this.last = 0; // when the last good read was asked for
    this.seen = new Set(); // "h<history id>", "c<comment id>": seen already
  }

  // How many minutes back the next read asks for.
  window(now = Date.now()) {
    if (!this.last) return WINDOW_MIN;
    return Math.min(WINDOW_MAX, Math.max(WINDOW_MIN, Math.ceil((now - this.last) / 60000) + 5));
  }

  // A read, asked for at `asked`, found these issues (each with `changelog`
  // and `fields.comment`): the events in them. `me` is the token's user.
  // [{ kind: "assigned" | "status" | "comment", key, summary, by, from, to, text, at }]
  read(issues, me, asked = Date.now()) {
    if (!this.since) this.since = asked;
    this.last = asked;
    const events = [];
    for (const issue of issues ?? []) {
      const key = issue?.key, summary = issue?.fields?.summary ?? "";
      if (!key) continue;
      for (const h of issue.changelog?.histories ?? []) {
        const id = `h${h?.id}`;
        if (this.seen.has(id)) continue;
        this.seen.add(id);
        const at = timeOf(h.created);
        if (at < this.since || isMe(h.author, me)) continue;
        for (const item of h.items ?? []) {
          if (item?.field === "assignee" && [me?.name, me?.key, me?.accountId].filter(Boolean).includes(item.to)) {
            events.push({ kind: "assigned", key, summary, by: nameOf(h.author), from: item.fromString ?? "", at });
          } else if (item?.field === "status") {
            events.push({ kind: "status", key, summary, by: nameOf(h.author), from: item.fromString ?? "", to: item.toString ?? "", at });
          }
        }
      }
      for (const c of issue.fields?.comment?.comments ?? []) {
        const id = `c${c?.id}`;
        if (this.seen.has(id)) continue;
        this.seen.add(id);
        const at = timeOf(c.created);
        if (at < this.since || isMe(c.author, me)) continue;
        events.push({ kind: "comment", key, summary, by: nameOf(c.author), text: gist(c.body), at });
      }
    }
    return events.sort((a, b) => a.at - b.at);
  }
}

export const muted = (event, keys) => keys.has(event.key);

// A toast's title, body and level.
export function toast(event) {
  const by = event.by || "Someone";
  if (event.kind === "assigned") {
    return { title: `${event.key} is yours now`, body: [event.summary, `${by} assigned it to you`].filter(Boolean).join(" · "), level: "info" };
  }
  if (event.kind === "status") {
    return { title: `${event.key} is ${event.to || "moved"}`, body: [event.summary, `${by} moved it${event.from ? ` from ${event.from}` : ""}`].filter(Boolean).join(" · "), level: "info" };
  }
  return { title: `${by} commented on ${event.key}`, body: event.text || event.summary, level: "info" };
}

// One toast for several events of one read.
export function summary(events) {
  const keys = [...new Set(events.map((e) => e.key))];
  return {
    title: `${events.length} updates on your Jira issues`,
    body: keys.join(", ").slice(0, 160),
    level: "info",
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
    if (!found && c && c.mod === MOD && c.props?.panel === true) found = { node, path };
  });
  return found;
}

function chainOf(layout) {
  const found = panelBlock(layout);
  return found ? [...found.path, found.node] : [];
}

// The stack above the panel that shows another of its children (`active`,
// else its first), with the child the panel is in, or null.
function unselected(chain) {
  for (let i = 0; i < chain.length - 1; i++) {
    const node = chain[i], next = chain[i + 1];
    if (node.type !== "stack" || !Array.isArray(node.children) || !node.children.includes(next)) continue;
    if ((node.active ?? node.children[0]?.id) !== next.id) return { stack: node, child: next };
  }
  return null;
}

// Whether each stack above the panel shows the child it is in, hidden or not.
export const panelSelected = (layout) => !unselected(chainOf(layout));

// Whether the panel shows: selected, and neither it nor a block above it
// hidden. True when the layout has no panel.
export const panelShows = (layout) => panelSelected(layout) && !chainOf(layout).some((node) => node.hidden);

// The one command that brings the panel nearest to showing, for a toast's
// click while it does not show (the click runs one): selecting it in its
// stack first, else showing the hidden block above it (Herdr's `panel`
// column). null when it shows, or there is no panel.
export function showStep(layout) {
  const chain = chainOf(layout), off = unselected(chain);
  if (off) return off.stack.id && off.child.id ? ["layout.select", { stack: off.stack.id, child: off.child.id }] : null;
  const hidden = chain.find((node) => node.hidden && node.id);
  return hidden ? ["layout.show", { id: hidden.id }] : null;
}

// The commands that show the panel: select it in its stack, then show every
// hidden block above it. [] when there is no panel.
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
