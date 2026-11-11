#!/bin/sh
# Puts the corgi binary at target/release/corgi, where the Herdr app's mods
# and the daemon run it. herdr-app/install.sh runs this for the CLI.
#
# It downloads the prebuilt binary of the release named by Cargo.toml's
# `version` for this machine, checks it against this checkout's committed
# checksum pins, and falls back to `cargo build --release --locked` when
# there is no such binary or it cannot be verified. A checkout with
# uncommitted changes to tracked files is a developer's, so it always builds
# what it has.
#
# A git checkout is a developer's unless it is exactly the release: when HEAD
# is not the commit of the tag v<version>, the CLI is built from source, so an
# install never replaces a build of newer code with an older release binary.
# An install that sets CORGI_BUILD=release keeps the pinned download whatever
# its HEAD (herdr-app/install.sh does for the checkout a piped install
# fetched). A folder that is not a git checkout (an unpacked release)
# downloads too.
#
#   CORGI_BUILD=source         always build from source
#   CORGI_BUILD=release        use the pinned download even when the checkout
#                              is not on the release tag (uncommitted changes,
#                              a missing pin or a failed check still build)
#   CORGI_DOWNLOAD_URL=<url>   download from <url>/v<version>/ instead of the
#                              GitHub release of `origin`
set -u

cd "$(dirname "$0")/.." || exit 1

say() { printf 'corgi install: %s\n' "$*" >&2; }

tmp=
build_from_source() {
  if [ -n "$tmp" ]; then
    rm -rf "$tmp"
    tmp=
  fi
  say "$1"
  say "building from source instead: cargo build --release --locked (this takes a minute or two)"
  if ! command -v cargo >/dev/null 2>&1; then
    say "cargo is not installed. Install Rust (https://rustup.rs), then install again."
    exit 1
  fi
  cargo build --release --locked || exit $?
  exit 0
}

case "${CORGI_BUILD:-}" in
  source) build_from_source "CORGI_BUILD=source is set" ;;
  "" | release) ;;
  *) say "ignoring CORGI_BUILD=$CORGI_BUILD (it is source or release)" ;;
esac

version=$(sed -n 's/^version *= *"\([^"]*\)".*/\1/p' Cargo.toml | head -n 1)
[ -n "$version" ] || build_from_source "Cargo.toml has no version"
tag="v$version"

case "$(uname -s)" in
  Darwin) os=apple-darwin ;;
  Linux) os=unknown-linux-musl ;;
  *) build_from_source "there is no prebuilt binary for $(uname -s)" ;;
esac
case "$(uname -m)" in
  arm64 | aarch64) arch=aarch64 ;;
  x86_64 | amd64) arch=x86_64 ;;
  *) build_from_source "there is no prebuilt binary for $(uname -m)" ;;
esac
asset="corgi-$arch-$os"

origin=
if [ -e .git ] && command -v git >/dev/null 2>&1; then
  if [ -n "$(git status --porcelain --untracked-files=no 2>/dev/null)" ]; then
    build_from_source "this checkout has uncommitted changes"
  fi
  origin=$(git remote get-url origin 2>/dev/null)
  case "${CORGI_BUILD:-}" in
    release) ;;
    *)
      head=$(git rev-parse --verify -q HEAD 2>/dev/null)
      tagged=$(git rev-parse --verify -q "refs/tags/$tag^{commit}" 2>/dev/null)
      if [ -z "$head" ] || [ "$head" != "$tagged" ]; then
        build_from_source "this checkout is not on the release tag $tag (CORGI_BUILD=release downloads it anyway)"
      fi
      ;;
  esac
fi

# The file binds each digest to both a version and an exact platform asset.
# Reject the whole file on malformed or duplicate entries; never let remote
# SHA256SUMS (including a download mirror's) supply or override trusted bytes.
pins="checksums/$tag.sha256"
[ -f "$pins" ] || build_from_source "no reviewed checksum pins for $tag ($pins)"
expected=$(awk -v tag="$tag" -v name="$asset" '
  NF != 3 || $1 != tag || length($2) != 64 || $2 ~ /[^0-9a-f]/ ||
    $3 !~ /^corgi-(aarch64|x86_64)-(apple-darwin|unknown-linux-musl)$/ { bad = 1 }
  { if (seen[$3]++) bad = 1; if ($3 == name) { digest = $2; matches++ } }
  END { if (bad || matches != 1) exit 1; print digest }
' "$pins") || build_from_source "invalid or missing reviewed checksum pin for $asset $tag in $pins"

if [ -n "${CORGI_DOWNLOAD_URL:-}" ]; then
  base="${CORGI_DOWNLOAD_URL%/}/$tag"
else
  repo=$(printf '%s\n' "$origin" | sed -n 's#^.*github\.com[:/]\([^/]*/[^/]*\)$#\1#p' | sed 's#\.git$##')
  base="https://github.com/${repo:-zyrre/corgi}/releases/download/$tag"
fi

if command -v curl >/dev/null 2>&1; then
  fetch() { curl -fsSL --retry 2 --connect-timeout 10 -o "$2" "$1"; }
elif command -v wget >/dev/null 2>&1; then
  fetch() { wget -q -T 10 -O "$2" "$1"; }
else
  build_from_source "neither curl nor wget is installed"
fi
if command -v sha256sum >/dev/null 2>&1; then
  sha256() { sha256sum "$1" | cut -d ' ' -f 1; }
elif command -v shasum >/dev/null 2>&1; then
  sha256() { shasum -a 256 "$1" | cut -d ' ' -f 1; }
else
  build_from_source "neither sha256sum nor shasum is installed to check the download"
fi

mkdir -p target/release
tmp=$(mktemp -d "target/release/.download.XXXXXX") || {
  tmp=
  build_from_source "could not create a download folder"
}
trap '[ -z "$tmp" ] || rm -rf "$tmp"' EXIT

say "downloading $asset $tag"
fetch "$base/$asset" "$tmp/$asset" ||
  build_from_source "could not download $base/$asset"
actual=$(sha256 "$tmp/$asset")
[ "$actual" = "$expected" ] ||
  build_from_source "the downloaded $asset does not match its reviewed checksum pin (got $actual, expected $expected)"

chmod 755 "$tmp/$asset"
# "corgi 0.2.4", and after it the commit a build from Git names: "corgi 0.2.4
# (5b50895)". Nothing else may follow the version: 0.2.40 is not 0.2.4.
reported=$("$tmp/$asset" --version 2>/dev/null)
case $reported in
  "corgi $version" | "corgi $version ("*")") ;;
  *) build_from_source "the downloaded binary did not run or is not $version (it said: ${reported:-nothing})" ;;
esac

# Rename, never copy over: macOS kills a binary rewritten in place.
mv -f "$tmp/$asset" target/release/corgi ||
  build_from_source "could not move the binary into target/release"
say "installed the prebuilt $asset $tag"
