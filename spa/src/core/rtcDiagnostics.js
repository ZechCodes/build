// Only content-free reason names and counters can enter a shareable report.
const DIRECT_CHECK_REASONS = ["direct-checks-pending", "direct-checks-not-sent", "direct-checks-no-reply", "direct-checks-succeeded"];
const DISCOVERY_CHECK_REASONS = ["mdns-pending", "mdns-unresolved", "no-host-candidates", ...DIRECT_CHECK_REASONS];
const REASONS = new Set([...DISCOVERY_CHECK_REASONS, "direct-checks-no-success"]);
const EVENTS = new Set(["generation", "remote-candidate", "mdns-resolved", "mdns-unresolved", "direct-checks", "host-sweep"]);
const DISCOVERY_REASONS = new Set([
  "cached", "mdns-capacity", "mdns-retry", "mdns-candidate-rejected",
  "mdns-invalid-name", "mdns-interfaces-unavailable", "mdns-no-lan-interfaces",
  "mdns-socket-unavailable", "mdns-unresolved", "mdns-answer-rejected",
]);
const COUNTERS = new Set(["host_mdns", "host_ip", "srflx", "relay", "mdns_pending", "mdns_resolved", "mdns_unresolved"]);
const SWEEP_STATUSES = new Set(["started", "progress", "stopped", "skipped"]);
const SWEEP_REASONS = new Set([
  "subnet-too-large", "no-host-socket", "no-on-link-interface", "non-private-subnet", "invalid-netmask", "point-to-point",
  "unsupported-platform", "port-limit", "packet-limit", "window-expired", "resolved", "direct-selected", "generation-changed", "closed", "send-error",
  "no-usable-addresses", "completed", "ambiguous-interface",
  "neighbor-pressure", "neighbor-snapshot-unavailable", "scout-socket-limit",
  "nat-evidence-missing", "nat-address-mismatch", "interface-scout-cooldown",
]);
const OPTIONAL_SWEEP_COUNTERS = new Set([
  "addresses_attempted", "scout_datagrams_sent", "scout_attempted", "destinations_scouted", "neighbors_pending", "neighbors_pending_peak",
  "early_neighbors_probed", "scout_holds", "scout_starts",
]);
const HOST_LIMIT = 64;
const safeCount = (value) => Number.isSafeInteger(value) && value >= 0;
const safeHostCheck = (host) => host && safeCount(host.ordinal) && host.ordinal >= 1 && host.ordinal <= HOST_LIMIT
  && safeCount(host.requests_sent) && safeCount(host.responses_received) && typeof host.succeeded === "boolean";
const hostChecks = (hosts) => hosts.slice(0, HOST_LIMIT).filter(safeHostCheck).map((host) => ({
  ordinal: host.ordinal, requests_sent: host.requests_sent, responses_received: host.responses_received, succeeded: host.succeeded,
}));
const optionalSweepCounters = (sweep) => Object.fromEntries(Object.entries(sweep)
  .filter(([name, value]) => OPTIONAL_SWEEP_COUNTERS.has(name) && safeCount(value)));

function safeSweep(sweep) {
  if (!sweep || !SWEEP_STATUSES.has(sweep.status) || typeof sweep.eligible !== "boolean"
    || !safeCount(sweep.generation) || !safeCount(sweep.addresses_sent) || !safeCount(sweep.eligible_unresolved) || typeof sweep.prflx_followed !== "boolean") return {};
  return { sweep: {
    status: sweep.status, eligible: sweep.eligible, eligible_unresolved: sweep.eligible_unresolved, generation: sweep.generation,
    addresses_sent: sweep.addresses_sent, reason: SWEEP_REASONS.has(sweep.reason) ? sweep.reason : null,
    prflx_followed: sweep.prflx_followed,
    ...optionalSweepCounters(sweep),
  } };
}

export const safeCandidateReason = (reason) => REASONS.has(reason) ? reason : null;

export function candidateDiagnostic(push) {
  const reason = safeCandidateReason(push.reason);
  if (!reason) return null;
  const candidates = Object.fromEntries(Object.entries(push.candidates || {})
    .filter(([name, value]) => COUNTERS.has(name) && Number.isSafeInteger(value) && value >= 0));
  const discovery = DISCOVERY_REASONS.has(push.detail) ? { discoveryReason: push.detail } : {};
  const checks = push.event === "direct-checks" && Array.isArray(push.hosts) ? { hosts: hostChecks(push.hosts) } : {};
  const generation = safeCount(push.generation) ? { generation: push.generation } : {};
  const sweep = push.event === "host-sweep" ? safeSweep(push.sweep) : {};
  return { reason, candidates, phase: EVENTS.has(push.event) ? push.event : "unknown", ...discovery, ...checks, ...generation, ...sweep };
}

export function directPairNoTryReason(browserReason, bridgeReason) {
  return DISCOVERY_CHECK_REASONS.includes(bridgeReason)
    ? bridgeReason : browserReason;
}
