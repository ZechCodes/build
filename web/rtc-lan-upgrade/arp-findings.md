# #377: passive ARP investigation

Part 1 findings were measured on main `98df30c0` with Chromium
152.0.7977.82 and Linux 7.2.5-3-omarchy on 2026-10-05. The production
bridge, vendored transport and SPA were unchanged for those measurements.
Part 2 implementation and verification are in progress below.

## What holds

A cold on-link browser sends ARP for the bridge before its first host STUN
check. Linux learns its address and MAC as a STALE neighbour on the advertised
socket's owning interface. That state is already usable by #374. Suppressing
scout-generated ARP still lets the existing bridge send a real-port indication,
authenticate the peer and select direct before any encrypted application RPC.

Five valid cold runs (`cold-3` through `cold-6`, `final-cold`) produced:

| Event | After the browser receives the first bridge answer |
| --- | ---: |
| Phone broadcast ARP request at bridge ingress | 2.1–2.9 ms |
| First `ip neigh` observation of STALE phone entry | 8.3–26.4 ms |
| First real host-socket indication to phone | 271.5–309.4 ms |
| First matching integrity-bearing STUN request/success exchange | 490.8–505.2 ms |
| Bridge selected-pair `prflx_followed` observation | 553–570 ms |

Each sends two real indications total, to the gateway and phone, uses zero
restarts, and carries both full encrypted pulls directly. Capture timestamps
come from `SO_TIMESTAMPNS_NEW`; answer timestamps come from browser `Date.now()`
without awaiting a measurement IPC. Neighbour snapshot completion gives an
observation upper bound, with roughly 20 ms sampling plus command duration.
The timestamped monitor independently records the STALE transition.

The first incoming check precedes the indication and is dropped by the bridge's
inbound IP firewall. A later request and matching integrity-bearing response
provide observable protocol evidence of connectivity. The packet decoder checks
attribute framing and presence, not the cryptographic MAC. Authenticated
host/prflx and carrying encrypted RPCs are also checked through the unchanged
transport and Chromium stats. `prflx_followed` is selected-pair recognition,
not an exact timestamp of the first validated incoming check.

## Where it does not hold

All times below are answer-relative. UDP9 scouts are suppressed except in the
active control. The real bridge and production single optional restart remain
enabled.

| Control | ARP / neighbour result | Connectivity result |
| --- | --- | --- |
| Phone has bridge MAC cached as PERMANENT; bridge lacks phone (3 runs) | No phone ARP, no phone neighbour, no phone indication | TURN through 26 s, no successful private STUN exchange, one optional restart |
| Phone has REACHABLE bridge MAC; bridge lacks phone (1 run) | No phone ARP in the observation; cache later becomes STALE/DELAY | TURN through 26 s, one optional restart |
| Phone has STALE bridge MAC; bridge lacks phone (2 runs) | Unicast refresh at 5.35/5.42 s; bridge learns phone; indication at 5.39/5.49 s | First successful exchange at 6.93/6.94 s, browser first observes a succeeded direct pair at 7.48 s; existing restart selects direct around 21.94 s |
| Phone routes checks via gateway, as the old unknown fixture does (2 runs) | No phone ARP addressed to bridge; phone neighbour remains absent | TURN through 26 s, one optional restart |
| Cold on-link phone, production scouts unsuppressed (2 runs) | Own ARP at 2.3/2.6 ms, then 39/40 successful scout enqueues and 39/40 bridge ARP requests | Exchange around 498 ms, direct before application RPCs, zero restarts |

PERMANENT is an intentional no-refresh control, not a claim about a phone's
normal cache. The REACHABLE and STALE controls demonstrate why a cached MAC
alone is not enough to predict timing. No neighbour is flushed. Initial cache
asymmetry is verified after signaling opens but before ICE negotiation begins.
The browser waits for the runner's firewall-ready barrier before starting ICE.

In suppressed cases the namespace OUTPUT rule rejects UDP9 before ARP; library
`scout_datagrams_sent` stays zero, but scheduler attempts are nonzero (40–47 in
the initial four cold runs). This proves discovery without scout-generated ARP,
not a new production no-scout policy. Active controls emit no decoded UDP9
payload to unknown destinations either: unresolved UDP enqueues generate ARP
first. The aggregate ARP capture and scout enqueue counters expose their cost.

