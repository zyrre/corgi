# Changes the app's layouts for install.sh, in place, and the tab bar their
# `tabs` block shows: run by install.sh as
#
#   python3 -I install-layout.py ACTION HOME LAYOUT APP
#
# ACTION `add` puts into one layout: the panel, a child of its right-hand
# panel stack `panel-body` beside Files, Changes (and GitLab, Jenkins); the
# watcher, a floating block on the layout's root that draws nothing and
# keeps the drop folder watched while the panel has never been shown (the
# app starts a stack's other children only when they show); and a Media
# button with the unseen count in that layout's tab bar. `off` takes the
# panel, the watcher and the button out of one; `all` takes every block
# showing the mod, and every button, out of every layout of the person's (or
# of LAYOUT's), for the uninstall. LAYOUT may be empty: the shown layout
# (`add`, `off`) or all (`all`). APP is the herdr-app to check a changed tab
# bar with, or empty.
#
# Herdr's own layouts and mods are never changed. When the tab bar is Herdr's
# own `tabs` mod, `add` clones it to `tabs-media` (as `herdr-app do
# mod.clone` would, which needs the window) and points the layout's `tabs`
# block at the copy; `off` and `all` point it back at `tabs`, and remove the
# copy when no layout shows it, while the copy is still as cloned. When it is
# a copy already (herdr-gitlab's `tabs-gitlab`, the Jenkins mod's
# `tabs-jenkins`, Corgi's `tabs-corgi`, the person's own), two places of its
# main.js change, each between marker lines, as Corgi's installer does, so
# that other installers' marked lines stay as they are:
#
# - above the tab bar's call that remembers per space which panel was open
#   (`this.syncPanel(…)`), and above any comment lines or other installers'
#   marked lines just over it, a line adds `media-panel` to that call's list
#   while the layout's stack has the panel. Its end marker ends the code
#   line itself, so that an installer that walks up over comment lines from
#   the call (Corgi's) stops at it rather than inside it;
# - just before the window buttons (`.when(w?.buttons,`), the button: the
#   tab bar's own `panelButton`, as Files' and Changes', with the count the
#   mod keeps in `shared("media.unseen")` as a pill.
#
# Every file is written whole with one rename, keeping its mode, and only
# after every change was worked out: a failure writes nothing.
import hashlib, json, os, re, shutil, subprocess, sys

action, home, name, app = sys.argv[1:5]
MOD = "media"
BLOCK = "media-panel"
WATCH = "media-watch"
PANEL = {"type": "block", "content": {"mod": MOD, "props": {"panel": True}}}
WATCHER = {"content": {"mod": MOD, "props": {"watch": True}}, "edge": "bottom", "align": "end"}
STACK, COLUMN = "panel-body", "panel"
STOCK, CLONE = "tabs", "tabs-media"
UNSEEN = "media.unseen"
CLONED = "// Herdr's tabs, cloned by mods/media/install.sh for its Media button; --panel off and --uninstall put the layout back on tabs while the rest is as cloned. As cloned: "
LIST_TAG, LIST_END = "// >>> Media panel", "// <<< Media panel"
BUTTON_TAG, BUTTON_END = "// >>> Media button", "// <<< Media button"
ANCHOR = ".when(w?.buttons,"
SYNC = "this.syncPanel("
# What the lines use of the tab bar around them, as Herdr's has them.
NEEDS = {
    "find": r"\bfunction\s+find\b|\bfind\s*=",
    "body": r"\bbody\s*=\s*find\(",
    "panelButton": r"\bpanelButton\s*=",
    "enabled": r"\benabled\s*=",
    "shared": r"\bshared\b",
    "div": r"\bdiv\b",
    "t": r"\bt\s*=\s*theme\(\)",
}
BY_HAND = "add the button by hand, between the marker lines, as the Media mod's README.md shows (\"The Media button\")"


def say(text):
    print(f"media mod: {text}", file=sys.stderr)


def fail(text):
    say(text)
    sys.exit(1)


def load(path):
    try:
        with open(path, encoding="utf-8") as f:
            return json.load(f)
    except (OSError, ValueError):
        return None


def read(path):
    with open(path, encoding="utf-8") as f:
        return f.read()


