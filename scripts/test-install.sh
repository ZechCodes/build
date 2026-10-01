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
# Accepts or rejects the bundle as the case under test asked it to, talking
# on stderr the way the real one does.
set -eu
if [ "$BUILD_TEST_COSIGN_STATUS" = "0" ]; then
    printf 'Verified OK\n' >&2
else
    printf 'Error: none of the expected identities matched\n' >&2
fi
exit "$BUILD_TEST_COSIGN_STATUS"
COSIGN

    chmod 0755 "$ws_dir/curl" "$ws_dir/uname" "$ws_dir/cosign"
}

# One sandbox per case: the mirror a release would serve, the shims that stand
# in for the network and the machine, and an empty install directory.
new_sandbox() {
    ns_root="$WORK/$1"
    mkdir -p "$ns_root/mirror" "$ns_root/dest" "$ns_root/payload" "$ns_root/home"

    cat > "$ns_root/payload/build-bridge" <<'BRIDGE'
#!/bin/sh
# Stands in for the released binary: records each subcommand the installer
# runs and answers it the way the real one does, with the status the case
# chose for pairing.
case "${1:-}" in
    pair)
        printf 'pair\n' >> "$HOME/bridge-calls"
        printf '\n  Pair this device to your account:\n    pairing code: TEST-CODE\n\n' >&2
        [ "${BUILD_TEST_PAIR_STATUS:-0}" = "0" ] || { printf 'not paired: refused\n' >&2; exit 1; }
        printf 'paired to account test-owner\n'
        ;;
    install-service)
        printf 'install-service\n' >> "$HOME/bridge-calls"
        printf 'installed test-manager for account owner test-owner\n'
        ;;
    *) printf 'build-bridge 0.0.0-test\n' ;;
esac
BRIDGE
    chmod 0755 "$ns_root/payload/build-bridge"
    tar -czf "$ns_root/mirror/$PINNED_TARBALL" -C "$ns_root/payload" build-bridge
    : > "$ns_root/mirror/SHA256SUMS.sigstore.json"
    printf '%s  %s\n' "$(digest_of "$ns_root/mirror/$PINNED_TARBALL")" "$PINNED_TARBALL" \
        > "$ns_root/mirror/SHA256SUMS"

    write_shims "$ns_root"
    printf '%s\n' "$ns_root"
}

# A host without cosign: the shim goes, and PATH is cut down to links to the
# tools install.sh runs, so a cosign the test machine happens to have cannot
# stand in for the one this case removed.
without_cosign() {
    rm "$1/shims/cosign"
    mkdir -p "$1/host"
    for wc_tool in tar gzip mktemp rm awk sha256sum shasum cut mkdir chmod mv cat cp; do
        wc_found="$(command -v "$wc_tool")" || continue
        ln -s "$wc_found" "$1/host/$wc_tool"
    done
}

