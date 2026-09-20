// What the connection icon says, worked out from the machines and their
// recovery records. The icon itself draws exactly this and decides nothing.
import { describe, expect, it } from "vitest";
import { connectionStatus, secondsUntilAttempt } from "../src/core/connectionStatusModel.js";

const NOW = 1_700_000_000_000;
const STUDIO = { id: "a", name: "Studio", live: true };
const LAPTOP = { id: "b", name: "Laptop", live: true };

const status = (devices, recoveries = [], nowMs = NOW) =>
  connectionStatus({ devices, recoveries, nowMs });

describe("the seconds until the next attempt", () => {
  it("rounds up, so a countdown never says a second it has already spent", () => {
    expect(secondsUntilAttempt(NOW + 2001, NOW)).toBe(3);
    expect(secondsUntilAttempt(NOW + 2000, NOW)).toBe(2);
    expect(secondsUntilAttempt(NOW + 1, NOW)).toBe(1);
  });

  it("never runs past zero, whatever the clock or the record says", () => {
    expect(secondsUntilAttempt(NOW - 5000, NOW)).toBe(0);
    expect(secondsUntilAttempt(null, NOW)).toBe(0);
    expect(secondsUntilAttempt(undefined, NOW)).toBe(0);
  });
});

describe("every machine connected", () => {
  it("counts the machines holding a session, and nothing else", () => {
    const seen = status([STUDIO, LAPTOP, { id: "c", name: "Tower", live: false }]);
    expect(seen.state).toBe("connected");
    expect(seen.connectedCount).toBe(2);
    expect(seen.centre).toBe("2");
    expect(seen.label).toBe("Connected to 2 devices");
    expect(seen.ticking).toBe(false);
  });

  it("says one device in the singular", () => {
    expect(status([STUDIO]).label).toBe("Connected to 1 device");
  });

  it("says so when nothing is connected and nothing is being tried", () => {
    const seen = status([{ ...STUDIO, live: false }]);
    expect(seen.centre).toBe("0");
    expect(seen.label).toBe("No devices connected");
  });

  it("is nothing to show at all before the account has a machine", () => {
    expect(status([]).visible).toBe(false);
    expect(status([STUDIO]).visible).toBe(true);
  });
});

// The ring says what is true of the account; the menu behind it says what is
// true of each machine. One derivation, because the two must never disagree
// about the same machine — and the words are the menu's own: a row has the
// room for a state and no room for a sentence.
describe("the machines, one row each", () => {
  const attempting = { deviceId: "b", status: "attempting", failedAttempts: 0, nextAttemptAt: null };
  const waiting = { deviceId: "b", status: "waiting", failedAttempts: 1, nextAttemptAt: NOW + 2400 };

  it("says Connected for a machine holding a session", () => {
    expect(status([STUDIO]).rows).toEqual([
      { id: "a", name: "Studio", status: "connected", seconds: null, path: null, label: "Connected" },
    ]);
  });

  // Both are connected and neither is a fault, but they are not the same
  // connection to be on, and which one the reader got is the one thing they
  // cannot see from the ring.
  it("says which way a connected machine is carrying, once its path is known", () => {
    expect(status([{ ...STUDIO, path: "direct" }]).rows[0].label).toBe("Connected WebRTC");
    expect(status([{ ...STUDIO, path: "turn" }]).rows[0].label).toBe("Connected TURN");
  });

  it("says the plain word while nothing has answered for the path yet", () => {
    expect(status([STUDIO]).rows[0].label).toBe("Connected");
    expect(status([{ ...STUDIO, path: "something new" }]).rows[0].label).toBe("Connected");
  });

  it("says nothing about the path of a machine that is not connected", () => {
    const rows = status([{ ...STUDIO, live: false, path: "turn" }]).rows;
    expect(rows[0].label).toBe("Offline");
    expect(rows[0].path).toBe(null);
  });

  // Not every machine that is not connected is being reconnected to: recovery
  // stands down when the account stops calling a machine online, and never
  // starts for one that was never online. Such a machine is off, which is a
  // state and not a failure.
  it("says Offline for a machine with no session and nothing being done about it", () => {
    expect(status([{ ...STUDIO, live: false }]).rows[0]).toMatchObject({ status: "offline", label: "Offline", seconds: null });
  });

  it("says Reconnecting while an attempt is in flight", () => {
    const rows = status([STUDIO, { ...LAPTOP, live: false }], [attempting]).rows;
    expect(rows.map((row) => row.label)).toEqual(["Connected", "Reconnecting"]);
    expect(rows[1]).toMatchObject({ id: "b", status: "attempting", seconds: null });
  });

  it("counts that machine's own wait down, not the soonest of them all", () => {
    const rows = status([STUDIO, LAPTOP], [
      { deviceId: "a", status: "waiting", failedAttempts: 1, nextAttemptAt: NOW + 9000 },
      waiting,
    ]).rows;
    expect(rows.map((row) => row.label)).toEqual(["Reconnecting in 9 s", "Reconnecting in 3 s"]);
    expect(rows.map((row) => row.seconds)).toEqual([9, 3]);
  });

  it("drops the count from a wait that is spent, because the try is beginning", () => {
    expect(status([LAPTOP], [{ ...waiting, nextAttemptAt: NOW - 1 }]).rows[0].label).toBe("Reconnecting");
  });

  it("keeps the account's own order, and names a machine the list has not caught up with", () => {
    const rows = status([STUDIO], [{ deviceId: "z", status: "attempting", failedAttempts: 0, nextAttemptAt: null }]).rows;
    expect(rows.map((row) => row.name)).toEqual(["Studio", "a device"]);
  });

  it("holds the clock open while any machine is counting, whatever the ring shows", () => {
    // The ring shows the attempt in flight and has no countdown of its own,
    // but the menu behind it is counting a second machine down.
    const seen = status([STUDIO, LAPTOP], [attempting, { ...waiting, deviceId: "a" }]);
    expect(seen.state).toBe("attempting");
    expect(seen.ticking).toBe(true);
  });
});

