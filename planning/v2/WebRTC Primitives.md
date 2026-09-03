# WebRTC Primitives

Status: design, 2026-09-02. The component list for `WebRTC Transport Spec.md`, which is binding.

Two rules govern every component below. Each is stated once, in the component that owns it; everything else defers to it
by name.

- **A carrier is a wire, not a protocol.** A session's key, frames, dispatch and teardown live above the wire; the relay
  socket and a DataChannel are two implementations of one narrow interface below it, and nothing above that line learns
  which is carrying. Signaling is the exception and is pinned to the relay: `rtc.offer` / `rtc.ice` / `rtc.close` never
  ride the channel they negotiate.
- **A session ends when its last carrier is gone, or when its client says so** (spec §Shared session registry). Owner:
  `SessionRegistry`. A relay `session_closed`, a DataChannel close and a bridge relay-socket drop are each *one carrier
  released*; only a client `close` frame ends a session outright. `SessionSwitch` is the same rule in the browser.

## Bridge (Rust)
### `OutboundEnvelope` and `CarrierHandle` — `bridge/src/carrier.rs` (new, stage 1)
`OutboundEnvelope` is one encrypted frame bound for one session, before any wire wrapper exists: a newtype over one
`Envelope`, whose `session_id` `encrypt_frame` already stamped and which `session_id()` reads back. It is what the
envelope-only outbound queue carries; the relay writer
merges that queue with its own control-message channel (`session_accept`, `heartbeat`, `Ping`, the auth reply), so
no `tungstenite::Message` reaches a `SessionSender`. A carrier reads `session_id()` off it and needs nothing else,
because one wire already carries every client session of the device, as the relay socket does today. `CarrierHandle` is one live wire: a process-unique `CarrierId` plus the
`UnboundedSender<OutboundEnvelope>` draining to it, built once per relay socket generation and once per DataChannel.
It **hides** that ids exist from every caller but the registry, and is what makes "which carriers does this session
ride" answerable — the question the teardown rule asks.

### `SessionSender` — moves `bridge/src/relay.rs` → `bridge/src/carrier.rs` (stage 1)
**Boundary** the app's handle for pushing to one session: owns the key and the frame encryption, no socket and no wire
wrapper. **Hides** the key, `encrypt_frame`, `route_to`, the carrier. **Interface** unchanged for every caller —
`session_id()`, `push(payload) -> bool`, `detached(session_id)`, and the test pair `observable` / `decrypt_push` (now
over `OutboundEnvelope`). The keyed constructor is `pub(crate)` and the only builder is `SessionRegistry::admit`, so no
call site outside the registry holds key material. **Replaces** today's WebSocket-typed sender: `TermScreen.attached`,
`AttachedClient`, `ChangeBus.subscribers` and every `app.rs` call site stop being typed on tungstenite, signatures
unchanged.

### `SessionRegistry` — `bridge/src/carrier.rs` (new, stage 1)
**Boundary** the one home of `session_id → (session_key, carriers riding it)`: `Arc`-shared, own leaf lock, built in
`main.rs`, outliving every socket. **Owns the teardown rule**, the only place it is written, and **hides** every session
key — a caller gets a decrypted frame and a sender, never the material.
```rust
pub fn open(&self, session_id: &str, session_key: String, carrier: &CarrierHandle) -> Result<(), CarrierError>;
pub fn admit(&self, envelope: &Envelope, carrier: &CarrierHandle) -> Result<(Frame, SessionSender), CarrierError>;
pub fn release_session(&self, session_id: &str, carrier: CarrierId) -> Vec<String>;
pub fn release_carrier(&self, carrier: CarrierId) -> Vec<String>;
pub fn end(&self, session_id: &str) -> Vec<String>;
```
The last three return the session ids that actually ended, from one private rule: *no carriers left, or `end`*. `admit`
records the ride, so a session rides a carrier the moment a frame for it arrives there. `open` is idempotent for a
known session whose unwrapped key matches — that is the carrier re-attach a browser's relay reconnect performs — and
errors on a known session with a different key, which is the frame the spec drops. **Replaces** the
`HashMap<String, String>` local to `relay::run`'s read loop, which died with the socket, and the key-removal half of
`relay::end_session`.

