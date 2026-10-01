#!/bin/sh
# Install the Build bridge — the device daemon that runs coding agents on this
# machine. Published verbatim as a release asset and served from
# https://getbuild.ing/install.sh, so it is run as:
#
#   curl -fsSL https://getbuild.ing/install.sh | sh
#
# stdin is the curl pipe, so nothing here may prompt. Every message goes to
# stderr, as a short header and one line per step; a failure is the only thing
# that names URLs and files. Colour and bold are used only when stderr is a
# terminal, NO_COLOR is unset and TERM is not dumb. stdout carries whatever
# `build-bridge pair` and `install-service` print there and, when stdout is
# not a terminal, ends on `installed build-bridge <path>` after a success; a
# failure adds nothing to it.
#
# Environment:
#   BUILD_BRIDGE_VERSION       `latest` (default) or `X.Y.Z`
#   BUILD_BRIDGE_INSTALL_DIR   where the binary lands (default ~/.local/bin)
#   BUILD_RELEASES_REPO        GitHub owner/name holding the release assets
#   BUILD_BRIDGE_SKIP_SERVICE  `1` stops after installing the binary
#   BRIDGE_WEB_URL, BRIDGE_API_URL  read as the bridge reads them, to name the
#                              web app the last line sends the person to
#
# Exit codes: 0 installed, 1 something failed, 2 unsupported platform.
set -eu

DEFAULT_REPO="ZechCodes/build-releases"
COSIGN_IDENTITY_REGEXP='^https://github\.com/ZechCodes/(build-web|build)/\.github/workflows/release\.yml@refs/tags/bridge-v'
COSIGN_ISSUER='https://token.actions.githubusercontent.com'
ASSET_PREFIX="build-bridge-"
CHECKSUMS="SHA256SUMS"
BUNDLE="SHA256SUMS.sigstore.json"

REPO="${BUILD_RELEASES_REPO:-$DEFAULT_REPO}"
VERSION="${BUILD_BRIDGE_VERSION:-latest}"
INSTALL_DIR="${BUILD_BRIDGE_INSTALL_DIR:-$HOME/.local/bin}"
SKIP_SERVICE="${BUILD_BRIDGE_SKIP_SERVICE:-0}"

say() {
    printf '%s\n' "$*" >&2
}

# The escape sequences, or nothing: decided once, so every line below is
# written the same way whether or not it ends up styled.
if [ -t 2 ] && [ -z "${NO_COLOR:-}" ] && [ "${TERM:-}" != "dumb" ]; then
    BOLD="$(printf '\033[1m')"
    STEP="$(printf '\033[1;34m')"
    ACT="$(printf '\033[1;33m')"
    GOOD="$(printf '\033[1;32m')"
    BAD="$(printf '\033[1;31m')"
    RESET="$(printf '\033[0m')"
else
    BOLD=""
    STEP=""
    ACT=""
    GOOD=""
    BAD=""
    RESET=""
fi

step() {
    say "${STEP}==>${RESET} $*"
}

# The step a person has to act on, set apart from the rest.
action_step() {
    say ""
    say "${ACT}==> $1${RESET}"
    say "    $2"
}

# Lines under a step or a verdict; each argument may itself span lines.
detail() {
    for d_text in "$@"; do
        printf '%s\n' "$d_text" | while IFS= read -r d_line; do
            say "    $d_line"
        done
    done
}

finish() {
    say ""
    say "${GOOD}Done.${RESET} $1"
    shift
    detail "$@"
}

# An error says what went wrong, then the one thing to do next.
fail_with() {
    fw_status="$1"
    shift
    say ""
    say "${BAD}Error:${RESET} $1"
    shift
    detail "$@"
    exit "$fw_status"
}

fail() {
    fail_with 1 "$@"
}

