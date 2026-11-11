# Changes the app's layouts for install.sh, in place, and the tab bar their
# `tabs` block shows: run by install.sh as
#
#   python3 -I install-layout.py ACTION HOME LAYOUT APP
#
# ACTION `add` puts the panel into one layout, as a child of its right-hand
# panel stack `panel-body` beside Files, Changes (and GitLab), and a Jenkins
# button that shows it there into that layout's tab bar; `off` takes the
# panel this installer put in, and the button, out of one; `all` takes every
# block showing the mod, and every button, out of every layout of the
# person's (or of LAYOUT's), for the uninstall. LAYOUT may be empty: the
# shown layout (`add`, `off`) or all (`all`). APP is the herdr-app to check a
# changed tab bar with, or empty.
#
# An earlier version put the panel in a block of its own at the end of the
# layout's top row; `add` moves it into the stack.
#
# Herdr's own layouts and mods are never changed. When the tab bar is Herdr's
# own `tabs` mod, `add` clones it to `tabs-jenkins` (as `herdr-app do
# mod.clone` would, which needs the window) and points the layout's `tabs`
# block at the copy; `off` and `all` point it back at `tabs`, and remove the
# copy when no layout shows it, while the copy is still as cloned. When it is
# a copy already (herdr-gitlab's `tabs-gitlab`, or the person's own), the
# button goes into its main.js between marker lines. Every file is written
# whole with one rename, keeping its mode, and only after every change was
# worked out: a failure writes nothing.
import hashlib, json, os, re, shutil, subprocess, sys

action, home, name, app = sys.argv[1:5]
MOD = "jenkins"
# The panel: a child of the stack panel-body, as GitLab's is. `panel` tells
# the mod it is the panel, and this installer that the block is its own.
PANEL = {"type": "block", "content": {"mod": MOD, "props": {"panel": True}}}
# Its id: one no earlier version gave a block. The app keeps a block's state
# (hidden, size) by id in layouts/.state/, also after the block left the
# layout, and an earlier version's block `jenkins` (a block of its own the
# tab bar hid and showed) left `hidden` there, which would hide a stack child
# of that id, so the stack showed another child while its button was lit.
BLOCK = "jenkins-panel"
STACK, COLUMN = "panel-body", "panel"
STOCK, CLONE = "tabs", "tabs-jenkins"
# The line of the clone's main.js that says this installer made it, and what
# its main.js held then (without this line and the marked lines), so that
# `off` can tell whether it is still as cloned.
CLONED = "// Herdr's tabs, cloned by mods/jenkins/install.sh for its Jenkins button; --panel off and --uninstall put the layout back on tabs while the rest is as cloned. As cloned: "
# Two marked places in a tab bar's main.js: the button, and the tab bar's
# call of syncPanel (which remembers per space which of the panels in its
# list was open), whose list gets Jenkins while the layout's stack has it.
# The tab bar's own call is kept after WAS, which --panel off puts back. Not
# its `enabled` line: herdr-gitlab's installer finds that line by its text
# and replaces it, so a copy of it kept in a comment would be replaced too.
BEGIN_TAG, END = "// >>> Jenkins button", "// <<< Jenkins button"
BEGIN = f"{BEGIN_TAG}: install.sh of mods/jenkins puts it here, and takes it out (--panel off)"
LIST_TAG, LIST_END = "// >>> Jenkins panel", "// <<< Jenkins panel"
LIST_BEGIN = f"{LIST_TAG}: install.sh of mods/jenkins adds it to the panels remembered per space; --panel off puts back the line after was:"
WAS = "// was: "
# The button goes just before the window buttons, after Files, Changes and any other panel's.
ANCHOR = ".when(w?.buttons,"
# The tab bar's call that remembers the open panel per space, of the panels in `enabled`.
SYNC = re.compile(r"^([ \t]*)(this\.syncPanel\((.*)\benabled\);[ \t]*)$", re.M)
# What the button's code uses from the tab bar around it, as Herdr's has them.
NEEDS = {
    "find": r"\bfunction\s+find\b|\bfind\s*=",
    "body": r"\bbody\s*=\s*find\(",
    "panelButton": r"\bpanelButton\s*=",
}
BY_HAND = "add the button by hand, between the marker lines, as the Jenkins mod's README.md shows (\"The Jenkins button\")"


def say(text):
    print(f"jenkins mod: {text}", file=sys.stderr)


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


def shows(node, ours_only):
    content = node.get("content")
    if not isinstance(content, dict) or content.get("mod") != MOD:
        return False
    return not ours_only or (content.get("props") or {}).get("panel") is True