### `FrameIntake` — `bridge/src/carrier.rs` (new, stage 1)
**Boundary** "one envelope arrived on some carrier", whole job: admit it through the registry, honour a `close` frame,
else dispatch. Owns the `Dispatcher` — workers, ordered terminal lanes, read folding — as one `Arc` shared by the relay
loop and every DataChannel reader, and is the *effect* side of the teardown rule: for every session id the registry
reports ended it emits that session's synthetic `close` frame. **Hides** the dispatcher, the fold, the lanes.
```rust
pub fn new(registry: Arc<SessionRegistry>, handler: FrameHandler) -> Arc<Self>;
pub async fn accept(&self, envelope: Envelope, carrier: &CarrierHandle) -> Result<(), CarrierError>;
pub async fn close_session(&self, session_id: &str, carrier: CarrierId);
pub async fn close_carrier(&self, carrier: CarrierId);
```
`&self`, not `&mut self`, so the sharing is real: the lane map moves behind its own `Mutex` inside the `Dispatcher`,
held only long enough to clone a lane sender — **no lock crosses an await**. What still makes a flooding carrier wait is
the bounded job queue, the existing designed backpressure, shared on purpose. **Replaces** `relay::handle_envelope` and
`relay::end_session`, which stage 4 would otherwise copy into `rtc.rs`. It reads the session id off the envelope, so **a
carrier binds to no session**.

### `relay::run` — `bridge/src/relay.rs` (extended, stage 1)
Takes `Arc<FrameIntake>` where it took a `FrameHandler`, mints one `CarrierHandle` per socket generation, and calls
`close_carrier` when that socket ends. `session_init` → `registry.open`; `e2ee_envelope` → `intake.accept`;
`session_closed` → `intake.close_session`, one carrier released. Its writer task keeps the one job the wrapper move
leaves it: wrap each `OutboundEnvelope` as `{"type":"e2ee_envelope",…}`. `main.rs` builds registry and intake once and
passes the intake into every reconnect.

### `SessionPeer` — the peer-connection trait, `bridge/src/rtc.rs` (new, stage 2)
One implementor per live `RTCPeerConnection`, one per E2EE session, always the answerer. New; the bridge has no peer
concept today, so this replaces nothing.
```rust
#[async_trait]
pub trait SessionPeer: Send + Sync {
    async fn answer(&self, offer_sdp: &str, ice_servers: &[Value], signaling: SessionSender) -> Result<String, RtcError>;
    async fn add_remote_candidate(&self, candidate: Value) -> Result<(), RtcError>;
    async fn close(&self);
}
```
- **`answer` carries the ICE servers and is the only thing that does.** The first call configures the peer, a later one
  reconfigures it and restarts ICE — how fresh TURN credentials arrive. No second verb, no copy of the list elsewhere.
- **`signaling` is the caller's own sender, passed with every offer, never owned from construction.** It carries the
  `out` of the carrier the offer arrived on; a peer that captured one at construction would push `rtc.ice` candidates
  into a dead relay socket generation after the bridge reconnects. The peer keeps only the latest. Candidates trickled
  between a bridge relay reconnect and the next offer still go to the old generation; that is accepted because trickle
  finishes shortly after `answer` and an ICE restart re-answers with a fresh sender.
- **Hides** webrtc-rs, DTLS, ICE state, the negotiated channels, chunking, backpressure and the winning candidate-pair
  type; `app.rs` sees three async methods and an SDP string, so stage 2 ships against a recording stub.

### `SessionPeerFactory` — the single construction point, `bridge/src/rtc.rs` (stage 2)
```rust
pub trait SessionPeerFactory: Send + Sync {
    fn open(&self, session_id: &str) -> Result<Arc<dyn SessionPeer>, RtcError>;
}
```
The only place a peer implementation is chosen and built: `WebrtcPeerFactory::new(intake)` once in `main.rs`, handed to
`AppState`; tests build `RecordingPeerFactory`. The intake goes in because every DataChannel reader delivers through it.
**Hides** the crate, the ring provider, channel labels and ids.

