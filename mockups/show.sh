#!/usr/bin/env bash
# Prints every card mockup, dark theme first and then light, one after
# another with a header each, for a pager: mockups/show.sh | less -R
set -euo pipefail
cd "$(dirname "$0")/ansi"
for theme in dark light; do
  for file in [0-9]-*.ans; do
    case "$file" in *-light.ans) [ "$theme" = light ] || continue ;; *) [ "$theme" = dark ] || continue ;; esac
    printf '\n\033[1;38;5;208m━━━━ %s ━━━━\033[0m\n' "${file%.ans}"
    cat "$file"
  done
done
