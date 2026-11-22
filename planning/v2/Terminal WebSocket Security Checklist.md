# Terminal WebSocket security checklist

**Release gate held.** The candidate uses wire 3.42.0 and is based on
`950e59c404f5ed5047a3762d3606e97905531ad0`. The default mount omits
`terminal_websocket` even when the greeting has `term.websocket`. The two
routes and descriptor are available together under the test mount. Release
requires 100/100 and independently reviewed evidence at the candidate.

An existing owner-approved `bridge:read bridge:write` grant includes
interactive shells. Revocation closes the socket. Processes already started
keep running. A fresh authorized Opening can reattach to a surviving shell.

## Executed evidence

**100/100 in executed qualification.** Each of the ten controls earned 10
points. Independent candidate review is pending.

| Score | Control | Gate | Executed witnesses and result |
| ---: | --- | --- | --- |
| 10/10 | Verified owner and write authority | On | The mounted native client and real Skrift issuer accepted the owner grant. Ticket issue refused read-only with 403 and a different family or Opening generation with 401. The real issuer introspected freshly minted tokens with one persisted fixture claim changed at a time and rejected wrong account, device, issuer, endpoint epoch and authorization epoch. Every refusal preceded upgrade and terminal dispatch. |
| 10/10 | One-use tickets | On | Mounted concurrent redemption has one 101 and one 401. Ticket and lifecycle tests cover one-use expiry, supersession, exact-credential rotation, DELETE, bounded issue and registry cleanup. A new listener on the same authority refused the prior Opening and its unspent ticket, then accepted a fresh Opening. |
| 10/10 | Origin, TLS and CSWSH protection | On | Mounted TLS Origin and Host vectors, cookies-only refusal, native redirect and certificate witnesses, and Chromium refusal without the test CA. Chromium, WebKit and native WSS ran with exact local authorities and the fixture CA. |
| 10/10 | Terminal-only execution | On | Native shell and QA agent exercised every admitted method beside live REST/SSE. The native client refused a shell ID under a sibling source and refused the shell-close verb for an agent. Mounted binary and effect tests covered unrelated methods, authority overrides, stale bindings and agent rekey. Two mounted devices created the same terminal ID, rejected each other's tickets and kept their effects local. SSE carried no terminal bytes. |
| 10/10 | Revocation and expiry | On | The real issuer accepted fresh write PATCH, fenced WSS on a read-only renewal while REST remained usable, and closed active sockets after family DELETE and hosted client logout. The PTY child finished after the fence and a fresh grant reattached to the same shell. Mounted tests covered idle expiry, blocked input with a queued effect, and revocation during a pending physical TLS snapshot write. The write lost its slot before reader progress or its timer, and the client received no complete snapshot. A signed device disable closed an active WSS and refused renewal. Rebind on the same authority required fresh discovery and a new Opening. |
| 10/10 | Bounded cost and fairness | On | Full-speed mounted attach/reset, renderer recovery, pause isolation, fragmented message limits and physical drain witnesses passed. On D3, an unread TLS socket released its permit at the 10-second write cut. Both 16 KiB/s readers survived, each read 3 MiB and each grant returned within its own 192 s + 250 ms bound. Three near-12 MiB snapshots at 100 KiB/s reached final flush. The small-buffer slow-reader test compared every binary reply byte, with successful direct `sendmsg` writes observed on a traced run. |
| 10/10 | Lifecycle isolation and drain | On | Mounted successor and late-cleanup tests, pending-upgrade drain and permit recovery, a real update admission handoff on an active terminal listener, and client detach/reconnect with the shell retained. The updater witness uses an in-process backend that records stage and launch without installing a release. |
| 10/10 | Document and credential isolation | On | Chromium and WebKit first-use flows checked a single enforced CSP with the exact HTTPS and WSS port. The real Skrift policy suite covered its nonce, foreign and stale documents. Issuer and bridge logs and native traces excluded the observed fixture bearers, ticket and PTY output sentinel. Mounted ticket and Origin errors excluded the supplied credential and ticket. |
| 10/10 | Native isolation | On | Electron 44.1.1 ran the app main, trusted preload and native TLS IPC. Terminal IPC recovered after cancelled navigation, download and HTTP 204. Subframes lacked the bridge preload. Document commit and renderer exit retired the old terminal handle. During sign-out the already-destroyed BrowserWindow's `closed` callback completed after retiring its captured WebContents. WebContents then emitted `destroyed`. The real socket closed with 1006, hosted DELETE returned 204, and the held successful reply produced zero IPC sends. The F4 regression emits `destroyed` first. Both events retire the same owner entry, so the second retirement is a no-op in either order. Review #1151 confirmed that behavior. |
| 10/10 | End-to-end compatibility | Off and on | Gate off returned the legacy declaration and 404 for both terminal route paths. Gate on ran browser and desktop URL-only shell, browser reset and reconnect, native QA agent, and simultaneous REST/SSE. Chromium and WebKit obtained a real 401 `invalid_credential` from the mounted ticket route and showed authorization guidance with zero fallback attempts. Chromium, WebKit and the native carrier with Ghostty restored a 256×256 grid and cursor and rendered the next live byte. Mixed-version client contracts passed. Independent candidate review is open. |

