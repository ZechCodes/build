// The lab's chaotic variants are drawn pill by pill: a pose for every moment,
// sampled into one Web Animation of transform and opacity per pill, and a
// drift on the pill's own translate.
import assert from "node:assert/strict";
import { describe, it } from "node:test";
import { element } from "../hero/fake-animation.mjs";
import { poseFrame, posedMotion } from "../../src/lab/posed.js";

const still = () => ({ x: 0, y: 0, scale: 1, rotate: 0, opacity: 1 });

describe("a pose as a keyframe", () => {
  it("is a transform and an opacity, rounded", () => {
    assert.deepEqual(poseFrame({ x: 1.234, y: -5, scale: 1.23456, rotate: 3.333, opacity: 0.12345 }), {
      transform: "translate(1.2px, -5px) rotate(3.33deg) scale(1.235)",
      opacity: "0.123",
    });
  });
});

describe("a posed variant", () => {
  it("samples each pill's pose up to its last moment and holds it there", () => {
    const pill = { element: element(), pose: (t) => ({ ...still(), x: t * 100 }), until: 0.5 };
    posedMotion({ pills: [pill], end: 2 });
    const [motion] = pill.element.animations;
    assert.equal(motion.options.duration, 500);
    assert.equal(motion.options.fill, "both");
    assert.equal(motion.keyframes[0].transform, "translate(0px, 0px) rotate(0deg) scale(1)");
    assert.equal(motion.keyframes.at(-1).transform, "translate(50px, 0px) rotate(0deg) scale(1)");
    assert.ok(motion.keyframes.length >= 30, "at least 60 a second");
    for (const frame of motion.keyframes) assert.deepEqual(Object.keys(frame).sort(), ["offset", "opacity", "transform"]);
  });

  it("drifts a pill on its translate, apart from its pose", () => {
    const pill = { element: element(), pose: still, until: 2, drift: { speed: -50 } };
    posedMotion({ pills: [pill], end: 2 });
    const drift = pill.element.animations.find((animation) => "translate" in animation.keyframes[0]);
    assert.deepEqual(drift.keyframes.map((frame) => frame.translate), ["0px 0px", "-100px 0px"]);
    assert.equal(drift.options.duration, 2000);
  });

  it("loops a pose and a drift endlessly over their periods", () => {
    const pill = { element: element(), pose: still, drift: { from: 10, to: -90, duration: 4000, iterationStart: 0.25 } };
    posedMotion({ pills: [pill], end: Infinity, period: 3 });
    const [pose, drift] = pill.element.animations;
    assert.equal(pose.options.duration, 3000);
    assert.equal(pose.options.iterations, Infinity);
    assert.deepEqual(drift.keyframes.map((frame) => frame.translate), ["10px 0px", "-90px 0px"]);
    assert.equal(drift.options.iterationStart, 0.25);
    assert.equal(drift.options.iterations, Infinity);
  });

  it("stacks pills in the order asked, and hides the ones it leaves out", () => {
    const pill = { element: element(), pose: still, until: 1, z: 7 };
    const unused = element();
    posedMotion({ pills: [pill], hidden: [unused], end: 1 });
    assert.equal(pill.element.style.zIndex, "7");
    assert.equal(unused.style.visibility, "hidden");
  });

  it("seeks every animation to the clock at its rate", () => {
    const pill = { element: element(), pose: still, until: 1, drift: { speed: 10 } };
    const motion = posedMotion({ pills: [pill], end: 1 });
    motion.update(0.4, false, 0.5);
    for (const animation of pill.element.animations) {
      assert.equal(animation.currentTime, 400);
      assert.equal(animation.playbackRate, 0.5);
      assert.equal(animation.playState, "paused");
    }
    assert.equal(motion.end, 1);
  });
});

describe("a posed variant's held pills", () => {
  it("leaves an animation held at its end alone until the clock goes back", () => {
    const pill = { element: element(), pose: still, until: 0.5 };
    const motion = posedMotion({ pills: [pill], end: 2 });
    const [animation] = pill.element.animations;
    motion.update(1, true, 1);
    assert.equal(animation.currentTime, 500);
    animation.currentTime = 123;
    motion.update(1.1, true, 1);
    assert.equal(animation.currentTime, 123, "not seeked again");
    motion.update(0.2, false, 1);
    assert.equal(animation.currentTime, 200);
  });
});

describe("a posed variant on the compositor", () => {
  it("builds every animation held, and leaves one alone while the compositor starts it", () => {
    const pill = { element: element(), pose: still, until: 1 };
    const motion = posedMotion({ pills: [pill], end: 1 });
    const [animation] = pill.element.animations;
    assert.equal(animation.playState, "paused");
    motion.update(0.1, true, 1);
    assert.equal(animation.playState, "running");
    animation.pending = true;
    motion.update(0.5, true, 1);
    assert.equal(animation.currentTime, 100, "not seeked while pending");
    animation.pending = false;
    motion.update(0.5, true, 1);
    assert.equal(animation.currentTime, 500);
  });
});

describe("a posed variant's finished pills", () => {
  it("leaves one that ran out on its own as it is", () => {
    const pill = { element: element(), pose: still, until: 0.5 };
    const motion = posedMotion({ pills: [pill], end: 2 });
    const [animation] = pill.element.animations;
    motion.update(0.1, true, 1);
    animation.run(400);
    assert.equal(animation.playState, "finished");
    motion.update(0.6, true, 1);
    assert.equal(animation.state, "running", "not paused by hand");
  });
});