def nodes(node):
    yield node
    for child in node.get("children") or []:
        if isinstance(child, dict):
            yield from nodes(child)


def shows(node, prop=None):
    content = node.get("content")
    if not isinstance(content, dict) or content.get("mod") != MOD:
        return False
    return prop is None or (content.get("props") or {}).get(prop) is True


def by_id(layout, id):
    return next((x for x in nodes(layout) if x.get("id") == id), None)


def stack_of(layout):
    column = by_id(layout, COLUMN)
    body = by_id(column, STACK) if column else None
    if body is None or body.get("type") != "stack" or not isinstance(body.get("children"), list):
        return None
    return body


def tab_block(layout):
    block = by_id(layout, "tabs")
    content = block.get("content") if block else None
    if isinstance(content, dict) and isinstance(content.get("mod"), str) and content["mod"]:
        return block
    return None


def panel_of(layout):
    return next((x for x in nodes(layout) if shows(x, "panel")), None)


# Takes the blocks showing the mod (with props `prop`, or any) out from under
# `node`, its children and floating blocks; a stack showing one shows its
# first child left instead, and a floating list left empty goes. Returns
# their ids.
def take(node, prop=None):
    gone = []
    floating = node.get("floating")
    if isinstance(floating, list):
        kept = [f for f in floating if not (isinstance(f, dict) and shows(f, prop))]
        gone += [f.get("id", "") for f in floating if f not in kept]
        if kept:
            node["floating"] = kept
        elif len(kept) != len(floating):
            node.pop("floating")
    children = node.get("children")
    if isinstance(children, list):
        kept = []
        for child in children:
            if isinstance(child, dict) and shows(child, prop):
                gone.append(child.get("id", ""))
                continue
            if isinstance(child, dict):
                gone += take(child, prop)
            kept.append(child)
        node["children"] = kept
        if node.get("active") in gone:
            first = next((c.get("id") for c in kept if isinstance(c, dict) and c.get("id")), None)
            if first:
                node["active"] = first
            else:
                node.pop("active", None)
    return gone


def write_text(path, text):
    temp = os.path.join(os.path.dirname(path), f".{os.path.basename(path)}.media")
    with open(temp, "w", encoding="utf-8") as f:
        f.write(text)
    if os.path.exists(path):
        os.chmod(temp, os.stat(path).st_mode & 0o777)
    os.replace(temp, path)


def write(path, layout):
    write_text(path, json.dumps(layout, indent=2, ensure_ascii=False) + "\n")


# --- the tab bar's lines ------------------------------------------------------

def list_lines(indent):
    lines = [
        f"{LIST_TAG}: install.sh of mods/media adds the Media panel to the panels remembered per space; --panel off takes it out",
        f'try {{ if (find(body, "{BLOCK}") && !enabled.includes("{BLOCK}")) enabled.push("{BLOCK}"); }} catch (_) {{}} {LIST_END}',
    ]
    return "".join(f"{indent}{line}\n" for line in lines)


def button_lines(indent):
    count = f'Number(shared("{UNSEEN}"))'
    lines = [
        f"{BUTTON_TAG}: install.sh of mods/media puts it here, and takes it out (--panel off)",
        f'.when(Boolean(find(body, "{BLOCK}")), el => el.child(panelButton("{BLOCK}", "Media", "mods/media/icons/media.svg")',
        f"  .when({count} > 0, b => b.child(div().px(5).rounded(4).bg(t.accent).text_color(t.accent_foreground).text_size(10.5).font_semibold()"
        f".child(String({count}))))))",
        BUTTON_END,
    ]
    return "".join(f"{indent}{line}\n" for line in lines)


# Where the lines between `begin` and `end` are: (start, stop) offsets of
# whole lines, or None. The end marker may end a code line.
def marked(text, where, begin, end):
    lines = text.splitlines(keepends=True)
    begins = [i for i, l in enumerate(lines) if l.strip().startswith(begin)]
    ends = [i for i, l in enumerate(lines) if end in l]
    if not begins and not ends:
        return None
    if len(begins) != 1 or len(ends) != 1 or ends[0] < begins[0]:
        fail(f"{where} has Media marker lines ({begin}) that do not pair up: fix them by hand; left alone")
    start = sum(len(l) for l in lines[:begins[0]])
    stop = sum(len(l) for l in lines[:ends[0] + 1])
    return start, stop


