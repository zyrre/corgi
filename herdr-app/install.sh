#!/bin/sh
# Installs Corgi into the Herdr app: the corgi CLI, the Corgi and Projects
# mods, and the Projects list in the sidebar of the app's layout.
#
#   curl -fsSL https://raw.githubusercontent.com/zyrre/corgi/main/herdr-app/install.sh | sh
#   herdr-app/install.sh [--version X.Y.Z] [--home DIR] [--cli-only | --mod-only] [--no-daemon]
#   herdr-app/install.sh --uninstall [--cli] [--home DIR]
#
# Run again to update.
#
# Without a Corgi checkout around it (piped, as above, or saved on its own),
# it first clones https://github.com/zyrre/corgi into $CORGI_CHECKOUT
# (default ~/.local/share/corgi), or fetches it again, checks out the
# release tag vX.Y.Z (--version; default: the main branch) and runs that
# checkout's copy of this script with the same arguments. A release tag
# does not hold its own checksum pins (they are reviewed after the release),
# so the tag's checkout takes checksums/vX.Y.Z.sha256 from the main branch.
#
# In a checkout:
#   - the CLI: scripts/install.sh puts the checkout's version of corgi at
#     target/release/corgi: a release binary verified against the checkout's
#     checksum pins when HEAD is the tag v<version> (the piped install's own
#     checkout counts as such), else built with cargo build --release --locked,
#     so a developer's checkout is never downgraded to the release
#     (CORGI_BUILD=source builds anyway, CORGI_BUILD=release downloads
#     anyway); then
#     ~/.local/bin/corgi ($CORGI_BIN_DIR, else $XDG_BIN_HOME/corgi) links to
#     it, unless something else is there: a file, or a working link
#     elsewhere, is left alone
#   - the daemon: `corgi daemon install` runs `corgi daemon` as a service of
#     the user's session (a LaunchAgent on macOS, a systemd user unit on
#     Linux), restarted with the new binary on every run, so supervisors on
#     herdr-core are woken and handed over while the app's window is closed
#   - OpenCode's status plugin: when OpenCode is installed and does not
#     report its status to herdr-core itself, the script asks once whether to
#     put herdr-app/opencode/corgi-hc-status.js into
#     ~/.config/opencode/plugin/ (without it, an OpenCode worker on hc wakes
#     no supervisor), and remembers the answer; a run with no terminal to ask
#     in skips it and says so, and one where OpenCode reports by itself
#     removes Corgi's plugin
#   - the mods: herdr-app/mods/projects (the Projects list of the
#     sidebar), herdr-app/mods/corgi-brand (the sidebar's top row, "Herdr +
#     Corgi") and herdr-app/mods/corgi (an overview of every project, shown
#     nowhere until it is given a tab; see `corgi app-space` below) are each
#     put together in a hidden folder of the app home ($HERDR_APP_HOME, else
#     ~/.config/herdr-app; --home DIR), with a config.json naming the CLI,
#     checked with `herdr-app check`, and swapped in with one rename, only
#     when it differs from the installed one: the app reloads a mod on every
#     file it sees change
#   - the layout: `corgi app-layout` puts the Projects mod in place of the
#     sidebar's Spaces, takes out its Agents list and puts the brand mod in
#     the sidebar's brand row, and changes nothing else: a layout of the person's own in place; Herdr's shipped main is
#     first cloned to layouts/corgi.json, which an open window switches to.
#     Run again, it changes nothing
#
# It makes no space in the app, and neither does the daemon. The Corgi space
# (a space named Corgi showing the Corgi mod), which earlier versions made,
# is made only by hand, with `corgi app-space`; one that is there is left.
#
#   --version X.Y.Z  the release to install; in a checkout it must be the
#                    checkout's own (Cargo.toml), so nothing else is
#                    ever installed in its place
#   --cli-only       the CLI, the daemon, OpenCode's plugin, the Projects mod
#                    and the layout, without the Corgi mod (the Corgi
#                    mod runs this to install corgi)
#   --mod-only       the mods and the layout alone, with no config.json, so
#                    the mods run the corgi on PATH (scripts/app-preview
#                    installs its window's mods this way)
#   --no-daemon      leaves the daemon as it is
#   --uninstall      puts Herdr's Spaces and Agents back where Projects is in
#                    the layouts (no layout is deleted), removes the mods,
#                    the daemon and Corgi's OpenCode plugin, and closes a
#                    Corgi space made with `corgi app-space`, whose mod is
#                    going; with --cli also the link to this checkout's
#                    corgi (the checkout itself is left). With --home only
#                    what is in that app home goes: the daemon, the plugin
#                    and the link are the machine's, and stay
#
#   CORGI_BUILD=source|release   build the CLI from source, or use the pinned
#                                download, whatever the checkout's HEAD
#   CORGI_CHECKOUT=<dir>         where the piped install keeps its checkout
#   CORGI_REPO_URL=<url>         clone from <url> instead of GitHub
#   HERDR_APP_BIN=<path>         the herdr-app to check the mod with
#   CORGI_OPENCODE_PLUGIN=yes|no the answer to the OpenCode question, kept
#                                as if given in a terminal
#   CORGI_OPENCODE_BIN=<path>    the opencode to look at, instead of the one
#                                on PATH or in ~/.opencode/bin
set -u

