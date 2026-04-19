#!/bin/sh
# Build device client installer — https://getbuild.ing
# Usage: curl -sSf getbuild.ing | sh
set -eu

BOLD='\033[1m'
DIM='\033[2m'
CYAN='\033[36m'
GREEN='\033[32m'
YELLOW='\033[33m'
RED='\033[31m'
RESET='\033[0m'

BRIDGE_REF="${BUILD_BRIDGE_REF:-main}"
TRANSPORT_REF="${BUILD_TRANSPORT_REF:-main}"
BRIDGE_TARBALL="https://codeload.github.com/ZechCodes/build-bridge/tar.gz/refs/heads/$BRIDGE_REF"
TRANSPORT_TARBALL="https://codeload.github.com/ZechCodes/build-secure-transport/tar.gz/refs/heads/$TRANSPORT_REF"

DATA_HOME="${XDG_DATA_HOME:-$HOME/.local/share}/build"
BIN_HOME="$HOME/.local/bin"
SHIM_PATH="$BIN_HOME/build"
BRIDGE_DIR="$DATA_HOME/build-bridge"
TRANSPORT_DIR="$DATA_HOME/build-secure-transport"

LAUNCHD_LABEL="sh.getbuild.device"
LAUNCHD_PLIST="$HOME/Library/LaunchAgents/$LAUNCHD_LABEL.plist"
SYSTEMD_UNIT="build-device.service"
SYSTEMD_PATH="${XDG_CONFIG_HOME:-$HOME/.config}/systemd/user/$SYSTEMD_UNIT"
LOG_DIR="${XDG_CONFIG_HOME:-$HOME/.config}/build/logs"

info()  { printf "  ${CYAN}=>${RESET} %s\n" "$1"; }
ok()    { printf "  ${GREEN}✓${RESET}  %s\n" "$1"; }
warn()  { printf "  ${YELLOW}!${RESET}  %s\n" "$1"; }
err()   { printf "  ${RED}x${RESET}  %s\n" "$1" >&2; }
dim()   { printf "     ${DIM}%s${RESET}\n" "$1"; }

# Read a single yes/no line from the controlling TTY.
# Echoes "yes", "no", or "" (no TTY available — caller uses default).
prompt_yn() {
    question="$1"
    default="$2"  # yes | no
    if [ ! -r /dev/tty ] || [ ! -t 1 ]; then
        printf ""
        return 0
    fi
    if [ "$default" = "yes" ]; then
        suffix="[Y/n]"
    else
        suffix="[y/N]"
    fi
    printf "  ${CYAN}?${RESET}  %s %s " "$question" "$suffix" > /dev/tty
    IFS= read -r reply < /dev/tty || reply=""
    case "$reply" in
        [Yy]|[Yy][Ee][Ss]) printf "yes" ;;
        [Nn]|[Nn][Oo])     printf "no" ;;
        "")                printf "%s" "$default" ;;
        *)                 printf "%s" "$default" ;;
    esac
}

# --- Header ---
printf "\n"
printf "  ${BOLD}Build${RESET} — device client installer\n"
printf "  ${DIM}https://getbuild.ing${RESET}\n"
printf "\n"
printf "  ${DIM}--------------------------------${RESET}\n"
printf "\n"

# --- Platform check ---
OS="$(uname -s)"
case "$OS" in
    Darwin) PLATFORM="macos" ;;
    Linux)  PLATFORM="linux" ;;
    *)
        err "Unsupported platform: $OS"
        dim "Build's installer currently supports macOS and Linux."
        exit 1
        ;;
esac
ok "Platform: $PLATFORM"

# --- Required tools ---
need_cmd() {
    if ! command -v "$1" > /dev/null 2>&1; then
        err "Missing required command: $1"
        exit 1
    fi
}
need_cmd curl
need_cmd tar

# --- Ensure uv is available ---
if command -v uv > /dev/null 2>&1; then
    ok "uv already installed ($(uv --version))"
