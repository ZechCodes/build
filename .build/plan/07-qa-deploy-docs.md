# Stage 07 — QA over the peer path, deploy, docs

Binding contract: spec "QA and deploy after this plan" and "What stays the same".

## Context a cold agent needs

- `deploy/compose.real.yml` — services `app`, `relay`, `bridge`, one-shot `qa` (profile
  `qa`) which runs `web/qa.mjs` / `web/e2e.mjs` / `web/pair.mjs`. `web/client.mjs`
  :30-40 `awaitDeviceKey` no longer works (stage 02). `web/pair.mjs` is pure HTTP and
  still works. `web/README.md:17-20, 38-40` describe the relay round trip; `:28-40` tell
  you to run `bridge/examples/dev_relay.rs`. `deploy/README.md:42, :62` say "peers that
  cannot hole-punch simply keep working over the relay". `README.md` architecture block
  and the "second infrastructure party" paragraph; `HANDOFF.md` "Topology (final)" :10-44,
  "browser↔relay contract" :45-57 (steps 2, 5 change), "Known gaps" :117-128 item 3.
- `bridge/src/rtc/testing.rs` — the in-process browser-side peer harness
  (`browser_peer_with` :75). `bridge/tests/rtc_peer.rs` shows how a full negotiation is
  driven from Rust. `bridge/src/transport.rs` has the client-side session functions
  (`create_session_init`, `open_session_accept`, `encrypt_frame`, `decrypt_envelope`)
  used by `interop_python.rs`.
- `bridge/examples/dev_relay.rs` (184) — the self-contained dev relay + echo device.
- `.github/workflows/ci.yml` and `.github/changed-tiers.sh` decide which suites run per
  changed tier; the compose QA is invoked somewhere in there — find it.

## What to build

1. **`bridge/examples/qa_peer.rs`** — a Rust "browser": `POST /api/gateway-token` with a
   dummy-auth session cookie (see how `web/qa.mjs` :30-80 logs in), `ws /ws/client`,
   `authenticate`, `GET /api/devices` for the pinned key, `session_init` for an app and a
   terminal session, `rtc.offer` through the app session with the api's ICE servers (or
   STUN-only in compose), trickle both ways, open the two negotiated channels, **close
   the relay socket**, `session.hello` over `app`, `term.create`/`term.attach`/input echo
   over `term`, and the e2e assertions `web/e2e.mjs` and `web/qa.mjs` make (list them in
   the file header, port each; drop any that only make sense over the relay). Exit
   non-zero on the first failure with a one-line reason. Reuse `rtc/testing.rs` where it
   can be exposed via a `pub` test-support module; do not duplicate the chunker.
2. **Negative check** in the same binary (`--expect-refusal`): send `session.hello` over
   the relay carrier and assert the `relay_is_not_a_data_plane` error (stage 03).
3. **Compose**: the `qa` one-shot builds/runs `qa_peer` (add a `qa` target to
   `bridge/Containerfile` or run it from the bridge image) after `pair.mjs`; keep
   `pair.mjs`. Retire `web/qa.mjs`, `web/e2e.mjs`, `web/client.mjs`, `web/terminal.mjs`,
   `web/feature-check.mjs` from the profile and list them as stale in `web/README.md`
   beside the three already listed; delete `bridge/examples/dev_relay.rs`'s presence code
   if stage 02 left any.
4. **CI**: the tier script runs `qa_peer` where it ran the Node QA; nothing else.
5. **Docs**: README architecture block (relay = "auth + rendezvous, closes after
   negotiation"), the Cloudflare paragraph (unchanged facts, plus "TURN is the only path
   that relays bytes, and it is opt-in by cost"), HANDOFF topology + contract (steps: token
   → devices → `/ws/client` → mint → `rtc.offer` → channels → socket closed), `deploy/README.md`
   (both sentences, plus the LAN/Tailscale paragraph from stage 04 if missing),
   `planning/v2/WebRTC Transport Spec.md` status line → "superseded in part by Strict P2P
   Transport Spec.md", `planning/v2/roadmap.md` §0 broker bullet gains one sentence.
6. Run the full compose QA locally (`podman compose -f deploy/compose.real.yml --profile
   qa run --rm qa`; docker is reachable via `newgrp docker` on this machine if podman is
   not) and paste the tail of its output into the commit message.

## Done when

Compose QA green over the peer path with the relay socket closed; CI tier script updated;
every doc sentence that said the relay carries app traffic is gone (`grep -rn "keep working
over the relay\|stays on the relay" README.md HANDOFF.md deploy planning/v2/Strict*`); commits landed.