say() { printf 'corgi app install: %s\n' "$*" >&2; }
die() {
  say "$*"
  exit 1
}

version='' home_dir='' home_given='' uninstall='' cli='' cli_only='' mod_only='' no_daemon=''
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
        home_given=1
        shift
        ;;
      --uninstall) uninstall=1 ;;
      --cli) cli=1 ;;
      --cli-only) cli_only=1 ;;
      --mod-only) mod_only=1 ;;
      --no-daemon) no_daemon=1 ;;
      -h | --help)
        sed -n '2,87p' "$0" 2>/dev/null || say "see https://github.com/zyrre/corgi#install"
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
  # Its own checkout is the release being installed, even on the main branch.
  CORGI_BUILD=${CORGI_BUILD:-release}
  export CORGI_BUILD
  exec sh "$checkout/herdr-app/install.sh" "$@"
fi

# --- in a checkout -------------------------------------------------------------
root=$(cd "$here/.." && pwd -P)
home_dir=${home_dir:-${HERDR_APP_HOME:-${HOME:?}/.config/herdr-app}}
mods="$home_dir/mods"
bin="$root/target/release/corgi"
bin_dir=${CORGI_BIN_DIR:-${XDG_BIN_HOME:-$HOME/.local/bin}}
link="$bin_dir/corgi"

own_version=$(sed -n 's/^version *= *"\([^"]*\)".*/\1/p' "$root/Cargo.toml" | head -n 1)
if [ -n "$version" ] && [ "$version" != "$own_version" ]; then
  die "this checkout is Corgi $own_version, not $version; install $version from its release instead"
fi

# Only ever replace or remove a folder that holds one of Corgi's mods.
ours() { [ ! -e "$1" ] || grep -q 'io.github.zyrre.corgi' "$1/main.js" 2>/dev/null; }
# What a mod's folder is called to a person.
mod_title() {
  case $1 in
    projects) echo "Projects mod" ;;
    corgi-brand) echo "brand row mod" ;;
    *) echo "Corgi mod" ;;
  esac
}

