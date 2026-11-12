// Media in the Herdr app: images, GIFs and videos that coding agents put in
// front of the person by copying a file into a drop folder
// (~/.local/share/herdr-media by default; media/settings.json of the app's
// home names another). The newest is shown large, scaled to fit, with its
// caption (the first line of a `<same name>.txt` beside it), file name and
// time; earlier ones are listed below it, to click. It keeps the last 50.
// A new one posts a toast whose click shows the panel, and the tab bar's
// Media button counts what came since the panel was last seen.
//
// Copies, by their block's props (`props()`; init's argument is null):
// `{ panel: true }` is the panel in the layout's right-hand stack, a card
// as Files, Changes and GitLab are there; `{ watch: true }` draws nothing
// and is there so that the folder is watched while the panel has never
// been shown (the app starts a stack's other children only when shown),
// a floating block install.sh puts on the layout; any other copy (a tab, an
// overlay) is the panel's view filling its place. They share one store
// (store.js). Standalone: no other mod and no CLI is needed.

import { View, div, svg, image } from "gpui-kit";
import { v_flex, h_flex } from "gpui-base";
import { theme, nextCommand, layout, run, settings, props as given } from "herdr";
import { store } from "./store.js";
import { fit, ago, titleOf, oneLine, noteOf, showCommands } from "./items.js";

const MOD = "media";
const icon = (name) => `mods/${MOD}/icons/${name}.svg`;
const stop = (_e, cx) => cx.stop_propagation();

// A square icon button. `action(cx)` gets the click's context.
function iconButton(id, name, tip, action, t, { size = 24 } = {}) {
  return div().id(id).size(size).flex_none().flex().items_center().justify_center().rounded(6).text_color(t.dim)
    .cursor_pointer().hover((s) => s.bg(t.hover).text_color(t.foreground)).tooltip(tip)
    .on_mouse_down("left", stop)
    .on_click((_e, cx) => { cx.stop_propagation(); action(cx); })
    .child(svg(icon(name)).size(14));
}

// A labeled button; tone "solid" is the main action, "danger" deletes.
function pill(id, label, t, { name, tone = "plain", tip, onClick } = {}) {
  const solid = tone === "solid";
  const fg = solid ? t.solid_foreground : tone === "danger" ? t.ansi1 : t.foreground;
  return h_flex().id(id).h(26).px(10).gap(6).flex_none().items_center().rounded(7).text_size(12)
    .border_1().border_color(solid ? t.solid : tone === "danger" ? t.ansi1 : t.border).bg(solid ? t.solid : t.background).text_color(fg)
    .cursor_pointer().hover((s) => s.opacity(0.85))
    .on_mouse_down("left", stop)
    .on_click((_e, cx) => { cx.stop_propagation(); onClick?.(cx); })
    .when(Boolean(tip), (el) => el.tooltip(tip))
    .when(Boolean(name), (el) => el.child(svg(icon(name)).size(12).flex_none()))
    .child(label);
}

export default class Media extends View {
  init(initial, cx) {
    let p = null;
    try { p = given(); } catch { /* shown where it has no props */ }
    p = p ?? initial ?? {};
    this.panel = p.panel === true;
    this.watcher = p.watch === true;
    this.selected = null; // the id of the item shown large (none: the newest)
    this.newest = null; // the newest item's id when last drawn: a new one is shown large
    this.confirm = false; // "Clear all" being asked about
    this.size = null;
    this.drawn = -1; // the store's version last drawn
    this.watching = false;
    cx.timer.every(1000, (cx) => this.tick(cx));
    this.tick(cx);
    // `media.show` (the toast's click, or a key), `media.refresh`.
    cx.spawn(async (cx) => {
      for (;;) {
        let command;
        try { command = await nextCommand(); } catch { return; } // no more commands: the copy is going
        if (command === "show") this.show();
        else if (command === "refresh") { store.dirty = true; store.kick(cx); }
        cx.notify();
      }
    });
  }

  tick(cx) {
    store.tick(cx);
    if (store.loaded && store.folder && !this.watching) store.watch(cx, this);
    if (!this.watcher && store.version !== this.drawn) cx.notify();
  }

  // Shows the panel: selects it in its stack, and shows the column it is in.
  show() {
    let tree = null;
    try { tree = layout(); } catch { return; }
    for (const [command, args] of showCommands(tree)) run(command, args);
    this.selected = null;
  }

