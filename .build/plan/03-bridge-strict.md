# Stage 03 — The bridge refuses to be relayed, and signaling is carrier-neutral

Binding contract: spec rules 1 and 7, "Bridge after this plan" (relay.rs, carrier/dispatch.rs,
transport_ledger.rs), and the `transport_admin.py` bucket rename.

## Goal

Two enforcement points and one seam. After this stage a browser that tries to run app
RPC over the relay gets an error, and `session_accept` no longer depends on the relay
client's private control channel — the shape a future direct-network listener reuses.

## Context a cold agent needs

- `bridge/src/relay.rs` `open_session` :287-306 is the **only** emitter of
  `session_accept`, and it writes it to the relay writer's control channel, not through
  the `CarrierHandle`. `RelayConnection::accept` :243-264 dispatches four types.
- `bridge/src/carrier.rs`: `CarrierKind { Relay, Channel }` :202-205; `CarrierHandle`
  :215-230 carries `UnboundedSender<OutboundEnvelope>`; `FrameIntake::open` :507-522
  returns the accept envelope; `admit` :380-398; `SessionSender::push` :135-155.
  `OutboundEnvelope` is a newtype over one E2EE `Envelope`; the relay writer wraps it as
  `e2ee_envelope` (`relay.rs:353-362`) and a channel writes it raw (`rtc.rs:654-668`).
- **The carrier kind never reaches the dispatcher.** `Dispatcher::dispatch(sender, frame)`
  (`carrier/dispatch.rs:302`) gets only `(SessionSender, Frame)`; `CarrierKind` is private
  to `carrier.rs`. The one place holding both the decrypted `Frame` and `&CarrierHandle`
  before dispatch is `FrameIntake::accept` (`carrier.rs:527-539`: `admit` :532, the close
  check :533, dispatch :537). Put rule 1's check there, after the close check. The verb
  table is `app/rpc.rs:111` (`session.hello`) and :117-119 (`rtc.*`). The error reply
  shape is `Bridge Wire Protocol Spec.md` :420-437 (`ApiError` closed enum; use
  `unavailable` with `details.reason = "relay_is_not_a_data_plane"`, `retryable: false`) —
  find how `rpc.rs` builds an error reply and reuse it; a reply needs the frame's `id`.
- `bridge/src/transport_ledger.rs:33-48` `TransportEvent::FellBack` and `render` :76-;
  `note_carrier_left` in `carrier.rs:325-329` records it; `bridge/src/transport_report.rs`
  maps events to wire strings (`"fell_back"`); the api side
  `skriftapp/buildapp/transport_report.py` and `transport_controller.py` accept that string,
  `transport_admin.py:30-52` classifies `RELAY_ONLY` when `first_path is None` and
  `UNSTABLE` on `fell_back_count`; the template under `skriftapp/buildapp/templates/`
  (find with `grep -rn relay_only skriftapp`) prints the bucket.
- Tests: `bridge/tests/relay_end_to_end.rs`, `bridge/tests/rtc_peer.rs:302-331` (relay
  `session_closed` under a live channel), carrier unit tests in `carrier.rs:566-1243`.

## What to build

1. **Carrier-neutral `session_accept`.** Give `OutboundEnvelope` a second variant (or a
   sibling enum `Outbound { Envelope(Envelope), SessionAccept { session_id, envelope } }`
   — pick the smaller diff) so `FrameIntake::open(session_id, init, carrier)` **sends the
   accept itself** through `carrier.out` and returns `Result<(), CarrierError>`. The relay
   writer serialises the accept variant as today's `{"type":"session_accept",...}`; the
   channel writer serialises it as the same JSON object (a channel-borne init is not used
   yet but must not be unrepresentable). `relay.rs::open_session` shrinks to parse + call.
   Tests: `relay_end_to_end.rs` still sees `session_accept` on the socket; a unit test that
   an init over a `Channel` carrier yields the accept on that carrier's outbound.
2. **Rule 1 enforcement.** In the dispatcher, before routing: if the frame arrived on
   `CarrierKind::Relay` and `method` does not start with `rtc.`, reply with the standard
   error shape from the wire spec (`error_code: "unavailable"`, `retryable: false`,
   `details: {reason: "relay_is_not_a_data_plane"}`, `error` string) and `tracing::warn!` once per
   session (a `HashSet<session_id>` on the intake, cleared on session end). Do not
   dispatch. Tests: a `session.hello` over the relay carrier is refused; `rtc.offer` over
   the relay is dispatched; `session.hello` over a channel is dispatched; the closing
   `close` frame type is still honoured on the relay (`accept` :527-539 handles
   `CLOSE_FRAME_TYPE` before dispatch — keep it that way).
3. **Ledger rename.** `FellBack` → `ChannelsLost`, wire string `"channels_lost"`; the api
   accepts both strings for one release (`transport_report.py`), counts them in the same
   column (`fell_back_count` stays as the DB column — no migration), and
   `transport_admin.py` renames `RELAY_ONLY` → `NEVER_CONNECTED` (`"never_connected"`),
   template label "never connected". Update the api tests and `bridge/tests/transport_report.rs`.
4. Doc comments: `rtc.rs:5-8` ("Signaling is the one thing pinned to the relay carrier")
   and `:210-211` ("stays on the relay carrier") now say the client fails closed;
   `carrier.rs` module doc for the seam (rule 7, one paragraph).

## Done when

Rust suites, clippy, fmt, ruff, pytest green; no new ratchet; `grep -n "FellBack\|fell_back"
bridge/src` is empty; commits landed.
