#!/usr/bin/env bash
# Apple notarization with a bounded wait and a way back in (#293).
#
# `notarytool submit --wait` has no ceiling of its own: on the bridge 0.2.3
# release both macOS legs sat in it until GitHub cancelled them at six hours,
# and the submission ids Apple had issued were only in the log. So this
# submits WITHOUT waiting, publishes the id at once (a job notice, the step
# summary, and a step output), then polls `notarytool info`
# for at most NOTARIZE_BUDGET_SECONDS and fails cleanly, id in hand, when Apple
# is slower than that.
#
# Given a submission id instead, it submits nothing and polls that one: a
# submission Apple finished after we gave up is picked up without re-signing.
# Either way the ticket must name the binary we are about to ship — its
# CDHash, which re-signing the same bytes reproduces — or this fails, because
# an Accepted ticket for different bytes notarizes nothing.
#
# A bare Mach-O cannot be stapled; Gatekeeper fetches the ticket from Apple
# by CDHash. So Accepted is the end of it here.
#
#   scripts/notarize.sh ZIP BINARY                # submit ZIP, then poll
#   scripts/notarize.sh ZIP BINARY SUBMISSION_ID  # poll an existing submission
#
# ZIP is the archive to submit (unused when resuming); BINARY is the signed
# binary inside it, whose CDHash the ticket must list.
#
# Needs: APPLE_ID APPLE_TEAM_ID APPLE_APP_PASSWORD, xcrun notarytool, codesign
#   and jq (all on GitHub's macOS runners).
# Knobs: NOTARIZE_BUDGET_SECONDS (5400) NOTARIZE_POLL_SECONDS (30)
#   NOTARIZE_LABEL (the binary's name; names the leg in the summary)
#   RESUME_HINT (a line for the summary saying how to resume; {id} is replaced
#     with the submission id)
#   NOTARIZE_OUTPUT (submission-id; the step output the id is written to)
set -euo pipefail

zip="${1:?usage: notarize.sh ZIP BINARY [SUBMISSION_ID]}"
binary="${2:?usage: notarize.sh ZIP BINARY [SUBMISSION_ID]}"
resume="${3:-}"
budget="${NOTARIZE_BUDGET_SECONDS:-5400}"
interval="${NOTARIZE_POLL_SECONDS:-30}"
label="${NOTARIZE_LABEL:-$(basename "$binary")}"
: "${APPLE_ID:?}" "${APPLE_TEAM_ID:?}" "${APPLE_APP_PASSWORD:?}"

creds=(--apple-id "$APPLE_ID" --team-id "$APPLE_TEAM_ID" --password "$APPLE_APP_PASSWORD")
scratch="$(mktemp -d)"
trap 'rm -rf "$scratch"' EXIT

summary() {
  if [ -n "${GITHUB_STEP_SUMMARY:-}" ]; then
    printf '%s\n' "$@" >> "$GITHUB_STEP_SUMMARY"
  fi
}

if [ -n "$resume" ]; then
  # The id arrives from a workflow_dispatch input; it is a UUID or it is not
  # an id, and nothing else reaches notarytool's argv.
  if ! [[ "$resume" =~ ^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$ ]]; then
    echo "::error::'${resume}' is not a notarization submission id"
    exit 1
  fi
  id="$resume"
  echo "::notice::${label}: resuming notarization submission ${id}; nothing is resubmitted"
  summary "**${label}**: resuming notarization submission \`${id}\` (not resubmitted)."
else
  xcrun notarytool submit "$zip" "${creds[@]}" --output-format json > "$scratch/submit.json"
  id="$(jq -r '.id // empty' "$scratch/submit.json")"
  if [ -z "$id" ]; then
    echo "::error::${label}: notarytool submit returned no submission id"
    cat "$scratch/submit.json"
    exit 1
  fi
  echo "::notice::${label}: notarization submission ${id}"
  summary "**${label}**: notarization submission \`${id}\`."
fi
if [ -n "${GITHUB_OUTPUT:-}" ]; then
  echo "${NOTARIZE_OUTPUT:-submission-id}=${id}" >> "$GITHUB_OUTPUT"
fi

# `info` failing is a network blip or Apple having a moment, not a verdict:
# it is retried until the budget runs out like any other not-yet.
deadline=$((SECONDS + budget))
status=""
while :; do
  if xcrun notarytool info "$id" "${creds[@]}" --output-format json > "$scratch/info.json" 2> "$scratch/info.err"; then
    status="$(jq -r '.status // empty' "$scratch/info.json")"
    echo "$(date -u +%H:%M:%SZ) ${id}: ${status:-no status}"
  else
    echo "::warning::${label}: notarytool info ${id} failed; retrying"
    cat "$scratch/info.err" >&2
    status=""
  fi
  case "$status" in
    Accepted | Invalid | Rejected) break ;;
  esac
  if [ "$SECONDS" -ge "$deadline" ]; then
    hint="${RESUME_HINT:-}"
    if [ -z "$hint" ]; then hint="Resume by passing \`{id}\` back to this script."; fi
    echo "::error::${label}: Apple has not finished notarization submission ${id} after ${budget}s (last status: ${status:-unknown})"
    summary "**${label}**: gave up waiting after ${budget}s with Apple at \`${status:-unknown}\`." \
      "${hint//\{id\}/$id}"
    exit 1
  fi
  sleep "$interval"
done

# The log is the only place Apple says why, and for Accepted it is where the
# ticket's CDHashes are listed. It can trail the verdict by a moment.
tries=0
until xcrun notarytool log "$id" "${creds[@]}" "$scratch/log.json"; do
  tries=$((tries + 1))
  if [ "$tries" -ge 5 ]; then
    echo "::error::${label}: Apple answered ${status} for submission ${id}, but its log could not be fetched"
    exit 1
  fi
  sleep "$interval"
done
if [ "$status" != "Accepted" ]; then
  echo "::error::${label}: Apple answered ${status} for notarization submission ${id}"
  jq . "$scratch/log.json"
  summary "**${label}**: Apple answered \`${status}\`; the notarization log is in the job output."
  exit 1
fi

cdhash="$(codesign --display --verbose=3 "$binary" 2>&1 | sed -n 's/^CDHash=//p' | head -n 1)"
if [ -z "$cdhash" ]; then
  echo "::error::${label}: codesign reports no CDHash for ${binary}"
  exit 1
fi
if ! jq -e --arg cdhash "$cdhash" \
  '[.ticketContents[]? | .cdhash // empty | ascii_downcase] | index($cdhash | ascii_downcase) != null' \
  "$scratch/log.json" > /dev/null; then
  echo "::error::${label}: submission ${id} was Accepted for other bytes: its ticket does not list this binary's CDHash ${cdhash}"
  jq '.ticketContents' "$scratch/log.json"
  summary "**${label}**: submission \`${id}\` is Accepted but its ticket does not cover this build (CDHash \`${cdhash}\`); submit it afresh."
  exit 1
fi
echo "${label}: notarized, submission ${id}, CDHash ${cdhash}"
summary "**${label}**: notarized (submission \`${id}\`, CDHash \`${cdhash}\`)."
