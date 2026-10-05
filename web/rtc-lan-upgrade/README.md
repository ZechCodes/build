This explicit Linux integration gate reproduces tasks #372 and #374 using Chromium's
real UUID `.local` candidate, the production SPA peer link and crypto, the
bridge's real `FrameIntake` and WebRTC factory, and a local UDP TURN fixture.

From `bridge/`, after installing the SPA dependencies:

```sh
nice -n 10 cargo test --test rtc_lan_upgrade -- --ignored --nocapture
```

It requires `python3`, `node`, Chromium at `/usr/bin/chromium`, `ip`, `nft`,
`unshare`, and `nsenter`; the pressure control also uses `ss`. Every network process and firewall rule lives in a
temporary user/network namespace. The bridge fixture gets a cleared
environment, a temporary HOME, an explicit temporary identity path, a new
database and an unsigned test repository. Chromium uses a fresh profile.

The bridge firewall is stock inbound DROP with established traffic and
multicast mDNS allowed. TURN lives at a separate address in the browser
namespace. A real nft SNAT rule gives phone-to-TURN packets a documentation
address, so TURN checks disclose that public mapping, rather than accidentally
disclosing the phone's private address through an on-LAN TURN server. The
bridge routes only the TURN address; the public phone mapping is off-link.
The default `delayed` mode disables only the sweep through the test factory,
so the original late-check and final failure-history assertions measure
those components independently. All six unresolved modes enable the
production sweep. The default mode holds the browser's mDNS answers until
the session completes `board.list`, `project.list`, `tasks.list` and `ping`
over TURN. After resolving that name, the bridge must originate host STUN
checks. The browser's host responses remain gated across another full TURN
pull, so its direct pair must still be in progress during that pull. The gate
then releases the responses and requires a succeeded direct pair in Chromium
stats before the SPA's production 20-second monitor makes exactly one ICE
restart. Both ICE ufrags must change; the same encrypted session must then
answer the full pull on the nominated direct pair. It then removes browser
connection-state listeners for this final failure-only phase, keeping native
ICE running while preventing automatic browser recovery from replacing the
generation being measured. Dropping UDP returns in the disposable namespace
must produce the bridge's actual ICE Failed event using its unchanged 5+25 s
timeout. An encrypted `rtc.close` must retain nonzero successful host counters;
the final session summary must retain direct time and the actual restart count.

Set `BUILD_RTC_LAN_ARTIFACTS` to a new directory to keep JSON stats, packet
evidence, and process logs. `BUILD_RTC_LAN_BASELINE=1` disables only the new
pending-pair checks through the test factory constructor: this negative
control must fail on zero outbound late-host checks, while TURN still carries
the session and the bridge reports successful mDNS resolution. No fixture
timer replaces the SPA monitor or its 20-second settling period.

`BUILD_RTC_LAN_MODE=early-unresolved` delivers the genuine host trickle at its
normal production arrival time with mDNS permanently silenced and inbound
DROP. The bridge must select host/prflx with zero restarts before the first
encrypted application RPC. The fixture records Chromium's selected direct
pair before `session.hello` and before each full-pull RPC, requires that pair
to carry the pull, and requires the bridge's complete carrying history and
final summary to contain only direct. TURN ICE and DTLS setup packets do not
count as application traffic. This preserves the early win as its own case.

`BUILD_RTC_LAN_MODE=far-edge-unresolved` repeats that production early-win
proof on a mostly empty /22: the real phone owns `10.72.3.254/22`, near the
far end of the bridge's actual on-link interface subnet. Real signaling and
TURN traffic establish that interface's neighbor entry; no ICE address is
invented. The JSON result records first indication timing against genuine
candidate gathering and sweep start, the first application RPC time, and
timestamped sweep pass events. The native host/prflx pair must carry every
application RPC with zero restarts, under the same permanently silenced mDNS,
inbound DROP and 65-second outer deadline.