The negative observation spans 26 s from candidate gathering, including the
production restart around 20 s. It is not a measurement of a single generation
surviving to its 25 s expiry; its credentials change on restart as before.

## Why #374's unknown far-edge phone took 11.6–19 s

`run.py` originally added a phone-side `10.72.0.1/32 via 10.72.0.254` route
in the `unknown-neighbor-*` cases, now named `never-arps-*`. The gateway
preserves the private UDP tuple
but owns the L2 next hop. The phone ARPs for the gateway, not the bridge;
signaling also uses the gateway rendezvous. That fixture deliberately prevents
the bridge from learning the phone through its own ARP. Its result measures
scout coverage and kernel pressure at the far end of a /22, not ordinary
cold-cache on-link host-check timing. The suppressed `arp-proxy` controls
reproduce this asymmetry.

The cold packet and neighbour traces rule out a wrong receiving interface or
rejection of STALE state in these runs. They do not establish how frequently
this fast path occurs on physical Wi-Fi phones or APs.

## Part 2 decision and red baselines

The approved part 2 design takes an owning-interface usable-neighbor baseline
before the bridge sends its answer, then polls the existing bounded, read-only
snapshot every 20 ms through the initial 250 ms and every 100 ms afterward. A
newly usable neighbor can receive a credential-free real-port indication during
that initial grace; at most eight distinct addresses per ICE generation qualify,
shared across candidate ports. This preserves the existing snapshot path and
does not add a netlink notification socket. The pre-answer baseline matters:
the browser's ARP can precede its unresolved `.local` candidate trickle. The
baseline's owning-interface set is fixed for that generation; a later owner
change cannot add an unobserved interface to the early path.

A successful early indication holds an **unstarted** scout pass until one second
after the latest successful early indication.
An authenticated inbound STUN check from the exact host/peer-reflexive tuple
can prevent scout start while TURN is still selected, while leaving the
existing unresolved evidence available to the optional restart. Without that
proof, the original
scout fallback begins. A later STALE neighbor can move ahead of ordinary
cluster order, but cannot pause a pass already scouting. Early observations
never claim the process-wide 60-second interface lease; only the first
successful scout enqueue does. All original #374 NAT, socket, subnet, packet,
pressure, credential and cancellation checks still gate sends. The 25-second
plan lifetime, single optional restart and paired LAN hint cache are outside
this change. The early path needs no firewall change, privileged socket,
media permission or TURN application-data inference.

The first production red controls, retained in `/tmp/task377-part2/`, use the
unmodified discovery behavior from main. They establish the regression target;
they are not green evidence for the new design. The PERMANENT and routed runs
retain `failure.json` from obsolete fixture expectations of a TURN-only outcome;
their measured selected paths, packet captures and restart counts show direct
after one restart. The REACHABLE and STALE baseline fixtures passed their
updated assertions.

| Red control | Observed outcome | Scout/ARP cost |
| --- | --- | ---: |
| Cold on-link, active scouts | Direct before application RPC, zero restarts, but zero-scout assertion fails | 38 successful scout enqueues; 38 bridge ARP requests (the preceding part 1 control saw 39/40) |
| Browser bridge-MAC cache PERMANENT, bridge lacks browser | First nominated host/host at 21.981 s after gathering, one existing restart | 991 successful scout enqueues, 3,002 scout attempts, 3,058 ARP requests, 1,020 unique targets |
| Browser bridge-MAC cache REACHABLE, bridge lacks browser | First nominated host/host at 12.854 s after gathering, one existing restart | 1,020 successful scout enqueues, 1,709 scout attempts, 3,058 ARP requests, 1,020 unique targets |
| Browser bridge-MAC cache STALE, bridge lacks browser | First nominated host/host at 7.566 s after gathering, one existing restart | 1,019 successful scout enqueues, 1,927 scout attempts, 3,058 ARP requests, 1,020 unique targets |
| Gateway-routed browser (formerly `unknown-neighbor-*`) | First nominated host/host at 13.453 s after gathering, one existing restart | 1,020 successful scout enqueues, 1,757 scout attempts, 3,058 ARP requests, 1,020 unique targets |

