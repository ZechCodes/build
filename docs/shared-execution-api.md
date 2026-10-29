# Shared execution implementation interface

The controlling design remains the frozen #906 architecture review and the
#916 application stream and budget qualifications. This document describes the
implemented neutral boundary, not approval of an HTTP listener or codec.

`FrameIntake::execution()` returns the same `Arc<ExecutionService>` on every
call. Both intake and execution use its existing dispatcher, SQL workers,
observation workers, queued-read folding and ordered terminal lanes. Neither
adapter creates another application execution pool.

## Requests and results

- `Invocation::decode(method, &params)` uses the existing typed v1 registry.
  Existing legacy methods retain their original parameter object and domain
  validators; only object shape is checked at decode. Unknown legacy fields
  remain compatible. Signaling and generic method tunnels are excluded.
- `Invocation::with_metadata(RequestMetadata)` checks existing write metadata
  before any fast path. Metadata contains optional string request ID, typed
  `RequestPriority` and optional existing `WriteMetadata`. Request ID, priority
  and transport details are outside semantic operation identity. No durable
  method is enabled by this extraction; the existing allowlist remains empty.
- `ExecutionService::submit(RequestContext, Invocation).await` admits work and
  returns `Call`. It does not imply a durable acknowledgment. `Call::reply`
  observes the final application reply; adapters can instead await their own
  reply sink. Execution progresses without polling that convenience future.
- Retain `Call` through response completion. Dropping it cancels request
  interest and output, while running effects and deferred settlement finish.
  The worker retains the reply sink and its resource guards until actual
  completion. A canceled worker retains its shared opening request permit too.
  Legacy work already admitted to ordinary dispatch preserves its existing
  opening lifetime when one carrier closes. Read interest and output still
  require the exact live carrier; new RequestContext work additionally requires
  live request interest. Internal owner close cleanup runs after retirement.

## Captured RTC read interest

Trusted RTC adapters use the crate-restricted
`RequestContext::narrow_read_interest(self, &Opening, Arc<dyn Fn() -> bool + Send + Sync>)`
before submit. A different Opening allocation returns `WrongOpening` before the
callback is evaluated. The callback captures only the already verified exact
Opening, peer and channel proof; it cannot look up or rebind a session by ID.
Callbacks must be nonblocking and may not reenter the proof state.

Every restriction is conjoined, and an observed false result permanently retires
that request's read proof. Adding another callback cannot replace an earlier
restriction or revive it. The captured proof contains no request, reply sink or
budget guard. Default read interest is true for adapters without this extra
restriction.

Execution checks read interest at read admission and worker handoff, using the
existing effect registry. Authority, Opening lifetime and request cancellation
remain mandatory independently of the extra proof. Retiring only this proof
stops queued reads and read delivery while ordinary admitted work may settle.
The RTC adapter retains its ordinary Call until settlement and retires the
captured reply/event/peer proofs separately. Dropping the Call is request
cancellation, not wire-loss cleanup. Whole-Opening closure, authority expiry and
revocation override both read and ordinary-work liveness. Folded reads compute
for any surviving captured caller; one retired representative cannot hide it.
Producer event contexts strip this request-specific proof as well as reply
resources. Internal CLOSE-only owner cleanup remains separate from admission of
new effects.

## Authority and output

`ClientSessions` owns execution openings. Its public `open` and `resume` accept
opaque `VerifiedCaller` values, whose constructors are restricted to trusted
crate adapters. HTTP conversion supplies the verified issuer/account/device/
family/origin/endpoint/auth-epoch identity, monotonic deadline and a retained
live verifier callback. Relay conversion accepts the trusted device authority
and exact Opening allocation. Both map to the same `device_user` principal.
The execution module imports no HTTP verifier or cryptographic codec.

`resume` renews the same still-live HTTP opening only after exact identity
comparison; expired, revoked or explicitly closed openings cannot revive.
`AuthorizationLease` checks its deadline independently of verifier progress.
`request` defaults to application output; `request_for_stream` explicitly
selects another stream class. There are at most 32 unfinished requests per
opening, including canceled work that is still executing.