`BUILD_RTC_LAN_MODE=far-edge-pressure` is a separate no-direct /22 socket
pressure control. It uses genuine Chromium native offer/trickle, channels,
the same encrypted SessionRPC and bridge intake, and the same far-edge phone.
The browser's host-port UDP checks are dropped by a counted namespace OUTPUT
rule installed before applying its genuine remote answer. This control omits
the SPA optional-upgrade monitor so the bridge's fixed 25-second sweep can
expire without a fresh generation replacing it. It does not prove production
monitor behavior. It requires zero restarts, a full encrypted TURN pull after
expiry, and the actual firewall drop count. Results record elapsed time,
successful sends, attempted sends and the incomplete tail's `window-expired`
reason. They never call a direct stop or an expired tail a completed pass.
Read-only `ss` samples select the actual advertised host socket internally
using its native candidate port; the saved samples retain only occupied RX/TX,
TX capacity and neighbor-state counts. They contain no addresses or ports.
Results report peak occupied fraction and remaining headroom, including normal
TURN traffic when it shares that socket. The outer deadline remains 65 seconds.

`BUILD_RTC_LAN_MODE=unresolved` permanently silences mDNS and leaves browser
host connectivity checks running under the bridge's inbound DROP. It delivers
the genuine `.local` host trickle after the initial encrypted TURN pull; early
production probing can otherwise make the initial connection direct before
its application channels open. The same
encrypted session must first complete a full TURN pull, then upgrade and
complete the pull on direct without creating a second peer connection or
making more than one optional restart. Packet evidence requires a 28-byte
Binding Indication containing only FINGERPRINT from the advertised host
socket, followed by authenticated browser checks; the bridge must select a
host/prflx pair and report that result for the sweep's generation. This is
the regression command:

```sh
BUILD_RTC_LAN_MODE=unresolved nice -n 10 cargo test --test rtc_lan_upgrade -- --ignored --nocapture
```

Adding `BUILD_RTC_LAN_SWEEP_BASELINE=1` disables only the sweep in the test
factory. This negative control must fail the direct-upgrade assertion after
32 seconds: the existing encrypted session remains on TURN, its mDNS name
stays unresolved, and the production 20-second monitor does not restart ICE.
With this topology, Chromium initially retries the unanswered host pair for
about 15 seconds; packet capture records the actual cadence rather than
assuming retries continue forever.

`BUILD_RTC_LAN_MODE=large-subnet` uses /21 instead of /24 on the same candidate
interface. It requires the fixed `subnet-too-large` skip reason, zero sweep
indications, and a full TURN pull after the production monitor's first sample
with zero restarts. All modes use the same 65-second fixture process deadline.

`BUILD_RTC_LAN_MODE=late-unresolved` holds the real `.local` host trickle until
packet capture records Chromium's measured 31-check retry budget and at least
15 seconds since its first check. Native stats may remove that pair or retain
it as in progress; the fixture must prove retrospectively that no original-port
check resumed between that snapshot and the actual optional restart.
The exact candidate is then delivered through the encrypted `rtc.ice` RPC,
with the responder still silent. The actual indication to the phone from the
advertised host socket must leave TURN selected
with zero restarts; the existing single optional upgrade must issue fresh ICE
credentials, revive authenticated host checks, and upgrade the same encrypted
session to direct. Chromium can reuse its UDP port across this restart, so the
first pass's useful conntrack entries remain valid; no test forces a new port
or clears them. This deliberate late delivery retains the original 15-second
signaling completion deadline, which may report a timeout before the candidate
arrives, and the original restart budget.

From the repository root, run the deterministic namespace safety tests with:

```sh
nice -n 10 python3 -m unittest discover -s web/rtc-lan-upgrade -p 'test_*.py'
```

These tests mock UID mappings, sockets and firewall commands to verify that
host namespace entry is rejected before any side effects.