# The Herdr app: $HERDR_APP_BIN, herdr-app on PATH, or a macOS app bundle.
is_herdr_app() { "$1" --version 2>/dev/null | grep -q '^herdr-app '; }
find_app() {
  if [ -n "${HERDR_APP_BIN:-}" ]; then
    # The one named, or none: never another in its place.
    if is_herdr_app "$HERDR_APP_BIN"; then printf '%s\n' "$HERDR_APP_BIN"; fi
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

# --- OpenCode's status plugin -----------------------------------------------------
oc_src="$root/herdr-app/opencode/corgi-hc-status.js"
oc_plugin="${XDG_CONFIG_HOME:-$HOME/.config}/opencode/plugin/corgi-hc-status.js"
oc_answer="${XDG_STATE_HOME:-$HOME/.local/state}/corgi/opencode-plugin"

# OpenCode's binary: $CORGI_OPENCODE_BIN, else on PATH, else where its
# installer puts it.
find_opencode() {
  if [ -n "${CORGI_OPENCODE_BIN:-}" ]; then
    [ -x "$CORGI_OPENCODE_BIN" ] && printf '%s\n' "$CORGI_OPENCODE_BIN"
    return
  fi
  c=$(command -v opencode 2>/dev/null) || c=
  [ -n "$c" ] || c="$HOME/.opencode/bin/opencode"
  [ -x "$c" ] && printf '%s\n' "$c"
}
# Whether that OpenCode writes OSC 7501 itself: its binary holds the escape.
opencode_reports() { LC_ALL=C grep -a -q ']7501;state' "$1" 2>/dev/null; }
# Only ever replace or remove a plugin file that is Corgi's.
oc_ours() { [ ! -e "$1" ] || grep -q 'io.github.zyrre.corgi' "$1" 2>/dev/null; }
remove_oc_plugin() {
  if [ -f "$oc_plugin" ] && oc_ours "$oc_plugin"; then
    rm -f "$oc_plugin" && say "$1"
  fi
}
# Asks the person running this, when there is one, whether to install the
# plugin; prints yes or no, or nothing when nobody can be asked.
ask_opencode() {
  [ -t 2 ] && [ -r /dev/tty ] || return 0
  printf '%s' "corgi app install: install Corgi's status plugin for OpenCode in $(dirname "$oc_plugin")? Without it an OpenCode worker on herdr-core wakes no supervisor. Every OpenCode session loads it; outside herdr-core it does nothing. [y/N] " >&2
  reply=
  read -r reply </dev/tty 2>/dev/null || reply=
  case $reply in
    [Yy] | [Yy][Ee][Ss]) echo yes ;;
    *) echo no ;;
  esac
}
install_opencode_plugin() {
  oc=$(find_opencode)
  if [ -z "$oc" ]; then
    return 0
  fi
  if opencode_reports "$oc"; then
    remove_oc_plugin "OpenCode reports its status to herdr-core itself: removed Corgi's plugin $oc_plugin"
    return 0
  fi
  answer=${CORGI_OPENCODE_PLUGIN:-}
  [ -n "$answer" ] || answer=$(cat "$oc_answer" 2>/dev/null) || answer=
  if [ -z "$answer" ]; then
    answer=$(ask_opencode)
    if [ -z "$answer" ]; then
      say "skipped OpenCode's status plugin: no terminal to ask in (run this again in one, or set CORGI_OPENCODE_PLUGIN=yes)"
      return 0
    fi
  fi
  case $answer in
    yes | no) ;;
    *) die "CORGI_OPENCODE_PLUGIN is yes or no, not $answer" ;;
  esac
  mkdir -p "$(dirname "$oc_answer")" && printf '%s\n' "$answer" >"$oc_answer"
  if [ "$answer" = no ]; then
    remove_oc_plugin "removed Corgi's OpenCode plugin $oc_plugin, as answered"
    return 0
  fi
  if ! oc_ours "$oc_plugin"; then
    say "left $oc_plugin alone: it is not Corgi's"
  elif [ -f "$oc_plugin" ] && cmp -s "$oc_src" "$oc_plugin"; then
    :
  elif mkdir -p "$(dirname "$oc_plugin")" && cp "$oc_src" "$oc_plugin.tmp" && mv "$oc_plugin.tmp" "$oc_plugin"; then
    say "installed Corgi's OpenCode status plugin in $oc_plugin"
  else
    rm -f "$oc_plugin.tmp"
    say "could not install Corgi's OpenCode plugin in $oc_plugin"
  fi
}

# Runs `corgi app-space` with this app and home. Only ever with remove:
# closing a Corgi space someone made by hand, when its mod is uninstalled.
app_space() {
  set -- "$@" --home "$home_dir"
  [ -z "$app" ] || set -- "$@" --app "$app"
  said=$("$bin" app-space "$@") || return 1
  [ -z "$said" ] || say "$said"
}

# Runs `corgi app-layout` with this app's home: puts the Projects list into
# the layout, or with remove puts Spaces and Agents back in its place.
app_layout() {
  set -- "$@" --home "$home_dir"
  said=$("$bin" app-layout "$@") || return 1
  [ -z "$said" ] || say "$said"
}

if [ -n "$uninstall" ]; then
  app=$(find_app)
  if [ -x "$bin" ]; then
    app_space remove || say "could not close the Corgi space; close it in the Herdr app if there is one"
    # Before the Projects mod goes, which the layouts show: Spaces and
    # Agents go back in its place, and every layout stays.
    app_layout remove || say "could not put Spaces and Agents back in the layouts in $home_dir/layouts; run $bin app-layout remove"
    [ -n "$home_given" ] || "$bin" daemon uninstall || say "could not remove the corgi daemon; run $bin daemon uninstall"
  else
    say "no $bin to put Spaces back with: in $home_dir/layouts/*.json make the \"projects\" block \"spaces\" and the \"corgi-brand\" block \"workspace-chrome\" again"
    [ -n "$home_given" ] || say "and remove the daemon with: corgi daemon uninstall"
  fi
  # The daemon, OpenCode's plugin and the link are this machine's, not an
  # app home's: --home removes only what is in that home.
  if [ -n "$home_given" ]; then
    say "left the corgi daemon, Corgi's OpenCode plugin${cli:+ and $link} alone: --home removes only what is in $home_dir (run --uninstall without --home for those)"
  else
    remove_oc_plugin "removed Corgi's OpenCode plugin $oc_plugin"
    rm -f "$oc_answer"
  fi
  for name in corgi projects corgi-brand; do
    mod_dir="$mods/$name"
    if [ -d "$mod_dir" ]; then
      ours "$mod_dir" || die "$mod_dir is not Corgi's mod; leaving it"
      rm -rf "$mod_dir" && say "removed $mod_dir"
    else
      say "no $(mod_title "$name") in $mods"
    fi
  done
  if [ -n "$cli" ] && [ -z "$home_given" ]; then
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
  # The pinned download when HEAD is the release tag, else a source build.
  sh "$root/scripts/install.sh" || die "installing the corgi CLI failed"
  reported=$("$bin" --version 2>/dev/null) || reported=
  [ -n "$reported" ] || die "$bin does not run"
  say "corgi CLI: $reported at $bin"
  link_cli
