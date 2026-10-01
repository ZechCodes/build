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
  pillPath,
  posterReveal,
  reachesField,
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

  it("keeps the pills that will cross the field, whichever way their lane runs", () => {
    // A 100px pill, the field 1440px wide, half a second left to move.
    const pill = (x) => ({ x, width: 100 });
    assert.ok(reachesField(pill(700), 300, 1440, 0.5), "already in sight");
    assert.ok(reachesField(pill(-150), 300, 1440, 0.5), "comes in from the left");
    assert.ok(!reachesField(pill(-250), 300, 1440, 0.5), "too far left to arrive in time");
    assert.ok(!reachesField(pill(1500), 300, 1440, 0.5), "gone past the right edge");
    assert.ok(reachesField(pill(1580), -300, 1440, 0.5), "comes in from the right");
    assert.ok(!reachesField(pill(1700), -300, 1440, 0.5), "too far right to arrive in time");
    assert.ok(!reachesField(pill(-60), -300, 1440, 0.5), "gone past the left edge");
  });

  it("carries a pill with its lane until the wave reaches it, then brakes and fades it smoothly", () => {
    const reach = rippleReach(origin, box);
    const ripple = { origin, reach, timing: HERO_TIMING, start: 0.5, nudge: 10 };
    const path = pillPath({ x: 300, y: 200 }, 240, ripple);
    assert.ok(path.hit >= HERO_TIMING.ripple[0] && path.hit + HERO_TIMING.fade <= HERO_TIMING.ripple[1] + 1e-9);
    assert.deepEqual(path.at(0.5), { dx: 0, dy: 0, faded: 0 });
    assert.deepEqual(path.at(1), { dx: 120, dy: 0, faded: 0 });
    // No jump where the brake begins, and no speed left where it ends.
    const at = (time) => path.at(time).dx;
    assert.ok(Math.abs(at(path.hit + 1e-6) - at(path.hit - 1e-6)) < 1e-3);
    // From its lane's speed, give or take the push outward.
    assert.ok(Math.abs((at(path.hit + 2e-4) - at(path.hit)) / 2e-4 - 240) <= (2 * 10) / HERO_TIMING.decel + 1, "brakes from its lane's speed");
    const end = path.hit + HERO_TIMING.decel;
    assert.ok(Math.abs(at(end) - at(end - 1e-4)) < 1e-3);
    const stop = path.at(end + 1);
    const [pushX, pushY] = outward([300 + 240 * (HERO_TIMING.ripple[0] - 0.5), 200], origin, 10);
    assert.ok(Math.abs(stop.dx - (240 * (path.hit - 0.5) + decelDistance(240, HERO_TIMING.decel) + pushX)) < 1e-9);
    assert.ok(Math.abs(stop.dy - pushY) < 1e-9);
    assert.equal(path.at(path.hit + HERO_TIMING.fade).faded, 1);
    assert.ok(path.at(path.hit + HERO_TIMING.fade / 2).faded < 0.5, "fades slowly first");
  });

  it("fades a pill coming in from beyond the wave's reach by the ripple's end", () => {
    const reach = rippleReach(origin, box);
    const path = pillPath({ x: -2400, y: 200 }, 900, { origin, reach, timing: HERO_TIMING, start: 0, nudge: 10 });
    assert.ok(path.hit + HERO_TIMING.fade <= HERO_TIMING.ripple[1] + 1e-9);
  });

  it("brakes a pill running left the same way", () => {
    const reach = rippleReach(origin, box);
    const path = pillPath({ x: 1200, y: 600 }, -300, { origin, reach, timing: HERO_TIMING, start: 0, nudge: 10 });
    assert.ok(path.at(path.hit).dx < 0);
    assert.ok(path.at(path.hit + HERO_TIMING.decel).dx < path.at(path.hit).dx, "carries on left while it brakes");
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
