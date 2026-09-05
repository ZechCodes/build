#!/bin/sh
# Behaviour tests for install.sh, run entirely offline. A mirror directory
# holds what a release would serve and a `curl` shim in front of PATH copies
# out of it, so the rules the installer exists for — a digest that must match,
# a digest that must be published at all, an uname it cannot map — are
# exercised without a network, a release, or a signing key.
#
# `uname` is shimmed in every case so the mapping under test is the script's
# table and not the machine the suite happens to run on, and `cosign` is
# shimmed to succeed because no bundle can be minted offline; the signature
# rule is the release workflow's own verify step, not this suite's.
#
# Usage: scripts/test-install.sh   (0 every case passed, 1 otherwise)
set -eu

SCRIPT_DIR="$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd)"
INSTALL_SH="$SCRIPT_DIR/install.sh"

# The platform the shimmed uname claims, and the key install.sh must map it to.
PINNED_UNAME_S="Linux"
PINNED_UNAME_M="x86_64"
PINNED_KEY="linux-x86_64"
PINNED_TARBALL="build-bridge-${PINNED_KEY}.tar.gz"

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

digest_of() {
    if command -v sha256sum > /dev/null 2>&1; then
        sha256sum "$1" | cut -d ' ' -f 1
    else
        shasum -a 256 "$1" | cut -d ' ' -f 1
    fi
}

write_shims() {
    ws_dir="$1/shims"
    mkdir -p "$ws_dir"

    cat > "$ws_dir/curl" <<'CURL'
#!/bin/sh
# The release mirror served from a directory: install.sh only ever asks for
# `-o <dest> <url>`, and the last segment of the url names the asset.
set -eu
dest=""
url=""
while [ $# -gt 0 ]; do
    case "$1" in
        -o) dest="$2"; shift 2 ;;
        --retry) shift 2 ;;
        -*) shift ;;
        *) url="$1"; shift ;;
    esac
done
asset="${url##*/}"
[ -f "$BUILD_TEST_MIRROR/$asset" ] || exit 22
cp "$BUILD_TEST_MIRROR/$asset" "$dest"
CURL

    cat > "$ws_dir/uname" <<'UNAME'
#!/bin/sh
set -eu
case "${1:-}" in
    -s) printf '%s\n' "$BUILD_TEST_UNAME_S" ;;
    -m) printf '%s\n' "$BUILD_TEST_UNAME_M" ;;
    *) printf '%s %s\n' "$BUILD_TEST_UNAME_S" "$BUILD_TEST_UNAME_M" ;;
esac
UNAME

    cat > "$ws_dir/cosign" <<'COSIGN'
#!/bin/sh
exit 0
COSIGN

    chmod 0755 "$ws_dir/curl" "$ws_dir/uname" "$ws_dir/cosign"
}

# One sandbox per case: the mirror a release would serve, the shims that stand
# in for the network and the machine, and an empty install directory.
new_sandbox() {
    ns_root="$WORK/$1"
    mkdir -p "$ns_root/mirror" "$ns_root/dest" "$ns_root/payload"

    printf '#!/bin/sh\nprintf "build-bridge 0.0.0-test\\n"\n' > "$ns_root/payload/build-bridge"
    chmod 0755 "$ns_root/payload/build-bridge"
    tar -czf "$ns_root/mirror/$PINNED_TARBALL" -C "$ns_root/payload" build-bridge
    : > "$ns_root/mirror/SHA256SUMS.sigstore.json"
    printf '%s  %s\n' "$(digest_of "$ns_root/mirror/$PINNED_TARBALL")" "$PINNED_TARBALL" \
        > "$ns_root/mirror/SHA256SUMS"

    write_shims "$ns_root"
    printf '%s\n' "$ns_root"
}

# Runs install.sh against a sandbox and prints its exit status; stdout and
# stderr land beside the sandbox for the assertions to read.
run_install() {
    ri_root="$1"
    ri_status=0
    (
        PATH="$ri_root/shims:$PATH"
        BUILD_TEST_MIRROR="$ri_root/mirror"
        BUILD_TEST_UNAME_S="${2:-$PINNED_UNAME_S}"
        BUILD_TEST_UNAME_M="${3:-$PINNED_UNAME_M}"
        BUILD_BRIDGE_INSTALL_DIR="$ri_root/dest"
        BUILD_BRIDGE_SKIP_SERVICE=1
        export PATH BUILD_TEST_MIRROR BUILD_TEST_UNAME_S BUILD_TEST_UNAME_M
        export BUILD_BRIDGE_INSTALL_DIR BUILD_BRIDGE_SKIP_SERVICE
        sh "$INSTALL_SH" > "$ri_root/stdout" 2> "$ri_root/stderr"
    ) || ri_status=$?
    printf '%s\n' "$ri_status"
}

installs_a_verified_tarball() {
    name="installs_a_verified_tarball"
    root="$(new_sandbox "$name")"
    status="$(run_install "$root")"
    if [ "$status" != "0" ]; then
        fail "$name" "exit $status, wanted 0: $(cat "$root/stderr")"
        return 0
    fi
    if [ ! -x "$root/dest/build-bridge" ]; then
        fail "$name" "no executable build-bridge in the install directory"
        return 0
    fi
    if ! grep -q "^installed build-bridge $root/dest/build-bridge\$" "$root/stdout"; then
        fail "$name" "stdout was '$(cat "$root/stdout")'"
        return 0
    fi
    pass "$name"
}

refuses_a_tampered_tarball() {
    name="refuses_a_tampered_tarball"
    root="$(new_sandbox "$name")"
    printf 'tampered' >> "$root/mirror/$PINNED_TARBALL"
    status="$(run_install "$root")"
    if [ "$status" != "1" ]; then
        fail "$name" "exit $status, wanted 1"
        return 0
    fi
    if [ -e "$root/dest/build-bridge" ]; then
        fail "$name" "installed the binary anyway"
        return 0
    fi
    pass "$name"
}

refuses_an_asset_with_no_published_digest() {
    name="refuses_an_asset_with_no_published_digest"
    root="$(new_sandbox "$name")"
    printf '%s  %s\n' "$(digest_of "$root/mirror/$PINNED_TARBALL")" install.sh \
        > "$root/mirror/SHA256SUMS"
    status="$(run_install "$root")"
    if [ "$status" != "1" ]; then
        fail "$name" "exit $status, wanted 1"
        return 0
    fi
    if [ -e "$root/dest/build-bridge" ]; then
        fail "$name" "installed the binary anyway"
        return 0
    fi
    pass "$name"
}

unmapped_uname_exits_2() {
    name="unmapped_uname_exits_2"
    root="$(new_sandbox "$name")"
    status="$(run_install "$root" Plan9 mips)"
    if [ "$status" != "2" ]; then
        fail "$name" "exit $status, wanted 2"
        return 0
    fi
    if ! grep -q 'unsupported platform Plan9/mips' "$root/stderr"; then
        fail "$name" "stderr was '$(cat "$root/stderr")'"
        return 0
    fi
    pass "$name"
}

installs_a_verified_tarball
refuses_a_tampered_tarball
refuses_an_asset_with_no_published_digest
unmapped_uname_exits_2

if [ "$FAILURES" -ne 0 ]; then
    printf '%s case(s) failed\n' "$FAILURES" >&2
    exit 1
fi
printf 'install.sh: every case passed\n'
