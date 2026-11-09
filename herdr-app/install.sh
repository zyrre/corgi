#!/bin/sh
# Installs Corgi into the Herdr app: the corgi CLI and the Corgi mod.
#
#   curl -fsSL https://raw.githubusercontent.com/zyrre/corgi/main/herdr-app/install.sh | sh
#   herdr-app/install.sh [--version X.Y.Z] [--home DIR] [--cli-only | --mod-only]
#   herdr-app/install.sh --uninstall [--cli] [--home DIR]
#
# Run again to update. Needs no Herdr TUI and no Herdr TUI plugin.
#
# Without a Corgi checkout around it (piped, as above, or saved on its own),
# it first clones https://github.com/zyrre/corgi into $CORGI_CHECKOUT
# (default ~/.local/share/corgi), or fetches it again, checks out the
# release tag vX.Y.Z (--version; default: the main branch) and runs that
# checkout's copy of this script with the same arguments. A release tag
# does not hold its own checksum pins (they are reviewed after the release),
# so the tag's checkout takes checksums/vX.Y.Z.sha256 from the main branch,
# as `herdr plugin install` reads them there.
#
# In a checkout:
#   - the CLI: scripts/install.sh puts the checkout's version of corgi at
#     target/release/corgi, a release binary verified against the checkout's
#     checksum pins, else built with cargo build --release --locked; then
#     ~/.local/bin/corgi ($CORGI_BIN_DIR, else $XDG_BIN_HOME/corgi) links to
#     it, unless something else is there: a file, or a working link elsewhere
#     (such as the Herdr TUI plugin's), is left alone
#   - the mod: herdr-app/mods/corgi is put together in a hidden folder of the
#     app home ($HERDR_APP_HOME, else ~/.config/herdr-app; --home DIR), with
#     a config.json naming the CLI, checked with `herdr-app check`, and swapped
#     in with one rename, only when it differs from the installed one: the app
#     reloads a mod on every file it sees change
#
#   --version X.Y.Z  the release to install; in a checkout it must be the
#                    checkout's own (herdr-plugin.toml), so nothing else is
#                    ever installed in its place
#   --cli-only       the CLI alone (the mod runs this to install corgi)
#   --mod-only       the mod alone, with no config.json (scripts/install.sh
#                    runs this during `herdr plugin install`)
#   --uninstall      removes the mod; with --cli also the link to this
#                    checkout's corgi (the checkout itself is left)
#
#   CORGI_CHECKOUT=<dir>   where the piped install keeps its checkout
#   CORGI_REPO_URL=<url>   clone from <url> instead of GitHub
#   HERDR_APP_BIN=<path>   the herdr-app to check the mod with
set -u

say() { printf 'corgi app install: %s\n' "$*" >&2; }
die() {
  say "$*"
  exit 1
}

version='' home_dir='' uninstall='' cli='' cli_only='' mod_only=''
parse_args() {
  while [ $# -gt 0 ]; do
    case $1 in
      --version)
        [ $# -ge 2 ] || die "--version needs a version, such as 0.2.4"
        version=${2#v}
        shift
        ;;
      --home)
        [ $# -ge 2 ] || die "--home needs a folder"
        home_dir=$2
        shift
        ;;
      --uninstall) uninstall=1 ;;
      --cli) cli=1 ;;
      --cli-only) cli_only=1 ;;
      --mod-only) mod_only=1 ;;
      -h | --help)
        sed -n '2,48p' "$0" 2>/dev/null || say "see https://github.com/zyrre/corgi#install"
        exit 0
        ;;
      *) die "unknown argument $1 (see --help)" ;;
    esac
    shift
  done
  [ -z "$cli_only" ] || [ -z "$mod_only" ] || die "--cli-only and --mod-only exclude each other"
}
parse_args "$@"

