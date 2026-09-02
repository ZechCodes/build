# WebRTC Primitives

Status: design, 2026-09-02. The component list for `WebRTC Transport Spec.md`. The spec is binding:
where the two disagree the spec wins and this is wrong.

The rule: **a carrier is a wire, not a protocol.** Every fact about an E2EE session — its key, its
frames, its dispatch, its teardown — lives once, above the wire; the relay socket and a DataChannel
are two implementations of one narrow interface below it, and nothing above that line learns which
is carrying. **Signaling is the exception and is pinned to the relay**: `rtc.offer` / `rtc.ice` /
`rtc.close` never ride the channel they negotiate.

## Bridge (Rust)
### `OutboundEnvelope` — `bridge/src/carrier.rs` (new, stage 1)
- **Boundary** One encrypted frame bound for one session, before any wire wrapper exists:
  `{ session_id: String, envelope: Envelope }`.
- **Replaces** `tungstenite::Message` as what every outbound queue carries. A carrier reads
  `session_id` off it and needs nothing else, which is why one DataChannel carries the app session
  and the terminal session at once.

### `SessionSender` — moves `bridge/src/relay.rs` → `bridge/src/carrier.rs` (stage 1)
- **Boundary** The app's handle for pushing to one session: owns the key and the frame encryption,
  owns no socket and no wire wrapper. **Hides** the key, `encrypt_frame`, `route_to`, the carrier.
- **Interface** unchanged for every caller — `session_id()`, `push(payload) -> bool`,
  `detached(session_id)`, plus the test pair `observable` / `decrypt_push`. The keyed constructor is
  `pub(crate) fn new(session_id, session_key, UnboundedSender<OutboundEnvelope>)`; the only public
  builder is `SessionRegistry::sender`, so no call site outside the registry holds key material.
- **Replaces** today's WebSocket-typed sender: `TermScreen.attached`, `AttachedClient`,
  `ChangeBus.subscribers` and every `SessionSender` call site in `app.rs` stop being transitively
  typed on tungstenite. No signature there changes.

### `SessionRegistry` — `bridge/src/carrier.rs` (new, stage 1)
- **Boundary** The one home of `session_id → session_key`: `Arc`-shared, own leaf lock, built in
  `main.rs`, outliving every socket. **Hides** every session key — a carrier asks for a decrypted
  frame or a sender, never for the material.
  ```rust
  pub fn open(&self, session_id: &str, session_key: String);
  pub fn decrypt(&self, session_id: &str, envelope: &Envelope) -> Result<Frame, CarrierError>;
  pub fn sender(&self, session_id: &str, out: &UnboundedSender<OutboundEnvelope>) -> Option<SessionSender>;
  pub fn close(&self, session_id: &str) -> bool;
  ```
- **Replaces** the `HashMap<String, String>` local to `relay::run`'s read loop, which died with the
  socket. Its survival is what lets a session keep working over the peer path while the bridge's
  relay socket reconnects.

### `FrameIntake` — `bridge/src/carrier.rs` (new, stage 1)
- **Boundary** "One envelope arrived on some carrier", whole job: decrypt through the registry,
  honour a `close` frame by ending the session, else build the sender and dispatch. Owns the
  `Dispatcher` — workers, ordered terminal lanes, read folding — as one `Arc` shared by the relay
  loop and every DataChannel reader. **Hides** the dispatcher, fold, lanes, close-frame rule.
- **Interface** — `&self`, not `&mut self`, so the sharing is real:
  ```rust
  pub fn new(registry: Arc<SessionRegistry>, handler: FrameHandler) -> Arc<Self>;
  pub async fn accept(&self, envelope: Envelope, out: &UnboundedSender<OutboundEnvelope>) -> Result<(), CarrierError>;
  pub async fn end_session(&self, session_id: &str);
  ```
  The lane map moves behind its own `Mutex` inside the `Dispatcher`, held only long enough to clone
  a lane sender: **no lock crosses an await.** What still makes a flooding carrier wait is the
  bounded job queue — the existing designed backpressure, shared on purpose.
- **Replaces** `relay::handle_envelope` + `relay::end_session`, which stage 4 would otherwise copy
  into `rtc.rs`. It reads the session id off `envelope.session_id`, so a carrier binds to no session.

### `SessionPeer` — the peer-connection trait, `bridge/src/rtc.rs` (new, stage 2)
One implementor per live `RTCPeerConnection`, one per E2EE session, always the answerer.
```rust
pub trait SessionPeer: Send + Sync {                                          // #[async_trait]
    async fn answer(&self, offer_sdp: &str, ice_servers: &[Value]) -> Result<String, RtcError>;
    async fn add_remote_candidate(&self, candidate: Value) -> Result<(), RtcError>;
    async fn close(&self);
}
```
- **ICE restart is `answer` again.** A second `rtc.offer` on a live peer carries fresh credentials
  and restarts ICE through that same method: no second verb, no second state machine.
