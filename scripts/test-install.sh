#!/bin/sh
# Behaviour tests for install.sh, run entirely offline. A mirror directory
# holds what the api's download routes would serve and a `curl` shim in front
# of PATH copies out of it, so the rules the installer exists for — a token it
# cannot proceed without, a token Build refuses, a digest that must match, a
# digest that must be published at all, a signature that must verify, an uname
# it cannot map — are exercised without a network, a release, or a signing key.
#
# install.sh is never run as it sits on disk: the api fills its two slots on
# every serve, so each sandbox renders its own copy the same way (one sed per
# placeholder) and runs that. The token in the rendered copy and the token the
# curl shim accepts are what a case varies to ask for a refusal.
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

# What the api would substitute into the served script: its own origin and a
# download token minted for the member who copied the install line.
PINNED_BASE_URL="https://build.test"
PINNED_TOKEN="dl_installer-suite-token-aaaaaaaaaa"

# The two verdicts a user reads when the install line cannot be used. Stated
# here in full because they are the specification: install.sh may reword
# nothing without this suite saying so.
NO_TOKEN_MESSAGE='no download token in this install line — copy the install line from Build (Settings → Downloads) and run it as one line'
REFUSED_MESSAGE='Build refused the download: this install line has expired or was already used — copy a fresh one from Build'

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

# The three assertions every case is written from. Each reports the failure
# itself and returns 1, so a case reads as `assert_… && pass "$name"`.
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

# A verdict the user cannot read is a verdict they cannot act on, so the
# message is part of the behaviour and not a detail of it.
assert_says() {
    as_name="$1"
    as_root="$2"
    grep -qF -- "$3" "$as_root/stderr" && return 0
    fail "$as_name" "stderr was '$(cat "$as_root/stderr")'"
    return 1
}

# The mirror image of assert_says: a value the user must never read, checked
# against the whole of stderr. Named without repeating the value, so a failure
# report cannot leak what the case exists to keep out of the output.
assert_never_says() {
    ans_name="$1"
    ans_root="$2"
    grep -qF -- "$3" "$ans_root/stderr" || return 0
    fail "$ans_name" "stderr repeated a value it must never print: $(cat "$ans_root/stderr")"
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
# The api's download routes, served from a directory. install.sh only ever
# asks for `-o <dest> -w <format> <url>`; the last segment of the url names
# the download and carries the install line's token, which this shim judges
# the way the api does — a token it does not know is a 401 and writes no
# bytes. Every request is logged, url segment and all, before it is judged.
set -eu
dest=""
url=""
while [ $# -gt 0 ]; do
    case "$1" in
        -o) dest="$2"; shift 2 ;;
        -w) shift 2 ;;
        --retry) shift 2 ;;
        -*) shift ;;
        *) url="$1"; shift ;;
    esac
done
request="${url##*/}"
printf '%s\n' "$request" >> "$BUILD_TEST_REQUESTS"

segment="${request%%\?*}"
token=""
case "$request" in
    *\?t=*) token="${request#*\?t=}" ;;
esac

if [ "$token" != "$BUILD_TEST_TOKEN" ]; then
    printf '401'
    exit 0
fi
if [ ! -f "$BUILD_TEST_MIRROR/$segment" ]; then
    printf '404'
    exit 0
fi
cp "$BUILD_TEST_MIRROR/$segment" "$dest"
printf '200'
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

# The script as the api serves it: both slots filled, one sed apiece — the
# same substitution `render_install_script` applies, so what runs here is what
# a user pipes into sh and never the template on disk.
serve_script() {
    sed -e "s|{{api_base_url}}|$PINNED_BASE_URL|" -e "s|{{download_token}}|$2|" \
        "$INSTALL_SH" > "$1/install.sh"
}

