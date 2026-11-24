import { canonicalBridgeOrigin, httpFailure } from "./httpEndpoint.js";

const terminalModes = new Set(["webrtc", "wss", "protocol_error"]);
const id = (value) => typeof value === "string" && value.length > 0 && value.length <= 128;
const clean = (identity, selected = "http") => {
  if (!id(identity.deviceId)) throw httpFailure("protocol");
  const { deviceId, endpointEpoch, familyId, fingerprint } = identity;
  return {
    deviceId, origin: canonicalBridgeOrigin(identity.origin), selected,
    ...(id(endpointEpoch) && id(familyId) && { endpointEpoch, familyId }),
    ...(id(fingerprint) && { fingerprint }),
  };
};
export const sameHttpDevice = (setting, identity) => setting.deviceId === identity.deviceId
  && setting.origin === identity.origin && (!setting.fingerprint || setting.fingerprint === identity.fingerprint);

export function createHttpConnectionPreferences({ accountId, read, write, isCurrent = () => true }) {
  let connections = new Map();
  let modes = new Map();
  let writes = Promise.resolve();
  const check = () => { if (!isCurrent()) throw httpFailure("scope_ended"); };
  const change = (deviceId, edit, expected = null) => {
    const next = writes.then(async () => {
      check();
      const proposed = new Map(connections);
      const presentation = new Map(modes);
      if (edit(proposed, presentation) === false) return false;
      if (proposed.size > 16) throw new Error("Up to 16 HTTPS device addresses can be selected in this document.");
      const preferred = [...proposed.values()];
      const currentModes = new Map(preferred.filter((item) => presentation.has(item)).map((item) => [item, presentation.get(item)]));
      const stored = { accountId, connections: preferred.map((item) =>
        currentModes.has(item) ? { ...item, terminalMode: currentModes.get(item) } : item) };
      if (await write(stored, { deviceId, expected }) === false) return false;
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
      return change(preference.deviceId, (next, presentation) => {
        if (!current() || preference.selected !== "http" || next.get(preference.deviceId) !== preference) return false;
        if (!terminalModes.has(mode) || presentation.get(preference) === mode) return false;
        presentation.set(preference, mode);
        return true;
      }, preference);
    },
    async rememberAuthority(preference, identity, current = isCurrent) {
      if (!sameHttpDevice(preference, identity)) throw httpFailure("wrong_bridge");
      const verified = clean(identity);
      const changed = await change(preference.deviceId, (next, presentation) => {
        if (!current() || next.get(preference.deviceId) !== preference || preference.selected !== "http") return false;
        next.set(preference.deviceId, verified);
        if (presentation.has(preference)) presentation.set(verified, presentation.get(preference));
        return true;
      }, preference);
      return changed ? verified : null;
    },
    selectHttp(identity) { const value = clean(identity); return change(value.deviceId, (next) => next.set(value.deviceId, value)); },
    selectWebRtc(deviceId) {
      return change(deviceId, (next) => {
        const previous = next.get(deviceId);
        if (previous) next.set(deviceId, { ...previous, selected: "webrtc" });
        return next;
      });
    },
    forget: (deviceId) => change(deviceId, (next) => { next.delete(deviceId); return next; }),
  };
}

// Eligibility to try a selected HTTPS transport, not a claim of
// reachability. Account list membership remains authoritative for retirement.
export const httpRecoveryDevices = (devices, usesHttp) => devices.map((device) =>
  usesHttp(device.id) ? { ...device, status: "online" } : device);
