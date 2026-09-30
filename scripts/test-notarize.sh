#!/usr/bin/env bash
# Behaviour tests for notarize.sh, run offline on any machine. `xcrun` and
# `codesign` are shims in front of PATH: each case scripts the statuses Apple
# answers `notarytool info` with and the CDHashes the ticket lists, and the
# shim logs every notarytool call it gets. What is under test is what the
# script does with Apple's answers — the bounded wait, the resume path, the
# log on a refusal, the ticket that must cover this binary — not notarytool.
#
# Usage: scripts/test-notarize.sh   (0 every case passed, 1 otherwise)
set -euo pipefail

SCRIPT_DIR="$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd)"
NOTARIZE_SH="$SCRIPT_DIR/notarize.sh"

SUBMITTED_ID="11111111-2222-3333-4444-555555555555"
RESUMED_ID="48d83fe6-d384-499f-b72f-d4999bac2633"
BINARY_CDHASH="0123456789abcdef0123456789abcdef01234567"
TEST_HINT="RESUME-HINT -f id={id}"

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT INT TERM
FAILURES=0

pass() { printf 'ok   %s\n' "$1"; }
fail() {
  printf 'FAIL %s: %s\n' "$1" "$2"
  FAILURES=$((FAILURES + 1))
}

mkdir -p "$WORK/bin"
# notarytool, as far as notarize.sh uses it. `info` pops the next line of
# $FAKE/statuses each call (the last one sticks); a line reading `error` fails
# that call, and `_` stands for a space, so "In_Progress" answers "In Progress".
cat > "$WORK/bin/xcrun" <<'SHIM'
#!/usr/bin/env bash
set -euo pipefail
[ "$1" = notarytool ] || exit 64
shift
echo "$*" >> "$FAKE/calls"
case "$1" in
  submit)
    printf '{"id":"%s","message":"Successfully uploaded file","path":"%s"}\n' "$FAKE_SUBMIT_ID" "$2" ;;
  info)
    status="$(head -n 1 "$FAKE/statuses")"
    if [ "$(wc -l < "$FAKE/statuses")" -gt 1 ]; then sed -i.bak 1d "$FAKE/statuses"; fi
    if [ "$status" = error ]; then echo "Error: HTTP status code: 503" >&2; exit 69; fi
    status="${status//_/ }"
    printf '{"id":"%s","status":"%s"}\n' "$2" "$status" ;;
  log)
    for arg in "$@"; do out="$arg"; done
    printf '{"status":"%s","issues":[{"message":"FAKE-LOG-ISSUE"}],"ticketContents":[{"path":"build-bridge.zip/build-bridge","cdhash":"%s"}]}\n' \
      "$(tail -n 1 "$FAKE/statuses")" "$FAKE_TICKET_CDHASH" > "$out" ;;
  *) exit 64 ;;
esac
SHIM
cat > "$WORK/bin/codesign" <<SHIM
#!/bin/sh
printf 'Executable=%s\nCDHash=%s\n' "\$3" "$BINARY_CDHASH" >&2
SHIM
chmod +x "$WORK/bin/xcrun" "$WORK/bin/codesign"

# Runs notarize.sh against a fresh fake. $1 names the case; $2 the statuses,
# space-separated; $3 the CDHash the ticket lists; the rest are its arguments.
run() {
  local name="$1" statuses="$2" ticket="$3"
  shift 3
  export FAKE="$WORK/$name"
  mkdir -p "$FAKE"
  : > "$FAKE/calls"
  tr ' ' '\n' <<< "$statuses" > "$FAKE/statuses"
  : > "$FAKE/summary"
  : > "$FAKE/output"
  set +e
  PATH="$WORK/bin:$PATH" FAKE_SUBMIT_ID="$SUBMITTED_ID" FAKE_TICKET_CDHASH="$ticket" \
    APPLE_ID=a@example.com APPLE_TEAM_ID=TEAM APPLE_APP_PASSWORD=pw \
    NOTARIZE_POLL_SECONDS=0 NOTARIZE_BUDGET_SECONDS="${BUDGET:-60}" NOTARIZE_LABEL=macos-arm64 \
    RESUME_HINT="${RESUME_HINT-$TEST_HINT}" GITHUB_STEP_SUMMARY="$FAKE/summary" GITHUB_OUTPUT="$FAKE/output" \
    "$NOTARIZE_SH" "$WORK/build-bridge.zip" "$WORK/build-bridge" "$@" > "$FAKE/stdout" 2>&1
  STATUS=$?
  set -e
}

expect() {
  local name="$1" want="$2"
  if [ "$STATUS" -ne "$want" ]; then
    fail "$name" "exit $STATUS, wanted $want; output:"
    sed 's/^/     /' "$FAKE/stdout"
    return 1
  fi
}

has() {
  local name="$1" file="$2" text="$3"
  grep -qF -- "$text" "$FAKE/$file" || { fail "$name" "$file lacks '$text'"; return 1; }
}

