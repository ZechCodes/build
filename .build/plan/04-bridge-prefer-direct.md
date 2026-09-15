# Stage 04 — Prefer direct: mDNS, interface policy, relay acceptance delay

Binding contract: spec rule 8 and "Bridge after this plan" (`rtc.rs` config).

## Goal

The bridge's ICE agent is tuned so that a browser on the same LAN or Tailscale network
connects host-to-host, and TURN only wins when nothing direct can. This is also the
groundwork for the direct-network mode (spec rule 7): the same knobs, with
`direct-only`, are what that mode will run with.

## Context a cold agent needs

- `bridge/src/rtc.rs`: `WebrtcPeer::connect` :372-405 builds
  `PeerConnectionBuilder::new().with_configuration(...).with_data_channel_send_buffer_limit(...)
  .with_handler(...).with_udp_addrs(vec![GATHER_FROM])` with `GATHER_FROM = "0.0.0.0:0"` :254.
  **No `SettingEngine` is used anywhere.** `offered_server` :593-608 maps the browser's
  `ice_servers` list. `answer` :316-345 builds `RTCConfiguration` from that list per offer.
- Crate: `webrtc` 0.20.x. Knobs (paths in `~/.cargo/registry/src/*/`):
  `PeerConnectionBuilder::with_setting_engine` (`webrtc-0.20.4/src/peer_connection/mod.rs:234`),
  `SettingEngine::set_ice_multicast_dns_mode`, `set_network_types`, `set_interface_filter`,
  `set_ip_filter`, `set_include_loopback_candidate`, `set_host_acceptance_min_wait`,
  `set_srflx_acceptance_min_wait`, `set_relay_acceptance_min_wait`, `set_nat_1to1_ips`
  (`rtc-0.20.4/src/peer_connection/configuration/setting_engine.rs`). Verify each exists in
  the pinned version before relying on it; `cargo doc` or read the source.
- The test harness `bridge/src/rtc/testing.rs` builds a "browser" peer in-process (uses
  `RTCIceTransportPolicy` at :90-94). `bridge/tests/rtc_peer.rs` is the integration suite.
  `report_negotiated_path` :465-488 and `NegotiatedPath` :497-558 already classify the
  nominated pair; a test can assert `Direct` with candidate types `host/host`.
- Config surface today: `bridge/src/main.rs` module doc :9-29 lists every `BRIDGE_*` env;
  `env(name, default)` helper at ≈:95. `RuntimeConfig` (≈:178) is where a parsed
  `IcePolicy` belongs; `WebrtcPeerFactory::new(intake)` at `main.rs:461-463` is where it is
  handed to `rtc.rs`.

## What to build

1. `rtc::IcePolicy { mode: All | DirectOnly, relay_min_wait: Duration, interfaces:
   Option<Vec<String>> }` parsed from `BRIDGE_ICE_POLICY` (`all` default, `direct-only`),
   `BRIDGE_ICE_RELAY_MIN_WAIT_MS` (default 1500, `0` disables), `BRIDGE_ICE_INTERFACES`
   (comma list; absent = every non-loopback interface). Pure parse function with unit
   tests for each value and each bad value (bad value → startup error, fail fast).
2. `WebrtcPeerFactory::new(intake, policy)`; `connect` builds a `SettingEngine`:
   `set_ice_multicast_dns_mode(MulticastDnsMode::QueryOnly)`,
   `set_network_types([Udp4, Udp6])`, `set_include_loopback_candidate(false)`,
   `set_relay_acceptance_min_wait(Some(policy.relay_min_wait))`, and
   `set_interface_filter` when `interfaces` is set. Under `DirectOnly`: `offered_server`
   drops any `turn:`/`turns:` URL before building the configuration, and the
   configuration's `ice_transport_policy` stays `All` (STUN still helps a Tailscale peer
   learn its own address); document why in a comment.
3. Tests in `bridge/tests/rtc_peer.rs`: (a) two in-process peers connect host/host and the
   ledger records `Direct` with `host/host` detail; (b) `DirectOnly` with a TURN URL in the
   offer never passes it to the configuration (assert via a factory hook or by inspecting
   `offered_server` output); (c) an interface allow-list that names nothing existing
   yields an answer with no host candidates and the peer fails within the deadline
   (proves the filter is applied — skip with a printed reason on CI if no interface
   enumeration is possible). mDNS resolution cannot be unit-tested without a browser;
   document it as verified in stage 06's browser pass.
4. `main.rs` module doc and `README.md` "Develop"/bridge env section gain the three
   variables with one line each; `deploy/README.md` gets a "LAN / Tailscale" paragraph:
   set `BRIDGE_ICE_POLICY=direct-only` on the bridge when browser and bridge share a
   network.

## Done when

Rust suites, clippy, fmt green; no new ratchet; commits landed.
