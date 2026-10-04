import { describe, expect, it } from "vitest";
import { candidateDiagnostic, directPairNoTryReason, safeCandidateReason } from "../src/core/rtcDiagnostics.js";

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
