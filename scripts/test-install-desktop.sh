#!/bin/sh
# Offline integration tests: real archives and hashing, mocked transport/platform.
set -eu
script=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd)/install-desktop.sh
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT HUP INT TERM
mkdir -p "$work/shims"
cat > "$work/shims/curl" <<'CURL'
#!/bin/sh
set -eu
while [ $# -gt 0 ]; do
    case "$1" in -o) dest=$2; shift 2;; --retry) shift 2;; -*) shift;; *) url=$1; shift;; esac
done
printf '%s\n' "$url" >> "$TEST_ROOT/urls"
cp "$TEST_ROOT/mirror/${url##*/}" "$dest"
CURL
cat > "$work/shims/uname" <<'UNAME'
#!/bin/sh
case "$1" in -s) echo "$TEST_OS";; -m) echo "$TEST_ARCH";; esac
UNAME
cat > "$work/shims/cosign" <<'COSIGN'
#!/bin/sh
printf '%s\n' "$*" > "$TEST_ROOT/cosign-args"
if [ "${TEST_SIGNATURE_STATUS:-0}" = 0 ]; then
    echo 'Verified OK' >&2
else
    echo 'Error: none of the expected identities matched' >&2
fi
exit "${TEST_SIGNATURE_STATUS:-0}"
COSIGN
# Linux GNU tar does not read ZIP; emulate macOS's native bsdtar there.
cat > "$work/shims/tar" <<'TAR'
#!/bin/sh
exec "$TEST_TAR" "$@"
TAR
# Avoid modifying the real desktop MIME database while testing.
for tool in xdg-mime update-desktop-database; do
    printf '#!/bin/sh\nexit 0\n' > "$work/shims/$tool"
done
chmod +x "$work/shims/"*
if command -v bsdtar >/dev/null 2>&1; then
    TEST_TAR=$(command -v bsdtar)
elif [ "$(uname -s)" = Darwin ]; then
    TEST_TAR=$(command -v tar)
else
    printf 'ZIP fixture tests require bsdtar; install libarchive-tools before running this suite.\n' >&2
    exit 1