else
    info "Installing uv (Astral's Python package manager)…"
    curl -LsSf https://astral.sh/uv/install.sh | sh
    # uv installs to ~/.local/bin by default; make sure we can find it for the rest of this script.
    if [ -x "$HOME/.local/bin/uv" ]; then
        PATH="$HOME/.local/bin:$PATH"
        export PATH
    fi
    if ! command -v uv > /dev/null 2>&1; then
        err "uv installation failed — couldn't find 'uv' on PATH after install."
        exit 1
    fi
    ok "uv installed"
fi

# --- Download and extract source ---
mkdir -p "$DATA_HOME"

download_and_extract() {
    url="$1"
    dir="$2"
    name="$3"
    ref="$4"
    verb="Downloading"
    [ -d "$dir" ] && verb="Updating"
    info "$verb $name…"
    tmp_dir="$(mktemp -d)"
    # Trap cleanup for this invocation only.
    trap 'rm -rf "$tmp_dir"' EXIT
    if ! curl -fsSL -o "$tmp_dir/src.tar.gz" "$url"; then
        err "Failed to download $name from $url"
        exit 1
    fi
    if ! tar -xz -C "$tmp_dir" -f "$tmp_dir/src.tar.gz"; then
        err "Failed to extract $name archive."
        exit 1
    fi
    extracted="$(find "$tmp_dir" -mindepth 1 -maxdepth 1 -type d | head -n 1)"
    if [ -z "$extracted" ]; then
        err "Tarball for $name did not contain an extracted directory."
        exit 1
    fi
    rm -rf "$dir"
    mv "$extracted" "$dir"
    # Record what we installed for later diagnosis / diffing.
    printf 'ref=%s\ndate=%s\nurl=%s\n' "$ref" "$(date -u +%Y-%m-%dT%H:%M:%SZ)" "$url" > "$dir/.build-install"
    rm -rf "$tmp_dir"
    trap - EXIT
    ok "$name ready"
}

download_and_extract "$TRANSPORT_TARBALL" "$TRANSPORT_DIR" "build-secure-transport" "$TRANSPORT_REF"
download_and_extract "$BRIDGE_TARBALL"    "$BRIDGE_DIR"    "build-bridge"           "$BRIDGE_REF"

# --- Install deps ---
info "Installing Python dependencies (uv sync)…"
(cd "$BRIDGE_DIR" && uv sync --quiet)
ok "Dependencies installed"

VENV_BUILD="$BRIDGE_DIR/.venv/bin/build"
if [ ! -x "$VENV_BUILD" ]; then
    err "Expected launcher not found at $VENV_BUILD"
    exit 1
fi

# --- Install shim to ~/.local/bin/build ---
mkdir -p "$BIN_HOME"
cat > "$SHIM_PATH" <<EOF
#!/bin/sh
# Launcher for the Build device client. Managed by the Build installer.
# Source: https://getbuild.ing
exec "$VENV_BUILD" "\$@"
EOF
chmod +x "$SHIM_PATH"
ok "Installed launcher at $SHIM_PATH"

# --- PATH check ---
case ":$PATH:" in
    *":$BIN_HOME:"*) PATH_OK=1 ;;
    *)               PATH_OK=0 ;;
esac

if [ "$PATH_OK" -ne 1 ]; then
    warn "$BIN_HOME is not on your PATH."
    user_shell="$(basename "${SHELL:-sh}")"
    case "$user_shell" in
        zsh)  rc_file="$HOME/.zshrc" ;;
        bash) rc_file="$HOME/.bashrc" ;;
        *)    rc_file="$HOME/.profile" ;;
    esac
    path_line="export PATH=\"\$HOME/.local/bin:\$PATH\""
    dim "Add this line to $rc_file to make 'build' available:"
    printf "\n     %s\n\n" "$path_line"
    answer="$(prompt_yn "Append it to $rc_file for you?" "no")"
    if [ "$answer" = "yes" ]; then
        printf '\n# Added by Build installer (https://getbuild.ing)\n%s\n' "$path_line" >> "$rc_file"
        ok "Updated $rc_file — open a new shell or 'source $rc_file' to pick it up."
    else
        dim "Skipped. Add it manually when you're ready."
    fi
