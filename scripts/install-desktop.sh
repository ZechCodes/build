#!/bin/sh
# Public, user-level Build app installer. Requires curl and tar (macOS bsdtar
# reads ZIP files). Never prompts, starts the app, or uses administrator access.
# BUILD_DESKTOP_VERSION: latest (default) or X.Y.Z
# BUILD_RELEASES_REPO: GitHub owner/repo (default ZechCodes/build-releases)
# Messages go to stderr as a header and one line per step, styled only when
# stderr is a terminal and NO_COLOR is unset; URLs and file names appear only
# in an error, which ends on the one thing to do next. scripts/install.sh
# writes the same way.
set -eu

DEFAULT_REPO="ZechCodes/build-releases"
COSIGN_IDENTITY_REGEXP='^https://github\.com/ZechCodes/(build-web|build)/\.github/workflows/release-desktop\.yml@refs/tags/desktop-v'
COSIGN_ISSUER='https://token.actions.githubusercontent.com'

if [ -t 2 ] && [ -z "${NO_COLOR:-}" ] && [ "${TERM:-}" != dumb ]; then
    BOLD=$(printf '\033[1m') STEP=$(printf '\033[1;34m') GOOD=$(printf '\033[1;32m')
    BAD=$(printf '\033[1;31m') RESET=$(printf '\033[0m')
else
    BOLD='' STEP='' GOOD='' BAD='' RESET=''
fi
AGAIN='Run the installer again.'
BROKEN='Nothing was installed. Run the installer again later; if it fails the same way, this release is broken.'