The PERMANENT cache is a deliberate no-refresh control, not a model of a
physical phone. The routed fixture must remain a full-coverage fallback
control after its clearer `never-arps-*` rename. In the old STALE production
run, the browser's unicast ARP refresh made its address available around 5.4 s;
the existing sweep sent a real indication about 20.6 ms after that ARP. Direct
selection still waited for the existing restart. This trace supports prompt
usable-neighbor probing in the old code and does not by itself show a benefit
from the new late-neighbor priority.

### Final-source cold on-link runs

Three independent runs with the final rebuilt source (`arp-cold-1` through
`arp-cold-3` under `/tmp/task377-part2/final/`) passed the production fixture.
Each selected host/host direct with zero restarts and two encrypted pulls over
direct. Each recorded one successful early real-port indication, one scout
hold, zero scout starts/enqueues and zero bridge-originated ARP requests. The
browser's own ARP request remained observable.

| Event | After the browser receives the first bridge answer |
| --- | ---: |
| Browser ARP request at bridge ingress | 2.9–3.8 ms |
| First real host-socket indication to browser | 44.9–71.3 ms |
| First matching integrity-bearing STUN request/success exchange | 306.8–317.4 ms |

The one-second hold covered these authenticated exchanges; first nominated
host/host selection followed 0.381–0.439 s after candidate gathering.

### Final-source fallback and NAT controls

The full final-source fixture matrix passed 12/12 modes, with each process exit
code checked (`/tmp/task377-part2/final-matrix.log`). The original old/final
fallback runs and the planned paired repetitions are summarized in
`/tmp/task377-part2/part2-fixture-evidence.json`. Time is from the first browser
host candidate gathering to the first nominated host/host browser snapshot,
using the same rule in both source versions; browser nomination was sampled
about every 50 ms. The counts are successful direct runs / attempted runs.

| Condition | Old direct runs and time | Final direct runs and time | Successful scout enqueues, old → final |
| --- | ---: | ---: | ---: |
| Bridge MAC cached PERMANENT at browser | 3/3, 13.451–21.981 s | 3/3, 12.839–21.977 s | 991–1,020 → 1,020 |
| Bridge MAC cached REACHABLE at browser | 3/3, 12.854–21.983 s | 3/3, 21.966–21.969 s | 999–1,020 → 973–1,020 |
| Bridge MAC cached STALE at browser | 3/3, 6.466–7.566 s | 3/3, 7.475–7.569 s | 998–1,019 → 997–1,019 |
| Browser routes checks via gateway (`never-arps`) | 1/2, 13.453 s | 3/3, 14.466–22.017 s | 1,020 → 1,020 |

Every successful fallback run selected direct after one existing restart and
produced 3,058 bridge ARP requests, including kernel retries, to 1,020 unique
destinations. The planned second old-code routed run did not reach the browser
by the 26-second observation boundary: it stayed on TURN after one restart,
with 1,007 of 1,020 scout destinations and 3,021 bridge ARP requests. Its next
old-code repetition was skipped without retrying the failure. The 25-second
plan lifetime and pressure guards therefore do not guarantee full coverage in
every run, either before or after this change.

The PERMANENT-cache old/final order reversed across repetitions, and the
observed direct-time ranges overlap in the cached conditions. There is no
consistent timing ordering in these samples. These small
samples show preserved fallback behavior in the successful final runs, but do
not establish statistical timing equivalence or a stable speed change. The
old-code routed failure remains part of the sample tally.

The STALE final run sent its real indication 89.6 ms after the browser's
unicast ARP refresh, compared with 20.6 ms in the old-code run. The 100 ms
later polling phase and scheduling variance explain why that one trace cannot
support a claim of faster late-neighbor probing. The cached and routed cases
confirm that the no-hit scout fallback remains available and the existing
single restart still selects direct.

The `different-nat` and `missing-srflx` modes each produced a cold browser ARP
and a fresh usable neighbor, yet all feature probe/scout counters stayed zero,
there was no non-gateway bridge ARP, and no optional restart. The three routed
`never-arps-unresolved`, `never-arps-clustered` and `never-arps-pressure` controls
also passed. The pressure run completed all 1,020 scout enqueues and destinations
with 3,058 ARP requests to 1,020 unique targets, peaked at 256 pending
neighbors, and selected direct without a restart. Separate socket-pressure
and scout-pressure controls passed; the scout-pressure fixture held at most five
scout sockets, peaked at 256 pending neighbors, kept the ordinary ICE socket's
send queue empty and closed its temporary sockets.

