// What the Corgi mod's views draw alike: the sidebar's width and its rail,
// one-line text, the spinner, and an agent's state as a badge.
import { div } from "gpui-kit";
import { run, window, shared, setShared, layout, home } from "herdr";

export function find(node, id) {
  if (node?.id === id) return node;
  for (const child of node?.children ?? []) {
    const found = find(child, id);
    if (found) return found;
  }
  return null;
}

// The sidebar's width now (Infinity in a layout without one).
export const sidebarPx = () => find(layout(), "sidebar")?.size?.px ?? Infinity;

// The sidebar's rail goes by its width, not a flag: dragging its edge narrows it too.
export const narrow = () => sidebarPx() < 160;

// To the rail (on macOS wide enough for the traffic lights), or back to the width it had.
export function toggleSidebar() {
  const width = find(layout(), "sidebar")?.size?.px;
  if (!narrow()) setShared("sidebar.width", width);
  // Mods written before the rail went by width still read the flag.
  setShared("sidebar.collapsed", !narrow());
  run("layout.resize", { id: "sidebar", px: narrow() ? (shared("sidebar.width") ?? 268) : window()?.platform === "macos" ? 76 : 44 });
}

// Text shown on one line: a `\n` would start a second one even when truncated.
export const oneLine = (text) => (text ?? "").replace(/\s+/g, " ").trim();

// A text with the paths under the home folder as `~/…`, to fit the sidebar.
export const tildePaths = (text) => {
  const h = home();
  return h ? text.split(`${h}/`).join("~/") : text;
};

// Something running (a pull, a merge, a close, a pause, a working space in
// the rail): braille frames, stepped natively while shown, as Herdr's peek
// mod draws its spinner. Not a turning svg: the app turns one at about 10 fps
// while the window is quiet and smoothly only while something else redraws
// it, so it reads as lagging; frames are meant to step. In a fixed box `size`
// square, so nothing beside it moves whatever width the font gives a glyph;
// the glyph is drawn larger than the box, since braille is small in the UI
// font.
const SPIN = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
export function spinner(t, size = 12) {
  return div().w(size).h(size).flex_none().flex().items_center().justify_center().overflow_hidden()
    .text_size(size + 6).line_height(`${size}px`).text_color(t.working).frames(SPIN, 90).child(SPIN[0]);
}

// An agent's state as a badge. A worker tagged ready to merge shows MERGE in place of its
// state (the mark that opens the merge question, `mergeMark`); a paused
// one PAUSED; one whose agent exited EXITED. An agent in no state shows none.
export const BADGES = { working: "WORKING", blocked: "BLOCKED", merge: "MERGE", done: "DONE", idle: "IDLE", paused: "PAUSED", exited: "EXITED" };
export function badgeKey(a) {
  if (a.status === "paused") return "paused";
  if (a.corgi?.tag === "merge") return "merge";
  if (BADGES[a.status]) return a.status;
  return a.corgi?.state === "exited" ? "exited" : null;
}
// Text on a solid fill: near-black on a light one, white on a dark one.
function onFill(c) {
  const m = /^#([0-9a-f]{6})/i.exec(c ?? "");
  if (!m) return "#ffffff";
  const n = parseInt(m[1], 16);
  const [r, g, b] = [n >> 16, (n >> 8) & 255, n & 255].map((v) => {
    v /= 255;
    return v <= 0.03928 ? v / 12.92 : ((v + 0.055) / 1.055) ** 2.4;
  });
  return 0.2126 * r + 0.7152 * g + 0.0722 * b > 0.3 ? "#16161a" : "#ffffff";
}
// The badge of state `key`, drawn as the MERGE mark is: a small pill, solid
// in the state's color (`text`, default its word). IDLE, the state at rest, is
// a quiet tint; EXITED an outline, so that neither shouts. No animation.
export function badge(t, key, text = BADGES[key]) {
  const pill = div().flex_none().px(5).rounded(4).text_size(10).font_semibold().child(text);
  if (key === "idle") return pill.bg(t.muted + "33").text_color(t.muted);
  if (key === "exited") return pill.border_1().border_color(t.blocked).text_color(t.blocked);
  const fill = { working: t.working, blocked: t.blocked, merge: t.accent, done: t.done ?? t.dim, paused: t.muted }[key] ?? t.dim;
  return pill.bg(fill).text_color(key === "merge" ? t.accent_foreground : onFill(fill));
}