describe("a machine being reconnected to", () => {
  const attempting = { deviceId: "a", status: "attempting", failedAttempts: 0, nextAttemptAt: null };
  const waiting = { deviceId: "a", status: "waiting", failedAttempts: 1, nextAttemptAt: NOW + 2400 };

  it("shows from the very first attempt, with no number in the middle", () => {
    // The toasts this replaces stayed quiet until an attempt had already
    // failed. The ring is small enough to say so from the first one.
    const seen = status([STUDIO, LAPTOP], [attempting]);
    expect(seen.state).toBe("attempting");
    expect(seen.centre).toBe("");
    expect(seen.label).toBe("Reconnecting to Studio");
    expect(seen.ticking).toBe(false);
  });

  it("counts down to the next attempt while it waits", () => {
    const seen = status([STUDIO], [waiting]);
    expect(seen.state).toBe("waiting");
    expect(seen.seconds).toBe(3);
    expect(seen.centre).toBe("3");
    expect(seen.label).toBe("Reconnecting to Studio in 3 seconds");
    expect(seen.ticking).toBe(true);
  });

  it("says a second in the singular, and drops the wait once it is spent", () => {
    expect(status([STUDIO], [{ ...waiting, nextAttemptAt: NOW + 900 }]).label)
      .toBe("Reconnecting to Studio in 1 second");
    expect(status([STUDIO], [{ ...waiting, nextAttemptAt: NOW - 10 }]).label)
      .toBe("Reconnecting to Studio");
  });

  it("counts down to the soonest attempt when several machines are waiting", () => {
    const seen = status([STUDIO, LAPTOP], [
      { ...waiting, nextAttemptAt: NOW + 9000 },
      { deviceId: "b", status: "waiting", failedAttempts: 2, nextAttemptAt: NOW + 3000 },
    ]);
    expect(seen.seconds).toBe(3);
    expect(seen.label).toBe("Reconnecting to Studio and Laptop in 3 seconds");
  });

  it("leads with the attempt in flight when another machine is still waiting", () => {
    // One machine is being dialled right now and another is between tries:
    // what the ring shows is the thing actually happening.
    const seen = status([STUDIO, LAPTOP], [
      { deviceId: "b", status: "waiting", failedAttempts: 2, nextAttemptAt: NOW + 3000 },
      attempting,
    ]);
    expect(seen.state).toBe("attempting");
    expect(seen.label).toBe("Reconnecting to Studio");
  });

  it("names two machines, and counts more than two", () => {
    const both = [attempting, { deviceId: "b", status: "attempting", failedAttempts: 1, nextAttemptAt: null }];
    expect(status([STUDIO, LAPTOP], both).label).toBe("Reconnecting to Studio and Laptop");

    const three = [...both, { deviceId: "c", status: "attempting", failedAttempts: 1, nextAttemptAt: null }];
    expect(status([STUDIO, LAPTOP, { id: "c", name: "Tower", live: false }], three).label)
      .toBe("Reconnecting to 3 devices");
  });

  it("falls back to the plain word for a machine the account cannot name", () => {
    expect(status([], [{ deviceId: "z", status: "attempting", failedAttempts: 0, nextAttemptAt: null }]).label)
      .toBe("Reconnecting to a device");
  });

  it("still counts the machines that are connected behind it", () => {
    // The count is not shown while the ring is yellow, but it is what the
    // ring goes back to saying, and it must not have gone stale meanwhile.
    expect(status([STUDIO, LAPTOP], [attempting]).connectedCount).toBe(2);
  });

  it("is shown even where the account list has not caught up with the machine", () => {
    expect(status([], [attempting]).visible).toBe(true);
  });

  it("ignores a record that says nothing is happening", () => {
    expect(status([STUDIO], [{ deviceId: "a", status: "idle", failedAttempts: 0, nextAttemptAt: null }]).state)
      .toBe("connected");
  });
});