# One sandbox per case: the assets the api would deliver, keyed by url segment
# (a platform key fetches its tarball; the two support files are asked for by
# name), the shims that stand in for the network and the machine, an empty
# install directory, and the script rendered with a token the shim accepts.
new_sandbox() {
    ns_root="$WORK/$1"
    mkdir -p "$ns_root/mirror" "$ns_root/dest" "$ns_root/payload"

    printf '#!/bin/sh\nprintf "build-bridge 0.0.0-test\\n"\n' > "$ns_root/payload/build-bridge"
    chmod 0755 "$ns_root/payload/build-bridge"
    tar -czf "$ns_root/mirror/$PINNED_KEY" -C "$ns_root/payload" build-bridge
    : > "$ns_root/mirror/SHA256SUMS.sigstore.json"
    : > "$ns_root/requests"
    # SHA256SUMS names the tarball by its asset name, whatever url segment it
    # was fetched from, so the digest lines a release publishes are the digest
    # lines the installer checks.
    printf '%s  %s\n' "$(digest_of "$ns_root/mirror/$PINNED_KEY")" "$PINNED_TARBALL" \
        > "$ns_root/mirror/SHA256SUMS"

    write_shims "$ns_root"
    serve_script "$ns_root" "$PINNED_TOKEN"
    printf '%s\n' "$ns_root"
}

