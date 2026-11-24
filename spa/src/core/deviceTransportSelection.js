import { canonicalBridgeOrigin } from "./httpEndpoint.js";

const webrtc = Object.freeze({ kind: "webrtc", key: "webrtc" });
const hold = (reason) => ({ kind: "blocked", key: "blocked", reason });

function httpsSelection(device, transport) {
  try {
    const origin = canonicalBridgeOrigin(transport.origin);
    if (origin !== transport.origin) return hold("invalid_origin");
    const key = JSON.stringify([transport.source, origin, transport.assignment_revision, device.fingerprint]);
    return { kind: "http", key, origin, source: transport.source, fingerprint: device.fingerprint, wssOnly: true };
  } catch {
    return hold(transport.reason || "setup_pending");
  }
}

function legacySelection(preference) {
  if (preference?.selected !== "http") return webrtc;
  return { kind: "http", key: JSON.stringify(["manual", preference.origin]),
    origin: preference.origin, wssOnly: false };
}

const bySource = new Map([
  ["hosted", (device, transport) => !transport.origin && transport.rtc_allowed === true
    ? webrtc : httpsSelection(device, transport)],
  ["personal", (device, transport, preference) => transport.rtc_allowed === false
    ? httpsSelection(device, transport) : permittedLegacySelection(transport, preference)],
  ["webrtc", (_device, transport, preference) => transport.rtc_allowed === false
    ? hold(transport.reason || "setup_pending") : permittedLegacySelection(transport, preference)],
]);

function permittedLegacySelection(transport, preference) {
  if (transport.rtc_allowed !== true) return hold("policy_unknown");
  return transport.reason ? webrtc : legacySelection(preference);
}

export function selectDeviceTransport(device, preference, policyAware) {
  const transport = device?.transport;
  if (!transport) return policyAware ? hold("policy_unknown") : legacySelection(preference);
  const select = bySource.get(transport.source);
  return select ? select(device, transport, preference) : hold("policy_unknown");
}

export function transportReasonText(reason) {
  switch (reason) {
    case "policy_unknown": return "Waiting for Build to refresh this device’s connection policy.";
    case "update_required": return "Update the Build bridge to finish HTTPS setup.";
    case "invalid_origin": return "The bridge reported an invalid HTTPS address.";
    case "setup_pending": return "Waiting for this device’s HTTPS setup to finish.";
    default: return `HTTPS setup is unavailable (${reason}).`;
  }
}

export const terminalCompatibilityText = new Map([
  ["unknown", "HTTPS terminal support has not been checked yet."],
  ["wss", "Terminal over HTTPS"],
  ["update_required", "Update the Build bridge or desktop app to use HTTPS terminals."],
]);
