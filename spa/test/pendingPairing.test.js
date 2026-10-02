// A device the reader just approved (#321): remembered while it comes up, read
// for every second for PAIRING_WINDOW_MS, then remembered as late until it
// lands.

import { beforeEach, describe, expect, it, vi } from "vitest";
import {
  PAIRING_WINDOW_MS,
  accountReadCadence,
  isPairingConnecting,
  notePairingApproved,
  onPairingChanged,
  pairingLanded,
  pairingLandedAmong,
  pairingState,
  resetPendingPairing,
} from "../src/core/pendingPairing.js";

const APPROVED_AT = 1_000_000;

beforeEach(() => resetPendingPairing());

describe("a pending pairing", () => {
  it("is nothing until something is approved", () => {
    expect(pairingState()).toBe(null);
    expect(accountReadCadence(15000)).toBe(15000);
  });

  it("is connecting for its window, then late", () => {
    notePairingApproved({ device_id: "dev-1", name: "Studio" }, APPROVED_AT);

    expect(pairingState(APPROVED_AT + 1)).toMatchObject({ deviceId: "dev-1", name: "Studio", phase: "connecting" });
    expect(isPairingConnecting("dev-1", APPROVED_AT + PAIRING_WINDOW_MS - 1)).toBe(true);
    expect(isPairingConnecting("dev-2", APPROVED_AT + 1)).toBe(false);
    expect(pairingState(APPROVED_AT + PAIRING_WINDOW_MS).phase).toBe("late");
    expect(isPairingConnecting("dev-1", APPROVED_AT + PAIRING_WINDOW_MS)).toBe(false);
  });

  it("reads the account every second while connecting, and at its own cadence otherwise", () => {
    notePairingApproved({ device_id: "dev-1", name: "Studio" }, APPROVED_AT);

    expect(accountReadCadence(15000, APPROVED_AT + 10)).toBe(1000);
    expect(accountReadCadence(3000, APPROVED_AT + 10)).toBe(1000);
    expect(accountReadCadence(500, APPROVED_AT + 10)).toBe(500);
    expect(accountReadCadence(15000, APPROVED_AT + PAIRING_WINDOW_MS)).toBe(15000);
  });

  it("ends when that device lands, and not when another does", () => {
    const heard = vi.fn();
    const stop = onPairingChanged(heard);
    notePairingApproved({ device_id: "dev-1", name: "Studio" });
    pairingLanded("dev-2");
    expect(pairingState()).not.toBe(null);
    pairingLanded("dev-1");
    expect(pairingState()).toBe(null);
    expect(heard.mock.calls.map(([state]) => state?.deviceId ?? null)).toEqual(["dev-1", null]);
    stop();
  });

  it("ends when that device is among the machines answering", () => {
    notePairingApproved({ device_id: "dev-1", name: "Studio" });
    pairingLandedAmong(["dev-2"]);
    expect(pairingState()).not.toBe(null);
    pairingLandedAmong(["dev-2", "dev-1"]);
    expect(pairingState()).toBe(null);
  });

  it("ignores an approve that names no device", () => {
    notePairingApproved({ name: "Studio" });
    expect(pairingState()).toBe(null);
  });
});
