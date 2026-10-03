// Only content-free reason names and counters can enter a shareable report.
const REASONS = new Set(["mdns-pending", "mdns-unresolved", "no-host-candidates", "direct-checks-no-success"]);
const EVENTS = new Set(["remote-candidate", "mdns-resolved", "mdns-unresolved"]);
const COUNTERS = new Set(["host_mdns", "host_ip", "srflx", "relay", "mdns_pending", "mdns_resolved", "mdns_unresolved"]);

export const safeCandidateReason = (reason) => REASONS.has(reason) ? reason : null;

export function candidateDiagnostic(push) {
  const reason = safeCandidateReason(push.reason);
  if (!reason) return null;
  const candidates = Object.fromEntries(Object.entries(push.candidates || {})
    .filter(([name, value]) => COUNTERS.has(name) && Number.isSafeInteger(value) && value >= 0));
  return { reason, candidates, phase: EVENTS.has(push.event) ? push.event : "unknown" };
}

export function directPairNoTryReason(browserReason, bridgeReason) {
  return ["mdns-pending", "mdns-unresolved", "no-host-candidates"].includes(bridgeReason)
    ? bridgeReason : browserReason;
}