# Runs the served script against a sandbox and prints its exit status; stdout
# and stderr land beside the sandbox for the assertions to read. A case shapes
# the run by setting CASE_COSIGN_STATUS (the verdict the cosign shim returns)
# or CASE_ONLY_SHIMS_ON_PATH=1 (nothing but the shims is installed on this
# host) in front of the call, where the command substitution keeps it local.
run_install() {
    ri_root="$1"
    ri_path="$ri_root/shims"
    [ "${CASE_ONLY_SHIMS_ON_PATH:-0}" = "1" ] || ri_path="$ri_path:$PATH"
    ri_status=0
    (
        PATH="$ri_path"
        BUILD_TEST_MIRROR="$ri_root/mirror"
        BUILD_TEST_REQUESTS="$ri_root/requests"
        BUILD_TEST_TOKEN="$PINNED_TOKEN"
        BUILD_TEST_UNAME_S="${2:-$PINNED_UNAME_S}"
        BUILD_TEST_UNAME_M="${3:-$PINNED_UNAME_M}"
        BUILD_TEST_COSIGN_STATUS="${CASE_COSIGN_STATUS:-0}"
        BUILD_BRIDGE_INSTALL_DIR="$ri_root/dest"
        BUILD_BRIDGE_SKIP_SERVICE=1
        export PATH BUILD_TEST_MIRROR BUILD_TEST_REQUESTS BUILD_TEST_TOKEN
        export BUILD_TEST_UNAME_S BUILD_TEST_UNAME_M
        export BUILD_TEST_COSIGN_STATUS BUILD_BRIDGE_INSTALL_DIR BUILD_BRIDGE_SKIP_SERVICE
        /bin/sh "$ri_root/install.sh" > "$ri_root/stdout" 2> "$ri_root/stderr"
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
    printf 'tampered' >> "$root/mirror/$PINNED_KEY"
    status="$(run_install "$root")"
    assert_refused "$name" "$root" "$status" 1 && pass "$name"
}

refuses_an_asset_with_no_published_digest() {
    name="refuses_an_asset_with_no_published_digest"
    root="$(new_sandbox "$name")"
    printf '%s  %s\n' "$(digest_of "$root/mirror/$PINNED_KEY")" build-bridge-macos-arm64.tar.gz \
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
    assert_says "$name" "$root" 'unsupported platform Plan9/mips' && pass "$name"
}

# Mapping the platform is the first thing the script decides, so a host it does
# not build for is told exactly that — not that it is missing a download tool
# it would never have needed.
an_unsupported_platform_is_named_before_any_tool_is_demanded() {
    name="an_unsupported_platform_is_named_before_any_tool_is_demanded"
    root="$(new_sandbox "$name")"
    rm "$root/shims/curl"
    status="$(CASE_ONLY_SHIMS_ON_PATH=1 run_install "$root" Plan9 mips)"
    assert_refused "$name" "$root" "$status" 2 && pass "$name"
}

# The script downloads nothing anonymously, so a copy served without a token —
# the script fetched by hand rather than through the line Build shows — stops
# before it asks the api for anything at all.
refuses_to_run_without_a_token() {
    name="refuses_to_run_without_a_token"
    root="$(new_sandbox "$name")"
    serve_script "$root" ""
    status="$(run_install "$root")"
    assert_refused "$name" "$root" "$status" 2 || return 1
    assert_says "$name" "$root" "$NO_TOKEN_MESSAGE" || return 1
    if [ -s "$root/requests" ]; then
        fail "$name" "asked the api for '$(tr '\n' ' ' < "$root/requests")'"
        return 1
    fi
    pass "$name"
}

# An expired or already-spent install line is a 401 from the api, and a 401 is
# the end of the install — not a missing file the script works around.
a_refused_token_installs_nothing() {
    name="a_refused_token_installs_nothing"
    root="$(new_sandbox "$name")"
    serve_script "$root" "dl_a-token-the-api-does-not-know-aa"
    status="$(run_install "$root")"
    assert_refused "$name" "$root" "$status" 1 || return 1
    assert_says "$name" "$root" "$REFUSED_MESSAGE" && pass "$name"
}

every_download_carries_the_token() {
    name="every_download_carries_the_token"
    root="$(new_sandbox "$name")"
    status="$(run_install "$root")"
    assert_exit "$name" "$root" "$status" 0 || return 1
    if [ "$(wc -l < "$root/requests")" -ne 3 ]; then
        fail "$name" "asked the api for '$(tr '\n' ' ' < "$root/requests")'"
        return 1
    fi
    if [ "$(grep -cF -- "?t=$PINNED_TOKEN" "$root/requests")" -ne 3 ]; then
        fail "$name" "a request went without the token: '$(tr '\n' ' ' < "$root/requests")'"
        return 1
    fi
    pass "$name"
}

# The tarball is the one request that spends the token, so it goes last: a
# token spent before its checksums and signature arrived has bought nothing.
the_binary_is_fetched_last() {
    name="the_binary_is_fetched_last"
    root="$(new_sandbox "$name")"
    status="$(run_install "$root")"
    assert_exit "$name" "$root" "$status" 0 || return 1
    order="$(sed 's/?.*//' "$root/requests" | tr '\n' ' ')"
    if [ "$order" != "SHA256SUMS SHA256SUMS.sigstore.json $PINNED_KEY " ]; then
        fail "$name" "fetched in the order '$order'"
        return 1
    fi
    pass "$name"
}

# A download that fails for any reason but a refusal is a message the user
# pastes into a support thread, so it names the download and the api it was
# asked of — never the url, whose query string carries an install line that a
# 404 or a 502 has left live and usable by whoever reads it.
a_failed_download_names_the_asset_and_not_the_token() {
    name="a_failed_download_names_the_asset_and_not_the_token"
    root="$(new_sandbox "$name")"
    rm "$root/mirror/SHA256SUMS.sigstore.json"
    status="$(run_install "$root")"
    assert_refused "$name" "$root" "$status" 1 || return 1
    assert_says "$name" "$root" \
        "could not download SHA256SUMS.sigstore.json from $PINNED_BASE_URL (HTTP 404)" || return 1
    assert_never_says "$name" "$root" "$PINNED_TOKEN" && pass "$name"
}

# Having a token is decided as early as knowing the platform, and for the same
# reason: a user whose install line is spent is told that, not sent to install
# a download tool their next copied line will not need either.
the_no_token_verdict_comes_before_any_tool_is_demanded() {
    name="the_no_token_verdict_comes_before_any_tool_is_demanded"
    root="$(new_sandbox "$name")"
    serve_script "$root" ""
    rm "$root/shims/curl"
    status="$(CASE_ONLY_SHIMS_ON_PATH=1 run_install "$root")"
    assert_refused "$name" "$root" "$status" 2 || return 1
    assert_says "$name" "$root" "$NO_TOKEN_MESSAGE" && pass "$name"
}

# Every case reports its own failure through `fail`, so a non-zero return only
# says the case is over; the suite's verdict is FAILURES, not $?.
for case_name in \
    installs_a_verified_tarball \
    refuses_a_tampered_tarball \
    refuses_an_asset_with_no_published_digest \
    refuses_when_cosign_rejects_the_bundle \
    unmapped_uname_exits_2 \
    an_unsupported_platform_is_named_before_any_tool_is_demanded \
    refuses_to_run_without_a_token \
    a_refused_token_installs_nothing \
    every_download_carries_the_token \
    the_binary_is_fetched_last \
    a_failed_download_names_the_asset_and_not_the_token \
    the_no_token_verdict_comes_before_any_tool_is_demanded; do
    "$case_name" || true
done

if [ "$FAILURES" -ne 0 ]; then
    printf '%s case(s) failed\n' "$FAILURES" >&2
    exit 1
fi
printf 'install.sh: every case passed\n'
