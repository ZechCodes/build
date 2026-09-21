// #65: what an issue's record says about watching, and how far it has been read.
//
// The gate itself is NOT here. It moved to a capability at `ea3de439`
// (`bridgeCapabilities(deviceId)?.issues?.watching === true`) and the shell
// agent's test/trackerWatchGate.test.js holds it, with
// test/issueWatchGate.test.js holding this page on top of a real greeting.
// What is left here is the reading of the record — the two functions only this
// page uses.

import { describe, expect, it } from "vitest";
import { readThrough, watchStateOf } from "../src/core/trackerWatch.js";

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
