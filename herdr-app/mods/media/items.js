// What the Media mod decides, without the app: which files of the drop
// folder are media and when one is whole, the history and what leaves it,
// the unseen count, an image's size and whether it moves (read from its
// first bytes), how big to draw it, and how long ago something came.
// store.js does the reading and writing; tests/test.mjs runs this alone.

// How many items the panel keeps; older ones leave it, with the mod's own
// copies of them (never the files in the drop folder).
export const KEEP = 50;
// A file is taken in once its size has stayed the same this long ...
export const STABLE_MS = 1000;
// ... and, without a caption file beside it, once it has been there this
// long: a caption written just after the image still makes the toast.
export const CAPTION_WAIT_MS = 2000;

const IMAGE = new Set(["png", "apng", "jpg", "jpeg", "gif", "webp", "svg", "bmp"]);
const VIDEO = new Set(["mp4", "mov", "m4v", "webm"]);
export const CAPTION = "txt";

export function extOf(name) {
  const dot = name.lastIndexOf(".");
  return dot > 0 ? name.slice(dot + 1).toLowerCase() : "";
}

export function baseOf(name) {
  const dot = name.lastIndexOf(".");
  return dot > 0 ? name.slice(0, dot) : name;
}

// "image", "video", "caption", or null for a file the panel leaves alone:
// hidden files and what editors and downloads write while they work.
export function kindOf(name) {
  if (!name || name.startsWith(".") || name.endsWith("~")) return null;
  if (/\.(tmp|part|partial|crdownload|download|swp)$/i.test(name)) return null;
  const ext = extOf(name);
  if (IMAGE.has(ext)) return "image";
  if (VIDEO.has(ext)) return "video";
  if (ext === CAPTION) return "caption";
  return null;
}

// The listing store.js's script prints: `<size>\t<name>` a line, oldest first.
export function parseListing(text) {
  const files = [];
  for (const line of String(text ?? "").split("\n")) {
    const tab = line.indexOf("\t");
    if (tab < 1) continue;
    const size = Number(line.slice(0, tab).trim());
    const name = line.slice(tab + 1);
    if (!Number.isFinite(size) || !name || name.includes("/")) continue;
    files.push({ name, size });
  }
  return files;
}

// What a scan of the drop folder comes to. `state` holds `known` (the
// files already taken in, name → size) and `pending` (files waiting to be
// whole, name → { size, first, since }), both changed in place: `first` is
// when the file was first seen, `since` when it last changed size.
// Returns the files to take in now, oldest first, each with its caption
// file's name when there is one, and the captions that came or changed for
// files taken in before ({ name, caption }).
export function plan(files, state, now) {
  const sizes = new Map(files.map((f) => [f.name, f.size]));
  const captions = new Map();
  for (const f of files) if (kindOf(f.name) === "caption") captions.set(baseOf(f.name), f);
  const take = [];
  for (const f of files) {
    const kind = kindOf(f.name);
    if (kind !== "image" && kind !== "video") continue;
    if (state.known[f.name] === f.size) { delete state.pending[f.name]; continue; }
    const p = state.pending[f.name];
    if (!p) { state.pending[f.name] = { size: f.size, first: now, since: now }; continue; }
    if (p.size !== f.size) { p.size = f.size; p.since = now; continue; }
    if (f.size <= 0 || now - p.since < STABLE_MS) continue;
    const caption = captions.get(baseOf(f.name));
    const captionPending = caption && state.pending[caption.name];
    // A caption file still being written holds its image back a moment.
    if (caption) {
      if (!captionPending || captionPending.size !== caption.size) {
        state.pending[caption.name] = { size: caption.size, first: now, since: now };
        continue;
      }
      if (now - captionPending.since < STABLE_MS / 2) continue;
    } else if (now - p.first < CAPTION_WAIT_MS) continue;
    take.push({ name: f.name, size: f.size, kind, caption: caption?.name ?? null });
  }
  // Captions of files taken in before: a new one, or one that changed.
  const late = [];
  for (const [base, c] of captions) {
    const owner = Object.keys(state.known).find((n) => baseOf(n) === base && kindOf(n) !== "caption" && sizes.has(n));
    if (!owner || take.some((t) => t.name === owner)) continue;
    const key = `${c.name}`;
    const p = state.pending[key];
    if (state.known[key] === c.size) { delete state.pending[key]; continue; }
    if (!p || p.size !== c.size) { state.pending[key] = { size: c.size, first: now, since: now }; continue; }
    if (now - p.since < STABLE_MS / 2) continue;
    late.push({ name: owner, caption: c.name, size: c.size });
  }
  // What left the folder is forgotten, so a file of that name later is new.
  for (const name of Object.keys(state.known)) if (!sizes.has(name)) delete state.known[name];
  for (const name of Object.keys(state.pending)) if (!sizes.has(name)) delete state.pending[name];
  return { take, late };
}

