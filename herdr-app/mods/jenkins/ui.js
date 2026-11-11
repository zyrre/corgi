// Shared drawing: state marks, the spinner, the progress bar, buttons.

import { div, svg } from "gpui-kit";
import { h_flex } from "gpui-base";

// This mod's folder name: its icons go by it.
export const MOD = "jenkins";
export const icon = (name) => `mods/${MOD}/icons/${name}.svg`;

// Braille frames, stepped natively and only while shown; never a timer, and
// not a turning svg, which the app turns too slowly to read as smooth.
const SPIN = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
export function spinner(t, size = 14) {
  return div().w(size).h(size).flex_none().flex().items_center().justify_center()
    .text_size(size + 4).line_height(`${size}px`).text_color(t.working).frames(SPIN, 90).child(SPIN[0]);
}

// A state's icon and color. What went well is green, what failed red, what
// needs a look yellow; the rest is quiet.
export function look(state, t) {
  switch (state) {
    case "success": return { icon: "success", color: t.ansi2 };
    case "failed": return { icon: "failed", color: t.ansi1 };
    case "unstable": return { icon: "unstable", color: t.ansi3 };
    case "aborted": return { icon: "aborted", color: t.muted };
    case "queued": return { icon: "queued", color: t.ansi3 };
    case "running": return { icon: null, color: t.working };
    case "disabled": return { icon: "disabled", color: t.dim };
    case "paused": return { icon: "paused", color: t.ansi3 };
    case "skipped": return { icon: "skipped", color: t.dim };
    case "pending": return { icon: "notbuilt", color: t.dim };
    default: return { icon: "notbuilt", color: t.dim };
  }
}

// The mark before a job or a build: its state's icon, or the spinner while it runs.
export function mark(state, t, size = 14) {
  const l = look(state, t);
  return l.icon ? svg(icon(l.icon)).size(size).flex_none().text_color(l.color) : spinner(t, size);
}

// How far a running build is: a bar `width` px wide filled to `part` (0..1).
export function bar(part, t, width = 44) {
  return div().w(width).h(4).flex_none().rounded(2).bg(t.border)
    .child(div().h(4).rounded(2).bg(t.working).w(Math.max(2, Math.round(width * part))));
}

const stop = (_e, cx) => cx.stop_propagation();

// A square icon button. `action(cx)` gets the click's context.
export function iconButton(id, name, tip, action, t, { size = 24, danger = false } = {}) {
  return div().id(id).size(size).flex_none().flex().items_center().justify_center().rounded(6).text_color(t.dim)
    .cursor_pointer().hover((s) => s.bg(t.hover).text_color(danger ? t.ansi1 : t.foreground)).tooltip(tip)
    .on_mouse_down("left", stop)
    .on_click((_e, cx) => { cx.stop_propagation(); action(cx); })
    .child(svg(icon(name)).size(14));
}

// A labeled button: tone "solid" (the main action), "plain", or "danger".
export function pill(id, label, t, { icon: name, tone = "plain", disabled = false, busy = false, tip, onClick } = {}) {
  const solid = tone === "solid" && !disabled;
  const fg = disabled ? t.dim : solid ? t.solid_foreground : tone === "danger" ? t.ansi1 : t.foreground;
  return h_flex().id(id).h(26).px(10).gap(6).flex_none().items_center().rounded(7).text_size(12)
    .border_1().border_color(solid ? t.solid : tone === "danger" && !disabled ? t.ansi1 : t.border).bg(solid ? t.solid : t.background).text_color(fg)
    .when(!disabled && !busy, (el) => el.cursor_pointer().hover((s) => s.opacity(0.85))
      .on_mouse_down("left", stop)
      .on_click((_e, cx) => { cx.stop_propagation(); onClick?.(cx); }))
    .when(Boolean(tip), (el) => el.tooltip(tip))
    .when(busy, (el) => el.child(spinner(t, 12)))
    .when(!busy && Boolean(name), (el) => el.child(svg(icon(name)).size(12).flex_none()))
    .child(label);
}