def panel_of(layout):
    return next((x for x in nodes(layout) if shows(x, True)), None)


# The block of the layout's tab bar, when it has one: the block `tabs`, as in Herdr's layouts.
def tab_block(layout):
    block = next((x for x in nodes(layout) if x.get("id") == "tabs"), None)
    content = block.get("content") if block else None
    if isinstance(content, dict) and isinstance(content.get("mod"), str) and content["mod"]:
        return block
    return None


def by_id(layout, id):
    return next((x for x in nodes(layout) if x.get("id") == id), None)


# The layout's right-hand panel stack (`panel-body` inside the column `panel`,
# as in Herdr's layouts), or None.
def stack_of(layout):
    column = by_id(layout, COLUMN)
    body = by_id(column, STACK) if column else None
    if body is None or body.get("type") != "stack" or not isinstance(body.get("children"), list):
        return None
    return body


# Takes the blocks showing the mod out from under `node` (not from under
# `spare`); a stack showing one shows its first child left instead. Returns
# their ids.
def take(node, ours_only, spare=None):
    gone = []
    children = node.get("children")
    if isinstance(children, list):
        kept = []
        for child in children:
            if child is spare:
                kept.append(child)
                continue
            if isinstance(child, dict) and shows(child, ours_only):
                gone.append(child.get("id", ""))
                continue
            if isinstance(child, dict):
                gone += take(child, ours_only, spare)
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
    temp = os.path.join(os.path.dirname(path), f".{os.path.basename(path)}.jenkins")
    with open(temp, "w", encoding="utf-8") as f:
        f.write(text)
    if os.path.exists(path):
        os.chmod(temp, os.stat(path).st_mode & 0o777)
    os.replace(temp, path)


def write(path, layout):
    write_text(path, json.dumps(layout, indent=2, ensure_ascii=False) + "\n")


# --- the button ---------------------------------------------------------------

# The button: the tab bar's own panelButton, as Files', Changes' and GitLab's
# are, so it shows Jenkins in the stack, switches to it from another child,
# hides the panel while Jenkins shows, and looks selected only then. It shows
# while the layout's stack has Jenkins.
def button(block, indent):
    lines = [
        BEGIN,
        f'.when(Boolean(find(body, "{block}")), el => el.child(panelButton("{block}", "Jenkins", "mods/jenkins/icons/jenkins.svg")))',
        END,
    ]
    return "".join(f"{indent}{line}\n" for line in lines)


# The tab bar's syncPanel call with Jenkins in its list while the layout's
# stack has the panel (the tab bar then remembers per space whether it was
# open), and the call as it was, kept to be put back.
def listed(block, indent, was):
    head = SYNC.match(was).group(3)
    lines = [
        LIST_BEGIN,
        f"{WAS}{was}",
        f'this.syncPanel({head}[...enabled, ...(find(body, "{block}") ? ["{block}"] : [])]);',
        LIST_END,
    ]
    return "".join(f"{indent}{line}\n" for line in lines)


# Where the lines marked by `begin` and `end` are in a main.js: (start, end)
# of them, or None. Marker lines without their pair are an error, which
# `fail` reports.
def marked(text, where, begin=BEGIN_TAG, end=END):
    lines = text.splitlines(keepends=True)
    begins = [i for i, l in enumerate(lines) if l.strip().startswith(begin)]
    ends = [i for i, l in enumerate(lines) if l.strip().startswith(end)]
    if not begins and not ends:
        return None
    if len(begins) != 1 or len(ends) != 1 or ends[0] < begins[0]:
        fail(f"{where} has Jenkins marker lines ({begin}) that do not pair up: fix them by hand ({BY_HAND}); left alone")
    start = sum(len(l) for l in lines[:begins[0]])
    stop = sum(len(l) for l in lines[:ends[0] + 1])
    return start, stop


# The tab bar's own syncPanel call kept between the list's markers, with its
# indent: (indent, line).
def kept_line(text, span, where):
    lines = text[span[0]:span[1]].splitlines()
    indent = re.match(r"[ \t]*", lines[0]).group(0)
    was = next((l.lstrip()[len(WAS):] for l in lines[1:] if l.lstrip().startswith(WAS)), None)
    if was is None or not SYNC.match(was):
        fail(f"{where} has Jenkins marker lines ({LIST_TAG}) without the line they replaced: fix them by hand ({BY_HAND}); left alone")
    return indent, was


