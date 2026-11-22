# Explicit HTTP resources

This subtree owns placement and transport contracts, not authentication,
application handlers, scheduling, session lifecycle or SSE. `resources.tsv` is
the only resource catalogue. It maps closed resource/action endpoints to the
existing method names; no request chooses a method name. The existing typed
registry and execution service remain the only domain validator and executor.

The integration seam is:

```rust
operation(verb: &str, path: &str, query: Option<&str>)
    -> Result<&'static ResourceOperation, RouteError>
resolve(verb: &str, path: &str, query: Option<&str>, body: Value)
    -> Result<ResolvedRoute, RouteError>
// ResolvedRoute { method: &'static str, params: Value,
//                 operation: &'static ResourceOperation }
```

The runtime authenticates the device credential and binds the execution Opening
before either call. It uses `operation` to reserve the route's byte budget
before reading the body. Both calls use the same matcher. After bounded JSON
parsing, it resolves the route, calls `execution::Invocation::decode` and submits
with the trusted exact request/event context. A resolved route grants no
permission. Every shared error/detail, source/path precondition, operation
metadata and cancellation rule stays with shared execution/runtime.

Nonempty queries are rejected. Complex and nullable selectors use named POST
query resources with JSON. Path placeholders use existing selector names and
one URL-encoded segment each. The decoder decodes once and refuses malformed
encoding, separators, control characters and dot segments. A body cannot
contradict a path selector. All remaining fields, including unknown fields,
reach existing validation without new parameter rules. Registered v1 methods
reject unknown fields; six legacy methods preserve their existing unknown-field
compatibility, defaults and optional-field behavior. GET resources accept only path selectors and no body
fields. The REST catalogue excludes route aliases, redirects, absolute URLs,
arbitrary Git commands, method tunnels, interactive terminal operations and QA
routes.

Run `nice -n 10 node scripts/http-resource-manifest.mjs` from the repository root
to regenerate `fixtures/api/http/{resource-manifest,openapi}.json`; `--check`
rejects drift. The browser imports the manifest; desktop copies the reviewed
artifact into its packaged source tree and checks equality at integration.
Manifest version 1 is a resource schema, not a bridge API version. Operation IDs
are exact method names with dots replaced by underscores. A full catalogue is
not proof of a mounted runtime: Opening.transport must contain only the actual
supported mounted operation IDs, plus resource_manifest_version:1,
terminal:'webrtc', durable_events:false. The terminal WebSocket descriptor is
an additive transport member. Clients reject incompatible versions or partial
required operation sets before enabling the HTTP connection.

The OpenAPI describes actual placement, headers, auth and wire envelopes. Its
domain parameter/result schemas are explicitly incomplete: the registry has no
schema reflection. Type names/source links come from `v1_method!`; examples are
existing wire fixtures checked against the compiled registry. Neither examples
nor opaque objects claim complete machine-readable parameter constraints.
Legacy entries link to their implementation and have no declared parameter
types at this base. Their unknown fields remain compatible; existing domain
checks still apply. #908 preserves these objects at its shared execution seam
with an object-shape check, rather than silently tightening WebRTC semantics. Lifecycle/SSE routes are owned by #911 and remain a
separate contract. Shared API_VERSION allocation belongs to integration.

## Metadata and event binding

Resource replies retain existing `{id,ok,result,error,error_code,retryable,
details,bridge_epoch,write}` frames. Lifecycle replies are separate typed
objects. Metadata travels in Authorization, X-Build-Session,
X-Build-Request-Id (optional, runtime generates an ID if absent),
X-Build-Priority (user/background), Idempotency-Key and X-Build-After. The latter
two retain existing per-method write semantics; no universal acknowledgement,
automatic retry or exactly-once claim is introduced.

X-Build-Subscription is the sole event handle. `requires_events` is true only
for session.hello, changes.subscribe/unsubscribe, writes.subscribe and
fs.uploadBegin/Chunk/Finish/Abort. These methods capture exact event/output
bindings or binding-owned upload lifetimes. Stored tasks.attach/thread.attach
and writes broadcasting independent ChangeBus notifications do not require a
stream. Any supplied handle is checked, even for a pure read. Runtime rejects
missing required handles as stream_required, stale handles as stream_replaced
before dispatch, and passes the captured binding to shared execution. The
Opening's initial pure greeting is separate from exposed session.hello, which
registers update/model/harness producers and therefore requires a stream.

