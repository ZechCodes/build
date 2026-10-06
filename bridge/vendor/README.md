# Vendored crates: webrtc, rtc and rtc-ice 0.20.4, patched (#166, #179, #298, #372, #374)

The bridge builds `webrtc`, `rtc` and `rtc-ice` from here instead of crates.io, through
the `[patch.crates-io]` entries at the end of `bridge/Cargo.toml`:

| crate | upstream | crates.io checksum | patch |
| --- | --- | --- | --- |
| `webrtc/` | [webrtc 0.20.4](https://crates.io/crates/webrtc/0.20.4) (github.com/webrtc-rs/webrtc) | `3daa8f2f6366331ae3275a6c02a855c6fb3faa1d16960498d7daaf61c96e76bd` | `webrtc-driver-drain.patch`, then `webrtc-negotiated-first-message-test.patch`, `webrtc-late-direct-tests.patch`, `webrtc-host-candidate-sweep.patch` |
| `rtc/` | [rtc 0.20.4](https://crates.io/crates/rtc/0.20.4) (github.com/webrtc-rs/rtc) | `c9005c36795ad076abd36db3ea9ae0275a60395944647d58c1f2bc3e118dddba` | `rtc-dtls-client-hello.patch`, `rtc-negotiated-first-message.patch`, `rtc-late-direct-stats.patch`, `rtc-host-candidate-sweep.patch` |
| `rtc-ice/` | [rtc-ice 0.20.4](https://crates.io/crates/rtc-ice/0.20.4) (github.com/webrtc-rs/rtc) | `2c06eeabd250a7693e1e8b28222b78c4a81a7c6ca7fe3cb99bbdff2f6c0ff0ab` | `rtc-ice-late-direct-checks.patch`, then `rtc-ice-host-candidate-sweep.patch` |

`webrtc` is the async driver. `rtc` is the sans-I/O peer connection it drives
(ICE, DTLS, SCTP), which the bridge also uses directly. `rtc-ice` is its ICE
agent. The other `rtc-*`
crates still come from crates.io.

Each directory is the published crate as the registry unpacks it, with its
upstream `Cargo.toml`, `Cargo.toml.orig`, `Cargo.lock` and license files, plus
its patches. Nothing else is changed, apart from these omissions:

- `webrtc/codecov.yml` and `rtc/codecov.yml`: upstream's CI configuration,
  holding webrtc-rs's Codecov upload token, which is not ours to carry and
  which gitleaks rightly flags.
- `rtc/examples/test-data/`: 6.9 MB of media samples that only the example
  binaries and two integration tests read.
- `rtc/tests/testdata/rsa_2048_{answerer,offerer}_key.pem`: RSA keys for one
  integration test. The repo ignores `*.pem` and keeps private keys out.

Upstreaming is deferred (Zech on #166): no fork and no upstream PR for now.

## What changed and why

[#166](https://github.com/ZechCodes/build/issues/166): output a wake of the
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

[#179](https://github.com/ZechCodes/build/issues/179): the DTLS ClientHello
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

[#298](https://github.com/ZechCodes/build/issues/298): a negotiated channel's
first inbound message could be lost for good. rtc dials the negotiated
channels when it handles `SCTPHandshakeComplete`, and it only handles that
event when the driver pumps events, after the reads. The far end is the SCTP
server, so its association is up when it takes the bridge's COOKIE ECHO, and
its channels can send before the COOKIE ACK reaches the bridge. When the ACK
and that first DATA came in one read burst, the DATA found its channel
registered but not dialed. `DataChannelHandler::handle_read` failed with
`ErrDataChannelNotExisted`, and the message was dropped after SCTP had already
acked it, so nothing would send it again. In CI the QA harness's first `ping`
hung for 30 s. The bridge logged both channels opening with no
`carrier_bound` after them (runs 36743782697 and 36747551226).

`rtc-negotiated-first-message.patch` changes
`peer_connection/handler/datachannel.rs` only:

- A channel still `Connecting` when a message arrives on its stream is opened
  right there, before the message is handled: dialed, announced open and its
  dial output queued. The handshake event then finds it open and leaves it
  alone. Both paths share one function, `open_connecting`.
- One side effect is left as it is. The dial's internal DATA_CHANNEL_OPEN reaches
  the SCTP handler after the peer's DATA has already created the stream, so
  `open_stream` there answers `ErrStreamAlreadyExist`, which the pipeline logs,
  and the stream keeps SCTP's defaults (ordered, reliable) instead of the
  channel's reliability parameters. Both of the bridge's channels are ordered
  and reliable, so nothing changes for them. An unordered or partially
  reliable negotiated channel would need that handler to reuse the stream.

`webrtc-negotiated-first-message-test.patch` applies after
`webrtc-driver-drain.patch` and adds one driver-module test:
`negotiated_channels::the_first_message_in_the_burst_that_completes_the_handshake_is_delivered`.
It drives two `rtc` cores in memory. The far end offers and sends `ping` the
moment its channel accepts a send. The bridge reads everything the far end
wrote, COOKIE ACK and ping together, before it pumps an event, then answers
`pong` on the same channel. Without the rtc change it fails with "the bridge
never delivered the far end's first message".

The measurement harness for #166 is `bridge/experiments/166/` at commit
`1ca86df4`. The numbers for both changes are on their tasks.

#372: a browser host candidate resolved after TURN nomination created a Waiting
pair, but the selected-pair branch only sent consent keepalives. With an inbound
drop firewall, the bridge never sent the direct STUN request needed to open the
return path.

`rtc-ice-late-direct-checks.patch` adds opt-in pending direct checks to both full
ICE selectors after selection. `SettingEngine::set_check_pending_direct_pairs`
enables them in the bridge; it defaults to false. ICE-lite is unchanged. Only
Waiting/InProgress pairs with neither endpoint relayed are checked, using the
normal binding-request budget. Succeeded, Failed and nominated pairs are skipped.
Selected-pair keepalives continue and nomination is unchanged: a browser ICE
restart still makes the direct pair carrying.

`rtc-late-direct-stats.patch` carries the setting into `AgentConfig` and syncs
all active pair counters, states and nomination flags into RTC stats, preserving
the application byte counters. Candidate metadata is backfilled from the exact
accepted ICE candidates, including candidates embedded in SDP and candidates
discovered by checks. Earlier-generation pair accumulators and remote candidate
metadata are removed when the ICE agent no longer holds them.
Previously only the selected pair's counters were exposed, so a resolved host
could not be distinguished from an address that actually received checks.

Four ICE unit regressions check outgoing STUN packets, budget exhaustion, success
responses, selection and keepalive preservation, default/lite behavior and relay
exclusion. `webrtc-late-direct-tests.patch` drives two real sans-I/O cores through
TURN nomination before adding a host and verifies Waiting, InProgress, Succeeded
and Failed counters plus generation retirement through public RTC stats.
An SDP-only regression verifies that each checked pair references metadata with
the actual accepted remote candidate ID. A repeated-restart regression verifies
that only the current generation's remote candidate metadata survives.

#374: sampling only when a session ended could report zero checks after ICE
failure had deleted the generation's active candidates and pairs. The ICE agent
now saves one final statistics snapshot immediately before failure cleanup,
including pair counters, states and remote candidate metadata. Operational
candidates and selection are still cleared, and the failed agent schedules no
checks. Statistics retain that generation's evidence until a restart with valid
credentials or close; a rejected restart preserves it. RTC reads the metadata
through the statistics-only accessor, so the first `getStats` after failure can
still join candidates received in SDP or discovered by inbound checks. No
sampling timer or connectivity behavior changes.

ICE lifecycle regressions cover unsampled Checking-to-Failed and
Connected-to-Disconnected-to-Failed transitions, discovered peer-reflexive
metadata, unchanged cleanup, and snapshot retirement on restart and close.
Bridge diagnostic tests additionally verify the public RTC report after failure,
same-socket peer-reflexive evidence for a tracked host, relay exclusion and
success-state evidence without fabricated request or response counts.

#374: unresolved browser host names can leave inbound firewall DROP intact even
while Chromium continues checking the bridge's advertised host candidate.
`rtc-host-candidate-sweep.patch` adds an opt-in `SettingEngine` switch (default
false) and narrow operational ICE candidate/credential accessors. Its async
implementation is `webrtc-host-candidate-sweep.patch`, applied after the earlier
patches. No ICE candidate is fabricated or revived.

The async peer exposes generation-guarded start/cancel/clear operations for an
unresolved candidate port. Candidate ports below 1024 are rejected at bridge
and vendor admission without consuming the 32-port capacity. Ordinary probes
wait 250 ms; #377's newly usable neighbors can be probed before that grace ends.
Both paths emit 28-byte STUN Binding Indications,
each with a fresh random transaction ID and only FINGERPRINT. They contain no
USERNAME, integrity attribute, credential or session identifier and require no
reply. A later authenticated inbound check may create a peer-reflexive candidate
through normal ICE processing; the indication alone proves nothing.

Only an accepted UDP host candidate's existing bound socket is used. Its exact
source IP must have one current owning interface, a contiguous actual IPv4 mask,
and an RFC 1918 or link-local subnet of at most 1024 total addresses. Network,
broadcast and every current local address are excluded. Interface name, index,
address and mask are rechecked before every send. Linux's Tokio socket adapter
uses IP_PKTINFO plus MSG_DONTROUTE and MSG_DONTWAIT on that same socket, so there
is no gateway or source-port fallback. Other platforms and runtimes skip until
an equivalent explicit-interface operation is supplied. A candidate port of
5353 still uses the host socket: probes bypass the driver's mDNS write dispatch.

Real host-socket indications target only neighbors freshly observed in
REACHABLE, STALE or DELAY state. Unknown addresses are discovered through at
most five separate ephemeral UDP sockets process-wide, bound to the same owned
host IP and explicit interface. Scouts send one zero byte to UDP Discard port 9;
the payload has no credential, session marker or reply requirement. Scout-only
SO_ERROR is drained before sends; prior-destination ICMP/unreachable errors get
one paced retry, and persistent errors do not count as successful coverage.
Ordinary ICE socket errors and sends are unchanged.

Both scouts and real indications require a matching IPv4 server-reflexive IP
in the current operational local and remote ICE candidate sets. Only UDP
component 1 srflx candidates qualify. The local candidate's base must match a
currently advertised, live supported host socket; retained locals on that same
base qualify. Relay, peer-reflexive, related-address, IPv6 and historical stats
are not substitutes. Matching IPs are a same-NAT heuristic, not peer identity
or proof that both devices share a LAN. Missing evidence or disjoint IP sets
send no feature packets and remain ineligible while later trickle is checked
inside the original window. Expiry reports `nat-evidence-missing` or
`nat-address-mismatch`. Losing evidence closes active scout sockets immediately.

Scouts and real indications share a process-wide 200-packet/s ceiling. Each
generation admits at most 32 distinct candidate ports and 32768 combined
attempts, inside the original absolute 25-second window. Each subnet has one
discovery cursor reused across ports. A bounded tombstone retaining only the
owned source/interface/mask prevents late same-generation ports from restarting
discovery after retirement. The real indication pass may repeat once, a second
after settlement, only while a relay pair is selected. Settlement requires
successful discovery admissions or known-neighbor exclusions for the entire
approved set, settled ARP outcomes, and indications to every resolved usable
neighbor. Dead neighbors never receive an ICE-socket indication. Incomplete
coverage reports `window-expired`, with successful sends separate from attempts.

Discovery also has one process-wide 60-second window per owning interface,
shared by all aliases, ports, peers and ICE generations. The first successful
scout enqueue atomically stores only the interface index and its start timestamp;
known-neighbor-only work, failed sends and preparation consume no window.
An owner continues without renewing the timestamp. Cancellation, completion,
failure and peer close never refund it. Other work reports
`interface-scout-cooldown` and may send real indications to fresh usable
neighbors, without another discovery pass. The registry keeps at most 64
interface timestamps, prunes only expired entries, and denies new discovery
when full. It retains no neighbor addresses.

Before every scout or real indication, Linux reads TIOCOUTQ/SIOCOUTQ and
SO_SNDBUF. It yields unless queued bytes plus 4096 bytes of bookkeeping fit
within one quarter of that socket's send buffer. Pressure preserves the
current destination and does not extend the window or consume a pass. Ordinary
ICE, DTLS and SCTP output keep their existing path and never enter a new queue.

Read-only RTM_GETNEIGH snapshots refresh every 20 ms through the original
250 ms grace, then every 100 ms, and authorize only exact interface neighbors.
RTM_GETNEIGHTBL supplies actual global ARP-table occupancy
and gc thresholds, including other namespaces; the neighbor dump's INCOMPLETE
count covers the current namespace. Fresh complete dumps and numeric unobserved
send reservations enforce a pending cap of min(256, gc_thresh2 / 2) and a global
occupancy ceiling below 75% of gc_thresh3. Canceled sockets release their file
descriptors before permits; reservations survive until a later dump begun after
the sends accounts for them. Missing, stale, partial or invalid metadata admits
no new scout and stale neighbor observations authorize no real indication.
No neighbor, threshold, route or firewall configuration is changed.

#377 captures an immutable usable-neighbor baseline for the live socket's owning
interface when remote credentials are accepted, before answer creation. At most
32 owners and 1024 usable addresses per owner share one absolute 5 ms read budget.
Failed snapshots and owners introduced later receive no early privilege. At most
eight distinct newly usable interface/address pairs per generation, shared across
ports, are prioritized on real candidate ports under all the same traffic gates.
Only successful early enqueues hold an unstarted scout pass until one second
after the most recent enqueue. Authenticated checks matching an exact successfully
probed host/peer-reflexive tuple prevent scout startup even before direct selection,
without changing unresolved restart eligibility. A started scout pass is not
paused by late neighbors. Zero-scout generations consume no interface lease.
Baseline and early address state are erased on retirement; their numeric counts
survive. Bounded snapshot polling reuses the existing netlink path without adding
a notification socket or subscription lifecycle. Physical iOS/Android/AP timing
is unmeasured.

The initial destination order uses distance to the owned source IP and initial
usable authorized neighbors, with stable numeric ties. This favors nearby DHCP
allocations without adding addresses. All read-only queries in a preparation
share an absolute 5 ms budget and validated byte/message caps. Usable membership
is a bounded active-plan set; raw observations and addresses are erased on
resolution, direct selection, close, restart or the original expiry. Only
numeric eligibility and bounded owned-interface tombstones remain afterward.

The initial pass can race mDNS during checking; a repeat requires a selected
relay pair. Resolution, direct selection, close and restart retire the work.
Synchronous cancellation stays locked through each nonblocking syscall;
generation high-water marks and immutable, privately captured full credentials
reject queued stale commands, including password-only restarts. Clear queues
only a wake, so its delayed notification cannot clear a newer generation.

Events expose only generation totals, fixed statuses/reasons, an aggregate
eligible-unresolved count and whether an authenticated PRFLX tuple
matched a successfully sent probe. No address, port, name or credential is
emitted; observed tuple matching does not establish DNS identity or causation.
Completed/expired eligible unresolved plans remain evidence for the bridge's
single optional fresh-generation upgrade restart, without renewing its budget.
Only a remembered safe-subnet boolean survives address expiry; current NAT
authorization masks/restores the published eligibility with coalesced events.
That observation never rearms an expired packet plan or timer.
`addresses_sent`/`addresses_attempted` count real host-socket probes only.
`scout_datagrams_sent`/`scout_attempted` count successful scout enqueues and paced
attempts; `destinations_scouted` counts unique successful admissions across
candidate ports and excludes known neighbors. `neighbors_pending` and its peak
report conservative current-namespace INCOMPLETE pressure plus unobserved
process reservations. Coalesced progress reasons distinguish `neighbor-pressure`,
`neighbor-snapshot-unavailable` and `scout-socket-limit` pauses. NAT waiting
reasons are also coalesced, and report zero current eligibility.
The additive counts `early_neighbors_probed`, `scout_holds` and `scout_starts`
describe new neighbors reached and whether this generation held or started scouts;
the latter two are zero or one. No event code or wire version changes.

Policy/scheduler regressions live in `host_sweep_tests.rs`; queued-generation
and clear races use real sans-I/O cores in `host_sweep_driver_tests.rs`. A Tokio
UDP test pins the actual advertised source port and interface. The namespace
fixture in `web/rtc-lan-upgrade/` proves the authenticated PRFLX upgrade and
subnet-size skip with real Chromium and encrypted data channels.

#383 recovery regressions cover direct-selected PRFLX followed by a native
credential-changing restart onto relay: fresh sweep counters start at zero,
old tuple evidence is retired, and the recovery candidate port can follow a
fresh probe. The native test exchanges ICE, DTLS and SCTP between two cores.

The ignored Linux real-socket regression uses a disposable /22 namespace with
unanswered neighbors. It requires an ordinary ICE-sized send and a 65507-byte
GSO batch to poll immediately ready on the same bound port after probe pressure.
65507 is the IPv4 UDP payload ceiling; the larger generic batching cap produces
an immediate size error rather than socket pressure. The test requires actual
GSO support and reports numeric queue/buffer occupancy and send timings. From
the repo root, build the unit-test ELF, then pass its printed path to the runner:

    nice -n 10 cargo test --locked --manifest-path bridge/vendor/Cargo.toml -p webrtc --lib --no-run
    nice -n 10 python web/rtc-lan-upgrade/socket-pressure.py bridge/vendor/target/debug/deps/webrtc-<hash>
    nice -n 10 python web/rtc-lan-upgrade/socket-pressure.py --scout bridge/vendor/target/debug/deps/webrtc-<hash>

The scout mode runs the actual discovery pool while ordinary sends use the
advertised ICE socket. Its `--scout-red` control deliberately saturates only that
fixture's host socket to prove the immediate-write assertion detects pressure.

The runner creates private user/network namespaces and veth interfaces, starts
only a private UDP receiver, and never starts a production bridge or changes the
host firewall. Removing the probe headroom gate from an isolated source copy
makes the ordinary send poll pending, providing the red control.

## Tests

The bridge's own gates never reach in here. The crates are not members of
the bridge's workspace (the bridge has no `[workspace]` table, so it is the
only member), so `cargo fmt --all`, `cargo clippy` and `cargo test --all` in
`bridge/` stop at the bridge. `webrtc` is tested through the small workspace
in this directory. It has its own lockfile and patches `rtc` to `rtc/`, as
the bridge does. From `bridge/`:

    nice -n 10 cargo test --locked --manifest-path vendor/Cargo.toml --lib

That runs webrtc's and rtc-ice's unit tests, the driver tests included, on the patched rtc.
webrtc's integration tests (`tests/`) are left out: `play_save_disk` needs
media files the published crate does not ship.

`rtc` is not a member of that workspace: its tests' dev-dependencies include
webrtc 0.14 and a web server stack, which the bridge has no use for. Its
changes are covered by the ClientHello and negotiated-channel driver tests
above. Its own unit tests run from a copy outside this tree, on its upstream lockfile (this fetches the
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
    pristine rtc-ice $V/rtc-ice
    for p in webrtc-driver-drain webrtc-negotiated-first-message-test \
        rtc-dtls-client-hello rtc-negotiated-first-message \
        rtc-ice-late-direct-checks rtc-late-direct-stats webrtc-late-direct-tests \
        rtc-ice-host-candidate-sweep rtc-host-candidate-sweep webrtc-host-candidate-sweep; do
      (cd $V && patch -p1 < "$OLDPWD/bridge/vendor/$p.patch")
    done
    diff -r -x target $V/webrtc bridge/vendor/webrtc
    diff -r -x target $V/rtc bridge/vendor/rtc
    diff -r -x target $V/rtc-ice bridge/vendor/rtc-ice

No output means each vendored tree is exactly the registry source plus its
patches. To move to a new upstream version, copy the new crate over its
directory, apply the patches in the order above (fix them where they no longer apply), and update the
checksum above and `bridge/Cargo.lock`. The crates' own `.gitignore` files and
the global gitignore drop a few upstream files (`Cargo.lock`,
`webrtc/.vscode/`), so stage them with `git add -f`.

After editing the vendored source, regenerate the patch that carries the
change. Each patch covers its own files, so diff only those. For rtc, whose two
patches touch different files:

    P=$(mktemp -d); mkdir $P/a $P/b
    pristine rtc $P/a/rtc
    cp -r bridge/vendor/rtc $P/b/rtc
    f=src/peer_connection/handler/dtls.rs
    (cd $P && git -c diff.noprefix=true diff --no-index a/rtc/$f b/rtc/$f) > bridge/vendor/rtc-dtls-client-hello.patch
    f=src/peer_connection/handler/datachannel.rs
    (cd $P && git -c diff.noprefix=true diff --no-index a/rtc/$f b/rtc/$f) > bridge/vendor/rtc-negotiated-first-message.patch

Both webrtc patches touch `src/peer_connection/driver.rs`. The test patch is
the diff to the vendored file from the pristine crate with
`webrtc-driver-drain.patch` applied; the drain patch is the diff from the
pristine crate to that intermediate copy.

The #372 patches touch different files from those earlier patches. Re-derive
`rtc-ice-late-direct-checks.patch` against pristine `rtc-ice` for
`src/agent/{agent_config.rs,agent_selector.rs,mod.rs,agent_proto.rs,agent_stats.rs,late_direct_test.rs}`;
`rtc-late-direct-stats.patch` against pristine `rtc` for
`src/peer_connection/{configuration/setting_engine.rs,internal.rs}` and
`src/statistics/accumulator/mod.rs`; and `webrtc-late-direct-tests.patch` against
pristine `webrtc` for `src/peer_connection/{mod.rs,late_direct_tests.rs}`. New test
files use an empty source file in the comparison directory. Include each exact
file in the diff, preserving the `a/<crate>/` and `b/<crate>/` paths so `patch -p1`
can apply it.


The #374/#377 conntrack patches are layered diffs against the trees after all
earlier patches are applied. `rtc-ice-host-candidate-sweep.patch` touches
`src/agent/{mod.rs,late_direct_test.rs}` and adds the allocation-free authenticated
operational peer-reflexive pair iterator, with regression coverage.
`rtc-host-candidate-sweep.patch` touches
`src/peer_connection/{mod.rs,configuration/setting_engine.rs}`.
`webrtc-host-candidate-sweep.patch` touches `Cargo.toml`, `Cargo.toml.orig`,
`src/peer_connection/{mod.rs,driver.rs,host_sweep.rs,host_sweep_tests.rs,host_sweep_driver_tests.rs,host_sweep_nat.rs,host_neighbors.rs,host_neighbor_table.rs,host_scout.rs,host_scout_socket_tests.rs}`
and `src/runtime/{mod.rs,tokio.rs,host_egress.rs}`. The Linux-only libc dependency
is already in the dependency graph; the workspace lockfile records it as a direct
webrtc dependency. Regenerate each patch from its pre-conntrack tree rather than
pristine source, using the same `a/<crate>/` and `b/<crate>/` paths above. A full
registry reconstruction, including all three conntrack patches, must match all three final
vendored trees exactly.
