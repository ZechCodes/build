// The field's motion through the entrance: lanes and fading pills run as
// Web Animations on the compositor, seeked from the entrance's clock. These
// drive it with stand-ins for the elements, as the clock would.
import assert from "node:assert/strict";
import { describe, it } from "node:test";
import { createFieldMotion } from "../../src/hero/flood.js";
import { HERO_TIMING, rippleReach } from "../../src/hero/timing.js";

class FakeAnimation {
  constructor(keyframes, options) {
    this.keyframes = keyframes;
    this.options = options;
    this.currentTime = 0;
    this.playState = "running";
  }

  pause() { this.playState = "paused"; }

  play() { this.playState = "running"; }

  cancel() { this.playState = "idle"; }
}

function element() {
  return {
    style: {},
    animations: [],
    animate(keyframes, options) {
      const animation = new FakeAnimation(keyframes, options);
      this.animations.push(animation);
      return animation;
    },
    live() { return this.animations.filter((animation) => animation.playState !== "idle"); },
  };
}

const box = { width: 1440, height: 900 };
const origin = [900, 450];
const ripple = { origin, reach: rippleReach(origin, box), timing: HERO_TIMING, start: 0.5, nudge: 10 };

function fieldOf() {
  const routine = { element: element(), attention: null, x: 300, y: 200, width: 120, shown: true, opacity: 0.6 };
  const offscreen = { element: element(), attention: null, x: -2000, y: 200, width: 120, shown: true, opacity: 0.6 };
  const request = { element: element(), attention: "review", x: 700, y: 420, width: 160, shown: true, opacity: 1 };
  const lane = { element: element(), x: 40, speed: 120, pills: [routine, offscreen, request], shown: true };
  const hidden = { element: element(), x: 0, speed: 0, pills: [], shown: false };
  return { field: { ...box, lanes: [lane, hidden] }, lane, hidden, routine, offscreen, request };
}

const flights = new Map([["review", 2.2]]);

describe("the field's motion", () => {
  it("moves each shown lane on one animation, seeked to the clock", () => {
    const { field, lane, hidden } = fieldOf();
    const motion = createFieldMotion({ field, ripple, flights });
    motion.update(0.8, false);
    assert.equal(lane.element.live().length, 1);
    const [drift] = lane.element.live();
    assert.equal(drift.playState, "paused");
    assert.equal(drift.currentTime, 800);
    // The lane is where it was measured at the clock's start.
    const at = (offset) => drift.keyframes.find((frame) => frame.offset === offset).transform;
    assert.equal(at(0), `translateX(${40 - 120 * 0.5}px)`);
    assert.equal(at(1), `translateX(${40 + 120 * (HERO_TIMING.ripple[1] - 0.5)}px)`);
    assert.equal(drift.options.duration, HERO_TIMING.ripple[1] * 1000);
    assert.equal(hidden.element.animations.length, 0, "a lane a phone leaves out is left alone");
  });

  it("plays what it seeked once the clock runs, and leaves a running animation alone", () => {
    const { field, lane } = fieldOf();
    const motion = createFieldMotion({ field, ripple, flights });
    motion.update(0.8, false);
    motion.update(0.81, true);
    const [drift] = lane.element.live();
    assert.equal(drift.playState, "running");
    assert.equal(drift.currentTime, 810);
    drift.currentTime = 830;
    motion.update(0.84, true);
    assert.equal(drift.currentTime, 830, "close enough: the compositor keeps its own time");
    motion.update(1.2, true);
    assert.equal(drift.currentTime, 1200, "a clock that has run away is caught up");
  });

  it("leaves a routine pill to its lane until the wave reaches it, fades it on the compositor, then hides it", () => {
    const { field, routine } = fieldOf();
    const motion = createFieldMotion({ field, ripple, flights });
    const { hit } = motion.pathOf(routine);
    motion.update(hit - 0.01, true);
    assert.equal(routine.element.animations.length, 0);
    assert.equal(routine.element.style.transform, "");
    motion.update(hit + 0.1, true);
    const [fade] = routine.element.live();
    assert.ok(Math.abs(fade.currentTime - 100) < 1e-6);
    assert.equal(fade.options.duration, HERO_TIMING.fade * 1000);
    assert.equal(fade.keyframes[0].opacity, "0.6");
    assert.equal(fade.keyframes[0].transform, "translate(0px, 0px)");
    assert.equal(fade.keyframes.at(-1).opacity, "0");
    motion.update(hit + HERO_TIMING.fade + 0.01, true);
    assert.equal(routine.element.live().length, 0);
    assert.equal(routine.element.style.visibility, "hidden");
    // Back before the wave, as the scrubber may go: untouched again.
    motion.update(hit - 0.2, false);
    assert.equal(routine.element.live().length, 0);
    assert.deepEqual([routine.element.style.transform, routine.element.style.opacity, routine.element.style.visibility], ["", "", ""]);
  });

  it("starts nothing for a pill that never comes into the field", () => {
    const { field, offscreen } = fieldOf();
    const motion = createFieldMotion({ field, ripple, flights });
    for (const time of [0.5, 1.3, 1.6, 2.4]) motion.update(time, true);
    assert.equal(offscreen.element.animations.length, 0);
  });

  it("brakes a request until it takes off, and hands it over where it has stopped", () => {
    const { field, request } = fieldOf();
    const motion = createFieldMotion({ field, ripple, flights });
    const { hit } = motion.pathOf(request);
    motion.update(hit + 0.1, true);
    const [brake] = request.element.live();
    assert.ok(brake.keyframes.every((frame) => frame.opacity === undefined), "a request never fades");
    motion.update(2.21, true);
    assert.equal(request.element.live().length, 0, "the flight has it now");
    const [x, y] = motion.offsetAt(request, 2.2);
    assert.equal(request.element.style.transform, `translate(${Math.round(x * 10) / 10}px, ${Math.round(y * 10) / 10}px)`);
  });
});
