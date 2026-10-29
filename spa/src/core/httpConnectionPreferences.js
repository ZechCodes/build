import { canonicalBridgeOrigin, httpFailure } from "./httpEndpoint.js";

const id = (value) => typeof value === "string" && value.length > 0 && value.length <= 128;
const clean = (identity, selected = "http") => {
  if (![identity.deviceId, identity.endpointEpoch, identity.familyId].every(id)) throw httpFailure("protocol");
  return { deviceId: identity.deviceId, origin: canonicalBridgeOrigin(identity.origin), endpointEpoch: identity.endpointEpoch, familyId: identity.familyId, selected };
};

export function createHttpConnectionPreferences({ accountId, read, write, isCurrent = () => true }) {
  let connections = new Map();
  let writes = Promise.resolve();
  const check = () => { if (!isCurrent()) throw httpFailure("scope_ended"); };
  const change = (edit) => {
    const next = writes.then(async () => {
      check();
      const proposed = edit(new Map(connections));
      if (proposed.size > 16) throw new Error("Up to 16 HTTPS device addresses can be selected in this document.");
      await write({ accountId, connections: [...proposed.values()] });
      check();
      connections = proposed;
    });
    writes = next.catch(() => {});
    return next;
  };
  return {
    async restore() {
      const saved = await read(); check();
      connections = new Map();
      if (saved?.accountId !== accountId || !Array.isArray(saved.connections)) return;
      for (const value of saved.connections.slice(0, 16)) {
        try {
          if (!["http", "webrtc"].includes(value.selected)) continue;
          const valid = clean(value, value.selected);
          connections.set(valid.deviceId, valid);
        } catch { /* Invalid and foreign hints supply no authority. */ }
      }
    },
    forDevice: (deviceId) => connections.get(deviceId) || null,
    all: () => [...connections.values()],
    selectHttp(identity) { const value = clean(identity); return change((next) => next.set(value.deviceId, value)); },
    selectWebRtc(deviceId) {
      return change((next) => {
        const previous = next.get(deviceId);
        if (previous) next.set(deviceId, { ...previous, selected: "webrtc" });
        return next;
      });
    },
    forget: (deviceId) => change((next) => { next.delete(deviceId); return next; }),
  };
}

// Eligibility to try an explicitly selected HTTPS transport, not a claim of
// reachability. Account list membership remains authoritative for retirement.
export const httpRecoveryDevices = (devices, preferenceFor) => devices.map((device) =>
  preferenceFor(device.id)?.selected === "http" ? { ...device, status: "online" } : device);
