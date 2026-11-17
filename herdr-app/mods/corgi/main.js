// Corgi for the Herdr app (io.github.zyrre.corgi), one mod: each block of
// the layout that shows it shows the view its props name.
//
//   { "mod": "corgi", "props": { "view": "projects" } }   the Projects list
//   { "mod": "corgi", "props": { "view": "brand" } }      "Herdr + Corgi" at the sidebar's top
//   { "mod": "corgi", "props": { "view": "questions" } }  the Questions panel, in the right-hand panel stack
//   { "mod": "corgi", "props": { "view": "terminal" } }   the terminal panel's header, in that stack
//   { "mod": "corgi", "props": { "view": "terminal-hint" } }  its body while no shell shows
//
// A block with no view shows the Projects list. `corgi app-layout` puts
// the first two into the sidebar of the layout, in place of Herdr's Spaces
// and Agents lists and brand row, and the Questions and terminal panels
// into its `panel-body` stack with a button each in the tab bar; `corgi
// app-layout remove` puts Herdr's back and takes the panels out.
//
// Each view is a module of its own (projects.js, brand.js, questions.js,
// terminal.js, whose shells shells.js keeps) with what they
// draw alike in ui.js. What Corgi says (`corgi status`, plan usage, the
// machines) is read in data.js once for every copy of the mod, however
// many blocks show it.
import { View, div } from "gpui-kit";
import { theme, props as blockProps } from "herdr";
import Brand from "./brand.js";
import Projects from "./projects.js";
import Questions from "./questions.js";
import { TerminalHead, TerminalHint } from "./terminal.js";

const VIEWS = new Map([
  ["projects", Projects],
  ["brand", Brand],
  ["questions", Questions],
  ["terminal", TerminalHead],
  ["terminal-hint", TerminalHint],
]);

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