def without_button(text, where):
    for begin, end in ((LIST_TAG, LIST_END), (BUTTON_TAG, BUTTON_END)):
        span = marked(text, where, begin, end)
        if span is not None:
            text = text[:span[0]] + text[span[1]:]
    return text


# A Media button not between the markers (one put in by hand).
def unmarked(text, where):
    return re.search(r'panelButton\(\s*"media', without_button(text, where)) is not None


def indent_of(line):
    return line[:len(line) - len(line.lstrip())]


# Lines inside any installer's marked lines (`// >>> …` to the line holding `// <<< …`).
def in_marked(lines):
    inside, open_ = set(), False
    for i, line in enumerate(lines):
        if line.strip().startswith("// >>> "):
            open_ = True
        if open_:
            inside.add(i)
        if open_ and "// <<< " in line:
            open_ = False
    return inside


# main.js with Media's lines in it: where they are already (afresh, in their
# places, so that lines other installers put in after them stay where they
# are), else put in. Fails when they cannot go in.
def with_button(text, where):
    spans = [marked(text, where, LIST_TAG, LIST_END), marked(text, where, BUTTON_TAG, BUTTON_END)]
    if all(spans):
        (ls, le), (bs, be) = spans
        fresh = [(ls, le, list_lines(indent_of(text[ls:le]))), (bs, be, button_lines(indent_of(text[bs:be])))]
        for start, stop, lines in sorted(fresh, reverse=True):
            text = text[:start] + lines + text[stop:]
        return text
    text = without_button(text, where)
    missing = [n for n, pattern in NEEDS.items() if not re.search(pattern, text)]
    if missing:
        fail(f"{where} is not like Herdr's tab bar: the button needs {', '.join(missing)} there; {BY_HAND}, then run this again. Nothing was changed")
    lines = text.splitlines(keepends=True)
    calls = [i for i, l in enumerate(lines) if l.lstrip().startswith(SYNC)]
    if len(calls) != 1:
        fail(f"{where} is not like Herdr's tab bar: no single this.syncPanel(…) call (which remembers the open panel per space); "
             f"{BY_HAND}, then run this again. Nothing was changed")
    inside = in_marked(lines)
    at = calls[0]
    while at > 0 and (lines[at - 1].lstrip().startswith("//") or at - 1 in inside):
        at -= 1
    offset = sum(len(l) for l in lines[:at])
    text = text[:offset] + list_lines(indent_of(lines[calls[0]])) + text[offset:]
    found = [m.start() for m in re.finditer(re.escape(ANCHOR), text)]
    if len(found) != 1:
        fail(f"{where} is not like Herdr's tab bar: no single place before its window buttons ({ANCHOR}) "
             f"to put a Media button; {BY_HAND}, then run this again. Nothing was changed")
    line = text.rfind("\n", 0, found[0]) + 1
    indent = re.match(r"[ \t]*", text[line:]).group(0)
    if text[line:found[0]].strip():
        return text[:found[0]] + "\n" + button_lines(indent) + indent + text[found[0]:]
    return text[:line] + button_lines(indent) + text[line:]


# --- the clone of Herdr's tab bar ---------------------------------------------

def digest(text):
    return hashlib.sha256(text.encode("utf-8")).hexdigest()


def app_written(file):
    return file == "jsconfig.json" or file.endswith(".d.ts")


def mod_files(folder):
    files = {}
    for root, dirs, names in os.walk(folder):
        dirs[:] = [d for d in dirs if not d.startswith(".")]
        for file in names:
            if app_written(file) or file.startswith("."):
                continue
            path = os.path.join(root, file)
            with open(path, "rb") as f:
                files[os.path.relpath(path, folder)] = f.read()
    return files


