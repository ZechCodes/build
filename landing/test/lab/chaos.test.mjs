// The lab's chaotic variants: each draws every pill it moves as one sampled
// animation of transform and opacity (and a translate for a lane's drift),
// ends as A does, and in endless mode loops without ever resolving.
import assert from "node:assert/strict";
import { describe, it } from "node:test";
import { element } from "../hero/fake-animation.mjs";
import { ATTENTION } from "../../src/hero/notifications.js";
import { burstFlood, crescendoFlood, shockwaveFlood } from "../../src/lab/chaos.js";
import { stageFor } from "../../src/lab/stage.js";

const WIDTH = 1440;
const HEIGHT = 900;

function fieldOf() {
  const lanes = Array.from({ length: 8 }, (_, row) => ({
    element: element(),
    x: 0,
    speed: (row % 2 ? -1 : 1) * (100 + row * 30),
    shown: true,
    pills: Array.from({ length: 14 }, (_, index) => ({ element: element(), attention: null, x: -300 + index * 160, y: 60 + row * 110, width: 140, shown: true, opacity: 0.6 })),
  }));
  ATTENTION.forEach((entry, index) => lanes[index * 2 + 1].pills.push({ element: element(), attention: entry.id, x: 500 + index * 200, y: 170 + index * 220, width: 180, shown: true, opacity: 1 }));
  return { width: WIDTH, height: HEIGHT, lanes };
}

const stage = stageFor(false, { width: WIDTH, height: HEIGHT });
const VARIANTS = { burst: burstFlood, shockwave: shockwaveFlood, crescendo: crescendoFlood };

const posesOf = (pill) => pill.element.animations.find((animation) => "transform" in animation.keyframes[0]);
const scaleOf = (frame) => Number(/scale\(([^)]+)\)/.exec(frame.transform)[1]);
const translateOf = (frame) => /translate\(([-\d.]+)px, ([-\d.]+)px\)/.exec(frame.transform).slice(1).map(Number);

for (const [name, flood] of Object.entries(VARIANTS)) {
  describe(`variant ${name}`, () => {
    it("animates transform and opacity only", () => {
      const field = fieldOf();
      flood({ field, stage });
      const pills = field.lanes.flatMap((lane) => lane.pills);
      for (const pill of pills) {
        for (const animation of pill.element.animations) {
          for (const frame of animation.keyframes) {
            for (const key of Object.keys(frame)) assert.ok(["offset", "transform", "opacity", "translate"].includes(key), key);
          }
        }
      }
      assert.ok(field.lanes.every((lane) => lane.element.animations.length === 0), "pills move, never their lanes");
    });

    it("clears the field when Build takes over and lands the requests on their rows", () => {
      const field = fieldOf();
      const motion = flood({ field, stage });
      assert.equal(motion.end, stage.timing.settle[1]);
      const pills = field.lanes.flatMap((lane) => lane.pills).filter((pill) => posesOf(pill));
      assert.ok(pills.length > 40, "the field is full");
      for (const pill of pills) {
        const last = posesOf(pill).keyframes.at(-1);
        assert.equal(last.opacity, "0", `${pill.attention || "routine"} pill gone`);
        assert.ok(posesOf(pill).options.duration <= stage.timing.settle[1] * 1000);
      }
      for (const [index, entry] of ATTENTION.entries()) {
        const request = pills.find((pill) => pill.attention === entry.id);
        const last = posesOf(request).keyframes.at(-1);
        assert.equal(scaleOf(last), 0.6);
        const [dx, dy] = translateOf(last);
        assert.ok(Math.abs(request.x + dx - stage.rows[index][0]) < 0.2 && Math.abs(request.y + dy - stage.rows[index][1]) < 0.2, "on its row");
        assert.equal(posesOf(request).options.duration, stage.timing.landings[index] * 1000);
      }
    });

    it("never resolves when endless: every pose loops back onto itself", () => {
      const field = fieldOf();
      const motion = flood({ field, stage }, { endless: true });
      assert.equal(motion.end, Infinity);
      const pills = field.lanes.flatMap((lane) => lane.pills).filter((pill) => posesOf(pill));
      assert.ok(pills.length > 40);
      for (const pill of pills) {
        const poses = posesOf(pill);
        assert.equal(poses.options.iterations, Infinity);
        const [first, last] = [poses.keyframes[0], poses.keyframes.at(-1)];
        const [[x0, y0], [x1, y1]] = [translateOf(first), translateOf(last)];
        assert.ok(Math.abs(x0 - x1) < 1.5 && Math.abs(y0 - y1) < 1.5, `${name}: ${first.transform} vs ${last.transform}`);
        assert.ok(Math.abs(scaleOf(first) - scaleOf(last)) < 0.02 && Math.abs(first.opacity - last.opacity) < 0.02);
      }
    });
  });
}

describe("variant B, burst", () => {
  it("flies late arrivals in from beyond an edge and stacks each on those before", () => {
    const field = fieldOf();
    burstFlood({ field, stage });
    const moved = field.lanes.flatMap((lane) => lane.pills).filter((pill) => !pill.attention && posesOf(pill));
    const outside = moved.filter((pill) => {
      const [dx, dy] = translateOf(posesOf(pill).keyframes[0]);
      const [x, y] = [pill.x + dx, pill.y + dy];
      return x < 0 || x > WIDTH || y < 0 || y > HEIGHT;
    });
    assert.ok(outside.length > moved.length / 2, "most come from off the field");
    const stacks = moved.map((pill) => Number(pill.element.style.zIndex));
    assert.equal(new Set(stacks).size, stacks.length, "each lands on its own level");
  });
});

describe("variant C, shockwave", () => {
  it("drifts the lanes as A does and jolts the pills as waves pass", () => {
    const field = fieldOf();
    shockwaveFlood({ field, stage });
    const pill = field.lanes[0].pills[6];
    const drift = pill.element.animations.find((animation) => "translate" in animation.keyframes[0]);
    assert.ok(drift, "a drift on its translate");
    const moves = posesOf(pill).keyframes.filter((frame) => frame.offset * posesOf(pill).options.duration < stage.timing.ripple[0] * 1000).map(translateOf);
    assert.ok(moves.some(([x, y]) => Math.hypot(x, y) > 8), "shoved by a wave before the takeover");
  });
});

describe("variant D, crescendo", () => {
  it("shakes harder as it rises", () => {
    const field = fieldOf();
    crescendoFlood({ field, stage });
    const pill = field.lanes.flatMap((lane) => lane.pills).find((candidate) => !candidate.attention && posesOf(candidate) && posesOf(candidate).keyframes[0].opacity !== "0");
    const { keyframes, options } = posesOf(pill);
    const at = (seconds) => keyframes.filter((frame) => Math.abs(frame.offset * options.duration - seconds * 1000) < 120).map((frame) => Number(/rotate\(([-\d.]+)deg/.exec(frame.transform)[1]));
    const spread = (values) => Math.max(...values) - Math.min(...values);
    assert.ok(spread(at(0.95)) > spread(at(0.12)) * 2, "a wider wobble near the peak");
  });
});