- **Hides** webrtc-rs, DTLS, ICE state, the two negotiated channels, chunking, backpressure, and the
  registration of DataChannel senders. `app.rs` sees three async methods and an SDP string, which is
  what lets stage 2 ship its handlers against a recording stub before stage 4 exists.

### `SessionPeerFactory` — the single construction point, `bridge/src/rtc.rs` (stage 2)
```rust
pub trait SessionPeerFactory: Send + Sync {
    fn open(&self, signaling: SessionSender, ice_servers: Vec<Value>) -> Result<Arc<dyn SessionPeer>, RtcError>;
}
```
- **Boundary** The only place a peer implementation is chosen and built.
  `WebrtcPeerFactory::new(registry, intake)` is built once in `main.rs` beside the registry and
  handed to `AppState`; tests build `RecordingPeerFactory`. The signaling sender goes in because a
  peer pushes its own `rtc.ice` candidates. **Hides** the crate, the ring provider, channel labels/ids.

### `rtc.offer` / `rtc.ice` / `rtc.close` — `bridge/src/app.rs::dispatch_frame` (stage 2)
- **Boundary** Three arms beside `session.hello` (they need the caller's own `SessionSender`), each a
  parse and one call on the trait. `AppState` gains `peers: HashMap<String, Arc<dyn SessionPeer>>`
  and a `peer_factory`, and no WebRTC knowledge.
- **Extends** `AppState::drop_session`, which already releases terminals and subscriptions: it closes
  the session's peer too, so a browser that vanishes takes its peer connection with it.

### `chunk` — `bridge/src/rtc/chunk.rs` (new, stage 4)
- **Boundary** Pure, no I/O, one exported pair: `chunk::split(envelope_json: &str) -> Vec<String>`
  (the input unchanged when it is under `CHUNK_BYTES`) and
  `chunk::Reassembler::accept(&mut self, text: &str) -> Result<Option<String>, ChunkError>`, erroring
  on a gap or past `MAX_REASSEMBLED_BYTES`.
- **Hides** the `{"part":{id,index,count},"data":…}` shape — the one fact stated twice, in two
  languages (`spa/src/core/chunk.js`), with the spec's table as its source.

### `DataChannelCarrier` — `bridge/src/rtc.rs` (stage 4)
- **Boundary** One channel's two tasks: a writer draining its `UnboundedReceiver<OutboundEnvelope>`
  through `chunk::split`, parked while `buffered_amount > DC_BUFFERED_HIGH` and woken by
  buffered-amount-low; a reader reassembling into `FrameIntake::accept`. **Hides** backpressure and
  chunking. It is the peer path's answer to the relay writer task, which keeps its job unchanged:
  wrap an envelope and send it.

## SPA (JavaScript)
### `Carrier` — the interface, `spa/src/core/carrier.js` (new, stage 5)
`{ send(envelope), onEnvelope(fn), onClose(fn), close() }` — the spec's contract and nothing else.
No holder may ask which implementation it has.

### `openCarrier` — the single construction point, `spa/src/core/carrier.js`
- **Boundary** `openCarrier({ socket, sessionId })` wraps an authenticated relay socket;
  `openCarrier({ channel })` wraps a DataChannel and its chunking. **Every `Carrier` in the SPA is
  built here** — by `RelayLink` and by `openPeerLink` — and the test that picks the variant lives in
  this function and nowhere else.
- **Replaces** the inline `ws.send(JSON.stringify({type:"e2ee_envelope",…}))` and the
  `e2ee_envelope` arm of the message listener in `core/session.js` and `terminal/session.js`.

### `SessionRpc` — `spa/src/core/sessionRpc.js` (new, stage 5)
- **Boundary** One E2EE session's crypto and correlation, over one carrier at a time. **Hides** the
  pending map, per-call timeouts, encrypt/decrypt, push demux.
- **Interface** `call(method, params, { timeoutMs, carrier })`, `onPush(fn)`, `rideOn(carrier)`,
  `sessionId`, `deviceId`, `close()`. `rideOn` swaps the wire and keeps the key — the spec's "one
  session, two carriers" as one operation. `carrier` pins a single call to a wire, which is how
  signaling stays on the relay.
- **Replaces** the request/reply halves of both session modules: `pending` + `call` in
  `openRelaySession`, and `TerminalSocket._call` / `_demux`.

### `RelayLink` — `spa/src/core/relayLink.js` (new, stage 5)
- **Boundary** One relay socket end to end: gateway token, `authenticate`, the device-key wait and
  pin check, `session_init` / `session_accept`, presence pushes, and **its own backoff reconnect, run
  whether or not a DataChannel is carrying.** **Hides** the handshake, the backoff, and which socket
  generation is current.
- **Interface** `start()`, `onSession(fn)`, `onDown(fn)`, `signal(method, params)`, `deviceId`,
  `close()`. On reconnect it re-presents the *same* `session_init`, so the id and key survive the
  socket and a live peer stays valid; a relay that refuses is a hard error through `onDown`, after
  which the app opens a fresh session as it does today. `signal` is `SessionRpc.call` pinned to the
  current relay carrier.
- **Replaces** the handshake body of `openRelaySession` and of `TerminalSocket._connect`, and the
  reconnect duty of `connection.js::resume()` and `TerminalSocket._onLost` — the two paths policy 6
  could not use, because each was gated on being offline.

### `SessionSwitch` — `spa/src/core/sessionSwitch.js` (new, stage 5)
- **Boundary** Which carrier one session rides, and what runs on every change. **Hides** migration
  from both session modules: upgrade, fallback, relay reconnect and relay loss are one code path in
  either direction.
- **Interface** `createSessionSwitch({ session, onActive, onIdle })` →
  `{ relay(carrier), peer(carrier), active(), close() }`; either slot takes `null` to clear. The rule
  is one line — **active = peer ?? relay**. On every change to a live carrier it calls
  `session.rideOn(carrier)` then `onActive()`, where `session.hello` and `_reattachAll()` live; with
  both slots empty it calls `onIdle()`.
- **Replaces** `goOffline()`'s trigger and `TerminalSocket._onLost`'s status and pending-rejection
  duties. `App.offline` becomes exactly "the app session's switch is idle", so a relay loss under a
  live peer never reaches it.

### `openPeerLink` — `spa/src/core/peerLink.js` (new, stage 5)
- **Boundary** The upgrade, whole job in one call:
  `openPeerLink({ signal, fetchIceServers, RTCPeerConnectionImpl })` builds the peer connection,
  creates the two negotiated channels (`app` id 0, `term` id 1), offers and trickles both ways over
  `signal`, and settles once both channels open. Resolves `{ app, term, close() }` — two `Carrier`s —
  and rejects on any failure, leaving the caller on the relay with no retry loop.
- **`signal` is `relayLink.signal`**, so every `rtc.*` RPC rides the relay for the life of the peer,
  never the channels it negotiates. It **owns the ICE restart**: one is scheduled before the
  credential TTL expires — fresh servers, a re-offer over `signal`, the same channels.
- **Hides** SDP, candidates, the `rtc.*` shapes, chunking, `bufferedAmountLow`. **Uses**
  `spa/src/core/chunk.js` (new) — `splitEnvelope(text)`, `createReassembler()`, the mirror of
  `bridge/src/rtc/chunk.rs` with the same constants.

### The upgrade policy — `spa/src/connection.js`, `spa/src/terminal/manager.js` (extended)
- **Boundary** The one place the spec's ordered policy is written: a `RelayLink` per stream (app,
  terminal), as today's two sockets; once the app session is live, fetch ICE servers, `openPeerLink`,
  then `appSwitch.peer(app)` and `terminalSwitch.peer(term)`. A failure logs and stays on the relay
  until the next relay reconnect.
