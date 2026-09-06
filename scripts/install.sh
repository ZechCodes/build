#!/bin/sh
# Install the Build bridge — the device daemon that runs coding agents on this
# machine. Published verbatim as a release asset and served from
# https://getbuild.ing/install.sh, so it is run as:
#
#   curl -fsSL https://getbuild.ing/install.sh | sh
#
# stdin is the curl pipe, so nothing here may prompt. Every message goes to
# stderr; the single line on stdout is the path the binary landed at.
#
# Environment:
#   BUILD_BRIDGE_VERSION       `latest` (default) or `X.Y.Z`
#   BUILD_BRIDGE_INSTALL_DIR   where the binary lands (default ~/.local/bin)
#   BUILD_RELEASES_REPO        GitHub owner/name holding the release assets
#   BUILD_BRIDGE_SKIP_SERVICE  `1` stops after installing the binary
#
# Exit codes: 0 installed, 1 something failed, 2 unsupported platform.
set -eu

DEFAULT_REPO="ZechCodes/build-releases"
COSIGN_IDENTITY_REGEXP='^https://github\.com/ZechCodes/build-web/\.github/workflows/release\.yml@refs/tags/bridge-v'
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

fail() {
    say "$1"
    exit "${2:-1}"
}

# uname's answers, mapped to the four platform keys the release pipeline, the
# api and the web client all name a build by. One table; nothing else in this
# script branches on the platform.
platform_key() {
    pk_os="$(uname -s)"
    pk_arch="$(uname -m)"
    case "$pk_os/$pk_arch" in
        Darwin/arm64) printf 'macos-arm64\n' ;;
        Darwin/x86_64) printf 'macos-x86_64\n' ;;
        Linux/x86_64 | Linux/amd64) printf 'linux-x86_64\n' ;;
        Linux/aarch64 | Linux/arm64) printf 'linux-aarch64\n' ;;
        *) fail "unsupported platform $pk_os/$pk_arch" 2 ;;
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
    has "$1" || fail "$1 is required to install build-bridge and is not on PATH"
}

fetch() {
    curl -fsSL --retry 3 -o "$2" "$1" || fail "could not download $1"
}

expected_sum() {
    awk -v want="$1" '$2 == want { print $1; hit = 1; exit } END { exit !hit }' "$CHECKSUMS" \
        || fail "$CHECKSUMS lists no digest for $1"
}

actual_sum() {
    if has sha256sum; then
        sha256sum "$1" | cut -d ' ' -f 1
    elif has shasum; then
        shasum -a 256 "$1" | cut -d ' ' -f 1
    else
        fail "neither sha256sum nor shasum is available, so $1 cannot be verified"
    fi
}

# Mandatory: a download that does not match the published digest is never
# installed, whatever the reason.
verify_checksum() {
    vc_want="$(expected_sum "$1")"
    vc_got="$(actual_sum "$1")"
    [ "$vc_want" = "$vc_got" ] || fail "checksum mismatch for $1: expected $vc_want, got $vc_got"
    say "checksum verified: $1"
}

# Mandatory when cosign is installed, skipped with a note when it is not:
# demanding cosign would make a signature the price of installing at all, and
# the checksum above was taken from the file this would verify.
verify_signature() {
    if ! has cosign; then
        say "cosign not found; skipping signature verification (checksum verified)"
        return 0
    fi
    cosign verify-blob \
        --bundle "$BUNDLE" \
        --certificate-identity-regexp "$COSIGN_IDENTITY_REGEXP" \
        --certificate-oidc-issuer "$COSIGN_ISSUER" \
        "$CHECKSUMS" >&2 || fail "cosign could not verify $CHECKSUMS; refusing to install"
    say "signature verified: $CHECKSUMS"
}

install_binary() {
    tar -xzf "$1" || fail "could not unpack $1"
    [ -f build-bridge ] || fail "$1 does not contain a build-bridge binary"
    mkdir -p "$INSTALL_DIR" || fail "could not create $INSTALL_DIR"
    chmod 0755 build-bridge
    mv build-bridge "$INSTALL_DIR/build-bridge" || fail "could not write $INSTALL_DIR/build-bridge"
    case ":$PATH:" in
        *":$INSTALL_DIR:"*) ;;
        *) say "$INSTALL_DIR is not on your PATH — add it with: export PATH=\"$INSTALL_DIR:\$PATH\"" ;;
    esac
}

# Pairing blocks until the human approves the printed code in Build, and being
# paired is what the service install is gated on, so the two run in this order.
enable_service() {
    if [ "$SKIP_SERVICE" = "1" ]; then
        say "BUILD_BRIDGE_SKIP_SERVICE=1: skipping pairing and the background service"
        return 0
    fi
    "$1" pair || fail "pairing failed; $1 is installed — rerun '$1 pair' when you can approve it"
    "$1" install-service || fail "could not install the background service; '$1 serve' still runs it by hand"
}

main() {
    # The platform is decided first: a host this project does not build for is
    # told exactly that, rather than being sent to find a download tool it was
    # never going to need.
    m_key="$(platform_key)"
    need curl
    need tar
    m_tarball="${ASSET_PREFIX}${m_key}.tar.gz"
    m_url="$(release_url)"
    m_workdir="$(mktemp -d)"
    trap 'rm -rf "$m_workdir"' EXIT INT TERM
    cd "$m_workdir" || fail "could not use the temporary directory $m_workdir"

    say "downloading $m_tarball from $m_url"
    fetch "$m_url/$m_tarball" "$m_tarball"
    fetch "$m_url/$CHECKSUMS" "$CHECKSUMS"
    fetch "$m_url/$BUNDLE" "$BUNDLE"

    verify_checksum "$m_tarball"
    verify_signature
    install_binary "$m_tarball"
    enable_service "$INSTALL_DIR/build-bridge"

    printf 'installed build-bridge %s\n' "$INSTALL_DIR/build-bridge"
}

main "$@"
