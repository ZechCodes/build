// Devices the reader just approved (#321): each remembered while it comes up,
// the account read every second meanwhile, until the account lists it online
// or its window runs out.

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  PAIRING_WINDOW_MS,
  accountReadCadence,
  isPairingConnecting,
  notePairingApproved,
  onPairingChanged,
  pairingsConnecting,
  pairingsLandedIn,
  resetPendingPairing,
} from "../src/core/pendingPairing.js";

const listed = (id, status) => ({ id, name: id, status });
const pendingIds = () => pairingsConnecting().map((pairing) => pairing.deviceId);

beforeEach(() => {
  vi.useFakeTimers();
  resetPendingPairing();
});

afterEach(() => {
  resetPendingPairing();
  vi.useRealTimers();
});

describe("a pending pairing", () => {
  it("is nothing until something is approved", () => {
    expect(pairingsConnecting()).toEqual([]);
    expect(accountReadCadence(15000)).toBe(15000);
  });

  it("names a listed device as connecting while the account does not call it online", () => {
    notePairingApproved({ device_id: "dev-1", name: "Studio" });

    expect(pairingsConnecting()).toMatchObject([{ deviceId: "dev-1", name: "Studio" }]);
    expect(isPairingConnecting(listed("dev-1", "offline"))).toBe(true);
    expect(isPairingConnecting(listed("dev-1", "online"))).toBe(false);
    expect(isPairingConnecting(listed("dev-2", "offline"))).toBe(false);
  });

  it("reads the account every second while one is pending, and at its own cadence otherwise", () => {
    notePairingApproved({ device_id: "dev-1", name: "Studio" });

    expect(accountReadCadence(15000)).toBe(1000);
    expect(accountReadCadence(3000)).toBe(1000);
    expect(accountReadCadence(500)).toBe(500);
  });

  it("ends when the account lists that device online, and not when it lists another", () => {
    const heard = vi.fn();
    const stop = onPairingChanged(heard);
    notePairingApproved({ device_id: "dev-1", name: "Studio" });

    pairingsLandedIn([listed("dev-1", "offline"), listed("dev-2", "online")]);
    expect(pendingIds()).toEqual(["dev-1"]);
    pairingsLandedIn([listed("dev-1", "online")]);
    expect(pendingIds()).toEqual([]);
    expect(accountReadCadence(15000)).toBe(15000);
    expect(heard).toHaveBeenCalledTimes(2);
    stop();
  });

  it("is forgotten, and heard of, when its window runs out with the device never online", () => {
    const heard = vi.fn();
    const stop = onPairingChanged(heard);
    notePairingApproved({ device_id: "dev-1", name: "Studio" });
    heard.mockClear();

    vi.advanceTimersByTime(PAIRING_WINDOW_MS - 1);
    expect(pendingIds()).toEqual(["dev-1"]);
    expect(heard).not.toHaveBeenCalled();
    vi.advanceTimersByTime(1);
    expect(pendingIds()).toEqual([]);
    expect(isPairingConnecting(listed("dev-1", "offline"))).toBe(false);
    expect(accountReadCadence(15000)).toBe(15000);
    expect(heard).toHaveBeenCalledTimes(1);
    stop();
  });

  it("keeps two approved devices apart: each lands or runs out on its own", () => {
    notePairingApproved({ device_id: "dev-1", name: "Studio" });
    vi.advanceTimersByTime(30_000);
    notePairingApproved({ device_id: "dev-2", name: "Laptop" });
    expect(pendingIds()).toEqual(["dev-1", "dev-2"]);

    pairingsLandedIn([listed("dev-2", "online"), listed("dev-1", "offline")]);
    expect(pendingIds()).toEqual(["dev-1"]);
    expect(accountReadCadence(15000)).toBe(1000);

    notePairingApproved({ device_id: "dev-3", name: "Mini" });
    vi.advanceTimersByTime(PAIRING_WINDOW_MS - 30_000);
    expect(pendingIds()).toEqual(["dev-3"]);
    vi.advanceTimersByTime(30_000);
    expect(pendingIds()).toEqual([]);
  });

  it("starts a device's window over when it is approved again", () => {
    notePairingApproved({ device_id: "dev-1", name: "Studio" });
    vi.advanceTimersByTime(60_000);
    notePairingApproved({ device_id: "dev-1", name: "Studio" });
    vi.advanceTimersByTime(60_000);
    expect(pendingIds()).toEqual(["dev-1"]);
    vi.advanceTimersByTime(30_000);
    expect(pendingIds()).toEqual([]);
  });

  it("ignores an approve that names no device", () => {
    notePairingApproved({ name: "Studio" });
    expect(pairingsConnecting()).toEqual([]);
  });

  it("forgets everything, timers included, on a reset", () => {
    const heard = vi.fn();
    const stop = onPairingChanged(heard);
    notePairingApproved({ device_id: "dev-1", name: "Studio" });
    resetPendingPairing();
    heard.mockClear();
    vi.advanceTimersByTime(PAIRING_WINDOW_MS);
    expect(heard).not.toHaveBeenCalled();
    expect(vi.getTimerCount()).toBe(0);
    stop();
  });
});
