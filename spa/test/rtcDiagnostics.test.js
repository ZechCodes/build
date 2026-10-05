import { describe, expect, it } from "vitest";
import { candidateDiagnostic, directPairNoTryReason, safeCandidateReason } from "../src/core/rtcDiagnostics.js";

const SCOUT_COUNTERS = ["scout_datagrams_sent", "scout_attempted", "destinations_scouted", "neighbors_pending", "neighbors_pending_peak"];
const baseSweep = { status: "progress", eligible: true, eligible_unresolved: 1, generation: 2, addresses_sent: 3, reason: null, prflx_followed: false };

describe("candidate diagnostic privacy", () => {
  it("keeps bounded per-host check counts while excluding addresses and arbitrary fields", () => {
    expect(candidateDiagnostic({
      reason: "direct-checks-no-reply", event: "direct-checks",
      hosts: [
        { ordinal: 1, requests_sent: 3, responses_received: 0, succeeded: false, address: "192.168.1.2:1234", client_id: "private" },
        { ordinal: 2, requests_sent: 7, responses_received: 1, succeeded: true },
        { ordinal: 3, requests_sent: -1, responses_received: 0, succeeded: false },
        { ordinal: 4, requests_sent: Infinity, responses_received: 0, succeeded: false },
        { ordinal: 5, requests_sent: 1, responses_received: 0.5, succeeded: false },
        { ordinal: 6, requests_sent: 1, responses_received: 0, succeeded: "private.local" },
        { ordinal: "private.local", requests_sent: 1, responses_received: 0, succeeded: false },
        { ordinal: 9999, requests_sent: 1, responses_received: 0, succeeded: false },
      ],
    })).toEqual({
      reason: "direct-checks-no-reply", phase: "direct-checks", candidates: {},
      hosts: [
        { ordinal: 1, requests_sent: 3, responses_received: 0, succeeded: false },
        { ordinal: 2, requests_sent: 7, responses_received: 1, succeeded: true },
      ],
    });
  });

  it("bounds the number of host observations copied from a diagnostics push", () => {
    const hosts = Array.from({ length: 100 }, () => ({ ordinal: 1, requests_sent: 1, responses_received: 0, succeeded: false }));
    expect(candidateDiagnostic({ reason: "direct-checks-pending", event: "direct-checks", hosts }).hosts).toHaveLength(64);
  });
  it.each(["direct-checks-pending", "direct-checks-not-sent", "direct-checks-no-reply", "direct-checks-succeeded"])(
    "preserves the precise bridge check result %s without copying endpoint data", (reason) => {
      expect(candidateDiagnostic({ reason, event: "generation", endpoint: "private.local:1234" }))
        .toEqual({ reason, phase: "generation", candidates: {} });
      expect(directPairNoTryReason("no-direct-pairs", reason)).toBe(reason);
    },
  );
  it("accepts known reasons, counters and phases while dropping arbitrary candidate content", () => {
    expect(candidateDiagnostic({
      reason: "mdns-unresolved", event: "mdns-unresolved", hostname: "private.local",
      candidates: { host_mdns: 1, host_ip: 0, mdns_unresolved: 1, relay: -1, srflx: 0.5, mdns_pending: Infinity, address: "192.168.1.2" },
    })).toEqual({ reason: "mdns-unresolved", phase: "mdns-unresolved", candidates: { host_mdns: 1, host_ip: 0, mdns_unresolved: 1 } });
  });

  it("recognizes the ordered generation marker without copying raw generation fields", () => {
    expect(candidateDiagnostic({ reason: "no-host-candidates", event: "generation", generation: "private.local", candidates: { mdns_resolved: 0 } }))
      .toEqual({ reason: "no-host-candidates", phase: "generation", candidates: { mdns_resolved: 0 } });
  });

  it("keeps only content-free host sweep evidence and numeric generation markers", () => {
    expect(candidateDiagnostic({ reason: "mdns-pending", event: "generation", generation: 2 }))
      .toEqual({ reason: "mdns-pending", phase: "generation", candidates: {}, generation: 2 });
    expect(candidateDiagnostic({
      reason: "mdns-unresolved", event: "host-sweep", candidates: { mdns_unresolved: 1 },
      sweep: { status: "stopped", eligible: true, eligible_unresolved: 2, generation: 2, addresses_sent: 253, addresses_attempted: 254, reason: "window-expired", prflx_followed: false,
        hostname: "private.local", endpoint: "192.168.1.2:1234", client_id: "private" },
    })).toEqual({
      reason: "mdns-unresolved", phase: "host-sweep", candidates: { mdns_unresolved: 1 },
      sweep: { status: "stopped", eligible: true, eligible_unresolved: 2, generation: 2, addresses_sent: 253, addresses_attempted: 254, reason: "window-expired", prflx_followed: false },
    });
  });

  it.each([
    { status: "private.local" }, { eligible: "192.168.1.2" }, { generation: -1 }, { generation: "private.local" },
    { addresses_sent: Infinity }, { addresses_sent: -1 }, { prflx_followed: "private" },
    { eligible_unresolved: undefined }, { eligible_unresolved: "private.local" }, { eligible_unresolved: -1 }, { eligible_unresolved: Infinity },
  ])("rejects malformed sweep evidence without letting it authorize a restart: %j", (invalid) => {
    expect(candidateDiagnostic({
      reason: "mdns-pending", event: "host-sweep",
      sweep: { status: "started", eligible: true, eligible_unresolved: 1, generation: 1, addresses_sent: 0, reason: null, prflx_followed: false, ...invalid },
    })).toEqual({ reason: "mdns-pending", phase: "host-sweep", candidates: {} });
  });

  it("does not copy arbitrary sweep reason text or sweep fields on other events", () => {
    const sweep = { status: "started", eligible: true, eligible_unresolved: 1, generation: 1, addresses_sent: 0, reason: "192.168.1.2 private.local", prflx_followed: false };
    expect(candidateDiagnostic({ reason: "mdns-pending", event: "host-sweep", sweep }).sweep)
      .toEqual({ ...sweep, reason: null });
    expect(candidateDiagnostic({ reason: "mdns-pending", event: "remote-candidate", sweep }))
      .toEqual({ reason: "mdns-pending", phase: "remote-candidate", candidates: {} });
  });

  it.each(["no-usable-addresses", "completed"])("preserves the fixed host sweep termination reason %s", (reason) => {
    const sweep = { status: "stopped", eligible: true, eligible_unresolved: 1, generation: 1, addresses_sent: 0, reason, prflx_followed: false, addresses_attempted: "private.local" };
    expect(candidateDiagnostic({ reason: "mdns-pending", event: "host-sweep", sweep }).sweep)
      .toEqual({ status: "stopped", eligible: true, eligible_unresolved: 1, generation: 1, addresses_sent: 0, reason, prflx_followed: false });
  });

  it("keeps distinct neighbor scout counters without copying content-bearing fields", () => {
    const counters = { scout_datagrams_sent: 12, scout_attempted: 15, destinations_scouted: 14, neighbors_pending: 1, neighbors_pending_peak: 2 };
    expect(candidateDiagnostic({
      reason: "mdns-pending", event: "host-sweep", generation: 2, candidates: { mdns_pending: 1 },
      sweep: { ...baseSweep, addresses_attempted: 4, ...counters, neighbor_address: "192.168.1.2", neighbor_state: "FAILED",
        interface_name: "wlan0", scout_port: 48861, hostname: "private.local", scout_payload: "private session" },
    })).toEqual({
      reason: "mdns-pending", phase: "host-sweep", generation: 2, candidates: { mdns_pending: 1 },
      sweep: { ...baseSweep, addresses_attempted: 4, ...counters },
    });
  });

  it.each(SCOUT_COUNTERS)("keeps valid optional %s independently of malformed scout fields", (counter) => {
    for (const invalid of [-1, 0.5, Infinity, NaN, Number.MAX_SAFE_INTEGER + 1, "private.local", null, {}, undefined]) {
      const others = Object.fromEntries(SCOUT_COUNTERS.filter((field) => field !== counter).map((field) => [field, 0]));
      expect(candidateDiagnostic({
        reason: "mdns-pending", event: "host-sweep", generation: 2,
        sweep: { ...baseSweep, [counter]: invalid, ...others },
      })).toEqual({
        reason: "mdns-pending", phase: "host-sweep", generation: 2, candidates: {},
        sweep: { ...baseSweep, ...others },
      });
    }
  });

  it("accepts older host sweep diagnostics without neighbor scout fields", () => {
    expect(candidateDiagnostic({ reason: "mdns-pending", event: "host-sweep", sweep: baseSweep }))
      .toEqual({ reason: "mdns-pending", phase: "host-sweep", candidates: {}, sweep: baseSweep });
  });

  it("preserves the ambiguous-interface fixed skip code without copying interface text", () => {
    expect(candidateDiagnostic({ reason: "mdns-pending", event: "host-sweep", sweep: { ...baseSweep, reason: "ambiguous-interface", interface_name: "wlan0" } }).sweep)
      .toEqual({ ...baseSweep, reason: "ambiguous-interface" });
  });

  it("rejects unknown reasons and never copies unknown event text", () => {
    expect(candidateDiagnostic({ reason: "192.168.1.2 failed" })).toBeNull();
    expect(candidateDiagnostic({ reason: "mdns-pending", event: "private.local" }))
      .toEqual({ reason: "mdns-pending", phase: "unknown", candidates: {} });
    expect(safeCandidateReason("private.local failed")).toBeNull();
  });

  it("preserves a fixed discovery rejection code while excluding raw error content", () => {
    expect(candidateDiagnostic({ reason: "mdns-unresolved", event: "mdns-unresolved", detail: "mdns-answer-rejected" }))
      .toEqual({ reason: "mdns-unresolved", phase: "mdns-unresolved", candidates: {}, discoveryReason: "mdns-answer-rejected" });
    expect(candidateDiagnostic({ reason: "mdns-unresolved", event: "mdns-unresolved", detail: "192.168.1.2 private.local was rejected" }))
      .toEqual({ reason: "mdns-unresolved", phase: "mdns-unresolved", candidates: {} });
  });

  it("uses the bridge's precise LAN-discovery reason while preserving browser check details", () => {
    expect(directPairNoTryReason("no-direct-pairs", "mdns-unresolved")).toBe("mdns-unresolved");
    expect(directPairNoTryReason("direct-checks-failed", "direct-checks-no-success")).toBe("direct-checks-failed");
    expect(directPairNoTryReason("direct-checks-pending", null)).toBe("direct-checks-pending");
  });
});
