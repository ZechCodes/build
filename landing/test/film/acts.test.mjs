import { describe, it } from "node:test";
import assert from "node:assert/strict";
import {
  ACTS,
  SCREEN_CUES,
  TOTAL_TRAVEL,
  actAt,
  actStart,
  at,
  entrancePose,
  fullPose,
  span,
} from "../../src/film/acts.js";
import { createScreenResolver, cueAt, upcomingCues } from "../../src/film/cues.js";

describe("the film's clock", () => {
  it("runs eight acts for about 820 viewport heights, in order", () => {
    assert.equal(ACTS.length, 8);
    assert.deepEqual(ACTS.map((act) => act.id), [1, 2, 3, 4, 5, 6, 7, 8]);
    assert.equal(TOTAL_TRAVEL, 820);
    assert.equal(actStart(1), 0);
    assert.equal(actStart(2), 100);
    assert.equal(actStart(8), 730);
  });

  it("places a local beat inside its act", () => {
    assert.equal(at(1, 0.5), 50);
    assert.equal(at(3, 0.2), 180);
    assert.equal(span(2, 0.25, 0.45), 12);
  });

  it("reads an act and local progress back from a position", () => {
    assert.deepEqual(actAt(0), { act: 1, local: 0 });
    assert.deepEqual(actAt(150), { act: 2, local: 5 / 6 });
    assert.deepEqual(actAt(820), { act: 8, local: 1 });
    assert.deepEqual(actAt(-5), { act: 1, local: 0 });
  });

  it("orders each device's display cues by time", () => {
    for (const cues of Object.values(SCREEN_CUES)) {
      const times = cues.map(([time]) => time);
      assert.deepEqual(times, [...times].sort((a, b) => a - b));
    }
  });
});

describe("poses", () => {
  it("fills every field so a tween has a target for each", () => {
    const pose = fullPose({ x: 65, y: 57, w: 52, pitch: 4 });
    assert.deepEqual(pose, { x: 65, y: 57, w: 52, yaw: 0, pitch: 4, roll: 0, opacity: 1, lidOpen: 1, faceCamera: 1 });
    assert.equal(fullPose({}).opacity, 0);
  });

  it("starts a remote screen offset, smaller, turned and transparent", () => {
    const settled = { x: 83, y: 56, w: 21, yaw: -12, pitch: 2, roll: -2 };
    const entrance = entrancePose("phone", settled);
    assert.equal(entrance.x, 96);
    assert.equal(entrance.y, 66);
    assert.ok(Math.abs(entrance.w - 15.12) < 1e-9);
    assert.equal(entrance.yaw, 16);
    assert.equal(entrance.opacity, 0);
  });

  it("only fades a device with no entrance", () => {
    assert.deepEqual(entrancePose("laptop", { x: 1, y: 2, w: 3 }), { x: 1, y: 2, w: 3, opacity: 0 });
  });
});

describe("cues", () => {
  const cues = [[10, "a"], [20, "b"], [30, "c"]];

  it("resolves to the last cue at or before the time, or nothing", () => {
    assert.equal(cueAt(cues, 5), null);
    assert.equal(cueAt(cues, 10), "a");
    assert.equal(cueAt(cues, 25), "b");
    assert.equal(cueAt(cues, 99), "c");
  });

  it("lists what is coming within the lookahead", () => {
    assert.deepEqual(upcomingCues(cues, 12, 10), ["b"]);
    assert.deepEqual(upcomingCues(cues, 12, 30), ["b", "c"]);
  });

  it("applies a display once per change, in either scroll direction", () => {
    const applied = [];
    const resolve = createScreenResolver({ laptop: cues }, (device, value) => applied.push(`${device}:${value}`));
    resolve(12); resolve(15); resolve(25); resolve(12); resolve(5);
    assert.deepEqual(applied, ["laptop:a", "laptop:b", "laptop:a"]);
  });
});
