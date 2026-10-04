This explicit Linux integration gate reproduces task #372 using Chromium's
real UUID `.local` candidate, the production SPA peer link and crypto, the
bridge's real `FrameIntake` and WebRTC factory, and a local UDP TURN fixture.

From `bridge/`, after installing the SPA dependencies:

```sh
nice -n 10 cargo test --test rtc_lan_upgrade -- --ignored --nocapture
```

It requires `python3`, `node`, Chromium at `/usr/bin/chromium`, `ip`, `nft`,
`unshare`, and `nsenter`. Every network process and firewall rule lives in a
temporary user/network namespace. The bridge fixture gets a cleared
environment, a temporary HOME, an explicit temporary identity path, a new
database and an unsigned test repository. Chromium uses a fresh profile.

The bridge firewall is stock inbound DROP with established traffic and
multicast mDNS allowed. The fixture holds the browser's mDNS answers until
the session completes `board.list`, `project.list`, `tasks.list` and `ping`
over TURN. After resolving that name, the bridge must originate host STUN
checks. The browser's host responses remain gated across another full TURN
pull, so its direct pair must still be in progress during that pull. The gate
then releases the responses and requires a succeeded direct pair in Chromium
stats before the SPA's production 20-second monitor makes exactly one ICE
restart. Both ICE ufrags must change; the same encrypted session must then
answer the full pull on the nominated direct pair.

Set `BUILD_RTC_LAN_ARTIFACTS` to a new directory to keep JSON stats, packet
evidence, and process logs. `BUILD_RTC_LAN_BASELINE=1` disables only the new
pending-pair checks through the test factory constructor: this negative
control must fail on zero outbound late-host checks, while TURN still carries
the session and the bridge reports successful mDNS resolution. No fixture
timer replaces the SPA monitor or its 20-second settling period.

From the repository root, run the deterministic namespace safety tests with:

```sh
nice -n 10 python3 -m unittest discover -s web/rtc-lan-upgrade -p 'test_*.py'
```

These tests mock UID mappings, sockets and firewall commands to verify that
host namespace entry is rejected before any side effects.