def clone_files(mods, version):
    stock = os.path.join(mods, STOCK)
    files = mod_files(stock)
    for rel, data in list(files.items()):
        if rel.endswith(".js"):
            files[rel] = data.decode("utf-8").replace(f"mods/{STOCK}/", f"mods/{CLONE}/").encode("utf-8")
    if "main.js" not in files:
        fail(f"no main.js in {stock}, Herdr's tab bar: is this an app home?")
    meta = load(os.path.join(stock, "mod.json"))
    meta = meta if isinstance(meta, dict) else {}
    meta["cloned_from"] = {"name": STOCK, **({"version": version} if version else {})}
    files["mod.json"] = (json.dumps(meta, indent=2, ensure_ascii=False) + "\n").encode("utf-8")
    main = files["main.js"].decode("utf-8")
    files["main.js"] = (f"{CLONED}{digest(main)}\n{main}").encode("utf-8")
    return files


# Whether mods/tabs-media is this installer's, and whether its main.js, but
# for Media's lines, is still as cloned.
def clone_state(mods):
    path = os.path.join(mods, CLONE, "main.js")
    if not os.path.isdir(os.path.join(mods, CLONE)):
        return "none"
    try:
        text = read(path)
    except OSError:
        return "other"
    first, _, rest = text.partition("\n")
    if not first.startswith(CLONED):
        return "other"
    return "as-cloned" if first[len(CLONED):] == digest(without_button(rest, f"mods/{CLONE}/main.js")) else "changed"


def same_files(folder, files):
    return mod_files(folder) == files


def check(mods, files, where):
    if not app:
        return
    stage = os.path.join(mods, ".media-tabs-check")
    shutil.rmtree(stage, ignore_errors=True)
    try:
        for rel, data in files.items():
            os.makedirs(os.path.dirname(os.path.join(stage, rel)), exist_ok=True)
            with open(os.path.join(stage, rel), "wb") as f:
                f.write(data)
        r = subprocess.run([app, "check", stage], capture_output=True, text=True)
        if r.returncode != 0:
            tail = "\n".join((r.stdout + r.stderr).strip().splitlines()[-3:])
            fail(f"herdr-app check refused {where} with the Media button: {tail}; nothing was changed")
    finally:
        shutil.rmtree(stage, ignore_errors=True)


def put_clone(mods, files):
    stage = os.path.join(mods, f".{CLONE}.media-install")
    shutil.rmtree(stage, ignore_errors=True)
    for rel, data in files.items():
        os.makedirs(os.path.dirname(os.path.join(stage, rel)), exist_ok=True)
        with open(os.path.join(stage, rel), "wb") as f:
            f.write(data)
    target = os.path.join(mods, CLONE)
    if os.path.isdir(target):
        shutil.rmtree(target)
    os.replace(stage, target)


# --- the layouts ----------------------------------------------------------------

folder = os.path.join(home, "layouts")
mods = os.path.join(home, "mods")
installed = load(os.path.join(home, "installed.json"))


def shipped(n):
    if isinstance(installed, dict):
        return f"layout:{n}" in installed
    return n in ("main", "terminals", "win98")


def shipped_mod(n):
    if isinstance(installed, dict):
        return n in installed
    return n == STOCK


def personal_layouts():
    if not os.path.isdir(folder):
        return []
    return sorted(f[:-5] for f in os.listdir(folder)
                  if f.endswith(".json") and not f.startswith(".") and not shipped(f[:-5]))


if not name and action != "all":
    name = (load(os.path.join(home, "shared.json")) or {}).get("layout")
    if not isinstance(name, str) or not name:
        fail("which layout? Name it with --layout (layouts/<name>.json)")
names = [name] if name else personal_layouts()
version = (load(os.path.join(home, "update-notes.json")) or {}).get("last_run")
AGAIN = "if an install of another mod (such as herdr-gitlab) later replaces that tab bar or points the layout at another, run install.sh --panel right again to bring the button back"


def open_layout(n):
    path = os.path.join(folder, f"{n}.json")
    if shipped(n):
        fail(f"{n} is Herdr's own layout, which Herdr replaces on every start: clone it first "
             f"(herdr-app do layout.clone '{{\"name\":\"{n}\",\"as\":\"mine\"}}') and name the clone with --layout")
    if not os.path.isfile(path):
        fail(f"no layout {n} in {folder}")
    layout = load(path)
    if not isinstance(layout, dict):
        fail(f"{path} is not a layout's JSON: left alone")
    return path, layout