def without_button(text, where):
    span = marked(text, where, LIST_TAG, LIST_END)
    if span is not None:
        indent, was = kept_line(text, span, where)
        text = text[:span[0]] + f"{indent}{was}\n" + text[span[1]:]
    span = marked(text, where)
    return text if span is None else text[:span[0]] + text[span[1]:]


# A Jenkins button that is not between the markers (one put in by hand).
def unmarked(text, where):
    return re.search(r'"panel-jenkins|panelButton\(\s*"jenkins', without_button(text, where)) is not None


# main.js with the button for `block` in it: the same text when it is there
# already. Fails when there is nowhere to put it.
def with_button(text, block, where):
    # The list first; the button's place is found in what that leaves.
    span = marked(text, where, LIST_TAG, LIST_END)
    if span is not None:
        indent, was = kept_line(text, span, where)
        text = text[:span[0]] + listed(block, indent, was) + text[span[1]:]
    else:
        found = list(SYNC.finditer(text))
        if len(found) != 1 or not text.startswith("\n", found[0].end()):
            fail(f"{where} is not like Herdr's tab bar: no single line this.syncPanel(…, enabled); (which remembers the open panel per space) "
                 f"to add Jenkins to; {BY_HAND}, then run this again. Nothing was changed")
        m = found[0]
        text = text[:m.start()] + listed(block, m.group(1), m.group(2)) + text[m.end() + 1:]
    span = marked(text, where)
    if span is not None:
        indent = re.match(r"[ \t]*", text[span[0]:]).group(0)
        return text[:span[0]] + button(block, indent) + text[span[1]:]
    at = [m.start() for m in re.finditer(re.escape(ANCHOR), text)]
    if len(at) != 1:
        fail(f"{where} is not like Herdr's tab bar: no single place before its window buttons ({ANCHOR}) "
             f"to put a Jenkins button; {BY_HAND}, then run this again. Nothing was changed")
    missing = [n for n, pattern in NEEDS.items() if not re.search(pattern, text)]
    if missing:
        fail(f"{where} is not like Herdr's tab bar: the button needs {', '.join(missing)} there; "
             f"{BY_HAND}, then run this again. Nothing was changed")
    line = text.rfind("\n", 0, at[0]) + 1
    indent = re.match(r"[ \t]*", text[line:]).group(0)
    if text[line:at[0]].strip():
        # The anchor is not at the start of its line: start the button on a line of its own.
        return text[:at[0]] + "\n" + button(block, indent) + indent + text[at[0]:]
    return text[:line] + button(block, indent) + text[line:]


# --- the clone of Herdr's tab bar ---------------------------------------------

def digest(text):
    return hashlib.sha256(text.encode("utf-8")).hexdigest()


def app_written(file):
    return file == "jsconfig.json" or file.endswith(".d.ts")


# The files of a clone of Herdr's tab bar, as `mod.clone` makes one: its asset
# paths rewritten to the copy's folder, `cloned_from` in mod.json. main.js
# starts with the line that says this installer made it.
def clone_files(mods, version):
    stock = os.path.join(mods, STOCK)
    files = {}
    for root, dirs, names in os.walk(stock):
        dirs[:] = [d for d in dirs if not d.startswith(".")]
        for file in names:
            if app_written(file) or file.startswith("."):
                continue
            path = os.path.join(root, file)
            rel = os.path.relpath(path, stock)
            with open(path, "rb") as f:
                data = f.read()
            if file.endswith(".js"):
                data = data.decode("utf-8").replace(f"mods/{STOCK}/", f"mods/{CLONE}/").encode("utf-8")
            files[rel] = data
    if "main.js" not in files:
        fail(f"no main.js in {stock}, Herdr's tab bar: is this an app home?")
    meta = load(os.path.join(stock, "mod.json"))
    meta = meta if isinstance(meta, dict) else {}
    meta["cloned_from"] = {"name": STOCK, **({"version": version} if version else {})}
    files["mod.json"] = (json.dumps(meta, indent=2, ensure_ascii=False) + "\n").encode("utf-8")
    main = files["main.js"].decode("utf-8")
    files["main.js"] = (f"{CLONED}{digest(main)}\n{main}").encode("utf-8")
    return files


# Whether mods/tabs-jenkins is this installer's, and whether its main.js,
# but for the button, is still as cloned.
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
    for root, dirs, names in os.walk(folder):
        dirs[:] = [d for d in dirs if not d.startswith(".")]
        for file in names:
            rel = os.path.relpath(os.path.join(root, file), folder)
            if app_written(file) or file.startswith("."):
                continue
            if rel not in files:
                return False
            with open(os.path.join(root, file), "rb") as f:
                if f.read() != files[rel]:
                    return False
    return all(os.path.isfile(os.path.join(folder, rel)) for rel in files)


