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
```

`PREFER_DEVICE_ID` pins one machine when the account has several
(`deploy/compose.two-bridges.yml`); without it the harness takes the first
device the api reports online.