lacks() {
  local name="$1" file="$2" text="$3"
  ! grep -qF -- "$text" "$FAKE/$file" || { fail "$name" "$file has '$text'"; return 1; }
}

case_submits_publishes_the_id_and_waits_for_accepted() {
  local n=submit-then-accepted
  run "$n" "In_Progress In_Progress Accepted" "$BINARY_CDHASH"
  expect "$n" 0 &&
    has "$n" calls "submit $WORK/build-bridge.zip" &&
    has "$n" calls "--output-format json" &&
    lacks "$n" calls "--wait" &&
    has "$n" output "submission-id=$SUBMITTED_ID" &&
    has "$n" summary "$SUBMITTED_ID" &&
    has "$n" stdout "::notice::macos-arm64: notarization submission $SUBMITTED_ID" || return
  local polls
  polls="$(grep -c '^info ' "$FAKE/calls")"
  [ "$polls" -eq 3 ] || { fail "$n" "polled $polls times, wanted 3"; return; }
  pass "$n"
}

case_resume_polls_the_given_id_and_submits_nothing() {
  local n=resume
  run "$n" "Accepted" "$BINARY_CDHASH" "$RESUMED_ID"
  expect "$n" 0 &&
    lacks "$n" calls "submit" &&
    has "$n" calls "info $RESUMED_ID" &&
    has "$n" output "submission-id=$RESUMED_ID" &&
    has "$n" summary "resuming notarization submission \`$RESUMED_ID\`" || return
  pass "$n"
}

case_slow_apple_fails_with_the_id_and_the_resume_hint() {
  local n=budget-spent
  BUDGET=0 run "$n" "In_Progress" "$BINARY_CDHASH"
  expect "$n" 1 &&
    has "$n" stdout "::error::macos-arm64: Apple has not finished notarization submission $SUBMITTED_ID" &&
    has "$n" summary "$SUBMITTED_ID" &&
    has "$n" summary "RESUME-HINT -f id=$SUBMITTED_ID" &&
    has "$n" output "submission-id=$SUBMITTED_ID" &&
    lacks "$n" calls "log " || return
  pass "$n"
}

case_the_default_resume_hint_names_the_id() {
  local n=default-hint
  RESUME_HINT="" BUDGET=0 run "$n" "In_Progress" "$BINARY_CDHASH"
  expect "$n" 1 &&
    has "$n" summary "Resume by passing \`$SUBMITTED_ID\` back to this script." &&
    lacks "$n" summary "}" || return
  pass "$n"
}

case_invalid_prints_apples_log() {
  local n=invalid
  run "$n" "In_Progress Invalid" "$BINARY_CDHASH"
  expect "$n" 1 &&
    has "$n" calls "log $SUBMITTED_ID" &&
    has "$n" stdout "FAKE-LOG-ISSUE" &&
    has "$n" stdout "Apple answered Invalid" || return
  pass "$n"
}

case_rejected_is_a_verdict_too() {
  local n=rejected
  run "$n" "Rejected" "$BINARY_CDHASH"
  expect "$n" 1 && has "$n" stdout "Apple answered Rejected" || return
  pass "$n"
}

case_ticket_for_other_bytes_fails() {
  local n=other-bytes
  run "$n" "Accepted" "ffffffffffffffffffffffffffffffffffffffff" "$RESUMED_ID"
  expect "$n" 1 && has "$n" stdout "does not list this binary's CDHash $BINARY_CDHASH" || return
  pass "$n"
}

case_ticket_cdhash_case_does_not_matter() {
  local n=cdhash-case
  run "$n" "Accepted" "${BINARY_CDHASH^^}"
  expect "$n" 0 || return
  pass "$n"
}

case_the_output_name_is_the_callers() {
  local n=output-name
  NOTARIZE_OUTPUT=notarization-id-macos-arm64 run "$n" "Accepted" "$BINARY_CDHASH"
  expect "$n" 0 && has "$n" output "notarization-id-macos-arm64=$SUBMITTED_ID" || return
  pass "$n"
}

case_info_errors_are_retried() {
  local n=info-flaky
  run "$n" "error error Accepted" "$BINARY_CDHASH"
  expect "$n" 0 && has "$n" stdout "::warning::macos-arm64: notarytool info $SUBMITTED_ID failed; retrying" || return
  pass "$n"
}

case_a_resume_id_that_is_not_a_uuid_reaches_nothing() {
  local n=bad-id
  # shellcheck disable=SC2016 # the literal text is the point
  run "$n" "Accepted" "$BINARY_CDHASH" '$(touch pwned)'
  expect "$n" 1 || return
  [ ! -s "$FAKE/calls" ] || { fail "$n" "notarytool was called"; return; }
  pass "$n"
}

for case_fn in $(declare -F | awk '{print $3}' | grep '^case_'); do
  "$case_fn" || true
done

if [ "$FAILURES" -ne 0 ]; then
  echo "$FAILURES case(s) failed"
  exit 1
fi
echo "all notarize.sh cases passed"
