# MOCKUP ONLY (questions round, 2026-11-11): puts the Corgi mod's mock
# `questions` view into the preview window's layout and a Questions button
# into its tab bar, in the preview's app home alone:
#
#   python3 -I scripts/questions-mock.py <preview home>
#
# The view goes into the right-hand panel stack `panel-body` as the block
# `corgi-questions` (beside Files and Changes, as the Jenkins panel does);
# the button goes into a copy of Herdr's `tabs` mod, `tabs-questions`,
# which the layout's `tabs` block is pointed at. The button's variant and
# count follow the mock's shared state (`corgi.mock.tab`, `corgi.mock.zero`).
# Never run it on the person's home.
import json, os, shutil, sys

home = sys.argv[1]
if "/corgi-ap-" not in os.path.realpath(home) and "app-preview" not in os.path.realpath(home):
    sys.exit("questions-mock: only for the preview's home")


def find(node, id):
    if node.get("id") == id:
        return node
    for c in node.get("children", []):
        f = find(c, id)
        if f:
            return f
    return None


path = os.path.join(home, "layouts", "corgi.json")
layout = json.load(open(path))
body = find(layout, "panel-body")
if not find(body, "corgi-questions"):
    body["children"].append({"id": "corgi-questions", "type": "block", "content": {"mod": "corgi", "props": {"view": "questions"}}})
find(layout, "tabs")["content"]["mod"] = "tabs-questions"

src, dst = os.path.join(home, "mods", "tabs"), os.path.join(home, "mods", "tabs-questions")
shutil.rmtree(dst, ignore_errors=True)
shutil.copytree(src, dst)
main = os.path.join(dst, "main.js")
s = open(main).read().replace('"mods/tabs/', '"mods/tabs-questions/').replace("`mods/tabs/", "`mods/tabs-questions/")
sync = "this.syncPanel(space, panel, body, enabled);"
anchor = ".when(w?.buttons,"
assert sync in s and anchor in s, "Herdr's tabs changed: patch by hand"
s = s.replace(sync, 'this.syncPanel(space, panel, body, [...enabled, "corgi-questions"]);')
BUTTON = r'''.child((() => {
        // MOCKUP ONLY: the Questions button (variant a: label and count,
        // b: icon with a count badge, c: a filled count chip).
        const v = shared("corgi.mock.tab") ?? "a", n = shared("corgi.mock.zero") ? 0 : 3;
        const on = panel && !panel.hidden && body?.active === "corgi-questions";
        const toggle = () => {
          if (on) run("layout.hide", { id: "panel" });
          else { run("layout.select", { stack: "panel-body", child: "corgi-questions" }); run("layout.show", { id: "panel" }); }
        };
        const icon = svg("mods/corgi/question.svg").size(14);
        const tip = n ? `${n} open questions from Corgi` : "No open questions from Corgi";
        if (v === "b" || (v === "c" && !n)) {
          return div().id("panel-corgi-questions").relative().size(26).flex_none().flex().items_center().justify_center().rounded(6)
            .bg(on ? t.selected : t.chrome).text_color(on || n ? t.foreground : t.dim).hover(s => s.bg(t.hover)).on_mouse_down("left", stop).on_click(toggle).tooltip(tip)
            .child(icon)
            .when(n > 0, el => el.child(div().absolute().top(0).right(0).min_w(13).h(13).px(3).rounded(7).bg(t.accent).text_color(t.accent_foreground)
              .text_size(9).font_bold().flex().items_center().justify_center().child(String(n))));
        }
        if (v === "c" && n > 0) {
          return h_flex().id("panel-corgi-questions").h(24).px(8).gap(5).flex_none().items_center().rounded(12)
            .bg(t.accent).text_color(t.accent_foreground).text_size(12).font_semibold().hover(s => s.opacity(0.9)).on_mouse_down("left", stop).on_click(toggle).tooltip(tip)
            .child(icon).child(`${n} questions`);
        }
        return h_flex().id("panel-corgi-questions").h(26).px(8).gap(5).flex_none().items_center().rounded(6)
          .text_size(12).bg(on ? t.selected : t.chrome).text_color(on ? t.foreground : t.muted)
          .hover(s => s.bg(t.hover)).on_mouse_down("left", stop).on_click(toggle).tooltip(tip)
          .child(icon).child("Questions")
          .when(n > 0, el => el.child(div().px(5).rounded(4).bg(t.accent).text_color(t.accent_foreground).text_size(10.5).font_semibold().child(String(n))));
      })())
      '''
s = s.replace(anchor, BUTTON + anchor, 1)
open(main, "w").write(s)
tmp = path + ".tmp"
json.dump(layout, open(tmp, "w"), indent=2)
os.replace(tmp, path)
print("questions-mock: the panel block and the tab bar's button are in", home)
