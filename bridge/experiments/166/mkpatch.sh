#!/usr/bin/env bash
# #166: write A-reorder.patch and B-readiness.patch from the local crate copies
# against the registry sources they were copied from. Paths are a/<crate>/src/….
set -euo pipefail
cd "$(dirname "$0")"
R=${CARGO_HOME:-$HOME/.cargo}/registry/src/index.crates.io-1949cf8c6b5b557f
mk() { # registry dir, local copy, name in the patch
  git -c diff.noprefix=false -c diff.mnemonicPrefix=false diff --no-index --src-prefix=a/ --dst-prefix=b/ \
    "$R/$1/src" "$2/src" | sed -e "s#a/${R#/}/$1/#a/$3/#g" -e "s#b/$2/#b/$3/#g" || true
}
mk webrtc-0.20.4 webrtc webrtc >A-reorder.patch
{ mk webrtc-0.20.4 webrtc-b webrtc; mk rtc-turn-0.20.4 rtc-turn rtc-turn; } >B-readiness.patch