  // --- drawing ----------------------------------------------------------------------

  header(t) {
    const n = store.items.length;
    return h_flex().h(42).flex_none().pl(12).pr(8).gap(8).items_center().border_b(1).border_color(t.border)
      .child(svg(icon("media")).size(15).flex_none().text_color(t.muted))
      .child(div().flex_none().font_semibold().child("Media"))
      .when(n > 0, (el) => el.child(div().flex_none().text_size(11.5).text_color(t.dim).child(n === 1 ? "1 item" : `${n} items`)))
      .child(div().flex_1())
      .child(iconButton("media-folder", "folder", `Open the drop folder, ${store.folder ?? ""}`, (cx) => { cx.spawn(async () => { await store.openFolder(); }); }, t))
      .when(n > 0, (el) => el.child(iconButton("media-clear", "trash", "Clear all", (cx) => { this.confirm = true; cx.notify(); }, t)));
  }

  confirmCard(t) {
    const n = store.items.length;
    return v_flex().id("media-confirm").flex_none().mx(10).mt(8).p(10).gap(8).rounded(7).border_1().border_color(t.border).bg(t.surface).text_size(12)
      .child(div().child(`Remove ${n === 1 ? "the item" : `all ${n} items`}? The files they came from are deleted from the drop folder too.`))
      .child(h_flex().gap(6)
        .child(pill("media-clear-yes", "Clear all", t, { tone: "danger", name: "trash", onClick: (cx) => {
          this.confirm = false; this.selected = null;
          cx.spawn(async (cx) => { await store.clear(); cx.notify(); });
          cx.notify();
        } }))
        .child(pill("media-clear-no", "Cancel", t, { onClick: (cx) => { this.confirm = false; cx.notify(); } })));
  }

  empty(t) {
    return v_flex().id("media-empty").flex_1().items_center().justify_center().gap(10).px(24).text_size(12.5).text_color(t.muted)
      .child(svg(icon("media")).size(28).text_color(t.dim))
      .child(div().font_semibold().text_color(t.foreground).child("Nothing to show yet"))
      .child(div().text_center().child("Agents show you images, GIFs and videos here by copying them into the drop folder:"))
      .child(div().id("media-empty-folder").max_w_full().px(8).py(4).rounded(6).bg(t.surface).text_size(11.5).truncate()
        .tooltip(store.folder ?? "").child(oneLine(store.folder ?? "")))
      .child(pill("media-empty-open", "Open the folder", t, { name: "folder", onClick: (cx) => { cx.spawn(async () => { await store.openFolder(); }); } }));
  }

  // The picture of an item at w×h, or what stands in for it.
  picture(item, w, h, t, small = false) {
    if (item.file) return image(item.file).w(w).h(h).flex_none().rounded(small ? 3 : 4);
    const busy = item.state === "converting";
    return v_flex().w(w).h(h).flex_none().items_center().justify_center().gap(6).rounded(small ? 3 : 4).bg(t.surface).text_color(t.dim)
      .child(svg(icon(item.kind === "video" ? "video" : "media")).size(small ? 14 : 28))
      .when(!small, (el) => el.child(div().text_size(11.5).child(busy ? "Making a preview…" : item.problem ?? "No preview")));
  }