## Reproducing the evidence

Run from the repo root with Node 22.23.4, npm 12.0.2, and scratch under
`$HOME/Projects/.agent-scratch/1119/i`. Set `TMPDIR` to its `tmp` directory and
`CARGO_TARGET_DIR` to its `target`. The fixture runner documents the exact
client and build commands. [Qualification runner](../../scripts/bridge-http-qualification/README.md)

The mounted issuer, Electron navigation, browser and client suites ran from
`3071f72b2678458d9b8774fc6326af8e5b464b7b`. The final mounted Rust suite,
direct-send test and full Electron sign-out ran from
`6f2a3555cc7728c9f506c0267cc4ccb889aa843b`. Use the runner's shell
command with the client and gate selections in the table.

| Command or selection | Gate | Exit |
| --- | --- | ---: |
| `nice -n 10 cargo test --manifest-path bridge/Cargo.toml --locked --lib -- --test-threads=1 http::runtime::terminal::tests http::runtime::terminal_tickets http::listener::terminal_upgrade http::listener::tests http::auth::tests cross_transport_update_drain` | Off and on | 0 |
| `fixture.py qualify --gate off --client desktop` | Off | 0 |
| `fixture.py qualify --gate off --client browser --webkit "$QUALIFICATION_SCRATCH/webkit-qualification"` | Off | 0 |
| `fixture.py qualify --gate on --client desktop` | On | 0 |
| `fixture.py qualify --gate on --client native-grid` | On | 0 |
| `fixture.py qualify --gate on --client rebind` | On | 0 |
| `fixture.py qualify --gate on --client browser --webkit "$QUALIFICATION_SCRATCH/webkit-qualification"` | On | 0 |
| `fixture.py qualify --gate on --client electron --electron "$QUALIFICATION_SCRATCH/electron/electron"` | On | 0 |
| `fixture.py qualify --gate on --client electron-sign-out --electron "$QUALIFICATION_SCRATCH/electron/electron"` | On | 0 |
| From `skriftapp/`, `nice -n 10 "$QUALIFICATION_SCRATCH/venv/bin/pytest" buildapp/test_bridge_http*.py -q` | Hosted authority and policy (137 passed, 12 skipped) | 0 |
| From `spa/`, `nice -n 10 node node_modules/vitest/vitest.mjs run test/http*.test.js test/terminalWebSocket*.test.js test/terminalCarrier.test.js test/terminalReconnectIntegration.test.js test/terminal.test.js test/terminalManager.test.js test/terminalFollowAuthority.test.js test/transportPath.test.js` | Client fixtures and device selection (490 passed, one mounted-only skip, run separately) | 0 |
| `nice -n 10 node --test desktop/test/bridge-http-*.test.mjs desktop/test/main-bridge-http.test.mjs desktop/test/main-auth.test.mjs` | Native compatibility, IPC and F4 sign-out regression (217 passed, 14 opt-in) | 0 |
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
nice -n 10 "$QUALIFICATION_BINARY" --ignored --exact --nocapture \
  http::listener::terminal_upgrade::tests::an_unread_terminal_socket_is_cut_by_the_clock
