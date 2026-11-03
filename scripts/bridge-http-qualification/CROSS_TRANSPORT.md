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
| Concurrent load | Four RTC sessions, 32 SSE streams, four families with eight streams each; 32 RTC and 32 REST calls complete; ninth family and 33rd global streams refuse with retryable `overloaded` |
| Separate admission | Saturating the production 1024-frame carrier queue returns `busy`; REST snapshot reads still finish with all streams admitted |
| Bounded buffering | Separate 32 MiB RPC and 32 MiB SSE allowances; at most 1 MiB/128 queued items per stream; all allowances returned after settlement |
| Cancellation | Carrier close and HTTP future cancellation fence output; running work retains its allowance until settlement; stream cancellation admits a same-family replacement |
| Reservation drop order | A synchronized blocked family cleanup cannot expose returned pool permits first; no sleep or scheduling race decides this assertion |
| Stalled revocation | All 32 pending writers end within 25 ms of local authority invalidation, alongside teardown of all four RTC carriers |
| Idle revocation | An already waiting stream must end within the same 25 ms without another event or heartbeat |
| Composed drain | HTTP shutdown plus RTC carrier close refuses old/new HTTP openings and old carrier traffic, joins every stream task within 5 s, fences in-flight output and returns both pools |

The idle-revocation assertion intentionally retains the task's 25 ms requirement.
It currently exposes a production gap: the body notices revocation at its 15 s
heartbeat unless another event wakes it. The mounted runtime's one-second reaper
can close it sooner, but does not establish a 25 ms guarantee. The 25 ms socket
authority timer applies to pending writes. No threshold is enlarged to turn
this test green. The scripted authority invalidation measures **local fences**,
not hosted revocation propagation latency; carrier closure is supplied explicitly.

Use these environment settings for every Rust command:

```sh
export CARGO_BUILD_JOBS=8
export CARGO_TARGET_DIR=/home/zech/Projects/.agent-scratch/977/target
export CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 CARGO_INCREMENTAL=0
nice -n 10 cargo test --manifest-path bridge/Cargo.toml --lib cross_transport -- --nocapture
nice -n 10 cargo test --manifest-path bridge/Cargo.toml --lib buffer_reservation_releases_family
```

`scripts/bridge-cross-transport-controls.sh` serially runs seven negative controls:
carrier admission, family admission, byte-pool charging, call cancellation,
reservation field drop order, authority recheck and shutdown retirement. Each
copies exactly one source file, changes the mechanism, requires an executed
test failure (exit 101, not a compile error), restores the copy under an EXIT
trap and verifies it with `cmp`. Each control has an independent `timeout 900`.
Logs stay under task-977 scratch. Never edit this script while it runs.

The client qualification extends #976 at
`804819b77f6b420a684bd60839c0b85922ad69f2`. Its `--scratch-task 977` option
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
