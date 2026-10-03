// Variant E, the wall (#316): pills cycling everywhere, then the three
// requests popping up fresh, turning green and flying to the laptop's rows
// while the wall blurs out. Endless, the wall only cycles.
import assert from "node:assert/strict";
import { describe, it } from "node:test";
import { fakeDocument } from "./fake-dom.mjs";
import { ATTENTION } from "../../src/hero/notifications.js";
import { stageFor } from "../../src/lab/stage.js";
import { wallFlood } from "../../src/lab/wall-variant.js";

const WIDTH = 1440;
const HEIGHT = 900;

function build(options = {}) {
  const document = fakeDocument();
  const root = document.createElement("div");
  const stage = stageFor(false, { width: WIDTH, height: HEIGHT });
  const flood = wallFlood({ field: { width: WIDTH, height: HEIGHT, lanes: [] }, stage, root, options: { shape: "dome", blur: "copy", ...options } });
  return { root, flood, stage };
}

const byClass = (root, name) => root.all().filter((element) => element.className.split(" ").includes(name));
const opacityAt = (animation, ms) => {
  const frames = animation.keyframes;
  const offset = ms / animation.options.duration;
  const after = frames.findIndex((frame) => frame.offset > offset);
  return Number((after === -1 ? frames.at(-1) : frames[Math.max(0, after - 1)]).opacity);
};

describe("variant E, the wall", () => {
  it("draws the wall in its own element and takes it away again", () => {
    const { root, flood } = build();
    const pills = byClass(root, "hero-wall__pill");
    assert.ok(pills.length > 20);
    flood.dispose();
    assert.equal(root.children.length, 0);
  });

  it("masks only the wall: the requests fly out of the dome", () => {
    const { root } = build();
    const [wall] = byClass(root, "hero-wall");
    assert.equal(wall.dataset.shape, "dome");
    for (const name of ["--rx", "--ry", "--inner", "--fade"]) assert.ok(wall.style.properties[name], name);
    const requests = byClass(root, "hero-wall__request");
    assert.equal(requests.length, ATTENTION.length);
    for (const request of requests) assert.ok(!wall.all().includes(request));
  });

  it("is a full field when asked, unmasked", () => {
    const { root } = build({ shape: "full" });
    const [wall] = byClass(root, "hero-wall");
    assert.equal(wall.dataset.shape, "full");
    assert.ok(byClass(root, "hero-wall__pill").length > byClass(build().root, "hero-wall__pill").length * 2);
  });

  it("blurs with a copy, or with the filter alone", () => {
    assert.ok(byClass(build().root, "hero-wall__blur").length > 0);
    const { root } = build({ blur: "filter" });
    assert.equal(byClass(root, "hero-wall__blur").length, 0);
    assert.ok(byClass(root, "hero-wall__pill").some((pill) => pill.animations[0].keyframes.some((frame) => "filter" in frame)));
  });

  it("has blurred the wall out by the time the requests land", () => {
    const { root, stage } = build();
    const last = stage.timing.landings.at(-1) * 1000;
    for (const pill of byClass(root, "hero-wall__pill")) assert.equal(opacityAt(pill.animations[0], last), 0);
  });

  it("pops each request up fresh just before it takes off, turns it green and lands it on its row", () => {
    const { root, stage, flood } = build();
    const requests = byClass(root, "hero-wall__request");
    requests.forEach((request, index) => {
      const landing = stage.timing.landings[index];
      const takeOff = landing - stage.timing.flight;
      const [motion] = request.animations;
      const at = (seconds) => motion.keyframes[Math.round((seconds / landing) * (motion.keyframes.length - 1))];
      assert.equal(Number(at(0).opacity), 0, "not there before");
      assert.equal(Number(at(takeOff - 0.6).opacity), 0, "not sitting there");
      assert.ok(Number(at(takeOff - 0.05).opacity) > 0.9, "popped up");
      const scales = motion.keyframes.map((frame) => Number(/scale\(([^)]+)\)/.exec(frame.transform)[1]));
      assert.ok(Math.max(...scales) > 1.05, "with an overshoot");
      const [green] = byClass(request, "hero-wall__green");
      const glow = green.animations[0].keyframes;
      const greenAt = (seconds) => Number(glow[Math.round((seconds / landing) * (glow.length - 1))].opacity);
      assert.equal(greenAt(takeOff - 0.45), 0);
      assert.ok(greenAt(takeOff) > 0.9, "green before it flies");
    });
    assert.equal(flood.end, stage.timing.settle[1]);
  });

  it("only cycles when endless: no requests, no exit, every animation looping", () => {
    const document = fakeDocument();
    const root = document.createElement("div");
    const stage = stageFor(false, { width: WIDTH, height: HEIGHT });
    const flood = wallFlood({ field: { width: WIDTH, height: HEIGHT, lanes: [] }, stage, root, options: { shape: "dome", blur: "copy" } }, { endless: true });
    assert.equal(flood.end, Infinity);
    assert.equal(byClass(root, "hero-wall__request").length, 0);
    for (const pill of byClass(root, "hero-wall__pill")) assert.equal(pill.animations[0].options.iterations, Infinity);
  });

  it("shows where the headline and the laptop stand, for judging the dome and the flight", () => {
    const { root } = build();
    assert.equal(byClass(root, "lab-standin").length, 2);
  });
});
