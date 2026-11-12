#!/bin/sh
# Installs the Media mod into the Herdr app, or updates it. It stands on its
# own: it needs no Corgi, and Corgi's herdr-app/install.sh does not run it.
#
#   herdr-app/mods/media/install.sh [--home DIR] [--folder DIR] [--panel right [--layout NAME]]
#   herdr-app/mods/media/install.sh --panel off [--layout NAME] [--home DIR]
#   herdr-app/mods/media/install.sh --uninstall [--layout NAME] [--home DIR]
#
# It copies the mod's files (main.js, store.js, items.js, mod.json and
# icons/) to mods/media in the app's home ($HERDR_APP_HOME, else
# ~/.config/herdr-app; --home DIR), checked first with `herdr-app check` and
# swapped in with one rename, and only when they differ from what is there:
# the app reloads a mod the moment its files change. Without --panel it
# changes no layout and no key: the mod shows where you put it (README.md).
#
#   --panel right  also shows it in the layout's right-hand panel, under
#                  the tab bar, as one more child of its stack panel-body
#                  beside Files and Changes (and GitLab, Jenkins): one of
#                  them shows at a time, and the tab bar keeps its width.
#                  It adds a watcher too, a floating block that draws
#                  nothing and keeps the drop folder watched while the
#                  panel has never been shown, and a Media button, with the
#                  count of what came since the panel was last seen, to the
#                  layout's tab bar: into a copy of Herdr's tab bar
#                  (mods/tabs-media, which the layout's tabs block then
#                  shows) when the layout shows Herdr's own, or between
#                  marker lines into the copy it shows (herdr-gitlab's
#                  tabs-gitlab, the Jenkins mod's tabs-jenkins, Corgi's
#                  tabs-corgi). Again: nothing changes. If another mod's
#                  install later replaces that copy, run --panel right
#                  again to bring the button back
#   --panel off    takes the panel, the watcher and the button out of the
#                  layout again (and points it back at Herdr's tab bar while
#                  the copy is as this script made it); the mod stays
#   --layout NAME  the layout to change (layouts/NAME.json): by default the
#                  one the window shows. Herdr's own layouts (main,
#                  terminals, win98) are refused, since Herdr replaces them
#                  on every start: clone one first (herdr-app do layout.clone)
#   --folder DIR   the drop folder agents copy files into (by default
#                  ~/.local/share/herdr-media), kept in media/settings.json
#                  of the app's home. It must be one agents may write to,
#                  so never inside the app's home
#   --uninstall    removes mods/media, and every block showing it and the
#                  button from your layouts (only from NAME's, with
#                  --layout), as --panel off does, and the mod's copies of
#                  what it showed (media/ in the app's home). The drop
#                  folder and its files stay
#
# A layout is changed in place, keeping all else in it as it was, which
# needs python3 (install-layout.py, beside this script) to read and write
# its JSON. The app shows a changed layout within a second.
#
#   HERDR_APP_BIN=<path>   the herdr-app to check the mod with
set -u

say() { printf 'media mod: %s\n' "$*" >&2; }
die() {
  say "$*"
  exit 1
}

home_dir='' uninstall='' panel='' layout='' folder=''
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
    --folder)
      [ $# -ge 2 ] || die "--folder needs a folder"
      folder=$2
      shift
      ;;
    --uninstall) uninstall=1 ;;
    -h | --help)
      sed -n '2,52p' "$0"
      exit 0
      ;;
    *) die "unknown argument $1 (see --help)" ;;
  esac
  shift
