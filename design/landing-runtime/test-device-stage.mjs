import assert from "node:assert/strict";
import test from "node:test";
import {
  getDeviceFramePoses,
  getDeviceScreenSource,
} from "../../skriftapp/buildapp/landing/device-stage.js";

const hidden = { x: 50, y: 50, w: 0, rotate: [0, 0, 0], opacity: 0 };
const pose = (x, y, w, opacity = 1) => ({ x, y, w, rotate: [0, 0, 0], opacity });
const scene = (laptop, tablet, phone, review = hidden) => ({
  id: "scene",
  poses: { desktop: { laptop, tablet, phone, review } },
});
const scenes = [
  scene(pose(65, 60, 58), hidden, hidden),
  scene(pose(28, 60, 34), hidden, pose(69, 59, 24)),
  scene(hidden, hidden, pose(68, 56, 28)),
  scene(hidden, pose(62, 59, 64), hidden),
  scene(hidden, hidden, hidden, pose(50, 60, 82)),
  scene(pose(30, 68, 34), pose(53, 69, 31), pose(73, 70, 12)),
];

function frame(sceneIndex, local) {
  return { sceneIndex, local, profile: "desktop" };
}

test("the hero begins present and settles forward", () => {
  assert.equal(getDeviceFramePoses(scenes, frame(0, 0)).laptop.w, 53);
  assert.equal(getDeviceFramePoses(scenes, frame(0, 0.25)).laptop.w, 58);
});

test("adjacent scenes share the same boundary pose", () => {
  for (let sceneIndex = 0; sceneIndex < scenes.length - 1; sceneIndex += 1) {
    const outgoing = getDeviceFramePoses(scenes, frame(sceneIndex, 1));
    const incoming = getDeviceFramePoses(scenes, frame(sceneIndex + 1, 0));
    assert.deepEqual(incoming, outgoing);
  }
});

test("review aligns the tablet before fading its hardware", () => {
  const aligned = getDeviceFramePoses(scenes, frame(4, 0)).tablet;
  assert.deepEqual(aligned, pose(50, 60, 82 * 0.252 / 0.234));
  assert.equal(getDeviceFramePoses(scenes, frame(4, 0.1)).tablet.w, aligned.w);
  assert.equal(getDeviceFramePoses(scenes, frame(4, 0.2)).tablet.opacity, 0);
  assert.equal(getDeviceFramePoses(scenes, frame(4, 0.85)).tablet.opacity, 0);
});

test("review width uses the physical tablet screen inset", () => {
  const reviewWidth = 55;
  const options = { reviewWidth };
  const outgoing = getDeviceFramePoses(scenes, frame(3, 1), options).tablet;
  const incoming = getDeviceFramePoses(scenes, frame(4, 0), options).tablet;
  assert.deepEqual(incoming, outgoing);
  assert.equal(incoming.w * 0.234 / 0.252, reviewWidth);
});

test("screen checkpoints resolve canonical maps", () => {
  assert.match(getDeviceScreenSource("phone", { sceneIndex: 2, checkpoint: "question" }), /ui03-question-mobile/);
  assert.match(getDeviceScreenSource("phone", { sceneIndex: 2, checkpoint: "answer" }), /ui03-answer-mobile/);
  assert.match(getDeviceScreenSource("phone", { sceneIndex: 2, checkpoint: "resumed" }), /ui03-resumed-mobile/);
  assert.match(getDeviceScreenSource("laptop", { sceneIndex: 5 }), /ui05-merged-desktop/);
});
