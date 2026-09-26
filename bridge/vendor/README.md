# Vendored crate: webrtc 0.20.4, patched (#166)

The bridge builds `webrtc` from here instead of crates.io, through the
`[patch.crates-io]` entry at the end of `bridge/Cargo.toml`:

| crate | upstream | crates.io checksum |
| --- | --- | --- |
| `webrtc/` | [webrtc 0.20.4](https://crates.io/crates/webrtc/0.20.4) (github.com/webrtc-rs/webrtc) | `3daa8f2f6366331ae3275a6c02a855c6fb3faa1d16960498d7daaf61c96e76bd` |

The directory is the published crate as the registry unpacks it, with its
upstream `Cargo.toml`, `Cargo.toml.orig`, `Cargo.lock` and license files, plus
`webrtc-driver-drain.patch`. Nothing else is changed, with one exception:
`webrtc/codecov.yml` is left out. It is upstream's CI configuration and holds
webrtc-rs's Codecov upload token, which is not ours to carry and which
gitleaks rightly flags.

Upstreaming is deferred (Zech on #166): no fork and no upstream PR for now.

## What changed and why

[#166](https://github.com/ZechCodes/build-web/issues/166): output a wake of the
webrtc driver produced could stay queued when the driver went to wait. The
next wake then sent it: an inbound packet, or, on an idle connection, ICE's
200 ms check timer. Two cases were measured:

- **Idle replies over a TURN relay.** Relay/relay round trips were p50 98 ms,
  max 202 ms. The driver drained the TURN client (1.b) before the core (1.c)
  and not after, so the core's relayed reply sat in the TURN client's queue.
- **The SCTP INIT after the DTLS handshake, on both paths.** When the core's
  events are pumped (2.c), the completed handshake reaches its SCTP handler,
  which queues the INIT. That happens after the write phase has run, and
  nothing woke the driver to send it for up to about 170 ms.

`webrtc-driver-drain.patch` changes `peer_connection/driver.rs` only:

- `poll_writes()` drains the TURN client on both sides of the core: gatherer
  (1.a), TURN client (1.b), core (1.c), TURN client again (1.d). The TURN
  client both makes traffic of its own and carries the core's. Its own
  (allocations, permissions, refreshes, retransmits) leaves first, as upstream
  has it, so a core send waiting on a full socket never holds it. The core's
  relayed writes, which 1.c hands it, leave in 1.d, in the same drain. The
  write-flush gate is cleared at the start of every write phase.
- `poll_pass()` runs writes, events, reads and then writes again, so a wake
  ends only after sending what its own events and reads produced. The first
  write phase still sends what woke the driver before any callback runs. rtc
  0.20.4 has no query for output the core holds, short of draining it, and
  pumping its events can queue output even when no event comes out, so the
  closing drain is the check.
- When relayed input reached the core (3.a), the wake takes one more round of
  events, reads and writes (`MAX_EXTRA_ROUNDS` = 1). That input is the one
  thing handed to the core after its events were pumped, and what it raises,
  such as the handshake completing on the last relayed flight, becomes output
  only when 2.c pumps it. One round is all a wake can use. A second would need
  the extra round to feed the core again, and relayed input reaches the core
  only from datagrams the TURN client read in `select!`.
- Five driver tests:
  - `relayed_output::a_relayed_core_write_leaves_in_the_same_pass`
  - `relayed_output::a_packet_held_for_a_permission_leaves_in_the_pass_that_takes_the_grant`
  - `relayed_output::a_blocked_host_send_does_not_hold_ready_turn_control`
  - `relayed_output::the_sctp_init_after_a_relayed_handshake_leaves_before_the_driver_waits`
  - `direct_output::the_sctp_init_leaves_in_the_pass_that_completes_the_handshake`

  The first three run against a mock TURN server on a real UDP socket; the
  third queues a real permission retransmit beside a core ICE check on a host
  socket whose sends stay pending. The two handshake tests run a real DTLS
  handshake against an `rtc` core, over a socket or through that mock TURN
  server.

Each part of the change has a test that fails without it:

| part switched off | tests that fail |
| --- | --- |
| the closing write phase | the permission grant, both SCTP INIT tests |
| the extra round | the relayed SCTP INIT |
| the TURN drain before the core (1.b) | the blocked host send |
| the TURN drain after the core (1.d) | the relayed SCTP INIT |

The measurement harness is `bridge/experiments/166/` at commit `1ca86df4`, and
the numbers are on #166.

## Tests

The bridge's own gates never reach in here. The crate is not a member of the
bridge's workspace (the bridge has no `[workspace]` table, so it is the only
member), so `cargo fmt --all`, `cargo clippy` and `cargo test --all` in
`bridge/` stop at the bridge. The crate is tested through the small workspace
in this directory, which has its own lockfile. From `bridge/`:

    nice -n 10 cargo test --locked --manifest-path vendor/Cargo.toml --lib

That runs the crate's unit tests, the driver tests included. webrtc's
integration tests (`tests/`) are left out: `play_save_disk` needs media files
the published crate does not ship.

## Re-deriving from the registry

From the repo root, with the crate in the local registry (a `cargo fetch` in
`bridge/` before this change, or any build of the unpatched crate):

    R=~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f
    V=$(mktemp -d)
    cp -r $R/webrtc-0.20.4 $V/webrtc
    rm -f "${V:?}"/webrtc/.cargo-ok "${V:?}"/webrtc/codecov.yml
    (cd $V && patch -p1 < "$OLDPWD/bridge/vendor/webrtc-driver-drain.patch")
    diff -r -x target $V/webrtc bridge/vendor/webrtc

No output means the vendored tree is exactly the registry source plus the
patch. To move to a new upstream version, copy the new crate over `webrtc/`,
apply the patch (fix it where it no longer applies), and update the checksum
above and `bridge/Cargo.lock`. The crate's own `.gitignore` and the global
gitignore drop a few upstream files (`webrtc/Cargo.lock`, `webrtc/.vscode/`),
so stage them with `git add -f`.

After editing the vendored source, regenerate the patch against a fresh
registry copy:

    P=$(mktemp -d); mkdir $P/a $P/b
    cp -r $R/webrtc-0.20.4 $P/a/webrtc
    rm -f "${P:?}"/a/webrtc/.cargo-ok "${P:?}"/a/webrtc/codecov.yml
    cp -r bridge/vendor/webrtc $P/b/webrtc
    (cd $P && git -c diff.noprefix=true diff --no-index a/webrtc b/webrtc) > bridge/vendor/webrtc-driver-drain.patch