`runtime/subscriptions.rs` mounts the stream (#902): `POST /v1/subscriptions`
`{kind:"application"}` issues a handle, replacing only the last-issued one
named in X-Build-Subscription; one-use `GET .../events` binds the Opening's
Application stream to SSE, opening with `hello` (`durable:false`, no event IDs,
Last-Event-ID refused as cursor_unsupported); `DELETE`, replacement, Opening
close and failed grant checks end it. #911 owns atomic expected-old stream
replacement, the last-issued-handle
tombstone, one-use GET and exact-generation DELETE/late-close fences.
Application SSE carries changes and state. `term.list` stays on REST for
terminal roster reads, including background cache sync.

## Terminal WebSocket

The terminal WSS routes and Opening advertisement are gated until slice K.
Both routes are lifecycle endpoints on the verified HTTPS listener, outside
`resources.tsv`, the REST operation IDs and the Application SSE subscription.

| Route | Contract |
| --- | --- |
| `POST /v1/terminal-tickets` | Supply the device bearer with `bridge:read` and `bridge:write`, `X-Build-Session`, the allowed caller Origin and JSON `{"opening_generation":"…"}`. The 201 response is `no-store` and carries `ticket`, `expires_at` and `opening_generation`. A successful explicit Retry with `"retry":true` clears a latched snapshot write timeout on that Opening. |
| `GET /v1/terminal` | HTTP/1.1 WebSocket upgrade over TLS with the exact issuing Origin. Offer `build.term.v1` and `ticket.<token>`. A 101 selects only `build.term.v1` and binds the Opening's exact Terminal stream. The ticket is single-use and each new socket needs its own ticket. |

Both routes require the configured Host and a single explicitly allowed HTTPS
Origin. Upgrade checks Origin before handshake syntax or redemption.
Quota refusal returns 429 without spending a ticket. If shutdown wins after
redemption but before acceptance, the upgrade can return 503 with the ticket
already spent. A snapshot write timeout returns 409 `snapshot_write_timeout`
on ticket renewal until successful explicit Retry or a new Opening.

Opening POST and PATCH keep `terminal: "webrtc"`. With the gate on they also
advertise `terminal_websocket: {"version":1,"protocol":"build.term.v1"}`.
Wire 3.42.0 announces `term.websocket` as implementation support. Terminal
WSS selection requires the descriptor and the capability.

One binary message carries one UTF-8 JSON envelope. The socket carries
`term.create`, `term.attach`, `term.input`, `term.resize`, `term.ack`,
`term.close`, `agent.attach`, and liveness `ping`, plus terminal pushes.
`{"type":"detach","attach_id":"…"}` is a WSS-only control keyed to the
original attach request ID. It releases that socket's interest without
ending the shell or agent. An established interest can receive a
WSS-only `attachment_error` with the same attach ID and a reason.
See the [v1 fixture](../../../../fixtures/api/http/terminal-websocket-v1.json),
[shared execution contract](../../../../docs/shared-execution-api.md#terminal-websocket-adapter),
and [terminal security checklist](<../../../../planning/v2/Terminal WebSocket Security Checklist.md>).

## Bounds

Default request cap is 1,048,576 encoded JSON bytes; every response cap is
8,388,608 serialized frame bytes. Oversize replies fail explicitly before send,
never truncate. Media's 64 MiB limit is total file size; existing raw/range pages
are at most 1 MiB and remain usable. A legacy unranged media result may exceed
the HTTP cap and must fail explicitly. Runtime serializes into bounded output
once rather than retaining duplicate full serialized responses.

Only four request operations have exceptions, `4 * ceil(decoded_limit / 3) +
65,536` bytes, allowing base64 plus bounded JSON metadata:

| Operation | Existing decoded maximum | Encoded JSON cap |
| --- | ---: | ---: |
| tasks.attach | 5,242,880 | 7,056,044 |
| thread.attach | 5,242,880 | 7,056,044 |
| fs.uploadChunk | 4,194,304 | 5,657,944 |
| fs.write | 1,048,576 | 1,463,640 |

Each attachment request has one content_b64 string. thread.post can refer to
ten separately uploaded attachments; it does not carry ten inline payloads.
Existing decoded limits are still authoritative. The adapter byte limit does
not equate to resident memory: JSON strings, typed normalization/cloning and
decoded bytes can coexist. #911 owns the approved 32 MiB aggregate input/output
wire reservation and 22 MiB bulk sub-budget, preserving 10 MiB small-RPC headroom, plus
separate large-work concurrency permits (at most two globally and one per
Opening) held through running work/cancellation cleanup. Boundary and streaming-body/concurrency tests
belong to that runtime. No second upload store or sync journal is introduced.

## Independent validation before runtime handoff

The small `contract/` Cargo package compiles this exact production module with
locked dependencies already present in the bridge, without editing Cargo/lib
files owned by #911. It tests every route and fixture translation, parser
rejections, conflict checks, metadata lookup and bounds. The bridge integration
contract test independently validates generated examples/unknown-param failures
against the real typed registry (141 registered methods only). Legacy coverage
is separately distinguished from typed normalization evidence: six tests in
bridge/tests/http_resource_legacy.rs characterize the actual old FrameHandler
seam, with success/compatibility for reads and required/domain refusals for
agent controls without launching harnesses. They do not claim future Invocation
validation or successful agent process execution. The SPA coverage
test scans actual sources without intersecting away unknown methods. It is a
reviewed lexical inventory, not a whole-program JavaScript dataflow proof.
Metadata exceptions identify audited literal uses with multiplicity rather than
whole file/value exclusions. Exact source fingerprints conservatively
cover RPC-helper/metadata files so changed aliases and producers require an
explicit inventory review, even when call syntax stays unchanged. This also
means unrelated semantic edits in those files can require fingerprint review;
formatting/comment changes can also trigger the guard and must not be used to
justify blindly refreshing its inventory. New
or changed helper patterns require coverage review before enabling their views.
These tests do not claim listener/auth/admission/SSE integration evidence.
