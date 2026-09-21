// #65: whether this bridge can be asked about watching, and what it says.
//
// The gate is the whole of why this module exists. The issue page makes two
// calls a bridge that predates #64 refuses — the watch verb and the read mark
// — and the read mark fires on every open and every scroll to the end, so an
// ungated one is an error toast per glance at an issue. Refused by default,
// the way the viewing context's gate is (#21): every unknown answers no.

import { describe, expect, it, vi, beforeEach } from "vitest";

let version = null;
vi.mock("../src/core/changeEvents.js", () => ({
  bridgeApiVersion: (deviceId) => (deviceId ? version : null),
}));

const { carriesWatch, readThrough, WATCH_SINCE, watchStateOf } = await import("../src/core/trackerWatch.js");

beforeEach(() => {
  version = "1.7.0";
});

describe("whether the bridge carries watching", () => {
  // The number #64 will name. Until it does, the switch is dark everywhere,
  // which is the one line that changes when it lands.
  it("is refused everywhere while the minor is not known", () => {
    expect(WATCH_SINCE).toBe("");
    expect(carriesWatch("dev-1")).toBe(false);
  });

  const CASES = [
    { name: "a bridge at the minor", version: "1.7.0", since: "1.7.0", carries: true },
    { name: "a bridge past it", version: "1.9.2", since: "1.7.0", carries: true },
    { name: "a bridge below it", version: "1.6.9", since: "1.7.0", carries: false },
    { name: "a bridge that said nothing readable", version: "not a version", since: "1.7.0", carries: false },
    { name: "a bridge that has not greeted yet", version: null, since: "1.7.0", carries: false },
  ];
  for (const one of CASES) {
    it(`${one.name} ${one.carries ? "carries" : "does not carry"} it`, () => {
      version = one.version;
      expect(carriesWatch("dev-1", one.since)).toBe(one.carries);
    });
  }

  it("is refused with no device to ask about", () => {
    expect(carriesWatch("", "1.7.0")).toBe(false);
  });
});

describe("what the record says about watching", () => {
  // Settled on #64 at 00:38Z: `watched` is the reader's own watch, `trackers`
  // stays the live array of agent-id strings, and the hover count is the
  // agents following it. Not a mixed list — `core/trackerAgentIssues.js`
  // matches that array with `includes`, and actor objects would have stopped
  // matching without a word.
  it("reads the reader's watch off `watched` and counts the agents beside it", () => {
    expect(watchStateOf({ watched: true, trackers: ["agent-1", "agent-2"] })).toEqual({ watching: true, watchers: 2 });
    expect(watchStateOf({ watched: false, trackers: ["agent-1"] })).toEqual({ watching: false, watchers: 1 });
  });

  it("reads a record carrying neither as not watching", () => {
    expect(watchStateOf({})).toEqual({ watching: false, watchers: 0 });
    expect(watchStateOf(null)).toEqual({ watching: false, watchers: 0 });
  });

});

describe("how far the reader has read", () => {
  it("is the newest row, whichever kind it is", () => {
    expect(readThrough([{ key: "ie-1" }, { key: "ic-2" }])).toBe("ic-2");
  });

  it("is nothing at all on an empty timeline", () => {
    expect(readThrough([])).toBe("");
    expect(readThrough(undefined)).toBe("");
  });
});
