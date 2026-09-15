# Stage 02 — Shrink the relay to rendezvous

Binding contract: spec rules 1, 4, 6 and the "Relay after this plan" table.

## Goal

Delete the relay's presence and transport-key duties, cut its frame and queue limits,
and make it obvious in the code that it is a signaling broker. No new message types.
`session_closed` stays (spec "What stays the same", open question 1).

## Context a cold agent needs

`bridge/src/bin/relay.rs` (726 lines) and `bridge/src/relay_server.rs` (1201, tests from
:625). The precise delete list, with current line numbers:

- `bin/relay.rs`: `device_offline` to displaced clients :371-376; `device_online` fan-out
  :377-381; `report_status(..., true)` :385; the `transport_key` arm :452-472;
  `device_offline` on teardown :501-504; `report_status(..., false)` :505; the `device_key`
  snapshot on client connect :548-558; `report_status` :709-719 and `internal_post` /
  `attach_internal_secret` :75-84 (keep `internal_get`); `websocket_limits()` :311-319
  reads `MAX_WS_MESSAGE_BYTES`; ratchets at :321 (`serve_device`, 32) and :508
  (`serve_client`, 26).
- `relay_server.rs`: `MAX_WS_MESSAGE_BYTES` :33-35 → `64 * 1024`; `MAX_OUTBOUND_QUEUE_BYTES`
  :43-47 → `1024 * 1024` (keep the overflow guard — it is the write-stall defence);
  `ConnectedDevice.transport_key` :332; `set_device_transport_key` :420-431;
  `device_keys_for_user` :450-458; `DeviceRegistration.displaced_clients/owner_clients`
  :352-356/:390-402/:416; `remove_device`'s outbound return :433-448 (return nothing);
  keep `remove_client`'s severed-session return :476-494 (feeds `session_closed`).
- Bridge relay client `bridge/src/relay.rs`: `transport_key` send in `authenticated`
  :273-279 and the module doc :9-15. `FrameIntake::transport_public_key` may then be
  unused — remove if so.
- Tests to rewrite or delete: `relay_server.rs` tests
  `transport_key_fanout_targets_only_owner_clients` :835,
  `removing_a_device_returns_only_owner_clients_to_notify` :865,
  `adding_a_device_reports_owner_clients_for_device_online_push` :1099,
  `reconnect_severs_stale_sessions_and_reports_their_clients` :1175 (keep the sever, drop
  the report), `outbound_queue_bounds_bytes...` :756 (adjust constant);
  `bridge/tests/relay_broker.rs`: `oversized_frames_close_the_connection` :283 (send 96 KiB,
  expect close; add a 48 KiB frame that passes), `device_online_and_offline_are_pushed...`
  :296 (delete), `one_client_sessions_to_multiple_devices...` :316 (drop the
  `device_online` asserts, keep ownership), every test that "greets on `device_online`"
  (:410, :448, :468, :522, :568, :653, :696) must instead proceed straight to
  `session_init`; `silent_device_is_severed_and_reported_offline` :435 and
  `device_that_stops_reading...` :548 assert severance by the device socket closing, not
  by a push; `bridge/tests/relay_wss.rs` :105/:150 waits for `transport_key` — remove.
  `bridge/examples/dev_relay.rs` :31, :119-121, :138-149 stores and re-pushes `device_key`
  — remove. `web/client.mjs` `awaitDeviceKey` breaks; stage 07 retires it, leave it.
- Other readers of `MAX_WS_MESSAGE_BYTES`: doc comments at
  `bridge/src/app/conversations/attachments.rs:26` and `bridge/src/gitgui/patches.rs:12`
  — reword ("the DataChannel reassembly cap, `MAX_REASSEMBLED_BYTES`"), and make
  `bridge/src/rtc/chunk.rs`'s `MAX_REASSEMBLED_BYTES` its own `8 * 1024 * 1024` literal if
  it currently aliases the relay constant.
- Ratchet: `bridge/tests/complexity_ratchet.rs:18` `RATCHETED_FUNCTIONS = 28` and the
  prose :12-17 ("2 each in the relay binary"). With the fan-out gone, bring `serve_device`
  and `serve_client` under 15 (extract `authenticate_device`, `device_frame`,
  `client_frame` helpers), delete both annotations, set the constant to 26.
- `deploy/k8s/relay.yaml:44` env comment mentions "status reports" — fix; memory limit
  :69-75 → `128Mi`; the `Recreate` comment :9-13 → "in-flight negotiations". Compose
  header comments :5 fine as is.

## What to build

Exactly the delete list above, in this order, one commit each: (1) limits + doc
comments, (2) presence/transport-key removal in `relay_server.rs` + its tests, (3) the
binary + integration tests + `dev_relay.rs` + `relay_wss.rs`, (4) bridge client
`transport_key` removal, (5) complexity extraction + ratchet retirement, (6) deploy yaml.
Do not touch the SPA or `web/`.

## Done when

`cargo test` (all relay tests), clippy, fmt green; `RATCHETED_FUNCTIONS == 26` and the
ratchet test passes; `grep -rn "device_key\|device_online\|device_offline\|transport_key"
bridge/src bridge/tests bridge/examples` returns nothing.