These are Chromium/Linux namespace results. Physical Android/iOS phones and
Wi-Fi access points have not been measured.

## Platform evidence and cleaner alternatives

- Linux's target-local ARP-request path calls `neigh_event_ns`, which learns a
  STALE sender entry subject to `arp_ignore`/`arp_filter`. Unicast replies can
  instead mark an entry REACHABLE. [Linux ARP source](https://github.com/torvalds/linux/blob/v6.17/net/ipv4/arp.c),
  [neighbour implementation](https://github.com/torvalds/linux/blob/v6.17/net/core/neighbour.c),
  [ARP sysctls](https://docs.kernel.org/networking/ip-sysctl.html).
- Android uses Linux-derived kernels with vendor components. Apple's published
  XNU ARP implementation likewise learns senders for local-target requests and
  supports unicast refresh of existing entries. These sources do not establish
  browser-check/cache timing, Wi-Fi offload or AP proxy behaviour on a real
  Android/iOS device; no physical phone was tested here. Private-MAC rotation
  is not the address-discovery key: fresh kernel IP/MAC state is consumed and
  ICE still authenticates the peer. [Android kernel overview](https://source.android.com/docs/core/architecture/kernel),
  [Apple XNU ARP source](https://github.com/apple-oss-distributions/xnu/blob/main/bsd/netinet/in_arp.c).
- Read-only `RTM_NEWNEIGH` notifications can wake the driver promptly, with a
  fresh owning-interface snapshot retaining the existing authorization. This
  could replace polling latency but cannot invent a missing phone IP; given
  the measured margin, existing bounded snapshots may be enough. mDNS remains
  exact discovery where supported. [Neighbour netlink specification](https://docs.kernel.org/networking/netlink_spec/rt_neigh.html).
- Srflx related addresses are not a dependable LAN-IP channel: libwebrtc's mDNS
  host-obfuscation test expects those related fields to be cleared. No additional universally reliable
  unknown-IP mechanism was found within the stated constraints.
  [libwebrtc allocator tests](https://chromium.googlesource.com/external/webrtc/+/master/p2p/client/basic_port_allocator_unittest.cc).
- The bridge already forms compatible srflx ICE pairs
  (`bridge/vendor/rtc-ice/src/agent/mod.rs`, `add_pair`) and sends checks from
  the underlying host base (`ping_candidate`), as ICE specifies. A router that
  supports hairpinning can make those ordinary pairs work. At the bridge host,
  outbound host:port → public-srflx:port state expects the reverse public tuple;
  a private-phone:port → host:port check has a different source and does not
  match it. A correctly translated hairpinned reply can match the public tuple.
  Same-public-IP equality cannot ensure the router's mapping/filter/hairpin
  behaviour. [ICE](https://datatracker.ietf.org/doc/html/rfc8445#section-6.1.2.4),
  [UDP NAT hairpinning](https://datatracker.ietf.org/doc/html/rfc4787#section-6),
  [conntrack tuples](https://docs.kernel.org/netlink/specs/conntrack.html).

## Verification and artifacts

The part 1 helper tests were written first and failed before implementation;
all 21 namespace, cache-control and packet-decoder tests passed. The expanded
part 2 fixture helper suite passes 24 tests. Valid observations,
including final cold/cached checks, are separate runs, not hidden retries.
The existing early-unresolved and unknown-neighbour-clustered fixture controls
are checked with the shared browser module. Gates run niced with inherited
`BRIDGE_*` removed; every bridge test process uses `env -i`, temporary HOME and
temporary identity. Semgrep and gitleaks gate the findings branch.

Raw two-sided PCAPs, decoded JSONL, timestamped `ip monitor neigh`, snapshot
transitions, browser stats, NAT evidence, firewall counters and bridge logs are
retained under `/tmp/task377/`. Representative evidence is attached to #377 as
text, including raw PCAP hex that can be restored with `xxd -r -p` after
extracting its marked block. Two calibration runs are retained but excluded:
`cold-1` awaited measurement IPC and lost initial remote candidates;
`cold-2` asserted cache absence after negotiation had already learned the
phone. The corrected observer neither delays signaling nor requires the
phone to stay absent after it sends legitimate ARP.
