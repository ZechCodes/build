#!/usr/bin/env bash
# Behaviour tests for changelog-section.sh, run offline on any machine: the
# repository's own CHANGELOG.md is the fixture for the entries that publish,
# and small changelogs written here cover each way an entry is refused.
#
# Usage: scripts/test-changelog-section.sh   (0 every case passed, 1 otherwise)
set -euo pipefail

SCRIPT_DIR="$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd)"
SECTION_SH="$SCRIPT_DIR/changelog-section.sh"
CHANGELOG="$SCRIPT_DIR/../CHANGELOG.md"

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT INT TERM
FAILURES=0

pass() { printf 'ok   %s\n' "$1"; }
fail() {
  printf 'FAIL %s: %s\n' "$1" "$2"
  FAILURES=$((FAILURES + 1))
}

# run NAME ARGS... — runs the script, keeping stdout, stderr and the status.
run() {
  local name="$1"
  shift
  status=0
  sh "$SECTION_SH" "$@" > "$WORK/$name.out" 2> "$WORK/$name.err" || status=$?
  out="$(cat "$WORK/$name.out")"
  err="$(cat "$WORK/$name.err")"
}

# expect_refused NAME STATUS STDERR-FRAGMENT — the run printed nothing, exited
# STATUS and said why.
expect_refused() {
  if [ "$status" -ne "$2" ]; then
    fail "$1" "exit $status, expected $2 (stderr: $err)"
  elif [ -n "$out" ]; then
    fail "$1" "printed an entry for a refused version: $out"
  elif [[ "$err" != *"$3"* ]]; then
    fail "$1" "stderr does not say '$3': $err"
  else
    pass "$1"
  fi
}

# --- The repository's CHANGELOG.md ---

# The dated 0.2.4 entry, whole: from the wire line to its last Security bullet,
# with neither its own heading nor the 0.2.3 one.
run released "0.2.4" "$CHANGELOG"
if [ "$status" -ne 0 ]; then
  fail "a dated entry is printed" "exit $status: $err"
elif [ "$(head -n 1 <<< "$out")" != "Wire 3.4.0." ]; then
  fail "a dated entry is printed" "first line is '$(head -n 1 <<< "$out")'"
elif [[ "$(tail -n 1 <<< "$out")" != *"(#242)." ]]; then
  fail "a dated entry is printed" "last line is '$(tail -n 1 <<< "$out")'"
elif grep -q '^## ' <<< "$out"; then
  fail "a dated entry is printed" "it carries a version heading"
elif ! grep -qF '(#298)' <<< "$out" || ! grep -qF '(#299)' <<< "$out" \
  || ! grep -qF '(#297)' <<< "$out" || ! grep -qF '### Security' <<< "$out"; then
  fail "a dated entry is printed" "lines are missing from it"
else
  pass "a dated entry is printed"
fi

# The oldest entry runs into the link definitions at the foot of the file,
# which belong to no entry.
run oldest "0.2.3" "$CHANGELOG"
if [ "$status" -ne 0 ]; then
  fail "the oldest entry stops at the link definitions" "exit $status: $err"
elif grep -q '^\[' <<< "$out"; then
  fail "the oldest entry stops at the link definitions" "it carries: $(grep '^\[' <<< "$out")"
elif [ "$(head -n 1 <<< "$out")" != "Wire 3.1.0." ]; then
  fail "the oldest entry stops at the link definitions" "first line is '$(head -n 1 <<< "$out")'"
else
  pass "the oldest entry stops at the link definitions"
fi

# A fixture, not the repository's own file: its Unreleased entry is dated at
# every release, and this case must not fail the release that dates it.
cat > "$WORK/unreleased.md" <<'MD'
# Changelog

## [1.2.0] - Unreleased

- One.

## [1.1.0] - 2026-02-01

- Zero.
MD
run unreleased "1.2.0" "$WORK/unreleased.md"
expect_refused "an Unreleased entry is refused" 1 "still says Unreleased"

run missing "9.9.9" "$CHANGELOG"
expect_refused "a version with no entry is refused" 1 'no "## [9.9.9]" entry'

# A dot in the version is a dot: 0x2x4 must not find 0.2.4.
run dots "0x2x4" "$CHANGELOG"
expect_refused "the version is matched literally" 1 'no "## [0x2x4]" entry'

# A prefix of a version is not that version.
run prefix "0.2" "$CHANGELOG"
expect_refused "a version prefix finds nothing" 1 'no "## [0.2]" entry'

# --- Written fixtures ---

cat > "$WORK/duplicate.md" <<'MD'
# Changelog

## [1.0.0] - 2026-01-01

- One.

## [1.0.0] - 2026-01-02

- Two.
MD
run duplicate "1.0.0" "$WORK/duplicate.md"
expect_refused "a version with two entries is refused" 1 "more than one"

cat > "$WORK/undated.md" <<'MD'
# Changelog

## [1.0.0]

- One.
MD
run undated "1.0.0" "$WORK/undated.md"
expect_refused "an entry with no date is refused" 1 'is not "## [1.0.0] - YYYY-MM-DD"'

cat > "$WORK/empty.md" <<'MD'
# Changelog

## [1.1.0] - 2026-02-01


## [1.0.0] - 2026-01-01

- One.

[1.1.0]: https://example.invalid/1.1.0
MD
run empty "1.1.0" "$WORK/empty.md"
expect_refused "an entry with nothing in it is refused" 1 "is empty"

# The entry before the foot ends where the link definitions begin.
run last "1.0.0" "$WORK/empty.md"
if [ "$status" -eq 0 ] && [ "$out" = "- One." ]; then
  pass "the last entry ends at the link definitions after a blank line"
else
  fail "the last entry ends at the link definitions after a blank line" "exit $status, out '$out', err '$err'"
fi

run unreadable "1.0.0" "$WORK/no-such-file.md"
expect_refused "an unreadable changelog is refused" 1 "cannot read"

run usage
expect_refused "no version is a usage error" 64 "usage:"

if [ "$FAILURES" -ne 0 ]; then
  printf '%d case(s) failed\n' "$FAILURES"
  exit 1
fi
echo "every case passed"
