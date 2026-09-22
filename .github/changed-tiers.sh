#!/bin/sh
# Which tiers of the tree have moved since production last picked them up.
#
# The answer is read from production itself, never from "the previous push":
# each tier's Deployment carries the commit it was built from as its image
# tag, and a tier has moved if any of its paths differ between that commit
# and HEAD. This is what lets a run be cancelled or go red without losing a
# deploy — the next run still sees every commit production has not caught up
# with — and it is what a manual dispatch needs to roll only what is behind.
#
# A tier that cannot be diffed with confidence counts as moved, so the failure
# mode is a redundant deploy and never a skipped one: an image tag that is not
# a commit (`:latest`, an unreadable cluster), or a commit this history does
# not contain (a force-push, a hand deploy from a branch). The other tier is
# still judged on its own — the relay's rollout is a Recreate that drops every
# WebSocket, and a tier already at HEAD is not made to pay that.
#
# Usage: APP_IMAGE=… RELAY_IMAGE=… .github/changed-tiers.sh [repo-dir]
#   stdout: one `tier=true|false` line per tier, ready for $GITHUB_OUTPUT
#   stderr: the same, as one line for a human
set -eu

REPO="${1:-.}"

APP_PATHS='^(spa|skriftapp|landing)/|^deploy/k8s/app\.yaml$|^scripts/install(-desktop)?\.sh$'
RELAY_PATHS='^bridge/|^deploy/k8s/relay\.yaml$'
E2E_HARNESS_PATHS='^web/|^deploy/compose\.real\.yml$'

# The commit an image tag names, or empty when the tag is not a commit this
# history can diff from HEAD.
deployed_commit() {
    dc_tag="${1##*:}"
    case "$dc_tag" in
        *[!0-9a-f]* | "") return 0 ;;
    esac
    [ "${#dc_tag}" -eq 40 ] || return 0
    git -C "$REPO" merge-base --is-ancestor "$dc_tag" HEAD 2> /dev/null || return 0
    printf '%s\n' "$dc_tag"
}

# The older of two known commits — the base for checks that deploy nothing,
# so they cover everything either tier is still owed. Either unknown, or two
# commits that are not on one line, means the base is unknown.
older_commit() {
    [ -n "$1" ] && [ -n "$2" ] || return 0
    if git -C "$REPO" merge-base --is-ancestor "$1" "$2"; then
        printf '%s\n' "$1"
    elif git -C "$REPO" merge-base --is-ancestor "$2" "$1"; then
        printf '%s\n' "$2"
    fi
}

# true when any path matching the pattern differs between the base and HEAD,
# and always true when there is no base to diff from.
moved_since() {
    [ -n "$1" ] || { echo true; return 0; }
    if git -C "$REPO" diff --name-only "$1" HEAD | grep -Eq "$2"; then
        echo true
    else
        echo false
    fi
}

app_base="$(deployed_commit "${APP_IMAGE:-}")"
relay_base="$(deployed_commit "${RELAY_IMAGE:-}")"
checks_base="$(older_commit "$app_base" "$relay_base")"

app="$(moved_since "$app_base" "$APP_PATHS")"
relay="$(moved_since "$relay_base" "$RELAY_PATHS")"
desktop="$(moved_since "$checks_base" '^desktop/|^scripts/build-desktop\.mjs$|^\.github/workflows/(ci|release-desktop)\.yml$')"
scripts="$(moved_since "$checks_base" '^scripts/')"
shell="$(moved_since "$checks_base" '\.sh$')"
# The end-to-end suite exercises both tiers, so it runs when either moved and
# when its own harness did — or a change to the test would skip the test.
if [ "$app" = true ] || [ "$relay" = true ]; then
    e2e=true
else
    e2e="$(moved_since "$checks_base" "$E2E_HARNESS_PATHS")"
fi

for tier in app relay e2e desktop scripts shell; do
    eval "printf '%s=%s\n' \"$tier\" \"\$$tier\""
done
printf 'production: app@%s relay@%s → app=%s relay=%s e2e=%s desktop=%s scripts=%s shell=%s\n' \
    "${app_base:-unknown}" "${relay_base:-unknown}" "$app" "$relay" "$e2e" "$desktop" "$scripts" "$shell" >&2
