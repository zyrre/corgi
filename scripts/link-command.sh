#!/bin/sh
# Puts `corgi` on PATH during `herdr plugin install`.
#
# Herdr runs the build in <plugins>/.tmp-install-*/checkout and, once it
# passes, moves the checkout to <plugins>/github/<id>-<first 12 hex digits of
# sha256(id)>. This links ~/.local/bin/corgi to the binary's final place
# there, by its absolute path, so the command works as soon as the install
# finishes. Anywhere else, such as a developer's own checkout, it does
# nothing. It replaces a link into Herdr's plugin folder (an older install)
# or a broken link, but never a file, a folder, or a working link elsewhere.
# It only ever warns: a link it cannot make does not fail the install.
#
#   CORGI_LINK=0          do not link
#   CORGI_BIN_DIR=<dir>   link into <dir> instead (else $XDG_BIN_HOME, else
#                         ~/.local/bin)
#
#   sh scripts/link-command.sh [checkout]   (default: the current folder)
set -u

say() { printf 'corgi install: %s\n' "$*" >&2; }

[ "${CORGI_LINK:-1}" != 0 ] || exit 0

checkout=$(cd "${1:-.}" 2>/dev/null && pwd -P) || exit 0
case "$checkout" in
  */.tmp-install-*/checkout) ;;
  *) exit 0 ;;
esac
plugins=${checkout%/.tmp-install-*/checkout}

id=$(sed -n 's/^id *= *"\([^"]*\)".*/\1/p' "$checkout/herdr-plugin.toml" 2>/dev/null | head -n 1)
# Herdr also rewrites other characters in the folder name; the id has none.
case "$id" in
  "" | *[!A-Za-z0-9._-]*)
    say "did not link corgi: cannot tell where Herdr puts plugin '$id'"
    exit 0
    ;;
esac
hash=$(printf %s "$id" | { sha256sum 2>/dev/null || shasum -a 256 2>/dev/null; } | cut -c 1-12)
if [ ${#hash} -ne 12 ]; then
  say "did not link corgi: neither sha256sum nor shasum is installed"
  exit 0
fi
target="$plugins/github/$id-$hash/target/release/corgi"

bin=${CORGI_BIN_DIR:-${XDG_BIN_HOME:-${HOME:-}/.local/bin}}
link="$bin/corgi"

if [ -L "$link" ]; then
  current=$(readlink "$link")
  case "$current" in
    "$plugins"/*) ;;
    *) if [ -e "$link" ]; then
         say "left $link alone: it links $current"
         exit 0
       fi ;;
  esac
elif [ -e "$link" ]; then
  say "left $link alone: it is not a link"
  exit 0
fi
if ! mkdir -p "$bin" 2>/dev/null || ! ln -sfn "$target" "$link" 2>/dev/null; then
  say "could not link $link to $target"
  exit 0
fi
case ":${PATH:-}:" in
  *":$bin:"*) say "linked $link" ;;
  *) say "linked $link, but $bin is not on your PATH: add export PATH=\"$bin:\$PATH\" to your shell profile" ;;
esac
exit 0
