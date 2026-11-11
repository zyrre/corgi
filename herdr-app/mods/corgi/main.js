// Corgi for the Herdr app (io.github.zyrre.corgi), one mod: each block of
// the layout that shows it shows the view its props name.
//
//   { "mod": "corgi", "props": { "view": "projects" } }   the Projects list
//   { "mod": "corgi", "props": { "view": "brand" } }      "Herdr + Corgi" at the sidebar's top
//
// A block with no view shows the Projects list. `corgi app-layout` puts
// both into the sidebar of the layout, in place of Herdr's Spaces and
// Agents lists and brand row, and `corgi app-layout remove` puts those back.
//
// Each view is a module of its own (projects.js, brand.js) with what they
// draw alike in ui.js. What Corgi says (`corgi status`, plan usage, the
// machines) is read in data.js once for every copy of the mod, however
// many blocks show it.
import { View, div } from "gpui-kit";
import { theme, props as blockProps } from "herdr";
import Brand from "./brand.js";
import Projects from "./projects.js";

const VIEWS = new Map([["projects", Projects], ["brand", Brand]]);

// A view this Corgi does not have (a layout made for a newer one): says so
// in its block rather than show another view there.
class Unknown {
  constructor(name) {
    this.name = String(name);
  }
  render() {
    const t = theme();
    return div().size_full().p(10).text_size(11.5).text_color(t.dim).whitespace_normal()
      .child(`This Corgi has no view ${this.name}: update Corgi.`);
  }
}

export default class Corgi extends View {
  init(_props, cx) {
    // The block's props: `props()`, which a layout block's copy is given
    // (init's own argument is not them).
    const props = blockProps() ?? {};
    const name = props.view ?? "projects";
    const Part = VIEWS.get(name);
    this.part = Part ? new Part() : new Unknown(name);
    this.part.init?.(props, cx);
  }
  render(cx) {
    return this.part.render(cx);
  }
}
