# Vendored crates: webrtc and rtc-turn 0.20.4, patched (#166)

The bridge builds two crates from here instead of crates.io, through the
`[patch.crates-io]` entries at the end of `bridge/Cargo.toml`:

| crate | upstream | crates.io checksum |
| --- | --- | --- |
| `webrtc/` | [webrtc 0.20.4](https://crates.io/crates/webrtc/0.20.4) (github.com/webrtc-rs/webrtc) | `3daa8f2f6366331ae3275a6c02a855c6fb3faa1d16960498d7daaf61c96e76bd` |
| `rtc-turn/` | [rtc-turn 0.20.4](https://crates.io/crates/rtc-turn/0.20.4) (github.com/webrtc-rs/rtc) | `ebf0b5fbb94085c86be7277c38ae8f1c21ec75ab47ba975be0984409732020dd` |

Each directory is the published crate as the registry unpacks it, with its
upstream `Cargo.toml`, `Cargo.toml.orig`, `Cargo.lock` and license files,
plus `B-readiness.patch`. Nothing else is changed, with two exceptions:

- The published rtc-turn ships no license files, so `rtc-turn/LICENSE-MIT`
  and `rtc-turn/LICENSE-APACHE` are copied from rtc 0.20.4, the crate from the
  same repository and release.
- `webrtc/codecov.yml` is left out. It is upstream's CI configuration and
  holds webrtc-rs's Codecov upload token, which is not ours to carry and which
  gitleaks rightly flags.

Upstreaming is deferred (Zech on #166): no fork and no upstream PR for now.

## What changed and why

[#166](https://github.com/ZechCodes/build-web/issues/166): on a TURN-relayed
path, an idle reply waited up to 200 ms before it left the bridge. The webrtc
driver's loop drains the TURN relayer (step 1.b) before the core (1.c), so a
relayed write the core queued in 1.c sat in the TURN client's queue until the
driver next woke. When the connection is idle, the next wake is ICE's 200 ms
check timer. Measured relay/relay idle round trips were p50 98 ms and max
202 ms.

`B-readiness.patch` adds a readiness check before the driver waits:

- `rtc-turn`: `Client::has_pending_write()`, true when the client or one of
  its transactions holds a queued transmit.
- `webrtc`, `transports/turn_relayer.rs`: the relayer's
  `has_pending_write()`, which checks its outer queue and every client. A
  packet held for a missing permission does not count, because it cannot
  leave yet.
- `webrtc`, `peer_connection/driver.rs`: one pass (writes, events, reads) runs
  as `poll_pass()`. The driver takes at most `MAX_READY_PASSES` (2) more
  passes while the relayer holds sendable output or relayed input reached the
  core, and only then waits. `poll_reads()` now reports whether it fed the
  core. It stays a loop condition, not a waker: the loop is bounded, and the
  loop's order is unchanged.
- Two driver tests in `driver.rs` (`mod relayed_output`), against a mock TURN
  server on a real UDP socket:
  - `a_relayed_core_write_leaves_in_the_same_pass`
  - `a_packet_held_for_a_permission_leaves_in_the_pass_that_takes_the_grant`

  Both fail with the check turned off.

On the vendored build, relay/relay idle round trips are p50 1.7 ms, p95
2.2 ms and max 2.8 ms over 250 pings. The number of packets on the wire is
unchanged: 5.25 in and 5.12 out of the bridge per ping, against 5.27 and 5.11
before the patch. The measurement harness is `bridge/experiments/166/` at
commit `1ca86df4`.

## Tests

The bridge's own gates never reach in here. Neither crate is a member of the
bridge's workspace (it has no `[workspace]` table, so it is the only member),
so `cargo fmt --all`, `cargo clippy` and `cargo test --all` in `bridge/` stop
at the bridge. These crates are tested through the small workspace in this
directory, which patches the webrtc crate onto this rtc-turn and has its own
lockfile. From `bridge/`:

    nice -n 10 cargo test --locked --manifest-path vendor/Cargo.toml --lib

That runs both crates' unit tests, the two driver tests included. webrtc's
integration tests (`tests/`) are left out: `play_save_disk` needs media files
the published crate does not ship.

## Re-deriving from the registry

From the repo root, with both crates in the local registry (a `cargo fetch`
in `bridge/` before this change, or any build of the unpatched crates):

    R=~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f
    V=$(mktemp -d)
    cp -r $R/webrtc-0.20.4 $V/webrtc && cp -r $R/rtc-turn-0.20.4 $V/rtc-turn
    rm -f "${V:?}"/*/.cargo-ok "${V:?}"/webrtc/codecov.yml
    cp $R/rtc-0.20.4/LICENSE-MIT $R/rtc-0.20.4/LICENSE-APACHE $V/rtc-turn/
    (cd $V && patch -p1 < "$OLDPWD/bridge/vendor/B-readiness.patch")
    diff -r -x target $V/webrtc bridge/vendor/webrtc && diff -r -x target $V/rtc-turn bridge/vendor/rtc-turn

No output means the vendored tree is exactly the registry source plus the
patch. To move to a new upstream version, copy the new crates over these
directories, apply the patch (fix it where it no longer applies), and update
the checksums above and `bridge/Cargo.lock`. The crates' own `.gitignore` and
the global gitignore drop a few upstream files (`webrtc/Cargo.lock`,
`webrtc/.vscode/`), so stage them with `git add -f`.

After editing the vendored sources, regenerate the patch against a fresh
registry copy:

    P=$(mktemp -d); mkdir $P/a $P/b
    cp -r $R/webrtc-0.20.4 $R/rtc-turn-0.20.4 $P/a/
    mv $P/a/webrtc-0.20.4 $P/a/webrtc && mv $P/a/rtc-turn-0.20.4 $P/a/rtc-turn
    cp -r bridge/vendor/webrtc bridge/vendor/rtc-turn $P/b/
    (cd $P && diff -ruN -x target -x .cargo-ok -x codecov.yml -x 'LICENSE-*' a b) > bridge/vendor/B-readiness.patch
