# #377: passive ARP investigation

Findings only, measured on main `98df30c0` with Chromium
152.0.7977.82 and Linux 7.2.5-3-omarchy on 2026-10-05. Production bridge,
vendored transport and SPA code are unchanged. Part 2 awaits the assigner's
decision.

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

`run.py` explicitly adds a phone-side `10.72.0.1/32 via 10.72.0.254` route in
the `unknown-neighbor-*` cases. The gateway preserves the private UDP tuple
but owns the L2 next hop. The phone ARPs for the gateway, not the bridge;
signaling also uses the gateway rendezvous. That fixture deliberately prevents
the bridge from learning the phone through its own ARP. Its result measures
scout coverage and kernel pressure at the far end of a /22, not ordinary
cold-cache on-link host-check timing. The suppressed `arp-proxy` controls
reproduce this asymmetry.

The cold packet and neighbour traces rule out a wrong receiving interface or
rejection of STALE state in these runs. They do not establish how frequently
this fast path occurs on physical Wi-Fi phones or APs.

## Recommendation for the part 2 decision

Use a **150 ms passive-neighbour observation window inside the existing 250 ms
grace**, then prioritize newly observed, still-usable addresses ahead of older
cluster anchors. This is a proposed policy, not a measured implementation. It
covers these <27 ms observations and one 100 ms snapshot interval with margin.
Take the generation's baseline before sending the answer: the phone ARP can
precede the unresolved `.local` trickle, so the first scout snapshot or port
plan creation is too late to identify it as newly appeared.

There is a design constraint to settle before claiming zero scouts: today's
first real phone indication waits until grace ends, and the authenticated
exchange arrives near 500 ms. A 150 ms passive window alone cannot prevent
scouts in the gap. Achieving zero requires either permitting fresh-neighbour
real probes during grace, or a bounded scout hold while their outcome is
pending. If every no-hit fallback must start by 250 ms and real probes must
also wait 250 ms, these measurements cannot promise zero scouts. Any approved
implementation must preserve the original 25 s expiry, NAT heuristic,
restart budget and every #374 packet, socket, interface and pressure bound.
Retain scouting for the demonstrated one-sided cache and routed cases.

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

The helper tests were written first and failed before implementation; all 21
namespace, cache-control and packet-decoder tests pass. Valid observations,
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
