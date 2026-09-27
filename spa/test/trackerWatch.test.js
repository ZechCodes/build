// #65: what a task's record says about watching, and how far it has been read.
//
// The gate itself is NOT here. It moved to a capability at `ea3de439`
// (`bridgeCapabilities(deviceId)?.tasks?.watching === true`) and the shell
// agent's test/trackerWatchGate.test.js holds it, with
// test/taskWatchGate.test.js holding this page on top of a real greeting.
// What is left here is the reading of the record — the two functions only this
// page uses.

import { describe, expect, it } from "vitest";
import { readThrough, watchStateOf } from "../src/core/trackerWatch.js";

describe("what the record says about watching", () => {
  // Settled on #64 at 00:38Z: `watched` is the reader's own watch, `trackers`
  // stays the live array of agent-id strings, and the hover count is the
  // agents following it. Not a mixed list — `core/trackerAgentTasks.js`
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
    expect(readThrough([{ key: "te-1" }, { key: "tc-2" }])).toBe("tc-2");
  });

  // core/trackerTimeline.js keys a record the bridge wrote without an id by
  // its position. Sent as a mark, #64 would read the part after the first `-`
  // as the instant — "3" sorts above every ULID — and its never-go-backwards
  // guard would then refuse every real mark after it. Unread would stick for
  // good, silently, because that refusal is a no-op on the bridge and this
  // page swallows read-mark failures on purpose.
  it("never sends a key the timeline invented for itself", () => {
    expect(readThrough([{ key: "tc-01M30" }, { key: "comment-3" }])).toBe("");
    expect(readThrough([{ key: "event-0" }])).toBe("");
    // A real id that merely looks similar is still sent.
    expect(readThrough([{ key: "te-01M30" }])).toBe("te-01M30");
  });

  it("is nothing at all on an empty timeline", () => {
    expect(readThrough([])).toBe("");
    expect(readThrough(undefined)).toBe("");
  });
});
