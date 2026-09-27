# Build web harnesses (E2EE, over the peer connection)

The Node half of Build's client. `client.mjs` holds the connect sequence the SPA
runs — find the device, mint its sessions on the relay, negotiate the peer
connection, **close the relay socket** — and `peer.mjs` holds the WebRTC half:
the two negotiated DataChannels, the chunked envelope carrier, and one E2EE
session per carrier. The crypto comes from the audited
[`build-secure-transport`](https://github.com/ZechCodes/build-secure-transport)
JS binding; the WebRTC from
[`node-datachannel`](https://github.com/murat-dogan/node-datachannel)'s
standard-interface polyfill, so `peer.mjs` is written against the same DOM API
the SPA is and one read tells you whether the two agree. (`werift` — pure
TypeScript, and the first choice for needing no prebuilt binary — was tried
first and got as far as DTLS; its SCTP association never completed against the
bridge's webrtc-rs.) `web/` is dev-only, which is why a dependency here costs
nothing shipped.

It depends on the binding as a sibling checkout:

```
<parent>/
  Build/                     ← this repo (web/ lives here)
  build-secure-transport/    ← the audited E2EE binding (js/)
```

## How the harness connects

Every check runs over the DataChannels, because that is the only wire there is
([`planning/v2/Strict P2P Transport Spec.md`](../planning/v2/Strict%20P2P%20Transport%20Spec.md)):

1. `skrift-auth.mjs` dummy-logs into the api and mints a 5-minute gateway token.
2. `openRendezvous()` opens `/ws/client` and authenticates. This is the one
   relay-shaped function in the harness; a future direct-network mode replaces
   it and nothing above it.
3. `pinnedDevice()` reads `GET /api/devices` for the device under test and the
   transport key the api pinned for it. The relay is never asked for a key.
4. `openDeviceLink()` mints the app session on that socket — and the terminals'
   second session beside it, when a check wants one — then fetches ICE servers
   from `POST /api/rtc/ice-servers`, offers, trickles candidates both ways, and
   settles when the `app` (id 0) and `term` (id 1) channels are open.
5. Each session sends one frame over its channel, and the relay socket is
   **closed**. `rendezvous.isClosed()` is a check in `qa.mjs`, not a comment.

`openRelaySignalingSession()` is the exception that proves the rule: a session
carried by the relay socket itself, kept so one check can offer the bridge app
RPC over it and watch the refusal —
`error_code: "unavailable"`, `details.reason: "relay_is_not_a_data_plane"`.

## One rule for anything reading this wire

**A frame bearing your request's id is not necessarily the answer.** The
bridge receipts every request the moment its intake admits it, before anything
decides how long answering will take:

```json
{ "id": "r12", "accepted": true }
```

`ok` is what says a reply settled a call. `accepted` with no `ok` settles
nothing — the device has the request and the answer is still coming. A reader
that resolved on the first frame carrying its id resolves every call on the
receipt; `peer.mjs` skips it in `openCarriedSession`, and `sessionRpc.js` in
the SPA treats it as proof of delivery and waits far longer after it than
before it.

That is what the receipt is FOR: before it, a deadline is a question about the
path; after it, the client knows the request is on the device and can stop
guessing. A 15 KB attachment was reported to its reader as failed after ten
seconds while the bridge was busy storing it, which is the bug that bought
this frame.

## The suites

```bash
# 1. Install (the binding resolves to ../../build-secure-transport/js)
cd web && npm install
#    and install the binding's own deps once:
( cd ../../build-secure-transport/js && npm install )

npm test           # the chunker, against the wire shape the bridge writes — no stack needed
```

The rest need the real stack (`deploy/compose.real.yml`), because the peer
connection needs a real bridge on the other end of it:

```bash
docker compose -f deploy/compose.real.yml up -d --build
docker compose -f deploy/compose.real.yml --profile qa run --rm qa    # pairs, then e2e + qa

npm run e2e        # one encrypted round-trip over the `app` channel
npm run qa         # workspaces, files, git and the terminal, end to end
node wire-check.mjs  # the 1.1 wire surface, by hand from the host

# The task lines and the three incoming kinds, measured under the real
# stylesheet at 390 px and on a desktop: gutter, row height, the gap between
# consecutive lines and the gap a real message keeps. jsdom has no layout, so
# this is the only thing that can check a claim about SPACE — it is what found
# a 90px gutter, rows wrapping to three lines, and a rhythm 4px out. Reads the
# committed fixture, which spa/test/taskLineFixture.test.js holds to the
# renderer; needs a seed from live-seed.mjs; exits non-zero on a failed check.
TASKS_REPO=$PWD/.. node task-line-measure.mjs

# A dropped session over an open surface: the cached copy is kept and marked,
# and the read lands again on reconnect (#24). PAUSES the bridge rather than
# stopping it — a stopped container deregisters the device, and a machine that
# is gone is a different case from one being reconnected to. Needs a seed from
# live-seed.mjs; exits non-zero on a failed check.
TASKS_REPO=$PWD/.. node dropped-read-check.mjs

# A session whose path died with ICE still calling it connected (#30): the probe
# asks the wire, judges it dead in ~15 s where SCTP took ~105 s, and the reconnect
# settles the uncertain post and re-fetches the attachment on its own. PINS
# `RTCPeerConnection` to keep reporting a reached state, because a paused
# container stops answering ICE's consent checks too and the browser would
# otherwise notice on its own — which is the symptom, not the fault.
TASKS_REPO=$PWD/.. node dead-path-check.mjs

# The bridge noticing its OWN frames are not leaving (#41). A paused bridge
# cannot exercise this — a frozen container runs no code — so the fault is made
# from the other end: a multi-megabyte attachment fetch in flight, then SIGSTOP on
# the browser's whole process tree, so it stops draining its sockets while the
# bridge writes. Seeds its own 4 MiB file and removes it. Thaws on any exit.
TASKS_REPO=$PWD/.. node stall-watch-check.mjs

# The hold making a direct pair win a race a relay pair would have won (#31).
# Builds the race the stack does not have: a coturn on the host, and every
# non-relay candidate delayed in BOTH directions. Runs its own control with a skew
# PAST the hold window, where the relay pair must win — without that, "direct won"
# in the other phase would prove nothing.
#
# On this machine the control does not hold, and the check says so with exit 3
# rather than pretending either way: the bridge is directly reachable and offers
# only host candidates, and ICE forms a direct pair from peer-reflexive candidates
# its own checks discover, so candidate ORDER is not what decides the race here.
# Making the direct PATH slow (tc netem, or a NAT'd second bridge) is what this
# would need, and both want root. Exit 0 the hold won, 1 it did not, 3 the
# experiment could not be set up. Takes coturn down on any exit.
TASKS_REPO=$PWD/.. node relay-wins-check.mjs
```

## The liveness gate (#131)

`scripts/liveness-gate.sh` (repo root) is the TURN soak as a gate, and what
`.github/workflows/liveness.yml` runs before a bridge release publishes. It
brings up its own stack (`deploy/compose.liveness.yml` over
`deploy/compose.real.yml`, project `liveness-gate`, app on 8128, relay on
18128) with a coturn on the stack's network, pairs, and then at once:

- `liveness-soak.mjs`, in the qa image: one TURN-only session for ten
  minutes, busy loops in a terminal and an `tasks.list` hammer beside it;
  fails on any drop, any timeout, any ping at 500 ms or over;
- `ice-restart-check.mjs`, four minutes in, in `mcr.microsoft.com/playwright`
  on the host's network: the real app over the same TURN, made to see its
  peer fail once, so it runs its own ICE restart; fails unless the restart
  lands within 15 s, on the same peer with a new ufrag at both ends and a
  new nominated relay pair, and that pair carries every probe the check has
  the app send (`buildConnectionProbe()`, a `ping` over the session's path)
  on landing and every 4 s through a 20 s hold. `NEGATIVE_CONTROL=1` strips
  `iceRestart` from the app's restart offer, and the gate must then fail.

It needs docker and nothing else, exits non-zero if either failed, and takes
the stack down whatever happened. `SOAK_MS=120000 RESTART_AT_S=45` is a short
run; the soak's other knobs (`LOAD_AGENTS`, `PROBE_STATS`, `LOAD_TERM_FLOOD`)
are in its header.

`PREFER_DEVICE_ID` pins one machine when the account has several
(`deploy/compose.two-bridges.yml`); without it the harness takes the first
device the api reports online.
