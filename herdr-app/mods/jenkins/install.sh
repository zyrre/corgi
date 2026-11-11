#!/bin/sh
# Installs the Jenkins mod into the Herdr app, or updates it. It stands on its
# own: it needs no Corgi, and Corgi's herdr-app/install.sh does not run it.
#
#   herdr-app/mods/jenkins/install.sh [--home DIR] [--panel right [--layout NAME]]
#   herdr-app/mods/jenkins/install.sh --panel off [--layout NAME] [--home DIR]
#   herdr-app/mods/jenkins/install.sh --uninstall [--forget] [--layout NAME] [--home DIR]
#
# It copies the mod's files (main.js, api.js, util.js, ui.js, mod.json and
# icons/) to mods/jenkins in the app's home ($HERDR_APP_HOME, else
# ~/.config/herdr-app; --home DIR), checked first with `herdr-app check` and
# swapped in with one rename, and only when they differ from what is there:
# the app reloads a mod the moment its files change. Without --panel it
# changes no layout and no key: the mod shows where you put it (README.md).
#
#   --panel right  also shows it as a panel on the right of the window: a
#                  block at the right end of the layout's top row, after
#                  everything that was there (the Files and Changes panel
#                  stays where it was), full height, in the sidebar's colour.
#                  Again: nothing changes. Drag its edge to size it
#   --panel off    takes that panel out of the layout again; the mod stays
#   --layout NAME  the layout to change (layouts/NAME.json): by default the
#                  one the window shows. Herdr's own layouts (main,
#                  terminals, win98) are refused, since Herdr replaces them
#                  on every start: clone one first (herdr-app do layout.clone)
#   --uninstall   removes mods/jenkins, and every block showing it from your
#                 layouts (only from NAME's, with --layout). The settings,
#                 which hold the token (jenkins/settings.json in the app's
#                 home), stay
#   --forget      with --uninstall: removes the settings too
#
# A layout is changed in place, keeping all else in it as it was, which
# needs python3 to read and write its JSON. The app shows a changed layout
# within a second.
#
#   HERDR_APP_BIN=<path>   the herdr-app to check the mod with
set -u

say() { printf 'jenkins mod: %s\n' "$*" >&2; }
die() {
  say "$*"
  exit 1
}

home_dir='' uninstall='' forget='' panel='' layout=''
while [ $# -gt 0 ]; do
  case $1 in
    --home)
      [ $# -ge 2 ] || die "--home needs a folder"
      home_dir=$2
      shift
      ;;
    --panel)
      [ $# -ge 2 ] || die "--panel needs a side: right, or off"
      case $2 in
        right | off) panel=$2 ;;
        *) die "--panel takes right or off, not $2" ;;
      esac
      shift
      ;;
    --layout)
      [ $# -ge 2 ] || die "--layout needs a layout's name"
      layout=$2
      shift
      ;;
    --uninstall) uninstall=1 ;;
    --forget) forget=1 ;;
    -h | --help)
      sed -n '2,40p' "$0"
      exit 0
      ;;
    *) die "unknown argument $1 (see --help)" ;;
  esac
  shift