// A caption file's text as the panel shows it: its first line that is not empty.
export function captionText(text) {
  for (const line of String(text ?? "").split(/\r?\n/)) {
    const t = line.trim();
    if (t) return t.length > 300 ? `${t.slice(0, 299)}…` : t;
  }
  return "";
}

// Puts `item` first in `items` (newest first) and keeps KEEP of them.
// Returns the items that left, whose own files are to be deleted.
export function addItem(items, item, keep = KEEP) {
  items.unshift(item);
  return items.length > keep ? items.splice(keep) : [];
}

// How many items came after the newest one the person has seen.
export function unseenCount(items, seenAt) {
  return items.filter((i) => i.at > (seenAt ?? 0)).length;
}

// The mod's own files of an item, relative to the app's home: the one it
// draws (a copy of an image, a video's converted clip or still frame).
export function filesOf(item) {
  return [item.file].filter((f) => typeof f === "string" && f.startsWith("media/items/"));
}

// --- the layout ------------------------------------------------------------------

function walk(node, path, visit) {
  if (!node || typeof node !== "object") return;
  visit(node, path);
  for (const child of node.children ?? []) walk(child, [...path, node], visit);
}

// The block of the right-hand panel showing this mod (`props.panel`), with
// its ancestors, or null.
export function panelBlock(layout) {
  let found = null;
  walk(layout, [], (node, path) => {
    const c = node.content;
    if (!found && c && c.mod === "media" && c.props?.panel === true) found = { node, path };
  });
  return found;
}

// Whether the panel is on screen: no ancestor hidden, and each stack on the
// way shows the child it is in (a stack with no `active` shows its first).
export function panelShown(layout) {
  const found = panelBlock(layout);
  if (!found || found.node.hidden) return false;
  const chain = [...found.path, found.node];
  for (let i = 0; i < chain.length - 1; i++) {
    const node = chain[i], child = chain[i + 1];
    if (node.hidden) return false;
    if (node.type === "stack") {
      const active = node.active ?? node.children?.find((c) => c?.id)?.id;
      if (active !== child.id) return false;
    }
  }
  return true;
}

// The commands that show the panel: select it in its stack, then show every
// hidden block above it (Herdr's `panel` column). [] when there is no panel.
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

// --- an image's first bytes ------------------------------------------------------

const u16be = (b, i) => (b[i] << 8) | b[i + 1];
const u16le = (b, i) => b[i] | (b[i + 1] << 8);
const u24le = (b, i) => b[i] | (b[i + 1] << 8) | (b[i + 2] << 16);
const u32be = (b, i) => ((b[i] << 24) >>> 0) + ((b[i + 1] << 16) | (b[i + 2] << 8) | b[i + 3]);
const ascii = (b, i, n) => String.fromCharCode(...b.subarray(i, i + n));

// How many frames a GIF has, up to `stop`.
function gifFrames(b, stop = 2) {
  let i = 13;
  if (b[10] & 0x80) i += 3 * (1 << ((b[10] & 7) + 1));
  let frames = 0;
  const skipBlocks = () => { while (i < b.length && b[i] !== 0) i += b[i] + 1; i += 1; };
  while (i < b.length && frames < stop) {
    const tag = b[i];
    if (tag === 0x21) { i += 2; skipBlocks(); }
    else if (tag === 0x2c) {
      frames += 1;
      const packed = b[i + 9];
      i += 10;
      if (packed & 0x80) i += 3 * (1 << ((packed & 7) + 1));
      i += 1; // the LZW code size
      skipBlocks();
    } else break;
  }
  return frames;
}