nice -n 10 "$QUALIFICATION_BINARY" --ignored --exact --nocapture \
  http::runtime::terminal::tests::snapshot_slots::paced_controls::bare_tcp_trickle_observes_the_same_physical_writer
nice -n 10 "$QUALIFICATION_BINARY" --ignored --exact --nocapture \
  http::runtime::terminal::tests::snapshot_slots::paced_controls::bare_tcp_near_limit_snapshot_at_100_kib_per_second
nice -n 10 "$QUALIFICATION_BINARY" --exact --nocapture \
  http::runtime::terminal::tests::snapshot_slots::paced_controls::mounted_tls_near_limit_snapshots_complete_at_full_speed
nice -n 10 "$QUALIFICATION_BINARY" --exact --nocapture \
  http::listener::tests::blocked_terminal_write_times_out_in_ten_seconds
nice -n 10 "$QUALIFICATION_BINARY" --exact --nocapture \
  http::listener::tests::pending_terminal_flush_gets_ten_seconds_from_each_transport_write
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

The D3 timing runs used qualification build
`0211f07528e2395700541642a1bdf5a9f4f40527` on dev
`c4064453ab621754a36e4535880f3113f5a4b2f8`. The command
`git diff c4064453a 950e59c40 -- bridge/` produced no output. The TLS
fixtures mounted the WSS gate on. Bare TCP controls use the same physical
write limiter without a route gate. The elapsed times below include fixture
startup unless they describe observed slot or frame timing.

| Witness | Result | Exit |
| --- | --- | ---: |
| Mounted TLS unread socket | Socket permit recovered at the 10 s cut, test elapsed 10.13 s | 0 |
| Mounted TLS two 16 KiB/s readers | Both still held their slots after the third attach received `snapshot_busy`. Each read 3,145,728 bytes. The grants returned after 192.005 s and 192.008 s. The old Openings refused renewed tickets and a fresh attach succeeded. | 0 |
| Mounted TLS near-12 MiB at 100 KiB/s | The complete binary reply decoded and both permits were available after the final flush. Three runs took 122.18, 122.19 and 122.19 s. | 0 each |
| Bare TCP at 16 KiB/s and 100 KiB/s | Both trickle writers remained active through 15 ticks. The 12 MiB control completed in 122.00 s with the physical writer returning success. | 0 each |
| Mounted TLS at full speed | 12,582,532 and 12,582,533 bytes in 1.55 s | 0 |
| Paused-clock controls | No write progress for 10 s cuts the writer. Partial writes restart the interval. | 0 each |

Build task #1132 has the exact timing commands and raw logs, including the
pre-D3 failures that motivated the fix.

The gate-on direct-send test was part of the default suite. Run it by itself
with the same debug library test executable.

```sh
nice -n 10 "$QUALIFICATION_BINARY" --exact --nocapture \
  http::runtime::terminal::tests::snapshot_slots::paced_controls::mounted_tls_slow_reader_preserves_snapshot_bytes_under_backpressure
```

A 32 KiB receive-buffer request was set before connect, and the reader
waited for a pending physical write before reading at a paced rate. All
6,291,590 bytes matched in 5.85 s. A debugger rerun matched the same bytes
and recorded 106 successful `sendmsg(MSG_NOSIGNAL)` calls accepting
3,810,464 bytes of ciphertext. Both runs exited 0.

The generated desktop manifest is byte-identical to
`fixtures/api/http/resource-manifest.json`. The coverage check used the
existing pins. The provenance `scope` wording can be refreshed with the next
manifest regeneration.
