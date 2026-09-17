#!/bin/sh
# Build the native bridge without signing or installing it.
set -eu

case "${1-}" in
  -h|--help) echo 'Usage: ./scripts/build-bridge.sh'; exit 0 ;;
esac
if [ "$#" -ne 0 ]; then
  echo 'Usage: ./scripts/build-bridge.sh' >&2
  exit 1
fi
case "$(uname -s)" in
  Linux|Darwin) ;;
  *) echo 'The bridge currently supports Linux and macOS only.' >&2; exit 1 ;;
esac
for tool in cargo rustc cc cmake pkg-config; do
  if ! command -v "$tool" >/dev/null 2>&1; then
    echo "Missing prerequisite: $tool. See README.md (Build locally)." >&2
    exit 1
  fi
done

repo_dir=$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)
# Explicit target/output paths keep the artifact location predictable even
# when the caller has configured a shared Cargo target directory.
host_target=$(rustc -vV | sed -n 's/^host: //p')
cargo build --manifest-path "$repo_dir/bridge/Cargo.toml" \
  --release --locked --bin build-bridge --target "$host_target" \
  --target-dir "$repo_dir/bridge/target"
printf '\nBridge built: %s/bridge/target/%s/release/build-bridge\n' "$repo_dir" "$host_target"
