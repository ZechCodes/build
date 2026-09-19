import { describe, it, expect } from "vitest";
import {
  creationDeviceId,
  homeDeviceId,
  listingCouldBeLagging,
  onlineStickyDeviceId,
} from "../src/core/devicePolicy.js";

// The sticky device choice is only honored when that device is actually online.
// Anything else must fall back to "any device" — pinning a resume to an offline
// sticky device would ignore the working device's return forever.

describe("onlineStickyDeviceId", () => {
  const devices = [
    { id: "dev-a", status: "online" },
    { id: "dev-b", status: "offline" },
  ];

  it("returns the sticky device when it is online", () => {
    expect(onlineStickyDeviceId(devices, "dev-a")).toBe("dev-a");
  });

  it("falls back to any device when the sticky device is offline", () => {
    expect(onlineStickyDeviceId(devices, "dev-b")).toBeNull();
  });

  it("falls back when the sticky device is unknown or unset", () => {
    expect(onlineStickyDeviceId(devices, "dev-z")).toBeNull();
    expect(onlineStickyDeviceId(devices, null)).toBeNull();
    expect(onlineStickyDeviceId([], "dev-a")).toBeNull();
  });
});

// The home device is where creation goes, and what a link that names no device
// is about: the sticky choice while it can answer, and otherwise whichever
// device can.
describe("homeDeviceId", () => {
  it("returns the sticky device when it is online", () => {
    const devices = [
      { id: "dev-a", status: "online" },
      { id: "dev-b", status: "online" },
    ];
    expect(homeDeviceId(devices, "dev-b")).toBe("dev-b");
  });

  it("falls back to the first online device in list order", () => {
    const devices = [
      { id: "dev-a", status: "offline" },
      { id: "dev-b", status: "online" },
      { id: "dev-c", status: "online" },
    ];
    expect(homeDeviceId(devices, "dev-a")).toBe("dev-b");
    expect(homeDeviceId(devices, null)).toBe("dev-b");
    expect(homeDeviceId(devices, "dev-z")).toBe("dev-b");
  });

  it("answers null when nothing is online", () => {
    expect(homeDeviceId([{ id: "dev-a", status: "offline" }], "dev-a")).toBeNull();
    expect(homeDeviceId([], "dev-a")).toBeNull();
  });

  // A machine this client has given up on keeps its place in the account's
  // list, and loses its claim to home: handing creation to a listed-online
  // machine that answers nothing is how every home-addressed call refuses
  // "Device not reachable" while the reader's own workspace works perfectly.
  describe("when the caller can say which machines it has given up on", () => {
    const devices = [
      { id: "dev-a", status: "online" },
      { id: "dev-b", status: "online" },
      { id: "dev-c", status: "online" },
    ];
    const exceptFor = (...givenUpOn) => (deviceId) => !givenUpOn.includes(deviceId);

    it("skips a listed-online machine it has given up on", () => {
      expect(homeDeviceId(devices, null, exceptFor("dev-a"))).toBe("dev-b");
    });

    it("keeps the sticky pick while it is still worth asking", () => {
      expect(homeDeviceId(devices, "dev-c", exceptFor("dev-a"))).toBe("dev-c");
    });

    it("moves off the sticky pick once it has been given up on", () => {
      expect(homeDeviceId(devices, "dev-a", exceptFor("dev-a", "dev-b"))).toBe("dev-c");
    });

    // With every machine given up on there is still a home to name, or the
    // surfaces that refuse would have no machine to refuse about.
    it("falls back to the listing when every machine has been given up on", () => {
      expect(homeDeviceId(devices, null, exceptFor("dev-a", "dev-b", "dev-c"))).toBe("dev-a");
      expect(homeDeviceId(devices, "dev-b", exceptFor("dev-a", "dev-b", "dev-c"))).toBe("dev-b");
    });

    it("never names a machine the account does not list online", () => {
      const listed = [
        { id: "dev-a", status: "offline" },
        { id: "dev-b", status: "online" },
      ];
      expect(homeDeviceId(listed, "dev-a", exceptFor())).toBe("dev-b");
    });
  });
});

// Whether the account calling a machine offline could be the list lagging
// rather than the machine being away. The api derives online from a 90 s
// heartbeat window, so the lag is short and bounded — and a machine last seen
// days ago is off, not late.
describe("listingCouldBeLagging", () => {
  const now = Date.parse("2026-09-19T21:15:47Z");
  const seenAt = (msAgo) => ({ last_seen_at: new Date(now - msAgo).toISOString() });

  it("second-guesses a machine that beat within the window", () => {
    expect(listingCouldBeLagging(seenAt(0), now)).toBe(true);
    expect(listingCouldBeLagging(seenAt(90000), now)).toBe(true);
    expect(listingCouldBeLagging(seenAt(180000), now)).toBe(true);
  });

  it("takes the listing at its word past that", () => {
    expect(listingCouldBeLagging(seenAt(180001), now)).toBe(false);
    expect(listingCouldBeLagging(seenAt(4 * 24 * 60 * 60 * 1000), now)).toBe(false);
  });

  it("does not guess at a machine the account has never seen beat", () => {
    expect(listingCouldBeLagging({ last_seen_at: null }, now)).toBe(false);
    expect(listingCouldBeLagging({}, now)).toBe(false);
    expect(listingCouldBeLagging({ last_seen_at: "not a date" }, now)).toBe(false);
    expect(listingCouldBeLagging(undefined, now)).toBe(false);
  });
});

// Where creation goes, said so a surface can name it: the machine that would
// take the work right now, and — while nothing can take it — the machine the
// user picked, so the composer can say whose return it is waiting for.
describe("creationDeviceId", () => {
  it("is the device home policy names while something is online", () => {
    const devices = [
      { id: "dev-a", status: "online" },
      { id: "dev-b", status: "online" },
    ];
    expect(creationDeviceId(devices, "dev-b")).toBe("dev-b");
    expect(creationDeviceId(devices, null)).toBe("dev-a");
  });

  it("keeps naming the picked device while nothing is online", () => {
    const devices = [{ id: "dev-a", status: "offline" }];
    expect(creationDeviceId(devices, "dev-a")).toBe("dev-a");
  });

  it("names nobody when the account has neither a pick nor an online device", () => {
    expect(creationDeviceId([], null)).toBeNull();
  });
});
