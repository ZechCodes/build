import { canonicalBridgeOrigin, httpFailure } from "./httpEndpoint.js";

const terminalModes = new Set(["webrtc", "wss", "protocol_error"]);
const id = (value) => typeof value === "string" && value.length > 0 && value.length <= 128;
const clean = (identity, selected = "http") => {
  if (![identity.deviceId, identity.endpointEpoch, identity.familyId].every(id)) throw httpFailure("protocol");
  return { deviceId: identity.deviceId, origin: canonicalBridgeOrigin(identity.origin), endpointEpoch: identity.endpointEpoch, familyId: identity.familyId, selected };
};

export function createHttpConnectionPreferences({ accountId, read, write, isCurrent = () => true }) {
  let connections = new Map();
  let modes = new Map();
  let writes = Promise.resolve();
  const check = () => { if (!isCurrent()) throw httpFailure("scope_ended"); };
  const change = (edit) => {
    const next = writes.then(async () => {
      check();
      const proposed = new Map(connections);
      const presentation = new Map(modes);
      if (edit(proposed, presentation) === false) return false;
      if (proposed.size > 16) throw new Error("Up to 16 HTTPS device addresses can be selected in this document.");
      const preferred = [...proposed.values()];
      const currentModes = new Map(preferred.filter((item) => presentation.has(item)).map((item) => [item, presentation.get(item)]));
      await write({ accountId, connections: preferred.map((item) =>
        currentModes.has(item) ? { ...item, terminalMode: currentModes.get(item) } : item) });
      check();
      connections = proposed;
      modes = currentModes;
      return true;
    });
    writes = next.catch(() => {});
    return next;
  };
  return {
    async restore() {
      const saved = await read(); check();
      connections = new Map();
      modes = new Map();
      if (saved?.accountId !== accountId || !Array.isArray(saved.connections)) return;
      for (const value of saved.connections.slice(0, 16)) {
        try {
          if (!["http", "webrtc"].includes(value.selected)) continue;
          const valid = clean(value, value.selected);
          connections.set(valid.deviceId, valid);
          if (valid.selected === "http" && terminalModes.has(value.terminalMode)) modes.set(valid, value.terminalMode);
        } catch { /* Invalid and foreign hints supply no authority. */ }
      }
    },
    forDevice: (deviceId) => connections.get(deviceId) || null,
    all: () => [...connections.values()],
    terminalModeFor(deviceId) {
      const preference = connections.get(deviceId);
      return preference?.selected === "http" ? modes.get(preference) ?? "webrtc" : undefined;
    },
    rememberTerminalMode(preference, mode, current) {
      return change((next, presentation) => {
        if (!current() || preference.selected !== "http" || next.get(preference.deviceId) !== preference) return false;
        if (!terminalModes.has(mode) || presentation.get(preference) === mode) return false;
        presentation.set(preference, mode);
        return true;
      });
    },
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
