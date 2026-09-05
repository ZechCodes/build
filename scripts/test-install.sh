#!/bin/sh
# Behaviour tests for install.sh, run entirely offline. A mirror directory
# holds what a release would serve and a `curl` shim in front of PATH copies
# out of it, so the rules the installer exists for — a digest that must match,
# a digest that must be published at all, a signature that must verify, an
# uname it cannot map — are exercised without a network, a release, or a
# signing key.
#
# `uname` is shimmed in every case so the mapping under test is the script's
# table and not the machine the suite happens to run on, and `cosign` is a
# shim whose exit status each case chooses: no real bundle can be minted
# offline, and what is under test is what the installer does with a verdict,
# not how the verdict is reached.
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

# The two assertions every case is written from. Both report the failure
# themselves and return 1, so a case reads as `assert_… && pass "$name"`.
assert_exit() {
    ae_name="$1"
    ae_root="$2"
    [ "$3" = "$4" ] && return 0
    fail "$ae_name" "exit $3, wanted $4: $(cat "$ae_root/stderr")"
    return 1
}

# A refusal is an exit status *and* an empty install directory: a script that
# reports a failure after putting the binary on disk has refused nothing.
assert_refused() {
    ar_name="$1"
    ar_root="$2"
    assert_exit "$ar_name" "$ar_root" "$3" "$4" || return 1
    [ ! -e "$ar_root/dest/build-bridge" ] && return 0
    fail "$ar_name" "installed the binary anyway"
    return 1
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
# Accepts or rejects the bundle as the case under test asked it to.
set -eu
exit "$BUILD_TEST_COSIGN_STATUS"
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
# stderr land beside the sandbox for the assertions to read. A case shapes the
# run by setting CASE_COSIGN_STATUS — the verdict the cosign shim returns — in
# front of the call, where the command substitution keeps it local.
run_install() {
    ri_root="$1"
    ri_status=0
    (
        PATH="$ri_root/shims:$PATH"
        BUILD_TEST_MIRROR="$ri_root/mirror"
        BUILD_TEST_UNAME_S="${2:-$PINNED_UNAME_S}"
        BUILD_TEST_UNAME_M="${3:-$PINNED_UNAME_M}"
        BUILD_TEST_COSIGN_STATUS="${CASE_COSIGN_STATUS:-0}"
        BUILD_BRIDGE_INSTALL_DIR="$ri_root/dest"
        BUILD_BRIDGE_SKIP_SERVICE=1
        export PATH BUILD_TEST_MIRROR BUILD_TEST_UNAME_S BUILD_TEST_UNAME_M
        export BUILD_TEST_COSIGN_STATUS BUILD_BRIDGE_INSTALL_DIR BUILD_BRIDGE_SKIP_SERVICE
        sh "$INSTALL_SH" > "$ri_root/stdout" 2> "$ri_root/stderr"
    ) || ri_status=$?
    printf '%s\n' "$ri_status"
}

installs_a_verified_tarball() {
    name="installs_a_verified_tarball"
    root="$(new_sandbox "$name")"
    status="$(run_install "$root")"
    assert_exit "$name" "$root" "$status" 0 || return 1
    if [ ! -x "$root/dest/build-bridge" ]; then
        fail "$name" "no executable build-bridge in the install directory"
        return 1
    fi
    if ! grep -q "^installed build-bridge $root/dest/build-bridge\$" "$root/stdout"; then
        fail "$name" "stdout was '$(cat "$root/stdout")'"
        return 1
    fi
    pass "$name"
}

refuses_a_tampered_tarball() {
    name="refuses_a_tampered_tarball"
    root="$(new_sandbox "$name")"
    printf 'tampered' >> "$root/mirror/$PINNED_TARBALL"
    status="$(run_install "$root")"
    assert_refused "$name" "$root" "$status" 1 && pass "$name"
}

refuses_an_asset_with_no_published_digest() {
    name="refuses_an_asset_with_no_published_digest"
    root="$(new_sandbox "$name")"
    printf '%s  %s\n' "$(digest_of "$root/mirror/$PINNED_TARBALL")" install.sh \
        > "$root/mirror/SHA256SUMS"
    status="$(run_install "$root")"
    assert_refused "$name" "$root" "$status" 1 && pass "$name"
}

# Signature verification is optional only in the sense that a machine without
# cosign may go without it. A cosign that is present and says no is final.
refuses_when_cosign_rejects_the_bundle() {
    name="refuses_when_cosign_rejects_the_bundle"
    root="$(new_sandbox "$name")"
    status="$(CASE_COSIGN_STATUS=1 run_install "$root")"
    assert_refused "$name" "$root" "$status" 1 && pass "$name"
}

unmapped_uname_exits_2() {
    name="unmapped_uname_exits_2"
    root="$(new_sandbox "$name")"
    status="$(run_install "$root" Plan9 mips)"
    assert_refused "$name" "$root" "$status" 2 || return 1
    if ! grep -q 'unsupported platform Plan9/mips' "$root/stderr"; then
        fail "$name" "stderr was '$(cat "$root/stderr")'"
        return 1
    fi
    pass "$name"
}

# Every case reports its own failure through `fail`, so a non-zero return only
# says the case is over; the suite's verdict is FAILURES, not $?.
for case_name in \
    installs_a_verified_tarball \
    refuses_a_tampered_tarball \
    refuses_an_asset_with_no_published_digest \
    refuses_when_cosign_rejects_the_bundle \
    unmapped_uname_exits_2; do
    "$case_name" || true
done

if [ "$FAILURES" -ne 0 ]; then
    printf '%s case(s) failed\n' "$FAILURES" >&2
    exit 1
fi
printf 'install.sh: every case passed\n'
