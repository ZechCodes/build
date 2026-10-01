// Where the lab's ripple starts and its requests land: where the home page's
// laptop screen and Needs you rows stand at rest, as shares of the field.
import assert from "node:assert/strict";
import { describe, it } from "node:test";
import { HERO_TIMING, NARROW_TIMING } from "../../src/hero/timing.js";
import { stageFor } from "../../src/lab/stage.js";

describe("the lab's stage", () => {
  it("places a wide field's ripple on the right, where the laptop's screen is", () => {
    const stage = stageFor(false, { width: 1000, height: 500 });
    assert.equal(stage.timing, HERO_TIMING);
    assert.equal(stage.nudge, 10);
    assert.ok(stage.origin[0] > 650 && stage.origin[0] < 800);
    assert.ok(stage.origin[1] > 180 && stage.origin[1] < 280);
    assert.equal(stage.rows.length, 3);
  });

  it("places a phone's lower down, under the copy, on the phone's own clock", () => {
    const stage = stageFor(true, { width: 400, height: 800 });
    assert.equal(stage.timing, NARROW_TIMING);
    assert.equal(stage.nudge, 6);
    assert.ok(stage.origin[1] > 500);
    // The rows run down the laptop's list, in ATTENTION's order.
    const ys = stage.rows.map(([, y]) => y);
    assert.deepEqual(ys, [...ys].sort((a, b) => a - b));
  });
});
