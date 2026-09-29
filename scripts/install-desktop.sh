#!/bin/sh
# Public, user-level Build app installer. Requires curl and tar (macOS bsdtar
# reads ZIP files). Never prompts, starts the app, or uses administrator access.
# BUILD_DESKTOP_VERSION: latest (default) or X.Y.Z
# BUILD_RELEASES_REPO: GitHub owner/repo (default ZechCodes/build-releases)
set -eu

DEFAULT_REPO="ZechCodes/build-releases"
COSIGN_IDENTITY_REGEXP='^https://github\.com/ZechCodes/(build-web|build)/\.github/workflows/release-desktop\.yml@refs/tags/desktop-v'
COSIGN_ISSUER='https://token.actions.githubusercontent.com'

say() { printf '%s\n' "$*" >&2; }
fail() { say "$1"; exit "${2:-1}"; }
has() { command -v "$1" >/dev/null 2>&1; }
fetch() { curl -fsSL --retry 3 -o "$2" "$1" || fail "could not download $1"; }

case "$(uname -s)/$(uname -m)" in
    Darwin/arm64) platform=macos-arm64 ;;
    Darwin/x86_64) platform=macos-x86_64 ;;
    Linux/x86_64|Linux/amd64) platform=linux-x86_64 ;;
    Linux/aarch64|Linux/arm64) platform=linux-aarch64 ;;
    *) fail "unsupported platform $(uname -s)/$(uname -m)" 2 ;;
esac
for tool in curl tar awk find readlink; do
    has "$tool" || fail "$tool is required to install Build"
done
has sha256sum || has shasum || fail 'sha256sum or shasum is required to verify Build'
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
    || fail 'BUILD_DESKTOP_VERSION (or published version.txt) must be X.Y.Z'
base="https://github.com/$repo/releases/download/desktop-v$version"
case "$platform" in
    macos-*) asset="build-desktop-$platform.zip"; destination="$HOME/Applications/Build.app" ;;
    linux-*) asset="build-desktop-$platform.tar.gz"; destination="$HOME/.local/share/build-desktop" ;;
esac
say "Downloading Build $version for $platform"
fetch "$base/$asset" "$work/$asset"
fetch "$base/SHA256SUMS" "$work/SHA256SUMS"
expected=$(awk -v name="$asset" '$2 == name {print $1; count++} END {exit count != 1}' "$work/SHA256SUMS") \
    || fail "SHA256SUMS must contain exactly one digest for $asset"
if has sha256sum; then
    actual=$(sha256sum "$work/$asset" | cut -d ' ' -f 1)
else
    actual=$(shasum -a 256 "$work/$asset" | cut -d ' ' -f 1)
fi
[ "$actual" = "$expected" ] || fail "checksum mismatch for $asset"
if has cosign; then
    fetch "$base/SHA256SUMS.sigstore.json" "$work/SHA256SUMS.sigstore.json"
    cosign verify-blob --bundle "$work/SHA256SUMS.sigstore.json" \
        --certificate-identity-regexp "$COSIGN_IDENTITY_REGEXP" \
        --certificate-oidc-issuer "$COSIGN_ISSUER" \
        "$work/SHA256SUMS" >&2 || fail 'signature verification failed; refusing to install'
else
    say 'cosign not found; skipping signature verification (checksum verified)'
fi

# Native tar additionally protects against extraction through symlinks. Inspect
# paths and entry types first, and check every extracted link before installation.
tar -tf "$work/$asset" > "$work/paths" || fail "could not list $asset"
awk '/^\// || /(^|\/)\.\.(\/|$)/ {exit 1}' "$work/paths" || fail 'unsafe archive path'
tar -tvf "$work/$asset" > "$work/types" || fail "could not inspect $asset"
awk 'substr($0,1,1) !~ /^[-dl]$/ {exit 1}' "$work/types" || fail 'unsupported archive entry type'
mkdir "$work/payload"
tar -xf "$work/$asset" -C "$work/payload" || fail "could not unpack $asset"
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
' sh "$payload" {} + || fail 'archive contains an escaping symlink'
case "$platform" in
    macos-*)
        if ! { [ -d "$work/payload/Build.app/Contents" ] && [ ! -L "$work/payload/Build.app" ]; }; then
            fail 'archive does not contain Build.app'
        fi
        [ "$(find "$work/payload" -mindepth 1 -maxdepth 1 | wc -l | tr -d ' ')" = 1 ] \
            || fail 'unexpected files outside Build.app'
        source="$work/payload/Build.app"
        ;;
    linux-*)
        if ! { [ -f "$work/payload/build-desktop" ] && [ ! -L "$work/payload/build-desktop" ]; }; then
            fail 'archive does not contain build-desktop'
        fi
        chmod 0755 "$work/payload/build-desktop"
        source="$work/payload"
        ;;
esac
parent=$(dirname "$destination")
mkdir -p "$parent"
[ ! -L "$destination" ] || fail "refusing to replace symlink $destination"
stage=$(mktemp -d "$parent/.build-install.XXXXXX")
mv "$source" "$stage/next"
backup=$(mktemp -d "$parent/.build-backup.XXXXXX")
[ ! -e "$destination" ] || mv "$destination" "$backup/previous"
mv "$stage/next" "$destination" || fail 'could not install Build; restoring previous version'

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
        update-desktop-database "$HOME/.local/share/applications" >&2 || say 'Could not refresh the desktop application database.'
    fi
    if has xdg-mime; then
        xdg-mime default build-desktop.desktop x-scheme-handler/getbuilding >&2 || say 'Could not register the getbuilding URL handler.'
    fi
    say 'Use build-desktop to open Build (add ~/.local/bin to PATH if needed).'
else
    say 'Open ~/Applications/Build.app to start Build.'
fi
say "Installed Build $version at $destination"