# --- without a checkout: fetch one, and run its copy ------------------------
here=$(cd "$(dirname "$0")" 2>/dev/null && pwd -P) || here=
if [ ! -f "$0" ] || [ -z "$here" ] || [ ! -f "$here/../scripts/install.sh" ] || [ ! -d "$here/mods/corgi" ]; then
  command -v git >/dev/null 2>&1 || die "git is not installed (on macOS: xcode-select --install)"
  checkout=${CORGI_CHECKOUT:-${HOME:?}/.local/share/corgi}
  url=${CORGI_REPO_URL:-https://github.com/zyrre/corgi.git}
  if [ -d "$checkout/.git" ]; then
    say "updating $checkout"
    git -C "$checkout" fetch --quiet --tags --force origin || die "could not fetch $url into $checkout"
  elif [ -n "$uninstall" ]; then
    die "nothing to uninstall: there is no Corgi checkout at $checkout"
  else
    say "cloning $url into $checkout"
    mkdir -p "$(dirname "$checkout")" || die "could not create $(dirname "$checkout")"
    git clone --quiet "$url" "$checkout" || die "could not clone $url into $checkout"
  fi
  main=$(git -C "$checkout" rev-parse --abbrev-ref origin/HEAD 2>/dev/null) || main=origin/main
  if [ -z "$uninstall" ]; then
    # Pins this script copied from the main branch before; checking out
    # another release must not trip over them.
    git -C "$checkout" clean -q -f -- checksums >/dev/null 2>&1
    ref=$main
    [ -z "$version" ] || ref="v$version"
    git -C "$checkout" -c advice.detachedHead=false checkout --quiet --detach "$ref" ||
      die "could not check out $ref in $checkout (is $version a Corgi release?)"
    if [ -n "$version" ] && ! git -C "$checkout" cat-file -e "HEAD:checksums/v$version.sha256" 2>/dev/null; then
      pins="$checkout/checksums/v$version.sha256"
      mkdir -p "$checkout/checksums"
      if git -C "$checkout" show "$main:checksums/v$version.sha256" >"$pins" 2>/dev/null; then
        say "took the reviewed checksum pins for v$version from $main"
      else
        rm -f "$pins"
      fi
    fi
  fi
  exec sh "$checkout/herdr-app/install.sh" "$@"
fi

# --- in a checkout -------------------------------------------------------------
root=$(cd "$here/.." && pwd -P)
mod_src="$root/herdr-app/mods/corgi"
home_dir=${home_dir:-${HERDR_APP_HOME:-${HOME:?}/.config/herdr-app}}
mods="$home_dir/mods"
mod_dir="$mods/corgi"
bin="$root/target/release/corgi"
bin_dir=${CORGI_BIN_DIR:-${XDG_BIN_HOME:-$HOME/.local/bin}}
link="$bin_dir/corgi"

own_version=$(sed -n 's/^version *= *"\([^"]*\)".*/\1/p' "$root/herdr-plugin.toml" | head -n 1)
if [ -n "$version" ] && [ "$version" != "$own_version" ]; then
  die "this checkout is Corgi $own_version, not $version; install $version from its release instead"
fi

# Only ever replace or remove a folder that holds this mod.
ours() { [ ! -e "$1" ] || grep -q 'io.github.zyrre.corgi' "$1/main.js" 2>/dev/null; }

# The Herdr app: $HERDR_APP_BIN, herdr-app on PATH, or a macOS app bundle.
is_herdr_app() { "$1" --version 2>/dev/null | grep -q '^herdr-app '; }
find_app() {
  if [ -n "${HERDR_APP_BIN:-}" ]; then
    printf '%s\n' "$HERDR_APP_BIN"
    return
  fi
  c=$(command -v herdr-app 2>/dev/null) || c=
  if [ -n "$c" ] && is_herdr_app "$c"; then
    printf '%s\n' "$c"
    return
  fi
  for c in /Applications/*.app/Contents/MacOS/Herdr "$HOME"/Applications/*.app/Contents/MacOS/Herdr; do
    if [ -x "$c" ] && is_herdr_app "$c"; then
      printf '%s\n' "$c"
      return
    fi
  done
}

if [ -n "$uninstall" ]; then
  if [ -d "$mod_dir" ]; then
    ours "$mod_dir" || die "$mod_dir is not Corgi's mod; leaving it"
    rm -rf "$mod_dir" && say "removed $mod_dir"
  else
    say "no Corgi mod in $mods"
  fi
  if [ -n "$cli" ]; then
    if [ -L "$link" ] && [ "$(readlink "$link")" = "$bin" ]; then
      rm -f "$link" && say "removed $link"
    else
      say "left $link alone: it does not link $bin"
    fi
    say "the checkout $root is left; delete it to remove the rest"
  fi
  exit 0
fi

# --- the CLI -----------------------------------------------------------------------
# Links $link to $bin unless something else is there.
link_cli() {
  if [ -L "$link" ]; then
    current=$(readlink "$link")
    if [ "$current" = "$bin" ]; then
      return 0
    elif [ -e "$link" ]; then
      say "left $link alone: it links $current; the mod runs $bin"
      return 0
    fi
  elif [ -e "$link" ]; then
    say "left $link alone: it is not a link; the mod runs $bin"
    return 0
  fi
  if mkdir -p "$bin_dir" 2>/dev/null && ln -sfn "$bin" "$link" 2>/dev/null; then
    case ":${PATH:-}:" in
      *":$bin_dir:"*) say "linked $link" ;;
      *) say "linked $link, but $bin_dir is not on your PATH" ;;
    esac
  else
    say "could not link $link to $bin"
  fi
}

if [ -z "$mod_only" ]; then
  # The checkout's own pinned download, else source build. It links nothing
  # (that is for `herdr plugin install`) and leaves the mod to this script.
  CORGI_LINK=0 CORGI_APP_MOD=0 sh "$root/scripts/install.sh" || die "installing the corgi CLI failed"
  reported=$("$bin" --version 2>/dev/null) || reported=
  [ -n "$reported" ] || die "$bin does not run"
  say "corgi CLI: $reported at $bin"
  link_cli
fi

# --- the daemon --------------------------------------------------------------------
# `corgi daemon` (wakes and delivery while the app is closed) is installed
# here once it exists: after the CLI, before the mod, skipped with --mod-only.

[ -z "$cli_only" ] || exit 0

# --- the mod -------------------------------------------------------------------------
app=$(find_app)
[ -n "$app" ] || die "the Herdr app is not installed (no herdr-app on PATH or in /Applications); set HERDR_APP_BIN"
mkdir -p "$mods" || die "could not create $mods"
ours "$mod_dir" || die "$mod_dir exists and is not Corgi's mod; not overwriting it"

stage="$mods/.corgi-install"
rm -rf "$stage"
mkdir -p "$stage" || die "could not create $stage"
trap 'rm -rf "$stage"' EXIT
cp -R "$mod_src" "$stage/corgi" || die "could not copy the mod"
# What `herdr-app check` writes into a mod folder is the app's, not ours.
rm -f "$stage/corgi/jsconfig.json" "$stage/corgi"/*.d.ts
if [ -z "$mod_only" ]; then
  json_bin=$(printf '%s' "$bin" | sed 's/\\/\\\\/g; s/"/\\"/g')
  printf '{ "corgi": "%s" }\n' "$json_bin" >"$stage/corgi/config.json"
fi
"$app" check "$stage/corgi" >"$stage/check.log" 2>&1 || {
  cat "$stage/check.log" >&2
  die "herdr-app check failed; the installed mod is unchanged"
}
rm -f "$stage/corgi/jsconfig.json" "$stage/corgi"/*.d.ts
if [ -d "$mod_dir" ] && diff -rq -x jsconfig.json -x '*.d.ts' -x '.*' "$stage/corgi" "$mod_dir" >/dev/null 2>&1; then
  say "the Corgi mod in $mod_dir is up to date"
else
  if [ -d "$mod_dir" ]; then
    mv "$mod_dir" "$stage/old" || die "could not move the old mod aside"
  fi
  mv "$stage/corgi" "$mod_dir" || die "could not move the mod into $mod_dir"
  say "installed the Corgi mod in $mod_dir"
fi