def unique(layout, base):
    taken = {x.get("id") for x in nodes(layout)}
    for x in nodes(layout):
        taken |= {f.get("id") for f in x.get("floating") or [] if isinstance(f, dict)}
    block, i = base, 2
    while block in taken:
        block, i = f"{base}-{i}", i + 1
    return block


def show_by_hand(block):
    return (f"herdr-app do layout.select --stack {STACK} --child {block}, then herdr-app do layout.show --id {COLUMN}, "
            f"shows it (layout.hide --id {COLUMN} hides it again)")


def add(n):
    path, layout = open_layout(n)
    changed = False
    body = stack_of(layout)
    if body is None:
        fail(f"layout {n} has no right-hand panel like Herdr's layouts (a stack {STACK} in a column {COLUMN}, "
             f"where Files and Changes are), so there is no place for Media beside them: left alone")
    there = next((c for c in body["children"] if isinstance(c, dict) and shows(c)), None)
    if there is not None:
        say(f"layout {n} shows Media in its right-hand panel already (block {there.get('id', '?')}): left as it is")
    else:
        there = {"id": unique(layout, BLOCK), **PANEL}
        body["children"].append(there)
        changed = True
    panel = there if shows(there, "panel") else None
    # The watcher: one floating block on the root, wherever the panel is.
    watchers = [f for x in nodes(layout) for f in x.get("floating") or [] if isinstance(f, dict) and shows(f, "watch")]
    if panel is not None and not watchers:
        layout.setdefault("floating", []).append({"id": unique(layout, WATCH), **WATCHER})
        changed = True
    # The button, worked out whole before anything is written.
    put = None  # ("clone", files) or ("main", path, text, whether it had the button)
    tabs = tab_block(layout)
    bar = None
    if panel is None:
        say(f"layout {n}: no Media button in its tab bar, which goes only with the panel install.sh --panel right puts in (props panel)")
    elif tabs is None:
        say(f"layout {n} has no tab bar (a block tabs): no Media button; {show_by_hand(panel['id'])}")
    else:
        bar = tabs["content"]["mod"]
        if bar == STOCK and shipped_mod(STOCK) or bar == CLONE:
            state = clone_state(mods)
            if state == "other":
                fail(f"{os.path.join(mods, CLONE)} is another mod, not the copy of Herdr's tab bar this installer makes: "
                     f"rename it, or point the layout's tabs block at a copy of your own and run this again. Nothing was changed")
            if state in ("none", "as-cloned"):
                files = clone_files(mods, version)
                first, _, rest = files["main.js"].decode("utf-8").partition("\n")
                files["main.js"] = f"{first}\n{with_button(rest, f'mods/{STOCK}/main.js')}".encode("utf-8")
                if state == "none" or not same_files(os.path.join(mods, CLONE), files):
                    check(mods, files, f"a copy of mods/{STOCK}")
                    put = ("clone", files)
            else:
                where = f"mods/{CLONE}/main.js"
                main_path = os.path.join(mods, CLONE, "main.js")
                text = read(main_path)
                new = with_button(text, where)
                if new != text:
                    files = mod_files(os.path.join(mods, CLONE))
                    files["main.js"] = new.encode("utf-8")
                    check(mods, files, where)
                    put = ("main", main_path, new, marked(text, where, BUTTON_TAG, BUTTON_END) is not None)
            if bar != CLONE:
                tabs["content"]["mod"] = CLONE
                changed = True
            bar = CLONE
        elif shipped_mod(bar):
            say(f"layout {n}: its tab bar is Herdr's {bar}, which this installer cannot put a button into: no Media button; "
                f"{show_by_hand(panel['id'])}")
            bar = None
        else:
            where = f"mods/{bar}/main.js"
            main_path = os.path.join(mods, bar, "main.js")
            if not os.path.isfile(main_path):
                say(f"layout {n}: its tab bar {bar} has no {where}: no Media button")
                bar = None
            else:
                text = read(main_path)
                if unmarked(text, where):
                    say(f"{where} has a Media button already, not between this installer's markers: left as it is")
                    bar = None
                else:
                    new = with_button(text, where)
                    if new != text:
                        files = mod_files(os.path.join(mods, bar))
                        files["main.js"] = new.encode("utf-8")
                        check(mods, files, where)
                        put = ("main", main_path, new, marked(text, where, BUTTON_TAG, BUTTON_END) is not None)
    # The tab bar first: its button shows only while the layout has the panel.
    if put and put[0] == "clone":
        fresh = not os.path.isdir(os.path.join(mods, CLONE))
        put_clone(mods, put[1])
        say(f"{'made' if fresh else 'updated'} mods/{CLONE}, a copy of Herdr's tab bar as it is now, with a Media button")
    elif put:
        write_text(put[1], put[2])
        say(f"{'updated the' if put[3] else 'put a'} Media button {'in' if put[3] else 'into'} mods/{bar}/main.js")
    elif bar:
        say(f"mods/{bar} has the Media button already")
    if bar:
        say(f"the tab bar's Media button shows the panel, with the count of what came since it was last seen; {AGAIN}")
    if changed:
        write(path, layout)
        if panel is not None:
            say(f"layout {n}: Media is in its right-hand panel (stack {STACK}, block {panel['id']}), beside Files and Changes, "
                f"with a watcher that draws nothing ({WATCH}, floating)")
        if tabs is not None and tabs["content"]["mod"] == CLONE:
            say(f"layout {n}: its tab bar is mods/{CLONE} now")


