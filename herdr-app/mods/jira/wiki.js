// Jira's wiki markup (Jira Server and Data Center keep descriptions and
// comments in it) made readable: `blocks(text)` gives what main.js draws, a
// list of blocks (headings, paragraphs' lines, list items, code, quotes and
// panels, table rows, rules), with the inline markup taken out by `inline`:
// `*bold*` reads as bold without its stars, `[a link|https://…]` as "a link",
// `[~jane]` as "@jane", `{{mono}}` as "mono", `!shot.png!` as "[image: shot.png]".
// Pure: no Herdr, no GPUI.

const EMOTICONS = [
  ["(/)", "✓"], ["(x)", "✗"], ["(!)", "⚠"], ["(i)", "ℹ"], ["(?)", "?"], ["(on)", "●"], ["(off)", "○"],
  ["(*)", "★"], ["(y)", "👍"], ["(n)", "👎"], ["(+)", "+"], ["(-)", "−"],
];

// Emphasis that a word boundary opens and closes: `*bold*`, `_italic_`,
// `+underline+`, `-strike-`, `^sup^`, `~sub~`, `??citation??`.
const EMPHASIS = ["\\*", "_", "\\+", "-", "\\^", "~", "\\?\\?"].map((m) =>
  new RegExp(`(^|[\\s(\\[{>|"'])${m}(\\S(?:.*?\\S)?)${m}(?=$|[\\s.,;:!?)\\]}<|"'])`, "g"));

// One line's inline markup taken out.
export function inline(text) {
  let s = String(text ?? "");
  s = s.replace(/\{color(?::[^}]*)?\}/g, "");
  s = s.replace(/\{\{(.*?)\}\}/g, "$1");
  s = s.replace(/!([^!\s|][^!|]*?)(?:\|[^!]*)?!/g, (_, file) => `[image: ${file.split("/").pop()}]`);
  s = s.replace(/\[~([^\]]+)\]/g, "@$1");
  s = s.replace(/\[\^([^\]]+)\]/g, "$1");
  s = s.replace(/\[([^\]|]*)\|([^\]]*)\]/g, (_, label, target) => label.trim() || target.trim());
  s = s.replace(/\[((?:https?|mailto|ftp|file):[^\]]+)\]/g, "$1");
  for (const re of EMPHASIS) s = s.replace(re, "$1$2");
  for (const [from, to] of EMOTICONS) s = s.split(from).join(to);
  s = s.replace(/\\([*_{}[\]!|^~+?#-])/g, "$1");
  return s;
}

// A table row's cells, `||head||head||` or `|cell|cell|`, after the inline
// markup is out (a link's `|` is not a cell's).
function cells(line) {
  const head = line.startsWith("||");
  const body = inline(line).trim().replace(head ? /^\|\||\|\|$/g : /^\||\|$/g, "");
  return { head, cells: body.split(head ? "||" : "|").map((c) => c.trim()) };
}

// The lines of a block (`{code}`, `{noformat}`, `{quote}`, `{panel}`) from
// line i, whose opening tag `rest` follows, to its closing tag: { body, next }.
function collect(lines, i, tag, rest) {
  const close = `{${tag}}`, body = [];
  const end = rest.indexOf(close);
  if (end >= 0) return { body: [rest.slice(0, end)], next: i + 1 };
  if (rest.trim()) body.push(rest);
  let j = i + 1;
  while (j < lines.length && !lines[j].includes(close)) body.push(lines[j++]);
  if (j < lines.length) {
    const before = lines[j].slice(0, lines[j].indexOf(close));
    if (before.trim()) body.push(before);
    j += 1;
  }
  return { body, next: j };
}

// The blocks of a text in wiki markup:
//   { kind: "h", level, text }        a heading (h1. … h6.)
//   { kind: "p", text }               a line of a paragraph
//   { kind: "gap" }                   between paragraphs
//   { kind: "li", level, ordered, n, text }  a list item (*, -, #, nested **, #*)
//   { kind: "code", lines }           {code} and {noformat}, as written
//   { kind: "quote", title, lines }   {quote}, bq. and {panel}
//   { kind: "row", head, cells }      a table row
//   { kind: "rule" }                  ----
export function blocks(text) {
  const lines = String(text ?? "").replace(/\r\n?/g, "\n").split("\n");
  const out = [];
  let counters = [];
  const push = (block) => {
    if (block.kind !== "li") counters = [];
    if (block.kind === "gap" && (!out.length || out.at(-1).kind === "gap")) return;
    out.push(block);
  };
  for (let i = 0; i < lines.length;) {
    const line = lines[i], trimmed = line.trim();
    let m = /^\{(code|noformat)(?::([^}]*))?\}(.*)$/.exec(trimmed);
    if (m) {
      const { body, next } = collect(lines, i, m[1], m[3]);
      while (body.length && !body[0].trim()) body.shift();
      while (body.length && !body.at(-1).trim()) body.pop();
      push({ kind: "code", lines: body });
      i = next;
      continue;
    }
    m = /^\{(quote|panel)(?::([^}]*))?\}(.*)$/.exec(trimmed);
    if (m) {
      const { body, next } = collect(lines, i, m[1], m[3]);
      const title = m[1] === "panel" ? /(?:^|\|)title=([^|]*)/.exec(m[2] ?? "")?.[1]?.trim() ?? "" : "";
      push({ kind: "quote", title, lines: body.map((l) => inline(l.trim().replace(/^h[1-6]\.\s*/, "").replace(/^[*#-]+\s+/, "• "))).filter(Boolean) });
      i = next;
      continue;
    }
    i += 1;
    if (!trimmed) { push({ kind: "gap" }); continue; }
    if ((m = /^bq\.\s*(.*)$/.exec(trimmed))) { push({ kind: "quote", title: "", lines: [inline(m[1])] }); continue; }
    if ((m = /^h([1-6])\.\s*(.*)$/.exec(trimmed))) { push({ kind: "h", level: Number(m[1]), text: inline(m[2]) }); continue; }
    if (/^-{4,}$/.test(trimmed)) { push({ kind: "rule" }); continue; }
    if ((m = /^([*#]+|-)\s+(.*)$/.exec(trimmed))) {
      const level = m[1].length, ordered = m[1].endsWith("#");
      counters = counters.slice(0, level);
      counters[level - 1] = ordered ? (counters[level - 1] ?? 0) + 1 : 0;
      out.push({ kind: "li", level, ordered, n: counters[level - 1], text: inline(m[2]) });
      continue;
    }
    if (trimmed.startsWith("|")) { push({ kind: "row", ...cells(trimmed) }); continue; }
    for (const part of inline(line).split("\\\\")) push({ kind: "p", text: part.trim() });
  }
  while (out.length && out.at(-1).kind === "gap") out.pop();
  return out;
}

// The first words of a text in wiki markup, on one line: for a popup.
export function gist(text, max = 140) {
  const words = blocks(text).flatMap((b) => b.text ?? b.lines?.join(" ") ?? b.cells?.join(" ") ?? "").join(" ").replace(/\s+/g, " ").trim();
  return words.length > max ? `${words.slice(0, max - 1).trimEnd()}…` : words;
}