### `rtc.offer` / `rtc.ice` / `rtc.close` — `bridge/src/app.rs::dispatch_frame` (stage 2)
Three arms beside `session.hello` (they need the caller's own `SessionSender`), each a parse and one call on the trait.
`AppState` gains `peers: HashMap<String, Arc<dyn SessionPeer>>` and a `peer_factory`, and no WebRTC knowledge.
**Extends** `AppState::drop_session`, which already releases terminals and subscriptions: it takes the session's peer
out of the map and spawns its `close` — safe because `drop_session` now runs only on a real session end, never on a bare
relay-socket loss.

### `chunk` — `bridge/src/rtc/chunk.rs` (new, stage 4)
Pure, no I/O, one exported pair: `chunk::split(envelope_json: &str) -> Vec<String>` (the input unchanged under
`CHUNK_BYTES`) and `chunk::Reassembler::accept(&mut self, text: &str) -> Result<Option<String>, ChunkError>`, erroring
on a gap or past `MAX_REASSEMBLED_BYTES`. **Hides** the `{"part":{id,index,count},"data":…}` shape — the one fact stated
twice, in two languages (`spa/src/core/chunk.js`), with the spec's table as its source.

### `DataChannelCarrier` — `bridge/src/rtc.rs` (stage 4)
**Boundary** one channel's two tasks over one `CarrierHandle`: a writer draining its
`UnboundedReceiver<OutboundEnvelope>` through `chunk::split`, parked while `buffered_amount > DC_BUFFERED_HIGH` and
woken by buffered-amount-low; a reader reassembling into `FrameIntake::accept`. A `ChunkError` closes the channel, as
the spec requires — a reassembly that lost a part cannot be resumed — and any close (that, ICE failure, DTLS close,
`rtc.close`) calls `FrameIntake::close_carrier`, where the teardown rule decides whether a session ended with it.
**Hides** chunking and backpressure; **extends** the relay writer task's job to a second wire. It registers no senders
itself: a migrating browser re-sends `session.hello` and re-attaches its terminals over the channel, and
`TermScreen::register` (`app.rs:394`) replaces the prior sender for that session id exactly as on a reconnect. That
refines the spec's "on channel open, register a DataChannel `SessionSender`" bullet — same registration, same
`app.rs:394`, reached at policy 3's re-attach, so a carrier still binds to no session.

## SPA (JavaScript)
### `Carrier` and `openCarrier` — the interface and its single construction point, `spa/src/core/carrier.js` (new, stage 5)
`Carrier` is `{ send(envelope), onEnvelope(fn), onClose(fn), close() }` — the spec's contract and nothing else; no
holder may ask which implementation it has. `openCarrier({ socket, sessionId })` wraps an authenticated relay socket and
`openCarrier({ channel })` wraps a DataChannel and its chunking. **Every `Carrier` in the SPA is built here** — by
`RelayLink` and `openPeerLink` — and the test picking the variant lives in this function and nowhere else. **Replaces**
the inline `e2ee_envelope` send and its arm of the message listener in `core/session.js` and `terminal/session.js`.

### `SessionRpc` — `spa/src/core/sessionRpc.js` (new, stage 5)
One E2EE session's crypto and correlation, over one carrier at a time. **Hides** the pending map, per-call timeouts,
encrypt/decrypt, push demux. **Interface** `call(method, params, { timeoutMs, carrier })`, `onPush(fn)`,
`rideOn(carrier)`, `sessionId`, `deviceId`, `close()`. `rideOn` swaps the wire and keeps the key — "one session, two
carriers" as one operation; `carrier` pins one call to a wire, which is how signaling stays on the relay. **Replaces**
`pending` + `call` in `openRelaySession` and `TerminalSocket._call` / `_demux`.

### `RelayLink` — `spa/src/core/relayLink.js` (new, stage 5)
**Boundary** one relay socket end to end: gateway token, `authenticate`, the device-key wait and pin check,
`session_init` / `session_accept`, presence pushes, and **its own backoff reconnect, run whether or not a DataChannel is
carrying**. **Hides** the handshake, the backoff, which socket generation is current. **Interface** `start()`,
`onSession(fn)`, `onDown(fn)`, `signal(method, params)`, `deviceId`, `close()`; `signal` is `SessionRpc.call` pinned to
the current relay carrier. It keeps this session's id and key across socket generations and re-presents the *same*
`session_init`, which `SessionRegistry::open` takes as a carrier re-attach; a relay that refuses is a hard error through
`onDown`. **Replaces** the handshake body of `openRelaySession` and `TerminalSocket._connect` and the reconnect duty of
`connection.js::resume()` and `TerminalSocket._onLost` — the two paths policy 6 could not use, each gated on offline.

### `SessionSwitch` — `spa/src/core/sessionSwitch.js` (new, stage 5)
Which carrier one session rides, and what runs on every change: the browser end of the teardown rule, written to match
`SessionRegistry`. **Hides** migration from both session modules — upgrade, fallback, relay reconnect and relay loss are
one code path in either direction. `createSessionSwitch({ session, onActive, onIdle })` →
`{ relay(carrier), peer(carrier), active(), close() }`, either slot taking `null` to clear; the rule is one line,
**active = peer ?? relay**. On every change to a live carrier it calls `session.rideOn(carrier)` then `onActive()`,
where `session.hello` and `_reattachAll()` live; with both slots empty, `onIdle()`. **Replaces** `goOffline()`'s trigger
and `_onLost`'s status and pending-rejection duties: `App.offline` becomes exactly "the app session's switch is idle",
so a relay loss under a live peer reaches neither it nor the bridge as a session end.

### `openPeerLink` — `spa/src/core/peerLink.js` (new, stage 5)
**Boundary** the upgrade, whole job in one call: `openPeerLink({ signal, fetchIceServers, RTCPeerConnectionImpl })`
builds the peer connection, creates the two negotiated channels (`app` id 0, `term` id 1), offers and trickles both ways
over `signal`, and settles once both open. Resolves `{ app, term, close() }` — two `Carrier`s — and rejects on any
failure, leaving the caller on the relay with no retry loop; `signal` is `relayLink.signal`, so every `rtc.*` RPC rides
the relay for the peer's life. **The ICE restart is event-driven and reads no TTL**: on the connection going
`failed` it fetches fresh ICE servers once and re-offers over `signal` on the same channels, one restart at a time, and
a failed one closes the link so `SessionSwitch` falls back to the relay. How long a credential lives has one home,
`TTL_SECONDS` in `ice_servers.py`; the spec's open question 1 stays open because nothing here depends on the answer.
**Hides** SDP, candidates, the `rtc.*` shapes, chunking, `bufferedAmountLow`. **Uses** `spa/src/core/chunk.js` (new) —
`splitEnvelope(text)`, `createReassembler()`, the mirror of `bridge/src/rtc/chunk.rs` with the same constants.

### The upgrade policy — `spa/src/connection.js`, `spa/src/terminal/manager.js` (extended, stage 5)
The one place the spec's ordered policy is written: a `RelayLink` per stream (app, terminal), as today's two sockets;
once the app session is live, fetch ICE servers, `openPeerLink`, then `appSwitch.peer(app)` and
`terminalSwitch.peer(term)`. A failure logs and stays on the relay until the next relay reconnect. **Extends** `api.js`
with `fetchIceServers()` and `terminal/manager.js` with the terminal's switch; `openRelaySession` keeps
`{ call, deviceId, close }` and `TerminalSocket` every method, so no `App.call` site changes.

## api (Python)
### `ice_servers` — `skriftapp/buildapp/ice_servers.py` (new, stage 3)
Pure logic plus one injectable sender, mirroring `web_push.py`:
```python
def ice_servers(key_id: str, api_token: str, ttl_seconds: int = TTL_SECONDS, send=requests.post) -> list[dict]
```
Cloudflare's array verbatim, or `STUN_ONLY` when no key is configured; an error is raised with context, never swallowed.
`TTL_SECONDS` lives here and nowhere else. **Hides** the Cloudflare URL, the request body, the no-key fallback; tested
against a fake `send`, so no test reaches the network.

### `RtcController` — `skriftapp/buildapp/rtc_controller.py` (new, stage 3)
One route: `@post("/api/rtc/ice-servers", guards=[auth_guard])` with `require_user(request)`, answering the array under
`iceServers`. Registered in `app.yaml`, `app.dev.yaml` and `app.mail.yaml` beside `DevicesController`; the existing
guard is the only auth. **Hides** `CF_TURN_KEY_ID` / `CF_TURN_KEY_API_TOKEN`, read from the environment (the `build-app`
Secret) and never returned. Sibling of `PushController`, replacing nothing.
