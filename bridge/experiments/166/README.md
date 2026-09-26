# #166: relayed replies wait for the webrtc driver's next wake

A private measurement, not a fix: nothing here is built into the bridge. The
numbers and the reasoning are on issue #166.

- `A-reorder.patch` is Experiment A. It changes `webrtc` 0.20.4's `poll_writes`
  to drain the TURN relayer after the core (gatherer, core, relayer).
- `B-readiness.patch` is Experiment B, on the original order. Before the driver
  waits, it takes up to two more passes while the TURN client or relayer holds
  sendable output, or while relayed input reached the core. The patch touches
  `webrtc` and `rtc-turn` 0.20.4 (`Client::has_pending_write`) and adds two
  driver tests.
- `runs/*.log` holds one idle soak per variant and path. Each log has every
  round trip on its `rtts` line, plus loadavg and container packet counters
  before and after.

## Reproducing

The crate copies are gitignored. Recreate them from the registry and apply a
patch:

    R=~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f
    cp -r $R/webrtc-0.20.4 webrtc && git apply A-reorder.patch
    # B patches paths under webrtc/ and rtc-turn/, so apply it where webrtc/ is
    # a fresh copy, then rename that copy to webrtc-b/:
    cp -r $R/rtc-turn-0.20.4 rtc-turn && git apply B-readiness.patch

`mkpatch.sh` writes both patches back out from the copies.

Point the bridge at a copy locally, and never commit it:

    # bridge/Cargo.toml
    [patch.crates-io]
    webrtc = { path = "experiments/166/webrtc" }          # A
    # webrtc = { path = "experiments/166/webrtc-b" }      # B
    # rtc-turn = { path = "experiments/166/rtc-turn" }    # B

Then, from the repo root, under the docker group:

    bridge/experiments/166/stack.sh up                   # or `rebuild` after switching the patch
    bridge/experiments/166/stack.sh soak NAME            # relay/relay, idle, 250 pings
    RELAY_BOTH_ENDS=0 ICE_TRANSPORT_POLICY=all bridge/experiments/166/stack.sh soak NAME-host
    bridge/experiments/166/stack.sh down
    python3 bridge/experiments/166/summarize.py bridge/experiments/166/runs/*.log

The stack is the #128 liveness stack under compose project `drain166`, on ports
8166/18166. B's tests run in the crate copy:

    cd webrtc-b && printf '\n[patch.crates-io]\nrtc-turn = { path = "../rtc-turn" }\n' >> Cargo.toml
    CARGO_TARGET_DIR=~/.cache/build-166-target cargo test --lib relayed_output

Use a target directory outside `bridge/`: the docker build context copies this
directory.