done
[ -z "$uninstall" ] || [ -z "$panel" ] || die "--uninstall takes the panel out by itself: no --panel"
[ -z "$uninstall" ] || [ -z "$folder" ] || die "--folder does not go with --uninstall"
[ "$panel" != off ] || [ -z "$folder" ] || die "--folder does not go with --panel off"
[ -z "$layout" ] || [ -n "$panel$uninstall" ] || die "--layout goes with --panel or --uninstall"
case $layout in
  */* | .*) die "--layout takes a layout's name, such as corgi, not $layout" ;;
esac

here=$(cd "$(dirname "$0")" && pwd -P) || die "could not find this script's folder"
[ -f "$here/main.js" ] || die "no main.js beside this script"
home_dir=${home_dir:-${HERDR_APP_HOME:-${HOME:?}/.config/herdr-app}}
mods="$home_dir/mods"
target="$mods/media"
mark='// Media in the Herdr app'

# Only ever replace or remove a folder that holds this mod.
ours() { [ -f "$1/main.js" ] && head -n 1 "$1/main.js" | grep -qF "$mark"; }

# Changes layouts in place, and the tab bar their `tabs` block shows
# (install-layout.py says how). Herdr's own layouts and mods are never changed.
edit_layouts() {
  command -v python3 >/dev/null 2>&1 ||
    die "changing a layout needs python3, which is not on PATH: add the blocks by hand (README.md has them)"
  [ -f "$here/install-layout.py" ] || die "no install-layout.py beside this script"
  python3 -I "$here/install-layout.py" "$1" "$home_dir" "$layout" "${2:-}"
}

if [ -n "$uninstall" ]; then
  if [ -d "$target" ]; then
    ours "$target" || die "$target is another mod: left alone"
  fi
  # The layouts first: a block left naming a mod that is gone shows an error.
  if grep -qs -e '"mod": *"media"' -e '"mod": *"tabs-media"' "$home_dir/layouts"/*.json; then edit_layouts all || exit 1; fi
  if [ -d "$target" ]; then
    rm -rf "$target" || die "could not remove $target"
    say "removed $target"
  else
    say "no Media mod in $mods"
  fi
  if [ -d "$home_dir/media" ]; then
    rm -rf "${home_dir:?}/media" || die "could not remove $home_dir/media"
    say "removed $home_dir/media, the mod's copies of what it showed and its settings; the drop folder stays"
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

  stage="$mods/.media-install"
  rm -rf "$stage"
  mkdir -p "$stage/media" || die "could not create $stage"
  trap 'rm -rf "$stage"' EXIT
  for file in main.js store.js items.js mod.json; do
    cp "$here/$file" "$stage/media/$file" || die "could not copy $file"
  done
  cp -R "$here/icons" "$stage/media/icons" || die "could not copy the icons"

  # The same files as installed (what the app writes into a mod folder aside)?
  if [ -d "$target" ] && diff -r -x jsconfig.json -x '*.d.ts' "$stage/media" "$target" >/dev/null 2>&1; then
    rm -rf "$stage"
    say "up to date in $target"
    return 0
  fi

  app=${HERDR_APP_BIN:-$(command -v herdr-app 2>/dev/null || true)}
  if [ -n "$app" ]; then
    "$app" check "$stage/media" >/dev/null || die "herdr-app check refused the mod: $("$app" check "$stage/media" 2>&1 | tail -n 3)"
    rm -f "$stage/media/jsconfig.json" "$stage/media"/*.d.ts
  else
    say "no herdr-app on PATH: installed without checking the mod"
  fi

  rm -rf "$target" || die "could not replace $target"
  mv "$stage/media" "$target" || die "could not put the mod in $target"
  rm -rf "$stage"
  say "installed in $target"
}

# Names the drop folder in media/settings.json, which the mod reads.
set_folder() {
  # A literal ~ (the shell did not expand it) is the home folder.
  # shellcheck disable=SC2088
  case $folder in
    "~") folder=$HOME ;;
    "~/"*) folder="$HOME/${folder#"~/"}" ;;
    /*) ;;
    *) folder="$(pwd -P)/$folder" ;;
  esac
  app_home=$(cd "$home_dir" 2>/dev/null && pwd -P) || app_home=$home_dir
  case $folder/ in
    "$home_dir"/* | "$app_home"/*) die "the drop folder must be outside the app's home ($home_dir), where agents may not write" ;;
  esac
  command -v python3 >/dev/null 2>&1 || die "--folder needs python3 to write media/settings.json"
  mkdir -p "$home_dir/media" || die "could not create $home_dir/media"
  python3 -I -c 'import json, sys; open(sys.argv[1], "w").write(json.dumps({"folder": sys.argv[2]}) + "\n")' \
    "$home_dir/media/settings.json" "$folder" || die "could not write $home_dir/media/settings.json"
  mkdir -p "$folder" || die "could not create the drop folder $folder"
  say "the drop folder is $folder (media/settings.json)"
}

install_mod
[ -z "$folder" ] || set_folder
if [ "$panel" = right ]; then
  edit_layouts add "${HERDR_APP_BIN:-$(command -v herdr-app 2>/dev/null || true)}" || exit 1
else
  say "show it with: install.sh --panel right, or herdr-app do tab.new --name Media --mod media"
fi