fi
export TEST_TAR
# A host without cosign: every shim but cosign, beside links to the tools the
# installer runs, so a cosign on the machine running the suite cannot stand in.
host_without_cosign() {
    mkdir -p "$1"
    for shim in "$work/shims/"*; do
        [ "${shim##*/}" = cosign ] || ln -s "$shim" "$1/${shim##*/}"
    done
    for tool in sh awk find readlink sha256sum shasum mktemp cat cut cp rm mv mkdir chmod sed dirname wc tr gzip; do
        found=$(command -v "$tool") || continue
        [ -e "$1/$tool" ] || ln -s "$found" "$1/$tool"
    done
}
says() {
    for line do grep -qF -- "$line" "$TEST_ROOT/output" || return 1; done
}
never_says() {
    for text do ! grep -qF -- "$text" "$TEST_ROOT/output" || return 1; done
}
escape=$(printf '\033')
# The pseudo-terminal cases need util-linux's `script`.
if script --version 2>/dev/null | grep -q util-linux; then has_pty=1; else has_pty=0; fi
failures=0
run_case() {
    name=$1; TEST_OS=$2; TEST_ARCH=$3; platform=$4; mode=$5
    TEST_ROOT="$work/$name"; TEST_SIGNATURE_STATUS=0
    export TEST_ROOT TEST_OS TEST_ARCH TEST_SIGNATURE_STATUS
    mkdir -p "$TEST_ROOT/mirror" "$TEST_ROOT/payload" "$TEST_ROOT/home"
    if [ "$TEST_OS" = Darwin ]; then
        asset="build-desktop-$platform.zip"
        mkdir -p "$TEST_ROOT/payload/Build.app/Contents/MacOS"
        printf 'new app\n' > "$TEST_ROOT/payload/Build.app/Contents/MacOS/Build"
        ln -s MacOS "$TEST_ROOT/payload/Build.app/Contents/Current"
        target="$TEST_ROOT/home/Applications/Build.app"
    else
        asset="build-desktop-$platform.tar.gz"
        printf '#!/bin/sh\necho new app\n' > "$TEST_ROOT/payload/build-desktop"
        target="$TEST_ROOT/home/.local/share/build-desktop"
    fi
    mkdir -p "$target"
    echo previous > "$target/previous"
    case "$mode" in
        escaping-link) ln -s ../../outside "$TEST_ROOT/payload/escape";;
        absolute-link) ln -s /tmp "$TEST_ROOT/payload/escape";;
        missing-binary) rm "$TEST_ROOT/payload/build-desktop";;
        bad-signature) TEST_SIGNATURE_STATUS=1;;
    esac
    case "$asset" in
        *.zip) "$TEST_TAR" --format zip -cf "$TEST_ROOT/mirror/$asset" -C "$TEST_ROOT/payload" Build.app;;
        *) tar -czf "$TEST_ROOT/mirror/$asset" -C "$TEST_ROOT/payload" .;;
    esac
    if [ "$mode" = absolute-path ]; then
        tar -P -czf "$TEST_ROOT/mirror/$asset" "$TEST_ROOT/payload/build-desktop"
    fi
    [ "$mode" != corrupt-archive ] || printf garbage > "$TEST_ROOT/mirror/$asset"
    if command -v sha256sum >/dev/null 2>&1; then
        digest=$(sha256sum "$TEST_ROOT/mirror/$asset" | cut -d ' ' -f 1)
    else
        digest=$(shasum -a 256 "$TEST_ROOT/mirror/$asset" | cut -d ' ' -f 1)
    fi
    printf '%s  %s\n' "$digest" "$asset" > "$TEST_ROOT/mirror/SHA256SUMS"
    : > "$TEST_ROOT/mirror/SHA256SUMS.sigstore.json"
    echo 1.2.3 > "$TEST_ROOT/mirror/version.txt"
    case "$mode" in
        tampered) echo tampered >> "$TEST_ROOT/mirror/$asset";;
        download-failure) rm "$TEST_ROOT/mirror/$asset";;
        missing-checksum) : > "$TEST_ROOT/mirror/SHA256SUMS";;
        bad-version) echo '../bad' > "$TEST_ROOT/mirror/version.txt";;
    esac
    status=0
    if [ "$mode" = pinned ]; then version=2.3.4; else version=latest; fi
    run_path="$work/shims:$PATH"
    if [ "$mode" = no-cosign ]; then
        host_without_cosign "$TEST_ROOT/host"
        run_path="$TEST_ROOT/host"
    fi
    case "$mode" in
        terminal|terminal-no-color|terminal-dumb)
            no_color=1 term=xterm
            [ "$mode" != terminal ] || no_color=''
            [ "$mode" != terminal-dumb ] || no_color='' term=dumb
            HOME="$TEST_ROOT/home" PATH="$run_path" BUILD_DESKTOP_VERSION="$version" \
                BUILD_RELEASES_REPO=example/releases TERM="$term" NO_COLOR="$no_color" \
                script -qec "/bin/sh '$script'" /dev/null > "$TEST_ROOT/output" 2>&1 || status=$?
            ;;
        *)
            HOME="$TEST_ROOT/home" PATH="$run_path" BUILD_DESKTOP_VERSION="$version" \
                BUILD_RELEASES_REPO=example/releases /bin/sh "$script" > "$TEST_ROOT/output" 2>&1 || status=$?
            ;;
    esac
    # Output that is not a terminal is plain, whatever happened.
    case "$mode" in
        terminal)
            grep -q "$escape" "$TEST_ROOT/output" || return 1
            # The words are checked below with the styling taken out.
            sed "s/$escape\[[0-9;]*m//g" "$TEST_ROOT/output" > "$TEST_ROOT/words"
            mv "$TEST_ROOT/words" "$TEST_ROOT/output"
            ;;
        *) ! grep -q "$escape" "$TEST_ROOT/output" || return 1 ;;
    esac
    # Nothing leans on the terminal to break a sentence (#319): a line over 80
    # columns is a single word, a URL or a path, that no break could help. The
    # curl shim's own `cp:` complaint is the shim's, not the installer's.
    ! tr -d '\r' < "$TEST_ROOT/output" | awk 'length($0) > 80 && /[^ ] +[^ ]/ && !/^cp: /' | grep -q . || return 1
    case "$mode" in
        success|pinned|no-cosign|terminal|terminal-no-color|terminal-dumb)
            if ! { [ "$status" = 0 ] && [ ! -e "$target/previous" ]; }; then return 1; fi
            if [ "$TEST_OS" = Linux ]; then
                if ! { [ -x "$target/build-desktop" ] && [ -x "$TEST_ROOT/home/.local/bin/build-desktop" ]; }; then return 1; fi
                [ -f "$TEST_ROOT/home/.local/share/applications/build-desktop.desktop" ] || return 1
            else
                if ! { [ -f "$target/Contents/MacOS/Build" ] && [ -L "$target/Contents/Current" ]; }; then return 1; fi
            fi
            if [ "$mode" = pinned ]; then
                ! grep -q desktop-latest "$TEST_ROOT/urls" || return 1
                grep -q /desktop-v2.3.4/ "$TEST_ROOT/urls" || return 1
            else
                grep -q /desktop-v1.2.3/ "$TEST_ROOT/urls" || return 1
            fi
            # Plain steps: no URLs or archive names unless something failed.
            never_says https:// .tar.gz .zip SHA256SUMS rror || return 1
            shipped=1.2.3
            [ "$mode" != pinned ] || shipped=2.3.4
            says "Verified the download" "Done. Build $shipped is installed." || return 1
            if [ "$mode" = no-cosign ]; then
                [ ! -e "$TEST_ROOT/cosign-args" ] || return 1
                never_says cosign signature skipping || return 1
            else
                grep -q 'release-desktop' "$TEST_ROOT/cosign-args" || return 1
                says "Verified the release signature" || return 1
                never_says "Verified OK" || return 1
            fi
            if [ "$TEST_OS" = Linux ]; then
                says "Downloading Build $shipped for Linux (" "Installed to ~/.local/share/build-desktop" || return 1
            else
                says "Downloading Build $shipped for macOS (" "Installed to ~/Applications/Build.app" \
                    "Open ~/Applications/Build.app to start Build." || return 1
            fi
            ;;
        *) if ! { [ "$status" != 0 ] && [ "$(cat "$target/previous")" = previous ]; }; then return 1; fi;;
    esac
    # A refusal reads as an error, names what failed, and says what to do next.
    case "$mode" in
        tampered) says "Error: build-desktop-linux-x86_64.tar.gz does not match its published" \
            "https://github.com/example/releases/releases/download/desktop-v1.2.3" "Run the installer again" ;;
        bad-signature) says "Error: the release signature did not verify" "none of the expected identities matched" ;;
        download-failure) says "Error: could not download" "run the installer again" ;;
        unsupported) says "Error: unsupported platform Plan9/mips" ;;
        *) true ;;
    esac
}
check() {
    if run_case "$@"; then printf 'ok %s\n' "$1"; else
        printf 'FAIL %s\n' "$1"
        cat "$work/$1/output" 2>/dev/null || true
        failures=$((failures + 1))
    fi
}
check linux-x64 Linux x86_64 linux-x86_64 success
check linux-arm Linux aarch64 linux-aarch64 success
check pinned Linux x86_64 linux-x86_64 pinned
check mac-arm Darwin arm64 macos-arm64 success
check mac-intel Darwin x86_64 macos-x86_64 success
check no-cosign Linux x86_64 linux-x86_64 no-cosign
if [ "$has_pty" = 1 ]; then
    check terminal Linux x86_64 linux-x86_64 terminal
    check terminal-no-color Linux x86_64 linux-x86_64 terminal-no-color
    check terminal-dumb Linux x86_64 linux-x86_64 terminal-dumb
else
    printf 'skip terminal cases: no util-linux script\n'
fi
for mode in tampered download-failure missing-checksum bad-signature bad-version corrupt-archive absolute-path escaping-link absolute-link missing-binary; do
    check "$mode" Linux x86_64 linux-x86_64 "$mode"
done
check unsupported Plan9 mips linux-x86_64 unsupported
[ "$failures" = 0 ] || exit 1
printf 'install-desktop.sh: every case passed\n'
