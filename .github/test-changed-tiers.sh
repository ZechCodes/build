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

# A fresh repo whose history is C0 (bridge + spa + desktop), C1 (bridge only,
# and none of it a file the relay is built from),
# C2 (spa only), C3 (desktop + a shell script + scripts/ + web/). Each case picks the
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
    at_got="$(grep -E '^(app|relay|bridge|e2e|desktop|scripts|shell)=' "$at_out" | sort | tr '\n' ' ')"
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
assert_tiers "$name" "$WORK/1" "app=true relay=true bridge=true e2e=true desktop=true scripts=true shell=true" && pass "$name"

name="a :latest tag says nothing about the commit: everything moved"
run_case "$REPO" "$WORK/2" "$APP:latest" "$RELAY:latest"
assert_tiers "$name" "$WORK/2" "app=true relay=true bridge=true e2e=true desktop=true scripts=true shell=true" && pass "$name"

name="both tiers on HEAD: nothing moved"
run_case "$REPO" "$WORK/3" "$APP:$C3" "$RELAY:$C3"
assert_tiers "$name" "$WORK/3" "app=false relay=false bridge=false e2e=false desktop=false scripts=false shell=false" && pass "$name"

name="a relay tag behind only bridge-only commits is current: with the app on HEAD nothing rolls or rechecks"
run_case "$REPO" "$WORK/4" "$APP:$C3" "$RELAY:$C0"
assert_tiers "$name" "$WORK/4" "app=false relay=false bridge=false e2e=false desktop=false scripts=false shell=false" && pass "$name"

name="app behind by a spa commit, relay current: app alone"
run_case "$REPO" "$WORK/5" "$APP:$C1" "$RELAY:$C3"
assert_tiers "$name" "$WORK/5" "app=true relay=false bridge=false e2e=true desktop=true scripts=true shell=true" && pass "$name"

name="a cancelled run's bridge checks are still owed when the next merge touches only spa, and roll no relay"
run_case "$REPO" "$WORK/6" "$APP:$C0" "$RELAY:$C0"
assert_tiers "$name" "$WORK/6" "app=true relay=false bridge=true e2e=true desktop=true scripts=true shell=true" && pass "$name"

name="checks that deploy nothing diff from the older of the two tiers"
run_case "$REPO" "$WORK/7" "$APP:$C2" "$RELAY:$C2"
assert_tiers "$name" "$WORK/7" "app=false relay=false bridge=false e2e=true desktop=true scripts=true shell=true" && pass "$name"

name="a deployed commit this history does not contain moves that tier and every check, not the tier that is current"
run_case "$REPO" "$WORK/8" "$APP:0123456789abcdef0123456789abcdef01234567" "$RELAY:$C3"
assert_tiers "$name" "$WORK/8" "app=true relay=false bridge=true e2e=true desktop=true scripts=true shell=true" && pass "$name"

# The landing page is built into the app image by skriftapp/Containerfile, so a
# page-only commit has to move the app tier — and only that tier.
LANDING_REPO="$WORK/landing-repo"
make_repo "$LANDING_REPO"
LANDING_BASE="$(sha_of "$LANDING_REPO" HEAD)"
mkdir -p "$LANDING_REPO/landing/src/pages"
echo a > "$LANDING_REPO/landing/src/pages/index.astro"
git -C "$LANDING_REPO" add -A && git -C "$LANDING_REPO" commit -qm C4

name="the landing page ships in the app image: a page-only commit moves the app tier"
run_case "$LANDING_REPO" "$WORK/9" "$APP:$LANDING_BASE" "$RELAY:$LANDING_BASE"
assert_tiers "$name" "$WORK/9" "app=true relay=false bridge=false e2e=true desktop=false scripts=false shell=false" && pass "$name"

# The relay deploys only when a file it is built from moved (relay-sources);
# every other bridge change is checked and end-to-end tested, and rolls nothing.
# Each case puts production on the fixture's HEAD and adds one commit.
change_case() {
    cc_name="$1"
    cc_path="$2"
    cc_want="$3"
    cc_repo="$WORK/change-$4"
    make_repo "$cc_repo"
    cc_base="$(sha_of "$cc_repo" HEAD)"
    mkdir -p "$(dirname "$cc_repo/$cc_path")"
    echo changed >> "$cc_repo/$cc_path"
    git -C "$cc_repo" add -A && git -C "$cc_repo" commit -qm change
    run_case "$cc_repo" "$cc_repo.out" "$APP:$cc_base" "$RELAY:$cc_base"
    assert_tiers "$cc_name" "$cc_repo.out" "$cc_want" && pass "$cc_name"
}

MOVED_BRIDGE="app=false relay=false bridge=true e2e=true desktop=false scripts=false shell=false"
MOVED_RELAY="app=false relay=true bridge=true e2e=true desktop=false scripts=false shell=false"

change_case "a bridge-only change is checked and never rolls the relay" \
    bridge/src/store.rs "$MOVED_BRIDGE" store
change_case "the patched webrtc under bridge/vendor is not in the relay" \
    bridge/vendor/rtc/src/lib.rs "$MOVED_BRIDGE" vendor
change_case "the relay bin moves the relay" \
    bridge/src/bin/relay.rs "$MOVED_RELAY" bin
change_case "a bridge module the relay uses moves the relay" \
    bridge/src/relay_server.rs "$MOVED_RELAY" relay-server
change_case "a module the relay reaches through relay_server moves the relay" \
    bridge/src/transport.rs "$MOVED_RELAY" transport
change_case "the relay manifest alone moves the relay, and the checks that gate its image" \
    deploy/k8s/relay.yaml "$MOVED_RELAY" manifest
change_case "a lockfile change moves the relay" \
    bridge/Cargo.lock "$MOVED_RELAY" lockfile
change_case "the image recipe moves the relay" \
    bridge/Containerfile "$MOVED_RELAY" containerfile
change_case "a path that only contains a relay source's name is not one" \
    bridge/src/bin/relay.rs.orig "$MOVED_BRIDGE" lookalike

name="every path relay-sources lists exists in this tree"
missing="$(grep -Ev '^(#|$)' "$SCRIPT_DIR/relay-sources" | while IFS= read -r path; do
    [ -e "$SCRIPT_DIR/../$path" ] || printf ' %s' "$path"
done)"
if [ -z "$missing" ]; then pass "$name"; else fail "$name" "missing:$missing"; fi

name="the summary goes to stderr, only key=value lines to stdout"
if [ "$(grep -cvE '^[a-z0-9]+=(true|false)$' "$WORK/3")" = "0" ] && [ -s "$WORK/3.err" ]; then
    pass "$name"
else
    fail "$name" "stdout: $(cat "$WORK/3"); stderr: $(cat "$WORK/3.err")"
fi

[ "$FAILURES" -eq 0 ]