  big(item, t, now) {
    // on_size_change gives the size on screen, and w()/h() are scaled by the
    // app's zoom (seen at 1.1 in the preview): the panel's width in w() units,
    // less the margins (12 a side) and the frame's padding and border (7 a side).
    let zoom = 1;
    try { zoom = Number(settings()?.zoom) || 1; } catch { /* 1 */ }
    const width = Math.max(120, (this.size?.width ?? 360) / zoom - 38);
    const maxH = Math.max(160, Math.min(520, ((this.size?.height ?? 640) / zoom - 42) * 0.55));
    const box = item.file ? fit(item.width, item.height, width, maxH, { grow: item.format === "svg" }) : { w: width, h: Math.round(Math.min(maxH, width * 0.5)) };
    const note = noteOf(item);
    const newest = store.items[0]?.id === item.id;
    return v_flex().id("media-big").flex_none().px(12).pt(12).pb(4).gap(8)
      .child(h_flex().id("media-big-picture").w_full().justify_center().p(6).rounded(8).bg(t.surface).border_1().border_color(t.border)
        .cursor_pointer().tooltip("Open it in its own viewer")
        .on_click((_e, cx) => { cx.spawn(async () => { await store.open(item); }); })
        .child(this.picture(item, box.w, box.h, t)))
      .when(Boolean(item.caption), (el) => el.child(div().id("media-caption").text_size(13.5).text_color(t.foreground).child(oneLine(item.caption))))
      .child(h_flex().gap(6).items_center().text_size(11.5).text_color(t.dim)
        .when(newest, (el) => el.child(div().flex_none().px(5).rounded(4).bg(t.raised).text_color(t.muted).child("Newest")))
        .child(div().flex_1().min_w(0).truncate().tooltip(item.name).child(oneLine(item.name)))
        .child(div().flex_none().child(ago(item.at, now))))
      .when(Boolean(note), (el) => el.child(div().id("media-note").text_size(11.5).text_color(t.muted).child(note)))
      .child(h_flex().gap(6).pt(2)
        .child(pill("media-open", "Open", t, { name: "external", tone: note ? "solid" : "plain", tip: "Open it in its own viewer", onClick: (cx) => { cx.spawn(async () => { await store.open(item); }); } }))
        .child(pill("media-remove", "Remove", t, { name: "x", tip: "Take it out of the panel (the file stays in the drop folder)", onClick: (cx) => {
          if (this.selected === item.id) this.selected = null;
          cx.spawn(async (cx) => { await store.remove(item.id); cx.notify(); });
        } })));
  }

  row(item, t, now) {
    const thumb = item.file ? fit(item.width, item.height, 56, 40, { grow: true }) : { w: 56, h: 40 };
    return h_flex().id(`media-item-${item.id}`).flex_none().mx(6).px(6).py(4).gap(10).items_center().rounded(7)
      .cursor_pointer().hover((s) => s.bg(t.hover))
      .on_click((_e, cx) => { this.selected = item.id; cx.notify(); })
      .child(h_flex().w(56).h(40).flex_none().items_center().justify_center().rounded(4).bg(t.surface)
        .child(this.picture(item, thumb.w, thumb.h, t, true)))
      .child(v_flex().flex_1().min_w(0).gap(2)
        .child(div().truncate().text_size(12.5).child(oneLine(titleOf(item))))
        .child(div().truncate().text_size(11).text_color(t.dim).child(item.caption ? `${oneLine(item.name)} · ${ago(item.at, now)}` : ago(item.at, now))))
      .child(iconButton(`media-remove-${item.id}`, "x", "Remove", (cx) => {
        cx.spawn(async (cx) => { await store.remove(item.id); cx.notify(); });
      }, t, { size: 22 }));
  }

  render() {
    const t = theme();
    if (this.watcher) return div();
    this.drawn = store.version;
    const now = Date.now();
    const items = store.items;
    if (items[0]?.id !== this.newest) { this.newest = items[0]?.id ?? null; this.selected = null; }
    const shown = items.find((i) => i.id === this.selected) ?? items[0] ?? null;
    const rest = items.filter((i) => i !== shown);
    const body = !store.loaded ? [div().p(12).text_color(t.dim).child("Reading…")]
      : !shown ? [this.empty(t)]
        : [
          this.big(shown, t, now),
          ...(rest.length ? [div().flex_none().px(12).pt(10).pb(4).text_size(11).font_semibold().text_color(t.dim).child("EARLIER")] : []),
          ...rest.map((i) => this.row(i, t, now)),
          div().flex_none().h(8),
        ];
    return v_flex().id("media").size_full().min_h(0).overflow_hidden().bg(t.background)
      .when(this.panel, (el) => el.rounded(10).border_1().border_color(t.border)).text_color(t.foreground).text_size(13)
      .on_size_change((size, cx) => { this.size = size; cx.notify(); })
      .child(this.header(t))
      .when(Boolean(store.problem), (el) => el.child(div().id("media-problem").flex_none().mx(10).mt(8).p(8).rounded(7).border_1().border_color(t.blocked).text_size(12).text_color(t.blocked).child(store.problem)))
      .when(this.confirm, (el) => el.child(this.confirmCard(t)))
      .child(v_flex().id("media-body").flex_1().min_h(0).overflow_y_scrollbar().children(body));
  }
}
