#!/bin/sh
# Installs the Jenkins mod into the Herdr app, or updates it. It stands on its
# own: it needs no Corgi, and Corgi's herdr-app/install.sh does not run it.
#
#   herdr-app/mods/jenkins/install.sh [--home DIR] [--panel right [--layout NAME]]
#   herdr-app/mods/jenkins/install.sh --panel off [--layout NAME] [--home DIR]
#   herdr-app/mods/jenkins/install.sh --uninstall [--forget] [--layout NAME] [--home DIR]
#
# It copies the mod's files (main.js, data.js, api.js, util.js, ui.js, mod.json and
# icons/) to mods/jenkins in the app's home ($HERDR_APP_HOME, else
# ~/.config/herdr-app; --home DIR), checked first with `herdr-app check` and
# swapped in with one rename, and only when they differ from what is there:
# the app reloads a mod the moment its files change. Without --panel it
# changes no layout and no key: the mod shows where you put it (README.md).
#
#   --panel right  also shows it in the layout's right-hand panel, under
#                  the tab bar, as one more child of its stack panel-body
#                  beside Files and Changes (and GitLab): one of them shows
#                  at a time, and the tab bar keeps its width. Again:
#                  nothing changes. An earlier version's panel, a block at
#                  the right end of the top row, is moved into the stack.
#                  It also puts a Jenkins button, which shows Jenkins in
#                  that panel as Files and Changes show theirs, into the
#                  layout's tab bar beside them: into a copy of Herdr's tab
#                  bar (mods/tabs-jenkins, which the layout's tabs block
#                  then shows) when the layout shows Herdr's own, or between
#                  marker lines into the copy it shows (such as
#                  herdr-gitlab's tabs-gitlab), with Jenkins in the list
#                  of panels its syncPanel remembers per space.
#                  If another mod's install later replaces that copy, run
#                  --panel right again to bring the button back
#   --panel off    takes that panel and its button out of the layout again
#                  (and points it back at Herdr's tab bar while the copy is
#                  as this script made it); the mod stays
#   --layout NAME  the layout to change (layouts/NAME.json): by default the
#                  one the window shows. Herdr's own layouts (main,
#                  terminals, win98) are refused, since Herdr replaces them
#                  on every start: clone one first (herdr-app do layout.clone)
#   --uninstall   removes mods/jenkins, and every block showing it and the
#                 button from your layouts (only from NAME's, with
#                 --layout), as --panel off does. The settings,
#                 which hold the token (jenkins/settings.json in the app's
#                 home), stay
#   --forget      with --uninstall: removes the settings too
#
# A layout is changed in place, keeping all else in it as it was, which
# needs python3 (install-layout.py, beside this script) to read and write
# its JSON. The app shows a changed layout
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
      sed -n '2,51p' "$0"
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

# Changes layouts in place, and the tab bar their `tabs` block shows
# (install-layout.py says how): `add` puts the panel into one and a button
# for it into its tab bar, `off` takes the panel this script put in and the
# button out of one, `all` takes every block showing the mod and the button
# out of every layout of the person's (or of --layout's), for the uninstall.
# Herdr's own layouts and mods are never changed.
edit_layouts() {
  command -v python3 >/dev/null 2>&1 ||
    die "changing a layout needs python3, which is not on PATH: add the block by hand (README.md has it)"
  [ -f "$here/install-layout.py" ] || die "no install-layout.py beside this script"
  python3 -I "$here/install-layout.py" "$1" "$home_dir" "$layout" "${2:-}"
}

if [ -n "$uninstall" ]; then
  if [ -d "$target" ]; then
    ours "$target" || die "$target is another mod: left alone"
  fi
  # The layouts first: a block left naming a mod that is gone shows an error.
  # (python3 is needed only when one does, or shows the tab bar this script made.)
  if grep -qs -e '"mod": *"jenkins"' -e '"mod": *"tabs-jenkins"' "$home_dir/layouts"/*.json; then edit_layouts all || exit 1; fi
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
  for file in main.js data.js api.js util.js ui.js mod.json; do
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
  edit_layouts add "${HERDR_APP_BIN:-$(command -v herdr-app 2>/dev/null || true)}" || exit 1
else
  say "show it with: install.sh --panel right, herdr-app do mod.open --name jenkins, or herdr-app do tab.new --name Jenkins --mod jenkins"
fi
