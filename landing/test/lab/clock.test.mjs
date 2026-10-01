// The lab's clock: the time the field is shown at, advanced by the frame's
// elapsed seconds at the chosen speed.
import assert from "node:assert/strict";
import { describe, it } from "node:test";
import { ENDLESS_WINDOW, advance, scrubbed, scrubberAt } from "../../src/lab/clock.js";

const base = { time: 0, playing: true, speed: 1, loop: false, endless: false };

describe("the lab's clock", () => {
  it("runs at its speed", () => {
    assert.equal(advance({ ...base, time: 1 }, 0.1, 4).time, 1.1);
    assert.equal(advance({ ...base, time: 1, speed: 0.25 }, 0.4, 4).time, 1.1);
  });

  it("stands still while paused", () => {
    assert.equal(advance({ ...base, time: 1, playing: false }, 0.5, 4).time, 1);
  });

  it("stops at the end of the beat unless it loops", () => {
    assert.deepEqual(advance({ ...base, time: 3.9 }, 0.5, 4), { ...base, time: 4, playing: false });
  });

  it("starts the beat again at the end when it loops", () => {
    const next = advance({ ...base, time: 3.9, loop: true }, 0.25, 4);
    assert.ok(Math.abs(next.time - 0.15) < 1e-9);
    assert.equal(next.playing, true);
  });

  it("never ends in endless mode", () => {
    const next = advance({ ...base, time: 3.9, endless: true }, 0.5, 4);
    assert.equal(next.time, 4.4);
    assert.equal(next.playing, true);
    assert.ok(ENDLESS_WINDOW > 4);
  });

  it("restarts a finished beat when played again", () => {
    assert.equal(advance({ ...base, time: 4 }, 0.1, 4).time, 0.1);
  });
});

describe("the lab's clock against its timestamps", () => {
  it("never runs backwards on a frame stamped before the last one", () => {
    assert.equal(advance({ ...base, time: 0 }, -0.004, 4).time, 0);
    assert.equal(advance({ ...base, time: 0, endless: true }, -0.004, 4).time, 0);
  });
});

describe("the scrubber", () => {
  it("spans the beat, or one lap of the endless window", () => {
    assert.equal(scrubberAt({ ...base, time: 2.5 }), 2.5);
    assert.equal(scrubberAt({ ...base, endless: true, time: ENDLESS_WINDOW * 2 + 3 }), 3);
  });

  it("moves within the lap it is on in endless mode", () => {
    assert.equal(scrubbed({ ...base, time: 1 }, 2.5), 2.5);
    assert.equal(scrubbed({ ...base, endless: true, time: ENDLESS_WINDOW * 2 + 3 }, 7), ENDLESS_WINDOW * 2 + 7);
  });
});
