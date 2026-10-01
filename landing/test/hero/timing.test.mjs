// The entrance's clock and the maths its tweens are built from. Pure.
import assert from "node:assert/strict";
import { describe, it } from "node:test";
import {
  HERO_TIMING,
  NARROW_TIMING,
  decelDistance,
  laptopEntrancePose,
  laptopRevealPose,
  outward,
  phaseAt,
  posterReveal,
  rippleHit,
  rippleReach,
} from "../../src/hero/timing.js";

const FINAL = { x: 68, y: 54, w: 56, yaw: -6, pitch: 4, roll: 0, opacity: 1, lidOpen: 1, faceCamera: 1 };

describe("the entrance clock", () => {
  it("runs the brief's phases in about four seconds, the laptop overlapping the ripple", () => {
    const t = HERO_TIMING;
    assert.deepEqual(t.field, [0, 1.2]);
    assert.deepEqual(t.ripple, [1.2, 2]);
    assert.deepEqual(t.laptop, [1.6, 3.2]);
    assert.deepEqual(t.converge, [2.2, 3]);
    assert.deepEqual(t.message, [3, 3.5]);
    assert.deepEqual(t.settle, [3.5, 4]);
    assert.ok(t.laptop[0] < t.ripple[1], "the laptop comes in while the ripple passes");
    for (const landing of t.landings) assert.ok(landing > t.converge[0] && landing <= t.converge[1]);
  });

  it("is a little shorter on a phone, in the same order", () => {
    const order = ["field", "ripple", "laptop", "converge", "message", "settle"];
    assert.ok(NARROW_TIMING.settle[1] < HERO_TIMING.settle[1]);
    assert.ok(NARROW_TIMING.settle[1] >= 3.2);
    for (const timing of [HERO_TIMING, NARROW_TIMING]) {
      order.slice(1).forEach((phase, index) => assert.ok(timing[phase][0] >= timing[order[index]][0], phase));
    }
  });

  it("names the phase at any moment, for the scrubber", () => {
    assert.equal(phaseAt(0.5, HERO_TIMING), "field");
    assert.equal(phaseAt(1.3, HERO_TIMING), "ripple");
    assert.equal(phaseAt(2.5, HERO_TIMING), "converge");
    assert.equal(phaseAt(3.2, HERO_TIMING), "message");
    assert.equal(phaseAt(3.8, HERO_TIMING), "settle");
    assert.equal(phaseAt(9, HERO_TIMING), "settled");
  });
});

describe("the ripple", () => {
  const origin = [900, 450];
  const box = { width: 1440, height: 900 };

  it("reaches the farthest corner of the field", () => {
    assert.ok(Math.abs(rippleReach(origin, box) - Math.hypot(900, 450)) < 1e-9);
  });

  it("reaches a pill by its distance from the origin, inside the ripple", () => {
    const reach = rippleReach(origin, box);
    const near = rippleHit([920, 460], origin, reach, HERO_TIMING);
    const mid = rippleHit([500, 300], origin, reach, HERO_TIMING);
    const far = rippleHit([0, 0], origin, reach, HERO_TIMING);
    assert.ok(near < mid && mid < far);
    assert.ok(near >= HERO_TIMING.ripple[0]);
    assert.ok(far + HERO_TIMING.fade <= HERO_TIMING.ripple[1] + 1e-9, "faded by the end of the ripple");
    // Two pills the same distance away are reached together: a ring, not a random fade.
    assert.equal(rippleHit([900, 250], origin, reach, HERO_TIMING), rippleHit([1100, 450], origin, reach, HERO_TIMING));
  });

  it("pushes a pill away from the origin, a little", () => {
    const [dx, dy] = outward([1000, 450], origin, 8);
    assert.ok(Math.abs(dx - 8) < 1e-9 && Math.abs(dy) < 1e-9);
    assert.deepEqual(outward(origin, origin, 8), [0, 0]);
  });

  it("brakes a pill to a stop without a jolt", () => {
    // power1.out starts at twice its average speed, so a pill moving at
    // `speed` stops after speed × seconds / 2.
    assert.equal(decelDistance(200, 0.5), 50);
  });
});

describe("the laptop's entrance", () => {
  it("starts a quarter turn away, lid partly open, out of sight, and ends at rest", () => {
    const from = laptopEntrancePose(FINAL);
    assert.ok(from.yaw - FINAL.yaw >= 60 && from.yaw - FINAL.yaw <= 90, `turn ${from.yaw - FINAL.yaw}`);
    assert.ok(from.lidOpen > 0 && from.lidOpen < 0.5);
    assert.equal(from.opacity, 0);
    assert.deepEqual(laptopRevealPose(1, from, FINAL), FINAL);
    assert.deepEqual(laptopRevealPose(0, from, FINAL), from);
  });

  it("turns less and travels less on a phone", () => {
    const wide = laptopEntrancePose(FINAL);
    const narrow = laptopEntrancePose(FINAL, { narrow: true });
    assert.ok(narrow.yaw - FINAL.yaw < wide.yaw - FINAL.yaw);
    assert.ok(Math.abs(narrow.x - FINAL.x) < Math.abs(wide.x - FINAL.x));
  });

  it("is in sight before the lid finishes opening", () => {
    const from = laptopEntrancePose(FINAL);
    const early = laptopRevealPose(0.35, from, FINAL);
    assert.equal(early.opacity, 1);
    assert.ok(early.lidOpen < 0.9);
  });

  it("gives the poster the same arc, flattened", () => {
    assert.deepEqual(posterReveal(1), { turn: 0, shift: 0, scale: 1, opacity: 1 });
    const start = posterReveal(0);
    assert.equal(start.opacity, 0);
    assert.ok(start.turn > 0 && start.shift > 0 && start.scale < 1);
    assert.ok(posterReveal(0, { narrow: true }).turn < start.turn);
  });
});

describe("the laptop's ease", () => {
  it("runs 0 to 1, gathering speed and then braking, without a jump in speed", async () => {
    const { revealEase } = await import("../../src/hero/timing.js");
    assert.equal(revealEase(0), 0);
    assert.equal(revealEase(1), 1);
    const speed = (p) => (revealEase(p + 1e-6) - revealEase(p - 1e-6)) / 2e-6;
    assert.ok(speed(0.1) < speed(0.24), "accelerates");
    assert.ok(speed(0.6) > speed(0.9), "decelerates");
    assert.ok(Math.abs(speed(0.25 - 1e-4) - speed(0.25 + 1e-4)) < 0.01, "no jump in speed");
    for (let p = 0; p < 1; p += 0.01) assert.ok(revealEase(p + 0.01) >= revealEase(p));
  });
});
