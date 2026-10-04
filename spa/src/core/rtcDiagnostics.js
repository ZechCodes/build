// Only content-free reason names and counters can enter a shareable report.
const DIRECT_CHECK_REASONS = ["direct-checks-pending", "direct-checks-not-sent", "direct-checks-no-reply", "direct-checks-succeeded"];
const DISCOVERY_CHECK_REASONS = ["mdns-pending", "mdns-unresolved", "no-host-candidates", ...DIRECT_CHECK_REASONS];
const REASONS = new Set([...DISCOVERY_CHECK_REASONS, "direct-checks-no-success"]);
const EVENTS = new Set(["generation", "remote-candidate", "mdns-resolved", "mdns-unresolved", "direct-checks"]);
const DISCOVERY_REASONS = new Set([
  "cached", "mdns-capacity", "mdns-retry", "mdns-candidate-rejected",
  "mdns-invalid-name", "mdns-interfaces-unavailable", "mdns-no-lan-interfaces",
  "mdns-socket-unavailable", "mdns-unresolved", "mdns-answer-rejected",
]);
const COUNTERS = new Set(["host_mdns", "host_ip", "srflx", "relay", "mdns_pending", "mdns_resolved", "mdns_unresolved"]);
const HOST_LIMIT = 64;
const safeCount = (value) => Number.isSafeInteger(value) && value >= 0;
const safeHostCheck = (host) => host && safeCount(host.ordinal) && host.ordinal >= 1 && host.ordinal <= HOST_LIMIT
  && safeCount(host.requests_sent) && safeCount(host.responses_received) && typeof host.succeeded === "boolean";
const hostChecks = (hosts) => hosts.slice(0, HOST_LIMIT).filter(safeHostCheck).map((host) => ({
  ordinal: host.ordinal, requests_sent: host.requests_sent, responses_received: host.responses_received, succeeded: host.succeeded,
}));

export const safeCandidateReason = (reason) => REASONS.has(reason) ? reason : null;

export function candidateDiagnostic(push) {
  const reason = safeCandidateReason(push.reason);
  if (!reason) return null;
  const candidates = Object.fromEntries(Object.entries(push.candidates || {})
    .filter(([name, value]) => COUNTERS.has(name) && Number.isSafeInteger(value) && value >= 0));
  const discovery = DISCOVERY_REASONS.has(push.detail) ? { discoveryReason: push.detail } : {};
  const checks = push.event === "direct-checks" && Array.isArray(push.hosts) ? { hosts: hostChecks(push.hosts) } : {};
  return { reason, candidates, phase: EVENTS.has(push.event) ? push.event : "unknown", ...discovery, ...checks };
}

export function directPairNoTryReason(browserReason, bridgeReason) {
  return DISCOVERY_CHECK_REASONS.includes(bridgeReason)
    ? bridgeReason : browserReason;
}
