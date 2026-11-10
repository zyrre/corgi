// Corgi's brand row for the Herdr app (io.github.zyrre.corgi): the top of
// the sidebar reads "Herdr + Corgi", each with its logo, so the window says
// it is Herdr with Corgi. `corgi app-layout` puts it in the layout's
// `brand` block, where Herdr's `workspace-chrome` drew "Herdr" alone, and
// `corgi app-layout remove` puts that back.
//
// It began as a copy of the brand row of Herdr's shipped `workspace-chrome`
// mod and follows no later version of it. Everything but the names is that
// row as it was: the search and sidebar buttons, the room for the macOS
// traffic lights and the collapsed rail, which shows the buttons alone.
// The sidebar's footer stays `workspace-chrome`'s.
import { View, div, image, svg } from "gpui-kit";
import { h_flex } from "gpui-base";
import { theme, run, window, shared, setShared, layout } from "herdr";

function find(node, id) {
  if (node?.id === id) return node;
  for (const child of node?.children ?? []) {
    const found = find(child, id);
    if (found) return found;
  }
  return null;
}

// The sidebar's rail goes by its width, not a flag: dragging its edge narrows it too.
const narrow = () => (find(layout(), "sidebar")?.size?.px ?? Infinity) < 160;

// To the rail (on macOS wide enough for the traffic lights), or back to the width it had.
function toggleSidebar() {
  const width = find(layout(), "sidebar")?.size?.px;
  if (!narrow()) setShared("sidebar.width", width);
  // Mods edited before the rail went by width still read the flag.
  setShared("sidebar.collapsed", !narrow());
  run("layout.resize", { id: "sidebar", px: narrow() ? (shared("sidebar.width") ?? 268) : window()?.platform === "macos" ? 76 : 44 });
}

// How much faster "Corgi" gives up its width than "Herdr" when the row is
// too narrow for both words: "Corgi" is cut to "…" before "Herdr" is cut
// at all. The logos and the "+" are never cut.
const CUT_CORGI_FIRST = 10000;
// What the row takes right of the names: its right padding, the two
// buttons and the gaps before them.
const BUTTONS = 10 + 26 + 26 + 4 + 4;
// The least room for both logos and the "+" whole, with both words gone:
// 20 + 20 for the logos, about 7 for the "+", 4 gaps of 5, and a little
// air. A sidebar with less (on macOS beside the traffic lights, under
// about 220 px) shows Herdr's logo alone, rather than logos cut or under
// the buttons.
const BOTH_LOGOS = 72;

export default class CorgiBrand extends View {
  button(id, label, action) {
    const t = theme();
    return div().id(id).size(26).flex_none().flex().items_center().justify_center().rounded(6).text_color(t.muted).hover(s => s.bg(t.hover).text_color(t.foreground))
      .on_mouse_down("left", (_e, cx) => cx.stop_propagation()).on_click(action).child(label);
  }
  brand(room) {
    const t = theme();
    const logo = (path) => image(path).size(20).flex_none().rounded(5);
    const name = (id, text, shrink) => div().id(id).min_w(0).flex_shrink(shrink).truncate().font_semibold().text_color(t.foreground).child(text);
    const row = h_flex().id("brand-names").flex_1().min_w(0).overflow_hidden().gap(5).items_center()
      .child(logo("frame/welcome/herdr.png"));
    if (room < BOTH_LOGOS) return row;
    return row
      .child(name("brand-herdr", "Herdr", 1))
      .child(div().id("brand-plus").flex_none().text_color(t.dim).child("+"))
      .child(logo("mods/corgi-brand/corgi.png"))
      .child(name("brand-corgi", "Corgi", CUT_CORGI_FIRST));
  }
  render() {
    const t = theme();
    const collapsed = narrow();
    const left = window()?.platform === "macos" && !window()?.fullscreen ? 76 : 14;
    const room = (find(layout(), "sidebar")?.size?.px ?? Infinity) - left - BUTTONS;
    return h_flex().size_full().bg(t.chrome).pl(collapsed ? 6 : 14).pr(10).gap(4).items_center().text_size(13.5)
      .when(window()?.platform === "macos" && !window()?.fullscreen && !collapsed, el => el.pl(76))
      .when(!collapsed, el => el.child(this.brand(room)))
      .when(!collapsed, el => el.child(this.button("search", svg("mods/corgi-brand/search.svg").size(15), () => run("mod.open", { name: "picker" }))))
      // On the macOS rail the traffic lights fill the row; the sidebar's list unfolds it.
      .when(!(collapsed && window()?.platform === "macos"), el => el.child(this.button("sidebar-toggle", svg("mods/corgi-brand/sidebar.svg").size(15), toggleSidebar)));
  }
}
