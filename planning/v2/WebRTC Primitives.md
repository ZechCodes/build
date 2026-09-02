# WebRTC Primitives

Status: design, 2026-09-02. The component list for `WebRTC Transport Spec.md`. The spec
is binding: where the two disagree the spec wins and this is wrong.

The rule: a carrier is a wire, not a protocol. Every fact about an E2EE session — its
key, its frames, its dispatch, its teardown — lives once, above the wire, and the relay
socket and the DataChannel are two implementations of one narrow interface below it.
Nothing above that line learns which one is carrying.

## Bridge (Rust)

### `OutboundEnvelope` — `bridge/src/carrier.rs` (new)

- **Boundary** One encrypted frame bound for one client session, before any wire wrapper
  exists: `{ session_id: String, envelope: Envelope }`.
- **Replaces** `tungstenite::Message` as what every outbound queue carries.

### `SessionSender` — moves `bridge/src/relay.rs` → `bridge/src/carrier.rs`

- **Boundary** The app's handle for pushing to one client session: owns the session key
  and the frame encryption, owns no socket and no wrapper.
- **Interface** unchanged for every caller — `session_id()`, `push(payload) -> bool`,
  `detached(session_id)`, and the test pair `observable` / `decrypt_push`. Construction
  becomes `new(session_id, session_key, UnboundedSender<OutboundEnvelope>)`.
- **Hides** The session key, `encrypt_frame`, `route_to`, and now the carrier.
- **Replaces** today's WebSocket-typed sender: `TermScreen.attached`, `AttachedClient`,
  `ChangeBus.subscribers` and 100+ call sites stop being typed on tungstenite.

### `SessionRegistry` — `bridge/src/carrier.rs` (new)

- **Boundary** The one home of `session_id → session_key`, shared by both carriers and
  outliving any socket. `Arc`-shared, own leaf lock, built in `main`.
- **Interface**
  ```rust
  pub fn open(&self, session_id: &str, session_key: String);
  pub fn decrypt(&self, session_id: &str, envelope: &Envelope) -> Result<Frame, CarrierError>;
  pub fn sender(&self, session_id: &str, out: &UnboundedSender<OutboundEnvelope>) -> Option<SessionSender>;
  pub fn close(&self, session_id: &str) -> bool;
  ```
- **Hides** Every session key: a carrier asks for a decrypted frame or a sender, never
  for the material.
- **Replaces** the relay read loop's local map, which died with the socket.

### `FrameIntake` — `bridge/src/carrier.rs` (new)

- **Boundary** "One envelope arrived on some carrier", whole job: decrypt through the
  registry, honour a `close` frame by ending the session, else build the sender and
  dispatch. Owns the `Dispatcher` — workers, terminal lanes, read folding — as one
  instance shared by both carriers.
- **Interface**
  ```rust
  pub fn new(registry: Arc<SessionRegistry>, handler: FrameHandler) -> Self;
  pub async fn accept(&mut self, session_id: &str, envelope: Envelope,
                      out: &UnboundedSender<OutboundEnvelope>) -> Result<(), CarrierError>;
  pub async fn end_session(&mut self, session_id: &str);
  ```
- **Hides** The dispatcher, the fold, the lanes, and the close-frame rule.
- **Replaces** `relay::handle_envelope` + `end_session`, which stage 4 would otherwise
  copy into `rtc.rs`.

### `SessionPeer` — the peer-connection trait, `bridge/src/rtc.rs` (new, stage 2)

One implementor per live `RTCPeerConnection`, one per session, always the answerer.

```rust
#[async_trait]
pub trait SessionPeer: Send + Sync {
    async fn answer(&self, offer_sdp: &str) -> Result<String, RtcError>;
    async fn add_remote_candidate(&self, candidate: Value) -> Result<(), RtcError>;
    async fn close(&self);
}
```

- **Hides** webrtc-rs, DTLS, ICE state, the two negotiated channels, chunking,
  backpressure and the registration of DataChannel senders — `app.rs` sees three async
  methods and an SDP string. It is also the seam that lets stage 2 ship handlers against
  a recording stub, before stage 4 exists.

### `SessionPeerFactory` — the single construction point, `bridge/src/rtc.rs`

```rust
pub trait SessionPeerFactory: Send + Sync {
    fn open(&self, sender: SessionSender, ice_servers: Vec<Value>) -> Result<Arc<dyn SessionPeer>, RtcError>;
}
```

- **Boundary** The only place a peer variant is chosen and built. `WebrtcPeerFactory`
  (stage 4) is constructed once in `main.rs` beside the registry and handed to
  `AppState`; tests construct `RecordingPeerFactory`. The sender goes in because a peer
  pushes its own `rtc.ice` candidates and registers the senders it opens.
- **Hides** The crate, the ring provider, the channel labels and ids.

### `rtc.offer` / `rtc.ice` / `rtc.close` — `bridge/src/app.rs::dispatch_frame`

- **Boundary** Three arms beside `session.hello` (they need the caller's own
  `SessionSender`), each a parse and one call on the trait. `AppState` gains a
  `peers` map keyed by session id and a `peer_factory`, and no WebRTC knowledge.
- **Extends** the existing teardown: `drop_session` closes the peer, so a browser that
  vanishes takes its peer connection with it on the path that already releases terminals
  and subscriptions.

### `EnvelopeChunker` / `ChunkReassembler` — `bridge/src/rtc.rs`