# Runs install.sh against a sandbox and prints its exit status; stdout and
# stderr land beside the sandbox for the assertions to read. A case shapes the
# run by setting, in front of the call where the command substitution keeps it
# local: CASE_COSIGN_STATUS (the verdict the cosign shim returns),
# CASE_ONLY_SHIMS_ON_PATH=1 (nothing but the shims is installed on this host),
# CASE_SKIP_SERVICE=0 (go on to pair and install the service),
# CASE_PAIR_STATUS (how the fake bridge's pairing ends) or CASE_TERMINAL=1
# (run under a pseudo-terminal; stdout and stderr then both land in stdout).
run_install() {
    ri_root="$1"
    ri_path="$ri_root/shims"
    if [ -d "$ri_root/host" ]; then
        ri_path="$ri_path:$ri_root/host"
    elif [ "${CASE_ONLY_SHIMS_ON_PATH:-0}" != "1" ]; then
        ri_path="$ri_path:$PATH"
    fi
    ri_status=0
    (
        PATH="$ri_path"
        BUILD_TEST_MIRROR="$ri_root/mirror"
        BUILD_TEST_UNAME_S="${2:-$PINNED_UNAME_S}"
        BUILD_TEST_UNAME_M="${3:-$PINNED_UNAME_M}"
        BUILD_TEST_COSIGN_STATUS="${CASE_COSIGN_STATUS:-0}"
        BUILD_BRIDGE_INSTALL_DIR="$ri_root/dest"
        BUILD_BRIDGE_SKIP_SERVICE="${CASE_SKIP_SERVICE:-1}"
        BUILD_TEST_PAIR_STATUS="${CASE_PAIR_STATUS:-0}"
        HOME="$ri_root/home"
        export PATH BUILD_TEST_MIRROR BUILD_TEST_UNAME_S BUILD_TEST_UNAME_M BUILD_TEST_PAIR_STATUS
        export BUILD_TEST_COSIGN_STATUS BUILD_BRIDGE_INSTALL_DIR BUILD_BRIDGE_SKIP_SERVICE HOME
        if [ "${CASE_TERMINAL:-0}" = "1" ]; then
            : > "$ri_root/stderr"
            script -qec "/bin/sh '$INSTALL_SH'" /dev/null > "$ri_root/stdout"
        else
            /bin/sh "$INSTALL_SH" > "$ri_root/stdout" 2> "$ri_root/stderr"
        fi
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
    if [ "$(sed -n '1p' "$root/home/.build/installed-bridge")" != "$root/dest/build-bridge" ] ||
        [ "$(sed -n '2p' "$root/home/.build/installed-bridge")" != "$(digest_of "$root/dest/build-bridge")" ]; then
        fail "$name" "install marker has the wrong path or binary digest"
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

# What a person reads, line by line, in the order it was printed.
assert_says() {
    as_name="$1"
    as_file="$2"
    shift 2
    for as_line in "$@"; do
        grep -qF -- "$as_line" "$as_file" && continue
        fail "$as_name" "no '$as_line' in: $(cat "$as_file")"
        return 1
    done
}

assert_never_says() {
    ans_name="$1"
    ans_file="$2"
    shift 2
    for ans_text in "$@"; do
        grep -qF -- "$ans_text" "$ans_file" || continue
        fail "$ans_name" "said '$ans_text': $(cat "$ans_file")"
        return 1
    done
}

# The happy path speaks in steps, not in URLs and tarball names.
success_reads_as_plain_steps() {
    name="success_reads_as_plain_steps"
    root="$(new_sandbox "$name")"
    status="$(run_install "$root")"
    assert_exit "$name" "$root" "$status" 0 || return 1
    assert_says "$name" "$root/stderr" \
        "Downloading Build bridge for Linux (x86_64)" \
        "Verified the download" \
        "Installed to $root/dest/build-bridge" || return 1
    assert_never_says "$name" "$root/stderr" "https://" ".tar.gz" "SHA256SUMS" "rror" && pass "$name"
}

# No cosign is the normal case on most machines, so it is not news: the
# checksum line still prints, and nothing mentions what was not run.
a_missing_cosign_goes_unmentioned() {
    name="a_missing_cosign_goes_unmentioned"
    root="$(new_sandbox "$name")"
    without_cosign "$root"
    status="$(run_install "$root")"
    assert_exit "$name" "$root" "$status" 0 || return 1
    [ -x "$root/dest/build-bridge" ] || { fail "$name" "not installed"; return 1; }
    assert_says "$name" "$root/stderr" "Verified the download" || return 1
    assert_never_says "$name" "$root/stderr" "cosign" "signature" "skipping" && pass "$name"
}

# A present cosign is reported in one line of the installer's own; cosign's
# chatter stays out of the way.
a_present_cosign_is_one_line() {
    name="a_present_cosign_is_one_line"
    root="$(new_sandbox "$name")"
    status="$(run_install "$root")"
    assert_exit "$name" "$root" "$status" 0 || return 1
    if [ "$(grep -ci 'signature' "$root/stderr")" != "1" ]; then
        fail "$name" "wanted one signature line in: $(cat "$root/stderr")"
        return 1
    fi
    assert_says "$name" "$root/stderr" "Verified the release signature" || return 1
    assert_never_says "$name" "$root/stderr" "Verified OK" && pass "$name"
}

# A rejected signature is loud: an error, what cosign said, and what to do.
a_rejected_signature_is_a_loud_error() {
    name="a_rejected_signature_is_a_loud_error"
    root="$(new_sandbox "$name")"
    status="$(CASE_COSIGN_STATUS=1 run_install "$root")"
    assert_refused "$name" "$root" "$status" 1 || return 1
    assert_says "$name" "$root/stderr" \
        "Error: the release signature did not verify" \
        "none of the expected identities matched" && pass "$name"
}

# A failure names the file and the source, which a success never shows.
a_failure_names_what_failed() {
    name="a_failure_names_what_failed"
    root="$(new_sandbox "$name")"
    printf 'tampered' >> "$root/mirror/$PINNED_TARBALL"
    status="$(run_install "$root")"
    assert_refused "$name" "$root" "$status" 1 || return 1
    assert_says "$name" "$root/stderr" \
        "Error: $PINNED_TARBALL does not match its published checksum" \
        "https://github.com/ZechCodes/build-releases/releases/latest/download" && pass "$name"
}

# Output that is not going to a terminal carries no escape codes, so a log
# file or a CI job reads plainly.
piped_output_is_plain() {
    name="piped_output_is_plain"
    root="$(new_sandbox "$name")"
    status="$(CASE_COSIGN_STATUS=1 run_install "$root")"
    assert_exit "$name" "$root" "$status" 1 || return 1
    if grep -q "$(printf '\033')" "$root/stdout" "$root/stderr"; then
        fail "$name" "escape codes in piped output"
        return 1
    fi
    pass "$name"
}

# The pseudo-terminal cases need util-linux's `script`; elsewhere they are
# reported as skipped, not passed.
has_pty_script() {
    script --version 2> /dev/null | grep -q util-linux
}

a_terminal_gets_styled_output() {
    name="a_terminal_gets_styled_output"
    has_pty_script || { printf 'skip %s: no util-linux script\n' "$name"; return 0; }
    root="$(new_sandbox "$name")"
    status="$(CASE_TERMINAL=1 run_install "$root")"
    assert_exit "$name" "$root" "$status" 0 || return 1
    if ! grep -q "$(printf '\033')" "$root/stdout"; then
        fail "$name" "no styling on a terminal: $(cat "$root/stdout")"
        return 1
    fi
    pass "$name"
}

no_color_keeps_a_terminal_plain() {
    name="no_color_keeps_a_terminal_plain"
    has_pty_script || { printf 'skip %s: no util-linux script\n' "$name"; return 0; }
    root="$(new_sandbox "$name")"
    status="$(NO_COLOR=1 CASE_TERMINAL=1 run_install "$root")"
    assert_exit "$name" "$root" "$status" 0 || return 1
    if grep -q "$(printf '\033')" "$root/stdout"; then
        fail "$name" "escape codes despite NO_COLOR"
        return 1
    fi
    pass "$name"
}

# The full run: pair, then the service, then where to go.
pairs_then_starts_the_service() {
    name="pairs_then_starts_the_service"
    root="$(new_sandbox "$name")"
    status="$(CASE_SKIP_SERVICE=0 run_install "$root")"
    assert_exit "$name" "$root" "$status" 0 || return 1
    if [ "$(cat "$root/home/bridge-calls")" != "$(printf 'pair\ninstall-service')" ]; then
        fail "$name" "bridge ran: $(cat "$root/home/bridge-calls")"
        return 1
    fi
    assert_says "$name" "$root/stderr" \
        "Pairing with your Build account" \
        "pairing code: TEST-CODE" \
        "Starting the background service" \
        "Done. The Build bridge is installed and running." \
        "Open https://getbuild.ing/app" || return 1
    if [ "$(grep -n 'Pairing with your Build account' "$root/stderr" | head -1 | cut -d: -f1)" -gt \
        "$(grep -n 'Starting the background service' "$root/stderr" | cut -d: -f1)" ]; then
        fail "$name" "service step came before pairing"
        return 1
    fi
    pass "$name"
}

# Where to go follows the bridge's own web address setting.
where_to_go_follows_the_bridge_web_url() {
    name="where_to_go_follows_the_bridge_web_url"
    root="$(new_sandbox "$name")"
    status="$(BRIDGE_WEB_URL=http://localhost:8090 CASE_SKIP_SERVICE=0 run_install "$root")"
    assert_exit "$name" "$root" "$status" 0 || return 1
    assert_says "$name" "$root/stderr" "Open http://localhost:8090/app" && pass "$name"
}

# A pairing that fails leaves the binary installed and says the one command
# that picks up where the installer stopped.
a_failed_pairing_says_how_to_resume() {
    name="a_failed_pairing_says_how_to_resume"
    root="$(new_sandbox "$name")"
    status="$(CASE_SKIP_SERVICE=0 CASE_PAIR_STATUS=1 run_install "$root")"
    assert_exit "$name" "$root" "$status" 1 || return 1
    [ -x "$root/dest/build-bridge" ] || { fail "$name" "binary removed"; return 1; }
    assert_says "$name" "$root/stderr" \
        "Error: pairing did not finish" \
        "$root/dest/build-bridge pair" || return 1
    assert_never_says "$name" "$root/stderr" "Starting the background service" && pass "$name"
}

# Skipping the service is a choice, so it ends on what to run later, not on
# a warning.
skipping_the_service_says_what_to_run_later() {
    name="skipping_the_service_says_what_to_run_later"
    root="$(new_sandbox "$name")"
    status="$(run_install "$root")"
    assert_exit "$name" "$root" "$status" 0 || return 1
    [ ! -e "$root/home/bridge-calls" ] || { fail "$name" "bridge was run"; return 1; }
    assert_says "$name" "$root/stderr" \
        "BUILD_BRIDGE_SKIP_SERVICE=1" \
        "$root/dest/build-bridge pair" \
        "$root/dest/build-bridge install-service" && pass "$name"
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
    success_reads_as_plain_steps \
    a_missing_cosign_goes_unmentioned \
    a_present_cosign_is_one_line \
    a_rejected_signature_is_a_loud_error \
    a_failure_names_what_failed \
    piped_output_is_plain \
    a_terminal_gets_styled_output \
    no_color_keeps_a_terminal_plain \
    pairs_then_starts_the_service \
    where_to_go_follows_the_bridge_web_url \
    a_failed_pairing_says_how_to_resume \
    skipping_the_service_says_what_to_run_later; do
    "$case_name" || true
done

if [ "$FAILURES" -ne 0 ]; then
    printf '%s case(s) failed\n' "$FAILURES" >&2
    exit 1
fi
printf 'install.sh: every case passed\n'
