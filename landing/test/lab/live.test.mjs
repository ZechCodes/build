// Variant A on the lab page: the live hero's flood, moved by flood.js as on
// the home page, with the requests flying to where the laptop's rows would
// be; and its endless form, every pill running round its lane's loop.
import assert from "node:assert/strict";
import { describe, it } from "node:test";
import { element } from "../hero/fake-animation.mjs";
import { HERO_TIMING } from "../../src/lab/legacy/timing.js";
import { liveEndless, liveFlood } from "../../src/lab/live.js";

const stage = { origin: [1000, 400], rows: [[1080, 350], [1080, 370], [1080, 390]], nudge: 10, timing: HERO_TIMING };

function fieldOf() {
  const routine = { element: element(), attention: null, x: 300, y: 200, width: 120, shown: true, opacity: 0.6 };
  const request = { element: element(), attention: "review", x: 700, y: 420, width: 160, shown: true, opacity: 1 };
  const lane = { element: element(), x: 0, speed: 120, pills: [routine, request], shown: true };
  const still = { element: element(), x: 0, speed: 0, pills: [{ element: element(), attention: null, x: 500, y: 600, width: 100, shown: true, opacity: 1 }], shown: true };
  const hidden = { element: element(), x: 0, speed: 0, pills: [], shown: false };
  return { field: { width: 1440, height: 900, lanes: [lane, still, hidden] }, lane, still, hidden, routine, request };
}

const takeOff = HERO_TIMING.landings[0] - HERO_TIMING.flight;

describe("variant A, the live flood", () => {
  it("runs the live hero's beat, to its settle", () => {
    const flood = liveFlood({ field: fieldOf().field, stage });
    assert.equal(flood.end, HERO_TIMING.settle[1]);
    assert.equal(flood.everyFrame, true, "its brakes and flights start on the clock's frames");
  });

  it("flies a request from where it stopped to its row, shrinking and fading", () => {
    const { field, request } = fieldOf();
    const flood = liveFlood({ field, stage });
    flood.update(takeOff + HERO_TIMING.flight / 2, true, 1);
    assert.match(request.element.style.transform, /scale\(0\.80*\d?\)$/);
    assert.equal(request.element.style.opacity, "1");
    flood.update(takeOff + HERO_TIMING.flight, true, 1);
    assert.match(request.element.style.transform, /scale\(0\.6\)$/);
    assert.equal(request.element.style.opacity, "0");
    assert.equal(request.element.style.visibility, "hidden");
  });

  it("hands a request back to the flood when the clock goes back before its take-off", () => {
    const { field, request } = fieldOf();
    const flood = liveFlood({ field, stage });
    flood.update(takeOff + 0.3, true, 1);
    flood.update(takeOff - 0.05, false, 1);
    assert.doesNotMatch(request.element.style.transform, /scale/);
    assert.equal(request.element.style.opacity, "");
    assert.equal(request.element.style.visibility, "");
    flood.update(0.1, false, 1);
    assert.deepEqual([request.element.style.transform, request.element.style.opacity], ["", ""], "before the wave, untouched");
  });
});

describe("variant A, endless", () => {
  it("runs each moving pill round its lane's loop on one endless animation, seeked to the clock", () => {
    const { field, lane, still, hidden, routine, request } = fieldOf();
    const flood = liveEndless({ field });
    flood.update(12.5, false, 0.5);
    for (const pill of [routine, request]) {
      const [loop] = pill.element.live();
      assert.equal(loop.options.iterations, Infinity);
      assert.equal(loop.currentTime, 12_500);
      assert.equal(loop.playbackRate, 0.5);
      assert.equal(loop.playState, "paused");
    }
    assert.equal(lane.element.animations.length, 0, "the lane itself stays put");
    assert.equal(still.pills[0].element.animations.length, 0);
    assert.equal(hidden.element.animations.length, 0);
    flood.update(12.51, true, 1);
    assert.equal(routine.element.live()[0].playState, "running");
    assert.equal(flood.end, Infinity);
  });
});
