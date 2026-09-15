# Stage 07 — QA over the peer path, deploy, docs

Binding contract: spec "QA and deploy after this plan" and "What stays the same".

## Context a cold agent needs

- QA today is `deploy/compose.real.yml:115` → `node pair.mjs && node e2e.mjs && node qa.mjs`,
  run by CI at `.github/workflows/ci.yml:167-172` when `.github/changed-tiers.sh` says so
  (`E2E_HARNESS_PATHS` line 28, `RELAY_PATHS` line 27). `web/wire-check.mjs` (9 checks a–i,
  by hand from the host, header :14-16) and `web/pair-another.mjs` are not in CI.
  `deploy/compose.two-bridges.yml` + `deploy/README.md` "Two bridges on one account".
- **Every Node check rides the relay.** `web/client.mjs` waits for `device_key` (:33-37) and
  runs `session_init`/`session_accept` over the socket (:59-64, :113-118, :204-210); `qa.mjs`
  (574 lines, ~40 checks) opens `ws://…/ws/client` :39, authenticates :61-80 and runs
  workspace/fs/git/terminal checks over the relay (`:224` "workspace terminal echoes over the
  relay"); `e2e.mjs` the same; `wire-check.mjs` uses `openPushSession` from `client.mjs`.
  After stages 02/03 all of them fail: no `device_key`, and the bridge refuses app RPC over
  the relay.
- Node has no built-in WebRTC. Options, in order of preference: `werift` (pure TypeScript
  WebRTC; supports `createDataChannel(label, {negotiated: true, id})` — verify in its README
  /types before committing to it), `node-datachannel` (native, prebuilt binaries; also
  supports negotiated ids). `web/` is dev-only (`web/package.json`), so a dependency is
  acceptable; it must also install inside the `qa` image (`web/Containerfile` or wherever
  the compose `qa` service builds from — check `deploy/compose.real.yml` `qa.build`).
- The SPA's chunker mirror is `spa/src/core/chunk.js` (16 KiB parts, `{"part":{id,index,
  count},"data"}`); the DataChannel carrier is `spa/src/core/carrier.js:105-171`; the
  negotiated channels are `app` id 0 and `term` id 1, ordered. Port these into
  `web/peer.mjs` (small; do not import from `spa/`).
- Docs to change: `README.md` architecture block + Cloudflare paragraph; `HANDOFF.md`
  "Topology (final)" :10-44, "browser↔relay contract" :45-57 (rules 5, 6), "Known gaps" :146-
  (item 3 list), the multi-device section's line about `device_offline` push (:≈427) and
  follow-on 3 (:430 per-device terminal sockets — still true, reword to "terminal session");
  `deploy/README.md` :42 and :62 ("keep working over the relay") and the two-bridges
  section; `planning/v2/WebRTC Transport Spec.md` status line; `planning/v2/roadmap.md` §0
  broker bullet. `deploy/k8s/relay.yaml` was done in stage 02 — verify.

## What to build

1. **`web/peer.mjs`**: given an authenticated relay socket and a minted session (from
   `client.mjs`), negotiate a peer connection as the browser does: fetch ICE servers via
   `POST /api/rtc/ice-servers` (dummy-auth cookie as `qa.mjs` already does), `rtc.offer
   {sdp, ice_servers}` → answer, trickle `rtc.ice` both ways, two negotiated channels, chunked
   envelope carrier with the 8 MiB reassembly cap, `close()`. Export `openPeerSession()` that
   returns the same `{call, onPush, close}` shape `client.mjs`'s session has, so checks are
   carrier-agnostic.
2. **`client.mjs`**: delete the `device_key` wait — take the pinned transport key from
   `GET /api/devices` — and make `openSession` = mint over the relay → `openPeerSession` →
   **close the relay socket** → return the peer-backed session. Keep the old relay-backed
   session under `openRelaySignalingSession` for the negative check only.
3. **`qa.mjs` / `e2e.mjs` / `wire-check.mjs`**: run every existing check unchanged over the
   peer session; rename the `:224` check "… echoes over the DataChannel"; add one check in
   `qa.mjs`: `session.hello` over the relay signaling session is refused with
   `details.reason === "relay_is_not_a_data_plane"`; add one: the relay socket is closed
   (`readyState === 3`) while the checks run. `wire-check.mjs` stays by-hand but must pass.
4. **Compose / CI**: the `qa` image installs the WebRTC dependency; nothing else changes in
   `ci.yml`. Add the compose STUN-only note: with no `CF_TURN_KEY_*` the api returns STUN
   only and containers on one compose network connect host/host.
5. **Docs** as listed, plus `web/README.md` (how the harness connects now).
6. Run `podman compose -f deploy/compose.real.yml up -d --build` and `--profile qa run --rm qa`
   locally (docker via `newgrp docker` if podman is absent) and, separately, the two-bridge
   stack with `pair-another.mjs` then `wire-check.mjs`; paste the tails into the commit
   message.

## Done when

Compose QA green over the peer path with the relay socket closed; `wire-check.mjs` green by
hand; `grep -rn "keep working over the relay\|stays on the relay\|device_key" README.md
HANDOFF.md deploy web/*.mjs` returns only historical lines in HANDOFF's dated sections;
commits landed.