# Checks a tab bar's files with `herdr-app check` in a folder beside the mods
# (the app takes no folder starting with a dot for a mod), when there is an app.
def check(mods, files, where):
    if not app:
        return
    stage = os.path.join(mods, ".jenkins-tabs-check")
    shutil.rmtree(stage, ignore_errors=True)
    try:
        for rel, data in files.items():
            os.makedirs(os.path.dirname(os.path.join(stage, rel)), exist_ok=True)
            with open(os.path.join(stage, rel), "wb") as f:
                f.write(data)
        r = subprocess.run([app, "check", stage], capture_output=True, text=True)
        if r.returncode != 0:
            tail = "\n".join((r.stdout + r.stderr).strip().splitlines()[-3:])
            fail(f"herdr-app check refused {where} with the Jenkins button: {tail}; nothing was changed")
    finally:
        shutil.rmtree(stage, ignore_errors=True)


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


# Puts a whole new clone in place of mods/tabs-jenkins with one rename.
def put_clone(mods, files):
    stage = os.path.join(mods, f".{CLONE}.jenkins-install")
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


def show_by_hand(block):
    return (f"herdr-app do layout.select --stack {STACK} --child {block}, then herdr-app do layout.show --id {COLUMN}, "
            f"shows it (layout.hide --id {COLUMN} hides it again)")


def add(n):
    path, layout = open_layout(n)
    changed = False
    body = stack_of(layout)
    if body is None:
        fail(f"layout {n} has no right-hand panel like Herdr's layouts (a stack {STACK} in a column {COLUMN}, "
             f"where Files and Changes are), so there is no place for Jenkins beside them: left alone")
    # An earlier install's panel, a block of its own at the end of the top row, moves into the stack.
    moved = take(layout, True, body)
    if moved:
        changed = True
    there = next((c for c in body["children"] if isinstance(c, dict) and shows(c, False)), None)
    if there is not None:
        say(f"layout {n} shows Jenkins in its right-hand panel already (block {there.get('id', '?')}): left as it is")
    else:
        taken = {x.get("id") for x in nodes(layout)}
        block, i = BLOCK, 2
        while block in taken:
            block, i = f"{BLOCK}-{i}", i + 1
        there = {"id": block, **PANEL}
        body["children"].append(there)
        changed = True
    panel = there if shows(there, True) else None
    # The button, worked out whole before anything is written.
    put = None  # ("clone", files) or ("main", path, text, whether it had the button)
    tabs = tab_block(layout)
    if panel is None:
        say(f"layout {n}: no Jenkins button in its tab bar, which goes only with the panel install.sh --panel right puts in (props panel)")
    elif tabs is None:
        say(f"layout {n} has no tab bar (a block tabs): no Jenkins button; {show_by_hand(panel['id'])}")
    else:
        bar = tabs["content"]["mod"]
        if bar == STOCK and shipped_mod(STOCK) or bar == CLONE:
            state = clone_state(mods)
            if state == "other":
                fail(f"{os.path.join(mods, CLONE)} is another mod, not the copy of Herdr's tab bar this installer makes: "
                     f"rename it, or point the layout's tabs block at a copy of your own and run this again. Nothing was changed")
            if state in ("none", "as-cloned"):
                # A fresh clone of Herdr's tab bar as it is now, with the button.
                files = clone_files(mods, version)
                main = files["main.js"].decode("utf-8")
                first, _, rest = main.partition("\n")
                files["main.js"] = f"{first}\n{with_button(rest, panel['id'], f'mods/{STOCK}/main.js')}".encode("utf-8")
                if state == "none" or not same_files(os.path.join(mods, CLONE), files):
                    check(mods, files, f"a copy of mods/{STOCK}")
                    put = ("clone", files)
            else:
                # Changed since it was made (another mod's button, say): only the button is ours.
                where = f"mods/{CLONE}/main.js"
                main_path = os.path.join(mods, CLONE, "main.js")
                text = read(main_path)
                if unmarked(text, where):
                    say(f"{where} has a Jenkins button already, not between this installer's markers: left as it is")
                else:
                    new = with_button(text, panel["id"], where)
                    if new != text:
                        files = mod_files(os.path.join(mods, CLONE))
                        files["main.js"] = new.encode("utf-8")
                        check(mods, files, where)
                        put = ("main", main_path, new, marked(text, where) is not None)
            if bar != CLONE:
                tabs["content"]["mod"] = CLONE
                changed = True
            bar = CLONE
        elif shipped_mod(bar):
            say(f"layout {n}: its tab bar is Herdr's {bar}, which this installer cannot put a button into: no Jenkins button; "
                f"{show_by_hand(panel['id'])}")
            bar = None
        else:
            where = f"mods/{bar}/main.js"
            main_path = os.path.join(mods, bar, "main.js")
            if not os.path.isfile(main_path):
                say(f"layout {n}: its tab bar {bar} has no {where}: no Jenkins button")
                bar = None
            else:
                text = read(main_path)
                if unmarked(text, where):
                    say(f"{where} has a Jenkins button already, not between this installer's markers: left as it is")
                    bar = None
                else:
                    new = with_button(text, panel["id"], where)
                    if new != text:
                        files = mod_files(os.path.join(mods, bar))
                        files["main.js"] = new.encode("utf-8")
                        check(mods, files, where)
                        put = ("main", main_path, new, marked(text, where) is not None)
        # The tab bar first: its button shows only while the layout has the panel.
        if put and put[0] == "clone":
            fresh = not os.path.isdir(os.path.join(mods, CLONE))
            put_clone(mods, put[1])
            say(f"{'made' if fresh else 'updated'} mods/{CLONE}, a copy of Herdr's tab bar as it is now, with a Jenkins button")
        elif put:
            write_text(put[1], put[2])
            had = put[3]
            say(f"{'updated the' if had else 'put a'} Jenkins button {'in' if had else 'into'} mods/{bar}/main.js")
        elif bar:
            say(f"mods/{bar} has the Jenkins button already")
        if bar:
            say(f"the tab bar's Jenkins button shows Jenkins in the right-hand panel, as Files and Changes show theirs; {AGAIN}")
    if changed:
        write(path, layout)
        if moved:
            say(f"layout {n}: took out the Jenkins panel of an earlier install at the end of its top row ({', '.join(moved)})")
        if panel is not None:
            say(f"layout {n}: Jenkins is in its right-hand panel (stack {STACK}, block {panel['id']}), beside Files and Changes")
        if tabs is not None and tabs["content"]["mod"] == CLONE:
            say(f"layout {n}: its tab bar is mods/{CLONE} now")