`ReplySink` and `EventSink` perform bounded nonblocking `try_send(EncodedEvent)`.
The event contains stamped application JSON and already encoded bytes. Use
`bytes()` directly and recheck `is_live()` before buffered native writes.
Successful enqueue does not mean client receipt or cache commit.

One Application EventBinding and an independent Terminal binding can coexist.
`bind_events` is initial-only. `replace_events` requires the exact predecessor,
including the latest closed tombstone; a stale predecessor cannot change the
current binding. Each request explicitly captures an optional current binding.
Event replacement cannot redirect old producers or cancel an independent reply.
HTTP external handles, one-use GET, header CAS and wire budgets belong to its
adapter. There is no terminal SSE or durable ChangeBus replay here.

Producer contexts strip request/reply resources. ChangeBus admission enforces
eight logical IDs for new session bindings atomically at subscription effect;
legacy carrier contexts preserve their existing limit behavior. Uploads and
subscription cleanup compare the exact opening and event binding. Closing a
binding detaches attachments, not the agent process or PTY. Terminal attachment
and child viewport effects serialize through a shared attachment gate; the exact
event proof is checked after acquiring it and again under the screen model lock.
Child I/O does not hold that model lock. writes.subscribe checks both request
and exact event liveness under its subscription registration lock.

## Lifecycle and greeting

`ExecutionService::greeting(&SessionLease)` returns captured authoritative base
metadata without registering producers or acquiring the mutable app guard.
An explicit `session.hello` with an exact application event context activates
update/model/harness producers; repeated activation upserts the same binding.
The SSE adapter itself only binds output and sends its transient stream hello.

The daemon reaps authorization expiry/revocation each second. Admission also
reaps dead openings. Neutral session close retires read/lane bookkeeping
without spawning an unbounded cleanup task per close. One coalesced blocking
cleanup pass releases dead subscriptions, uploads and terminal attachments.

## Adapter tests and integration limits

With the existing `testing` feature, `execution::testing::owner_session` supplies
an owner fixture without a carrier or encryption key. Tests can construct a real
AppState handler, obtain execution from FrameIntake, bind capture sinks and use
the normal Invocation and submit path. HTTP authentication acceptance must use
the actual verifier/runtime; this fixture is not authentication evidence.

The construction signatures are `AppState::handler(Arc<Mutex<AppState>>) ->
FrameHandler` and `ExecutionService::new(FrameHandler) -> Arc<ExecutionService>`.
Tests supply their existing isolated AppState fixture to the first constructor.
`FrameIntake::new(handler, transport_keys)` is the legacy carrier constructor;
`intake.execution()` reuses its service. HTTP-only tests can construct execution
directly without any carrier keys.

The normal adapter sequence, using a session obtained from the real verifier or
`testing::owner_session` in execution-only tests, is:

```rust
let sessions = execution.sessions();
let request = sessions.request(&session, reply_sink, Some(&application_binding))?;
let invocation = Invocation::decode(method, &params)?
    .with_metadata(RequestMetadata {
        request_id: Some(request_id),
        priority: RequestPriority::Foreground,
        write: None,
    })?;
let mut call = execution.submit(request, invocation).await?;
let reply = call.reply().await?;
// Retain call until the sink's encoded reply has finished its native write.
```

`ReplySink: Send + Sync` defines
`fn try_send(&self, event: EncodedEvent) -> Result<(), DeliveryError>`; EventSink
has the same signature. `Call::context()` exposes the captured RequestContext,
`Call::cancel()` cancels that request, and dropping Call has the same interest
retirement effect. `Call::reply(&mut self)` returns
`Result<serde_json::Value, CallError>` asynchronously; the adapter may instead
await its own final-event sink while retaining Call. `EncodedEvent: Clone` owns
encoded bytes and its proof, so response bodies can retain it directly rather
than serializing the Value again. Sinks must not call back into session methods
while synchronously enqueuing under an output gate.

The exported seam is exercised independently of HTTP runtime and clear-codec
wiring. Those adapters must be imported only at reviewed exact heads. The
coordinator owns final capability/version allocation and cross-transport load,
HTTP authentication, SSE recovery and codec qualification. No schema, wire
version, dependency, durable journal or enabled semantic write changes are made
by this boundary extraction.