- **Extends** `api.js` with `fetchIceServers()` (`POST /api/rtc/ice-servers`) and
  `terminal/manager.js` with the terminal's switch, which the `term` carrier is handed to.
  `openRelaySession` keeps `{ call, deviceId, close }` and `TerminalSocket` keeps every method, so no
  `App.call` site and no terminal pane changes.

## api (Python)
### `ice_servers` — `skriftapp/buildapp/ice_servers.py` (new, stage 3)
- **Boundary** Pure logic plus one injectable sender, mirroring `web_push.py`:
  ```python
  def ice_servers(key_id: str, api_token: str, ttl_seconds: int = TTL_SECONDS, send=requests.post) -> list[dict]
  ```
  Cloudflare's array verbatim, or `STUN_ONLY` when no key is configured; a Cloudflare error is raised
  with context, never swallowed.
- **Hides** the Cloudflare URL, the request body, the no-key fallback. Tested against a fake `send`;
  no test reaches the network.

### `RtcController` — `skriftapp/buildapp/rtc_controller.py` (new, stage 3)
- **Boundary** One route: `@post("/api/rtc/ice-servers", guards=[auth_guard])` with
  `require_user(request)`, answering the array under `iceServers`. Registered in `app.yaml`,
  `app.dev.yaml` and `app.mail.yaml` beside `DevicesController`; the existing guard is the only auth.
  **Hides** `CF_TURN_KEY_ID` / `CF_TURN_KEY_API_TOKEN`, read from the environment (the `build-app`
  Secret) and never returned.
