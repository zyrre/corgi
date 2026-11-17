// Shared drawing: the spinner, an issue's type mark, a status's pill, buttons.

import { div, svg } from "gpui-kit";
import { h_flex } from "gpui-base";

// This mod's folder name: its icons go by it.
export const MOD = "jira";
export const icon = (name) => `mods/${MOD}/icons/${name}.svg`;

// Braille frames, stepped natively and only while shown; never a timer, and
// not a turning svg, which the app turns too slowly to read as smooth.
const SPIN = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
export function spinner(t, size = 14) {
  return div().w(size).h(size).flex_none().flex().items_center().justify_center()
    .text_size(size + 4).line_height(`${size}px`).text_color(t.working).frames(SPIN, 90).child(SPIN[0]);
}

// A status category's color: at work, to do, done.
export function categoryColor(category, t) {
  return category === "indeterminate" ? t.ansi4 : category === "done" ? t.ansi2 : t.muted;
}

// An issue type's icon and color, by its name: a bug is red, a story green,
// an epic purple, a sub-task and anything else a task's blue.
export function typeLook(type, t) {
  const name = String(type?.name ?? "").toLowerCase();
  if (type?.subtask || /sub-?task/.test(name)) return { icon: "subtask", color: t.ansi4 };
  if (/bug|defect|incident/.test(name)) return { icon: "bug", color: t.ansi1 };
  if (/story/.test(name)) return { icon: "story", color: t.ansi2 };
  if (/epic/.test(name)) return { icon: "epic", color: t.ansi5 };
  return { icon: "task", color: t.ansi4 };
}

export function typeMark(type, t, size = 14) {
  const l = typeLook(type, t);
  return svg(icon(l.icon)).size(size).flex_none().text_color(l.color);
}

// A status as Jira shows it: its name in a small frame, colored by its category.
export function statusPill(status, t) {
  const color = categoryColor(status?.statusCategory?.key, t);
  return div().flex_none().h(18).px(6).flex().items_center().rounded(5).border_1().border_color(color)
    .text_size(10.5).font_semibold().text_color(color).child(String(status?.name ?? "?").toUpperCase());
}

const stop = (_e, cx) => cx.stop_propagation();

// A square icon button. `action(cx)` gets the click's context.
export function iconButton(id, name, tip, action, t, { size = 24 } = {}) {
  return div().id(id).size(size).flex_none().flex().items_center().justify_center().rounded(6).text_color(t.dim)
    .cursor_pointer().hover((s) => s.bg(t.hover).text_color(t.foreground)).tooltip(tip)
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
