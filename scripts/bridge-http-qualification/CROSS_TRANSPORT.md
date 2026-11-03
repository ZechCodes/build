# Cross-transport acceptance (#977)

The Rust acceptance probes live in
`bridge/src/http/runtime/tests/subscriptions/cross_transport/`. They mount the
production HTTP router and share **the same** `FrameIntake::execution()` with
four negotiated DataChannel carrier sessions. Requests, worker scheduling,
authority fences, stream bindings and buffer reservations are production code.
Only the domain handler, verified HTTP authority and stalled socket are test
fixtures. This is an in-process carrier proof; it does not negotiate ICE/DTLS,
contact a relay, run an installed daemon or measure process RSS.

Each test states its pass criterion in its doc comment:

| Mechanism | Required observation |
| --- | --- |
| Concurrent load | Four RTC sessions, 32 SSE streams, four families with eight streams each; all four RTC handlers enter a gate and remain pending throughout each batch's four successful REST calls; release them only after every REST response, completing 32 calls per transport; ninth family and 33rd global streams refuse with retryable `overloaded` |
| Ordered SSE pushes | Each of the 32 SSE streams delivers exactly `0, 1, 2`, with no trailing event queued; no RTC push-order claim is made |
| Separate admission | Saturating the production 1024-frame carrier queue returns `busy`; REST snapshot reads still finish with all streams admitted |
| Bounded buffering | Separate 32 MiB RPC and 32 MiB SSE allowances; at most 1 MiB/128 queued items per stream; all allowances returned after settlement |
| Cancellation | Carrier close and HTTP future cancellation fence output; running work retains its allowance until settlement; stream cancellation admits a same-family replacement |
| Reservation drop order | A synchronized blocked family cleanup cannot expose returned pool permits first; no sleep or scheduling race decides this assertion |
| Stalled revocation | All 32 pending writers end within 25 ms of local authority invalidation, alongside teardown of all four RTC carriers |
| Idle revocation | An already waiting stream must end within the same 25 ms without another event or heartbeat |
| Cleanup reentry | Reaper notification can synchronously re-enter cleanup without a registry-lock deadlock; the body then closes and returns its reservation |
| Composed drain | HTTP shutdown plus RTC carrier close and the production dedicated-runtime stop refuses old/new HTTP openings and old carrier traffic, cancels fresh RTC admission on the stopped driver, joins every stream task within 5 s, fences in-flight output and returns both pools |

The idle-revocation assertion retains the task's 25 ms requirement. It first
measured 1 s mounted cleanup (15 s for the earlier body-only probe). The contained
HTTP fix gives the body an independent 25 ms authority tick and reaps expired
sessions when the body finishes. All 32 idle streams/session entries now end in
25 ms. The ordinary reaper interval and healthy 15 s heartbeat, 10 s stalled-write
deadline and 25 ms socket check remain unchanged. Reaped bindings close outside
the registry mutex; a deliberately reentrant notification tests that handoff
under an external real-time watchdog. The scripted authority invalidation
measures **local fences**, not hosted revocation propagation latency; carrier
closure is supplied explicitly.

Use these environment settings for every Rust command:

```sh
export CARGO_BUILD_JOBS=8
export CARGO_TARGET_DIR=/home/zech/Projects/.agent-scratch/977/target
export CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 CARGO_INCREMENTAL=0
nice -n 10 cargo test --manifest-path bridge/Cargo.toml --lib cross_transport -- --nocapture
nice -n 10 cargo test --manifest-path bridge/Cargo.toml --lib buffer_reservation_releases_family
```

`scripts/bridge-cross-transport-controls.sh` serially runs thirteen negative controls:
carrier admission, family admission, byte-pool charging, call cancellation,
reservation field drop order, authority recheck, HTTP shutdown retirement and
WebRTC driver shutdown, the idle wake, reentrant registry locking, completing
RTC before REST, swapping SSE pushes and adding a trailing duplicate. Each
copies exactly one source file, changes the mechanism, requires an executed
test failure (exit 101, not a compile error), restores the copy under an EXIT
trap and verifies it with `cmp`. Each control has an independent `timeout 900`.
Logs stay under task-977 scratch. Never edit this script while it runs.

The client qualification extends #976 as merged on dev at
`6c1f53f954bd60521c8ccfee26b09b22a1fa8b8d` (harness head
`804819b77f6b420a684bd60839c0b85922ad69f2`). Its `--scratch-task 977` option
restricts both the supplied binary and every disposable fixture to task-977
scratch. Task 977 uses a dedicated example that starts the real AppState and
HTTP supervisor through typed constructors, with no `BRIDGE_*` environment
variables. The existing default remains task 976. The runner never builds Rust.

```sh
nice -n 10 cargo build --manifest-path bridge/Cargo.toml --example bridge-http-qualification
BUILD_HTTP_QUALIFICATION_BINARY=/home/zech/Projects/.agent-scratch/977/target/debug/examples/bridge-http-qualification \
  nice -n 10 <qualification-venv>/bin/python \
  scripts/bridge-http-qualification/run.py --scratch-task 977
```

The SPA cases revoke a real issuer credential family before the bridge first
sees it, and hold eight actual streams of another family before the SPA opens
the ninth. Signed discovery responses remain unchanged. Both failures must
produce a visible sentence in the shipped Add by URL sheet, plus the actual
401/429 status. Screenshots and sanitized evidence omit credentials.

The desktop cases drive the actual native HTTP module and the actual SPA
desktop endpoint/session consumer, with only Electron's IPC process hop
represented locally. They check the real issuer DELETE/bridge 401, eight real
SSE hellos followed by a ninth-stream 429, and the consumer's plain error.
This does not qualify Electron window ownership or sandbox policy. Existing
unit tests cover those boundaries.

Functional discovery, browser and desktop groups each get a fresh owned runtime
process, preserving the disposable issuer/database but avoiding carryover of
the public signing limit (10 requests per source per 60 s). The production rate
limit is unchanged. `--client-group discovery|browser|desktop` isolates one
group while debugging. Simultaneous transport load is proved separately by the
Rust probes; no client-functional claim depends on resetting a quota mid-case.