fi

# --- the daemon --------------------------------------------------------------------
# Wakes, deliveries and handovers on herdr-core while the app is closed.
# Installed again on every run, so it runs the binary just installed.
if [ -z "$mod_only" ] && [ -z "$no_daemon" ]; then
  if "$bin" daemon install; then
    say "the corgi daemon runs as a service; corgi daemon status says how it is"
  else
    say "could not install the corgi daemon; run $bin daemon install to try again"
  fi
fi

if [ -z "$mod_only" ]; then
  install_opencode_plugin
fi

# --- the mods ------------------------------------------------------------------------
app=$(find_app)
if [ -z "$app" ]; then
  # The Corgi mod asked for the CLI, which is there: the rest can wait.
  [ -z "$cli_only" ] || {
    say "the Herdr app was not found (no herdr-app on PATH or in /Applications): the Projects mod and the layout are not installed"
    exit 0
  }
  die "the Herdr app is not installed (no herdr-app on PATH or in /Applications); set HERDR_APP_BIN"
fi
mkdir -p "$mods" || die "could not create $mods"
stage="$mods/.corgi-install"
rm -rf "$stage"
mkdir -p "$stage" || die "could not create $stage"
trap 'rm -rf "$stage"' EXIT

# Puts herdr-app/mods/$1 into the app's home: checked first, and swapped in
# whole, only when it differs from the one installed.
install_mod() {
  name=$1
  title=$(mod_title "$name")
  mod_dir="$mods/$name"
  ours "$mod_dir" || die "$mod_dir exists and is not Corgi's mod; not overwriting it"
  rm -rf "${stage:?}/$name" "$stage/old"
  cp -R "$root/herdr-app/mods/$name" "$stage/$name" || die "could not copy the $title"
  # What `herdr-app check` writes into a mod folder is the app's, not ours.
  rm -f "$stage/$name/jsconfig.json" "$stage/$name"/*.d.ts
  if [ -z "$mod_only" ]; then
    json_bin=$(printf '%s' "$bin" | sed 's/\\/\\\\/g; s/"/\\"/g')
    printf '{ "corgi": "%s" }\n' "$json_bin" >"$stage/$name/config.json"
  fi
  "$app" check "$stage/$name" >"$stage/check.log" 2>&1 || {
    cat "$stage/check.log" >&2
    die "herdr-app check failed; the installed $title is unchanged"
  }
  rm -f "$stage/$name/jsconfig.json" "$stage/$name"/*.d.ts
  if [ -d "$mod_dir" ] && diff -rq -x jsconfig.json -x '*.d.ts' -x '.*' "$stage/$name" "$mod_dir" >/dev/null 2>&1; then
    say "the $title in $mod_dir is up to date"
  else
    if [ -d "$mod_dir" ]; then
      mv "$mod_dir" "$stage/old" || die "could not move the old $title aside"
    fi
    mv "$stage/$name" "$mod_dir" || die "could not move the $title into $mod_dir"
    say "installed the $title in $mod_dir"
  fi
}

# The Projects and brand row mods before the layout that shows them. The
# Corgi mod is left alone when it is the one that asked (--cli-only): it is
# running.
install_mod projects
install_mod corgi-brand
[ -n "$cli_only" ] || install_mod corgi

# --- the layout ------------------------------------------------------------------------
# The Projects list in the sidebar of the layout the window shows: a shipped
# one is cloned to corgi first, and an open window switched to the clone.
# --mod-only has a corgi when the checkout was built before.
if [ -x "$bin" ]; then
  app_layout --app "$app" || say "could not put the Projects list into the layout; run $bin app-layout to try again"
else
  say "no $bin to put the Projects list into the layout with; run corgi app-layout once it is installed"
fi