# Takes the panel out of the layouts named, and the button out of their tab
# bars, unless a layout that keeps a panel shows that tab bar too.
def remove(names, ours_only):
    edited = {}
    bars = set()
    for n in names:
        if shipped(n) and not ours_only:
            continue
        path, layout = open_layout(n)
        gone = take(layout, ours_only)
        tabs = tab_block(layout)
        if tabs is not None:
            bars.add(tabs["content"]["mod"])
        edited[n] = (path, layout, gone)
    # Every personal layout as it will be, to see which tab bars still need the button.
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
    # Each tab bar's main.js without the button, worked out first.
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
            say(f"{where} has a Jenkins button that this installer did not put in: left as it is (it shows only while a layout has a block jenkins)")
    # The copy this installer made goes back to Herdr's own while it is as cloned.
    back = CLONE in bars and clone_state(mods) == "as-cloned" and os.path.isfile(os.path.join(mods, STOCK, "main.js"))
    if CLONE in bars and not back:
        say(f"mods/{CLONE} was changed since this installer made it: the layout keeps it")
    for n, (path, layout, gone) in edited.items():
        tabs = tab_block(layout)
        if back and tabs is not None and tabs["content"]["mod"] == CLONE and panel_of(layout) is None:
            tabs["content"]["mod"] = STOCK
            gone.append(f"tab bar {CLONE}, back to {STOCK}")
            edited[n] = (path, layout, gone)
    # The layouts first, then the tab bars: the button shows only with the panel.
    for n, (path, layout, gone) in edited.items():
        if gone:
            write(path, layout)
            say(f"layout {n}: took Jenkins out ({', '.join(gone)})")
        elif name:
            say(f"layout {n} has no Jenkins panel: left as it is")
    for bar, (main_path, new) in texts.items():
        if bar == CLONE and back and not users(CLONE):
            continue
        write_text(main_path, new)
        say(f"took the Jenkins button out of mods/{bar}/main.js")
    if back and not users(CLONE) and os.path.isdir(os.path.join(mods, CLONE)):
        shutil.rmtree(os.path.join(mods, CLONE))
        say(f"removed mods/{CLONE}, the copy of Herdr's tab bar this installer made")


if action == "add":
    add(names[0])
elif action == "off":
    remove(names, True)
else:
    remove(names, False)