done
[ -z "$forget" ] || [ -n "$uninstall" ] || die "--forget goes with --uninstall"
[ -z "$uninstall" ] || [ -z "$panel" ] || die "--uninstall takes the panel out by itself: no --panel"
[ -z "$layout" ] || [ -n "$panel$uninstall" ] || die "--layout goes with --panel or --uninstall"
case $layout in
  */* | .* ) die "--layout takes a layout's name, such as corgi, not $layout" ;;
esac

here=$(cd "$(dirname "$0")" && pwd -P) || die "could not find this script's folder"
[ -f "$here/main.js" ] || die "no main.js beside this script"
home_dir=${home_dir:-${HERDR_APP_HOME:-${HOME:?}/.config/herdr-app}}
mods="$home_dir/mods"
target="$mods/jenkins"
mark='// Jenkins in the Herdr app'

# Only ever replace or remove a folder that holds this mod.
ours() { [ -f "$1/main.js" ] && head -n 1 "$1/main.js" | grep -qF "$mark"; }

# Changes layouts in place: `add` puts the panel into one, `off` takes the
# panel this script put in out of one, `all` takes every block showing the
# mod out of every layout of the person's (or of --layout's), for the
# uninstall. Herdr's own layouts are never changed. Each is written whole
# with one rename, keeping its mode and all else in it.
edit_layouts() {
  command -v python3 >/dev/null 2>&1 ||
    die "changing a layout needs python3, which is not on PATH: add the block by hand (README.md has it)"
  python3 -I - "$1" "$home_dir" "$layout" <<'PY'
import json, os, sys

action, home, name = sys.argv[1:4]
MOD = "jenkins"
# The block of the panel: full height at the right end of the top row, in the sidebar's colour.
PANEL = {"type": "block", "content": {"mod": MOD, "props": {"panel": True}},
         "size": {"px": 320}, "min": 260, "max": 640, "background": "chrome"}


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


# Takes the blocks showing the mod out from under `node`; a stack showing one
# shows its first child left instead. Returns their ids.
def take(node, ours_only):
    gone = []
    children = node.get("children")
    if isinstance(children, list):
        kept = []
        for child in children:
            if isinstance(child, dict) and shows(child, ours_only):
                gone.append(child.get("id", ""))
                continue
            if isinstance(child, dict):
                gone += take(child, ours_only)
            kept.append(child)
        node["children"] = kept
        if node.get("active") in gone:
            first = next((c.get("id") for c in kept if isinstance(c, dict) and c.get("id")), None)
            if first:
                node["active"] = first
            else:
                node.pop("active", None)
    return gone


def write(path, layout):
    temp = os.path.join(os.path.dirname(path), f".{os.path.basename(path)}.jenkins")
    with open(temp, "w", encoding="utf-8") as f:
        f.write(json.dumps(layout, indent=2, ensure_ascii=False) + "\n")
    os.chmod(temp, os.stat(path).st_mode & 0o777)
    os.replace(temp, path)


folder = os.path.join(home, "layouts")
installed = load(os.path.join(home, "installed.json"))


def shipped(n):
    if isinstance(installed, dict):
        return f"layout:{n}" in installed
    return n in ("main", "terminals", "win98")


if not name and action != "all":
    name = (load(os.path.join(home, "shared.json")) or {}).get("layout")
    if not isinstance(name, str) or not name:
        fail("which layout? Name it with --layout (layouts/<name>.json)")
if name:
    names = [name]
else:
    names = sorted(f[:-5] for f in os.listdir(folder) if f.endswith(".json") and not f.startswith(".")) if os.path.isdir(folder) else []

for n in names:
    path = os.path.join(folder, f"{n}.json")
    if shipped(n):
        if action == "all":
            continue
        fail(f"{n} is Herdr's own layout, which Herdr replaces on every start: clone it first "
             f"(herdr-app do layout.clone '{{\"name\":\"{n}\",\"as\":\"mine\"}}') and name the clone with --layout")
    if not os.path.isfile(path):
        fail(f"no layout {n} in {folder}")
    layout = load(path)
    if not isinstance(layout, dict):
        fail(f"{path} is not a layout's JSON: left alone")
    if action == "add":
        there = next((x for x in nodes(layout) if shows(x, False)), None)
        if there is not None:
            say(f"layout {n} shows Jenkins already (block {there.get('id', '?')}): left as it is")
            continue
        if layout.get("type") != "row" or not isinstance(layout.get("children"), list):
            fail(f"layout {n} is not a row of columns, so it has no right side for a panel: left alone")
        taken = {x.get("id") for x in nodes(layout)}
        block, i = "jenkins", 2
        while block in taken:
            block, i = f"jenkins-{i}", i + 1
        layout["children"].append({"id": block, **PANEL})
        write(path, layout)
        say(f"layout {n}: Jenkins is a panel on the right now (block {block}); drag its edge to size it")
    else:
        gone = take(layout, action == "off")
        if gone:
            write(path, layout)
            say(f"layout {n}: took Jenkins out (block {', '.join(gone)})")
        elif name:
            say(f"layout {n} has no Jenkins panel: left as it is")
PY
}

if [ -n "$uninstall" ]; then
  if [ -d "$target" ]; then
    ours "$target" || die "$target is another mod: left alone"
  fi
  # The layouts first: a block left naming a mod that is gone shows an error.
  # (python3 is needed only when one does.)
  if grep -qs '"mod": *"jenkins"' "$home_dir/layouts"/*.json; then edit_layouts all || exit 1; fi
  if [ -d "$target" ]; then
    rm -rf "$target" || die "could not remove $target"
    say "removed $target"
  else
    say "no Jenkins mod in $mods"
  fi
  if [ -n "$forget" ]; then
    rm -rf "$home_dir/jenkins" || die "could not remove $home_dir/jenkins"
    say "removed the settings and the token in $home_dir/jenkins"
  elif [ -e "$home_dir/jenkins/settings.json" ]; then
    say "the settings and the token stay in $home_dir/jenkins/settings.json (--forget removes them)"
  fi
  exit 0
fi

if [ "$panel" = off ]; then
  edit_layouts off || exit 1
  exit 0
fi

# Copies the mod into place when it differs from what is there.
install_mod() {
  [ -d "$mods" ] || die "no Herdr app home at $home_dir: start Herdr once first, or name it with --home"
  if [ -e "$target" ] && ! ours "$target"; then
    die "$target is another mod: rename one of them first"
  fi

  stage="$mods/.jenkins-install"
  rm -rf "$stage"
  mkdir -p "$stage/jenkins" || die "could not create $stage"
  trap 'rm -rf "$stage"' EXIT
  for file in main.js api.js util.js ui.js mod.json; do
    cp "$here/$file" "$stage/jenkins/$file" || die "could not copy $file"
  done
  cp -R "$here/icons" "$stage/jenkins/icons" || die "could not copy the icons"

  # The same files as installed (what the app writes into a mod folder aside)?
  if [ -d "$target" ] && diff -r -x jsconfig.json -x '*.d.ts' "$stage/jenkins" "$target" >/dev/null 2>&1; then
    rm -rf "$stage"
    say "up to date in $target"
    return 0
  fi

  app=${HERDR_APP_BIN:-$(command -v herdr-app 2>/dev/null || true)}
  if [ -n "$app" ]; then
    "$app" check "$stage/jenkins" >/dev/null || die "herdr-app check refused the mod: $("$app" check "$stage/jenkins" 2>&1 | tail -n 3)"
    rm -f "$stage/jenkins/jsconfig.json" "$stage/jenkins"/*.d.ts
  else
    say "no herdr-app on PATH: installed without checking the mod"
  fi

  rm -rf "$target" || die "could not replace $target"
  mv "$stage/jenkins" "$target" || die "could not put the mod in $target"
  rm -rf "$stage"
  say "installed in $target"
}

install_mod
if [ "$panel" = right ]; then
  edit_layouts add || exit 1
else
  say "show it with: install.sh --panel right, herdr-app do mod.open --name jenkins, or herdr-app do tab.new --name Jenkins --mod jenkins"
fi
