# Stage 01 — Presence via a signed bridge heartbeat to the api

Binding contract: `planning/v2/Strict P2P Transport Spec.md` rule 6, "Bridge after this
plan" (`presence.rs`), "api after this plan". Read it first.

## Goal

The api owns presence. The bridge posts a device-signed heartbeat every 30 s; the api
derives `status` from `last_seen_at`; the relay's status POST goes away in stage 02, so
after this stage both writers coexist for one commit and nothing user-visible changes.

## Context a cold agent needs

- **The signed-request pattern to copy, exactly.** `bridge/src/transport_report.rs`
  (`report_challenge` :46-54, `build_transport_report` :57-76, `TransportReporter::start`
  :87-97, the drain task :116-125, `SystemTime` timestamp :128-131, `.post(url).json()`
  :139-146) and its api side `skriftapp/buildapp/transport_controller.py:90-141`:
  `read_json_object` → device approved and owned → rebuild challenge →
  `pairing_crypto.verify_registration` → `web_push.notify_timestamp_fresh` (5-min window)
  → `NotifyReplayGuard.check_and_record`. Helpers: `buildapp.request_body.read_json_object`,
  `buildapp.clock.utc_now`, `pairing_crypto.verify_registration`,
  `web_push.notify_timestamp_fresh`, `web_push.NotifyReplayGuard`. Tests for the pattern:
  `skriftapp/buildapp/tests/test_transport_controller.py` (or wherever transport tests
  live — find with `grep -rl "transport/report" skriftapp/buildapp`) and
  `bridge/tests/transport_report.rs`.
- **Device model** `skriftapp/buildapp/models.py:25-60`: `status` (`pending|online|offline`),
  `last_seen_at`. `device_summary` at `devices_controller.py:46-63` returns `status`.
  `/internal/devices/{id}/status` at `:276-290` is the relay's writer (delete in stage 02,
  not here). Other `status` writers: register → `pending` (:120), revoke → `offline` (:225).
- **Bridge wiring**: `bridge/src/main.rs` builds `TransportReporter::start(...)` at ≈:448
  and `Notifier::new(...)` at ≈:370 with `config.api_url` and the identity; the identity
  type is `relay::DeviceIdentity { device_id, identity_private_key_b64 }` (`relay.rs:79-84`).
- Repo rules: TDD, commit as you go, `cargo test && cargo clippy --all-targets -- -D warnings
  && cargo fmt --check`, `uv run --frozen ruff check buildapp && uv run --frozen pytest buildapp`,
  semgrep + gitleaks before each commit, complexity gates (C901 ≤ 10, clippy 15) — no new
  ratchet annotations.

## What to build

### api

1. `skriftapp/buildapp/presence.py` (pure, no framework imports): `ONLINE_WINDOW =
   timedelta(seconds=90)`, `heartbeat_challenge(device_id, timestamp) ->
   "heartbeat.{device_id}.{timestamp}"`, `derived_status(device, now) -> str` (`pending`
   until approved; `online` iff `last_seen_at` and `now - last_seen_at <= ONLINE_WINDOW`;
   else `offline`), and `heartbeat_timestamp_fresh = web_push.notify_timestamp_fresh`,
   `replay_guard()` like `transport_report.py:37-41`.
2. `POST /api/devices/heartbeat` in `devices_controller.py` — public route, body
   `{device_id, timestamp, signature_b64}`, the six-step verification sequence, sets
   `device.last_seen_at = utc_now()` and `device.status = "online"` (keep the column
   coherent), returns `{"ok": true}`. Reject unapproved/unowned with
   `NotAuthorizedException`, malformed with `ClientException`.
3. `device_summary` reports `status: presence.derived_status(device, utc_now())`.
   `onlineStickyDeviceId` and the SPA read only this field, so the relay's writer and the
   heartbeat agree from this commit on.
4. Tests: challenge string, fresh/stale timestamp, replay refused, unapproved refused,
   bad signature refused, `derived_status` at the window edges, `GET /api/devices`
   showing `offline` for a device whose `last_seen_at` is 2 minutes old even when the
   column says `online`.

### bridge

5. `bridge/src/presence.rs`: `HEARTBEAT_INTERVAL = 30 s`, `heartbeat_challenge`,
   `build_heartbeat(identity, timestamp) -> HeartbeatRequest {device_id, timestamp,
   signature_b64}`, `PresenceReporter::start(api_url, identity) -> JoinHandle` — sends one
   immediately, then every interval; a failed POST is logged at `warn` and the loop
   continues (never panics, never exits). Register the module in `lib.rs`; wire it in
   `main.rs` next to `TransportReporter::start`, behind the same "api_url configured"
   condition the reporter uses.
6. Tests: challenge string matches the api's byte-for-byte (write the expected string
   literally in both languages; there is no shared fixture yet — add
   `bridge/tests/fixtures/presence_challenge.txt` read by both test suites, mirroring how
   `agent_surfaces.json` is shared), signature verifies with the public key, an
   integration test in `bridge/tests/presence.rs` against a fake api that records the
   POST body and returns 200 then 500 and shows the loop survives.

## Done when

`GET /api/devices` reports `online` for a running bridge with the relay's status POST
removed by hand in a scratch run (or with `RELAY_INTERNAL_SECRET` wrong), the two test
suites and lints are green, and the commit lands.
