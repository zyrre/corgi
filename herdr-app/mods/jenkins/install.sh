#!/bin/sh
# Installs the Jenkins mod into the Herdr app, or updates it. It stands on its
# own: it needs no Corgi, and Corgi's herdr-app/install.sh does not run it.
#
#   herdr-app/mods/jenkins/install.sh [--home DIR]
#   herdr-app/mods/jenkins/install.sh --uninstall [--forget] [--home DIR]
#
# It copies the mod's files (main.js, api.js, util.js, ui.js, mod.json and
# icons/) to mods/jenkins in the app's home ($HERDR_APP_HOME, else
# ~/.config/herdr-app; --home DIR), checked first with `herdr-app check` and
# swapped in with one rename, and only when they differ from what is there:
# the app reloads a mod the moment its files change. It changes no layout and
# no key: the mod shows where you put it (see README.md).
#
#   --uninstall   removes mods/jenkins. The settings, which hold the token
#                 (jenkins/settings.json in the app's home), stay
#   --forget      with --uninstall: removes the settings too
#
#   HERDR_APP_BIN=<path>   the herdr-app to check the mod with
set -u

say() { printf 'jenkins mod: %s\n' "$*" >&2; }
die() {
  say "$*"
  exit 1
}

home_dir='' uninstall='' forget=''
while [ $# -gt 0 ]; do
  case $1 in
    --home)
      [ $# -ge 2 ] || die "--home needs a folder"
      home_dir=$2
      shift
      ;;
    --uninstall) uninstall=1 ;;
    --forget) forget=1 ;;
    -h | --help)
      sed -n '2,20p' "$0"
      exit 0
      ;;
    *) die "unknown argument $1 (see --help)" ;;
  esac
  shift
done
[ -z "$forget" ] || [ -n "$uninstall" ] || die "--forget goes with --uninstall"

here=$(cd "$(dirname "$0")" && pwd -P) || die "could not find this script's folder"
[ -f "$here/main.js" ] || die "no main.js beside this script"
home_dir=${home_dir:-${HERDR_APP_HOME:-${HOME:?}/.config/herdr-app}}
mods="$home_dir/mods"
target="$mods/jenkins"
mark='// Jenkins in the Herdr app'

# Only ever replace or remove a folder that holds this mod.
ours() { [ -f "$1/main.js" ] && head -n 1 "$1/main.js" | grep -qF "$mark"; }

if [ -n "$uninstall" ]; then
  if [ -d "$target" ]; then
    ours "$target" || die "$target is another mod: left alone"
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
  say "up to date in $target"
  exit 0
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
say "installed in $target"
say "show it with: herdr-app do mod.open --name jenkins   (or as a tab: herdr-app do tab.new --name Jenkins --mod jenkins)"
