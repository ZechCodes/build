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
reach typed validation. GET resources accept only path selectors and no body
fields. No route aliases, redirects, absolute URLs, arbitrary Git commands,
method tunnel, terminal output/control or QA route is exported.

Run `nice -n 10 node scripts/http-resource-manifest.mjs` from the repository root
to regenerate `fixtures/api/http/{resource-manifest,openapi}.json`; `--check`
rejects drift. The browser imports the manifest; desktop copies the reviewed
artifact into its packaged source tree and checks equality at integration.
Manifest version 1 is a resource schema, not a bridge API version. Operation IDs
are exact method names with dots replaced by underscores. A full catalogue is
not proof of a mounted runtime: Opening.transport must contain only the actual
supported mounted operation IDs, plus resource_manifest_version:1,
terminal:'webrtc', durable_events:false. Clients reject incompatible versions
or partial required operation sets before enabling the HTTP connection.

The OpenAPI describes actual placement, headers, auth and wire envelopes. Its
domain parameter/result schemas are explicitly incomplete: the registry has no
schema reflection. Type names/source links come from `v1_method!`; examples are
existing wire fixtures checked against the compiled registry. Neither examples
nor opaque objects claim complete machine-readable parameter constraints.
Legacy entries link to their implementation until shared execution provides
their normalized types. Lifecycle/SSE routes are owned by #911 and remain a
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

#911 owns atomic expected-old stream replacement, the last-issued-handle
tombstone, one-use GET and exact-generation DELETE/late-close fences. HTTP has
no terminal stream; term.list is exposed solely as roster metadata required by
background cache sync. agent.attach and all terminal I/O remain WebRTC.

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
wire reservation and separate large-work concurrency permits held through
running work/cancellation cleanup. Boundary and streaming-body/concurrency tests
belong to that runtime. No second upload store or sync journal is introduced.

## Independent validation before runtime handoff

The small `contract/` Cargo package compiles this exact production module with
locked dependencies already present in the bridge, without editing Cargo/lib
files owned by #911. It tests every route and fixture translation, parser
rejections, conflict checks, metadata lookup and bounds. The bridge integration
contract test independently validates generated examples/unknown-param failures
against the real typed registry. The SPA coverage test scans actual sources,
including dynamic dispatch sites, without intersecting away unknown methods.
These tests do not claim listener/auth/admission/SSE integration evidence.