# Takes the panel and the watcher out of the layouts named (`ours`: only
# what install.sh put in; else every block showing the mod), and the button
# out of their tab bars, unless a layout that keeps a panel shows that tab
# bar too.
def remove(names, ours):
    edited = {}
    bars = set()
    for n in names:
        if shipped(n) and not ours:
            continue
        path, layout = open_layout(n)
        gone = take(layout, "panel") + take(layout, "watch") if ours else take(layout)
        tabs = tab_block(layout)
        if tabs is not None:
            bars.add(tabs["content"]["mod"])
        edited[n] = (path, layout, gone)
    after = {}
    for n in personal_layouts():
        if n in edited:
            after[n] = edited[n][1]
        else:
            layout = load(os.path.join(folder, f"{n}.json"))
            if isinstance(layout, dict):
                after[n] = layout

    def users(bar):
        return [n for n, l in after.items() if (tab_block(l) or {}).get("content", {}).get("mod") == bar]

    keep = {bar for bar in bars for n in users(bar) if panel_of(after[n]) is not None}
    texts = {}
    for bar in sorted(bars - keep):
        if shipped_mod(bar):
            continue
        main_path = os.path.join(mods, bar, "main.js")
        try:
            text = read(main_path)
        except OSError:
            continue
        where = f"mods/{bar}/main.js"
        new = without_button(text, where)
        if new != text:
            texts[bar] = (main_path, new)
        elif unmarked(text, where):
            say(f"{where} has a Media button that this installer did not put in: left as it is")
    back = CLONE in bars and clone_state(mods) == "as-cloned" and os.path.isfile(os.path.join(mods, STOCK, "main.js"))
    if CLONE in bars and not back:
        say(f"mods/{CLONE} was changed since this installer made it: the layout keeps it")
    for n, (path, layout, gone) in edited.items():
        tabs = tab_block(layout)
        if back and tabs is not None and tabs["content"]["mod"] == CLONE and panel_of(layout) is None:
            tabs["content"]["mod"] = STOCK
            gone.append(f"tab bar {CLONE}, back to {STOCK}")
    for n, (path, layout, gone) in edited.items():
        if gone:
            write(path, layout)
            say(f"layout {n}: took Media out ({', '.join(gone)})")
        elif name:
            say(f"layout {n} has no Media panel: left as it is")
    for bar, (main_path, new) in texts.items():
        if bar == CLONE and back and not users(CLONE):
            continue
        write_text(main_path, new)
        say(f"took the Media button out of mods/{bar}/main.js")
    if back and not users(CLONE) and os.path.isdir(os.path.join(mods, CLONE)):
        shutil.rmtree(os.path.join(mods, CLONE))
        say(f"removed mods/{CLONE}, the copy of Herdr's tab bar this installer made")


if action == "add":
    add(names[0])
elif action == "off":
    remove(names, True)
else:
    remove(names, False)