// { format, width, height, animated } of an image file's bytes; width and
// height are null when they cannot be read.
export function imageInfo(bytes, name = "") {
  const b = bytes instanceof Uint8Array ? bytes : new Uint8Array(bytes ?? []);
  const out = { format: extOf(name) || "unknown", width: null, height: null, animated: false };
  if (b.length >= 24 && b[0] === 0x89 && ascii(b, 1, 3) === "PNG") {
    out.format = "png";
    out.width = u32be(b, 16); out.height = u32be(b, 20);
    // An APNG has an acTL chunk before its first IDAT.
    for (let i = 8; i + 8 <= b.length;) {
      const len = u32be(b, i), type = ascii(b, i + 4, 4);
      if (type === "acTL") { out.animated = true; out.format = "apng"; break; }
      if (type === "IDAT" || type === "IEND") break;
      i += 12 + len;
    }
  } else if (b.length >= 10 && ascii(b, 0, 3) === "GIF") {
    out.format = "gif";
    out.width = u16le(b, 6); out.height = u16le(b, 8);
    out.animated = gifFrames(b) > 1;
  } else if (b.length >= 30 && ascii(b, 0, 4) === "RIFF" && ascii(b, 8, 4) === "WEBP") {
    out.format = "webp";
    const chunk = ascii(b, 12, 4);
    if (chunk === "VP8X") {
      out.animated = Boolean(b[20] & 0x02);
      out.width = u24le(b, 24) + 1; out.height = u24le(b, 27) + 1;
    } else if (chunk === "VP8 ") {
      out.width = u16le(b, 26) & 0x3fff; out.height = u16le(b, 28) & 0x3fff;
    } else if (chunk === "VP8L") {
      const bits = b[21] | (b[22] << 8) | (b[23] << 16) | (b[24] << 24);
      out.width = (bits & 0x3fff) + 1; out.height = ((bits >>> 14) & 0x3fff) + 1;
    }
  } else if (b.length >= 4 && b[0] === 0xff && b[1] === 0xd8) {
    out.format = "jpeg";
    for (let i = 2; i + 9 < b.length;) {
      if (b[i] !== 0xff) { i += 1; continue; }
      const marker = b[i + 1];
      if (marker === 0xd8 || marker === 0x01 || (marker >= 0xd0 && marker <= 0xd7)) { i += 2; continue; }
      const len = u16be(b, i + 2);
      if (marker >= 0xc0 && marker <= 0xcf && ![0xc4, 0xc8, 0xcc].includes(marker)) {
        out.height = u16be(b, i + 5); out.width = u16be(b, i + 7);
        break;
      }
      i += 2 + len;
    }
  } else if (b.length >= 26 && ascii(b, 0, 2) === "BM") {
    out.format = "bmp";
    out.width = Math.abs(b[18] | (b[19] << 8) | (b[20] << 16) | (b[21] << 24));
    out.height = Math.abs(b[22] | (b[23] << 8) | (b[24] << 16) | (b[25] << 24));
  } else {
    const text = ascii(b, 0, Math.min(b.length, 4096));
    if (/<svg[\s>]/i.test(text)) {
      out.format = "svg";
      const tag = text.slice(text.search(/<svg[\s>]/i)).split(">")[0];
      const attr = (n) => { const m = new RegExp(`\\b${n}\\s*=\\s*["']\\s*([\\d.]+)(px)?\\s*["']`).exec(tag); return m ? Number(m[1]) : null; };
      out.width = attr("width"); out.height = attr("height");
      const vb = /\bviewBox\s*=\s*["']\s*[-\d.]+[\s,]+[-\d.]+[\s,]+([\d.]+)[\s,]+([\d.]+)/.exec(tag);
      if ((!out.width || !out.height) && vb) { out.width = Number(vb[1]); out.height = Number(vb[2]); }
    }
  }
  if (!(out.width > 0) || !(out.height > 0)) { out.width = null; out.height = null; }
  return out;
}

// The size to draw a `width`×`height` image at inside `maxW`×`maxH`,
// keeping its shape and never larger than it is (a vector may grow).
// Unknown sizes are drawn 4:3.
export function fit(width, height, maxW, maxH, { grow = false } = {}) {
  const w = width > 0 ? width : 4, h = height > 0 ? height : 3;
  let scale = Math.min(maxW / w, maxH / h);
  if (!grow && width > 0 && height > 0) scale = Math.min(scale, 1);
  return { w: Math.max(1, Math.round(w * scale)), h: Math.max(1, Math.round(h * scale)) };
}

// "just now", "5 min ago", "3 h ago", "2 days ago".
export function ago(at, now) {
  const s = Math.max(0, Math.round((now - at) / 1000));
  if (s < 45) return "just now";
  const m = Math.round(s / 60);
  if (m < 60) return `${m} min ago`;
  const h = Math.round(m / 60);
  if (h < 24) return `${h} h ago`;
  const d = Math.round(h / 24);
  return d === 1 ? "yesterday" : `${d} days ago`;
}

// What an item is called: its caption, else its file name.
export function titleOf(item) {
  return item.caption || item.name;
}

// One line of text: a `\n` would start a second line even where text is cut.
export function oneLine(text) {
  return String(text ?? "").replace(/\s+/g, " ").trim();
}

// What the panel says under an item it cannot show as it is.
export function noteOf(item) {
  if (item.kind === "video") {
    if (item.state === "converting") return "Video: making a preview…";
    if (item.preview === "converted") return "Video: the panel shows its first frame; Open plays it";
    if (item.preview === "still") return "Video: a still frame; Open plays it";
    return "Video: no preview; Open plays it";
  }
  if (item.animated) return `Animated ${String(item.format ?? "").toUpperCase()}: the panel shows its first frame; Open plays it`;
  return "";
}
