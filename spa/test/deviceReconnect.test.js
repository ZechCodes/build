// When one machine can answer again: the subscription a surface waits on after
// a read was lost with the wire, instead of polling for the wire to come back.

import { beforeEach, describe, expect, it, vi } from "vitest";
import { ATTEMPTING, WAITING } from "../src/core/connectionStatusModel.js";

let context = null;
let recovery = null;
const stateListeners = new Set();
const recoveryListeners = new Set();

vi.mock("../src/core/deviceContexts.js", () => ({
  contextFor: () => context,
  canAnswer: (held) => Boolean(held && held.call && !held.offline),
  onDeviceStateChanged: (fn) => {
    stateListeners.add(fn);
    return () => stateListeners.delete(fn);
  },
}));

// A stand-in for the app's spine answers everything the app asks it, not only
// the two this module reads (test/mockedExports.test.js).
vi.mock("../src/connection.js", () => ({
  deviceRecoverySnapshot: () => recovery,
  onDeviceRecoveryChanged: (fn) => {
    recoveryListeners.add(fn);
    return () => recoveryListeners.delete(fn);
  },
  forgetHomeFollow: () => {}, forgetRendezvousSockets: () => {}, forgetSecurityStops: () => {},
  connectDevice: async () => null, deviceWentAway: () => {},
  openDeviceSessions: () => ({ first: Promise.resolve(null), settled: Promise.resolve([]) }),
  syncDeviceRecoveryPresence: () => {},
  // The wake listeners the gate arms and disarms (#60).
  watchForWake: () => {},
  stopWatchingForWake: () => {}, syncHome: () => {},
  openDeviceSettingsSession: async () => ({}), retireDevice: () => {},
  securityStopText: () => "", chooseCreationDevice: () => {},
}));

const { deviceIsAway, deviceIsReconnecting, deviceWatch, onDeviceMoved, onDeviceReachable } = await import(
  "../src/core/deviceReconnect.js"
);

const announce = () => [...stateListeners, ...recoveryListeners].forEach((fn) => fn());
const live = () => ({ call: () => {}, offline: false });

beforeEach(() => {
  context = null;
  recovery = null;
  stateListeners.clear();
  recoveryListeners.clear();
});

describe("whether a machine can answer", () => {
  it("is away with no context, an offline one, or no session under it", () => {
    expect(deviceIsAway("dev-1")).toBe(true);
    context = { call: () => {}, offline: true };
    expect(deviceIsAway("dev-1")).toBe(true);
    context = { call: null, offline: false };
    expect(deviceIsAway("dev-1")).toBe(true);
  });

  // The session under a context mid-reconnect is the one that just died: a
  // read sent into it fails the way the last one did.
  it("is away while something is being done about it, context or no context", () => {
    context = live();
    for (const status of [ATTEMPTING, WAITING]) {
      recovery = { status };
      expect(deviceIsReconnecting("dev-1")).toBe(true);
      expect(deviceIsAway("dev-1")).toBe(true);
    }
  });

  // #130: a link restarting a failed path in place leaves the supervisor with
  // no record — the session is still held — and the ring reads the link.
  it("is reconnecting while its link restarts the path in place", () => {
    let restoring = true;
    context = { ...live(), peerLink: { restoring: () => restoring } };
    expect(deviceIsReconnecting("dev-1")).toBe(true);
    expect(deviceIsAway("dev-1")).toBe(true);
    restoring = false;
    expect(deviceIsReconnecting("dev-1")).toBe(false);
    expect(deviceIsAway("dev-1")).toBe(false);
  });

  it("is here with a live context and nothing being done about it", () => {
    context = live();
    recovery = { status: "idle" };
    expect(deviceIsReconnecting("dev-1")).toBe(false);
    expect(deviceIsAway("dev-1")).toBe(false);
    recovery = null;
    expect(deviceIsAway("dev-1")).toBe(false);
  });
});

describe("hearing that it moved", () => {
  it("listens to the registry and the supervisor, and lets go of both", () => {
    const heard = vi.fn();
    const stop = onDeviceMoved(heard);
    expect(stateListeners.size).toBe(1);
    expect(recoveryListeners.size).toBe(1);
    announce();
    expect(heard).toHaveBeenCalledTimes(2);
    stop();
    expect(stateListeners.size).toBe(0);
    expect(recoveryListeners.size).toBe(0);
  });
});

describe("waiting for it to come back", () => {
  it("does nothing while it is still away", () => {
    const back = vi.fn();
    onDeviceReachable("dev-1", back);
    recovery = { status: ATTEMPTING };
    announce();
    expect(back).not.toHaveBeenCalled();
  });

  // Both subscriptions announce the same reconnect, so the one shot has to be
  // one shot: a surface run twice would read twice.
  it("runs once when it is back, however many subscriptions said so", () => {
    const back = vi.fn();
    onDeviceReachable("dev-1", back);
    context = live();
    announce();
    expect(back).toHaveBeenCalledTimes(1);
    announce();
    expect(back).toHaveBeenCalledTimes(1);
    expect(stateListeners.size).toBe(0);
  });

  it("lets go when the surface goes first", () => {
    const back = vi.fn();
    onDeviceReachable("dev-1", back)();
    context = live();
    announce();
    expect(back).not.toHaveBeenCalled();
  });
});

describe("the watch a surface holds", () => {
  it("answers the three questions core/transientRead.js asks", () => {
    const watch = deviceWatch("dev-1");
    expect(watch.away()).toBe(true);
    expect(watch.reconnecting()).toBe(false);
    recovery = { status: WAITING };
    expect(watch.reconnecting()).toBe(true);
    const heard = vi.fn();
    const stop = watch.moved(heard);
    announce();
    expect(heard).toHaveBeenCalled();
    stop();
    expect(stateListeners.size).toBe(0);
  });
});
