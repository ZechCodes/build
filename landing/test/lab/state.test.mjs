// The lab page's state lives in its query, so a link sent back opens on
// exactly what its sender was looking at.
import assert from "node:assert/strict";
import { describe, it } from "node:test";
import { DEFAULT_STATE, readState, writeState } from "../../src/lab/state.js";

const variants = ["a", "b", "e"];

describe("the lab's state in the URL", () => {
  it("opens on the wall, in its dome, playing in a loop at full speed", () => {
    assert.deepEqual(readState("", variants), DEFAULT_STATE);
    assert.deepEqual(DEFAULT_STATE, { variant: "e", speed: 1, loop: true, endless: false, shape: "dome", blur: "copy", playing: true, time: 0 });
  });

  it("reads back everything it writes", () => {
    const state = { variant: "b", speed: 0.25, loop: false, endless: true, shape: "full", blur: "filter", playing: false, time: 1.37 };
    assert.deepEqual(readState(writeState(state), variants), state);
  });

  it("writes a short query: the moment only when held there", () => {
    assert.equal(writeState(DEFAULT_STATE), "?v=e&speed=1&loop=1&endless=0&shape=dome&blur=copy");
    assert.equal(writeState({ ...DEFAULT_STATE, time: 2.5 }), "?v=e&speed=1&loop=1&endless=0&shape=dome&blur=copy");
    assert.equal(writeState({ ...DEFAULT_STATE, playing: false, time: 2.5 }), "?v=e&speed=1&loop=1&endless=0&shape=dome&blur=copy&paused=1&t=2.5");
  });

  it("falls back to the default for anything it does not know", () => {
    const state = readState("?v=zz&speed=3&loop=maybe&endless=2&shape=cube&blur=lots&t=-4&paused=1", variants);
    assert.deepEqual(state, { ...DEFAULT_STATE, playing: false });
    assert.equal(readState("?t=abc&paused=1", variants).time, 0);
  });

  it("opens a link from before the wall on what it named", () => {
    assert.deepEqual(readState("?v=a&speed=1&loop=1&endless=0", variants), { ...DEFAULT_STATE, variant: "a" });
  });

  it("starts a running link from the beginning", () => {
    assert.equal(readState("?t=2", variants).time, 0);
  });
});
