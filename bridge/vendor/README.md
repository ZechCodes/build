# Vendored crates: webrtc and rtc 0.20.4, patched (#166, #179)

The bridge builds `webrtc` and `rtc` from here instead of crates.io, through
the `[patch.crates-io]` entries at the end of `bridge/Cargo.toml`:

| crate | upstream | crates.io checksum | patch |
| --- | --- | --- | --- |
| `webrtc/` | [webrtc 0.20.4](https://crates.io/crates/webrtc/0.20.4) (github.com/webrtc-rs/webrtc) | `3daa8f2f6366331ae3275a6c02a855c6fb3faa1d16960498d7daaf61c96e76bd` | `webrtc-driver-drain.patch` |
| `rtc/` | [rtc 0.20.4](https://crates.io/crates/rtc/0.20.4) (github.com/webrtc-rs/rtc) | `c9005c36795ad076abd36db3ea9ae0275a60395944647d58c1f2bc3e118dddba` | `rtc-dtls-client-hello.patch` |

`webrtc` is the async driver. `rtc` is the sans-I/O peer connection it drives
(ICE, DTLS, SCTP), which the bridge also uses directly. The other `rtc-*`
crates still come from crates.io.

Each directory is the published crate as the registry unpacks it, with its
upstream `Cargo.toml`, `Cargo.toml.orig`, `Cargo.lock` and license files, plus
its patch. Nothing else is changed, apart from these omissions:

- `webrtc/codecov.yml` and `rtc/codecov.yml`: upstream's CI configuration,
  holding webrtc-rs's Codecov upload token, which is not ours to carry and
  which gitleaks rightly flags.
- `rtc/examples/test-data/`: 6.9 MB of media samples that only the example
  binaries and two integration tests read.
- `rtc/tests/testdata/rsa_2048_{answerer,offerer}_key.pem`: RSA keys for one
  integration test. The repo ignores `*.pem` and keeps private keys out.

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

[#179](https://github.com/ZechCodes/build-web/issues/179): the DTLS ClientHello
waited for ICE's next 200 ms check tick. The bridge answers, so it is the DTLS
client, and its core starts the handshake when ICE selects a pair: pumping the
core's events (2.c) hands the selected pair to the DTLS handler, which calls
`connect()`. `connect()` queues the ClientHello in the DTLS endpoint, and the
handler left it there. Only the handler's `handle_timeout` moved that queue
out, and the next timeout was ICE's tick. That cost 0–200 ms on every setup,
host or relayed.

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
- Six driver tests:
  - `relayed_output::a_relayed_core_write_leaves_in_the_same_pass`
  - `relayed_output::a_packet_held_for_a_permission_leaves_in_the_pass_that_takes_the_grant`
  - `relayed_output::a_blocked_host_send_does_not_hold_ready_turn_control`
  - `relayed_output::the_sctp_init_after_a_relayed_handshake_leaves_before_the_driver_waits`
  - `direct_output::the_sctp_init_leaves_in_the_pass_that_completes_the_handshake`
  - `direct_output::the_client_hello_leaves_in_the_pass_that_selects_the_pair`
    (#179)

  The first three run against a mock TURN server on a real UDP socket; the
  third queues a real permission retransmit beside a core ICE check on a host
  socket whose sends stay pending. The handshake tests run a real ICE and
  DTLS exchange against an `rtc` core, over a socket or through that mock
  TURN server. The ClientHello test has that core offer, as a browser does,
  so the driver's core answers, is ICE-controlled and is the DTLS client, as
  in the bridge.

`rtc-dtls-client-hello.patch` changes `peer_connection/handler/dtls.rs` only:

- The DTLS handler queues what the endpoint has to send right after
  `connect()`, so the ClientHello leaves in the driver pass that selected the
  pair, in its closing write phase. The four places that move the endpoint's
  queue out (read, write, timeout and now connect) share one helper,
  `DtlsHandlerContext::queue_transmits()`.

Each part of the changes has a test that fails without it:

| part switched off | tests that fail |
| --- | --- |
| the closing write phase | the permission grant, both SCTP INIT tests, the ClientHello |
| the extra round | the relayed SCTP INIT |
| the TURN drain before the core (1.b) | the blocked host send |
| the TURN drain after the core (1.d) | the relayed SCTP INIT |
| rtc: queueing the ClientHello after `connect()` | the ClientHello |

The measurement harness for #166 is `bridge/experiments/166/` at commit
`1ca86df4`. The numbers for both changes are on their tasks.

## Tests

The bridge's own gates never reach in here. The crates are not members of
the bridge's workspace (the bridge has no `[workspace]` table, so it is the
only member), so `cargo fmt --all`, `cargo clippy` and `cargo test --all` in
`bridge/` stop at the bridge. `webrtc` is tested through the small workspace
in this directory. It has its own lockfile and patches `rtc` to `rtc/`, as
the bridge does. From `bridge/`:

    nice -n 10 cargo test --locked --manifest-path vendor/Cargo.toml --lib

That runs webrtc's unit tests, the driver tests included, on the patched rtc.
webrtc's integration tests (`tests/`) are left out: `play_save_disk` needs
media files the published crate does not ship.

`rtc` is not a member of that workspace: its tests' dev-dependencies include
webrtc 0.14 and a web server stack, which the bridge has no use for. Its
change is covered by the ClientHello driver test above. Its own unit tests run
from a copy outside this tree, on its upstream lockfile (this fetches the
dev-dependencies once):

    S=$(mktemp -d -p /var/tmp); cp -r bridge/vendor/rtc $S/rtc
    (cd $S/rtc && CARGO_TARGET_DIR=$S/target nice -n 10 cargo test --locked --lib)

## Re-deriving from the registry

From the repo root, with the crates in the local registry (a `cargo fetch` in
`bridge/` before these changes, or any build of the unpatched crates):

    R=~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f
    pristine() { # crate, destination: the registry copy minus the omissions above
      cp -r "$R/$1-0.20.4" "$2"
      rm -rf "${2:?}"/.cargo-ok "${2:?}"/codecov.yml "${2:?}"/examples/test-data \
        "${2:?}"/tests/testdata/rsa_2048_answerer_key.pem \
        "${2:?}"/tests/testdata/rsa_2048_offerer_key.pem
    }
    V=$(mktemp -d)
    pristine webrtc $V/webrtc; pristine rtc $V/rtc
    (cd $V && patch -p1 < "$OLDPWD/bridge/vendor/webrtc-driver-drain.patch")
    (cd $V && patch -p1 < "$OLDPWD/bridge/vendor/rtc-dtls-client-hello.patch")
    diff -r -x target $V/webrtc bridge/vendor/webrtc
    diff -r -x target $V/rtc bridge/vendor/rtc

No output means each vendored tree is exactly the registry source plus its
patch. To move to a new upstream version, copy the new crate over its
directory, apply the patch (fix it where it no longer applies), and update the
checksum above and `bridge/Cargo.lock`. The crates' own `.gitignore` files and
the global gitignore drop a few upstream files (`Cargo.lock`,
`webrtc/.vscode/`), so stage them with `git add -f`.

After editing the vendored source, regenerate its patch against a fresh
registry copy. For rtc (webrtc's is the same with its names):

    P=$(mktemp -d); mkdir $P/a $P/b
    pristine rtc $P/a/rtc
    cp -r bridge/vendor/rtc $P/b/rtc
    (cd $P && git -c diff.noprefix=true diff --no-index a/rtc b/rtc) > bridge/vendor/rtc-dtls-client-hello.patch