say() { printf '%s\n' "$*" >&2; }
step() { say "${STEP}==>${RESET} $*"; }
# stdin broken between words into lines of at most $1 columns. A line that
# fits, or starts with spaces (given as it is, like cosign's fields), is kept
# whole; a word longer than a line gets one to itself. Plain shell, because an
# error can come before the tools are checked.
wrap() {
    while IFS= read -r text; do
        case $text in " "*) printf '%s\n' "$text"; continue;; esac
        [ "${#text}" -gt "$1" ] || { printf '%s\n' "$text"; continue; }
        wrapped='' && set -f
        for word in $text; do
            if [ -z "$wrapped" ]; then wrapped=$word
            elif [ $((${#wrapped} + 1 + ${#word})) -gt "$1" ]; then printf '%s\n' "$wrapped"; wrapped=$word
            else wrapped="$wrapped $word"; fi
        done
        set +f && printf '%s\n' "$wrapped"
    done
}
# Lines under a step or a verdict; each argument may itself span lines, and
# each is broken to fit an 80-column terminal.
detail() {
    for text do
        printf '%s\n' "$text" | wrap 76 | while IFS= read -r line; do say "    $line"; done
    done
}
# A verdict: its label, then the sentence after it, the overflow indented
# under it, all of it within 80 columns.
verdict() {
    styled=$1 plain=$2
    shift 2
    say ""
    printf '%s %s\n' "$plain" "$1" | wrap 76 | {
        IFS= read -r first && say "$styled${first#"$plain"}"
        while IFS= read -r line; do say "    $line"; done
    }
    shift
    detail "$@"
}
finish() { verdict "${GOOD}Done.${RESET}" Done. "$@"; }
# An error says what went wrong, then the one thing to do next.
fail_with() { status=$1; shift; verdict "${BAD}Error:${RESET}" Error: "$@"; exit "$status"; }
fail() { fail_with 1 "$@"; }
has() { command -v "$1" >/dev/null 2>&1; }
fetch() {
    curl -fsSL --retry 3 -o "$2" "$1" || fail "could not download $1" \
        "Check your internet connection, then run the installer again."
}
# A path under $HOME as a person would type it.
# shellcheck disable=SC2088 # the ~ is shown, not expanded
shown() { case "$1" in "$HOME"/*) printf '~/%s\n' "${1#"$HOME"/}" ;; *) printf '%s\n' "$1" ;; esac; }

say "${BOLD}Build installer${RESET}"
say ""
case "$(uname -s)/$(uname -m)" in
    Darwin/arm64) platform=macos-arm64 platform_name='macOS (arm64)' ;;
    Darwin/x86_64) platform=macos-x86_64 platform_name='macOS (x86_64)' ;;
    Linux/x86_64|Linux/amd64) platform=linux-x86_64 platform_name='Linux (x86_64)' ;;
    Linux/aarch64|Linux/arm64) platform=linux-aarch64 platform_name='Linux (arm64)' ;;
    *) fail_with 2 "unsupported platform $(uname -s)/$(uname -m)" \
        "The Build app is built for macOS and Linux, on arm64 and x86_64." ;;
esac
for tool in curl tar awk find readlink; do
    has "$tool" || fail "$tool is required to install Build and is not on PATH." \
        "Install $tool, then run the installer again."
done
has sha256sum || has shasum || fail 'sha256sum or shasum is required to verify Build.' \
    "Install either one, then run the installer again."
repo=${BUILD_RELEASES_REPO:-$DEFAULT_REPO}
version=${BUILD_DESKTOP_VERSION:-latest}
work=$(mktemp -d)
stage=
backup=
destination=
cleanup() {
    rm -rf "$work"
    [ -z "$stage" ] || rm -rf "$stage"
    if [ -n "$backup" ] && [ -e "$backup/previous" ] && [ ! -e "$destination" ]; then
        mv "$backup/previous" "$destination"
    fi
    [ -z "$backup" ] || rm -rf "$backup"
}
trap cleanup EXIT
trap 'exit 1' HUP INT TERM
if [ "$version" = latest ]; then
    fetch "https://github.com/$repo/releases/download/desktop-latest/version.txt" "$work/version.txt"
    version=$(cat "$work/version.txt")
fi
printf '%s\n' "$version" | awk '/^[0-9]+\.[0-9]+\.[0-9]+$/ {ok=1} END {exit !ok || NR != 1}' \
    || fail "the version to install, '$version', is not a release number like 1.2.3." \
        "Set BUILD_DESKTOP_VERSION to a release number, or unset it to install the latest."
base="https://github.com/$repo/releases/download/desktop-v$version"
case "$platform" in
    macos-*) asset="build-desktop-$platform.zip"; destination="$HOME/Applications/Build.app" ;;
    linux-*) asset="build-desktop-$platform.tar.gz"; destination="$HOME/.local/share/build-desktop" ;;
esac
step "Downloading Build $version for $platform_name"
fetch "$base/$asset" "$work/$asset"
fetch "$base/SHA256SUMS" "$work/SHA256SUMS"
expected=$(awk -v name="$asset" '$2 == name {print $1; count++} END {exit count != 1}' "$work/SHA256SUMS") \
    || fail "SHA256SUMS from $base must contain exactly one digest for $asset." "$BROKEN"
if has sha256sum; then
    actual=$(sha256sum "$work/$asset" | cut -d ' ' -f 1)
else
    actual=$(shasum -a 256 "$work/$asset" | cut -d ' ' -f 1)
fi
[ "$actual" = "$expected" ] || fail "$asset does not match its published checksum, so nothing was installed." \
    "Downloaded from $base" "expected $expected" "got      $actual" \
    "Run the installer again; a download that keeps failing this check must not be installed by hand."
step "Verified the download"
# Mandatory when cosign is installed and silent when it is not: the checksum
# above is always checked, and a missing optional tool is not news.
if has cosign; then
    fetch "$base/SHA256SUMS.sigstore.json" "$work/SHA256SUMS.sigstore.json"
    cosign_said=$(cosign verify-blob --bundle "$work/SHA256SUMS.sigstore.json" \
        --certificate-identity-regexp "$COSIGN_IDENTITY_REGEXP" \
        --certificate-oidc-issuer "$COSIGN_ISSUER" \
        "$work/SHA256SUMS" 2>&1) || fail "the release signature did not verify, so nothing was installed." \
        "cosign checked SHA256SUMS from $base and said:" "$cosign_said" \
        "Do not install this download by hand. Run the installer again later."
    step "Verified the release signature"
fi

# Native tar additionally protects against extraction through symlinks. Inspect
# paths and entry types first, and check every extracted link before installation.
tar -tf "$work/$asset" > "$work/paths" || fail "could not list $asset from $base." "$BROKEN"
awk '/^\// || /(^|\/)\.\.(\/|$)/ {exit 1}' "$work/paths" || fail "$asset from $base has an unsafe path." "$BROKEN"
tar -tvf "$work/$asset" > "$work/types" || fail "could not inspect $asset from $base." "$BROKEN"
awk 'substr($0,1,1) !~ /^[-dl]$/ {exit 1}' "$work/types" \
    || fail "$asset from $base has an unsupported entry type." "$BROKEN"
mkdir "$work/payload"
tar -xf "$work/$asset" -C "$work/payload" || fail "could not unpack $asset from $base." "$BROKEN"
payload=$(CDPATH='' cd -- "$work/payload" && pwd -P)
find "$payload" -type l -exec sh -c '
    root=$1; shift
    for link do
        target=$(readlink "$link") || exit 1
        case "$target" in /*) exit 1 ;; esac
        # Check physical parents too: a contained link can change how a later
        # '..' component resolves. Reject dangling links rather than guessing.
        [ -e "$link" ] || exit 1
        if [ -d "$link" ]; then
            resolved=$(CDPATH="" cd -P -- "$link" && pwd -P) || exit 1
        else
            resolved=$(CDPATH="" cd -P -- "$(dirname "$link")/$(dirname "$target")" && pwd -P) || exit 1
        fi
        case "$resolved" in "$root"|"$root/"*) ;; *) exit 1 ;; esac
        relative=${link#"$root/"}
        printf "%s/%s\n" "$(dirname "$relative")" "$target" | awk '\''
            {n=split($0, parts, "/"); depth=0
             for(i=1;i<=n;i++) {
                 if(parts[i]=="..") {if(--depth < 0) exit 1}
                 else if(parts[i]!="." && parts[i]!="") depth++
             }}'\'' || exit 1
    done
' sh "$payload" {} + || fail "$asset from $base contains a symlink that points outside it." "$BROKEN"
case "$platform" in
    macos-*)
        if ! { [ -d "$work/payload/Build.app/Contents" ] && [ ! -L "$work/payload/Build.app" ]; }; then
            fail "$asset from $base does not contain Build.app." "$BROKEN"
        fi
        [ "$(find "$work/payload" -mindepth 1 -maxdepth 1 | wc -l | tr -d ' ')" = 1 ] \
            || fail "$asset from $base has files outside Build.app." "$BROKEN"
        source="$work/payload/Build.app"
        ;;
    linux-*)
        if ! { [ -f "$work/payload/build-desktop" ] && [ ! -L "$work/payload/build-desktop" ]; }; then
            fail "$asset from $base does not contain build-desktop." "$BROKEN"
        fi
        chmod 0755 "$work/payload/build-desktop"
        source="$work/payload"
        ;;
esac
parent=$(dirname "$destination")
mkdir -p "$parent"
[ ! -L "$destination" ] || fail "$(shown "$destination") is a symlink, and the installer will not replace one." \
    "Move or remove it, then run the installer again."
stage=$(mktemp -d "$parent/.build-install.XXXXXX")
mv "$source" "$stage/next"
backup=$(mktemp -d "$parent/.build-backup.XXXXXX")
[ ! -e "$destination" ] || mv "$destination" "$backup/previous"
mv "$stage/next" "$destination" || fail "could not install Build at $(shown "$destination"); the previous version is kept." "$AGAIN"
step "Installed to $(shown "$destination")"

if [ "${platform#linux-}" != "$platform" ]; then
    mkdir -p "$HOME/.local/bin" "$HOME/.local/share/applications"
    cat > "$work/launcher" <<'LAUNCHER'
#!/bin/sh
exec "$HOME/.local/share/build-desktop/build-desktop" "$@"
LAUNCHER
    chmod 0755 "$work/launcher"
    mv "$work/launcher" "$HOME/.local/bin/build-desktop"
    # Desktop Exec accepts double-quoted paths with reserved characters escaped.
    desktop_path=$(printf '%s' "$HOME/.local/bin/build-desktop" | sed 's/[\\"`$]/\\&/g; s/%/%%/g')
    cat > "$work/build-desktop.desktop" <<DESKTOP
[Desktop Entry]
Type=Application
Name=Build
Comment=Build desktop app
Exec="$desktop_path" %U
Terminal=false
Categories=Development;
MimeType=x-scheme-handler/getbuilding;
StartupWMClass=Build
DESKTOP
    if [ -f "$destination/resources/icon.png" ]; then
        printf 'Icon=%s\n' "$destination/resources/icon.png" >> "$work/build-desktop.desktop"
    fi
    mv "$work/build-desktop.desktop" "$HOME/.local/share/applications/build-desktop.desktop"
    if has update-desktop-database; then
        update-desktop-database "$HOME/.local/share/applications" > /dev/null 2>&1 \
            || detail 'The applications menu could not be refreshed; Build appears there after you log in again.'
    fi
    if has xdg-mime; then
        xdg-mime default build-desktop.desktop x-scheme-handler/getbuilding > /dev/null 2>&1 \
            || detail 'getbuilding:// links could not be registered; sign-in links may open in the browser instead.'
    fi
    case ":$PATH:" in
        *":$HOME/.local/bin:"*) open_hint='Open Build from your applications menu, or run build-desktop.' ;;
        *) open_hint='Open Build from your applications menu, or run ~/.local/bin/build-desktop.' ;;
    esac
    finish "Build $version is installed." "$open_hint"
else
    finish "Build $version is installed." 'Open ~/Applications/Build.app to start Build.'
fi
