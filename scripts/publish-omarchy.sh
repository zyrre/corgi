#!/bin/sh
# Publishes the Omarchy bar widget to its own repository, the one
# `omarchy plugin add` installs from.
#
#   sh scripts/publish-omarchy.sh <version> <repository URL>
#
# Run from a Corgi checkout. It takes the files Git tracks under omarchy/ at
# HEAD, plus Corgi's LICENSE, and makes them the whole root of the
# repository's default branch, in one commit ("Corgi v<version>") on top of
# its history; the very first publish starts that history (as `main`) in an
# empty repository. `omarchy plugin update` only fast-forwards, so this never
# rewrites or force-pushes: if the repository moved in the meantime, the push
# fails instead. When nothing changed, it commits and pushes nothing.
#
# It refuses to publish when omarchy/manifest.json is not at <version>, or
# when the files hold a symlink, which Omarchy rejects. Test files are left
# out. It needs jq. Pushing uses whatever credentials and identity Git has,
# such as GIT_SSH_COMMAND and GIT_AUTHOR_NAME.
set -eu

say() { printf 'publish-omarchy: %s\n' "$*" >&2; }

[ $# -eq 2 ] || { say "usage: publish-omarchy.sh <version> <repository URL>"; exit 2; }
version=$1
url=$2

source=$(git rev-parse --show-toplevel)
commit=$(git -C "$source" rev-parse HEAD)
work=$(mktemp -d "${TMPDIR:-/tmp}/corgi-omarchy-publish.XXXXXX")
trap 'rm -rf "$work"' EXIT

git clone --quiet "$url" "$work/repo"
cd "$work/repo"
if git rev-parse --verify --quiet HEAD >/dev/null; then
  branch=$(git symbolic-ref --short HEAD)
else
  # An empty repository, or one whose HEAD names a branch it does not have.
  branch=main
  if git rev-parse --verify --quiet "refs/remotes/origin/$branch" >/dev/null; then
    git checkout --quiet -B "$branch" "origin/$branch"
  else
    git symbolic-ref HEAD "refs/heads/$branch"
  fi
fi

# The widget becomes the whole tree: what it no longer has goes too.
git rm -r --quiet --ignore-unmatch -- .
git -C "$source" archive --format=tar "HEAD:omarchy" | tar -xf -
git -C "$source" show "HEAD:LICENSE" >LICENSE
find . -path ./.git -prune -o \( -name 'test_*' -o -name '*_test.*' -o -name '*.test.*' -o -name tests \) \
  -prune -exec rm -rf {} +

if [ -n "$(find . -path ./.git -prune -o -type l -print)" ]; then
  say "omarchy/ holds a symlink, which Omarchy refuses to install"
  exit 1
fi
manifest_version=$(jq -r .version manifest.json 2>/dev/null || true)
if [ "$manifest_version" != "$version" ]; then
  say "omarchy/manifest.json is at version '$manifest_version', not $version"
  exit 1
fi

git add --all
if git diff --cached --quiet; then
  say "the widget at v$version is already published"
  exit 0
fi
git commit --quiet -m "Corgi v$version" -m "Published from zyrre/corgi@$commit."
git push --quiet origin "HEAD:refs/heads/$branch"
say "published v$version to $branch"
