# Terminal WebSocket security checklist

**Release gate held.** The candidate uses wire 3.42.0 from
`6e408a18213104a059d174d3630f541d3ad8fe38`. The default mount omits
`terminal_websocket` even when the greeting has `term.websocket`. The two
routes and descriptor are available together under the test mount. Release
requires 100/100 and independently reviewed evidence at the candidate.

An existing owner-approved `bridge:read bridge:write` grant includes
interactive shells. Revocation closes the socket. Processes already started
keep running. A fresh authorized Opening can reattach to a surviving shell.

## Executed evidence

Each complete control is worth ten points. Partial evidence earns zero.
The two paced transport requirements are withheld pending the write-timeout
fix.

| Control | Gate | Executed witnesses and result | Score |
| --- | --- | --- | ---: |
| Verified owner and write authority | On | The mounted native client and real Skrift issuer accepted the owner grant. Ticket issue refused read-only with 403 and a different family or Opening generation with 401. The real issuer introspected freshly minted tokens with one persisted fixture claim changed at a time and rejected wrong account, device, issuer, endpoint epoch and authorization epoch. Every refusal preceded upgrade and terminal dispatch. | 10/10 |
| One-use tickets | On | Mounted concurrent redemption has one 101 and one 401. Ticket and lifecycle tests cover one-use expiry, supersession, exact-credential rotation, DELETE, bounded issue and registry cleanup. A new listener on the same authority refused the prior Opening and its unspent ticket, then accepted a fresh Opening. | 10/10 |
| Origin, TLS and CSWSH protection | On | Mounted TLS Origin and Host vectors, cookies-only refusal, native redirect and certificate witnesses, and Chromium refusal without the test CA. Chromium, WebKit and native WSS ran with exact local authorities and the fixture CA. | 10/10 |
| Terminal-only execution | On | Native shell and QA agent exercised every admitted method beside live REST/SSE. The native client refused a shell ID under a sibling source and refused the shell-close verb for an agent. Mounted binary and effect tests covered unrelated methods, authority overrides, stale bindings and agent rekey. Two mounted devices created the same terminal ID, rejected each other's tickets and kept their effects local. SSE carried no terminal bytes. | 10/10 |
| Revocation and expiry | On | The real issuer accepted fresh write PATCH, fenced WSS on a read-only renewal while REST remained usable, and closed active sockets after family DELETE and hosted client logout. The PTY child finished after the fence and a fresh grant reattached to the same shell. Mounted tests covered idle expiry, blocked input with a queued effect, and revocation during a pending physical TLS snapshot write. The write lost its slot before reader progress or its timer, and the client received no complete snapshot. A signed device disable closed an active WSS and refused renewal. Rebind on the same authority required fresh discovery and a new Opening. | 10/10 |
| Bounded cost and fairness | On | Full-speed mounted attach/reset, renderer recovery, pause isolation, fragmented message limits and physical drain witnesses passed. Both paced TLS witnesses reproduced a premature write timeout while the readers were progressing. | Withheld |
| Lifecycle isolation and drain | On | Mounted successor and late-cleanup tests, pending-upgrade drain and permit recovery, a real update admission handoff on an active terminal listener, and client detach/reconnect with the shell retained. The updater witness uses an in-process backend that records stage and launch without installing a release. | 10/10 |
| Document and credential isolation | On | Chromium and WebKit first-use flows checked a single enforced CSP with the exact HTTPS and WSS port. The real Skrift policy suite covered its nonce, foreign and stale documents. Issuer and bridge logs and native traces excluded the observed fixture bearers, ticket and PTY output sentinel. Mounted ticket and Origin errors excluded the supplied credential and ticket. | 10/10 |
| Native isolation | On | Electron 44.1.1 ran the app main, trusted preload and native TLS IPC. Terminal IPC recovered after cancelled navigation, download and HTTP 204. Subframes lacked the bridge preload. Document commit and renderer exit retired the old terminal handle. Contract tests covered renderer pressure, late navigation events, foreign handles and TLS policy. Delayed delivery during Electron sign-out remains open. | 0/10 |
| End-to-end compatibility | Off and on | Gate off returned the legacy declaration and 404 for both terminal route paths. Gate on ran browser and desktop URL-only shell, browser reset and reconnect, native QA agent, and simultaneous REST/SSE. Chromium, WebKit and the native carrier with Ghostty restored a 256×256 grid and cursor and rendered the next live byte. Mixed-version client contracts passed. Independent candidate review is open. | 0/10 |

**Credited evidence so far: 70/100.** The mounted timing failures hold
release. The open cases in the table require executed evidence for credit.

## Reproducing the evidence

Run from the repo root with Node 22.23.4, npm 12.0.2, and scratch under
`$HOME/Projects/.agent-scratch/1119/i`. Set `TMPDIR` to its `tmp` directory and
`CARGO_TARGET_DIR` to its `target`. The fixture runner documents the exact
client and build commands. [Qualification runner](../../scripts/bridge-http-qualification/README.md)

