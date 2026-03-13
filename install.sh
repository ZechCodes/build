#!/bin/sh
# Build — https://getbuild.ing
# Usage: curl -sSf getbuild.ing | sh
set -eu

BOLD='\033[1m'
DIM='\033[2m'
CYAN='\033[36m'
GREEN='\033[32m'
RED='\033[31m'
RESET='\033[0m'

API_URL="https://getbuild.ing/v2/api/campaign-signups"

# --- Header ---
clear
printf "\n"
printf "  ${BOLD}Build${RESET}\n"
printf "  ${DIM}Local-first agent orchestration${RESET}\n"
printf "  ${DIM}https://getbuild.ing${RESET}\n"
printf "\n"
printf "  ${DIM}--------------------------------${RESET}\n"
printf "\n"

# --- Email prompt ---
printf "  ${CYAN}=>${RESET} Enter your email to join the waitlist:\n"
printf "\n"
printf "    ${BOLD}Email:${RESET} "
read -r EMAIL < /dev/tty

# Validate
case "$EMAIL" in
  *@*.*) ;;
  *) printf "\n  ${RED}x${RESET} That doesn't look like a valid email.\n\n"; exit 1 ;;
esac

printf "\n"

# --- Braille spinner ---
set -- "⠋" "⠙" "⠹" "⠸" "⠼" "⠴" "⠦" "⠧" "⠇" "⠏"
SPIN_MSG="  Registering"
END_TIME=$(($(date +%s) + 2))
I=1

# Fire off the request in the background
STATUS_FILE=$(mktemp)
(
  HTTP_CODE=$(curl -s -o /dev/null -w "%{http_code}" \
    -X POST "$API_URL" \
    -H "Content-Type: application/json" \
    -d "{\"campaign_slug\":\"build-launch\",\"email\":\"$EMAIL\"}" 2>/dev/null || echo "000")
  echo "$HTTP_CODE" > "$STATUS_FILE"
) &
BG_PID=$!

# Spin for at least 2 seconds
while true; do
  IDX=$(( (I - 1) % 10 + 1 ))
  eval "CHAR=\${$IDX}"
  printf "\r${SPIN_MSG} ${CYAN}${CHAR}${RESET} "
  I=$((I + 1))
  sleep 0.08

  NOW=$(date +%s)
  if [ "$NOW" -ge "$END_TIME" ] && ! kill -0 "$BG_PID" 2>/dev/null; then
    break
  fi
done

# Wait for background request to finish
wait "$BG_PID" 2>/dev/null || true
printf "\r                              \r"

# --- Result ---
HTTP_CODE=$(cat "$STATUS_FILE" 2>/dev/null || echo "000")
rm -f "$STATUS_FILE"

printf "\n"
case "$HTTP_CODE" in
  201)
    printf "  ${GREEN}✓${RESET} ${BOLD}You're on the list!${RESET}\n"
    printf "\n"
    printf "  ${DIM}Check your inbox to confirm your email.${RESET}\n"
    printf "  ${DIM}We'll reach out as soon as Build is ready.${RESET}\n"
    ;;
  409)
    printf "  ${GREEN}✓${RESET} ${BOLD}You're already signed up!${RESET}\n"
    printf "\n"
    printf "  ${DIM}We'll reach out as soon as Build is ready.${RESET}\n"
    ;;
  *)
    printf "  ${RED}x${RESET} Something went wrong (${HTTP_CODE}).\n"
    printf "\n"
    printf "  ${DIM}Try again or sign up at https://getbuild.ing${RESET}\n"
    ;;
esac

printf "\n"
printf "  ${DIM}--------------------------------${RESET}\n"
printf "  ${DIM}Thanks for your interest in Build.${RESET}\n"
printf "\n"