# A path under $HOME as a person would type it.
# shellcheck disable=SC2088 # the ~ is shown, not expanded
shown() {
    case "$1" in
        "$HOME"/*) printf '~/%s\n' "${1#"$HOME"/}" ;;
        *) printf '%s\n' "$1" ;;
    esac
}

# uname's answers, mapped to the four platform keys the release pipeline, the
# api and the web client all name a build by, and to the words a person reads.
# One table; nothing else in this script branches on the platform.
detect_platform() {
    dp_os="$(uname -s)"
    dp_arch="$(uname -m)"
    case "$dp_os/$dp_arch" in
        Darwin/arm64) PLATFORM_KEY="macos-arm64" PLATFORM_NAME="macOS (arm64)" ;;
        Darwin/x86_64) PLATFORM_KEY="macos-x86_64" PLATFORM_NAME="macOS (x86_64)" ;;
        Linux/x86_64 | Linux/amd64) PLATFORM_KEY="linux-x86_64" PLATFORM_NAME="Linux (x86_64)" ;;
        Linux/aarch64 | Linux/arm64) PLATFORM_KEY="linux-aarch64" PLATFORM_NAME="Linux (arm64)" ;;
        *) fail_with 2 "unsupported platform $dp_os/$dp_arch" \
            "The Build bridge is built for macOS and Linux, on arm64 and x86_64." ;;
    esac
}

# Where the assets live: a named version pins its own release, `latest` follows
# GitHub's redirect, and both serve the same version-free asset names.
release_url() {
    if [ "$VERSION" = "latest" ]; then
        printf 'https://github.com/%s/releases/latest/download\n' "$REPO"
    else
        printf 'https://github.com/%s/releases/download/bridge-v%s\n' "$REPO" "$VERSION"
    fi
}

# Whether this machine has a command at all — asked of the tools the script
# cannot proceed without, and of the two it merely prefers.
has() {
    command -v "$1" > /dev/null 2>&1
}

need() {
    has "$1" || fail "$1 is required to install the Build bridge and is not on PATH." \
        "Install $1, then run the installer again."
}

fetch() {
    curl -fsSL --retry 3 -o "$2" "$1" || fail "could not download $1" \
        "Check your internet connection, then run the installer again."
}

expected_sum() {
    awk -v want="$1" '$2 == want { print $1; hit = 1; exit } END { exit !hit }' "$CHECKSUMS" \
        || fail "$CHECKSUMS from $SOURCE_URL lists no digest for $1, so it cannot be verified." \
            "Nothing was installed. Run the installer again later, or pin an earlier release with BUILD_BRIDGE_VERSION."
}

actual_sum() {
    if has sha256sum; then
        sha256sum "$1" | cut -d ' ' -f 1
    elif has shasum; then
        shasum -a 256 "$1" | cut -d ' ' -f 1
    else
        fail "neither sha256sum nor shasum is available, so $1 cannot be verified." \
            "Install either one, then run the installer again."
    fi
}

# Mandatory: a download that does not match the published digest is never
# installed, whatever the reason.
verify_checksum() {
    vc_want="$(expected_sum "$1")"
    vc_got="$(actual_sum "$1")"
    [ "$vc_want" = "$vc_got" ] || fail "$1 does not match its published checksum, so nothing was installed." \
        "Downloaded from $SOURCE_URL" \
        "expected $vc_want" \
        "got      $vc_got" \
        "Run the installer again; a download that keeps failing this check must not be installed by hand."
    step "Verified the download"
}

# Mandatory when cosign is installed, and silent when it is not: demanding
# cosign would make a signature the price of installing at all, the checksum
# above was taken from the file this would verify, and a missing optional tool
# is not news. cosign's own chatter is kept for the failure that needs it.
verify_signature() {
    has cosign || return 0
    vs_said="$(cosign verify-blob \
        --bundle "$BUNDLE" \
        --certificate-identity-regexp "$COSIGN_IDENTITY_REGEXP" \
        --certificate-oidc-issuer "$COSIGN_ISSUER" \
        "$CHECKSUMS" 2>&1)" || fail "the release signature did not verify, so nothing was installed." \
        "cosign checked $CHECKSUMS from $SOURCE_URL and said:" \
        "$vs_said" \
        "Do not install this download by hand. Run the installer again later."
    step "Verified the release signature"
}

install_binary() {
    tar -xzf "$1" || fail "could not unpack $1 from $SOURCE_URL" "Run the installer again."
    [ -f build-bridge ] || fail "$1 from $SOURCE_URL does not contain a build-bridge binary" \
        "Run the installer again later, or pin an earlier release with BUILD_BRIDGE_VERSION."
    mkdir -p "$INSTALL_DIR" || fail "could not create $INSTALL_DIR" \
        "Set BUILD_BRIDGE_INSTALL_DIR to a directory you can write to, then run the installer again."
    chmod 0755 build-bridge
    mv build-bridge "$INSTALL_DIR/build-bridge" || fail "could not write $INSTALL_DIR/build-bridge" \
        "Set BUILD_BRIDGE_INSTALL_DIR to a directory you can write to, then run the installer again."
    # The updater only changes a binary this installer placed. Record the
    # canonical path and its exact digest; a locally built replacement remains
    # visible in Settings but cannot be overwritten from the app.
    ib_dir="$(cd "$INSTALL_DIR" && pwd -P)" || fail "could not resolve $INSTALL_DIR"
    case "$ib_dir" in *'
'*) fail "install path contains a newline" ;; esac
    ib_path="$ib_dir/build-bridge"
    ib_digest="$(actual_sum "$ib_path")"
    mkdir -p "$HOME/.build" || fail "could not create $(shown "$HOME/.build")" \
        "Make your home directory writable, then run the installer again."
    umask 077
    printf '%s\n%s\n' "$ib_path" "$ib_digest" > "$HOME/.build/installed-bridge" \
        || fail "could not record installed bridge provenance in $(shown "$HOME/.build")" \
            "Make $(shown "$HOME/.build") writable, then run the installer again."
    step "Installed to $(shown "$ib_path")"
    case ":$PATH:" in
        *":$INSTALL_DIR:"*) ;;
        *) detail "$(shown "$INSTALL_DIR") is not on your PATH. To run build-bridge by name, add it with:" \
            "  export PATH=\"$INSTALL_DIR:\$PATH\"" ;;
    esac
}

# The web app the bridge pairs with, found by the bridge's own rule
# (bridge/src/config.rs): BRIDGE_WEB_URL, else the api url, which is the web
# origin too unless BRIDGE_WEB_URL says otherwise. The bridge's pairing prompt
# links to the same address.
web_app_url() {
    wa_base="${BRIDGE_WEB_URL:-${BRIDGE_API_URL:-https://getbuild.ing}}"
    printf '%s/app\n' "${wa_base%/}"
}

# Pairing blocks until the human approves the printed code in Build, and being
# paired is what the service install is gated on, so the two run in this order.
# What each command prints is its own, framed by the step it belongs to.
enable_service() {
    es_bridge="$(shown "$1")"
    if [ "$SKIP_SERVICE" = "1" ]; then
        finish "The Build bridge is installed at $es_bridge." \
            "Pairing and the background service were skipped (BUILD_BRIDGE_SKIP_SERVICE=1)." \
            "When you are ready, run: $es_bridge pair && $es_bridge install-service"
        return 0
    fi
    action_step "Pairing with your Build account" \
        "If a pairing code appears, approve it in Build. The installer waits until you do."
    "$1" pair || fail "pairing did not finish. The bridge is installed at $es_bridge." \
        "Run '$es_bridge pair' when you can approve it in Build, then '$es_bridge install-service'."
    say ""
    step "Starting the background service"
    "$1" install-service || fail "the background service could not be installed." \
        "Run '$es_bridge install-service' again; '$es_bridge serve' runs the bridge in this terminal meanwhile."
    finish "The Build bridge is installed and running." \
        "Open $(web_app_url) to start using it."
}

main() {
    say "${BOLD}Build bridge installer${RESET}"
    say ""
    # The platform is decided first: a host this project does not build for is
    # told exactly that, rather than being sent to find a download tool it was
    # never going to need.
    detect_platform
    need curl
    need tar
    m_tarball="${ASSET_PREFIX}${PLATFORM_KEY}.tar.gz"
    SOURCE_URL="$(release_url)"
    m_workdir="$(mktemp -d)"
    trap 'rm -rf "$m_workdir"' EXIT INT TERM
    cd "$m_workdir" || fail "could not use the temporary directory $m_workdir" \
        "Set TMPDIR to a directory you can write to, then run the installer again."

    step "Downloading the Build bridge for $PLATFORM_NAME"
    fetch "$SOURCE_URL/$m_tarball" "$m_tarball"
    fetch "$SOURCE_URL/$CHECKSUMS" "$CHECKSUMS"
    fetch "$SOURCE_URL/$BUNDLE" "$BUNDLE"

    verify_checksum "$m_tarball"
    verify_signature
    install_binary "$m_tarball"
    enable_service "$INSTALL_DIR/build-bridge"

    [ -t 1 ] || printf 'installed build-bridge %s\n' "$INSTALL_DIR/build-bridge"
}

main "$@"