else
    ok "$BIN_HOME is on your PATH."
fi

# --- Auto-start prompt ---
mkdir -p "$LOG_DIR"

setup_launchd() {
    cat > "$LAUNCHD_PLIST" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>$LAUNCHD_LABEL</string>
    <key>ProgramArguments</key>
    <array>
        <string>$SHIM_PATH</string>
        <string>start</string>
        <string>--foreground</string>
    </array>
    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <true/>
    <key>StandardOutPath</key>
    <string>$LOG_DIR/launchd.out.log</string>
    <key>StandardErrorPath</key>
    <string>$LOG_DIR/launchd.err.log</string>
</dict>
</plist>
EOF
    # bootstrap is idempotent-ish: unload first if already loaded, then bootstrap.
    launchctl bootout "gui/$(id -u)/$LAUNCHD_LABEL" 2>/dev/null || true
    launchctl bootstrap "gui/$(id -u)" "$LAUNCHD_PLIST"
    ok "LaunchAgent installed at $LAUNCHD_PLIST"
}

setup_systemd() {
    mkdir -p "$(dirname "$SYSTEMD_PATH")"
    cat > "$SYSTEMD_PATH" <<EOF
[Unit]
Description=Build device client
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
ExecStart=$SHIM_PATH start --foreground
Restart=on-failure
RestartSec=5
StandardOutput=append:$LOG_DIR/systemd.out.log
StandardError=append:$LOG_DIR/systemd.err.log

[Install]
WantedBy=default.target
EOF
    systemctl --user daemon-reload
    systemctl --user enable --now "$SYSTEMD_UNIT"
    ok "systemd user unit installed at $SYSTEMD_PATH"
}

autostart_answer="$(prompt_yn "Start Build automatically when you log in?" "no")"
if [ "$autostart_answer" = "yes" ]; then
    if [ "$PLATFORM" = "macos" ]; then
        setup_launchd
    else
        if ! command -v systemctl > /dev/null 2>&1; then
            warn "systemctl not found — skipping auto-start setup."
        else
            setup_systemd
        fi
    fi
else
    dim "Skipped auto-start. You can set it up later by re-running this installer."
fi

# --- Summary ---
printf "\n"
printf "  ${DIM}--------------------------------${RESET}\n"
printf "  ${GREEN}${BOLD}Install complete.${RESET}\n"
printf "\n"
printf "  ${BOLD}Next steps${RESET}\n"
if [ "$PATH_OK" -ne 1 ] && [ "${answer:-no}" != "yes" ]; then
    printf "    1. Add %s to your PATH (instructions above).\n" "$BIN_HOME"
    printf "    2. Run ${BOLD}build start${RESET} to authorize this device.\n"
else
    printf "    Run ${BOLD}build start${RESET} to authorize this device.\n"
fi
printf "\n"
printf "  ${BOLD}Where things live${RESET}\n"
printf "    source   %s\n" "$DATA_HOME"
printf "    launcher %s\n" "$SHIM_PATH"
printf "    config   %s\n" "${XDG_CONFIG_HOME:-$HOME/.config}/build"
printf "\n"
printf "  ${BOLD}Uninstall${RESET}\n"
if [ "$PLATFORM" = "macos" ] && [ -f "$LAUNCHD_PLIST" ]; then
    printf "    launchctl bootout gui/\$(id -u)/%s\n" "$LAUNCHD_LABEL"
    printf "    rm %s\n" "$LAUNCHD_PLIST"
fi
if [ "$PLATFORM" = "linux" ] && [ -f "$SYSTEMD_PATH" ]; then
    printf "    systemctl --user disable --now %s\n" "$SYSTEMD_UNIT"
    printf "    rm %s\n" "$SYSTEMD_PATH"
fi
printf "    rm -rf %s %s %s\n" "$DATA_HOME" "${XDG_CONFIG_HOME:-$HOME/.config}/build" "$SHIM_PATH"
printf "\n"
