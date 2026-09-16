// The authoritative lifetime of one paired device. Connection attempts are a
// short-lived concern; this owner begins where an attempt hands its session and
// peer connection over and ends when that exact lifetime is lost or retired.

import { deviceBlockedMark, deviceOfflineMark } from "./text.js";

export function createDeviceLifecycles() {
  const owners = new Map();

  return {
    forDevice(deviceId) {
      let owner = owners.get(deviceId);
      if (!owner) {
        owner = createOwner(deviceId);
        owners.set(deviceId, owner);
      }
      return owner;
    },

    existing(deviceId) {
      return owners.get(deviceId) || null;
    },

    retire(deviceId) {
      const owner = owners.get(deviceId);
      if (!owner) return null;
      owners.delete(deviceId);
      owner.retire();
      return owner;
    },

    clear() {
      const captured = [...owners.values()];
      owners.clear();
      for (const owner of captured) owner.retire();
    },

    securityStopText() {
      for (const owner of owners.values()) {
        if (owner.snapshot().securityStop) return owner.snapshot().securityStop;
      }
      return "";
    },

    clearSecurityStops() {
      for (const owner of owners.values()) owner.clearSecurityStop();
    },
  };
}

function createOwner(deviceId) {
  let state = "new";
  let bundle = null;
  let offlineSince = null;
  let blocked = null;
  let securityStop = null;

  const snapshot = () => ({
    state,
    session: bundle?.session || null,
    call: bundle?.session?.call || null,
    peerLink: bundle?.peerLink || null,
    offline: state !== "available",
    offlineSince,
    blocked,
    securityStop,
  });

  const isCurrent = (identity) =>
    Boolean(identity && state !== "retired" && bundle?.identity === identity);

  const detach = (mark) => {
    const departing = bundle;
    if (departing) departing.identity = null;
    bundle = null;
    state = mark.state;
    offlineSince = mark.offline ? mark.sinceMs || Date.now() : null;
    blocked = mark.offline ? mark.blocked || null : null;
    return departing;
  };

  const dispose = (departing, reason) => {
    if (!departing) return;
    safely(() => departing.session?.fail?.(reason));
    safely(() => departing.session?.close?.());
    safely(() => departing.onDetached?.());
    safely(() => departing.peerLink?.close?.());
  };

  const standDown = (identity, mark) => {
    if (state === "retired" || (securityStop && mark.state !== "refused")) return false;
    if (identity && !isCurrent(identity)) return false;
    const departing = detach(mark);
    dispose(departing, mark.reason);
    return true;
  };

  return {
    snapshot,

    adopt({ session, peerLink = null, onDetached = () => {} }) {
      if (state === "retired") throw new Error(`device lifecycle for ${deviceId} is retired`);
      if (securityStop) throw new Error(securityStop);
      const departing = bundle;
      if (departing) departing.identity = null;
      const identity = {};
      bundle = { session, peerLink, onDetached, identity };
      state = "available";
      offlineSince = null;
      blocked = null;
      dispose(departing, new Error("device connection replaced"));
      return {
        current: () => isCurrent(identity),
        lose: () => this.lose(identity),
        block: (reason) => this.block(identity, reason),
        identity,
      };
    },

    lose(lifetime = null) {
      const identity = lifetime?.identity || lifetime;
      return standDown(identity, unavailable("blocked", "lost"));
    },

    block(lifetime, reason) {
      const identity = lifetime?.identity || lifetime;
      return standDown(identity, unavailable("blocked", reason));
    },

    blockCurrent(reason) {
      return standDown(null, unavailable("blocked", reason));
    },

    presenceAway() {
      if (securityStop) return false;
      return standDown(null, unavailable("away", null));
    },

    refuse(message) {
      if (state === "retired") return false;
      securityStop ||= message;
      return standDown(null, unavailable("refused", "refused"));
    },

    retry() {
      if (securityStop || state === "retired") return false;
      if (state === "blocked") state = "away";
      blocked = null;
      return true;
    },

    setAvailability(mark = {}) {
      if (state === "retired" || securityStop) return false;
      const availability = projectedAvailability(mark, Boolean(bundle));
      state = availability.state;
      offlineSince = availability.offlineSince;
      blocked = availability.blocked;
      return true;
    },

    clearSecurityStop() {
      securityStop = null;
    },

    retire() {
      if (state === "retired") return;
      securityStop = null;
      const departing = detach({ state: "retired", offline: true, blocked: null });
      dispose(departing, new Error("device retired"));
    },
  };
}

function projectedAvailability(mark, hasBundle) {
  const offline = mark.offline ?? true;
  const reason = mark.blocked || null;
  if (!offline) return { state: hasBundle ? "available" : "new", offlineSince: null, blocked: null };
  return {
    state: reason ? "blocked" : "away",
    offlineSince: mark.sinceMs || Date.now(),
    blocked: reason,
  };
}

function unavailable(state, blocked) {
  return {
    state,
    offline: true,
    blocked,
    reason: new Error(blocked ? deviceBlockedMark : deviceOfflineMark),
  };
}

function safely(action) {
  try {
    action();
  } catch {
    // One broken resource must not strand the rest of the established bundle.
  }
}
