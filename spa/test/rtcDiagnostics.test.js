import { describe, expect, it } from "vitest";
import { candidateDiagnostic, directPairNoTryReason, safeCandidateReason } from "../src/core/rtcDiagnostics.js";

describe("candidate diagnostic privacy", () => {
  it("accepts known reasons, counters and phases while dropping arbitrary candidate content", () => {
    expect(candidateDiagnostic({
      reason: "mdns-unresolved", event: "mdns-unresolved", hostname: "private.local",
      candidates: { host_mdns: 1, host_ip: 0, mdns_unresolved: 1, relay: -1, srflx: 0.5, mdns_pending: Infinity, address: "192.168.1.2" },
    })).toEqual({ reason: "mdns-unresolved", phase: "mdns-unresolved", candidates: { host_mdns: 1, host_ip: 0, mdns_unresolved: 1 } });
  });

  it("rejects unknown reasons and never copies unknown event text", () => {
    expect(candidateDiagnostic({ reason: "192.168.1.2 failed" })).toBeNull();
    expect(candidateDiagnostic({ reason: "mdns-pending", event: "private.local" }))
      .toEqual({ reason: "mdns-pending", phase: "unknown", candidates: {} });
    expect(safeCandidateReason("private.local failed")).toBeNull();
  });

  it("uses the bridge's precise LAN-discovery reason while preserving browser check details", () => {
    expect(directPairNoTryReason("no-direct-pairs", "mdns-unresolved")).toBe("mdns-unresolved");
    expect(directPairNoTryReason("direct-checks-failed", "direct-checks-no-success")).toBe("direct-checks-failed");
    expect(directPairNoTryReason("direct-checks-pending", null)).toBe("direct-checks-pending");
  });
});