- **Boundary** Pure, no I/O. `split(envelope_json, CHUNK_BYTES) -> Vec<String>`,
  unchunked below the limit; `Reassembler::accept(&mut self, text: &str) ->
  Result<Option<String>, ChunkError>`, erroring on a gap or past `MAX_REASSEMBLED_BYTES`
  — which its caller turns into a channel close.
- **Hides** The `{"part":{id,index,count},"data":…}` shape — the one fact stated twice,
  in two languages (see `chunk.js`), with the spec's table as its source.

### `DataChannelCarrier` — `bridge/src/rtc.rs`

- **Boundary** One channel's two tasks: a writer draining its
  `UnboundedReceiver<OutboundEnvelope>` through the chunker, parked while
  `buffered_amount > DC_BUFFERED_HIGH`, woken by buffered-amount-low; a reader
  reassembling into `FrameIntake::accept`.
- **Hides** Backpressure and chunking. It is the peer path's answer to the relay writer
  task, which keeps its job: wrap an envelope and send it.

## SPA (JavaScript)

### `Carrier` — the interface, `spa/src/core/carrier.js` (new)

`{ send(envelope), onEnvelope(fn), onClose(fn), close() }` — the spec's contract and nothing else; no holder may ask which implementation it has.

### `relayCarrier(ws, sessionId)` — `spa/src/core/carrier.js`

- **Boundary** A `Carrier` over an authenticated relay socket: `send` writes
  `{type:"e2ee_envelope", session_id, envelope}`, `onEnvelope` fires for inbound ones
  and ignores control frames.
- **Replaces** the inline `ws.send(JSON.stringify(…))` and the `e2ee_envelope` arm in
  both `core/session.js` and `terminal/session.js`.

### `openPeerLink({ call, iceServers, RTCPeerConnectionImpl })` — `spa/src/core/peerLink.js` (new)

- **Boundary** The single construction point for the DataChannel carrier variant, whole
  job in one call: build the peer connection, create the two negotiated channels (`app`
  id 0, `term` id 1), `call("rtc.offer", …)`, trickle both ways, settle once both are
  open. Resolves `{ app, term, close() }`, two `Carrier`s; rejects on offer/ICE failure,
  and the caller stays on the relay with no retry loop.
- **Hides** SDP, candidates, the `rtc.*` RPCs, chunking, `bufferedAmountLow`.
- **Uses** `spa/src/core/chunk.js` (new) — `splitEnvelope(text)`, `createReassembler()`,
  the mirror of the Rust chunker, same constants and shape.

### `CarrierSwitch` — `spa/src/core/carrierSwitch.js` (new)

- **Boundary** Which carrier one session speaks on, and what happens when it closes.
  Holds a base carrier (relay) and an optional active one.
- **Interface** `send(envelope)`, `onEnvelope(fn)`, `use(carrier, onActive)`,
  `isLive()`, `close()`. A `use`d carrier's close reverts to the base and runs its
  `onActive` — where `session.hello` and `_reattachAll()` live — so migration in either
  direction is one code path.
- **Hides** Migration from both session modules: neither knows a channel exists.

### `openRelaySession` — `spa/src/core/session.js` (extended)

Keeps `{ call, deviceId, close }` and gains `useCarrier(carrier)`. The handshake still
runs on the relay socket; after it every `call` writes through the `CarrierSwitch`,
whose `onEnvelope` feeds today's demux. What it no longer holds is the socket write.

### `TerminalSocket` — `spa/src/terminal/session.js` (extended)

Every method unchanged, `useCarrier(carrier)` added, `_call` and `_demux` moved onto the
`CarrierSwitch`, `_reattachAll()` becoming the `onActive` hook a switch runs rather than
a step of `_connect`. The liveness ping and `FRAME_PROOF_OF_LIFE_MS` measure the active
carrier; `_onLost` still means no carrier left.

### The upgrade policy — `spa/src/connection.js` (extended)

- **Boundary** The only place the spec's ordered policy lives: after `adoptSession`,
  fetch ICE servers, `openPeerLink` over the live session, hand `app` to the session's
  switch and `term` to `terminalManager()`'s. A failure logs and stays on the relay
  until the next relay reconnect.
- **Extends** `App.offline` to mean no carrier is live, so relay loss with live channels
  is not offline; `api.js` gains `fetchIceServers()` (`POST /api/rtc/ice-servers`) and
  `terminal/manager.js` one export, `useTerminalCarrier(carrier)`.

## api (Python)

### `ice_servers` — `skriftapp/buildapp/ice_servers.py` (new)

- **Boundary** Pure logic plus one injectable sender, mirroring `web_push.py`:
  ```python
  def ice_servers(key_id: str, api_token: str, ttl_seconds: int = TTL_SECONDS, send=requests.post) -> list[dict]
  ```
  Cloudflare's array verbatim, or `STUN_ONLY` when no key is configured; a Cloudflare
  error is raised with context, never swallowed.
- **Hides** The Cloudflare URL, the request body, the no-key fallback. Tested against a
  fake `send`; no test reaches the network.

### `RtcController` — `skriftapp/buildapp/rtc_controller.py` (new)

- **Boundary** One route: `@post("/api/rtc/ice-servers", guards=[auth_guard])` with
  `require_user(request)` as the belt-and-suspenders read, answering the array under
  `iceServers`. Registered in `app.yaml`, `app.dev.yaml` and `app.mail.yaml` beside
  `DevicesController`; the existing guard is the only auth.
- **Hides** `CF_TURN_KEY_ID` / `CF_TURN_KEY_API_TOKEN`, read from the environment (the
  `build-app` Secret) and never returned.