| Command or selection | Gate | Exit |
| --- | --- | ---: |
| `nice -n 10 cargo test --manifest-path bridge/Cargo.toml --locked --lib -- --test-threads=1 http::runtime::terminal::tests http::runtime::terminal_tickets http::listener::terminal_upgrade http::auth::tests cross_transport_update_drain` | Off and on | 0 |
| `fixture.py qualify --gate off --client desktop` | Off | 0 |
| `fixture.py qualify --gate off --client browser --webkit "$QUALIFICATION_SCRATCH/webkit-qualification"` | Off | 0 |
| `fixture.py qualify --gate on --client desktop` | On | 0 |
| `fixture.py qualify --gate on --client native-grid` | On | 0 |
| `fixture.py qualify --gate on --client rebind` | On | 0 |
| `fixture.py qualify --gate on --client browser --webkit "$QUALIFICATION_SCRATCH/webkit-qualification"` | On | 0 |
| `fixture.py qualify --gate on --client electron --electron "$QUALIFICATION_SCRATCH/electron/electron"` | On | 0 |
| From `skriftapp/`, `nice -n 10 "$QUALIFICATION_SCRATCH/venv/bin/pytest" buildapp/test_bridge_http*.py -q` | Hosted authority and policy (137 passed, 12 skipped) | 0 |
| From `spa/`, `nice -n 10 node node_modules/vitest/vitest.mjs run test/http*.test.js test/terminalWebSocket*.test.js test/terminalCarrier.test.js test/terminalReconnectIntegration.test.js` | Client fixtures (396 passed, one mounted-only skip, run separately) | 0 |
| From `spa/`, `nice -n 10 node node_modules/vitest/vitest.mjs run test/terminal.test.js test/terminalManager.test.js test/terminalFollowAuthority.test.js test/transportPath.test.js` | Device and transport selection (94 passed) | 0 |
| `nice -n 10 node --test desktop/test/bridge-http-*.test.mjs desktop/test/main-bridge-http.test.mjs` | Native fixtures (151 passed, 13 opt-in) | 0 |
| `nice -n 10 cargo test --manifest-path bridge/Cargo.toml --locked --test terminal_websocket_contract` | Shared wire contract (14 passed) | 0 |
| `nice -n 10 node --test scripts/http-resource-coverage.test.mjs` | Static coverage | 0 |
| `nice -n 10 node scripts/http-resource-manifest.mjs --check` and `nice -n 10 cmp fixtures/api/http/resource-manifest.json desktop/src/bridge-http-resources.json` | Generated artifacts | 0 |

Use the `--lib` debug test binary described in the runner. These tests
are opt-in.

```sh
export TERMINAL_WSS_TEST_NODE="$(command -v node)"
nice -n 10 "$QUALIFICATION_BINARY" --ignored --exact \
  http::runtime::terminal::tests::snapshot_slots::mounted_tls_lost_close_stops_spa_ticket_requests
nice -n 10 "$QUALIFICATION_BINARY" --ignored --exact --nocapture \
  http::runtime::terminal::tests::snapshot_slots::mounted_tls_trickling_readers_recover_both_snapshot_slots
nice -n 10 "$QUALIFICATION_BINARY" --ignored --exact --nocapture \
  http::runtime::terminal::tests::snapshot_slots::mounted_tls_maximum_snapshot_completes_at_100_kib_per_second
```

The lost-Close JSDOM SPA witness exited 0. It used the real mounted TLS
bridge and the desktop's locked WebSocket client and observed 1006, ticket
409, a quiet retry interval and ticket 201 after explicit Retry. The nightly
timing job runs that witness with the exact pinned Node executable.

Run the same lost-Close sequence and maximum-grid recovery in Chromium
155.0.8059.84 and WebKit 27.2. Each run exited 0. The browser witness
uses the SPA terminal binding and Ghostty 0.4.0 installed by the SPA lock.
It checks all 65,536 cells, cursor position, the next live byte and
keystroke echo on one socket. The native maximum-grid runner used the same
Ghostty renderer with the native WSS carrier and the real hosted verifier.
The browser fixture uses a local document and a generated test-certificate
exception. The hosted browser and native groups verify the fixture CA.

```sh
for test in \
  snapshot_slots::mounted_tls_lost_close_stops_real_browser_ticket_requests \
  flow::mounted_tls_real_browser_recovers_maximal_snapshot
do
  CHROMIUM_PATH=/usr/bin/chromium \
    nice -n 10 "$QUALIFICATION_BINARY" --ignored --exact --nocapture \
    "http::runtime::terminal::tests::$test"
  TERMINAL_WSS_TEST_ENGINE=webkit \
    BUILD_HTTP_QUALIFICATION_WEBKIT="$QUALIFICATION_SCRATCH/webkit-qualification" \
    PLAYWRIGHT_BROWSERS_PATH="$QUALIFICATION_SCRATCH/playwright" \
    nice -n 10 "$QUALIFICATION_BINARY" --ignored --exact --nocapture \
    "http::runtime::terminal::tests::$test"
done
```

The two-reader witness exited 101. Both readers consumed 180,224 bytes
by 10 seconds and the third attach could receive a slot while the first
two were still reading. A trace showed both slots released following
`TimedOut: HTTP stalled write` at the listener. The 100 KiB/s witness
exited 101 on multiple runs and 0 on others. In a traced failure the
client received 9,519,464 bytes and then reported a WebSocket reset
without a Close frame. A test-only socket-buffer control reproduced the
failure after 6,275,302 bytes at 61 seconds. The same physical writer
reproduced both failures on plain loopback TCP. The full-speed mounted
TLS control delivered two near-12 MiB messages in 1.32 seconds, exit 0.
Raw timings, exact debugger commands and the standalone test commits are
attached to Build task #1132.

The generated desktop manifest is byte-identical to
`fixtures/api/http/resource-manifest.json`. The coverage check used the
existing pins. The provenance `scope` wording can be refreshed with the next
manifest regeneration.
