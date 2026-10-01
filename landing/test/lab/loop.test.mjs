// Endless mode: each lane's pills run round a loop longer than the field,
// on one infinite animation each, so the flood never runs dry or resolves.
import assert from "node:assert/strict";
import { describe, it } from "node:test";
import { laneLoop, pillLoop } from "../../src/lab/loop.js";

const pills = [{ x: -150, width: 100 }, { x: 300, width: 200 }, { x: 1700, width: 120 }];

// Where a pill's left edge is at `ms`, as the browser would play its loop.
function leftAt(pill, motion, ms) {
  const progress = (motion.iterationStart + ms / motion.duration) % 1;
  return pill.x - pill.width / 2 + motion.from + (motion.to - motion.from) * progress;
}

describe("a lane's loop", () => {
  it("spans every pill and the whole field, starting out of sight", () => {
    const loop = laneLoop({ pills }, 1440);
    assert.equal(loop.start, -200);
    assert.ok(loop.start + loop.length >= 1760 + 24, "past the last pill and a gap");
    assert.ok(loop.start <= -200 && loop.start + loop.length >= 1440 + 200, "a pill wraps out of sight at both ends");
  });

  it("stretches a short lane to clear the field", () => {
    const loop = laneLoop({ pills: [{ x: 700, width: 100 }] }, 1440);
    assert.ok(loop.start <= -100);
    assert.ok(loop.start + loop.length >= 1440 + 100);
  });
});

describe("a pill's loop", () => {
  const loop = laneLoop({ pills }, 1440);

  for (const speed of [120, -90]) {
    it(`starts where it stands and moves at its lane's speed (${speed} px/s)`, () => {
      for (const pill of pills) {
        const motion = pillLoop(pill, speed, loop);
        // A whole number of loops from where it should be.
        const onLoop = (actual, expected) => {
          const wrapped = ((((actual - expected) % loop.length) + loop.length) % loop.length);
          return Math.min(wrapped, loop.length - wrapped) < 1e-6;
        };
        assert.ok(onLoop(leftAt(pill, motion, 0), pill.x - pill.width / 2));
        assert.ok(onLoop(leftAt(pill, motion, 500), pill.x - pill.width / 2 + speed * 0.5));
        assert.ok(motion.iterationStart >= 0 && motion.iterationStart < 1);
        assert.equal(motion.duration, (loop.length / Math.abs(speed)) * 1000);
      }
    });
  }

  it("leaves a still lane's pills alone", () => {
    assert.equal(pillLoop(pills[0], 0, loop), null);
  });
});
