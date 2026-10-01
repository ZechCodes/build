#!/bin/sh
# Print the CHANGELOG.md entry for one bridge version: everything under its
# `## [X.Y.Z] - YYYY-MM-DD` heading up to the next `## ` heading or the link
# definitions at the foot, without the heading itself and without the blank
# lines around it. release.yml puts this above the install and verification
# notes, so a release cannot go out with an entry that is missing, still says
# Unreleased, carries no date, or says nothing.
#
# Usage: scripts/changelog-section.sh X.Y.Z [CHANGELOG.md]
#   0 the entry is on stdout; 1 the entry is not fit to publish (the reason is
#   on stderr); 64 usage.
set -eu

if [ "$#" -lt 1 ] || [ "$#" -gt 2 ] || [ -z "$1" ]; then
  echo "usage: $0 X.Y.Z [CHANGELOG.md]" >&2
  exit 64
fi
version="$1"
changelog="${2:-CHANGELOG.md}"

if [ ! -r "$changelog" ]; then
  echo "changelog-section: cannot read ${changelog}" >&2
  exit 1
fi

# The version is compared as a string, never as a pattern, so its dots match
# only dots. awk reports through its exit status: 2 no heading, 3 more than one,
# 4 Unreleased, 5 no date, 6 an empty entry.
status=0
section="$(awk -v version="$version" '
  BEGIN { prefix = "## [" version "]"; found = 0; inside = 0; n = 0 }
  /^## / {
    if (substr($0, 1, length(prefix)) == prefix) {
      found++
      inside = 1
      heading = $0
      next
    }
    inside = 0
  }
  /^\[[^]]+\]: / { inside = 0 }
  inside { lines[++n] = $0 }
  END {
    if (found == 0) exit 2
    if (found > 1) exit 3
    rest = substr(heading, length(prefix) + 1)
    if (rest ~ /[Uu]nreleased/) exit 4
    if (rest !~ /^ - [0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]$/) exit 5
    first = 1
    while (first <= n && lines[first] ~ /^[ \t]*$/) first++
    last = n
    while (last >= first && lines[last] ~ /^[ \t]*$/) last--
    if (first > last) exit 6
    for (i = first; i <= last; i++) print lines[i]
  }
' "$changelog")" || status=$?

case "$status" in
  0) printf '%s\n' "$section" ;;
  2) echo "changelog-section: ${changelog} has no \"## [${version}]\" entry" >&2 ;;
  3) echo "changelog-section: ${changelog} has more than one \"## [${version}]\" entry" >&2 ;;
  4) echo "changelog-section: the ${version} entry in ${changelog} still says Unreleased; date it before tagging" >&2 ;;
  5) echo "changelog-section: the ${version} heading in ${changelog} is not \"## [${version}] - YYYY-MM-DD\"" >&2 ;;
  6) echo "changelog-section: the ${version} entry in ${changelog} is empty" >&2 ;;
  *) echo "changelog-section: awk failed reading ${changelog} (status ${status})" >&2 ;;
esac
[ "$status" -eq 0 ] || exit 1
