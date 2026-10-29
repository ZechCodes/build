# HTTPS client adapter provenance

Task #912 starts at `fd2341443e0ffb88cdad7e4c9bb73ff53eb16269`, with source
base `12b8f869bd94e259666df69f1a8c0cde5c695982`. Its controlling architecture is
`planning/v2/Bridge Dual Transport Architecture Review.md` plus #916's final
application-binding disposition.

`fixtures/api/http/resource-manifest.json` was imported **from the committed git
object**, not a working tree, at coordinator-released #915 head
`79d60d9526e6d2acbe8a49bf307395c3bb45c4be` following #919 review. SHA-256:
`bcada2539a96e5784e864abf4e17564bb2d7260ac612f729db4dd5b7adbde616`.
The SPA imports that single canonical artifact; it has no independently maintained
REST method catalogue. `httpManifest.test.js` checks its bytes, all 147 operation
IDs, parameter placement, limits and explicit terminal/signaling/QA exclusions.
Future manifest updates require a reviewed exact-head handoff and updated pin.

The authenticated Opening must advertise all required operation IDs, resource
manifest version 1, `terminal: webrtc` and `durable_events: false`. Opening's
greeting establishes initial epoch authority. The application stream hello must
arrive before the actual `session.hello` resource activates binding-owned
producers. Logical subscriptions then settle successfully before ordinary reads
and actions are released. Current-state refresh follows; there is no durable
cursor, event replay or no-gap claim.

Stream loss/reset, authorization loss and uncertain stream creation close the
exact old execution Opening. Recovery opens a fresh one; it never retries an
ambiguous stream replacement or transfers pending actions to another transport.
Every request captures its original subscription handle. Terminal I/O uses the
separate WebRTC owner supplied by #909, with exact HTTP-session closure cleanup.

HTTP waits for final JSON; it does not synthesize admission receipts or call
`onReceipt`. A deadline after send reports uncertain delivery. Default request
deadline is 12 seconds; `workspace.get` and explicit `null`/`0` wait for caller
cancellation or actual completion, independently of authorization hard deadlines.
Durable write behavior remains governed by the existing per-method allowlist.

Only address/device/endpoint-epoch/family hints and explicit transport choice
persist in account-scoped UI storage. Tokens stay in browser memory or desktop
main. Every restored hint requires hosted renewal, including exact endpoint
binding. Relay presence controls WebRTC eligibility; an explicitly selected HTTPS
endpoint remains eligible for authenticated attempts independently of relay
presence. Account device removal and HTTP revocation/expiry still retire it.

The real HTTPS witness uses an isolated fixture server and an ephemeral CA,
exercises production browser adapter entry points, and separately verifies
untrusted-certificate refusal. It is not acceptance of the Rust runtime, hosted
CSP/CORS, browser local-network permission or native packaging. Those require
the reviewed integration slices and #913 qualification.
