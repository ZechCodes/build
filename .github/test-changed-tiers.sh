#!/bin/sh
# Behaviour tests for changed-tiers.sh, run against a throwaway git repo. Each
# case builds a short history of commits that touch known paths, declares what
# production is running per tier, and checks which tiers the script says moved.
#
# Usage: .github/test-changed-tiers.sh   (0 every case passed, 1 otherwise)
set -eu

SCRIPT_DIR="$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd)"
CHANGED_TIERS="$SCRIPT_DIR/changed-tiers.sh"

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT INT TERM
FAILURES=0

pass() {
    printf 'ok   %s\n' "$1"
}

fail() {
    printf 'FAIL %s: %s\n' "$1" "$2"
    FAILURES=$((FAILURES + 1))
}

# A fresh repo whose history is C0 (bridge + spa + desktop), C1 (bridge only),
# C2 (spa only), C3 (desktop + a shell script + scripts/). Each case picks the
# commits production is on and reads the tiers the script reports.
make_repo() {
    mr_dir="$1"
    mkdir -p "$mr_dir"
    git -C "$mr_dir" init -q -b main
    git -C "$mr_dir" config user.email ci@test
    git -C "$mr_dir" config user.name ci
    mkdir -p "$mr_dir/bridge/src" "$mr_dir/spa/src" "$mr_dir/desktop" "$mr_dir/scripts" "$mr_dir/web"
    echo a > "$mr_dir/bridge/src/main.rs"
    echo a > "$mr_dir/spa/src/main.js"
    echo a > "$mr_dir/desktop/main.js"
    git -C "$mr_dir" add -A && git -C "$mr_dir" commit -qm C0
    echo b > "$mr_dir/bridge/src/main.rs"
    git -C "$mr_dir" add -A && git -C "$mr_dir" commit -qm C1
    echo b > "$mr_dir/spa/src/main.js"
    git -C "$mr_dir" add -A && git -C "$mr_dir" commit -qm C2
    echo b > "$mr_dir/desktop/main.js"
    echo b > "$mr_dir/scripts/mirror.sh"
    echo b > "$mr_dir/web/e2e.mjs"
    git -C "$mr_dir" add -A && git -C "$mr_dir" commit -qm C3
}

sha_of() {
    git -C "$1" rev-parse "$2"
}

# Run the script with the given deployed images; the outputs land in a file
# so a case can assert on each key.
run_case() {
    rc_repo="$1"
    rc_out="$2"
    APP_IMAGE="$3" RELAY_IMAGE="$4" "$CHANGED_TIERS" "$rc_repo" > "$rc_out" 2> "$rc_out.err"
}

assert_tiers() {
    at_name="$1"
    at_out="$2"
    at_want="$3"
    at_got="$(grep -E '^(app|relay|e2e|desktop|scripts|shell)=' "$at_out" | sort | tr '\n' ' ')"
    at_want_sorted="$(printf '%s\n' "$at_want" | tr ' ' '\n' | sort | tr '\n' ' ')"
    [ "$at_got" = "$at_want_sorted" ] && return 0
    fail "$at_name" "got '$at_got', wanted '$at_want_sorted'"
    return 1
}

REPO="$WORK/repo"
make_repo "$REPO"
C0="$(sha_of "$REPO" HEAD~3)"
C1="$(sha_of "$REPO" HEAD~2)"
C2="$(sha_of "$REPO" HEAD~1)"
C3="$(sha_of "$REPO" HEAD)"
APP="ghcr.io/zechcodes/build-app"
RELAY="ghcr.io/zechcodes/build-relay"

name="nothing known about production: everything moved"
run_case "$REPO" "$WORK/1" "" ""
assert_tiers "$name" "$WORK/1" "app=true relay=true e2e=true desktop=true scripts=true shell=true" && pass "$name"

name="a :latest tag says nothing about the commit: everything moved"
run_case "$REPO" "$WORK/2" "$APP:latest" "$RELAY:latest"
assert_tiers "$name" "$WORK/2" "app=true relay=true e2e=true desktop=true scripts=true shell=true" && pass "$name"

name="both tiers on HEAD: nothing moved"
run_case "$REPO" "$WORK/3" "$APP:$C3" "$RELAY:$C3"
assert_tiers "$name" "$WORK/3" "app=false relay=false e2e=false desktop=false scripts=false shell=false" && pass "$name"

name="relay behind by a bridge commit, app current: relay alone, and the stack it is tested in"
run_case "$REPO" "$WORK/4" "$APP:$C3" "$RELAY:$C0"
assert_tiers "$name" "$WORK/4" "app=false relay=true e2e=true desktop=true scripts=true shell=true" && pass "$name"

name="app behind by a spa commit, relay current: app alone"
run_case "$REPO" "$WORK/5" "$APP:$C1" "$RELAY:$C3"
assert_tiers "$name" "$WORK/5" "app=true relay=false e2e=true desktop=true scripts=true shell=true" && pass "$name"

name="a cancelled run's bridge commit is still owed when the next merge touches only spa"
run_case "$REPO" "$WORK/6" "$APP:$C0" "$RELAY:$C0"
assert_tiers "$name" "$WORK/6" "app=true relay=true e2e=true desktop=true scripts=true shell=true" && pass "$name"

name="checks that deploy nothing diff from the older of the two tiers"
run_case "$REPO" "$WORK/7" "$APP:$C2" "$RELAY:$C2"
assert_tiers "$name" "$WORK/7" "app=false relay=false e2e=true desktop=true scripts=true shell=true" && pass "$name"

name="a deployed commit this history does not contain moves that tier and every check, not the tier that is current"
run_case "$REPO" "$WORK/8" "$APP:0123456789abcdef0123456789abcdef01234567" "$RELAY:$C3"
assert_tiers "$name" "$WORK/8" "app=true relay=false e2e=true desktop=true scripts=true shell=true" && pass "$name"

name="the summary goes to stderr, only key=value lines to stdout"
if [ "$(grep -cvE '^[a-z0-9]+=(true|false)$' "$WORK/3")" = "0" ] && [ -s "$WORK/3.err" ]; then
    pass "$name"
else
    fail "$name" "stdout: $(cat "$WORK/3"); stderr: $(cat "$WORK/3.err")"
fi

[ "$FAILURES" -eq 0 ]
